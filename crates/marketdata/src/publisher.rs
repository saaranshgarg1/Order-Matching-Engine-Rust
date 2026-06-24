use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use futures::SinkExt;
use tracing::{info, warn};

use crate::bus::MarketBus;

/// WebSocket publisher: every connected client receives all market events.
pub async fn run_ws_publisher(addr: SocketAddr, bus: MarketBus) {
    let listener = TcpListener::bind(addr).await
        .expect("failed to bind WS publisher");
    info!("Market-data WS on ws://{}", addr);

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                info!("WS client connected: {}", peer);
                let mut rx = bus.subscribe();
                tokio::spawn(async move {
                    match accept_async(stream).await {
                        Ok(ws) => {
                            let (mut sink, _source) = futures::StreamExt::split(ws);
                            loop {
                                match rx.recv().await {
                                    Some(ev) => {
                                        let json = match serde_json::to_string(&ev) {
                                            Ok(s)  => s,
                                            Err(e) => { warn!("serialise: {e}"); continue; }
                                        };
                                        if sink.send(Message::Text(json.into())).await.is_err() {
                                            break;
                                        }
                                    }
                                    None => break,
                                }
                            }
                        }
                        Err(e) => warn!("WS handshake failed from {peer}: {e}"),
                    }
                    info!("WS client disconnected: {}", peer);
                });
            }
            Err(e) => warn!("WS accept error: {e}"),
        }
    }
}
