use serde::{Deserialize, Serialize};
use exchange_core::{NewOrder, OrderType, Side, TimeInForce, SymbolId};
use crate::error::ProtoError;

const TICK_SCALE: i64 = 10_000; // 4 decimal places: $1.0000 = 10_000 ticks

/// Convert human-readable dollar price to integer ticks.
pub fn dollars_to_ticks(d: f64) -> i64 {
    (d * TICK_SCALE as f64).round() as i64
}

pub fn ticks_to_dollars(t: i64) -> f64 {
    t as f64 / TICK_SCALE as f64
}

// ─── inbound JSON ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum JsonInbound {
    New(JsonNewOrder),
    Cancel(JsonCancel),
    Replace(JsonReplace),
}

#[derive(Debug, Deserialize)]
pub struct JsonNewOrder {
    pub id:     u64,
    pub symbol: String,
    pub side:   JsonSide,
    #[serde(rename = "type")]
    pub kind:   JsonOrderType,
    #[serde(default)]
    pub tif:    JsonTif,
    #[serde(default)]
    pub price:  f64,
    #[serde(default)]
    pub stop_price: f64,
    pub qty:    u64,
}

#[derive(Debug, Deserialize)]
pub struct JsonCancel {
    pub id:     u64,
    pub symbol: String,
}

#[derive(Debug, Deserialize)]
pub struct JsonReplace {
    pub id:        u64,
    pub symbol:    String,
    pub new_price: f64,
    pub new_qty:   u64,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonSide { Buy, Sell }

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonOrderType { Limit, Market, Ioc, Fok, StopMarket, StopLimit }

#[derive(Debug, Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum JsonTif { #[default] Gtc, Day }

// ─── outbound JSON ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum JsonOutbound {
    Ack     { id: u64, seq: u64 },
    Reject  { id: u64, seq: u64, reason: String },
    Trade   { taker: u64, maker: u64, price: f64, qty: u64, side: String, seq: u64 },
    Fill    { id: u64, price: f64, qty: u64, leaves: u64, seq: u64 },
    Cancelled { id: u64, seq: u64 },
    Replaced  { id: u64, seq: u64 },
    Snapshot  { symbol: String, seq: u64, bids: Vec<[f64; 2]>, asks: Vec<[f64; 2]> },
    Delta     { symbol: String, seq: u64, bids: Vec<[f64; 2]>, asks: Vec<[f64; 2]> },
    MarketTrade { symbol: String, price: f64, qty: u64, side: String, seq: u64, ts: u64 },
}

// ─── parse / serialise ───────────────────────────────────────────────────────

pub fn parse_inbound(s: &str) -> Result<JsonInbound, ProtoError> {
    Ok(serde_json::from_str(s)?)
}

pub fn serialize_outbound(msg: &JsonOutbound) -> Result<String, ProtoError> {
    Ok(serde_json::to_string(msg)?)
}

// ─── conversions ─────────────────────────────────────────────────────────────

pub fn symbol_to_id(s: &str) -> SymbolId {
    // Simple deterministic hash for symbol string → u32
    let mut h: u32 = 0x811c9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

impl From<JsonNewOrder> for NewOrder {
    fn from(j: JsonNewOrder) -> NewOrder {
        NewOrder {
            id:         j.id,
            symbol:     symbol_to_id(&j.symbol),
            side:       match j.side { JsonSide::Buy => Side::Buy, JsonSide::Sell => Side::Sell },
            kind:       match j.kind {
                JsonOrderType::Limit      => OrderType::Limit,
                JsonOrderType::Market     => OrderType::Market,
                JsonOrderType::Ioc        => OrderType::Ioc,
                JsonOrderType::Fok        => OrderType::Fok,
                JsonOrderType::StopMarket => OrderType::StopMarket,
                JsonOrderType::StopLimit  => OrderType::StopLimit,
            },
            tif:        match j.tif { JsonTif::Gtc => TimeInForce::Gtc, JsonTif::Day => TimeInForce::Day },
            price:      dollars_to_ticks(j.price),
            stop_price: dollars_to_ticks(j.stop_price),
            qty:        j.qty,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_new_limit() {
        let s = r#"{"t":"new","id":1,"symbol":"AAPL","side":"buy","type":"limit","price":150.25,"qty":100}"#;
        let msg = parse_inbound(s).unwrap();
        if let JsonInbound::New(no) = msg {
            assert_eq!(no.id, 1);
            assert_eq!(dollars_to_ticks(no.price), 1_502_500);
        } else { panic!(); }
    }

    #[test]
    fn serialize_ack() {
        let msg = JsonOutbound::Ack { id: 42, seq: 7 };
        let s = serialize_outbound(&msg).unwrap();
        assert!(s.contains("\"t\":\"ack\""));
    }

    #[test]
    fn tick_roundtrip() {
        let d = 123.4567;
        let t = dollars_to_ticks(d);
        let back = ticks_to_dollars(t);
        assert!((back - d).abs() < 0.0001);
    }
}
