//! Error types for the Oryn engine.

use thiserror::Error;

/// Result alias used throughout the engine.
pub type Result<T> = std::result::Result<T, OrynError>;

/// Errors produced by the engine.
#[derive(Debug, Error)]
pub enum OrynError {
    /// Caller supplied an empty or otherwise unusable dataset.
    #[error("empty input: {0}")]
    EmptyInput(String),

    /// Two datasets that must align had mismatched lengths.
    #[error("length mismatch: {0}")]
    LengthMismatch(String),

    /// A parameter was outside its valid domain.
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    /// A subprocess (cargo/git) failed.
    #[error("{0}")]
    Process(String),

    /// JSON (de)serialization failure.
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    /// I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
