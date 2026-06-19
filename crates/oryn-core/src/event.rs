//! The normalized event model shared across every agent adapter.
//!
//! Adapters translate vendor-specific output into [`AgentEvent`]s. The `raw`
//! field always retains the original payload so adapter format drift can never
//! silently lose information.

use serde::{Deserialize, Serialize};

use crate::ids::{ArtifactId, EventId};

/// Kind of a normalized agent event. Stable across all adapters and clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// The agent session started (e.g. Claude Code `system/init`).
    SessionStart,
    /// Free-text assistant/user message.
    Message,
    /// The agent invoked a tool (Bash, Edit, Write, …).
    ToolUse,
    /// The result/output of a tool invocation (paired via `tool_id`). Carries
    /// `is_error` when the tool failed.
    ToolResult,
    /// A file changed on disk, observed as ground truth (not self-reported).
    FileChange,
    /// A token/cost update.
    Cost,
    /// The agent session ended naturally.
    SessionEnd,
}

/// Token usage broken out by class.
///
/// Kept as separate counters (not a single scalar) because budgets and the
/// future context broker care about the split: input vs. output price
/// differently, and cache-read/cache-write are what the broker's
/// cache-stable-prefix savings show up in. Per Anthropic's accounting the cache
/// fields are reported *separately* from `input` (not included in it), so
/// summing all four does not double-count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Non-cached prompt/input tokens.
    pub input: u64,
    /// Completion/output tokens.
    pub output: u64,
    /// Tokens served from the prompt cache (billed at a reduced rate).
    pub cache_read: u64,
    /// Tokens written to the prompt cache (billed at a premium).
    pub cache_write: u64,
}

impl TokenUsage {
    /// Total tokens across all classes. Used as the conservative figure for a
    /// hard-stop budget cap (an upper bound on volume; not cost-proportional —
    /// the classes are priced differently, so a USD budget should use cost).
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    /// Non-cached tokens (input + output) only. NOTE: cache tokens are still
    /// billed (at scaled rates); this is "uncached I/O", not "the only billed
    /// tokens" — do not treat it as total spend.
    pub fn non_cached(&self) -> u64 {
        self.input.saturating_add(self.output)
    }
}

/// One normalized event from any agent CLI.
///
/// `Eq` is intentionally not derived because `cost_usd` is an `f64`; events are
/// compared structurally via `PartialEq` only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
    /// Stable unique id. [`nil`](EventId::nil) until the engine assigns it.
    pub id: EventId,
    /// The event that caused this one, when known (for the provenance graph).
    pub parent_id: Option<EventId>,
    /// Adapter name that produced the event (e.g. `"claude"`).
    pub agent: String,
    /// Opaque session identifier the event belongs to.
    pub session_id: String,
    /// Unix milliseconds. Adapters set `0`; the capture layer stamps real time.
    pub ts: u64,
    /// What kind of event this is.
    pub kind: EventKind,
    /// Tool name, when `kind == ToolUse`.
    pub tool: Option<String>,
    /// Vendor tool-call id. Set on both `ToolUse` (the call) and `ToolResult`
    /// (the outcome) so a consumer can pair an outcome to its call without the
    /// parser holding cross-line state.
    pub tool_id: Option<String>,
    /// Files touched by this event (tool input or fs ground truth).
    pub files: Vec<String>,
    /// Shell command, when the tool is a command runner.
    pub command: Option<String>,
    /// Content handle into the [`crate::store::EventStore`] CAS, when this
    /// event references stored bytes (e.g. a diff snapshot).
    pub diff_ref: Option<ArtifactId>,
    /// Token usage associated with this event, when `kind == Cost`.
    pub usage: Option<TokenUsage>,
    /// Cumulative USD cost reported by the agent, when `kind == Cost`. Users
    /// think in dollars ("Max burn"); captured even when the hard-stop is
    /// token-based.
    pub cost_usd: Option<f64>,
    /// Whether a `ToolResult` represents a failure. Always `false` for other
    /// kinds.
    pub is_error: bool,
    /// The original, unparsed payload line. Always retained.
    ///
    /// **Untrusted, possibly secret-bearing.** Agent output routinely contains
    /// tokens, keys, env vars, and full file contents. Nothing here redacts it;
    /// a scrubbing hook must run before this value is persisted to disk or sent
    /// over any network (tracked for the persistence/broker segments).
    ///
    /// Likewise `command` and `files` carry attacker-influenced strings
    /// verbatim — consumers (UI render, logs) MUST treat them as untrusted and
    /// escape terminal control sequences; never re-execute `command`.
    pub raw: String,
}

