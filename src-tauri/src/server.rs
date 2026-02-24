use tokio::net::TcpListener;
use tokio::sync::broadcast;
use futures_util::{StreamExt, SinkExt};

pub async fn start_local_server(rx: broadcast::Receiver<String>) {
    let addr = "127.0.0.1:4141";
    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
    println!("Listening on: {}", addr);

    while let Ok((stream, peer_addr)) = listener.accept().await {
        println!("New connection from: {}", peer_addr);
        let mut rx_clone = rx.resubscribe();
        
        tokio::spawn(async move {
            let ws_stream = match tokio_tungstenite::accept_async(stream).await {
                Ok(ws) => ws,
                Err(e) => {
                    eprintln!("Error during the websocket handshake: {}", e);
                    return;
                }
            };

            let (mut write, _) = ws_stream.split();

            // Broadcast any incoming strings (transcripts) to the connected web app
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
