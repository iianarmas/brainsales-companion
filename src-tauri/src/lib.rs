pub mod audio;
pub mod deepgram;
pub mod server;

use tokio::sync::broadcast;
use audio::capture::AudioCapture;
use std::sync::Mutex;
use tauri::{Manager, Emitter};

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
    let api_key = std::env::var("DEEPGRAM_API_KEY")
        .unwrap_or_else(|_| option_env!("DEEPGRAM_API_KEY").unwrap_or("").to_string());
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

#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    if let Some(update) = updater.check().await.map_err(|e| e.to_string())? {
        update
            .download_and_install(|_downloaded, _total| {}, || {})
            .await
            .map_err(|e| e.to_string())?;
        app.restart();
    }
    Ok(())
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
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Position window at bottom-right of primary monitor
            let window = app.get_webview_window("main").unwrap();
            if let Ok(Some(monitor)) = window.primary_monitor() {
                let monitor_size = monitor.size();
                let monitor_pos = monitor.position();
                let window_size = window.outer_size().unwrap();

                let x = monitor_pos.x + (monitor_size.width as i32 - window_size.width as i32);
                let y = monitor_pos.y + (monitor_size.height as i32 - window_size.height as i32);

                let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
            }

            // System tray: icon + menu
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "Quit BrainSales Companion", true, None::<&str>)?;
            let show = tauri::menu::MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show, &quit])?;

            let icon = app.default_window_icon().unwrap().clone();
            let _tray = tauri::tray::TrayIconBuilder::new()
                .icon(icon)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app: &tauri::AppHandle, event: tauri::menu::MenuEvent| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray: &tauri::tray::TrayIcon, event: tauri::tray::TrayIconEvent| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;

            // Intercept window close → hide to tray instead
            let window_clone = window.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window_clone.hide();
                }
            });

            // Enable autostart on Windows login
            use tauri_plugin_autostart::ManagerExt;
            let _ = app.autolaunch().enable();

            // Check for updates in the background
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tauri_plugin_updater::UpdaterExt;
                if let Ok(updater) = handle.updater() {
                    if let Ok(Some(update)) = updater.check().await {
                        let _ = handle.emit("update-available", update.version.clone());
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![start_companion, install_update])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
