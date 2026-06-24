use crate::types::*;

#[derive(Debug, Clone)]
pub struct NewOrder {
    pub id: OrderId,
    pub symbol: SymbolId,
    pub side: Side,
    pub kind: OrderType,
    pub tif: TimeInForce,
    pub price: Price,
    pub stop_price: Price,
    pub qty: Qty,
}

#[derive(Debug, Clone)]
pub enum Command {
    New(NewOrder),
    Cancel { id: OrderId, symbol: SymbolId },
    Replace { id: OrderId, new_price: Price, new_qty: Qty, symbol: SymbolId },
}

#[derive(Debug, Clone)]
pub struct Sequenced {
    pub seq: Seq,
    pub ts: Ts,
    pub cmd: Command,
}

#[derive(Debug, Clone)]
pub enum OutputEvent {
    Accepted { id: OrderId, seq: Seq },
    Rejected { id: OrderId, reason: RejectReason, seq: Seq },
    Trade {
        seq: Seq,
        taker: OrderId,
        maker: OrderId,
        price: Price,
        qty: Qty,
        side: Side,
        ts: Ts,
    },
    PartiallyFilled { id: OrderId, filled: Qty, remaining: Qty, seq: Seq },
    Filled { id: OrderId, seq: Seq },
    Cancelled { id: OrderId, seq: Seq },
    Replaced { id: OrderId, seq: Seq },
}
