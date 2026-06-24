use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing::{debug, warn};

use engine::Engine;
use protocol::{parse_inbound, serialize_outbound, JsonOutbound};
use crate::router::{json_to_command, reject_reason_str};
use exchange_core::OutputEvent;

/// Handle one JSON-line client session.
pub async fn handle_json_session(stream: TcpStream, engine: Arc<Engine>) {
    let peer = stream.peer_addr().ok();
    debug!("JSON session from {:?}", peer);

    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() { continue; }

        let msg = match parse_inbound(&line) {
            Ok(m)  => m,
            Err(e) => {
                let resp = format!("{{\"t\":\"error\",\"msg\":\"{}\"}}\n", e);
                let _ = write_half.write_all(resp.as_bytes()).await;
                continue;
            }
        };

        let cmd = match json_to_command(msg) {
            Some(c) => c,
            None    => continue,
        };

        match engine.submit(cmd) {
            Ok(seq) => {
                // Ack — proper fill events come via the egress bus to the marketdata publisher.
                let resp = serde_json::json!({"t":"submitted","seq":seq}).to_string() + "\n";
                if write_half.write_all(resp.as_bytes()).await.is_err() { break; }
            }
            Err(_) => {
                let resp = "{\"t\":\"error\",\"msg\":\"ring_full\"}\n";
                let _ = write_half.write_all(resp.as_bytes()).await;
            }
        }
    }

    debug!("JSON session from {:?} closed", peer);
}
