pub mod error;
pub mod binary;
pub mod json_codec;

pub use error::ProtoError;
pub use binary::{
    decode_inbound, encode_outbound,
    InboundMsg, OutboundMsg,
    BinNewOrder, BinCancel, BinReplace,
    TAG_NEW_ORDER, TAG_CANCEL, TAG_REPLACE,
};
pub use json_codec::{
    parse_inbound, serialize_outbound,
    JsonInbound, JsonOutbound, JsonNewOrder,
    dollars_to_ticks, ticks_to_dollars, symbol_to_id,
};
