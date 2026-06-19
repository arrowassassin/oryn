//! Append-only event log plus a content-addressed artifact store (CAS).
//!
//! In M1 this is in-memory. It is deliberately the same store the M2 context
//! broker will route over (content handles, dedup, delta sync), so the public
//! API is kept stable and minimal.
//!
//! **Unbounded by design (for now).** Neither the event log nor the CAS evicts;
//! both grow with the session. A noisy or long-running agent can therefore
//! drive memory without limit. This is acceptable for trusted local sessions in
//! M1; a bounded/persistent backend with eviction is tracked for the
//! persistence segment. Do not point this store at untrusted, unbounded input
//! until that lands.

use std::collections::HashMap;

use crate::event::AgentEvent;
use crate::ids::ArtifactId;

/// Append-only event log plus a content-addressed artifact store.
#[derive(Debug, Default)]
pub struct EventStore {
    events: Vec<AgentEvent>,
    cas: HashMap<ArtifactId, Vec<u8>>,
}

impl EventStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Store bytes, returning their content handle. Identical bytes dedup to a
    /// single physical copy and yield the same handle; the first-written bytes
    /// are retained (a re-put of equal bytes is a no-op).
    pub fn put_artifact(&mut self, content: &[u8]) -> ArtifactId {
        let id = ArtifactId::from_content(content);
        self.cas
            .entry(id.clone())
            .or_insert_with(|| content.to_vec());
        id
    }

    /// Fetch previously stored bytes by handle.
    pub fn get_artifact(&self, id: &ArtifactId) -> Option<&[u8]> {
        self.cas.get(id).map(Vec::as_slice)
    }

    /// Number of distinct artifacts held (post-dedup).
    pub fn artifact_count(&self) -> usize {
        self.cas.len()
    }

    /// Append an event to the log, preserving insertion order.
    pub fn append(&mut self, event: AgentEvent) {
        self.events.push(event);
    }

    /// All events in insertion order.
    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    /// Number of events appended. Mirrors [`artifact_count`](Self::artifact_count).
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Whether any events have been appended.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;

    fn ev(kind: EventKind) -> AgentEvent {
        AgentEvent::new("claude", "s1", kind, "{}")
    }

    #[test]
    fn new_store_is_empty() {
        let s = EventStore::new();
        assert!(s.is_empty());
        assert_eq!(s.event_count(), 0);
        assert_eq!(s.artifact_count(), 0);
        assert!(s.events().is_empty());
    }

    #[test]
    fn identical_content_dedups_and_keeps_original_bytes() {
        let mut s = EventStore::new();
        let h1 = s.put_artifact(b"hello world");
        let h2 = s.put_artifact(b"hello world");
        assert_eq!(h1, h2);
        assert_eq!(s.artifact_count(), 1);
        assert_eq!(s.get_artifact(&h1), Some(b"hello world".as_slice()));
    }

    #[test]
    fn different_content_yields_different_handles() {
        let mut s = EventStore::new();
        let a = s.put_artifact(b"aaa");
        let b = s.put_artifact(b"bbb");
        assert_ne!(a, b);
        assert_eq!(s.artifact_count(), 2);
    }

    #[test]
    fn store_is_byte_clean_not_string_clean() {
        let mut s = EventStore::new();
        let bytes = [0xff, 0x00, 0xfe, 0x10];
        let id = s.put_artifact(&bytes);
        assert_eq!(s.get_artifact(&id), Some(bytes.as_slice()));
    }

    #[test]
    fn missing_artifact_is_none() {
        let s = EventStore::new();
        let absent = ArtifactId::from_content(b"never stored");
        assert!(s.get_artifact(&absent).is_none());
    }

    #[test]
    fn append_preserves_order_and_counts() {
        let mut s = EventStore::new();
        s.append(ev(EventKind::SessionStart));
        s.append(ev(EventKind::ToolUse));
        assert_eq!(s.event_count(), 2);
        assert!(!s.is_empty());
        assert_eq!(s.events()[0].kind, EventKind::SessionStart);
        assert_eq!(s.events()[1].kind, EventKind::ToolUse);
    }
}
