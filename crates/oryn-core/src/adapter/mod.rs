//! Agent adapters: translate one CLI's output stream into normalized events.
//!
//! Each adapter is **pure** — a line of the agent's stdout goes in, zero or
//! more [`AgentEvent`](crate::event::AgentEvent)s come out, with no I/O and no
//! panics on malformed input. The capture layer (engine segment) owns the
//! process, stamps timestamps and ids, and persists into the store. Keeping
//! adapters pure makes them trivially testable against recorded fixtures.

pub mod claude;

use crate::event::AgentEvent;

/// Normalizes one agent CLI's output stream into the common event model.
///
/// `parse_line` takes `&mut self` so adapters that need to carry state across
/// lines (multi-line frames, streaming hunks) can — even though the Claude
/// adapter is effectively stateless. Locking this in now keeps the four planned
/// adapters from baking in a stateless-only contract.
///
/// The trait stays object-safe (`Box<dyn AgentAdapter>`), so a registry can
/// hold heterogeneous adapters.
pub trait AgentAdapter: Send + Sync {
    /// Stable adapter name, recorded on every event's `agent` field.
    fn name(&self) -> &str;

    /// Parse a single line of the agent's stdout into normalized events.
    ///
    /// Unrecognized or malformed lines MUST return an empty vec rather than
    /// panic — agent CLIs emit junk, partial lines, and format drift. (Surfacing
    /// *why* a line produced no events — drift detection — is the capture
    /// layer's job, tracked for the engine segment.)
    fn parse_line(&mut self, line: &str) -> Vec<AgentEvent>;
}
