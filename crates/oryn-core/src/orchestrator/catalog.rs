//! Capability catalog — capability scores sourced from trusted benchmarks,
//! pinned into a versioned, replayable snapshot.
//!
//! Capability is **data, not a hardcoded routing table** (see
//! [`crate::orchestrator::capability`]). The bundled
//! [`default_profiles`](crate::orchestrator::capability::default_profiles) ship as
//! a seed fallback; a [`CapabilitySource`] (Aider polyglot, SWE-bench, long-context
//! leaderboards, …) is mapped through [`DimensionWeights`] into per-model
//! [`CapabilityProfile`]s and frozen into a [`CapabilityCatalog`] alongside its
//! [`CatalogProvenance`].
//!
//! # Determinism & offline operation
//!
//! - [`CapabilityCatalog::seed`] needs no network and reproduces the bundled
//!   defaults exactly — the cold-start / source-down fallback.
//! - [`CapabilityCatalog::refreshed`] takes the `now_unix` timestamp as an
//!   argument; nothing here reads a clock. [`map_benchmarks`] is a pure,
//!   order-independent weighted sum over [`BTreeMap`]s.
//!
//! A mission binds to exactly one catalog snapshot at session start and never
//! fetches mid-run. The refresh-interval policy (how often to call `refreshed`,
//! and falling back to `seed` on [`SourceError`]) lives in the app layer; the HTTP
//! source implementations are a later segment behind the [`CapabilitySource`]
//! trait.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::orchestrator::{
    capability::{CapabilityProfile, default_profiles},
    provider::ModelId,
    task::SubtaskKind,
};

/// Schema version stamped on a freshly-fetched catalog snapshot. The bundled
/// seed uses `"seed"` instead so the two are never confused.
pub const CATALOG_VERSION: &str = "1";

// ── CatalogProvenance ────────────────────────────────────────────────────────

/// Where a [`CapabilityCatalog`]'s profiles came from and when.
///
/// `fetched_at_unix` is always supplied by the caller, never read from a clock,
/// keeping refreshes deterministic and replayable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CatalogProvenance {
    /// Identifier of the source the profiles were derived from (e.g.
    /// `"bundled-seed"`, `"aider-polyglot"`).
    pub source: String,
    /// Unix timestamp (seconds) the snapshot was taken, passed in by the caller.
    pub fetched_at_unix: u64,
    /// Snapshot schema version (`"seed"` for the bundled defaults, otherwise
    /// [`CATALOG_VERSION`]).
    pub version: String,
}

// ── RawBenchmarks ──────────────────────────────────────────────────────────────

/// Raw, un-mapped benchmark metrics fetched from a [`CapabilitySource`].
///
/// `metrics[model][dimension]` is a normalized `0.0..=1.0` score for one model on
/// one benchmark dimension (e.g. `"aider-polyglot"`, `"swe-bench"`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawBenchmarks {
    /// Per-model, per-dimension benchmark scores.
    pub metrics: BTreeMap<ModelId, BTreeMap<String, f64>>,
}

// ── DimensionWeights ────────────────────────────────────────────────────────────

/// How each [`SubtaskKind`] is composed from benchmark dimensions.
///
/// For a kind, `map_benchmarks` computes a weighted average of the model's metrics
/// over the kind's dimensions, then clamps to `0.0..=1.0`. A kind with no weights
/// contributes no profile entry (treated as score 0.0 downstream).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DimensionWeights {
    by_kind: BTreeMap<SubtaskKind, BTreeMap<String, f64>>,
}

impl DimensionWeights {
    /// An empty weighting (every kind maps to nothing).
    pub fn new() -> Self {
        Self { by_kind: BTreeMap::new() }
    }

    /// Builder: add a `weight` for `dimension` under `kind`.
    #[must_use]
    pub fn with(mut self, kind: SubtaskKind, dimension: impl Into<String>, weight: f64) -> Self {
        self.by_kind.entry(kind).or_default().insert(dimension.into(), weight);
        self
    }

