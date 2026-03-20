pub mod audio;
pub mod deepgram;
pub mod server;

use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, watch};
use audio::capture::AudioCapture;
use tauri::{Manager, Emitter, Listener};

#[derive(Clone, Debug)]
pub struct AuthInfo {
    pub user_id: String,
    pub org_id: String,
}

pub struct AppState {
    pub audio_tx: broadcast::Sender<Vec<f32>>,
    pub audio_output_tx: broadcast::Sender<Vec<f32>>,
    pub transcript_tx: broadcast::Sender<String>,
    pub to_browser_tx: broadcast::Sender<String>,
    pub capture: Mutex<AudioCapture>,
    pub is_running: Mutex<bool>,
    pub cmd_tx: Arc<watch::Sender<String>>,
    pub auth_info: Arc<Mutex<Option<AuthInfo>>>,
}

/// The web app URL used for nonce exchange. Defaults to production;
/// can be overridden via the BRAINSALES_APP_URL env var.
fn web_app_url() -> String {
    std::env::var("BRAINSALES_APP_URL")
        .unwrap_or_else(|_| "https://app.brainsales.tech".to_string())
}

/// Exchange a one-time nonce with the web app API to get auth info.
async fn exchange_nonce(nonce: &str) -> Result<AuthInfo, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/auth/companion-nonce/exchange", web_app_url()))
        .json(&serde_json::json!({ "nonce": nonce }))
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("Exchange failed: {}", resp.status()).into());
    }

    let data: serde_json::Value = resp.json().await?;
    Ok(AuthInfo {
        user_id: data["userId"].as_str().unwrap_or_default().to_string(),
        org_id: data["orgId"].as_str().unwrap_or_default().to_string(),
    })
}

#[tauri::command]
async fn start_companion(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    gain: Option<f32>,
) -> Result<String, String> {
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

    let to_browser_tx = state.to_browser_tx.clone();

    let cmd_rx = state.cmd_tx.subscribe();
    let cmd_tx_for_server = state.cmd_tx.clone();

    let auth_info = state.auth_info.clone();

    // Start Audio Capture
    let ((mic_sample_rate, _mic_channels), (_sys_sample_rate, _sys_channels)) = {
        let mut capture = state.capture.lock().unwrap();
        if let Some(g) = gain {
            capture.mic_gain = g;
        }
        match capture.start(audio_tx, audio_output_tx) {
            Ok(config) => config,
            Err(e) => return Err(format!("Failed to start audio capture: {}", e)),
        }
    };

    let api_key = std::env::var("DEEPGRAM_API_KEY")
        .unwrap_or_else(|_| option_env!("DEEPGRAM_API_KEY").unwrap_or("").to_string());
    if api_key == "PLACEHOLDER" {
        println!("Warning: DEEPGRAM_API_KEY is not set, using placeholder.");
    }

    let transcript_tx_clone = transcript_tx.clone();

    // Deepgram loop: waits for "start" command from web app before connecting to Deepgram
    tauri::async_runtime::spawn(async move {
        println!("Deepgram loop ready ({}Hz, {}Hz). Waiting for start command...", mic_sample_rate, _sys_sample_rate);
        deepgram::run_deepgram_loop(window, audio_rx, audio_output_rx, transcript_tx_clone, api_key, mic_sample_rate, _sys_sample_rate, cmd_rx).await;
    });

    // Local WebSocket server: forwards transcripts to web app, reads control commands, handles sync messages
    tauri::async_runtime::spawn(async move {
        println!("Starting local WebSocket server task...");
        server::start_local_server(transcript_rx, to_browser_tx, cmd_tx_for_server, app, auth_info).await;
    });

    *is_running = true;
    Ok("Companion started".to_string())
}

