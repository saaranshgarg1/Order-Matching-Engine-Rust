pub mod ring;
pub mod sequencer;
pub mod egress;
pub mod shard;

use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

use exchange_core::{Command, Price, Sequenced, SymbolId};
use wal::writer::FsyncPolicy;
use exchange_metrics::LatencyRecorder;
use exchange_metrics::Stage;

use ring::{ring_buffer, SharedSender};
use sequencer::Sequencer;
use egress::{egress_bus, EgressBus, EgressReceiver};
use shard::{shard_for, spawn_shard, ShardConfig};

pub struct EngineConfig {
    pub num_shards:    usize,
    pub symbols:       Vec<(SymbolId, Price)>,
    pub ring_capacity: usize,
    pub wal_dir:       PathBuf,
    pub fsync:         FsyncPolicy,
    pub pin_cores:     bool,
}

pub struct Engine {
    senders:    Vec<SharedSender>,
    sequencer:  Arc<Sequencer>,
    egress_rx:  EgressReceiver,
    handles:    Vec<JoinHandle<()>>,
    latency:    Arc<LatencyRecorder>,
    num_shards: usize,
}

impl Engine {
    pub fn start(cfg: EngineConfig) -> Self {
        let sequencer = Arc::new(Sequencer::new());
        let latency   = Arc::new(LatencyRecorder::new(Stage::EndToEnd));

        let mut shard_symbols: Vec<Vec<(SymbolId, Price)>> = (0..cfg.num_shards)
            .map(|_| Vec::new())
            .collect();
        for &(sym, tick) in &cfg.symbols {
            let s = shard_for(sym, cfg.num_shards);
            shard_symbols[s].push((sym, tick));
        }

        let mut senders = Vec::with_capacity(cfg.num_shards);
        let mut handles = Vec::with_capacity(cfg.num_shards);

        let (egress_tx, egress_rx) = egress_bus();

        for shard_id in 0..cfg.num_shards {
            let (ring_tx, ring_rx) = ring_buffer(cfg.ring_capacity);
            let sender = SharedSender::from(ring_tx);
            senders.push(sender);

            let pin_core = if cfg.pin_cores { Some(shard_id) } else { None };
            let scfg = ShardConfig {
                shard_id,
                symbols:  shard_symbols[shard_id].clone(),
                wal_dir:  cfg.wal_dir.clone(),
                fsync:    cfg.fsync,
                pin_core,
                ring_rx,
                egress:   egress_tx.clone(),
                latency:  Arc::clone(&latency),
            };
            handles.push(spawn_shard(scfg));
        }

        Engine { senders, sequencer, egress_rx, handles, latency, num_shards: cfg.num_shards }
    }

    /// Submit a command. Returns the assigned seq, or Err(cmd) on back-pressure.
    pub fn submit(&self, cmd: Command) -> Result<u64, Command> {
        let symbol = match &cmd {
            Command::New(no)               => no.symbol,
            Command::Cancel { symbol, .. } => *symbol,
            Command::Replace { symbol, .. }=> *symbol,
        };
        let shard = shard_for(symbol, self.num_shards);
        let (seq, ts) = self.sequencer.next();
        let sequenced = Sequenced { seq, ts, cmd };

        self.senders[shard].try_push(sequenced).map_err(|s| s.cmd)?;
        Ok(seq)
    }

    pub fn recv_event(&self) -> Option<exchange_core::OutputEvent> {
        self.egress_rx.recv()
    }

    pub fn drain_events(&self, out: &mut Vec<exchange_core::OutputEvent>) {
        self.egress_rx.drain(out);
    }

    pub fn latency(&self) -> &LatencyRecorder { &self.latency }

    pub fn shutdown(self) {
        drop(self.senders);
        for h in self.handles {
            let _ = h.join();
        }
    }
}