    /// The dimension→weight map for `kind`, or `None` if `kind` has no weights.
    pub fn for_kind(&self, kind: SubtaskKind) -> Option<&BTreeMap<String, f64>> {
        self.by_kind.get(&kind)
    }
}

/// Default mapping from [`SubtaskKind`] to benchmark dimensions.
///
/// Dimensions: `"aider-polyglot"` (diff/edit fidelity), `"swe-bench"` (real-world
/// debugging & refactoring), `"long-context"` (large-window retrieval). Kinds that
/// blend two dimensions use a dominant + supporting split so the principal signal
/// drives the ranking while the secondary breaks near-ties.
pub fn default_weights() -> DimensionWeights {
    use SubtaskKind::{Debugging, DiffEdit, LargeContext, MechanicalEdit, Refactor, TestGen};
    DimensionWeights::new()
        .with(MechanicalEdit, "aider-polyglot", 1.0)
        .with(TestGen, "swe-bench", 1.0)
        .with(DiffEdit, "aider-polyglot", 0.8)
        .with(DiffEdit, "swe-bench", 0.2)
        .with(LargeContext, "long-context", 1.0)
        .with(Debugging, "swe-bench", 1.0)
        .with(Refactor, "swe-bench", 0.7)
        .with(Refactor, "aider-polyglot", 0.3)
}

// ── map_benchmarks ──────────────────────────────────────────────────────────────

/// Map raw benchmark metrics into per-model [`CapabilityProfile`]s using `weights`.
///
/// For each model and each [`SubtaskKind`] with weights, the score is the
/// weight-normalized average of the model's metrics over that kind's dimensions
/// (a missing dimension counts as 0.0), clamped to `0.0..=1.0`. A kind with zero
/// total weight, or for which the model has no relevant metrics, yields no entry.
///
/// Pure and deterministic: iteration is over [`BTreeMap`]s and the arithmetic is
/// order-independent.
pub fn map_benchmarks(
    raw: &RawBenchmarks,
    weights: &DimensionWeights,
) -> BTreeMap<ModelId, CapabilityProfile> {
    let mut out = BTreeMap::new();

    for (model, metrics) in &raw.metrics {
        let mut profile = CapabilityProfile::new();

        for kind in SubtaskKind::ALL {
            let Some(dim_weights) = weights.for_kind(kind) else {
                continue;
            };
            let total_weight: f64 = dim_weights.values().sum();
            if total_weight <= 0.0 {
                continue;
            }
            let weighted: f64 = dim_weights
                .iter()
                .map(|(dim, w)| w * metrics.get(dim).copied().unwrap_or(0.0))
                .sum();
            let score = (weighted / total_weight).clamp(0.0, 1.0);
            profile = profile.with(kind, score);
        }

        out.insert(model.clone(), profile);
    }

    out
}

/// Parse a real leaderboard JSON into [`RawBenchmarks`] for a single `dimension`.
///
/// Accepts a top-level array, or an object with a `data`/`results`/`leaderboard`
/// array. Each entry needs a model name (`model`/`name`/`id`) and a score
/// (`score`/`pass_rate`/`resolved`/`percent`/`acc`). Scores above `1.0` are treated
/// as percentages and divided by 100; all are clamped to `0.0..=1.0`. Tolerant of
/// extra fields and unknown entries (skipped), never panicking on drift.
pub fn parse_scored_list(body: &str, dimension: &str) -> Result<RawBenchmarks, SourceError> {
    use serde_json::Value;
    let root: Value = serde_json::from_str(body).map_err(|e| SourceError::Malformed(e.to_string()))?;
    let rows = root
        .as_array()
        .or_else(|| root.get("data").and_then(Value::as_array))
        .or_else(|| root.get("results").and_then(Value::as_array))
        .or_else(|| root.get("leaderboard").and_then(Value::as_array))
        .ok_or_else(|| SourceError::Malformed("no array of results".into()))?;

    let mut metrics: BTreeMap<ModelId, BTreeMap<String, f64>> = BTreeMap::new();
    for row in rows {
        let model = ["model", "name", "id"]
            .iter()
            .find_map(|k| row.get(*k).and_then(Value::as_str));
        let score = ["score", "pass_rate", "resolved", "percent", "acc"]
            .iter()
            .find_map(|k| row.get(*k).and_then(Value::as_f64));
        if let (Some(model), Some(mut score)) = (model, score) {
            if score > 1.0 {
                score /= 100.0;
            }
            metrics
                .entry(ModelId::new(model))
                .or_default()
                .insert(dimension.to_string(), score.clamp(0.0, 1.0));
        }
    }
    if metrics.is_empty() {
        return Err(SourceError::Malformed("no scored entries".into()));
    }
    Ok(RawBenchmarks { metrics })
}

