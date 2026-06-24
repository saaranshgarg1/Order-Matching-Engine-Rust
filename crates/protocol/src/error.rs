use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("truncated message")]
    Truncated,
    #[error("unknown message tag: {0:#x}")]
    UnknownTag(u8),
    #[error("invalid field: {0}")]
    BadField(&'static str),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
