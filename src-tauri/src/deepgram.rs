use tokio::sync::watch;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures_util::{StreamExt, SinkExt, stream::SplitSink};
use serde_json::Value;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tauri::Emitter;
use std::collections::VecDeque;

// ── VAD constants ────────────────────────────────────────────────────────────
const VAD_THRESHOLD: f32 = 0.015;
const SPEECH_TRIGGER_COUNT: u32 = 3;     // 3 × ~40ms = ~120ms of speech to open DG
const SILENCE_CLOSE_COUNT: u32 = 37;     // 37 × ~40ms ≈ 1.5s of silence to close DG
const PRE_BUFFER_SIZE: usize = 8;        // ~320ms of audio kept before speech onset

#[derive(Debug, Clone, Copy, PartialEq)]
enum StreamState {
    Listening,
    Streaming,
}

/// Compute root-mean-square of a sample buffer (energy-based VAD).
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Open a fresh Deepgram WebSocket connection.
/// Extracted as a helper so it can be called once per speech segment.
/// The receive task parses Deepgram JSON and forwards final transcripts.
async fn open_deepgram_ws(
    api_key: &str,
    target_sample_rate: u32,
    transcript_tx: tokio::sync::broadcast::Sender<String>,
) -> Result<
    (
        SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, Message>,
        tokio::task::JoinHandle<()>,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let url = format!(
        "wss://api.deepgram.com/v1/listen?encoding=linear16&sample_rate={}&channels=2&multichannel=true&model=nova-2&smart_format=true",
        target_sample_rate
    );
    println!("VAD: Opening fresh Deepgram WS ({}Hz)", target_sample_rate);

    let mut request = url.into_client_request()?;
    request.headers_mut().insert(
        "Authorization",
        format!("Token {}", api_key).parse()?,
    );

    let (ws_stream, _) = connect_async(request).await?;
    let (write, read) = ws_stream.split();

    // Spawn a receive task that parses Deepgram transcripts
    let receive_task = tokio::spawn(async move {
        let mut read = read;
        let mut last_transcript = String::new();

        while let Some(message) = read.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    if let Ok(json) = serde_json::from_str::<Value>(&text) {
                        if let Some(alternatives) = json["channel"]["alternatives"].as_array() {
                            for alt in alternatives {
                                let transcript = alt["transcript"].as_str().unwrap_or("").trim().to_string();
                                let is_final = json["is_final"].as_bool().unwrap_or(false);
                                let speech_final = json["speech_final"].as_bool().unwrap_or(false);

                                let speaker = if let Some(arr) = json["channel_index"].as_array() {
                                    arr.first().and_then(|v| v.as_i64()).unwrap_or(0) as i32
                                } else if let Some(idx) = json["channel_index"].as_i64() {
                                    idx as i32
                                } else if let Some(arr) = json["metadata"]["channel_index"].as_array() {
                                    arr.first().and_then(|v| v.as_i64()).unwrap_or(0) as i32
                                } else {
                                    json["metadata"]["channel_index"].as_i64().unwrap_or(0) as i32
                                };

                                if is_final && !transcript.is_empty() && transcript != last_transcript {
                                    println!("FINAL [Speaker {}][speech_final={}]: {}", speaker, speech_final, transcript);
                                    let payload = serde_json::json!({
                                        "text": transcript,
                                        "speaker": speaker,
                                        "speechFinal": speech_final
                                    }).to_string();
                                    let _ = transcript_tx.send(payload);
                                    last_transcript = transcript;
                                }
                            }
                        }
                    }
                }
                Ok(Message::Close(frame)) => {
                    println!("VAD: Deepgram WS closed by server: {:?}", frame);
                    break;
                }
                Err(e) => {
                    eprintln!("VAD: Error receiving from Deepgram: {}", e);
                    break;
                }
                _ => {}
            }
        }
    });

    Ok((write, receive_task))
}

