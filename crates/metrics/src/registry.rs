use metrics_exporter_prometheus::PrometheusBuilder;
use std::net::SocketAddr;

/// Install the global Prometheus recorder and start the HTTP exporter.
/// Call once at startup before any `metrics::counter!` / `metrics::gauge!` calls.
pub fn init_prometheus(bind: SocketAddr) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    PrometheusBuilder::new()
        .with_http_listener(bind)
        .install()?;
    Ok(())
}

/// Convenience: increment an order counter with symbol + type labels.
pub fn inc_orders(symbol: &'static str, order_type: &'static str) {
    metrics::counter!("exchange_orders_total", "symbol" => symbol, "type" => order_type).increment(1);
}

pub fn inc_trades(symbol: &'static str) {
    metrics::counter!("exchange_trades_total", "symbol" => symbol).increment(1);
}

pub fn inc_rejects(reason: &'static str) {
    metrics::counter!("exchange_rejects_total", "reason" => reason).increment(1);
}

pub fn set_book_depth(symbol: &'static str, side: &'static str, depth: f64) {
    metrics::gauge!("exchange_book_depth", "symbol" => symbol, "side" => side).set(depth);
}

pub fn set_spread(symbol: &'static str, ticks: f64) {
    metrics::gauge!("exchange_spread_ticks", "symbol" => symbol).set(ticks);
}

pub fn set_ring_occupancy(shard: u32, pct: f64) {
    let shard_s: &'static str = Box::leak(shard.to_string().into_boxed_str());
    metrics::gauge!("exchange_ring_occupancy", "shard" => shard_s).set(pct);
}
