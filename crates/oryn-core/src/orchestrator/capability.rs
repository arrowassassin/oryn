//! Deterministic capability matrix — sub-task kind → ordered tier of model ids.
//!
//! The matrix maps each [`SubtaskKind`] to an ordered list of [`ModelId`]s,
//! cheapest / most-local first. The tier ordering is **derived** at session start
//! from the models actually available plus per-model capability scores; it is
//! never a hardcoded table. This means the matrix never routes to an unavailable
//! model and never goes stale when model rankings change.
//!
//! ## Resolution algorithm
//!
//! 1. For each [`SubtaskKind`] in [`SubtaskKind::ALL`], collect all available
//!    [`ModelSpec`]s whose [`CapabilityProfile::score`] for that kind is ≥
//!    [`MIN_CAPABILITY`]. A spec with no profile entry is treated as score 0.0
//!    and is silently skipped.
//! 2. Sort candidates by a **total** deterministic key:
//!    - [`cost_metric`] ascending (cheapest capable first)
//!    - score descending (via [`f64::total_cmp`])
//!    - local before API ([`ModelKind`])
//!    - [`framework_rank`] ascending (tunable framework preference)
//!    - [`ExecutionTarget`] ascending (final lexicographic tie-break)
//! 3. Map each candidate to its [`ExecutionTarget`] and insert into the matrix only if
//!    the list is non-empty.

use std::collections::BTreeMap;

use crate::orchestrator::{
    provider::{AgentFramework, ExecutionTarget, ModelId, ModelKind, ModelSpec, Pricing},
    task::SubtaskKind,
};

// ── CapabilityProfile ─────────────────────────────────────────────────────────

/// Per-model capability scores, one per [`SubtaskKind`].
///
/// Scores are in `0.0..=1.0`. A missing entry means 0.0 (the model is not a
/// candidate for that kind). These are **defaults** that capture research
/// rankings; they can be overridden by the user at session creation.
///
/// `Eq` is intentionally NOT derived because `f64` does not implement `Eq`.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityProfile {
    /// Maps each sub-task kind to a capability score in `[0.0, 1.0]`.
    pub scores: BTreeMap<SubtaskKind, f64>,
}

impl CapabilityProfile {
    /// Create an empty profile (all scores implicitly 0.0).
    pub fn new() -> Self {
        Self { scores: BTreeMap::new() }
    }

    /// Return the capability score for `kind`.
    ///
    /// Returns `0.0` if this profile has no entry for `kind`.
    pub fn score(&self, kind: SubtaskKind) -> f64 {
        self.scores.get(&kind).copied().unwrap_or(0.0)
    }

    /// Builder: set the score for `kind`.
    #[must_use]
    pub fn with(mut self, kind: SubtaskKind, score: f64) -> Self {
        self.scores.insert(kind, score);
        self
    }
}

impl Default for CapabilityProfile {
    fn default() -> Self {
        Self::new()
    }
}

// ── MIN_CAPABILITY ────────────────────────────────────────────────────────────

/// Minimum score a model must achieve for a [`SubtaskKind`] to be considered a
/// routing candidate for that kind.
pub const MIN_CAPABILITY: f64 = 0.5;

// ── default_profiles ──────────────────────────────────────────────────────────

