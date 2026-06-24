use std::cmp::Reverse;

use crate::types::*;
use crate::events::*;
use crate::order::Order;
use crate::book::OrderBook;

/// Pure, deterministic apply. No I/O, no clock reads, no allocation beyond `out`.
/// Same (book_state, cmd) always → same (new_state, events).
pub fn apply(book: &mut OrderBook, cmd: &Sequenced, out: &mut Vec<OutputEvent>) {
    match &cmd.cmd {
        Command::New(no) => apply_new(book, no, cmd.seq, cmd.ts, out),
        Command::Cancel { id, .. } => apply_cancel(book, *id, cmd.seq, out),
        Command::Replace { id, new_price, new_qty, .. } => {
            apply_replace(book, *id, *new_price, *new_qty, cmd.seq, cmd.ts, out)
        }
    }
}

fn apply_new(
    book: &mut OrderBook,
    no: &NewOrder,
    seq: Seq,
    ts: Ts,
    out: &mut Vec<OutputEvent>,
) {
    // --- validation ---
    if no.qty == 0 {
        out.push(OutputEvent::Rejected { id: no.id, reason: RejectReason::ZeroQty, seq });
        return;
    }
    if book.id_index.contains_key(&no.id) {
        out.push(OutputEvent::Rejected { id: no.id, reason: RejectReason::DuplicateId, seq });
        return;
    }
    if matches!(no.kind, OrderType::Limit | OrderType::Ioc | OrderType::Fok | OrderType::StopLimit)
        && no.price <= 0
    {
        out.push(OutputEvent::Rejected { id: no.id, reason: RejectReason::BadPrice, seq });
        return;
    }

    // --- stop orders: hold until triggered ---
    if matches!(no.kind, OrderType::StopMarket | OrderType::StopLimit) {
        let slot = book.arena.insert(Order::new(
            no.id, no.symbol, no.side, no.kind, no.tif,
            no.price, no.stop_price, no.qty, seq, ts,
        ));
        book.id_index.insert(no.id, crate::book::Location {
            price: no.stop_price,
            side: no.side,
            slot,
        });
        match no.side {
            Side::Buy => book.stop_buys.entry(no.stop_price).or_default().push(slot),
            Side::Sell => book.stop_sells.entry(Reverse(no.stop_price)).or_default().push(slot),
        }
        out.push(OutputEvent::Accepted { id: no.id, seq });
        return;
    }

    out.push(OutputEvent::Accepted { id: no.id, seq });

    // --- FOK pre-check ---
    if no.kind == OrderType::Fok {
        let available = available_qty(book, no.side, no.price);
        if available < no.qty {
            out.push(OutputEvent::Rejected { id: no.id, reason: RejectReason::FokUnfillable, seq });
            return;
        }
    }

    // --- insert into arena ---
    let slot = book.arena.insert(Order::new(
        no.id, no.symbol, no.side, no.kind, no.tif,
        no.price, no.stop_price, no.qty, seq, ts,
    ));

    // --- match ---
    let filled = match_order(book, slot, seq, ts, out);
    let remaining = book.arena[slot].remaining;

    if filled > 0 && remaining == 0 {
        out.push(OutputEvent::Filled { id: no.id, seq });
        book.arena.remove(slot);
        book.id_index.remove(&no.id);
    } else if filled > 0 {
        out.push(OutputEvent::PartiallyFilled { id: no.id, filled, remaining, seq });
        // decide what to do with remainder
        if matches!(no.kind, OrderType::Limit) {
            // resting limit: add to book
            book.id_index.insert(no.id, crate::book::Location { price: no.price, side: no.side, slot });
            book.insert_resting(slot);
        } else {
            // IOC / Market / FOK (already pre-checked): cancel remainder
            book.arena.remove(slot);
            out.push(OutputEvent::Cancelled { id: no.id, seq });
        }
    } else {
        // zero fills
        match no.kind {
            OrderType::Limit => {
                book.id_index.insert(no.id, crate::book::Location { price: no.price, side: no.side, slot });
                book.insert_resting(slot);
            }
            OrderType::Market | OrderType::Ioc | OrderType::Fok => {
                book.arena.remove(slot);
                out.push(OutputEvent::Cancelled { id: no.id, seq });
            }
            _ => {}
        }
    }

    // --- check stop triggers ---
    if let Some(last) = book.last_trade {
        trigger_stops(book, last, seq, ts, out);
    }
}