/// Public entry point. Loops indefinitely, waiting for a "start" command from the
/// web app before activating VAD-gated streaming to Deepgram.
/// When "start" is received, audio capture runs locally with VAD monitoring.
/// Deepgram connections are only opened when speech is detected.
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

        println!("Deepgram: start command received, activating VAD listener...");

        // Drain stale audio that accumulated while idle
        while mic_rx.try_recv().is_ok() {}
        while sys_rx.try_recv().is_ok() {}

        if let Err(e) = vad_gated_stream(
            window.clone(),
            &mut mic_rx,
            &mut sys_rx,
            &transcript_tx,
            &api_key,
            target_sample_rate,
            source_sys_sample_rate,
            &mut cmd_rx,
        ).await {
            eprintln!("Deepgram VAD stream ended with error: {}", e);
        }

        // Emit idle state when loop returns (stopped/paused)
        let _ = window.emit("deepgram_state", serde_json::json!({"state": "idle"}));

        println!("Deepgram: VAD stream stopped, waiting for next start command...");
    }
}

/// VAD-gated streaming session. Runs while recording is active (start command).
/// Opens/closes Deepgram connections dynamically based on speech detection.
async fn vad_gated_stream(
    window: tauri::Window,
    mic_rx: &mut tokio::sync::broadcast::Receiver<Vec<f32>>,
    sys_rx: &mut tokio::sync::broadcast::Receiver<Vec<f32>>,
    transcript_tx: &tokio::sync::broadcast::Sender<String>,
    api_key: &str,
    target_sample_rate: u32,
    source_sys_sample_rate: u32,
    cmd_rx: &mut watch::Receiver<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut mic_buffer: VecDeque<f32> = VecDeque::new();
    let mut sys_buffer: VecDeque<f32> = VecDeque::new();

    // Resampling state for system audio
    let mut sys_accumulator = 0.0f32;
    let ratio = source_sys_sample_rate as f32 / target_sample_rate as f32;

    // VAD state
    let mut stream_state = StreamState::Listening;
    let mut vad_positive_count: u32 = 0;
    let mut silence_chunk_count: u32 = 0;

    // Pre-speech ring buffer: stores encoded PCM chunks before speech onset
    let mut pre_buffer: VecDeque<Vec<u8>> = VecDeque::with_capacity(PRE_BUFFER_SIZE);

    // Active Deepgram connection (only Some while streaming)
    let mut dg_write: Option<SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, Message>> = None;
    let mut dg_receive_task: Option<tokio::task::JoinHandle<()>> = None;

    // Metrics
    let mut loop_count = 0u64;
    let mut mic_peak = 0.0f32;
    let mut sys_peak = 0.0f32;

    loop {
        loop_count += 1;
        if loop_count % 100 == 0 {
            if mic_peak > 0.001 || sys_peak > 0.001 || mic_buffer.len() > 1000 {
                println!("VAD Status | State: {:?} | Buffers: Mic={}, Sys={} | Peak: Mic={:.4}, Sys={:.4}",
                    stream_state, mic_buffer.len(), sys_buffer.len(), mic_peak, sys_peak);
            }
            mic_peak = 0.0;
            sys_peak = 0.0;
        }

        // Emit audio levels for frontend visualizer
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

                            // Build interleaved PCM for this chunk
                            let mut interleaved_f32 = Vec::with_capacity(actual_size);
                            let mut interleaved_data = Vec::with_capacity(actual_size * 2 * 2);

                            for _ in 0..actual_size {
                                let mic_sample = mic_buffer.pop_front().unwrap_or(0.0f32);
                                mic_peak = mic_peak.max(mic_sample.abs());

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
                                        sys_sample = (sys_sum / count as f32) * 1.5;
                                    }
                                } else {
                                    sys_accumulator = 0.0;
                                }

                                sys_peak = sys_peak.max(sys_sample.abs());
                                interleaved_f32.push(mic_sample);
                                interleaved_f32.push(sys_sample);

                                let mic_i16 = (mic_sample.max(-1.0).min(1.0) * 32767.0) as i16;
                                let sys_i16 = (sys_sample.max(-1.0).min(1.0) * 32767.0) as i16;

                                interleaved_data.extend_from_slice(&mic_i16.to_le_bytes());
                                interleaved_data.extend_from_slice(&sys_i16.to_le_bytes());
                            }

                            if interleaved_data.is_empty() {
                                continue;
                            }

                            // ── VAD decision ──────────────────────────────────
                            let level = rms(&interleaved_f32);
                            let is_speech = level > VAD_THRESHOLD;

                            match stream_state {
                                StreamState::Listening => {
                                    // Store in pre-buffer ring
                                    pre_buffer.push_back(interleaved_data.clone());
                                    if pre_buffer.len() > PRE_BUFFER_SIZE {
                                        pre_buffer.pop_front();
                                    }

                                    if is_speech {
                                        vad_positive_count += 1;
                                        if vad_positive_count >= SPEECH_TRIGGER_COUNT {
                                            // ── Transition to Streaming ────────
                                            println!("VAD: Speech detected (RMS={:.4}), opening Deepgram WS", level);

                                            match open_deepgram_ws(api_key, target_sample_rate, transcript_tx.clone()).await {
                                                Ok((write, recv_task)) => {
                                                    dg_write = Some(write);
                                                    dg_receive_task = Some(recv_task);

                                                    // Flush pre-buffer
                                                    if let Some(ref mut writer) = dg_write {
                                                        for chunk in pre_buffer.drain(..) {
                                                            if let Err(e) = writer.send(Message::Binary(chunk.into())).await {
                                                                eprintln!("VAD: Error flushing pre-buffer: {}", e);
                                                                dg_write = None;
                                                                if let Some(task) = dg_receive_task.take() { task.abort(); }
                                                                stream_state = StreamState::Listening;
                                                                vad_positive_count = 0;
                                                                let _ = window.emit("deepgram_state", serde_json::json!({"state": "idle"}));
                                                                break;
                                                            }
                                                        }
                                                    }

                                                    stream_state = StreamState::Streaming;
                                                    silence_chunk_count = 0;
                                                    let _ = window.emit("deepgram_state", serde_json::json!({"state": "streaming"}));
                                                }
                                                Err(e) => {
                                                    eprintln!("VAD: Failed to open Deepgram WS: {}", e);
                                                    // Stay in Listening, will retry on next speech
                                                    vad_positive_count = 0;
                                                }
                                            }
                                        }
                                    } else {
                                        vad_positive_count = 0;
                                    }
                                }
                                StreamState::Streaming => {
                                    // Send audio to Deepgram
                                    if let Some(ref mut writer) = dg_write {
                                        if let Err(e) = writer.send(Message::Binary(interleaved_data.into())).await {
                                            eprintln!("VAD: Deepgram send error (recovering): {}", e);
                                            // Recover: close gracefully and return to Listening
                                            dg_write = None;
                                            if let Some(task) = dg_receive_task.take() { task.abort(); }
                                            stream_state = StreamState::Listening;
                                            vad_positive_count = 0;
                                            silence_chunk_count = 0;
                                            let _ = window.emit("deepgram_state", serde_json::json!({"state": "idle"}));
                                            continue;
                                        }
                                    }

                                    if is_speech {
                                        silence_chunk_count = 0;
                                    } else {
                                        silence_chunk_count += 1;
                                        if silence_chunk_count >= SILENCE_CLOSE_COUNT {
                                            // ── Silence timeout: close DG, return to Listening ──
                                            println!("VAD: Silence timeout ({}ms), closing Deepgram WS",
                                                silence_chunk_count * 40);

                                            if let Some(ref mut writer) = dg_write {
                                                let _ = writer.send(Message::Text(
                                                    r#"{"type":"CloseStream"}"#.into()
                                                )).await;
                                                let _ = writer.close().await;
                                            }
                                            dg_write = None;
                                            if let Some(task) = dg_receive_task.take() { task.abort(); }

                                            stream_state = StreamState::Listening;
                                            vad_positive_count = 0;
                                            let _ = window.emit("deepgram_state", serde_json::json!({"state": "idle"}));
                                        }
                                    }
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
                    println!("VAD: stopping due to '{}' command", cmd);
                    // Close any active Deepgram WS
                    if let Some(ref mut writer) = dg_write {
                        let _ = writer.send(Message::Close(None)).await;
                    }
                    dg_write = None;
                    if let Some(task) = dg_receive_task.take() { task.abort(); }
                    let _ = window.emit("deepgram_state", serde_json::json!({"state": "idle"}));
                    return Ok(());
                }
            }
        }

        // Safety valve: prevent buffer runaway
        if mic_buffer.len() > 32000 { mic_buffer.drain(..16000); }
        if sys_buffer.len() > 64000 { sys_buffer.drain(..32000); }
    }

    // Cleanup
    if let Some(ref mut writer) = dg_write {
        let _ = writer.send(Message::Close(None)).await;
    }
    if let Some(task) = dg_receive_task.take() { task.abort(); }

    Ok(())
}
