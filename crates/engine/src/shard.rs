use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use exchange_core::{apply, OrderBook, OutputEvent, Price, SymbolId};
use wal::writer::{FsyncPolicy, WalWriter};
use wal::record::RecordType;
use exchange_metrics::LatencyRecorder;
use exchange_metrics::Stage;

use crate::ring::{RingReceiver, SharedSender};
use crate::egress::EgressBus;
use crate::sequencer::Sequencer;

pub struct ShardConfig {
    pub shard_id:     usize,
    pub symbols:      Vec<(SymbolId, Price)>,
    pub wal_dir:      PathBuf,
    pub fsync:        FsyncPolicy,
    pub pin_core:     Option<usize>,
    pub ring_rx:      RingReceiver,
    pub egress:       EgressBus,
    pub latency:      Arc<LatencyRecorder>,
}

/// Spawn the matching thread for one shard. Returns join handle.
pub fn spawn_shard(cfg: ShardConfig) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name(format!("shard-{}", cfg.shard_id))
        .spawn(move || run_shard(cfg))
        .expect("failed to spawn shard thread")
}

fn run_shard(cfg: ShardConfig) {
    // Optional CPU pinning (best-effort).
    #[cfg(target_os = "linux")]
    if let Some(core) = cfg.pin_core {
        if let Some(cpus) = core_affinity::get_core_ids() {
            if let Some(id) = cpus.get(core) {
                core_affinity::set_for_current(*id);
            }
        }
    }

    // Build per-symbol books.
    let mut books: HashMap<SymbolId, OrderBook> = cfg.symbols.iter()
        .map(|&(sym, tick)| (sym, OrderBook::new(sym, tick)))
        .collect();

    // Open WAL for this shard.
    let wal_dir = cfg.wal_dir.join(format!("shard-{}", cfg.shard_id));
    let mut wal = WalWriter::open(&wal_dir, cfg.fsync)
        .expect("failed to open WAL");

    let mut events: Vec<OutputEvent> = Vec::with_capacity(64);

    loop {
        let cmd = match cfg.ring_rx.pop() {
            Some(c) => c,
            None    => break, // engine shutting down
        };

        let t0_ns = {
            use std::time::Instant;
            // We store a thread-local epoch; cheap elapsed call.
            static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
            let epoch = EPOCH.get_or_init(Instant::now);
            epoch.elapsed().as_nanos() as u64
        };

        // Write command to WAL before applying (command-sourcing).
        let payload = format!("{:?}", cmd.cmd).into_bytes(); // simple text for now
        let _ = wal.append(cmd.seq, cmd.ts, RecordType::Command, payload);

        // Determine target book.
        let symbol = match &cmd.cmd {
            exchange_core::Command::New(no)               => no.symbol,
            exchange_core::Command::Cancel { symbol, .. } => *symbol,
            exchange_core::Command::Replace { symbol, .. }=> *symbol,
        };

        events.clear();
        if let Some(book) = books.get_mut(&symbol) {
            apply(book, &cmd, &mut events);
            cfg.egress.send_batch(&events);
        }
        // If symbol unknown, silently drop (gateway should validate first).

        // Record end-to-end latency (ingress ts → now).
        let elapsed = t0_ns.saturating_sub(cmd.ts);
        cfg.latency.record_ns(elapsed);
    }

    let _ = wal.flush();
}

/// Compute which shard owns a symbol (stable hash).
pub fn shard_for(symbol: SymbolId, num_shards: usize) -> usize {
    (symbol as usize) % num_shards
}
