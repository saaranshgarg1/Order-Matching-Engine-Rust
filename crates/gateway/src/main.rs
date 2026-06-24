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
use marketdata::{market_bus, run_ws_publisher, SnapshotBuilder};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let symbol_names = vec!["AAPL", "MSFT", "TSLA", "GOOG"];
    let symbols: Vec<(u32, i64)> = symbol_names.iter()
        .map(|s| (symbol_to_id(s), 1))
        .collect();

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

    // ── Prometheus metrics ────────────────────────────────────────────
    let metrics_addr: SocketAddr = "0.0.0.0:9090".parse().unwrap();
    if let Err(e) = init_prometheus(metrics_addr) {
        warn!("metrics init failed: {e}");
    } else {
        info!("Prometheus metrics → http://0.0.0.0:9090/metrics");
    }

    // ── Market-data broadcast bus ─────────────────────────────────────
    let (mkt_bus, _first_rx) = market_bus(65_536);

    // Egress consumer: drain engine events → publish to market-data bus
    {
        let egress_rx  = engine.subscribe_egress();
        let bus        = mkt_bus.clone();
        let sym_names  = symbol_names.clone();
        let sym_ids: Vec<u32> = symbols.iter().map(|(id, _)| *id).collect();

        tokio::task::spawn_blocking(move || {
            // One SnapshotBuilder per symbol
            let mut builders: std::collections::HashMap<u32, SnapshotBuilder> =
                sym_ids.iter().zip(sym_names.iter())
                    .map(|(&id, &name)| {
                        (id, SnapshotBuilder::new(id, name.to_string(), bus.clone(), 20))
                    })
                    .collect();

            let mut seq = 0u64;
            loop {
                let ev = match egress_rx.recv() {
                    Some(e) => e,
                    None    => break,
                };
                seq += 1;

                // Route to the right builder by reading symbol from Trade events
                use exchange_core::OutputEvent;
                if let OutputEvent::Trade { .. } = &ev {
                    // All builders get trade events — each ignores wrong symbols
                    for b in builders.values_mut() { b.process(&ev, seq); }
                } else {
                    for b in builders.values_mut() { b.process(&ev, seq); }
                }
            }
        });
    }

    // ── WebSocket market-data publisher ───────────────────────────────
    let ws_addr: SocketAddr = "0.0.0.0:9002".parse().unwrap();
    info!("Market-data WS → ws://0.0.0.0:9002");
    tokio::spawn(run_ws_publisher(ws_addr, mkt_bus));

    // ── TCP order-entry gateway ───────────────────────────────────────
    let gateway_addr: SocketAddr = "0.0.0.0:9001".parse().unwrap();
    info!("Gateway → tcp://0.0.0.0:9001");
    tcp::run(gateway_addr, Arc::clone(&engine)).await;
}
