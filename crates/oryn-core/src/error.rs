//! Error types for the Oryn engine.

use thiserror::Error;

/// Result alias used throughout the engine.
pub type Result<T> = std::result::Result<T, OrynError>;

/// Errors produced by the reproducibility / evaluation-integrity engine.
#[derive(Debug, Error)]
pub enum OrynError {
    /// Caller supplied an empty or otherwise unusable dataset.
    #[error("empty input: {0}")]
    EmptyInput(String),

    /// Two datasets that must align (e.g. paired eval) had mismatched lengths.
    #[error("length mismatch: {0}")]
    LengthMismatch(String),

    /// A parameter was outside its valid domain.
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    /// JSON (de)serialization failure.
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    /// An attestation chain failed verification.
    #[error("attestation verification failed: {0}")]
    Attestation(String),

    /// Cryptographic signature error.
    #[error("signature: {0}")]
    Signature(String),

    /// Hex decoding error (keys, signatures).
    #[error("hex decode: {0}")]
    Hex(#[from] hex::FromHexError),

    /// I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