/// Parse the public **Aider polyglot leaderboard** YAML into [`RawBenchmarks`] for
/// the `aider-polyglot` dimension. Keyless — the file lives in the aider repo
/// (`…/_data/polyglot_leaderboard.yml`) and is fetchable from GitHub raw.
///
/// Each list item carries a `model:` and pass rates; the overall score prefers
/// `pass_rate_2`, then `pass_rate_1`, then `percent_cases_well_formed`, normalized
/// to `0.0..=1.0`. A minimal YAML-subset reader (list items + `key: value`) keeps
/// this dependency-free and tolerant of extra fields.
pub fn parse_aider_leaderboard(text: &str) -> Result<RawBenchmarks, SourceError> {
    #[derive(Default)]
    struct Item {
        model: Option<String>,
        pr2: Option<f64>,
        pr1: Option<f64>,
        pct: Option<f64>,
    }

    let mut metrics: BTreeMap<ModelId, BTreeMap<String, f64>> = BTreeMap::new();
    let mut cur = Item::default();

    let mut flush = |item: &mut Item| {
        if let Some(model) = item.model.take()
            && let Some(score) = item.pr2.or(item.pr1).or(item.pct)
        {
            let norm = if score > 1.0 { score / 100.0 } else { score };
            metrics
                .entry(ModelId::new(model))
                .or_default()
                .insert("aider-polyglot".to_string(), norm.clamp(0.0, 1.0));
        }
        *item = Item::default();
    };

    for raw in text.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("- ") || trimmed == "-" {
            flush(&mut cur);
        }
        // Strip an optional leading "- " so the first field of an item parses too.
        let kv = trimmed.trim_start_matches('-').trim();
        if let Some((key, value)) = kv.split_once(':') {
            let value = value.trim().trim_matches('"');
            match key.trim() {
                "model" if !value.is_empty() => cur.model = Some(value.to_string()),
                "pass_rate_2" => cur.pr2 = value.parse().ok(),
                "pass_rate_1" => cur.pr1 = value.parse().ok(),
                "percent_cases_well_formed" => cur.pct = value.parse().ok(),
                _ => {}
            }
        }
    }
    flush(&mut cur);

    if metrics.is_empty() {
        return Err(SourceError::Malformed("no leaderboard entries".into()));
    }
    Ok(RawBenchmarks { metrics })
}

// ── SourceError ────────────────────────────────────────────────────────────────

/// Errors a [`CapabilitySource`] can return.
#[derive(Debug, Error)]
pub enum SourceError {
    /// The source could not be reached (network failure, endpoint down, …).
    #[error("capability source unavailable")]
    Unavailable,
    /// The source responded but its payload could not be parsed. The string
    /// carries a human-readable reason.
    #[error("malformed benchmark payload: {0}")]
    Malformed(String),
}

// ── CapabilitySource trait ───────────────────────────────────────────────────