#[tauri::command]
async fn send_to_browser(state: tauri::State<'_, AppState>, payload: String) -> Result<(), String> {
    state.to_browser_tx.send(payload).map_err(|e| e.to_string())?;
    Ok(())
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

#[tauri::command]
fn list_audio_devices() -> serde_json::Value {
    let (inputs, outputs) = AudioCapture::list_devices();
    serde_json::json!({ "inputs": inputs, "outputs": outputs })
}

#[tauri::command]
async fn set_audio_devices(
    state: tauri::State<'_, AppState>,
    input: Option<String>,
    output: Option<String>,
) -> Result<(), String> {
    let audio_tx = state.audio_tx.clone();
    let audio_output_tx = state.audio_output_tx.clone();
    let mut capture = state.capture.lock().unwrap();
    capture.stop();
    capture.preferred_input = input;
    capture.preferred_output = output;
    capture.start(audio_tx, audio_output_tx).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn set_mic_gain(
    state: tauri::State<'_, AppState>,
    gain: f32,
) -> Result<(), String> {
    let audio_tx = state.audio_tx.clone();
    let audio_output_tx = state.audio_output_tx.clone();
    let mut capture = state.capture.lock().unwrap();

    // Only restart if gain actually changed and we are already running
    if (capture.mic_gain - gain).abs() > 0.001 {
        capture.mic_gain = gain;
        // If streams are active, we need to restart them to apply the new gain
        // (Since the gain is captured in the closure during .start())
        if capture.is_active() {
            capture.stop();
            capture.start(audio_tx, audio_output_tx).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
async fn show_window(window: tauri::Window) {
    let _ = window.show();
    let _ = window.set_focus();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Load .env file — try CWD first, then src-tauri/.env
    dotenv::dotenv().ok();
    dotenv::from_path("src-tauri/.env").ok();

    let (audio_tx, _) = broadcast::channel::<Vec<f32>>(1000);
    let (audio_output_tx, _) = broadcast::channel::<Vec<f32>>(1000);
    let (transcript_tx, _) = broadcast::channel::<String>(1000);
    let (to_browser_tx, _) = broadcast::channel::<String>(100);
    let (cmd_tx, _) = watch::channel("idle".to_string());
    let cmd_tx = Arc::new(cmd_tx);

    tauri::Builder::default()
        .manage(AppState {
            audio_tx,
            audio_output_tx,
            transcript_tx,
            to_browser_tx,
            capture: Mutex::new(AudioCapture::new()),
            is_running: Mutex::new(false),
            cmd_tx,
            auth_info: Arc::new(Mutex::new(None)),
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        let _ = app.emit("global_shortcut", shortcut.to_string());
                    }
                })
                .build(),
        )
        .setup(|app| {
            // Position window at bottom-right of primary monitor (fallback; JS restores saved position)
            let window = app.get_webview_window("main").unwrap();
            if let Ok(Some(monitor)) = window.primary_monitor() {
                let monitor_size = monitor.size();
                let monitor_pos = monitor.position();
                let window_size = window.outer_size().unwrap();

                let x = monitor_pos.x + (monitor_size.width as i32 - window_size.width as i32) - 16;
                let y = monitor_pos.y + (monitor_size.height as i32 - window_size.height as i32) - 48;

                let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
            }

            // Set window icon for taskbar
            if let Some(icon) = app.default_window_icon().cloned() {
                let _ = window.set_icon(icon);
            }

            // Register global shortcuts
            use tauri_plugin_global_shortcut::GlobalShortcutExt;
            let _ = app.global_shortcut().register("ctrl+shift+b");
            let _ = app.global_shortcut().register("ctrl+shift+p");
            let _ = app.global_shortcut().register("ctrl+shift+n");
            let _ = app.global_shortcut().register("ctrl+shift+o");

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

            // No close interception — close button quits the app

            // Handle deep link URL for auth
            let app_handle = app.handle().clone();
            app.listen("deep-link://new-url", move |event| {
                if let Ok(urls) = serde_json::from_str::<Vec<String>>(event.payload()) {
                    if let Some(url_str) = urls.first() {
                        if let Ok(parsed) = url::Url::parse(url_str) {
                            let params: std::collections::HashMap<String, String> =
                                parsed.query_pairs().map(|(k, v)| (k.to_string(), v.to_string())).collect();

                            if let Some(nonce) = params.get("nonce") {
                                let nonce = nonce.clone();
                                let ah = app_handle.clone();

                                // Also store userId/orgId from URL params as a fallback
                                let url_user_id = params.get("userId").cloned().unwrap_or_default();
                                let url_org_id = params.get("orgId").cloned().unwrap_or_default();

                                tokio::spawn(async move {
                                    match exchange_nonce(&nonce).await {
                                        Ok(auth) => {
                                            let state = ah.state::<AppState>();
                                            *state.auth_info.lock().unwrap() = Some(auth);
                                            let _ = ah.emit("auth_received", ());
                                            // Show the window when launched via deep link
                                            if let Some(w) = ah.get_webview_window("main") {
                                                let _ = w.show();
                                                let _ = w.set_focus();
                                            }
                                        }
                                        Err(e) => {
                                            eprintln!("Nonce exchange failed: {} — using URL params as fallback", e);
                                            // Fallback: use the userId/orgId from URL params directly
                                            if !url_user_id.is_empty() && !url_org_id.is_empty() {
                                                let state = ah.state::<AppState>();
                                                *state.auth_info.lock().unwrap() = Some(AuthInfo {
                                                    user_id: url_user_id,
                                                    org_id: url_org_id,
                                                });
                                                let _ = ah.emit("auth_received", ());
                                                if let Some(w) = ah.get_webview_window("main") {
                                                    let _ = w.show();
                                                    let _ = w.set_focus();
                                                }
                                            } else {
                                                let _ = ah.emit("auth_failed", e.to_string());
                                            }
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
            });

            // If no deep-link auth arrives within 3 seconds, tell the frontend
            {
                let ah = app.handle().clone();
                let auth_check = app.state::<AppState>().auth_info.clone();
                let is_dev = cfg!(debug_assertions) || std::env::var("BRAINSALES_DEV").unwrap_or_default() == "1";
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let has_auth = auth_check.lock().unwrap().is_some();
                    if !has_auth {
                        if is_dev {
                            let _ = ah.emit("auth_received", ());
                        } else {
                            let _ = ah.emit("auth_failed", "not_launched_from_web_app");
                        }
                    }
                });
            }

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
        .invoke_handler(tauri::generate_handler![start_companion, send_to_browser, install_update, list_audio_devices, set_audio_devices, set_mic_gain, show_window])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
