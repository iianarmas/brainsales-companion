use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use futures_util::{StreamExt, SinkExt};
use serde::Deserialize;

#[derive(Deserialize)]
struct ControlMessage {
    command: String,
}

pub async fn start_local_server(rx: broadcast::Receiver<String>, cmd_tx: Arc<watch::Sender<String>>) {
    let addr = "127.0.0.1:4141";
    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
    println!("Listening on: {}", addr);

    while let Ok((stream, peer_addr)) = listener.accept().await {
        println!("New connection from: {}", peer_addr);
        let mut rx_clone = rx.resubscribe();
        let cmd_tx_clone = cmd_tx.clone();

        tokio::spawn(async move {
            let ws_stream = match tokio_tungstenite::accept_async(stream).await {
                Ok(ws) => ws,
                Err(e) => {
                    eprintln!("Error during the websocket handshake: {}", e);
                    return;
                }
            };

            let (mut write, mut read) = ws_stream.split();

            // Spawn task to read control commands from the web app
            let cmd_tx_read = cmd_tx_clone.clone();
            tokio::spawn(async move {
                while let Some(Ok(msg)) = read.next().await {
                    if let tokio_tungstenite::tungstenite::protocol::Message::Text(text) = msg {
                        if let Ok(ctrl) = serde_json::from_str::<ControlMessage>(&text) {
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

            // Send transcripts to the web app
            loop {
                match rx_clone.recv().await {
                    Ok(msg) => {
                        if let Err(e) = write.send(tokio_tungstenite::tungstenite::protocol::Message::Text(msg.into())).await {
                            eprintln!("Error sending transcript to local WebSocket client: {}", e);
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("Client lagged by {} messages, resuming...", n);
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        eprintln!("Internal broadcast channel closed. Terminating connection.");
                        break;
                    }
                }
            }
            println!("Connection from {} closed", peer_addr);
        });
    }
}