/// Object-safe trait for a benchmark provider.
///
/// Concrete implementations (Aider polyglot, SWE-bench, OpenRouter leaderboards)
/// land in a later segment; this defines the contract and the in-memory mapping.
pub trait CapabilitySource {
    /// Stable identifier recorded in [`CatalogProvenance::source`].
    fn id(&self) -> &str;

    /// Fetch the latest raw benchmark metrics.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::Unavailable`] when the source cannot be reached and
    /// [`SourceError::Malformed`] when its payload cannot be parsed.
    fn fetch(&self) -> Result<RawBenchmarks, SourceError>;
}

// ── CapabilityCatalog ────────────────────────────────────────────────────────

/// A pinned, provenance-stamped snapshot of capability profiles.
///
/// `Eq` is not derived because profile scores are `f64`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityCatalog {
    /// Per-model capability profiles.
    pub profiles: BTreeMap<ModelId, CapabilityProfile>,
    /// Where these profiles came from and when.
    pub provenance: CatalogProvenance,
}

impl CapabilityCatalog {
    /// The bundled-seed catalog: the defaults from
    /// [`default_profiles`](crate::orchestrator::capability::default_profiles),
    /// stamped with the `"bundled-seed"` provenance. No network, fully offline.
    pub fn seed() -> Self {
        Self {
            profiles: default_profiles(),
            provenance: CatalogProvenance {
                source: "bundled-seed".to_string(),
                fetched_at_unix: 0,
                version: "seed".to_string(),
            },
        }
    }

    /// Build a catalog by fetching `source`, mapping it through `weights`, and
    /// stamping provenance with `now_unix` (supplied by the caller).
    ///
    /// The caller is expected to fall back to [`seed`](Self::seed) on error.
    ///
    /// # Errors
    ///
    /// Propagates any [`SourceError`] from `source.fetch()`.
    pub fn refreshed(
        source: &dyn CapabilitySource,
        weights: &DimensionWeights,
        now_unix: u64,
    ) -> Result<Self, SourceError> {
        let raw = source.fetch()?;
        let profiles = map_benchmarks(&raw, weights);
        Ok(Self {
            profiles,
            provenance: CatalogProvenance {
                source: source.id().to_string(),
                fetched_at_unix: now_unix,
                version: CATALOG_VERSION.to_string(),
            },
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::capability::resolve_matrix;
    use crate::orchestrator::provider::{AgentFramework, ModelKind, ModelSpec, Pricing};

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "expected {b}, got {a}");
    }

    // ── FakeSource ──────────────────────────────────────────────────────────────

    struct FakeSource {
        id: String,
        result: Result<RawBenchmarks, ()>,
    }

    impl FakeSource {
        fn ok(id: &str, raw: RawBenchmarks) -> Self {
            Self { id: id.to_string(), result: Ok(raw) }
        }
        fn failing(id: &str) -> Self {
            Self { id: id.to_string(), result: Err(()) }
        }
    }

    impl CapabilitySource for FakeSource {
        fn id(&self) -> &str {
            &self.id
        }
        fn fetch(&self) -> Result<RawBenchmarks, SourceError> {
            match &self.result {
                Ok(raw) => Ok(raw.clone()),
                Err(()) => Err(SourceError::Unavailable),
            }
        }
    }

    fn metrics(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|&(d, v)| (d.to_string(), v)).collect()
    }

    fn raw_two_models() -> RawBenchmarks {
        let mut m = BTreeMap::new();
        m.insert(
            ModelId::new("strong-aider"),
            metrics(&[("aider-polyglot", 0.9), ("swe-bench", 0.5), ("long-context", 0.6)]),
        );
        m.insert(
            ModelId::new("weak-aider"),
            metrics(&[("aider-polyglot", 0.2), ("swe-bench", 0.5), ("long-context", 0.6)]),
        );
        RawBenchmarks { metrics: m }
    }

    // ── seed ────────────────────────────────────────────────────────────────────

