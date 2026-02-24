pub mod audio;
pub mod deepgram;
pub mod server;

use tokio::sync::broadcast;
use audio::capture::AudioCapture;
use std::sync::Mutex;
use tauri::Manager;

pub struct AppState {
    pub audio_tx: broadcast::Sender<Vec<f32>>,
    pub audio_output_tx: broadcast::Sender<Vec<f32>>,
    pub transcript_tx: broadcast::Sender<String>,
    pub capture: Mutex<AudioCapture>,
    pub is_running: Mutex<bool>,
}

#[tauri::command]
async fn start_companion(window: tauri::Window, state: tauri::State<'_, AppState>) -> Result<String, String> {
    let mut is_running = state.is_running.lock().unwrap();
    if *is_running {
        return Ok("Companion already running".to_string());
    }
    
    let audio_tx = state.audio_tx.clone();
    let audio_rx = audio_tx.subscribe();
    
    let audio_output_tx = state.audio_output_tx.clone();
    let audio_output_rx = audio_output_tx.subscribe();
    
    let transcript_tx = state.transcript_tx.clone();
    let transcript_rx = transcript_tx.subscribe();

    // Start Audio Capture
    let ((mic_sample_rate, _mic_channels), (_sys_sample_rate, _sys_channels)) = match state.capture.lock().unwrap().start(audio_tx, audio_output_tx) {
        Ok(config) => config,
        Err(e) => return Err(format!("Failed to start audio capture: {}", e)),
    };

    // Start Deepgram stream
    let api_key = std::env::var("DEEPGRAM_API_KEY").unwrap_or_else(|_| "PLACEHOLDER".to_string());
    if api_key == "PLACEHOLDER" {
        println!("Warning: DEEPGRAM_API_KEY is not set, using placeholder.");
    }
    
    let transcript_tx_clone = transcript_tx.clone();
    
    // Single Merged Task: Both Mic and System Audio
    tauri::async_runtime::spawn(async move {
        println!("Starting Merged Deepgram task (Mic: {}Hz, Sys: {}Hz)...", mic_sample_rate, _sys_sample_rate);
        if let Err(e) = deepgram::start_deepgram_stream(window, audio_rx, audio_output_rx, transcript_tx_clone, api_key, mic_sample_rate, _sys_sample_rate).await {
            eprintln!("Deepgram task exited with error: {}", e);
        }
    });

    tauri::async_runtime::spawn(async move {
        println!("Starting local WebSocket server task...");
        server::start_local_server(transcript_rx).await;
    });

    *is_running = true;
    Ok("Companion started".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Load .env file automatically
    dotenv::dotenv().ok();

    let (audio_tx, _) = broadcast::channel::<Vec<f32>>(1000);
    let (audio_output_tx, _) = broadcast::channel::<Vec<f32>>(1000);
    let (transcript_tx, _) = broadcast::channel::<String>(1000);

    tauri::Builder::default()
        .manage(AppState {
            audio_tx,
            audio_output_tx,
            transcript_tx,
            capture: Mutex::new(AudioCapture::new()),
            is_running: Mutex::new(false),
        })
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let window = app.get_webview_window("main").unwrap();
            if let Ok(Some(monitor)) = window.primary_monitor() {
                let monitor_size = monitor.size();
                let monitor_pos = monitor.position();
                let window_size = window.outer_size().unwrap();

                let x = monitor_pos.x + (monitor_size.width as i32 - window_size.width as i32);
                let y = monitor_pos.y + (monitor_size.height as i32 - window_size.height as i32);

                let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![start_companion])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
