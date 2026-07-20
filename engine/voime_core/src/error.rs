//! 统一错误类型。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VoimeError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("onnxruntime: {0}")]
    Ort(#[from] ort::Error),
    #[error("model: {0}")]
    Model(String),
    #[error("feature: {0}")]
    Feature(String),
    #[error("tokenizer: {0}")]
    Tokenizer(String),
    #[error("invalid state: {0}")]
    State(String),
}

pub type Result<T> = std::result::Result<T, VoimeError>;
