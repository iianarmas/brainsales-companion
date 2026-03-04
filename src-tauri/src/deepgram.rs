use tokio::sync::watch;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures_util::{StreamExt, SinkExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tauri::Emitter;

/// Public entry point. Loops indefinitely, waiting for a "start" command from the
/// web app before connecting to Deepgram. Stops the Deepgram connection when a
/// "stop" or "pause" command is received, then waits for the next "start".
pub async fn run_deepgram_loop(
    window: tauri::Window,
    mut mic_rx: tokio::sync::broadcast::Receiver<Vec<f32>>,
    mut sys_rx: tokio::sync::broadcast::Receiver<Vec<f32>>,
    transcript_tx: tokio::sync::broadcast::Sender<String>,
    api_key: String,
    target_sample_rate: u32,
    source_sys_sample_rate: u32,
    mut cmd_rx: watch::Receiver<String>,
) {
    loop {
        // Wait until the web app sends a "start" command
        while cmd_rx.borrow().as_str() != "start" {
            if cmd_rx.changed().await.is_err() {
                return; // watch channel closed — companion shutting down
            }
        }

        println!("Deepgram: start command received, connecting...");

        // Drain stale audio that accumulated while Deepgram was idle
        while mic_rx.try_recv().is_ok() {}
        while sys_rx.try_recv().is_ok() {}

        if let Err(e) = start_deepgram_stream(
            window.clone(),
            &mut mic_rx,
            &mut sys_rx,
            &transcript_tx,
            &api_key,
            target_sample_rate,
            source_sys_sample_rate,
            &mut cmd_rx,
        ).await {
            eprintln!("Deepgram stream ended with error: {}", e);
        }

        println!("Deepgram: stream stopped, waiting for next start command...");
    }
}

async fn start_deepgram_stream(
    window: tauri::Window,
    mic_rx: &mut tokio::sync::broadcast::Receiver<Vec<f32>>,
    sys_rx: &mut tokio::sync::broadcast::Receiver<Vec<f32>>,
    transcript_tx: &tokio::sync::broadcast::Sender<String>,
    api_key: &str,
    target_sample_rate: u32,
    source_sys_sample_rate: u32,
    cmd_rx: &mut watch::Receiver<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url = format!(
        "wss://api.deepgram.com/v1/listen?encoding=linear16&sample_rate={}&channels=2&multichannel=true&model=nova-2&smart_format=true",
        target_sample_rate
    );
    println!("Connecting to Deepgram with URL: {}", url);

    let mut request = url.into_client_request()?;
    request.headers_mut().insert(
        "Authorization",
        format!("Token {}", api_key).parse()?,
    );

    let (ws_stream, _) = match connect_async(request).await {
        Ok(val) => val,
        Err(e) => {
            eprintln!("Failed to connect to Deepgram: {}. Check your API key and internet connection.", e);
            return Err(Box::new(e));
        }
    };
    println!("WebSocket connected to Deepgram API (Target: {}Hz, 2ch Merged)", target_sample_rate);

    let (mut write, mut read) = ws_stream.split();

    // Task to receive transcripts from Deepgram
    let transcript_tx_clone = transcript_tx.clone();
    let receive_task = tokio::spawn(async move {
        let mut last_transcript = String::new();

        while let Some(message) = read.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    if let Ok(json) = serde_json::from_str::<Value>(&text) {
                        // Log every message from Deepgram for debugging
                        if let Some(_metadata) = json.get("metadata") {
                            let is_final = json["is_final"].as_bool().unwrap_or(false);
                            let channels = json["channel"]["alternatives"].as_array();
                            let transcript_preview = channels.and_then(|c| c.first()).and_then(|a| a["transcript"].as_str()).unwrap_or("");

                            if !transcript_preview.is_empty() {
                                println!("DG DEBUG (final={}): {}", is_final, transcript_preview);
                            }
                        }

                        if let Some(alternatives) = json["channel"]["alternatives"].as_array() {
                            for alt in alternatives {
                                let transcript = alt["transcript"].as_str().unwrap_or("").trim().to_string();
                                let is_final = json["is_final"].as_bool().unwrap_or(false);

                                // In Deepgram multichannel streaming, channel_index is usually in the top-level object
                                // but can also be in metadata. We check both.
                                let speaker = if let Some(arr) = json["channel_index"].as_array() {
                                    arr.first().and_then(|v| v.as_i64()).unwrap_or(0) as i32
                                } else if let Some(idx) = json["channel_index"].as_i64() {
                                    idx as i32
                                } else if let Some(arr) = json["metadata"]["channel_index"].as_array() {
                                    arr.first().and_then(|v| v.as_i64()).unwrap_or(0) as i32
                                } else {
                                    json["metadata"]["channel_index"].as_i64().unwrap_or(0) as i32
                                };

                                if !transcript.is_empty() {
                                    println!("TRANSCRIPT [Spk {}][final={}]: {}", speaker, is_final, transcript);
                                }

                                if is_final && !transcript.is_empty() && transcript != last_transcript {
                                    println!("FINAL [Speaker {}]: {}", speaker, transcript);
                                    let payload = serde_json::json!({
                                        "text": transcript,
                                        "speaker": speaker
                                    }).to_string();
                                    let _ = transcript_tx_clone.send(payload.clone());
                                    last_transcript = transcript;
                                }
                            }
                        }
                    }
                }
                Ok(Message::Close(frame)) => {
                    println!("Deepgram WebSocket closed: {:?}", frame);
                    break;
                }
                Err(e) => {
                    eprintln!("Error receiving from Deepgram: {}", e);
                    break;
                }
                _ => {}
            }
        }
    });

    // Task to send audio data to Deepgram
    let mut mic_buffer: std::collections::VecDeque<f32> = std::collections::VecDeque::new();
    let mut sys_buffer: std::collections::VecDeque<f32> = std::collections::VecDeque::new();

    // Resampling state for system audio
    let mut sys_accumulator = 0.0f32;
    let ratio = source_sys_sample_rate as f32 / target_sample_rate as f32;

    let mut loop_count = 0;
    let mut mic_peak = 0.0f32;
    let mut sys_peak = 0.0f32;

    loop {
        loop_count += 1;
        if loop_count % 100 == 0 {
            // Log if we have content or if we are waiting
            if mic_peak > 0.001 || sys_peak > 0.001 || mic_buffer.len() > 1000 {
                println!("Status | Buffers: Mic={}, Sys={} | Peak: Mic={:.4}, Sys={:.4}",
                    mic_buffer.len(), sys_buffer.len(), mic_peak, sys_peak);
            }
            mic_peak = 0.0;
            sys_peak = 0.0;
        }

        // Emit audio usage to frontend for visualizer
        if loop_count % 10 == 0 {
            let _ = window.emit("audio-levels", serde_json::json!({
                "mic": mic_peak,
                "sys": sys_peak
            }));
        }

        tokio::select! {
            result = mic_rx.recv() => {
                match result {
                    Ok(data) => {
                        mic_buffer.extend(data);

                        while !mic_buffer.is_empty() {
                            let chunk_size = 640;
                            let actual_size = std::cmp::min(chunk_size, mic_buffer.len());

                            let mut interleaved_data = Vec::with_capacity(actual_size * 2 * 2);
                            for _ in 0..actual_size {
                                let mic_sample = mic_buffer.pop_front().unwrap_or(0.0f32);
                                mic_peak = mic_peak.max(mic_sample.abs());

                                // Proper resampling with averaging (Prevent runaway)
                                let mut sys_sample = 0.0f32;
                                if !sys_buffer.is_empty() {
                                    sys_accumulator += ratio;
                                    let mut sys_sum = 0.0f32;
                                    let mut count = 0;
                                    while sys_accumulator >= 1.0 && !sys_buffer.is_empty() {
                                        let s = sys_buffer.pop_front().unwrap_or(0.0f32);
                                        sys_sum += s;
                                        count += 1;
                                        sys_accumulator -= 1.0;
                                    }
                                    if count > 0 {
                                        sys_sample = (sys_sum / count as f32) * 1.5; // 50% gain boost for Sys
                                    }
                                } else {
                                    sys_accumulator = 0.0; // Reset if we fall behind to keep future sync
                                }

                                sys_peak = sys_peak.max(sys_sample.abs());

                                let mic_i16 = (mic_sample.max(-1.0).min(1.0) * 32767.0) as i16;
                                let sys_i16 = (sys_sample.max(-1.0).min(1.0) * 32767.0) as i16;

                                interleaved_data.extend_from_slice(&mic_i16.to_le_bytes());
                                interleaved_data.extend_from_slice(&sys_i16.to_le_bytes());
                            }

                            if !interleaved_data.is_empty() {
                                if let Err(e) = write.send(Message::Binary(interleaved_data.into())).await {
                                    eprintln!("Error sending audio to Deepgram: {}", e);
                                    receive_task.abort();
                                    return Err(Box::new(e));
                                }
                            }
                        }
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        println!("Warning: mic_rx lagged by {} messages", n);
                    }
                    Err(_) => break,
                }
            }
            result = sys_rx.recv() => {
                match result {
                    Ok(data) => {
                        sys_buffer.extend(data);
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        println!("Warning: sys_rx lagged by {} messages", n);
                    }
                    Err(_) => break,
                }
            }
            _ = cmd_rx.changed() => {
                let cmd = cmd_rx.borrow().clone();
                if cmd.as_str() != "start" {
                    println!("Deepgram: stopping stream due to '{}' command", cmd);
                    let _ = write.send(Message::Close(None)).await;
                    receive_task.abort();
                    return Ok(());
                }
            }
        }

        // Safety valve: prevent buffer runaway
        if mic_buffer.len() > 32000 { mic_buffer.drain(..16000); }
        if sys_buffer.len() > 64000 { sys_buffer.drain(..32000); }
    }

    let _ = receive_task.await;
    Ok(())
}
