//! Deterministic capability matrix — sub-task kind → ordered tier of model ids.
//!
//! The matrix maps each [`SubtaskKind`] to an ordered list of [`ModelId`]s,
//! cheapest / most-local first and strongest frontier last. Callers walk the
//! tier in order, trying each candidate until one succeeds, giving a
//! cost-optimal "route, don't race" dispatch strategy.
//!
//! The ids stored here are **logical** ids (e.g. `"sonnet"`, `"local-qwen-coder"`)
//! that are resolved against a [`crate::orchestrator::provider::ProviderRegistry`]
//! at dispatch time. This keeps the matrix decoupled from concrete model versions
//! so it can be updated independently of the registry.

use std::collections::BTreeMap;

use crate::orchestrator::{provider::ModelId, task::SubtaskKind};

// ── CapabilityMatrix ──────────────────────────────────────────────────────────

/// Maps each [`SubtaskKind`] to an ordered tier of [`ModelId`]s.
///
/// The tier is ordered cheap-first (local or low-cost models) to
/// frontier-last (strongest, most expensive models). Callers should try
/// candidates in order and fall through to the next on failure.
///
/// [`BTreeMap`] is used so that iteration over kinds is always
/// deterministic regardless of insertion order.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityMatrix {
    /// Maps each sub-task kind to its ordered model tier.
    pub tiers: BTreeMap<SubtaskKind, Vec<ModelId>>,
}

impl CapabilityMatrix {
    /// Create an empty matrix.
    pub fn new() -> Self {
        Self { tiers: BTreeMap::new() }
    }

    /// Return the ordered model tier for `kind`.
    ///
    /// Returns an empty slice if the kind has no mapping in this matrix.
    pub fn tier(&self, kind: SubtaskKind) -> &[ModelId] {
        self.tiers.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Builder method: set (or replace) the tier for `kind`.
    ///
    /// Consumes `self` and returns a new [`CapabilityMatrix`] so that calls
    /// can be chained fluently.
    #[must_use]
    pub fn with(mut self, kind: SubtaskKind, models: Vec<ModelId>) -> Self {
        self.tiers.insert(kind, models);
        self
    }
}

impl Default for CapabilityMatrix {
    fn default() -> Self {
        default_matrix()
    }
}

// ── default_matrix ────────────────────────────────────────────────────────────

/// Return the default capability matrix encoding the research routing table.
///
/// Each tier is ordered cheap/local first and frontier last. The model ids are
/// **logical** ids resolved against a [`crate::orchestrator::provider::ProviderRegistry`]
/// at dispatch time.
///
/// | Kind            | Tier (cheap → frontier)                        |
/// |-----------------|------------------------------------------------|
/// | `MechanicalEdit`| `local-qwen-coder`, `gemini-flash`             |
/// | `TestGen`       | `local-qwen-coder`, `gpt-5-mini`, `sonnet`     |
/// | `DiffEdit`      | `sonnet`, `opus`, `gpt-5-high`                 |
/// | `LargeContext`  | `gemini-2.5-pro`, `local-deepseek`, `opus`     |
/// | `Debugging`     | `gpt-5-high`, `opus`                           |
/// | `Refactor`      | `sonnet`, `opus`                               |
pub fn default_matrix() -> CapabilityMatrix {
    fn ids(names: &[&str]) -> Vec<ModelId> {
        names.iter().map(|s| ModelId::new(*s)).collect()
    }

    CapabilityMatrix::new()
        .with(SubtaskKind::MechanicalEdit, ids(&["local-qwen-coder", "gemini-flash"]))
        .with(SubtaskKind::TestGen, ids(&["local-qwen-coder", "gpt-5-mini", "sonnet"]))
        .with(SubtaskKind::DiffEdit, ids(&["sonnet", "opus", "gpt-5-high"]))
        .with(SubtaskKind::LargeContext, ids(&["gemini-2.5-pro", "local-deepseek", "opus"]))
        .with(SubtaskKind::Debugging, ids(&["gpt-5-high", "opus"]))
        .with(SubtaskKind::Refactor, ids(&["sonnet", "opus"]))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn mid(s: &str) -> ModelId {
        ModelId::new(s)
    }

    // ── default_matrix: every SubtaskKind has a non-empty tier ───────────────

    #[test]
    fn default_matrix_all_kinds_non_empty() {
        let matrix = default_matrix();
        let kinds = [
            SubtaskKind::MechanicalEdit,
            SubtaskKind::TestGen,
            SubtaskKind::DiffEdit,
            SubtaskKind::LargeContext,
            SubtaskKind::Debugging,
            SubtaskKind::Refactor,
        ];
        for kind in kinds {
            let tier = matrix.tier(kind);
            assert!(!tier.is_empty(), "{kind:?} should have a non-empty tier");
        }
    }

    // ── default_matrix: exact mappings per brief ──────────────────────────────

    #[test]
    fn default_matrix_mechanical_edit_tier() {
        let matrix = default_matrix();
        assert_eq!(
            matrix.tier(SubtaskKind::MechanicalEdit),
            &[mid("local-qwen-coder"), mid("gemini-flash")],
        );
    }

    #[test]
    fn default_matrix_test_gen_tier() {
        let matrix = default_matrix();
        assert_eq!(
            matrix.tier(SubtaskKind::TestGen),
            &[mid("local-qwen-coder"), mid("gpt-5-mini"), mid("sonnet")],
        );
    }

    #[test]
    fn default_matrix_diff_edit_tier() {
        let matrix = default_matrix();
        assert_eq!(
            matrix.tier(SubtaskKind::DiffEdit),
            &[mid("sonnet"), mid("opus"), mid("gpt-5-high")],
        );
    }

    #[test]
    fn default_matrix_large_context_tier() {
        let matrix = default_matrix();
        assert_eq!(
            matrix.tier(SubtaskKind::LargeContext),
            &[mid("gemini-2.5-pro"), mid("local-deepseek"), mid("opus")],
        );
    }

    #[test]
    fn default_matrix_debugging_tier() {
        let matrix = default_matrix();
        assert_eq!(
            matrix.tier(SubtaskKind::Debugging),
            &[mid("gpt-5-high"), mid("opus")],
        );
    }

    #[test]
    fn default_matrix_refactor_tier() {
        let matrix = default_matrix();
        assert_eq!(
            matrix.tier(SubtaskKind::Refactor),
            &[mid("sonnet"), mid("opus")],
        );
    }

    // ── tier: cheap-first ordering preserved ─────────────────────────────────

    #[test]
    fn tier_preserves_insertion_order_cheap_first() {
        // Build a custom matrix and verify the returned slice matches insertion order.
        let matrix = CapabilityMatrix::new().with(
            SubtaskKind::TestGen,
            vec![mid("cheap-local"), mid("mid-tier"), mid("frontier")],
        );
        let tier = matrix.tier(SubtaskKind::TestGen);
        assert_eq!(tier[0], mid("cheap-local"), "first entry should be cheapest");
        assert_eq!(tier[1], mid("mid-tier"));
        assert_eq!(tier[2], mid("frontier"), "last entry should be strongest");
    }

    // ── tier: unmapped kind returns empty slice ───────────────────────────────

    #[test]
    fn tier_unmapped_kind_returns_empty_slice() {
        let matrix = CapabilityMatrix::new(); // no entries
        assert!(
            matrix.tier(SubtaskKind::Debugging).is_empty(),
            "unmapped kind should return empty slice",
        );
    }

    // ── with: builder overrides existing tier ────────────────────────────────

    #[test]
    fn with_overrides_existing_tier() {
        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::Refactor, vec![mid("original")])
            .with(SubtaskKind::Refactor, vec![mid("override-a"), mid("override-b")]);

        let tier = matrix.tier(SubtaskKind::Refactor);
        assert_eq!(tier, &[mid("override-a"), mid("override-b")]);
    }

