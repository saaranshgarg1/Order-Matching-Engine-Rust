use exchange_core::{Command, NewOrder, RejectReason};
use protocol::{JsonInbound, symbol_to_id, dollars_to_ticks};

/// Convert a parsed JSON inbound message into an engine Command.
pub fn json_to_command(msg: JsonInbound) -> Option<Command> {
    match msg {
        JsonInbound::New(no) => Some(Command::New(NewOrder::from(no))),
        JsonInbound::Cancel(c) => Some(Command::Cancel {
            id:     c.id,
            symbol: symbol_to_id(&c.symbol),
        }),
        JsonInbound::Replace(r) => Some(Command::Replace {
            id:        r.id,
            symbol:    symbol_to_id(&r.symbol),
            new_price: dollars_to_ticks(r.new_price),
            new_qty:   r.new_qty,
        }),
    }
}

/// Map a RejectReason to a human-readable string for JSON clients.
pub fn reject_reason_str(r: RejectReason) -> &'static str {
    match r {
        RejectReason::UnknownSymbol    => "unknown_symbol",
        RejectReason::DuplicateId      => "duplicate_id",
        RejectReason::UnknownId        => "unknown_id",
        RejectReason::ZeroQty          => "zero_qty",
        RejectReason::BadPrice         => "bad_price",
        RejectReason::FokUnfillable    => "fok_unfillable",
        RejectReason::MarketNoLiquidity=> "no_liquidity",
        RejectReason::SelfTrade        => "self_trade",
        RejectReason::RateLimited      => "rate_limited",
    }
}