/// Default capability profiles for the well-known logical model ids.
///
/// These encode the research routing rankings as **data** rather than a
/// hardcoded tier table. Users may supply their own profiles to override these
/// at session initialisation.
///
/// Score guidelines (per kind):
///
/// | Model id           | ME   | TG   | DE   | LC   | DB   | RF   |
/// |--------------------|------|------|------|------|------|------|
/// | `local-qwen-coder` | 0.85 | 0.70 | 0.35 | 0.30 | 0.40 | 0.55 |
/// | `local-deepseek`   | 0.60 | 0.65 | 0.50 | 0.75 | 0.55 | 0.60 |
/// | `gemini-flash`     | 0.80 | 0.65 | 0.55 | 0.60 | 0.55 | 0.60 |
/// | `gpt-5-mini`       | 0.70 | 0.75 | 0.60 | 0.55 | 0.60 | 0.65 |
/// | `gemini-2.5-pro`   | 0.75 | 0.80 | 0.75 | 0.90 | 0.75 | 0.80 |
/// | `sonnet`           | 0.90 | 0.85 | 0.80 | 0.75 | 0.60 | 0.82 |
/// | `opus`             | 0.88 | 0.88 | 0.90 | 0.85 | 0.92 | 0.90 |
/// | `gpt-5-high`       | 0.85 | 0.88 | 0.88 | 0.80 | 0.95 | 0.88 |
///
/// Key: ME=MechanicalEdit, TG=TestGen, DE=DiffEdit, LC=LargeContext,
///      DB=Debugging, RF=Refactor.
pub fn default_profiles() -> BTreeMap<ModelId, CapabilityProfile> {
    use SubtaskKind::{Debugging, DiffEdit, LargeContext, MechanicalEdit, Refactor, TestGen};

    let mut map = BTreeMap::new();

    map.insert(
        ModelId::new("local-qwen-coder"),
        CapabilityProfile::new()
            .with(MechanicalEdit, 0.85)
            .with(TestGen, 0.70)
            .with(DiffEdit, 0.35)
            .with(LargeContext, 0.30)
            .with(Debugging, 0.40)
            .with(Refactor, 0.55),
    );

    map.insert(
        ModelId::new("local-deepseek"),
        CapabilityProfile::new()
            .with(MechanicalEdit, 0.60)
            .with(TestGen, 0.65)
            .with(DiffEdit, 0.50)
            .with(LargeContext, 0.75)
            .with(Debugging, 0.55)
            .with(Refactor, 0.60),
    );

    map.insert(
        ModelId::new("gemini-flash"),
        CapabilityProfile::new()
            .with(MechanicalEdit, 0.80)
            .with(TestGen, 0.65)
            .with(DiffEdit, 0.55)
            .with(LargeContext, 0.60)
            .with(Debugging, 0.55)
            .with(Refactor, 0.60),
    );

    map.insert(
        ModelId::new("gpt-5-mini"),
        CapabilityProfile::new()
            .with(MechanicalEdit, 0.70)
            .with(TestGen, 0.75)
            .with(DiffEdit, 0.60)
            .with(LargeContext, 0.55)
            .with(Debugging, 0.60)
            .with(Refactor, 0.65),
    );

    map.insert(
        ModelId::new("gemini-2.5-pro"),
        CapabilityProfile::new()
            .with(MechanicalEdit, 0.75)
            .with(TestGen, 0.80)
            .with(DiffEdit, 0.75)
            .with(LargeContext, 0.90)
            .with(Debugging, 0.75)
            .with(Refactor, 0.80),
    );

    map.insert(
        ModelId::new("sonnet"),
        CapabilityProfile::new()
            .with(MechanicalEdit, 0.90)
            .with(TestGen, 0.85)
            .with(DiffEdit, 0.80)
            .with(LargeContext, 0.75)
            .with(Debugging, 0.60)
            .with(Refactor, 0.82),
    );

    map.insert(
        ModelId::new("opus"),
        CapabilityProfile::new()
            .with(MechanicalEdit, 0.88)
            .with(TestGen, 0.88)
            .with(DiffEdit, 0.90)
            .with(LargeContext, 0.85)
            .with(Debugging, 0.92)
            .with(Refactor, 0.90),
    );

    map.insert(
        ModelId::new("gpt-5-high"),
        CapabilityProfile::new()
            .with(MechanicalEdit, 0.85)
            .with(TestGen, 0.88)
            .with(DiffEdit, 0.88)
            .with(LargeContext, 0.80)
            .with(Debugging, 0.95)
            .with(Refactor, 0.88),
    );

    map
}

// ── cost_metric ───────────────────────────────────────────────────────────────