    #[test]
    fn seed_profiles_equal_default_profiles() {
        let cat = CapabilityCatalog::seed();
        assert_eq!(cat.profiles, default_profiles());
    }

    #[test]
    fn seed_provenance_is_bundled() {
        let cat = CapabilityCatalog::seed();
        assert_eq!(cat.provenance.source, "bundled-seed");
        assert_eq!(cat.provenance.fetched_at_unix, 0);
        assert_eq!(cat.provenance.version, "seed");
    }

    // ── map_benchmarks ────────────────────────────────────────────────────────

    #[test]
    fn map_benchmarks_diffedit_favours_aider_strong_model() {
        let profiles = map_benchmarks(&raw_two_models(), &default_weights());
        let strong = profiles[&ModelId::new("strong-aider")].score(SubtaskKind::DiffEdit);
        let weak = profiles[&ModelId::new("weak-aider")].score(SubtaskKind::DiffEdit);
        assert!(strong > weak, "aider-strong ({strong}) must outrank aider-weak ({weak}) on DiffEdit");
        // DiffEdit = 0.8*aider + 0.2*swe, normalized by total weight 1.0.
        approx(strong, 0.8 * 0.9 + 0.2 * 0.5);
        approx(weak, 0.8 * 0.2 + 0.2 * 0.5);
    }

    #[test]
    fn map_benchmarks_single_dimension_passthrough() {
        // MechanicalEdit = 1.0 * aider-polyglot, so score == the raw metric.
        let profiles = map_benchmarks(&raw_two_models(), &default_weights());
        approx(profiles[&ModelId::new("strong-aider")].score(SubtaskKind::MechanicalEdit), 0.9);
    }

    #[test]
    fn map_benchmarks_normalizes_by_total_weight() {
        // Two equal-weight dimensions → plain average.
        let raw = RawBenchmarks {
            metrics: BTreeMap::from([(
                ModelId::new("m"),
                metrics(&[("a", 1.0), ("b", 0.0)]),
            )]),
        };
        let weights = DimensionWeights::new()
            .with(SubtaskKind::Refactor, "a", 2.0)
            .with(SubtaskKind::Refactor, "b", 2.0);
        let profiles = map_benchmarks(&raw, &weights);
        // (2*1.0 + 2*0.0) / 4 = 0.5
        approx(profiles[&ModelId::new("m")].score(SubtaskKind::Refactor), 0.5);
    }

    #[test]
    fn map_benchmarks_clamps_above_one() {
        let raw = RawBenchmarks {
            metrics: BTreeMap::from([(ModelId::new("m"), metrics(&[("a", 1.5)]))]),
        };
        let weights = DimensionWeights::new().with(SubtaskKind::Debugging, "a", 1.0);
        let profiles = map_benchmarks(&raw, &weights);
        approx(profiles[&ModelId::new("m")].score(SubtaskKind::Debugging), 1.0);
    }

    #[test]
    fn map_benchmarks_missing_dimension_counts_as_zero() {
        // Model has no "swe-bench" metric → Debugging score is 0.0.
        let raw = RawBenchmarks {
            metrics: BTreeMap::from([(ModelId::new("m"), metrics(&[("aider-polyglot", 0.9)]))]),
        };
        let profiles = map_benchmarks(&raw, &default_weights());
        approx(profiles[&ModelId::new("m")].score(SubtaskKind::Debugging), 0.0);
    }

    #[test]
    fn map_benchmarks_kind_without_weights_has_no_entry() {
        let raw = RawBenchmarks {
            metrics: BTreeMap::from([(ModelId::new("m"), metrics(&[("a", 1.0)]))]),
        };
        // Only DiffEdit is weighted; other kinds get no entry → score 0.0.
        let weights = DimensionWeights::new().with(SubtaskKind::DiffEdit, "a", 1.0);
        let profiles = map_benchmarks(&raw, &weights);
        let p = &profiles[&ModelId::new("m")];
        assert!(p.scores.contains_key(&SubtaskKind::DiffEdit));
        assert!(!p.scores.contains_key(&SubtaskKind::Debugging));
    }

