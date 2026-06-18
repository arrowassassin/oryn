//! Dynamic model discovery — each agent framework reports which models it can
//! access given the user's current credentials.
//!
//! [`ModelDiscovery`] is the object-safe trait implemented by per-framework
//! adapters. [`discover_targets`] fans out across all registered sources,
//! deduplicates by [`ExecutionTarget`], and collects partial errors so that one
//! missing credential set does not block the rest.

use thiserror::Error;

use crate::orchestrator::provider::{AgentFramework, ExecutionTarget, ModelSpec};

// ── DiscoveryError ────────────────────────────────────────────────────────────

/// Errors that a [`ModelDiscovery`] source can return.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    /// The framework rejected the discovery request — credentials absent or
    /// invalid.
    #[error("unauthorized")]
    Unauthorized,
    /// The framework is unreachable (network failure, daemon not running, …).
    #[error("unavailable")]
    Unavailable,
}

// ── ModelDiscovery trait ──────────────────────────────────────────────────────

/// Object-safe trait for a single framework's model-discovery adapter.
///
/// Each concrete implementation queries one agent framework (e.g. Claude Code,
/// Codex, aider) and returns the [`ModelSpec`]s the user's current credentials
/// can access. Real implementations are added in a later segment; this module
/// provides the contract and the fan-out aggregator.
pub trait ModelDiscovery: Send + Sync {
    /// The framework this source represents.
    fn framework(&self) -> AgentFramework;

    /// Query the framework and return all accessible model specs.
    ///
    /// # Errors
    ///
    /// Returns [`DiscoveryError::Unauthorized`] when credentials are absent or
    /// invalid, or [`DiscoveryError::Unavailable`] when the framework cannot be
    /// reached.
    fn discover(&self) -> Result<Vec<ModelSpec>, DiscoveryError>;
}

// ── discover_targets ──────────────────────────────────────────────────────────