/// Walk the opposite side and fill as much as possible. Returns total filled qty.
fn match_order(
    book: &mut OrderBook,
    aggressor_slot: usize,
    seq: Seq,
    ts: Ts,
    out: &mut Vec<OutputEvent>,
) -> Qty {
    let side = book.arena[aggressor_slot].side;
    let kind = book.arena[aggressor_slot].kind;
    let limit = book.arena[aggressor_slot].price;
    let mut total_filled: Qty = 0;

    loop {
        if book.arena[aggressor_slot].remaining == 0 { break; }

        // find best resting price on opposite side
        let (maker_price, crosses) = match side {
            Side::Buy => {
                if let Some((p, _)) = book.asks.iter().next().map(|(p, l)| (*p, l)) {
                    let ok = kind == OrderType::Market || kind == OrderType::Ioc
                          || kind == OrderType::Fok  || p <= limit;
                    (p, ok)
                } else { break; }
            }
            Side::Sell => {
                if let Some((Reverse(p), _)) = book.bids.iter().next().map(|(p, l)| (*p, l)) {
                    let ok = kind == OrderType::Market || kind == OrderType::Ioc
                          || kind == OrderType::Fok  || p >= limit;
                    (p, ok)
                } else { break; }
            }
        };

        if !crosses { break; }

        // pop head of resting level
        let maker_slot = match side {
            Side::Buy  => match book.pop_ask_head(maker_price) { Some(s) => s, None => break },
            Side::Sell => match book.pop_bid_head(maker_price) { Some(s) => s, None => break },
        };

        let trade_qty = book.arena[aggressor_slot].remaining
            .min(book.arena[maker_slot].remaining);

        book.arena[aggressor_slot].remaining -= trade_qty;
        book.arena[maker_slot].remaining     -= trade_qty;
        total_filled += trade_qty;
        book.last_trade = Some(maker_price);

        out.push(OutputEvent::Trade {
            seq,
            taker: book.arena[aggressor_slot].id,
            maker: book.arena[maker_slot].id,
            price: maker_price,  // maker sets the price
            qty: trade_qty,
            side,
            ts,
        });

        if book.arena[maker_slot].remaining == 0 {
            // maker fully filled
            let maker_id = book.arena[maker_slot].id;
            out.push(OutputEvent::Filled { id: maker_id, seq });
            book.id_index.remove(&maker_id);
            book.arena.remove(maker_slot);
            match side {
                Side::Buy  => book.remove_empty_ask(maker_price),
                Side::Sell => book.remove_empty_bid(maker_price),
            }
        } else {
            // maker partially filled — put it back at the head
            let maker_id = book.arena[maker_slot].id;
            let maker_remaining = book.arena[maker_slot].remaining;
            out.push(OutputEvent::PartiallyFilled {
                id: maker_id,
                filled: trade_qty,
                remaining: maker_remaining,
                seq,
            });
            // re-insert at head of its level (it keeps priority)
            reinsert_at_head(book, maker_slot, maker_price, side.opposite());
        }
    }

    total_filled
}

/// Re-attach a partially-filled maker at the head of its price level.
fn reinsert_at_head(book: &mut OrderBook, slot: usize, price: Price, side: Side) {
    book.arena[slot].prev = None;
    match side {
        Side::Buy => {
            let level = book.bids.entry(Reverse(price)).or_insert_with(crate::price_level::PriceLevel::new);
            let old_head = level.head;
            level.head = Some(slot);
            if level.tail.is_none() { level.tail = Some(slot); }
            level.total_qty += book.arena[slot].remaining;
            level.count += 1;
            book.arena[slot].next = old_head;
            if let Some(h) = old_head { book.arena[h].prev = Some(slot); }
        }
        Side::Sell => {
            let level = book.asks.entry(price).or_insert_with(crate::price_level::PriceLevel::new);
            let old_head = level.head;
            level.head = Some(slot);
            if level.tail.is_none() { level.tail = Some(slot); }
            level.total_qty += book.arena[slot].remaining;
            level.count += 1;
            book.arena[slot].next = old_head;
            if let Some(h) = old_head { book.arena[h].prev = Some(slot); }
        }
    }
}

