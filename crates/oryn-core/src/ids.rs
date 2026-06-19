//! Strongly-typed identifiers used across the event model and store.
//!
//! These newtypes exist so handles cannot be confused with arbitrary strings
//! (`session_id`, `agent`, …) and so invariants have a single home. Keeping
//! them in their own module lets both [`crate::event`] and [`crate::store`]
//! depend on them without a circular reference.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable unique identifier for an [`crate::event::AgentEvent`].
///
/// Every captured event carries one so later segments can build the typed
/// causal/provenance graph (an event's `parent_id` points at the [`EventId`]
/// of its cause). The model carries the field now; the engine segment assigns
/// real values at capture time. A freshly-[`new`](crate::event::AgentEvent::new)
/// event has a [`nil`](EventId::nil) id until assigned.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId(String);

impl EventId {
    /// Wrap an existing id string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The empty/unassigned id.
    pub fn nil() -> Self {
        Self(String::new())
    }

    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this id is still unassigned.
    pub fn is_nil(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A content-address handle: the lowercase hex SHA-256 of stored bytes.
///
/// **This is a dedup primitive, not a security one.** It identifies bytes the
/// local process produced; no trust, signature, or authentication rides on it.
/// A future cross-process broker MUST re-hash any bytes fetched against a
/// remote-supplied handle rather than trusting the handle as proof-of-content.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactId(String);

/// Error returned when a string is not a valid [`ArtifactId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidArtifactId;

impl fmt::Display for InvalidArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("artifact id must be 64 lowercase hex characters")
    }
}

impl std::error::Error for InvalidArtifactId {}

impl ArtifactId {
    /// Derive the handle for `content` (its lowercase hex SHA-256).
    pub fn from_content(content: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(content)))
    }

    /// Borrow the underlying hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&str> for ArtifactId {
    type Error = InvalidArtifactId;

    /// Validate an externally-supplied handle: exactly 64 lowercase hex chars.
    /// This is the boundary check that keeps forged/garbage handles out of the
    /// store's public API.
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let is_valid = value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if is_valid {
            Ok(Self(value.to_string()))
        } else {
            Err(InvalidArtifactId)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_nil_is_empty_and_detectable() {
        let nil = EventId::nil();
        assert!(nil.is_nil());
        assert_eq!(nil.as_str(), "");
        assert_eq!(nil.to_string(), "");
    }

    #[test]
    fn event_id_new_is_not_nil_and_displays() {
        let id = EventId::new("ev-123");
        assert!(!id.is_nil());
        assert_eq!(id.as_str(), "ev-123");
        assert_eq!(id.to_string(), "ev-123");
    }

    #[test]
    fn event_id_roundtrips_through_json() {
        let id = EventId::new("ev-9");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"ev-9\"");
        let back: EventId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn artifact_id_from_content_is_lowercase_sha256() {
        let id = ArtifactId::from_content(b"hello world");
        assert_eq!(
            id.as_str(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(id.to_string(), id.as_str());
    }

    #[test]
    fn artifact_id_empty_content_known_vector() {
        let id = ArtifactId::from_content(b"");
        assert_eq!(
            id.as_str(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn artifact_id_try_from_accepts_valid_handle() {
        let valid = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let id = ArtifactId::try_from(valid).expect("valid handle");
        assert_eq!(id.as_str(), valid);
    }

    #[test]
    fn artifact_id_try_from_rejects_bad_handles() {
        assert_eq!(ArtifactId::try_from("tooshort"), Err(InvalidArtifactId));
        assert_eq!(ArtifactId::try_from(""), Err(InvalidArtifactId));
        let upper = "B94D27B9934D3E08A52E52D7DA7DABFAC484EFE37A5380EE9088F7ACE2EFCDE9";
        assert_eq!(ArtifactId::try_from(upper), Err(InvalidArtifactId));
        let bad = "g94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert_eq!(ArtifactId::try_from(bad), Err(InvalidArtifactId));
    }

    #[test]
    fn invalid_artifact_id_displays_reason() {
        assert_eq!(
            InvalidArtifactId.to_string(),
            "artifact id must be 64 lowercase hex characters"
        );
    }
}
