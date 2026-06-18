//! UI-agnostic session view-state, folded from the event stream.
//!
//! The engine and every client (GPUI app, future TUI) read from this. It holds
//! no renderer types — only the distilled state a client needs to draw: the
//! timeline, the current diff, the budget, and the lifecycle flags.

use crate::budget::Budget;
use crate::event::{AgentEvent, EventKind};
use crate::worktree::WorktreeDiff;

/// All view state for one session, folded from its events.
#[derive(Debug)]
pub struct SessionState {
    /// Engine-assigned session id.
    pub id: String,
    /// Every event in arrival order — the faithful trace.
    pub timeline: Vec<AgentEvent>,
    /// The latest structured diff of the session's worktree.
    pub diff: WorktreeDiff,
    /// Token budget (charged from `Cost` events).
    pub budget: Budget,
    /// Latest cumulative USD cost reported by the agent.
    pub cost_usd: f64,
    /// Set when the budget is exceeded; the engine kills the agent.
    pub stop_requested: bool,
    /// Set when the agent signals `SessionEnd` (natural completion).
    pub finished: bool,
}

impl SessionState {
    /// Create empty state for `id` with an optional token budget cap.
    pub fn new(id: impl Into<String>, budget_limit: Option<u64>) -> Self {
        Self {
            id: id.into(),
            timeline: Vec::new(),
            diff: WorktreeDiff::default(),
            budget: Budget::new(budget_limit),
            cost_usd: 0.0,
            stop_requested: false,
            finished: false,
        }
    }

    /// Fold one event into the view state.
    ///
    /// `Cost` events charge the budget by [`TokenUsage::total`] (the
    /// conservative figure) and update the USD cost; crossing the limit sets
    /// [`stop_requested`](Self::stop_requested). `SessionEnd` marks the session
    /// finished. All events are appended to the timeline regardless.
    ///
    /// [`TokenUsage::total`]: crate::event::TokenUsage::total
    pub fn on_event(&mut self, event: AgentEvent) {
        match event.kind {
            EventKind::Cost => {
                if let Some(usage) = event.usage {
                    self.budget.add(usage.total());
                    if self.budget.exceeded() {
                        self.stop_requested = true;
                    }
                }
                if let Some(cost) = event.cost_usd {
                    self.cost_usd = cost;
                }
            }
            EventKind::SessionEnd => self.finished = true,
            _ => {}
        }
        self.timeline.push(event);
    }

    /// Replace the current diff snapshot.
    pub fn set_diff(&mut self, diff: WorktreeDiff) {
        self.diff = diff;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, TokenUsage};
    use crate::worktree::{FileDiff, FileStatus};

    fn cost_event(usage: Option<TokenUsage>, cost_usd: Option<f64>) -> AgentEvent {
        let mut ev = AgentEvent::new("claude", "s1", EventKind::Cost, "{}");
        ev.usage = usage;
        ev.cost_usd = cost_usd;
        ev
    }

    #[test]
    fn new_state_is_empty() {
        let s = SessionState::new("s1", Some(1000));
        assert_eq!(s.id, "s1");
        assert!(s.timeline.is_empty());
        assert!(s.diff.is_empty());
        assert_eq!(s.cost_usd, 0.0);
        assert!(!s.stop_requested);
        assert!(!s.finished);
        assert_eq!(s.budget.limit(), Some(1000));
    }

    #[test]
    fn tool_use_appends_to_timeline_without_side_effects() {
        let mut s = SessionState::new("s1", Some(1000));
        s.on_event(AgentEvent::new("claude", "s1", EventKind::ToolUse, "{}"));
        assert_eq!(s.timeline.len(), 1);
        assert!(!s.stop_requested);
        assert!(!s.finished);
        assert_eq!(s.budget.spent(), 0);
    }

    #[test]
    fn cost_event_charges_budget_by_total() {
        let mut s = SessionState::new("s1", Some(10_000));
        let usage = TokenUsage { input: 1000, output: 200, cache_read: 50, cache_write: 5 };
        s.on_event(cost_event(Some(usage), Some(0.01)));
        assert_eq!(s.budget.spent(), 1255);
        assert_eq!(s.cost_usd, 0.01);
        assert!(!s.stop_requested);
    }

    #[test]
    fn cost_event_without_usage_only_updates_dollars() {
        let mut s = SessionState::new("s1", Some(10));
        s.on_event(cost_event(None, Some(0.5)));
        assert_eq!(s.budget.spent(), 0);
        assert_eq!(s.cost_usd, 0.5);
    }

    #[test]
    fn exceeding_budget_requests_stop() {
        let mut s = SessionState::new("s1", Some(1000));
        let usage = TokenUsage { input: 1500, ..Default::default() };
        s.on_event(cost_event(Some(usage), None));
        assert!(s.budget.exceeded());
        assert!(s.stop_requested);
    }

    #[test]
    fn cost_usd_tracks_latest_cumulative_value() {
        let mut s = SessionState::new("s1", None);
        s.on_event(cost_event(None, Some(0.01)));
        s.on_event(cost_event(None, Some(0.03)));
        assert_eq!(s.cost_usd, 0.03);
    }

    #[test]
    fn session_end_marks_finished_not_stop() {
        let mut s = SessionState::new("s1", None);
        s.on_event(AgentEvent::new("claude", "s1", EventKind::SessionEnd, "{}"));
        assert!(s.finished);
        assert!(!s.stop_requested);
        assert_eq!(s.timeline.len(), 1);
    }

    #[test]
    fn set_diff_replaces_snapshot() {
        let mut s = SessionState::new("s1", None);
        let d = WorktreeDiff {
            files: vec![FileDiff {
                path: "a.txt".into(),
                old_path: None,
                status: FileStatus::Added,
                patch: "+x".into(),
            }],
        };
        s.set_diff(d.clone());
        assert_eq!(s.diff, d);
        assert_eq!(s.diff.file_count(), 1);
    }

    #[test]
    fn timeline_preserves_order() {
        let mut s = SessionState::new("s1", None);
        s.on_event(AgentEvent::new("claude", "s1", EventKind::SessionStart, "{}"));
        s.on_event(AgentEvent::new("claude", "s1", EventKind::ToolUse, "{}"));
        s.on_event(AgentEvent::new("claude", "s1", EventKind::ToolResult, "{}"));
        let kinds: Vec<_> = s.timeline.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![EventKind::SessionStart, EventKind::ToolUse, EventKind::ToolResult]
        );
    }
}
