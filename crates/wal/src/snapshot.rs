use std::fs;
use std::path::{Path, PathBuf};

use exchange_core::{OrderBook, Price, Qty, OrderId, Seq, Side, OrderType, TimeInForce, OrderStatus, SymbolId};
use exchange_core::order::Order;
use serde::{Deserialize, Serialize};
use crate::WalError;

/// Serialisable snapshot of book state. Rebuilt from slab + id_index.
#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub symbol:        SymbolId,
    pub tick:          Price,
    pub last_seq:      Seq,
    pub last_trade:    Option<Price>,
    pub resting_orders: Vec<SnapOrder>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SnapOrder {
    pub id:         OrderId,
    pub side:       u8,   // 0=Buy,1=Sell
    pub kind:       u8,
    pub tif:        u8,
    pub price:      Price,
    pub stop_price: Price,
    pub qty:        Qty,
    pub remaining:  Qty,
    pub seq:        Seq,
    pub ts:         u64,
    pub status:     u8,
}

pub fn save_snapshot(book: &OrderBook, last_seq: Seq, dir: &Path) -> Result<PathBuf, WalError> {
    fs::create_dir_all(dir)?;

    let resting_orders: Vec<SnapOrder> = book.id_index.values()
        .map(|loc| {
            let o = &book.arena[loc.slot];
            SnapOrder {
                id:         o.id,
                side:       match o.side { Side::Buy => 0, Side::Sell => 1 },
                kind:       match o.kind {
                    OrderType::Limit      => 0,
                    OrderType::Market     => 1,
                    OrderType::Ioc        => 2,
                    OrderType::Fok        => 3,
                    OrderType::StopMarket => 4,
                    OrderType::StopLimit  => 5,
                },
                tif:        match o.tif { TimeInForce::Gtc => 0, TimeInForce::Day => 1 },
                price:      o.price,
                stop_price: o.stop_price,
                qty:        o.qty,
                remaining:  o.remaining,
                seq:        o.seq,
                ts:         o.ts,
                status:     match o.status {
                    OrderStatus::New             => 0,
                    OrderStatus::Accepted        => 1,
                    OrderStatus::PartiallyFilled => 2,
                    OrderStatus::Filled          => 3,
                    OrderStatus::Cancelled       => 4,
                    OrderStatus::Rejected        => 5,
                },
            }
        })
        .collect();

    let snap = Snapshot {
        symbol:  book.symbol,
        tick:    book.tick,
        last_seq,
        last_trade: book.last_trade,
        resting_orders,
    };

    let path = dir.join(format!("snapshot-{:010}.json", last_seq));
    let json = serde_json::to_string(&snap)
        .map_err(|e| WalError::Serialise(e.to_string()))?;
    fs::write(&path, json)?;
    Ok(path)
}

pub fn load_snapshot(path: &Path) -> Result<(OrderBook, Seq), WalError> {
    let json = fs::read_to_string(path)?;
    let snap: Snapshot = serde_json::from_str(&json)
        .map_err(|e| WalError::Serialise(e.to_string()))?;

    let mut book = OrderBook::new(snap.symbol, snap.tick);
    book.last_trade = snap.last_trade;

    use exchange_core::events::{Command, NewOrder};
    use exchange_core::Sequenced;

    for so in snap.resting_orders {
        let side = if so.side == 0 { Side::Buy } else { Side::Sell };
        let kind = match so.kind {
            0 => OrderType::Limit,
            1 => OrderType::Market,
            2 => OrderType::Ioc,
            3 => OrderType::Fok,
            4 => OrderType::StopMarket,
            _ => OrderType::StopLimit,
        };
        let tif = if so.tif == 0 { TimeInForce::Gtc } else { TimeInForce::Day };

        // Re-insert resting orders directly into arena + index.
        // Use apply() with a special Limit order — but easier: insert directly.
        let slot = book.arena.insert(Order::new(
            so.id, snap.symbol, side, kind, tif,
            so.price, so.stop_price, so.remaining, so.seq, so.ts,
        ));
        // adjust remaining (Order::new sets remaining = qty)
        book.arena[slot].qty = so.qty;
        book.arena[slot].remaining = so.remaining;

        book.id_index.insert(so.id, exchange_core::book::Location {
            price: so.price,
            side,
            slot,
        });
        book.insert_resting(slot);
    }

    Ok((book, snap.last_seq))
}

/// Find the most recent snapshot file in dir, if any.
pub fn latest_snapshot(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map_or(false, |n| n.starts_with("snapshot-") && n.ends_with(".json"))
        })
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;
    use exchange_core::{apply, Command, NewOrder, OrderType, TimeInForce, Side, Sequenced};
    use tempfile::tempdir;

    fn seq_cmd(seq: u64, cmd: Command) -> Sequenced {
        Sequenced { seq, ts: seq * 100, cmd }
    }

    fn new_limit(id: u64, side: Side, price: i64, qty: u64) -> Command {
        Command::New(NewOrder {
            id, symbol: 1, side, kind: OrderType::Limit,
            tif: TimeInForce::Gtc, price, stop_price: 0, qty,
        })
    }

    #[test]
    fn snapshot_round_trip() {
        let mut book = OrderBook::new(1, 1);
        let mut out = vec![];
        apply(&mut book, &seq_cmd(1, new_limit(1, Side::Buy,  99, 10)), &mut out);
        apply(&mut book, &seq_cmd(2, new_limit(2, Side::Sell, 101, 5)), &mut out);

        let dir = tempdir().unwrap();
        let path = save_snapshot(&book, 2, dir.path()).unwrap();
        let (book2, last_seq) = load_snapshot(&path).unwrap();

        assert_eq!(last_seq, 2);
        assert_eq!(book2.best_bid(), Some((99, 10)));
        assert_eq!(book2.best_ask(), Some((101, 5)));
    }
}