    #[test]
    fn map_benchmarks_zero_total_weight_skips_kind() {
        let raw = RawBenchmarks {
            metrics: BTreeMap::from([(ModelId::new("m"), metrics(&[("a", 1.0)]))]),
        };
        let weights = DimensionWeights::new().with(SubtaskKind::Refactor, "a", 0.0);
        let profiles = map_benchmarks(&raw, &weights);
        assert!(!profiles[&ModelId::new("m")].scores.contains_key(&SubtaskKind::Refactor));
    }

    #[test]
    fn map_benchmarks_is_deterministic() {
        let raw = raw_two_models();
        let w = default_weights();
        assert_eq!(map_benchmarks(&raw, &w), map_benchmarks(&raw, &w));
    }

    // ── refreshed ────────────────────────────────────────────────────────────

    #[test]
    fn refreshed_stamps_provenance_with_supplied_timestamp() {
        let src = FakeSource::ok("aider-polyglot", raw_two_models());
        let cat = CapabilityCatalog::refreshed(&src, &default_weights(), 1_700_000_000).unwrap();
        assert_eq!(cat.provenance.source, "aider-polyglot");
        assert_eq!(cat.provenance.fetched_at_unix, 1_700_000_000);
        assert_eq!(cat.provenance.version, CATALOG_VERSION);
    }

    #[test]
    fn refreshed_profiles_reflect_fetched_metrics() {
        let src = FakeSource::ok("aider-polyglot", raw_two_models());
        let cat = CapabilityCatalog::refreshed(&src, &default_weights(), 42).unwrap();
        let expected = map_benchmarks(&raw_two_models(), &default_weights());
        assert_eq!(cat.profiles, expected);
    }

    #[test]
    fn refreshed_surfaces_source_error() {
        let src = FakeSource::failing("down");
        let err = CapabilityCatalog::refreshed(&src, &default_weights(), 0).unwrap_err();
        assert!(matches!(err, SourceError::Unavailable));
    }

    // ── integration with resolve_matrix ─────────────────────────────────────

    #[test]
    fn refreshed_catalog_resolves_into_a_matrix() {
        let src = FakeSource::ok("aider-polyglot", raw_two_models());
        let cat = CapabilityCatalog::refreshed(&src, &default_weights(), 100).unwrap();

        // Make available specs for both benchmarked models.
        let spec = |id: &str| ModelSpec {
            id: ModelId::new(id),
            kind: ModelKind::Api { provider: "test".into() },
            pricing: Pricing { input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 },
            context_window: 128_000,
            framework: AgentFramework::ClaudeCode,
        };
        let available = vec![spec("strong-aider"), spec("weak-aider")];

        let matrix = resolve_matrix(&available, &cat.profiles);
        let diff_tier = matrix.tier(SubtaskKind::DiffEdit);
        // strong-aider clears MIN_CAPABILITY on DiffEdit (0.82); weak-aider (0.26) does not.
        assert_eq!(diff_tier.len(), 1, "only the aider-strong model clears the bar");
        assert_eq!(diff_tier[0].model, ModelId::new("strong-aider"));
    }

    #[test]
    fn seed_catalog_resolves_into_a_matrix() {
        let cat = CapabilityCatalog::seed();
        let spec = |id: &str| ModelSpec {
            id: ModelId::new(id),
            kind: ModelKind::Api { provider: "test".into() },
            pricing: Pricing::ZERO,
            context_window: 128_000,
            framework: AgentFramework::ClaudeCode,
        };
        let available = vec![spec("opus"), spec("local-qwen-coder")];
        let matrix = resolve_matrix(&available, &cat.profiles);
        // opus clears Debugging (0.92); local-qwen-coder (0.40) does not.
        let dbg = matrix.tier(SubtaskKind::Debugging);
        assert_eq!(dbg.len(), 1);
        assert_eq!(dbg[0].model, ModelId::new("opus"));
    }

