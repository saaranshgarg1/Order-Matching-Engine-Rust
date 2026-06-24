use std::sync::{Arc, Mutex};
use crossbeam_channel::{unbounded, Receiver, Sender, TrySendError};
use exchange_core::OutputEvent;

/// Fan-out egress bus.  Multiple subscribers can be added via `subscribe()`.
/// Shards call `send()` from OS threads (sync path, no blocking).
#[derive(Clone)]
pub struct EgressBus {
    subs: Arc<Mutex<Vec<Sender<OutputEvent>>>>,
}

pub struct EgressReceiver {
    rx: Receiver<OutputEvent>,
}

pub fn egress_bus() -> (EgressBus, EgressReceiver) {
    let bus = EgressBus { subs: Arc::new(Mutex::new(Vec::new())) };
    let rx  = bus.subscribe();
    (bus, rx)
}

impl EgressBus {
    /// Add a new subscriber.  Returns an EgressReceiver that gets every future event.
    pub fn subscribe(&self) -> EgressReceiver {
        let (tx, rx) = unbounded();
        self.subs.lock().unwrap().push(tx);
        EgressReceiver { rx }
    }

    /// Send one event to all current subscribers.  Drops dead receivers.
    #[inline]
    pub fn send(&self, event: OutputEvent) {
        let mut subs = self.subs.lock().unwrap();
        subs.retain(|tx| tx.send(event.clone()).is_ok());
    }

    pub fn send_batch(&self, events: &[OutputEvent]) {
        if events.is_empty() { return; }
        let mut subs = self.subs.lock().unwrap();
        subs.retain(|tx| {
            events.iter().all(|ev| tx.send(ev.clone()).is_ok())
        });
    }
}

impl EgressReceiver {
    /// Blocking receive.  Returns None when all senders are dropped.
    pub fn recv(&self) -> Option<OutputEvent> {
        self.rx.recv().ok()
    }

    /// Non-blocking drain.
    pub fn drain(&self, out: &mut Vec<OutputEvent>) {
        while let Ok(ev) = self.rx.try_recv() {
            out.push(ev);
        }
    }
}