/// How much qty on the given side crosses at/through `limit_price`.
fn available_qty(book: &OrderBook, aggressor_side: Side, limit_price: Price) -> Qty {
    let mut total: Qty = 0;
    match aggressor_side {
        Side::Buy => {
            for (p, lvl) in &book.asks {
                if *p > limit_price { break; }
                total = total.saturating_add(lvl.total_qty);
            }
        }
        Side::Sell => {
            for (Reverse(p), lvl) in &book.bids {
                if *p < limit_price { break; }
                total = total.saturating_add(lvl.total_qty);
            }
        }
    }
    total
}

fn apply_cancel(book: &mut OrderBook, id: OrderId, seq: Seq, out: &mut Vec<OutputEvent>) {
    let loc = match book.id_index.remove(&id) {
        Some(l) => l,
        None => {
            out.push(OutputEvent::Rejected { id, reason: RejectReason::UnknownId, seq });
            return;
        }
    };
    let qty = book.arena[loc.slot].remaining;
    book.remove_slot(loc.slot, loc.side, loc.price, qty);
    book.arena.remove(loc.slot);
    out.push(OutputEvent::Cancelled { id, seq });
}

fn apply_replace(
    book: &mut OrderBook,
    id: OrderId,
    new_price: Price,
    new_qty: Qty,
    seq: Seq,
    ts: Ts,
    out: &mut Vec<OutputEvent>,
) {
    if new_qty == 0 {
        out.push(OutputEvent::Rejected { id, reason: RejectReason::ZeroQty, seq });
        return;
    }
    // Cancel-replace: cancel existing then submit new.
    let loc = match book.id_index.remove(&id) {
        Some(l) => l,
        None => {
            out.push(OutputEvent::Rejected { id, reason: RejectReason::UnknownId, seq });
            return;
        }
    };
    let old_qty = book.arena[loc.slot].remaining;
    let side = book.arena[loc.slot].side;
    let kind = book.arena[loc.slot].kind;
    let tif  = book.arena[loc.slot].tif;
    let symbol = book.arena[loc.slot].symbol;

    book.remove_slot(loc.slot, side, loc.price, old_qty);
    book.arena.remove(loc.slot);

    out.push(OutputEvent::Replaced { id, seq });

    // Re-submit as new order with same id — loses time priority (correct for replace).
    let no = NewOrder { id, symbol, side, kind, tif, price: new_price,
                        stop_price: 0, qty: new_qty };
    apply_new(book, &no, seq, ts, out);
}