    // ── error/provenance ergonomics ──────────────────────────────────────────

    #[test]
    fn source_error_displays() {
        assert_eq!(SourceError::Unavailable.to_string(), "capability source unavailable");
        assert_eq!(
            SourceError::Malformed("bad json".into()).to_string(),
            "malformed benchmark payload: bad json"
        );
    }

    #[test]
    fn default_weights_covers_all_kinds() {
        let w = default_weights();
        for kind in SubtaskKind::ALL {
            assert!(w.for_kind(kind).is_some(), "kind {kind:?} must have weights");
        }
    }

    #[test]
    fn provenance_equality() {
        let a = CatalogProvenance {
            source: "x".into(),
            fetched_at_unix: 1,
            version: "1".into(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    // ── parse_scored_list ────────────────────────────────────────────────────

    #[test]
    fn parse_scored_list_array_with_percentages() {
        let raw = parse_scored_list(
            r#"[{"model":"opus","pass_rate":92.0},{"model":"sonnet","pass_rate":80}]"#,
            "swe-bench",
        )
        .unwrap();
        approx(raw.metrics[&ModelId::new("opus")]["swe-bench"], 0.92);
        approx(raw.metrics[&ModelId::new("sonnet")]["swe-bench"], 0.80);
    }

    #[test]
    fn parse_scored_list_handles_results_envelope_and_fraction_scores() {
        let raw = parse_scored_list(
            r#"{"results":[{"name":"gemini-2.5-pro","score":0.75}]}"#,
            "aider-polyglot",
        )
        .unwrap();
        approx(raw.metrics[&ModelId::new("gemini-2.5-pro")]["aider-polyglot"], 0.75);
    }

    #[test]
    fn parse_scored_list_feeds_map_benchmarks() {
        let raw = parse_scored_list(r#"[{"model":"opus","score":0.9}]"#, "aider-polyglot").unwrap();
        let profiles = map_benchmarks(&raw, &default_weights());
        // MechanicalEdit = 1.0 * aider-polyglot → 0.9
        approx(profiles[&ModelId::new("opus")].score(SubtaskKind::MechanicalEdit), 0.9);
    }

    #[test]
    fn parse_scored_list_rejects_malformed() {
        assert!(parse_scored_list("{}", "d").is_err());
        assert!(parse_scored_list("garbage", "d").is_err());
        assert!(parse_scored_list("[]", "d").is_err());
    }

    // ── parse_aider_leaderboard ──────────────────────────────────────────────

    #[test]
    fn parse_aider_leaderboard_extracts_models_and_pass_rate_2() {
        let yaml = "\
- dirname: 2024-12-21-claude
  model: claude-3.5-sonnet
  pass_rate_1: 70.0
  pass_rate_2: 84.2
  percent_cases_well_formed: 99.6
- dirname: 2024-12-21-gpt
  model: gpt-4o
  pass_rate_2: 33.3
";
        let raw = parse_aider_leaderboard(yaml).unwrap();
        approx(raw.metrics[&ModelId::new("claude-3.5-sonnet")]["aider-polyglot"], 0.842);
        approx(raw.metrics[&ModelId::new("gpt-4o")]["aider-polyglot"], 0.333);
    }

    #[test]
    fn parse_aider_leaderboard_falls_back_when_pass_rate_2_absent() {
        let yaml = "- model: m1\n  pass_rate_1: 50.0\n";
        let raw = parse_aider_leaderboard(yaml).unwrap();
        approx(raw.metrics[&ModelId::new("m1")]["aider-polyglot"], 0.50);
    }

    #[test]
    fn parse_aider_leaderboard_rejects_empty() {
        assert!(parse_aider_leaderboard("# just a comment\n").is_err());
    }
}