impl AgentEvent {
    /// Construct a minimal event of `kind` for `agent`/`session_id`, retaining
    /// `raw`. The id is [`nil`](EventId::nil) and all other optional fields are
    /// empty/`None`/`false`; callers set what applies via the builder methods
    /// or by assigning fields. Adapters should prefer this over building the
    /// struct by hand so new fields get sane defaults in one place.
    pub fn new(
        agent: impl Into<String>,
        session_id: impl Into<String>,
        kind: EventKind,
        raw: impl Into<String>,
    ) -> Self {
        Self {
            id: EventId::nil(),
            parent_id: None,
            agent: agent.into(),
            session_id: session_id.into(),
            ts: 0,
            kind,
            tool: None,
            tool_id: None,
            files: Vec::new(),
            command: None,
            diff_ref: None,
            usage: None,
            cost_usd: None,
            is_error: false,
            raw: raw.into(),
        }
    }

    /// Set the stable id and return `self`.
    #[must_use]
    pub fn with_id(mut self, id: EventId) -> Self {
        self.id = id;
        self
    }

    /// Set the causal parent and return `self`.
    #[must_use]
    pub fn with_parent(mut self, parent: EventId) -> Self {
        self.parent_id = Some(parent);
        self
    }

    /// Set the wall-clock timestamp (unix milliseconds) and return `self`.
    #[must_use]
    pub fn with_ts(mut self, ts: u64) -> Self {
        self.ts = ts;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_defaults_and_retains_raw() {
        let ev = AgentEvent::new("claude", "s1", EventKind::ToolUse, "{\"x\":1}");
        assert_eq!(ev.agent, "claude");
        assert_eq!(ev.session_id, "s1");
        assert_eq!(ev.kind, EventKind::ToolUse);
        assert_eq!(ev.ts, 0);
        assert!(ev.id.is_nil());
        assert!(ev.parent_id.is_none());
        assert!(ev.tool.is_none());
        assert!(ev.tool_id.is_none());
        assert!(ev.files.is_empty());
        assert!(ev.command.is_none());
        assert!(ev.diff_ref.is_none());
        assert!(ev.usage.is_none());
        assert!(ev.cost_usd.is_none());
        assert!(!ev.is_error);
        assert_eq!(ev.raw, "{\"x\":1}");
    }

    #[test]
    fn builders_set_id_parent_and_ts() {
        let ev = AgentEvent::new("claude", "s1", EventKind::Message, "hi")
            .with_id(EventId::new("ev-2"))
            .with_parent(EventId::new("ev-1"))
            .with_ts(42);
        assert_eq!(ev.id, EventId::new("ev-2"));
        assert_eq!(ev.parent_id, Some(EventId::new("ev-1")));
        assert_eq!(ev.ts, 42);
    }

    #[test]
    fn with_ts_last_write_wins() {
        let ev = AgentEvent::new("claude", "s1", EventKind::Message, "hi")
            .with_ts(1)
            .with_ts(2);
        assert_eq!(ev.ts, 2);
    }

    #[test]
    fn event_roundtrips_through_json() {
        let mut ev =
            AgentEvent::new("claude", "s1", EventKind::ToolUse, "{}").with_id(EventId::new("ev-7"));
        ev.tool = Some("Bash".into());
        ev.tool_id = Some("toolu_1".into());
        ev.command = Some("ls -la".into());
        ev.files = vec!["src/main.rs".into()];
        ev.usage = Some(TokenUsage {
            input: 1000,
            output: 200,
            ..Default::default()
        });
        ev.cost_usd = Some(0.0125);
        ev.diff_ref = Some(ArtifactId::from_content(b"a diff"));

        let json = serde_json::to_string(&ev).expect("serialize");
        let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ev);
    }

    #[test]
    fn raw_retains_unicode_control_and_quotes_through_json() {
        let raw = "日本語\n\"quoted\"\u{0}tail";
        let ev = AgentEvent::new("claude", "s1", EventKind::Message, raw);
        let json = serde_json::to_string(&ev).unwrap();
        let back: AgentEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.raw, raw);
    }

    #[test]
    fn event_kind_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&EventKind::SessionStart).unwrap(),
            "\"session_start\""
        );
        assert_eq!(
            serde_json::to_string(&EventKind::ToolResult).unwrap(),
            "\"tool_result\""
        );
        let back: EventKind = serde_json::from_str("\"file_change\"").unwrap();
        assert_eq!(back, EventKind::FileChange);
    }

    #[test]
    fn token_usage_total_and_non_cached() {
        let u = TokenUsage {
            input: 100,
            output: 30,
            cache_read: 9,
            cache_write: 1,
        };
        assert_eq!(u.non_cached(), 130);
        assert_eq!(u.total(), 140);
        assert_eq!(TokenUsage::default().total(), 0);
    }

    #[test]
    fn token_usage_total_saturates() {
        let u = TokenUsage {
            input: u64::MAX,
            output: 10,
            cache_read: 0,
            cache_write: 0,
        };
        assert_eq!(u.total(), u64::MAX);
        assert_eq!(u.non_cached(), u64::MAX);
    }
}
