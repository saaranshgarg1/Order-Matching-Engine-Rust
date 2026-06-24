use bytes::{Buf, BufMut, Bytes, BytesMut};
use exchange_core::{OrderType, Side, TimeInForce};
use crate::error::ProtoError;

// Message type tags
pub const TAG_NEW_ORDER: u8 = b'N';
pub const TAG_CANCEL:    u8 = b'C';
pub const TAG_REPLACE:   u8 = b'R';
pub const TAG_ACK:       u8 = b'A';
pub const TAG_REJECT:    u8 = b'J';
pub const TAG_FILL:      u8 = b'F';
pub const TAG_CANCELLED: u8 = b'X';
pub const TAG_REPLACED:  u8 = b'Z';

/// Inbound: NewOrder — 40 bytes
#[derive(Debug, Clone)]
pub struct BinNewOrder {
    pub order_id:   u64,
    pub symbol_id:  u32,
    pub side:       Side,
    pub order_type: OrderType,
    pub tif:        TimeInForce,
    pub price:      i64,
    pub stop_price: i64,
    pub qty:        u64,
}

/// Inbound: Cancel — 13 bytes
#[derive(Debug, Clone)]
pub struct BinCancel {
    pub order_id:  u64,
    pub symbol_id: u32,
}

/// Inbound: Replace — 29 bytes
#[derive(Debug, Clone)]
pub struct BinReplace {
    pub order_id:  u64,
    pub symbol_id: u32,
    pub new_price: i64,
    pub new_qty:   u64,
}

#[derive(Debug, Clone)]
pub enum InboundMsg {
    NewOrder(BinNewOrder),
    Cancel(BinCancel),
    Replace(BinReplace),
}

/// Outbound messages
#[derive(Debug, Clone)]
pub enum OutboundMsg {
    Ack      { order_id: u64, seq: u64, status: u8 },
    Reject   { order_id: u64, seq: u64, reason: u8 },
    Fill     { order_id: u64, seq: u64, price: i64, qty: u64, leaves: u64, is_maker: bool },
    Cancelled{ order_id: u64, seq: u64 },
    Replaced { order_id: u64, seq: u64 },
}

// ─── encode ─────────────────────────────────────────────────────────────────

pub fn encode_outbound(msg: &OutboundMsg, buf: &mut BytesMut) {
    match msg {
        OutboundMsg::Ack { order_id, seq, status } => {
            buf.put_u8(TAG_ACK);
            buf.put_u64_le(*order_id);
            buf.put_u64_le(*seq);
            buf.put_u8(*status);
        }
        OutboundMsg::Reject { order_id, seq, reason } => {
            buf.put_u8(TAG_REJECT);
            buf.put_u64_le(*order_id);
            buf.put_u64_le(*seq);
            buf.put_u8(*reason);
        }
        OutboundMsg::Fill { order_id, seq, price, qty, leaves, is_maker } => {
            buf.put_u8(TAG_FILL);
            buf.put_u64_le(*order_id);
            buf.put_u64_le(*seq);
            buf.put_i64_le(*price);
            buf.put_u64_le(*qty);
            buf.put_u64_le(*leaves);
            buf.put_u8(*is_maker as u8);
        }
        OutboundMsg::Cancelled { order_id, seq } => {
            buf.put_u8(TAG_CANCELLED);
            buf.put_u64_le(*order_id);
            buf.put_u64_le(*seq);
        }
        OutboundMsg::Replaced { order_id, seq } => {
            buf.put_u8(TAG_REPLACED);
            buf.put_u64_le(*order_id);
            buf.put_u64_le(*seq);
        }
    }
}

// ─── decode ─────────────────────────────────────────────────────────────────

pub fn decode_inbound(buf: &mut Bytes) -> Result<InboundMsg, ProtoError> {
    if buf.is_empty() {
        return Err(ProtoError::Truncated);
    }
    let tag = buf.get_u8();
    match tag {
        TAG_NEW_ORDER => decode_new_order(buf),
        TAG_CANCEL    => decode_cancel(buf),
        TAG_REPLACE   => decode_replace(buf),
        other         => Err(ProtoError::UnknownTag(other)),
    }
}

