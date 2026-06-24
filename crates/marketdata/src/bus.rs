use std::sync::Arc;
use tokio::sync::broadcast;
use serde::{Deserialize, Serialize};
use exchange_core::{Price, Qty, SymbolId};

/// Public market-data event broadcast to all subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum MarketEvent {
    Trade {
        symbol: String,
        price:  f64,
        qty:    u64,
        side:   String,
        seq:    u64,
        ts:     u64,
    },
    Snapshot {
        symbol: String,
        seq:    u64,
        bids:   Vec<[f64; 2]>,
        asks:   Vec<[f64; 2]>,
    },
    Delta {
        symbol: String,
        seq:    u64,
        bids:   Vec<[f64; 2]>,
        asks:   Vec<[f64; 2]>,
    },
}

/// Fan-out broadcast bus. Clone the sender to publish; clone the bus to subscribe.
#[derive(Clone)]
pub struct MarketBus {
    tx: broadcast::Sender<MarketEvent>,
}

pub struct MarketReceiver {
    rx: broadcast::Receiver<MarketEvent>,
}

pub fn market_bus(capacity: usize) -> (MarketBus, MarketReceiver) {
    let (tx, rx) = broadcast::channel(capacity);
    (MarketBus { tx }, MarketReceiver { rx })
}

impl MarketBus {
    pub fn publish(&self, event: MarketEvent) {
        // Best-effort: lagged receivers are dropped.
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> MarketReceiver {
        MarketReceiver { rx: self.tx.subscribe() }
    }
}

impl MarketReceiver {
    pub async fn recv(&mut self) -> Option<MarketEvent> {
        loop {
            match self.rx.recv().await {
                Ok(ev) => return Some(ev),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return None,
            }
        }
    }
}
