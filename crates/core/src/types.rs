pub type Price = i64;
pub type Qty = u64;
pub type OrderId = u64;
pub type Seq = u64;
pub type Ts = u64;
pub type SymbolId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OrderType {
    Limit,
    Market,
    Ioc,
    Fok,
    StopMarket,
    StopLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TimeInForce {
    Gtc,
    Day,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    New,
    Accepted,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    UnknownSymbol,
    DuplicateId,
    UnknownId,
    ZeroQty,
    BadPrice,
    FokUnfillable,
    MarketNoLiquidity,
    SelfTrade,
    RateLimited,
}