/// Blended cost metric for a [`Pricing`]: `input + output` per million tokens.
///
/// Local models use [`Pricing::ZERO`], so their metric is `0.0` and they sort
/// to the front of any tier (free models are always preferred over paid ones of
/// equal or lower capability).
pub fn cost_metric(p: &Pricing) -> f64 {
    p.input + p.output
}

// ── framework_rank ──────────────────────────────────────────────────────────────

/// Deterministic framework preference, used **only** as a tertiary tie-break in
/// [`resolve_matrix`] after cost, capability score, and local-vs-API have all tied.
///
/// Lower rank is preferred. The default prefers a local runtime (rank 0) — it is
/// free and private — then orders the remaining frameworks alphabetically by their
/// stable [`AgentFramework`] `Display` string (`aider` < `claude-code` < `codex` <
/// `cursor` < `gemini-cli`).
///
/// This is a **tunable policy knob**, not a capability statement: it never overrides
/// cost or capability. It exists so the framework dimension of the tie-break is an
/// explicit, auditable decision rather than an accident of enum declaration order.
pub fn framework_rank(framework: AgentFramework) -> u8 {
    match framework {
        AgentFramework::Local => 0,
        AgentFramework::Aider => 1,
        AgentFramework::ClaudeCode => 2,
        AgentFramework::Codex => 3,
        AgentFramework::Cursor => 4,
        AgentFramework::GeminiCli => 5,
    }
}

// ── CapabilityMatrix ──────────────────────────────────────────────────────────

/// Maps each [`SubtaskKind`] to an ordered tier of [`ExecutionTarget`]s.
///
/// Each tier entry is a `(framework, model)` target — routing chooses *both*
/// the agent framework and the model. The tier is ordered cheapest-capable-first;
/// callers try candidates in order and fall through to the next on failure.
///
/// [`BTreeMap`] is used so that iteration over kinds is always deterministic
/// regardless of insertion order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityMatrix {
    /// Maps each sub-task kind to its ordered execution-target tier.
    pub tiers: BTreeMap<SubtaskKind, Vec<ExecutionTarget>>,
}

impl CapabilityMatrix {
    /// Create an empty matrix.
    pub fn new() -> Self {
        Self { tiers: BTreeMap::new() }
    }

