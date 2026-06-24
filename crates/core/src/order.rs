use crate::types::*;

#[derive(Debug, Clone)]
pub struct Order {
    pub id: OrderId,
    pub symbol: SymbolId,
    pub side: Side,
    pub kind: OrderType,
    pub tif: TimeInForce,
    pub price: Price,
    pub stop_price: Price,
    pub qty: Qty,
    pub remaining: Qty,
    pub seq: Seq,
    pub ts: Ts,
    pub status: OrderStatus,
    /// index of next order in same price level (intrusive FIFO list)
    pub next: Option<usize>,
    /// index of prev order in same price level
    pub prev: Option<usize>,
}

impl Order {
    pub fn new(
        id: OrderId,
        symbol: SymbolId,
        side: Side,
        kind: OrderType,
        tif: TimeInForce,
        price: Price,
        stop_price: Price,
        qty: Qty,
        seq: Seq,
        ts: Ts,
    ) -> Self {
        Order {
            id, symbol, side, kind, tif, price, stop_price,
            qty, remaining: qty, seq, ts,
            status: OrderStatus::New,
            next: None,
            prev: None,
        }
    }

    pub fn is_resting(&self) -> bool {
        matches!(self.kind, OrderType::Limit | OrderType::StopLimit)
            && matches!(self.tif, TimeInForce::Gtc | TimeInForce::Day)
    }
}