fn decode_new_order(buf: &mut Bytes) -> Result<InboundMsg, ProtoError> {
    if buf.remaining() < 39 { return Err(ProtoError::Truncated); }
    let order_id   = buf.get_u64_le();
    let symbol_id  = buf.get_u32_le();
    let side       = decode_side(buf.get_u8())?;
    let order_type = decode_order_type(buf.get_u8())?;
    let tif        = decode_tif(buf.get_u8())?;
    let price      = buf.get_i64_le();
    let stop_price = buf.get_i64_le();
    let qty        = buf.get_u64_le();
    Ok(InboundMsg::NewOrder(BinNewOrder { order_id, symbol_id, side, order_type, tif, price, stop_price, qty }))
}

fn decode_cancel(buf: &mut Bytes) -> Result<InboundMsg, ProtoError> {
    if buf.remaining() < 12 { return Err(ProtoError::Truncated); }
    let order_id  = buf.get_u64_le();
    let symbol_id = buf.get_u32_le();
    Ok(InboundMsg::Cancel(BinCancel { order_id, symbol_id }))
}

fn decode_replace(buf: &mut Bytes) -> Result<InboundMsg, ProtoError> {
    if buf.remaining() < 28 { return Err(ProtoError::Truncated); }
    let order_id  = buf.get_u64_le();
    let symbol_id = buf.get_u32_le();
    let new_price = buf.get_i64_le();
    let new_qty   = buf.get_u64_le();
    Ok(InboundMsg::Replace(BinReplace { order_id, symbol_id, new_price, new_qty }))
}

fn decode_side(b: u8) -> Result<Side, ProtoError> {
    match b {
        0 => Ok(Side::Buy),
        1 => Ok(Side::Sell),
        _ => Err(ProtoError::BadField("side")),
    }
}

fn decode_order_type(b: u8) -> Result<OrderType, ProtoError> {
    match b {
        0 => Ok(OrderType::Limit),
        1 => Ok(OrderType::Market),
        2 => Ok(OrderType::Ioc),
        3 => Ok(OrderType::Fok),
        4 => Ok(OrderType::StopMarket),
        5 => Ok(OrderType::StopLimit),
        _ => Err(ProtoError::BadField("order_type")),
    }
}

fn decode_tif(b: u8) -> Result<TimeInForce, ProtoError> {
    match b {
        0 => Ok(TimeInForce::Gtc),
        1 => Ok(TimeInForce::Day),
        _ => Err(ProtoError::BadField("tif")),
    }
}

// ─── conversion to core types ───────────────────────────────────────────────

impl From<BinNewOrder> for exchange_core::NewOrder {
    fn from(b: BinNewOrder) -> exchange_core::NewOrder {
        exchange_core::NewOrder {
            id:         b.order_id,
            symbol:     b.symbol_id,
            side:       b.side,
            kind:       b.order_type,
            tif:        b.tif,
            price:      b.price,
            stop_price: b.stop_price,
            qty:        b.qty,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_new_order() {
        let orig = BinNewOrder {
            order_id: 42, symbol_id: 1,
            side: Side::Buy, order_type: OrderType::Limit,
            tif: TimeInForce::Gtc, price: 10050, stop_price: 0, qty: 100,
        };
        let mut buf = BytesMut::new();
        buf.put_u8(TAG_NEW_ORDER);
        buf.put_u64_le(orig.order_id);
        buf.put_u32_le(orig.symbol_id);
        buf.put_u8(0); // Buy
        buf.put_u8(0); // Limit
        buf.put_u8(0); // Gtc
        buf.put_i64_le(orig.price);
        buf.put_i64_le(orig.stop_price);
        buf.put_u64_le(orig.qty);

        let mut b = buf.freeze();
        let msg = decode_inbound(&mut b).unwrap();
        match msg {
            InboundMsg::NewOrder(no) => {
                assert_eq!(no.order_id, 42);
                assert_eq!(no.price, 10050);
                assert_eq!(no.qty, 100);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn unknown_tag_is_error() {
        let mut b = Bytes::from_static(&[0xFFu8]);
        assert!(matches!(decode_inbound(&mut b), Err(ProtoError::UnknownTag(0xFF))));
    }
}
