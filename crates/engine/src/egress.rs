use std::sync::Arc;
use crossbeam_channel::{unbounded, Receiver, Sender};
use exchange_core::OutputEvent;

/// Outbound event bus: one sender per shard, many receivers (WAL, marketdata, gateway).
/// Uses an unbounded channel on the egress side — back-pressure is applied upstream
/// at the ring buffer, not here. WAL / marketdata must keep up or be dropped.
#[derive(Clone)]
pub struct EgressBus {
    tx: Sender<OutputEvent>,
}

pub struct EgressReceiver {
    rx: Receiver<OutputEvent>,
}

pub fn egress_bus() -> (EgressBus, EgressReceiver) {
    let (tx, rx) = unbounded();
    (EgressBus { tx }, EgressReceiver { rx })
}

impl EgressBus {
    pub fn send(&self, event: OutputEvent) {
        // Best-effort: if receiver dropped, silently ignore.
        let _ = self.tx.send(event);
    }

    pub fn send_batch(&self, events: &[OutputEvent]) {
        for e in events {
            let _ = self.tx.send(e.clone());
        }
    }
}

impl EgressReceiver {
    /// Blocking receive. Returns None when all senders are dropped (engine shut down).
    pub fn recv(&self) -> Option<OutputEvent> {
        self.rx.recv().ok()
    }

    /// Non-blocking drain — collects all pending events.
    pub fn drain(&self, out: &mut Vec<OutputEvent>) {
        while let Ok(ev) = self.rx.try_recv() {
            out.push(ev);
        }
    }
}
