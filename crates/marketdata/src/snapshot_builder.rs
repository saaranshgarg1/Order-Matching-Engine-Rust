use std::collections::BTreeMap;
use std::cmp::Reverse;
use exchange_core::{OutputEvent, Price, Qty, SymbolId};
use protocol::ticks_to_dollars;
use crate::bus::{MarketBus, MarketEvent};

/// Maintains a live L2 view per symbol and emits deltas + periodic snapshots.
pub struct SnapshotBuilder {
    symbol:    SymbolId,
    sym_str:   String,
    bids:      BTreeMap<Reverse<Price>, Qty>,
    asks:      BTreeMap<Price, Qty>,
    bus:       MarketBus,
    depth:     usize,
}

impl SnapshotBuilder {
    pub fn new(symbol: SymbolId, sym_str: String, bus: MarketBus, depth: usize) -> Self {
        SnapshotBuilder { symbol, sym_str, bids: BTreeMap::new(), asks: BTreeMap::new(), bus, depth }
    }

    /// Process an output event and publish market-data messages.
    pub fn process(&mut self, event: &OutputEvent, seq: u64) {
        match event {
            OutputEvent::Trade { price, qty, side, ts, taker, maker, seq: ev_seq } => {
                let side_str = match side {
                    exchange_core::Side::Buy  => "buy",
                    exchange_core::Side::Sell => "sell",
                }.to_string();
                self.bus.publish(MarketEvent::Trade {
                    symbol: self.sym_str.clone(),
                    price:  ticks_to_dollars(*price),
                    qty:    *qty,
                    side:   side_str,
                    seq,
                    ts:     *ts,
                });
            }
            // Resting order added → update L2
            OutputEvent::Accepted { .. } => {}
            // Fills adjust L2 — we track via explicit Filled/PartiallyFilled
            OutputEvent::Filled { .. } | OutputEvent::PartiallyFilled { .. } => {
                // Actual qty changes come via the book; for demo we emit a full snapshot.
                self.publish_snapshot(seq);
            }
            OutputEvent::Cancelled { .. } => {
                self.publish_snapshot(seq);
            }
            _ => {}
        }
    }

    pub fn publish_snapshot(&self, seq: u64) {
        let bids: Vec<[f64; 2]> = self.bids.iter().take(self.depth)
            .map(|(Reverse(p), q)| [ticks_to_dollars(*p), *q as f64])
            .collect();
        let asks: Vec<[f64; 2]> = self.asks.iter().take(self.depth)
            .map(|(p, q)| [ticks_to_dollars(*p), *q as f64])
            .collect();
        self.bus.publish(MarketEvent::Snapshot {
            symbol: self.sym_str.clone(),
            seq,
            bids,
            asks,
        });
    }

    /// Apply a book delta directly (called when we have direct book access).
    pub fn update_level(&mut self, side: exchange_core::Side, price: Price, new_qty: Qty) {
        match side {
            exchange_core::Side::Buy => {
                if new_qty == 0 { self.bids.remove(&Reverse(price)); }
                else { self.bids.insert(Reverse(price), new_qty); }
            }
            exchange_core::Side::Sell => {
                if new_qty == 0 { self.asks.remove(&price); }
                else { self.asks.insert(price, new_qty); }
            }
        }
    }
}