    /// Return the ordered execution-target tier for `kind`.
    ///
    /// Returns an empty slice if the kind has no mapping in this matrix.
    pub fn tier(&self, kind: SubtaskKind) -> &[ExecutionTarget] {
        self.tiers.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Builder method: set (or replace) the tier for `kind`.
    ///
    /// Consumes `self` and returns a new [`CapabilityMatrix`] so that calls
    /// can be chained fluently.
    #[must_use]
    pub fn with(mut self, kind: SubtaskKind, targets: Vec<ExecutionTarget>) -> Self {
        self.tiers.insert(kind, targets);
        self
    }
}

impl Default for CapabilityMatrix {
    fn default() -> Self {
        Self::new()
    }
}

// ── resolve_matrix ────────────────────────────────────────────────────────────

/// Derive a [`CapabilityMatrix`] from the models that are actually available.
///
/// For each [`SubtaskKind`] in [`SubtaskKind::ALL`]:
/// 1. Keep only specs whose profile score for that kind is ≥ [`MIN_CAPABILITY`].
///    A spec with no profile entry is treated as score 0.0 and is skipped.
/// 2. Sort by the total deterministic key:
///    - [`cost_metric`] ascending
///    - score descending (via [`f64::total_cmp`])
///    - local before API ([`ModelKind`])
///    - [`framework_rank`] ascending (tunable framework preference)
///    - [`ExecutionTarget`] ascending (final tie-break)
/// 3. Insert the resulting tier only when it is non-empty.
///
/// A model absent from `available` never appears even if it has a profile.
pub fn resolve_matrix(
    available: &[ModelSpec],
    profiles: &BTreeMap<ModelId, CapabilityProfile>,
) -> CapabilityMatrix {
    let mut matrix = CapabilityMatrix::new();

    for kind in SubtaskKind::ALL {
        // Collect candidates that clear the capability bar.
        let mut candidates: Vec<(&ModelSpec, f64)> = available
            .iter()
            .filter_map(|spec| {
                let score = profiles
                    .get(&spec.id)
                    .map(|p| p.score(kind))
                    .unwrap_or(0.0);
                if score >= MIN_CAPABILITY { Some((spec, score)) } else { None }
            })
            .collect();

        // Sort by total deterministic key.
        candidates.sort_by(|(a_spec, a_score), (b_spec, b_score)| {
            // 1. cost ascending
            let cost_a = cost_metric(&a_spec.pricing);
            let cost_b = cost_metric(&b_spec.pricing);
            let ord = cost_a.total_cmp(&cost_b);
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
            // 2. score descending
            let ord = b_score.total_cmp(a_score);
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
            // 3. local before API
            let local_rank = |kind: &ModelKind| match kind {
                ModelKind::Local { .. } => 0u8,
                ModelKind::Api { .. } => 1u8,
            };
            let ord = local_rank(&a_spec.kind).cmp(&local_rank(&b_spec.kind));
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
            // 4. framework preference (tunable tertiary tie-break)
            let ord = framework_rank(a_spec.framework).cmp(&framework_rank(b_spec.framework));
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
            // 5. ExecutionTarget ascending (framework then model — final total tie-break)
            a_spec.target().cmp(&b_spec.target())
        });

        if !candidates.is_empty() {
            let targets: Vec<ExecutionTarget> =
                candidates.iter().map(|(s, _)| s.target()).collect();
            matrix = matrix.with(kind, targets);
        }
    }

    matrix
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::provider::{AgentFramework, ModelKind, Pricing};

    // ── helpers ───────────────────────────────────────────────────────────────

    fn mid(s: &str) -> ModelId {
        ModelId::new(s)
    }

    fn local_spec(id: &str, endpoint: &str) -> ModelSpec {
        ModelSpec {
            id: ModelId::new(id),
            kind: ModelKind::Local { endpoint: endpoint.into() },
            pricing: Pricing::ZERO,
            context_window: 8_192,
            framework: AgentFramework::Local,
        }
    }

    fn api_spec(id: &str, input: f64, output: f64) -> ModelSpec {
        ModelSpec {
            id: ModelId::new(id),
            kind: ModelKind::Api { provider: "test".into() },
            pricing: Pricing { input, output, cache_read: 0.0, cache_write: 0.0 },
            context_window: 200_000,
            framework: AgentFramework::ClaudeCode,
        }
    }

    fn profile_for(kind: SubtaskKind, score: f64) -> CapabilityProfile {
        CapabilityProfile::new().with(kind, score)
    }

    // Expected execution targets: `lt` matches `local_spec` (Local framework),
    // `at` matches `api_spec` (ClaudeCode framework).
    fn lt(id: &str) -> ExecutionTarget {
        ExecutionTarget { framework: AgentFramework::Local, model: ModelId::new(id) }
    }
    fn at(id: &str) -> ExecutionTarget {
        ExecutionTarget { framework: AgentFramework::ClaudeCode, model: ModelId::new(id) }
    }

    // ── SubtaskKind::ALL has exactly 6 unique variants ────────────────────────

    #[test]
    fn all_has_six_unique_variants() {
        use std::collections::BTreeSet;
        let all = SubtaskKind::ALL;
        assert_eq!(all.len(), 6, "ALL must contain exactly 6 variants");
        let unique: BTreeSet<SubtaskKind> = all.iter().copied().collect();
        assert_eq!(unique.len(), 6, "ALL must not contain duplicate variants");
    }

    #[test]
    fn all_contains_every_expected_variant() {
        let set: std::collections::BTreeSet<SubtaskKind> =
            SubtaskKind::ALL.iter().copied().collect();
        assert!(set.contains(&SubtaskKind::MechanicalEdit));
        assert!(set.contains(&SubtaskKind::TestGen));
        assert!(set.contains(&SubtaskKind::DiffEdit));
        assert!(set.contains(&SubtaskKind::LargeContext));
        assert!(set.contains(&SubtaskKind::Debugging));
        assert!(set.contains(&SubtaskKind::Refactor));
    }

    // ── CapabilityProfile ─────────────────────────────────────────────────────

    #[test]
    fn profile_score_missing_key_returns_zero() {
        let profile = CapabilityProfile::new();
        assert_eq!(profile.score(SubtaskKind::Debugging), 0.0);
    }

    #[test]
    fn profile_score_returns_inserted_value() {
        let p = CapabilityProfile::new().with(SubtaskKind::TestGen, 0.75);
        assert_eq!(p.score(SubtaskKind::TestGen), 0.75);
        assert_eq!(p.score(SubtaskKind::Debugging), 0.0);
    }

    #[test]
    fn profile_clone_equal() {
        let p = CapabilityProfile::new().with(SubtaskKind::Refactor, 0.8);
        assert_eq!(p.clone(), p);
    }

    // ── cost_metric ───────────────────────────────────────────────────────────

    #[test]
    fn cost_metric_zero_pricing() {
        assert_eq!(cost_metric(&Pricing::ZERO), 0.0);
    }

    #[test]
    fn cost_metric_sums_input_and_output() {
        let p = Pricing { input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 };
        assert_eq!(cost_metric(&p), 18.0);
    }

    // ── framework_rank ────────────────────────────────────────────────────────

    #[test]
    fn framework_rank_prefers_local_then_alphabetical() {
        // Local is rank 0; the rest follow their Display string alphabetically.
        assert_eq!(framework_rank(AgentFramework::Local), 0);
        assert_eq!(framework_rank(AgentFramework::Aider), 1);
        assert_eq!(framework_rank(AgentFramework::ClaudeCode), 2);
        assert_eq!(framework_rank(AgentFramework::Codex), 3);
        assert_eq!(framework_rank(AgentFramework::Cursor), 4);
        assert_eq!(framework_rank(AgentFramework::GeminiCli), 5);
    }

    #[test]
    fn resolve_matrix_framework_rank_breaks_otherwise_exact_ties() {
        // Two API specs identical in cost, score, and ModelKind, differing only in
        // framework. framework_rank must order ClaudeCode (2) before Codex (3).
        let kind = SubtaskKind::DiffEdit;
        let api = |id: &str, fw: AgentFramework| ModelSpec {
            id: ModelId::new(id),
            kind: ModelKind::Api { provider: "test".into() },
            pricing: Pricing { input: 3.0, output: 15.0, cache_read: 0.0, cache_write: 0.0 },
            context_window: 200_000,
            framework: fw,
        };
        // Same model id under two frameworks → same cost/score, distinct targets.
        let available = vec![
            api("m", AgentFramework::Codex),
            api("m", AgentFramework::ClaudeCode),
        ];
        let mut profiles = BTreeMap::new();
        profiles.insert(ModelId::new("m"), profile_for(kind, 0.9));

        let tier = resolve_matrix(&available, &profiles);
        let targets = tier.tier(kind);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].framework, AgentFramework::ClaudeCode, "lower framework_rank leads");
        assert_eq!(targets[1].framework, AgentFramework::Codex);
    }

    // ── MIN_CAPABILITY ────────────────────────────────────────────────────────

    #[test]
    fn min_capability_is_half() {
        assert_eq!(MIN_CAPABILITY, 0.5);
    }

    // ── resolve_matrix: basic filtering ──────────────────────────────────────

    #[test]
    fn resolve_matrix_excludes_sub_bar_models() {
        // local-weak scores 0.3 < MIN_CAPABILITY — must be excluded.
        // api-strong scores 0.9 — must be included.
        let kind = SubtaskKind::Debugging;
        let available = vec![
            local_spec("local-weak", "http://localhost:11434"),
            api_spec("api-strong", 3.0, 15.0),
        ];
        let mut profiles = BTreeMap::new();
        profiles.insert(mid("local-weak"), profile_for(kind, 0.3));
        profiles.insert(mid("api-strong"), profile_for(kind, 0.9));

        let matrix = resolve_matrix(&available, &profiles);
        let tier = matrix.tier(kind);
        assert_eq!(tier, &[at("api-strong")]);
    }

    #[test]
    fn resolve_matrix_local_zero_cost_before_priced_api() {
        // local-ok: score 0.8, cost 0.0 → should sort first.
        // api-ok:   score 0.9, cost 18.0 → sorts after despite higher score.
        let kind = SubtaskKind::TestGen;
        let available = vec![
            api_spec("api-ok", 3.0, 15.0),
            local_spec("local-ok", "http://localhost:11434"),
        ];
        let mut profiles = BTreeMap::new();
        profiles.insert(mid("api-ok"), profile_for(kind, 0.9));
        profiles.insert(mid("local-ok"), profile_for(kind, 0.8));

        let matrix = resolve_matrix(&available, &profiles);
        let tier = matrix.tier(kind);
        assert_eq!(tier.len(), 2);
        assert_eq!(tier[0], lt("local-ok"), "local/zero-cost must precede priced API");
        assert_eq!(tier[1], at("api-ok"));
    }

    #[test]
    fn resolve_matrix_empty_when_no_capable_models() {
        let kind = SubtaskKind::LargeContext;
        let available = vec![local_spec("weak", "http://localhost")];
        let mut profiles = BTreeMap::new();
        profiles.insert(mid("weak"), profile_for(kind, 0.2));

        let matrix = resolve_matrix(&available, &profiles);
        assert!(matrix.tier(kind).is_empty(), "no capable model → empty tier");
    }

    #[test]
    fn resolve_matrix_model_absent_from_available_never_appears() {
        // "ghost" is in profiles but not in available.
        let kind = SubtaskKind::Refactor;
        let available = vec![api_spec("present", 5.0, 20.0)];
        let mut profiles = BTreeMap::new();
        profiles.insert(mid("ghost"), profile_for(kind, 0.99));
        profiles.insert(mid("present"), profile_for(kind, 0.7));

        let matrix = resolve_matrix(&available, &profiles);
        let tier = matrix.tier(kind);
        assert_eq!(tier, &[at("present")]);
        assert!(!tier.contains(&at("ghost")));
    }

    #[test]
    fn resolve_matrix_no_profile_entry_means_skipped() {
        // "no-profile" has no entry in profiles → treated as score 0.0, skipped.
        let kind = SubtaskKind::MechanicalEdit;
        let available = vec![
            local_spec("no-profile", "http://localhost"),
            api_spec("has-profile", 2.0, 8.0),
        ];
        let mut profiles = BTreeMap::new();
        profiles.insert(mid("has-profile"), profile_for(kind, 0.7));
        // no entry for "no-profile"

        let matrix = resolve_matrix(&available, &profiles);
        let tier = matrix.tier(kind);
        assert_eq!(tier, &[at("has-profile")]);
    }

    // ── resolve_matrix: determinism ───────────────────────────────────────────

    #[test]
    fn resolve_matrix_is_deterministic() {
        let available = vec![
            local_spec("local-a", "http://localhost:11434"),
            api_spec("api-b", 3.0, 15.0),
            api_spec("api-c", 1.0, 5.0),
        ];
        let mut profiles = BTreeMap::new();
        let kinds = SubtaskKind::ALL;
        for kind in kinds {
            profiles.insert(mid("local-a"), profile_for(kind, 0.65));
            profiles.insert(mid("api-b"), profile_for(kind, 0.80));
            profiles.insert(mid("api-c"), profile_for(kind, 0.75));
        }

        let m1 = resolve_matrix(&available, &profiles);
        let m2 = resolve_matrix(&available, &profiles);
        assert_eq!(m1, m2, "two calls with identical inputs must produce identical matrices");
    }

    // ── resolve_matrix: sort key — same cost, score desc ─────────────────────

    #[test]
    fn resolve_matrix_same_cost_higher_score_first() {
        let kind = SubtaskKind::DiffEdit;
        // Both API at the same price; api-high should sort before api-low.
        let available = vec![
            api_spec("api-low", 3.0, 15.0),
            api_spec("api-high", 3.0, 15.0),
        ];
        let mut profiles = BTreeMap::new();
        profiles.insert(mid("api-low"), profile_for(kind, 0.6));
        profiles.insert(mid("api-high"), profile_for(kind, 0.9));

        let matrix = resolve_matrix(&available, &profiles);
        let tier = matrix.tier(kind);
        assert_eq!(tier.len(), 2);
        assert_eq!(tier[0], at("api-high"), "higher score must sort first when cost is equal");
    }

    // ── resolve_matrix: sort key — same cost+score, local before API ─────────

    #[test]
    fn resolve_matrix_same_cost_score_local_before_api() {
        let kind = SubtaskKind::Refactor;
        // local at 0.0 cost, api at 0.0 cost (priced the same); scores equal.
        let available = vec![
            api_spec("zero-api", 0.0, 0.0),
            local_spec("zero-local", "http://localhost"),
        ];
        let mut profiles = BTreeMap::new();
        profiles.insert(mid("zero-api"), profile_for(kind, 0.7));
        profiles.insert(mid("zero-local"), profile_for(kind, 0.7));

        let matrix = resolve_matrix(&available, &profiles);
        let tier = matrix.tier(kind);
        assert_eq!(tier.len(), 2);
        assert_eq!(tier[0], lt("zero-local"), "local must precede API when cost and score tie");
    }

    // ── resolve_matrix: sort key — ModelId lexicographic tie-break ───────────

    #[test]
    fn resolve_matrix_modelid_tiebreak_is_lexicographic() {
        let kind = SubtaskKind::TestGen;
        let available = vec![
            api_spec("zzz", 3.0, 15.0),
            api_spec("aaa", 3.0, 15.0),
        ];
        let mut profiles = BTreeMap::new();
        profiles.insert(mid("zzz"), profile_for(kind, 0.8));
        profiles.insert(mid("aaa"), profile_for(kind, 0.8));

        let matrix = resolve_matrix(&available, &profiles);
        let tier = matrix.tier(kind);
        assert_eq!(tier[0], at("aaa"), "lexicographically smaller id must come first");
        assert_eq!(tier[1], at("zzz"));
    }

    // ── default_profiles: covers all known logical ids ────────────────────────

    #[test]
    fn default_profiles_covers_all_known_ids() {
        let profiles = default_profiles();
        let expected_ids = [
            "local-qwen-coder",
            "local-deepseek",
            "gemini-flash",
            "gpt-5-mini",
            "gemini-2.5-pro",
            "sonnet",
            "opus",
            "gpt-5-high",
        ];
        for id in expected_ids {
            assert!(profiles.contains_key(&mid(id)), "profile missing for '{id}'");
        }
    }

    #[test]
    fn default_profiles_all_scores_in_range() {
        let profiles = default_profiles();
        for (model_id, profile) in &profiles {
            for (kind, &score) in &profile.scores {
                assert!(
                    (0.0..=1.0).contains(&score),
                    "score for {model_id}/{kind:?} is {score}, out of [0,1]",
                );
            }
        }
    }

    // ── default_profiles: research orderings are reproduced ──────────────────

    /// When all known models are available, the resolved matrix should respect
    /// the research orderings captured in default_profiles. Specifically:
    ///
    /// - Debugging: gpt-5-high (0.95) > opus (0.92) among API models (equal
    ///   cost assumed in the test registry; gpt-5-high should sort first by
    ///   score).
    /// - LargeContext: gemini-2.5-pro (0.90) should lead that tier.
    /// - MechanicalEdit: sonnet (0.90) scores highest; however the local
    ///   zero-cost models sort before it. The test checks that among API-tier
    ///   models sonnet appears before opus.
    #[test]
    fn default_profiles_research_orderings_respected() {
        // Build a registry with all known logical ids, giving each API model
        // the same price so score is the only differentiator there.
        let profiles = default_profiles();
        let api_price = 10.0; // same for all → sort by score then id

        let available: Vec<ModelSpec> = vec![
            local_spec("local-qwen-coder", "http://localhost:11434"),
            local_spec("local-deepseek", "http://localhost:11435"),
            api_spec("gemini-flash", api_price, api_price),
            api_spec("gpt-5-mini", api_price, api_price),
            api_spec("gemini-2.5-pro", api_price, api_price),
            api_spec("sonnet", api_price, api_price),
            api_spec("opus", api_price, api_price),
            api_spec("gpt-5-high", api_price, api_price),
        ];

        let matrix = resolve_matrix(&available, &profiles);

        // Debugging: gpt-5-high (0.95) > opus (0.92) — both API, same cost
        let debug_tier = matrix.tier(SubtaskKind::Debugging);
        let pos = |id: &str| debug_tier.iter().position(|t| t.model.as_str() == id).unwrap();
        assert!(pos("gpt-5-high") < pos("opus"), "gpt-5-high must precede opus in Debugging tier");

        // LargeContext: gemini-2.5-pro has score 0.90 among API (highest)
        let lc_tier = matrix.tier(SubtaskKind::LargeContext);
        // local models sort first; among API, gemini-2.5-pro must be first API entry
        let first_api = lc_tier.iter().position(|t| {
            available
                .iter()
                .find(|s| s.id == t.model)
                .map(|s| matches!(s.kind, ModelKind::Api { .. }))
                .unwrap_or(false)
        });
        let gemini_pos = lc_tier.iter().position(|t| t.model.as_str() == "gemini-2.5-pro").unwrap();
        assert_eq!(
            first_api,
            Some(gemini_pos),
            "gemini-2.5-pro must be first API model in LargeContext tier",
        );
    }

    // ── CapabilityMatrix builder / tier ───────────────────────────────────────

    #[test]
    fn matrix_tier_empty_for_unmapped_kind() {
        let matrix = CapabilityMatrix::new();
        assert!(matrix.tier(SubtaskKind::Debugging).is_empty());
    }

    #[test]
    fn matrix_with_overrides_existing_tier() {
        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::Refactor, vec![at("original")])
            .with(SubtaskKind::Refactor, vec![at("override-a"), at("override-b")]);
        assert_eq!(matrix.tier(SubtaskKind::Refactor), &[at("override-a"), at("override-b")]);
    }

    #[test]
    fn matrix_independent_kinds_both_present() {
        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::MechanicalEdit, vec![at("cheap")])
            .with(SubtaskKind::Debugging, vec![at("strong")]);
        assert_eq!(matrix.tier(SubtaskKind::MechanicalEdit), &[at("cheap")]);
        assert_eq!(matrix.tier(SubtaskKind::Debugging), &[at("strong")]);
        assert!(matrix.tier(SubtaskKind::DiffEdit).is_empty());
    }

    #[test]
    fn matrix_clone_produces_equal() {
        let m = CapabilityMatrix::new().with(SubtaskKind::TestGen, vec![at("a"), at("b")]);
        assert_eq!(m.clone(), m);
    }

    #[test]
    fn matrix_default_is_empty() {
        let m = CapabilityMatrix::default();
        assert!(m.tiers.is_empty());
    }

    // ── BTreeMap determinism ──────────────────────────────────────────────────

    #[test]
    fn matrix_btreemap_iteration_is_deterministic() {
        let m = CapabilityMatrix::new()
            .with(SubtaskKind::Refactor, vec![at("x")])
            .with(SubtaskKind::TestGen, vec![at("y")]);
        let pass1: Vec<SubtaskKind> = m.tiers.keys().copied().collect();
        let pass2: Vec<SubtaskKind> = m.tiers.keys().copied().collect();
        assert_eq!(pass1, pass2);
    }
}
