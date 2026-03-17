use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use futures_util::{StreamExt, SinkExt};
use serde::Deserialize;
use tauri::Emitter;

#[derive(Deserialize)]
struct ControlMessage {
    command: String,
}

#[derive(Deserialize)]
struct TypedMessage {
    #[serde(rename = "type")]
    msg_type: String,
}

pub async fn start_local_server(
    rx: broadcast::Receiver<String>,
    to_browser_tx: broadcast::Sender<String>,
    cmd_tx: Arc<watch::Sender<String>>,
    app_handle: tauri::AppHandle,
) {
    let addr = "127.0.0.1:4141";
    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
    println!("Listening on: {}", addr);

    while let Ok((stream, peer_addr)) = listener.accept().await {
        println!("New connection from: {}", peer_addr);
        let mut rx_clone = rx.resubscribe();
        let mut to_browser_rx = to_browser_tx.subscribe();
        let cmd_tx_clone = cmd_tx.clone();
        let app_handle_clone = app_handle.clone();

        tokio::spawn(async move {
            let ws_stream = match tokio_tungstenite::accept_async(stream).await {
                Ok(ws) => ws,
                Err(e) => {
                    eprintln!("Error during the websocket handshake: {}", e);
                    return;
                }
            };

            let (mut write, mut read) = ws_stream.split();

            // Read task: control commands + typed sync messages from browser
            let cmd_tx_read = cmd_tx_clone.clone();
            tokio::spawn(async move {
                while let Some(Ok(msg)) = read.next().await {
                    if let tokio_tungstenite::tungstenite::protocol::Message::Text(text) = msg {
                        let text_str = text.as_str();
                        // Typed message (node_update, ai_suggestion, clear_suggestion) → emit to Tauri frontend
                        if let Ok(typed) = serde_json::from_str::<TypedMessage>(text_str) {
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(text_str) {
                                let _ = app_handle_clone.emit(&typed.msg_type, value);
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
                println!("Web app disconnected, sending stop command");
                let _ = cmd_tx_read.send("stop".to_string());
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
            println!("Connection from {} closed", peer_addr);
        });
    }
}
