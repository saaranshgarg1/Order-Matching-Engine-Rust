use std::sync::Arc;
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use exchange_core::Sequenced;

/// Bounded SPSC/MPSC ring buffer wrapping crossbeam's bounded channel.
/// The bound enforces back-pressure: producers block/error when full.
pub struct RingSender(pub Sender<Sequenced>);
pub struct RingReceiver(pub Receiver<Sequenced>);

pub fn ring_buffer(capacity: usize) -> (RingSender, RingReceiver) {
    let (tx, rx) = bounded(capacity);
    (RingSender(tx), RingReceiver(rx))
}

impl RingSender {
    /// Non-blocking push. Returns Err if ring is full (back-pressure signal).
    pub fn try_push(&self, cmd: Sequenced) -> Result<(), Sequenced> {
        self.0.try_send(cmd).map_err(|e| match e {
            TrySendError::Full(c) | TrySendError::Disconnected(c) => c,
        })
    }

    /// Blocking push. Blocks until space is available.
    pub fn push(&self, cmd: Sequenced) {
        let _ = self.0.send(cmd);
    }
}

impl RingReceiver {
    /// Blocking pop. Returns None if all senders dropped.
    pub fn pop(&self) -> Option<Sequenced> {
        self.0.recv().ok()
    }
}

/// Cloneable sender handle (for MPSC: multiple gateways → one shard).
#[derive(Clone)]
pub struct SharedSender(pub Arc<Sender<Sequenced>>);

impl SharedSender {
    pub fn from(tx: RingSender) -> Self {
        SharedSender(Arc::new(tx.0))
    }

    pub fn try_push(&self, cmd: Sequenced) -> Result<(), Sequenced> {
        self.0.try_send(cmd).map_err(|e| match e {
            TrySendError::Full(c) | TrySendError::Disconnected(c) => c,
        })
    }

    pub fn push(&self, cmd: Sequenced) {
        let _ = self.0.send(cmd);
    }

    pub fn occupancy_pct(&self) -> f64 {
        let cap = self.0.capacity().unwrap_or(1);
        let len = self.0.len();
        len as f64 / cap as f64 * 100.0
    }
}