/// Fan out across all discovery `sources` and return a deduplicated, sorted set
/// of [`ModelSpec`]s plus any per-source errors.
///
/// # Deduplication
///
/// Specs are keyed by [`ExecutionTarget`] (`(framework, model)`). When two
/// sources produce the same target the **first** occurrence wins and the
/// duplicate is silently dropped.
///
/// # Ordering
///
/// The returned specs are sorted by their [`ExecutionTarget`] so that callers
/// always receive a deterministic list regardless of source order.
///
/// # Error handling
///
/// A source that returns [`Err`] is skipped; its error is appended to the
/// second element of the returned tuple. Remaining sources are still queried.
pub fn discover_targets(
    sources: &[&dyn ModelDiscovery],
) -> (Vec<ModelSpec>, Vec<DiscoveryError>) {
    // Use a BTreeMap keyed by ExecutionTarget to deduplicate (first wins) and
    // to guarantee deterministic output ordering.
    let mut seen: std::collections::BTreeMap<ExecutionTarget, ModelSpec> =
        std::collections::BTreeMap::new();
    let mut errors: Vec<DiscoveryError> = Vec::new();

    for source in sources {
        match source.discover() {
            Ok(specs) => {
                for spec in specs {
                    let target = spec.target();
                    // entry().or_insert preserves the first winner.
                    seen.entry(target).or_insert(spec);
                }
            }
            Err(e) => errors.push(e),
        }
    }

    let specs: Vec<ModelSpec> = seen.into_values().collect();
    (specs, errors)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::provider::{ModelId, ModelKind, Pricing};

    // ── FakeDiscovery ─────────────────────────────────────────────────────────

    /// A test-only discovery source with a fixed result.
    struct FakeDiscovery {
        framework: AgentFramework,
        result: Result<Vec<ModelSpec>, ()>,
    }

    impl FakeDiscovery {
        fn ok(framework: AgentFramework, specs: Vec<ModelSpec>) -> Self {
            Self { framework, result: Ok(specs) }
        }

        fn unauthorized(framework: AgentFramework) -> Self {
            Self { framework, result: Err(()) }
        }
    }

    impl ModelDiscovery for FakeDiscovery {
        fn framework(&self) -> AgentFramework {
            self.framework
        }

        fn discover(&self) -> Result<Vec<ModelSpec>, DiscoveryError> {
            match &self.result {
                Ok(specs) => Ok(specs.clone()),
                Err(()) => Err(DiscoveryError::Unauthorized),
            }
        }
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn make_spec(framework: AgentFramework, id: &str) -> ModelSpec {
        ModelSpec {
            id: ModelId::new(id),
            kind: ModelKind::Api { provider: "test".into() },
            pricing: Pricing::ZERO,
            context_window: 128_000,
            framework,
        }
    }

    // ── discover_targets: basics ──────────────────────────────────────────────

    #[test]
    fn single_source_returns_all_specs() {
        let spec = make_spec(AgentFramework::ClaudeCode, "claude-opus-4-5");
        let src = FakeDiscovery::ok(AgentFramework::ClaudeCode, vec![spec.clone()]);
        let (specs, errors) = discover_targets(&[&src]);
        assert_eq!(errors.len(), 0);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, spec.id);
    }

    #[test]
    fn multiple_sources_are_unioned() {
        let src1 = FakeDiscovery::ok(
            AgentFramework::ClaudeCode,
            vec![make_spec(AgentFramework::ClaudeCode, "opus")],
        );
        let src2 = FakeDiscovery::ok(
            AgentFramework::Codex,
            vec![make_spec(AgentFramework::Codex, "gpt-5")],
        );
        let (specs, errors) = discover_targets(&[&src1, &src2]);
        assert_eq!(errors.len(), 0);
        assert_eq!(specs.len(), 2);
    }

    // ── discover_targets: deduplication ──────────────────────────────────────

    #[test]
    fn duplicate_target_first_wins() {
        // Two sources both expose ClaudeCode/opus — first registration wins.
        let spec_a = make_spec(AgentFramework::ClaudeCode, "opus");
        let mut spec_b = make_spec(AgentFramework::ClaudeCode, "opus");
        // Distinguish the two by a different context window.
        spec_b.context_window = 999;

        let src1 = FakeDiscovery::ok(AgentFramework::ClaudeCode, vec![spec_a.clone()]);
        let src2 = FakeDiscovery::ok(AgentFramework::ClaudeCode, vec![spec_b]);

        let (specs, _) = discover_targets(&[&src1, &src2]);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].context_window, spec_a.context_window, "first spec must win");
    }

    #[test]
    fn same_model_different_frameworks_are_distinct_targets() {
        // "opus" via ClaudeCode and "opus" via Codex are two different targets.
        let src1 = FakeDiscovery::ok(
            AgentFramework::ClaudeCode,
            vec![make_spec(AgentFramework::ClaudeCode, "opus")],
        );
        let src2 = FakeDiscovery::ok(
            AgentFramework::Codex,
            vec![make_spec(AgentFramework::Codex, "opus")],
        );
        let (specs, _) = discover_targets(&[&src1, &src2]);
        assert_eq!(specs.len(), 2, "same model id + different frameworks = two distinct targets");
        let targets: Vec<ExecutionTarget> = specs.iter().map(|s| s.target()).collect();
        assert!(targets.contains(&ExecutionTarget::new(AgentFramework::ClaudeCode, ModelId::new("opus"))));
        assert!(targets.contains(&ExecutionTarget::new(AgentFramework::Codex, ModelId::new("opus"))));
    }

    // ── discover_targets: error handling ─────────────────────────────────────

    #[test]
    fn erroring_source_is_skipped_and_error_recorded() {
        let good = FakeDiscovery::ok(
            AgentFramework::ClaudeCode,
            vec![make_spec(AgentFramework::ClaudeCode, "opus")],
        );
        let bad = FakeDiscovery::unauthorized(AgentFramework::Codex);
        let (specs, errors) = discover_targets(&[&good, &bad]);
        assert_eq!(specs.len(), 1, "good source should still contribute");
        assert_eq!(errors.len(), 1, "erroring source should be recorded");
    }

    #[test]
    fn all_erroring_sources_returns_empty_specs() {
        let bad1 = FakeDiscovery::unauthorized(AgentFramework::ClaudeCode);
        let bad2 = FakeDiscovery::unauthorized(AgentFramework::Codex);
        let (specs, errors) = discover_targets(&[&bad1, &bad2]);
        assert!(specs.is_empty());
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn empty_sources_returns_empty() {
        let (specs, errors) = discover_targets(&[]);
        assert!(specs.is_empty());
        assert!(errors.is_empty());
    }

    // ── discover_targets: ordering ────────────────────────────────────────────

    #[test]
    fn output_is_sorted_by_execution_target() {
        // Insert in reverse order; output must be sorted.
        let src = FakeDiscovery::ok(
            AgentFramework::ClaudeCode,
            vec![
                make_spec(AgentFramework::ClaudeCode, "zzz"),
                make_spec(AgentFramework::ClaudeCode, "aaa"),
                make_spec(AgentFramework::ClaudeCode, "mmm"),
            ],
        );
        let (specs, _) = discover_targets(&[&src]);
        let ids: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["aaa", "mmm", "zzz"], "specs must be sorted by ExecutionTarget");
    }

    #[test]
    fn framework_ordering_respected_in_sort() {
        // AgentFramework ord: Aider < ClaudeCode < Codex < Cursor < GeminiCli < Local
        let src_cursor =
            FakeDiscovery::ok(AgentFramework::Cursor, vec![make_spec(AgentFramework::Cursor, "m")]);
        let src_aider =
            FakeDiscovery::ok(AgentFramework::Aider, vec![make_spec(AgentFramework::Aider, "m")]);
        let (specs, _) = discover_targets(&[&src_cursor, &src_aider]);
        assert_eq!(specs[0].framework, AgentFramework::Aider);
        assert_eq!(specs[1].framework, AgentFramework::Cursor);
    }

    // ── AgentFramework Display + serde ────────────────────────────────────────

    #[test]
    fn agent_framework_display_stable_strings() {
        assert_eq!(AgentFramework::ClaudeCode.to_string(), "claude-code");
        assert_eq!(AgentFramework::Codex.to_string(), "codex");
        assert_eq!(AgentFramework::Cursor.to_string(), "cursor");
        assert_eq!(AgentFramework::Aider.to_string(), "aider");
        assert_eq!(AgentFramework::GeminiCli.to_string(), "gemini-cli");
        assert_eq!(AgentFramework::Local.to_string(), "local");
    }

    #[test]
    fn agent_framework_roundtrips_json() {
        for fw in [
            AgentFramework::ClaudeCode,
            AgentFramework::Codex,
            AgentFramework::Cursor,
            AgentFramework::Aider,
            AgentFramework::GeminiCli,
            AgentFramework::Local,
        ] {
            let json = serde_json::to_string(&fw).unwrap();
            let back: AgentFramework = serde_json::from_str(&json).unwrap();
            assert_eq!(back, fw, "serde roundtrip failed for {fw}");
        }
    }

    // ── ExecutionTarget Display + serde ───────────────────────────────────────

    #[test]
    fn execution_target_display_format() {
        let t = ExecutionTarget::new(AgentFramework::ClaudeCode, ModelId::new("claude-opus-4-5"));
        assert_eq!(t.to_string(), "claude-code/claude-opus-4-5");
    }

    #[test]
    fn execution_target_roundtrips_json() {
        let t = ExecutionTarget::new(AgentFramework::GeminiCli, ModelId::new("gemini-2.5-pro"));
        let json = serde_json::to_string(&t).unwrap();
        let back: ExecutionTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    // ── DiscoveryError Display ────────────────────────────────────────────────

    #[test]
    fn discovery_error_unauthorized_displays() {
        assert_eq!(DiscoveryError::Unauthorized.to_string(), "unauthorized");
    }

    #[test]
    fn discovery_error_unavailable_displays() {
        assert_eq!(DiscoveryError::Unavailable.to_string(), "unavailable");
    }
}