fn trigger_stops(book: &mut OrderBook, last: Price, seq: Seq, ts: Ts, out: &mut Vec<OutputEvent>) {
    // Buy stops trigger when last_trade >= stop_price (price rose through trigger).
    let triggered_buys: Vec<usize> = book.stop_buys
        .range(..=last)
        .flat_map(|(_, v)| v.iter().copied())
        .collect();

    for slot in triggered_buys {
        if !book.arena.contains(slot) { continue; }
        let order = book.arena[slot].clone();
        book.id_index.remove(&order.id);
        book.arena.remove(slot);

        let no = NewOrder {
            id: order.id, symbol: order.symbol, side: order.side,
            kind: if order.kind == OrderType::StopMarket { OrderType::Market } else { OrderType::Limit },
            tif: order.tif, price: order.price, stop_price: 0, qty: order.remaining,
        };
        apply_new(book, &no, seq, ts, out);
    }
    book.stop_buys.retain(|p, _| *p > last);

    // Sell stops trigger when last_trade <= stop_price (price fell through trigger).
    let triggered_sells: Vec<usize> = book.stop_sells
        .range(..=Reverse(last))
        .flat_map(|(_, v)| v.iter().copied())
        .collect();

    for slot in triggered_sells {
        if !book.arena.contains(slot) { continue; }
        let order = book.arena[slot].clone();
        book.id_index.remove(&order.id);
        book.arena.remove(slot);

        let no = NewOrder {
            id: order.id, symbol: order.symbol, side: order.side,
            kind: if order.kind == OrderType::StopMarket { OrderType::Market } else { OrderType::Limit },
            tif: order.tif, price: order.price, stop_price: 0, qty: order.remaining,
        };
        apply_new(book, &no, seq, ts, out);
    }
    book.stop_sells.retain(|Reverse(p), _| *p < last);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> OrderBook { OrderBook::new(1, 1) }

    fn seq_cmd(seq: Seq, cmd: Command) -> Sequenced {
        Sequenced { seq, ts: seq * 1000, cmd }
    }

    fn new_limit(id: OrderId, side: Side, price: Price, qty: Qty) -> Command {
        Command::New(NewOrder {
            id, symbol: 1, side, kind: OrderType::Limit,
            tif: TimeInForce::Gtc, price, stop_price: 0, qty,
        })
    }

    fn new_market(id: OrderId, side: Side, qty: Qty) -> Command {
        Command::New(NewOrder {
            id, symbol: 1, side, kind: OrderType::Market,
            tif: TimeInForce::Gtc, price: 0, stop_price: 0, qty,
        })
    }

    fn new_fok(id: OrderId, side: Side, price: Price, qty: Qty) -> Command {
        Command::New(NewOrder {
            id, symbol: 1, side, kind: OrderType::Fok,
            tif: TimeInForce::Gtc, price, stop_price: 0, qty,
        })
    }

    #[test]
    fn no_cross_no_trade() {
        let mut b = book();
        let mut out = vec![];
        apply(&mut b, &seq_cmd(1, new_limit(1, Side::Buy,  100, 10)), &mut out);
        apply(&mut b, &seq_cmd(2, new_limit(2, Side::Sell, 101, 10)), &mut out);
        assert!(!out.iter().any(|e| matches!(e, OutputEvent::Trade { .. })));
        assert_eq!(b.best_bid(), Some((100, 10)));
        assert_eq!(b.best_ask(), Some((101, 10)));
    }

    #[test]
    fn simple_full_cross() {
        let mut b = book();
        let mut out = vec![];
        apply(&mut b, &seq_cmd(1, new_limit(1, Side::Sell, 100, 10)), &mut out);
        apply(&mut b, &seq_cmd(2, new_limit(2, Side::Buy,  100, 10)), &mut out);
        let trades: Vec<_> = out.iter().filter(|e| matches!(e, OutputEvent::Trade { .. })).collect();
        assert_eq!(trades.len(), 1);
        if let OutputEvent::Trade { price, qty, .. } = trades[0] {
            assert_eq!(*price, 100);
            assert_eq!(*qty, 10);
        }
        assert!(b.best_bid().is_none());
        assert!(b.best_ask().is_none());
        assert!(b.check_no_cross());
    }

    #[test]
    fn partial_fill() {
        let mut b = book();
        let mut out = vec![];
        apply(&mut b, &seq_cmd(1, new_limit(1, Side::Sell, 100, 5)), &mut out);
        apply(&mut b, &seq_cmd(2, new_limit(2, Side::Buy,  100, 10)), &mut out);
        let trades: Vec<_> = out.iter().filter(|e| matches!(e, OutputEvent::Trade { .. })).collect();
        assert_eq!(trades.len(), 1);
        assert_eq!(b.best_bid(), Some((100, 5)));
        assert!(b.best_ask().is_none());
    }

    #[test]
    fn price_time_priority() {
        let mut b = book();
        let mut out = vec![];
        // Two resting sells at same price; earlier should fill first.
        apply(&mut b, &seq_cmd(1, new_limit(1, Side::Sell, 100, 5)), &mut out);
        apply(&mut b, &seq_cmd(2, new_limit(2, Side::Sell, 100, 5)), &mut out);
        apply(&mut b, &seq_cmd(3, new_limit(3, Side::Buy,  100, 5)), &mut out);
        let trade = out.iter().find(|e| matches!(e, OutputEvent::Trade { .. })).unwrap();
        if let OutputEvent::Trade { maker, .. } = trade {
            assert_eq!(*maker, 1); // order 1 was first → fills first
        }
    }

    #[test]
    fn cancel_removes_order() {
        let mut b = book();
        let mut out = vec![];
        apply(&mut b, &seq_cmd(1, new_limit(1, Side::Buy, 100, 10)), &mut out);
        apply(&mut b, &seq_cmd(2, Command::Cancel { id: 1, symbol: 1 }), &mut out);
        assert!(b.best_bid().is_none());
        assert!(b.check_qty_conservation());
    }

    #[test]
    fn cancel_unknown_is_reject() {
        let mut b = book();
        let mut out = vec![];
        apply(&mut b, &seq_cmd(1, Command::Cancel { id: 999, symbol: 1 }), &mut out);
        assert!(out.iter().any(|e| matches!(e, OutputEvent::Rejected { reason: RejectReason::UnknownId, .. })));
    }

    #[test]
    fn fok_full_fill() {
        let mut b = book();
        let mut out = vec![];
        apply(&mut b, &seq_cmd(1, new_limit(1, Side::Sell, 100, 10)), &mut out);
        apply(&mut b, &seq_cmd(2, new_fok(2, Side::Buy, 100, 10)), &mut out);
        assert!(out.iter().any(|e| matches!(e, OutputEvent::Filled { id: 2, .. })));
    }

    #[test]
    fn fok_unfillable_zero_trades() {
        let mut b = book();
        let mut out = vec![];
        apply(&mut b, &seq_cmd(1, new_limit(1, Side::Sell, 100, 5)), &mut out);
        apply(&mut b, &seq_cmd(2, new_fok(2, Side::Buy, 100, 10)), &mut out);
        assert!(!out.iter().any(|e| matches!(e, OutputEvent::Trade { .. })));
        assert!(out.iter().any(|e| matches!(e, OutputEvent::Rejected { reason: RejectReason::FokUnfillable, .. })));
    }

    #[test]
    fn market_order_walks_book() {
        let mut b = book();
        let mut out = vec![];
        apply(&mut b, &seq_cmd(1, new_limit(1, Side::Sell, 100, 5)), &mut out);
        apply(&mut b, &seq_cmd(2, new_limit(2, Side::Sell, 101, 5)), &mut out);
        out.clear();
        apply(&mut b, &seq_cmd(3, new_market(3, Side::Buy, 8)), &mut out);
        let trades: Vec<_> = out.iter().filter(|e| matches!(e, OutputEvent::Trade { .. })).collect();
        assert_eq!(trades.len(), 2);
    }

    #[test]
    fn no_cross_invariant_after_random_ops() {
        let mut b = book();
        let mut out = vec![];
        for i in 1..=20u64 {
            let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
            let price = 100 + (i as i64 % 5) * (if side == Side::Buy { -1 } else { 1 });
            apply(&mut b, &seq_cmd(i, new_limit(i, side, price, 10)), &mut out);
            assert!(b.check_no_cross(), "book crossed after op {}", i);
        }
    }

    #[test]
    fn qty_conservation_after_ops() {
        let mut b = book();
        let mut out = vec![];
        apply(&mut b, &seq_cmd(1, new_limit(1, Side::Buy,  99, 10)), &mut out);
        apply(&mut b, &seq_cmd(2, new_limit(2, Side::Buy, 100, 5)),  &mut out);
        apply(&mut b, &seq_cmd(3, new_limit(3, Side::Sell,101, 8)),  &mut out);
        apply(&mut b, &seq_cmd(4, new_limit(4, Side::Sell,100, 3)),  &mut out);
        assert!(b.check_qty_conservation());
    }
}