    // ── with: independent kinds are both present ─────────────────────────────

    #[test]
    fn with_independent_kinds_both_present() {
        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::MechanicalEdit, vec![mid("cheap")])
            .with(SubtaskKind::Debugging, vec![mid("strong")]);

        assert_eq!(matrix.tier(SubtaskKind::MechanicalEdit), &[mid("cheap")]);
        assert_eq!(matrix.tier(SubtaskKind::Debugging), &[mid("strong")]);
        // Unmapped kind still returns empty
        assert!(matrix.tier(SubtaskKind::DiffEdit).is_empty());
    }

    // ── Default impl equals default_matrix() ─────────────────────────────────

    #[test]
    fn default_impl_matches_default_matrix_fn() {
        let via_fn = default_matrix();
        let via_default = CapabilityMatrix::default();
        assert_eq!(via_fn, via_default);
    }

    // ── Clone and PartialEq ───────────────────────────────────────────────────

    #[test]
    fn clone_produces_equal_matrix() {
        let original = default_matrix();
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn two_distinct_matrices_not_equal() {
        let a = CapabilityMatrix::new().with(SubtaskKind::Refactor, vec![mid("x")]);
        let b = CapabilityMatrix::new().with(SubtaskKind::Refactor, vec![mid("y")]);
        assert_ne!(a, b);
    }

    // ── BTreeMap determinism: tier returns consistent order across calls ──────

    #[test]
    fn btreemap_iteration_order_is_deterministic() {
        let matrix = default_matrix();
        // Collect all kinds in BTreeMap order (two separate passes) and compare.
        let pass1: Vec<SubtaskKind> = matrix.tiers.keys().copied().collect();
        let pass2: Vec<SubtaskKind> = matrix.tiers.keys().copied().collect();
        assert_eq!(pass1, pass2, "BTreeMap iteration must be deterministic");
    }

    // ── new() produces empty matrix ───────────────────────────────────────────

    #[test]
    fn new_produces_empty_matrix() {
        let matrix = CapabilityMatrix::new();
        assert!(matrix.tiers.is_empty());
    }
}
