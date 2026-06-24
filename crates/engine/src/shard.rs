use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use exchange_core::{apply, Command, OrderBook, OutputEvent, Price, SymbolId};
use wal::writer::{FsyncPolicy, WalWriter};
use wal::record::RecordType;
use exchange_metrics::LatencyRecorder;

use crate::ring::RingReceiver;
use crate::egress::EgressBus;

pub struct ShardConfig {
    pub shard_id:  usize,
    pub symbols:   Vec<(SymbolId, Price)>,
    pub wal_dir:   PathBuf,
    pub fsync:     FsyncPolicy,
    pub pin_core:  Option<usize>,
    pub ring_rx:   RingReceiver,
    pub egress:    EgressBus,
    pub latency:   Arc<LatencyRecorder>,
    /// Shared epoch so cmd.ts and shard timestamps are on the same clock.
    pub epoch:     Arc<Instant>,
}

pub fn spawn_shard(cfg: ShardConfig) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name(format!("shard-{}", cfg.shard_id))
        .spawn(move || run_shard(cfg))
        .expect("failed to spawn shard thread")
}

fn run_shard(cfg: ShardConfig) {
    #[cfg(target_os = "linux")]
    if let Some(core) = cfg.pin_core {
        if let Some(cpus) = core_affinity::get_core_ids() {
            if let Some(id) = cpus.get(core) {
                core_affinity::set_for_current(*id);
            }
        }
    }

    let mut books: HashMap<SymbolId, OrderBook> = cfg.symbols.iter()
        .map(|&(sym, tick)| (sym, OrderBook::new(sym, tick)))
        .collect();

    let wal_dir = cfg.wal_dir.join(format!("shard-{}", cfg.shard_id));
    let mut wal = WalWriter::open(&wal_dir, cfg.fsync)
        .expect("failed to open WAL");

    let mut events: Vec<OutputEvent> = Vec::with_capacity(64);

    loop {
        let cmd = match cfg.ring_rx.pop() {
            Some(c) => c,
            None    => break,
        };

        // Ingress ts is nanos from shared epoch; match_ts is the same clock.
        let match_ts = cfg.epoch.elapsed().as_nanos() as u64;

        // Write to WAL before apply (command-sourcing, ADR-003).
        let payload = serde_json::to_vec(&cmd)
            .unwrap_or_else(|_| format!("{:?}", cmd.cmd).into_bytes());
        let _ = wal.append(cmd.seq, cmd.ts, RecordType::Command, payload);

        let symbol = match &cmd.cmd {
            Command::New(no)               => no.symbol,
            Command::Cancel { symbol, .. } => *symbol,
            Command::Replace { symbol, .. }=> *symbol,
        };

        events.clear();
        if let Some(book) = books.get_mut(&symbol) {
            apply(book, &cmd, &mut events);
            cfg.egress.send_batch(&events);

            // ── Prometheus counters ──────────────────────────────────────
            let sym_label: &'static str = Box::leak(symbol.to_string().into_boxed_str());
            let type_label: &'static str = match &cmd.cmd {
                Command::New(no) => match no.kind {
                    exchange_core::OrderType::Limit      => "limit",
                    exchange_core::OrderType::Market     => "market",
                    exchange_core::OrderType::Ioc        => "ioc",
                    exchange_core::OrderType::Fok        => "fok",
                    exchange_core::OrderType::StopMarket => "stop_market",
                    exchange_core::OrderType::StopLimit  => "stop_limit",
                },
                Command::Cancel { .. }  => "cancel",
                Command::Replace { .. } => "replace",
            };
            exchange_metrics::inc_orders(sym_label, type_label);

            for ev in &events {
                match ev {
                    OutputEvent::Trade { .. } => exchange_metrics::inc_trades(sym_label),
                    OutputEvent::Rejected { reason, .. } => {
                        let r: &'static str = match reason {
                            exchange_core::RejectReason::UnknownSymbol    => "unknown_symbol",
                            exchange_core::RejectReason::DuplicateId      => "duplicate_id",
                            exchange_core::RejectReason::UnknownId        => "unknown_id",
                            exchange_core::RejectReason::ZeroQty          => "zero_qty",
                            exchange_core::RejectReason::BadPrice         => "bad_price",
                            exchange_core::RejectReason::FokUnfillable    => "fok_unfillable",
                            exchange_core::RejectReason::MarketNoLiquidity=> "no_liquidity",
                            exchange_core::RejectReason::SelfTrade        => "self_trade",
                            exchange_core::RejectReason::RateLimited      => "rate_limited",
                        };
                        exchange_metrics::inc_rejects(r);
                    }
                    _ => {}
                }
            }

            // Book depth + spread gauges (cheap; post-match snapshot)
            let snap = book.depth(1);
            if let (Some((bid, bq)), Some((ask, aq))) = (snap.bids.first(), snap.asks.first()) {
                use exchange_metrics::{set_book_depth, set_spread};
                use protocol::ticks_to_dollars;
                set_book_depth(sym_label, "bid", *bq as f64);
                set_book_depth(sym_label, "ask", *aq as f64);
                set_spread(sym_label, (ask - bid) as f64);
            }
        }

        // End-to-end latency: ingress stamp → match completion (same epoch).
        let elapsed_ns = match_ts.saturating_sub(cmd.ts);
        cfg.latency.record_ns(elapsed_ns);
    }

    let _ = wal.flush();
}

pub fn shard_for(symbol: SymbolId, num_shards: usize) -> usize {
    (symbol as usize) % num_shards
}
