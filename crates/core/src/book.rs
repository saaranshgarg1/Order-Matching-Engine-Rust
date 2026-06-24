use std::collections::BTreeMap;
use std::cmp::Reverse;
use rustc_hash::FxHashMap;
use slab::Slab;

use crate::types::*;
use crate::order::Order;
use crate::price_level::PriceLevel;

#[derive(Debug, Clone)]
pub struct Location {
    pub price: Price,
    pub side: Side,
    pub slot: usize,
}

#[derive(Debug, Clone, Default)]
pub struct BookSnapshot {
    pub bids: Vec<(Price, Qty)>,
    pub asks: Vec<(Price, Qty)>,
    pub last_trade: Option<Price>,
}

/// Resting stop orders waiting for trigger price.
#[derive(Debug, Clone)]
pub struct StopEntry {
    pub slot: usize,
}

pub struct OrderBook {
    pub symbol: SymbolId,
    pub tick: Price,
    /// Asks sorted ascending (best = lowest). Key = price ticks.
    pub asks: BTreeMap<Price, PriceLevel>,
    /// Bids sorted descending (best = highest). Key = Reverse(price).
    pub bids: BTreeMap<Reverse<Price>, PriceLevel>,
    /// O(1) lookup for cancel/replace.
    pub id_index: FxHashMap<OrderId, Location>,
    /// Arena: all live orders live here. Index = slot.
    pub arena: Slab<Order>,
    /// Stop orders keyed by trigger price, per side.
    pub stop_buys: BTreeMap<Price, Vec<usize>>,   // trigger <= last_trade → activate
    pub stop_sells: BTreeMap<Reverse<Price>, Vec<usize>>, // trigger >= last_trade → activate
    pub last_trade: Option<Price>,
    pub seq: Seq,
}

impl OrderBook {
    pub fn new(symbol: SymbolId, tick: Price) -> Self {
        OrderBook {
            symbol,
            tick,
            asks: BTreeMap::new(),
            bids: BTreeMap::new(),
            id_index: FxHashMap::default(),
            arena: Slab::new(),
            stop_buys: BTreeMap::new(),
            stop_sells: BTreeMap::new(),
            last_trade: None,
            seq: 0,
        }
    }

    pub fn best_bid(&self) -> Option<(Price, Qty)> {
        self.bids.iter().next().map(|(Reverse(p), lvl)| (*p, lvl.total_qty))
    }

    pub fn best_ask(&self) -> Option<(Price, Qty)> {
        self.asks.iter().next().map(|(p, lvl)| (*p, lvl.total_qty))
    }

    pub fn depth(&self, levels: usize) -> BookSnapshot {
        let bids = self.bids.iter().take(levels)
            .map(|(Reverse(p), lvl)| (*p, lvl.total_qty))
            .collect();
        let asks = self.asks.iter().take(levels)
            .map(|(p, lvl)| (*p, lvl.total_qty))
            .collect();
        BookSnapshot { bids, asks, last_trade: self.last_trade }
    }

    /// Insert a resting order into the correct side.
    pub fn insert_resting(&mut self, slot: usize) {
        let order = &self.arena[slot];
        let price = order.price;
        let side = order.side;
        let qty = order.remaining;

        match side {
            Side::Buy => {
                let level = self.bids.entry(Reverse(price)).or_insert_with(PriceLevel::new);
                let old_tail = level.tail;
                level.push_back(slot, qty);
                // wire intrusive list
                self.arena[slot].prev = old_tail;
                self.arena[slot].next = None;
                if let Some(prev_slot) = old_tail {
                    self.arena[prev_slot].next = Some(slot);
                }
            }
            Side::Sell => {
                let level = self.asks.entry(price).or_insert_with(PriceLevel::new);
                let old_tail = level.tail;
                level.push_back(slot, qty);
                self.arena[slot].prev = old_tail;
                self.arena[slot].next = None;
                if let Some(prev_slot) = old_tail {
                    self.arena[prev_slot].next = Some(slot);
                }
            }
        }
    }

    /// Remove head of a bid level, returns slot index. Fixes head pointer.
    pub fn pop_bid_head(&mut self, price: Price) -> Option<usize> {
        let level = self.bids.get_mut(&Reverse(price))?;
        let slot = level.head?;
        let order_qty = self.arena[slot].remaining;
        level.pop_front(order_qty);
        // advance head
        let next = self.arena[slot].next;
        if let Some(lvl) = self.bids.get_mut(&Reverse(price)) {
            lvl.head = next;
            if next.is_none() { lvl.tail = None; }
        }
        if let Some(n) = next { self.arena[n].prev = None; }
        Some(slot)
    }

    /// Remove head of an ask level, returns slot index.
    pub fn pop_ask_head(&mut self, price: Price) -> Option<usize> {
        let level = self.asks.get_mut(&price)?;
        let slot = level.head?;
        let order_qty = self.arena[slot].remaining;
        level.pop_front(order_qty);
        let next = self.arena[slot].next;
        if let Some(lvl) = self.asks.get_mut(&price) {
            lvl.head = next;
            if next.is_none() { lvl.tail = None; }
        }
        if let Some(n) = next { self.arena[n].prev = None; }
        Some(slot)
    }

    pub fn remove_empty_bid(&mut self, price: Price) {
        if self.bids.get(&Reverse(price)).map_or(false, |l| l.is_empty()) {
            self.bids.remove(&Reverse(price));
        }
    }

    pub fn remove_empty_ask(&mut self, price: Price) {
        if self.asks.get(&price).map_or(false, |l| l.is_empty()) {
            self.asks.remove(&price);
        }
    }

    /// Cancel an order by slot index (O(1) list surgery).
    pub fn remove_slot(&mut self, slot: usize, side: Side, price: Price, qty: Qty) {
        let prev = self.arena[slot].prev;
        let next = self.arena[slot].next;
        // stitch neighbours
        if let Some(p) = prev { self.arena[p].next = next; }
        if let Some(n) = next { self.arena[n].prev = prev; }

        match side {
            Side::Buy => {
                if let Some(lvl) = self.bids.get_mut(&Reverse(price)) {
                    if lvl.head == Some(slot) { lvl.head = next; }
                    if lvl.tail == Some(slot) { lvl.tail = prev; }
                    lvl.remove(qty);
                    if lvl.is_empty() { self.bids.remove(&Reverse(price)); }
                }
            }
            Side::Sell => {
                if let Some(lvl) = self.asks.get_mut(&price) {
                    if lvl.head == Some(slot) { lvl.head = next; }
                    if lvl.tail == Some(slot) { lvl.tail = prev; }
                    lvl.remove(qty);
                    if lvl.is_empty() { self.asks.remove(&price); }
                }
            }
        }
    }

    /// Check invariant: no crossed book. Returns true if valid.
    pub fn check_no_cross(&self) -> bool {
        match (self.best_bid(), self.best_ask()) {
            (Some((bid, _)), Some((ask, _))) => bid < ask,
            _ => true,
        }
    }

    /// Sum of qty in id_index == sum across all levels (phantom-liquidity check).
    pub fn check_qty_conservation(&self) -> bool {
        let index_total: Qty = self.id_index.values()
            .map(|loc| self.arena[loc.slot].remaining)
            .sum();
        let bid_total: Qty = self.bids.values().map(|l| l.total_qty).sum();
        let ask_total: Qty = self.asks.values().map(|l| l.total_qty).sum();
        index_total == bid_total + ask_total
    }
}
