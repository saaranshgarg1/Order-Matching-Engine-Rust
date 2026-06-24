mod session;
mod tcp;
mod router;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use engine::{Engine, EngineConfig};
use exchange_metrics::init_prometheus;
use wal::writer::FsyncPolicy;
use protocol::symbol_to_id;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let symbols: Vec<(u32, i64)> = vec![
        (symbol_to_id("AAPL"), 1),
        (symbol_to_id("MSFT"), 1),
        (symbol_to_id("TSLA"), 1),
        (symbol_to_id("GOOG"), 1),
    ];

    let engine_cfg = EngineConfig {
        num_shards:    2,
        symbols:       symbols.clone(),
        ring_capacity: 65_536,
        wal_dir:       PathBuf::from("./data/wal"),
        fsync:         FsyncPolicy::Off,
        pin_cores:     false,
    };

    let engine = Arc::new(Engine::start(engine_cfg));
    info!("Engine started with {} symbols", symbols.len());

    let metrics_addr: SocketAddr = "0.0.0.0:9090".parse().unwrap();
    if let Err(e) = init_prometheus(metrics_addr) {
        warn!("metrics init failed: {e}");
    } else {
        info!("Prometheus metrics at http://0.0.0.0:9090/metrics");
    }

    let gateway_addr: SocketAddr = "0.0.0.0:9001".parse().unwrap();
    info!("Gateway listening on {}", gateway_addr);
    tcp::run(gateway_addr, Arc::clone(&engine)).await;
}
