use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tracing::{info, warn};

use engine::Engine;
use crate::session::handle_json_session;

/// Accept loop: spawn one task per connected client.
pub async fn run(addr: SocketAddr, engine: Arc<Engine>) {
    let listener = TcpListener::bind(addr).await
        .expect("failed to bind gateway TCP listener");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                info!("client connected: {}", peer);
                let eng = Arc::clone(&engine);
                tokio::spawn(async move {
                    handle_json_session(stream, eng).await;
                    info!("client disconnected: {}", peer);
                });
            }
            Err(e) => warn!("accept error: {e}"),
        }
    }
}
