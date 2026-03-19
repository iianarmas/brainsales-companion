use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use futures_util::{StreamExt, SinkExt};
use serde::Deserialize;
use tauri::Emitter;
use crate::AuthInfo;

#[derive(Deserialize)]
struct ControlMessage {
    command: String,
}

#[derive(Deserialize)]
struct TypedMessage {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(flatten)]
    value: serde_json::Value,
}

pub async fn start_local_server(
    rx: broadcast::Receiver<String>,
    to_browser_tx: broadcast::Sender<String>,
    cmd_tx: Arc<watch::Sender<String>>,
    app_handle: tauri::AppHandle,
    auth_info: Arc<Mutex<Option<AuthInfo>>>,
) {
    let addr = "127.0.0.1:4141";
    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
    println!("Listening on: {}", addr);

    // Connection generation counter: prevents stale browser_disconnected events
    // from old connections overriding a newer browser_connected.
    let conn_gen = Arc::new(AtomicU64::new(0));

    while let Ok((stream, peer_addr)) = listener.accept().await {
        println!("New connection from: {}", peer_addr);
        let mut rx_clone = rx.resubscribe();
        let mut to_browser_rx = to_browser_tx.subscribe();
        let cmd_tx_clone = cmd_tx.clone();
        let app_handle_clone = app_handle.clone();
        let auth_info_clone = auth_info.clone();
        let conn_gen_clone = conn_gen.clone();

        tokio::spawn(async move {
            let ws_stream = match tokio_tungstenite::accept_async(stream).await {
                Ok(ws) => ws,
                Err(e) => {
                    eprintln!("Error during the websocket handshake: {}", e);
                    return;
                }
            };

            let (mut write, mut read) = ws_stream.split();

            // ── Auth handshake: first message must be auth_handshake ─────────
            let authenticated = {
                let mut authed = false;
                // Wait for the first message with a 10-second timeout
                let first_msg = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    read.next(),
                ).await;

                match first_msg {
                    Ok(Some(Ok(tokio_tungstenite::tungstenite::protocol::Message::Text(text)))) => {
                        let text_str = text.as_str();
                        if let Ok(typed) = serde_json::from_str::<TypedMessage>(text_str) {
                            if typed.msg_type == "auth_handshake" {
                                let auth_lock = auth_info_clone.lock().unwrap().clone();
                                if let Some(auth) = auth_lock {
                                    let msg_user_id = typed.value.get("userId").and_then(|v| v.as_str());
                                    if msg_user_id == Some(&auth.user_id) {
                                        authed = true;
                                        let _ = write.send(tokio_tungstenite::tungstenite::protocol::Message::Text(
                                            serde_json::json!({ "type": "auth_confirmed" }).to_string().into()
                                        )).await;
                                        let _ = app_handle_clone.emit("browser_connected", ());
                                    } else {
                                        let _ = write.send(tokio_tungstenite::tungstenite::protocol::Message::Text(
                                            serde_json::json!({ "type": "auth_rejected", "reason": "user_id_mismatch" }).to_string().into()
                                        )).await;
                                    }
                                } else {
                                    // No auth info stored — only accept in dev/debug mode
                                    let is_dev = cfg!(debug_assertions) || std::env::var("BRAINSALES_DEV").unwrap_or_default() == "1";
                                    if is_dev {
                                        authed = true;
                                        let _ = write.send(tokio_tungstenite::tungstenite::protocol::Message::Text(
                                            serde_json::json!({ "type": "auth_confirmed" }).to_string().into()
                                        )).await;
                                        let _ = app_handle_clone.emit("browser_connected", ());
                                    } else {
                                        let _ = write.send(tokio_tungstenite::tungstenite::protocol::Message::Text(
                                            serde_json::json!({ "type": "auth_rejected", "reason": "not_launched_from_web_app" }).to_string().into()
                                        )).await;
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        // Timeout or error — reject
                    }
                }

                if !authed {
                    println!("Connection from {} rejected (auth failed)", peer_addr);
                }
                authed
            };

            if !authenticated {
                return;
            }

            // Stamp this connection with a generation number so stale
            // browser_disconnected events don't override a newer connection.
            let my_gen = conn_gen_clone.fetch_add(1, Ordering::SeqCst) + 1;

            // Read task: control commands + typed sync messages from browser
            let cmd_tx_read = cmd_tx_clone.clone();
            let app_for_disconnect = app_handle_clone.clone();
            let conn_gen_read = conn_gen_clone.clone();
            tokio::spawn(async move {
                while let Some(Ok(msg)) = read.next().await {
                    if let tokio_tungstenite::tungstenite::protocol::Message::Text(text) = msg {
                        let text_str = text.as_str();
                        // Typed message (node_update, ai_suggestion, clear_suggestion, etc.) → emit to Tauri frontend
                        if let Ok(typed) = serde_json::from_str::<TypedMessage>(text_str) {
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(text_str) {
                                let _ = app_for_disconnect.emit(&typed.msg_type, value);
                            }
                        } else if let Ok(ctrl) = serde_json::from_str::<ControlMessage>(text_str) {
                            // Control command → audio pipeline
                            let cmd = ctrl.command.as_str();
                            if matches!(cmd, "start" | "stop" | "pause") {
                                println!("Control command received: {}", cmd);
                                let _ = cmd_tx_read.send(cmd.to_string());
                            }
                        }
                    }
                }
                // Web app disconnected — ensure Deepgram stops
                println!("Web app disconnected (read task), sending stop command");
                let _ = cmd_tx_read.send("stop".to_string());
                // Only emit browser_disconnected if no newer connection has since connected
                if conn_gen_read.load(Ordering::SeqCst) == my_gen {
                    let _ = app_for_disconnect.emit("browser_disconnected", ());
                } else {
                    println!("Suppressed stale browser_disconnected (gen {} < current {})", my_gen, conn_gen_read.load(Ordering::SeqCst));
                }
            });

            // Write loop: forward transcripts AND to_browser messages (navigate, ai_feedback) to browser
            loop {
                tokio::select! {
                    result = rx_clone.recv() => {
                        match result {
                            Ok(msg) => {
                                if write.send(tokio_tungstenite::tungstenite::protocol::Message::Text(msg.into())).await.is_err() {
                                    break;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                eprintln!("Transcript client lagged by {} messages, resuming...", n);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                    result = to_browser_rx.recv() => {
                        match result {
                            Ok(msg) => {
                                if write.send(tokio_tungstenite::tungstenite::protocol::Message::Text(msg.into())).await.is_err() {
                                    break;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                }
            }
            println!("Connection from {} closed (write loop)", peer_addr);
            // Only emit browser_disconnected if no newer connection has since connected
            if conn_gen_clone.load(Ordering::SeqCst) == my_gen {
                let _ = app_handle_clone.emit("browser_disconnected", ());
            } else {
                println!("Suppressed stale browser_disconnected from write loop (gen {} < current {})", my_gen, conn_gen_clone.load(Ordering::SeqCst));
            }
        });
    }
}
