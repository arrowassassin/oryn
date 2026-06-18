//! Artificial Analysis source — pricing **and** benchmarks from one API.
//!
//! <https://artificialanalysis.ai> exposes `/api/v2/data/llms/models` (header
//! `x-api-key`) returning, per model, pricing (USD per **million** tokens) and an
//! `evaluations` block of benchmark indices. This module is the pure parser
//! ([`parse_aa`]) plus the [`aa_weights`] mapping from its coding/intelligence
//! indices onto Oryn's sub-task kinds. Fetching is the app's job.
//!
//! Field names follow the AA v2 schema; the parser is deliberately tolerant
//! (scans for keys containing `coding`/`intelligence`, accepts numbers or numeric
//! strings, normalizes 0–100 indices to 0–1) so it survives minor schema drift —
//! verify exact keys against a live response once the host is allowlisted.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::orchestrator::catalog::{DimensionWeights, RawBenchmarks, SourceError};
use crate::orchestrator::provider::{ModelId, Pricing};
use crate::orchestrator::task::SubtaskKind;

/// Benchmark dimension: Artificial Analysis coding index.
pub const DIM_CODING: &str = "aa-coding-index";
/// Benchmark dimension: Artificial Analysis general intelligence index.
pub const DIM_INTELLIGENCE: &str = "aa-intelligence-index";

/// Pricing + benchmarks parsed from one Artificial Analysis response.
#[derive(Debug, Clone, PartialEq)]
pub struct AaData {
    /// Per-model pricing (USD per million tokens).
    pub prices: BTreeMap<ModelId, Pricing>,
    /// Per-model benchmark scores keyed by dimension.
    pub benchmarks: RawBenchmarks,
}

/// Weights mapping each sub-task kind onto the AA indices: coding-dominant with an
/// intelligence component (large-context leans on general intelligence).
pub fn aa_weights() -> DimensionWeights {
    use SubtaskKind::{Debugging, DiffEdit, LargeContext, MechanicalEdit, Refactor, TestGen};
    DimensionWeights::new()
        .with(MechanicalEdit, DIM_CODING, 0.8)
        .with(MechanicalEdit, DIM_INTELLIGENCE, 0.2)
        .with(TestGen, DIM_CODING, 0.7)
        .with(TestGen, DIM_INTELLIGENCE, 0.3)
        .with(DiffEdit, DIM_CODING, 0.8)
        .with(DiffEdit, DIM_INTELLIGENCE, 0.2)
        .with(LargeContext, DIM_CODING, 0.3)
        .with(LargeContext, DIM_INTELLIGENCE, 0.7)
        .with(Debugging, DIM_CODING, 0.6)
        .with(Debugging, DIM_INTELLIGENCE, 0.4)
        .with(Refactor, DIM_CODING, 0.7)
        .with(Refactor, DIM_INTELLIGENCE, 0.3)
}

/// Read a number from a JSON value that may be a number or a numeric string.
fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

/// Normalize an index that may be on a 0–100 scale to 0–1, clamped.
fn norm(score: f64) -> f64 {
    let s = if score > 1.0 { score / 100.0 } else { score };
    s.clamp(0.0, 1.0)
}

/// Find an `evaluations` value whose key contains `needle` (case-insensitive).
fn eval_containing(evals: &Value, needle: &str) -> Option<f64> {
    evals.as_object()?.iter().find_map(|(k, v)| {
        if k.to_lowercase().contains(needle) {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        } else {
            None
        }
    })
}

/// Parse an Artificial Analysis `/api/v2/data/llms/models` response.
///
/// # Errors
///
/// [`SourceError::Malformed`] if the JSON is unparseable or yields no priced models.
pub fn parse_aa(body: &str) -> Result<AaData, SourceError> {
    let root: Value =
        serde_json::from_str(body).map_err(|e| SourceError::Malformed(e.to_string()))?;
    let items = root
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| root.as_array())
        .ok_or_else(|| SourceError::Malformed("missing `data` array".into()))?;

    let mut prices = BTreeMap::new();
    let mut metrics: BTreeMap<ModelId, BTreeMap<String, f64>> = BTreeMap::new();

    for item in items {
        let id = ["slug", "id", "name"]
            .iter()
            .find_map(|k| item.get(*k).and_then(Value::as_str));
        let Some(id) = id else { continue };
        let model = ModelId::new(id);

        if let Some(pricing) = item.get("pricing") {
            let input = num(pricing.get("price_1m_input_tokens")).unwrap_or(0.0);
            let output = num(pricing.get("price_1m_output_tokens")).unwrap_or(0.0);
            let cache_read = num(pricing.get("price_1m_input_cache_read")).unwrap_or(0.0);
            let cache_write = num(pricing.get("price_1m_input_cache_write")).unwrap_or(0.0);
            prices.insert(
                model.clone(),
                Pricing {
                    input,
                    output,
                    cache_read,
                    cache_write,
                },
            );
        }

        if let Some(evals) = item.get("evaluations") {
            let mut row = BTreeMap::new();
            if let Some(c) = eval_containing(evals, "coding") {
                row.insert(DIM_CODING.to_string(), norm(c));
            }
            if let Some(i) = eval_containing(evals, "intelligence") {
                row.insert(DIM_INTELLIGENCE.to_string(), norm(i));
            }
            if !row.is_empty() {
                metrics.insert(model, row);
            }
        }
    }

    if prices.is_empty() {
        return Err(SourceError::Malformed("no priced models parsed".into()));
    }
    Ok(AaData {
        prices,
        benchmarks: RawBenchmarks { metrics },
    })
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::catalog::map_benchmarks;

    // A fixture shaped like the AA v2 response.
    fn body() -> &'static str {
        r#"{"status":200,"data":[
            {"slug":"claude-opus","name":"Claude Opus",
             "pricing":{"price_1m_input_tokens":15.0,"price_1m_output_tokens":75.0,"price_1m_input_cache_read":1.5},
             "evaluations":{"artificial_analysis_coding_index":62,"artificial_analysis_intelligence_index":70}},
            {"slug":"gpt-mini","name":"GPT mini",
             "pricing":{"price_1m_input_tokens":"0.5","price_1m_output_tokens":"1.5"},
             "evaluations":{"artificial_analysis_coding_index":40,"artificial_analysis_intelligence_index":48}}
        ]}"#
    }

    #[test]
    fn parses_pricing_per_million_directly() {
        let aa = parse_aa(body()).unwrap();
        let opus = aa.prices.get(&ModelId::new("claude-opus")).unwrap();
        assert_eq!(opus.input, 15.0);
        assert_eq!(opus.output, 75.0);
        assert_eq!(opus.cache_read, 1.5);
        // numeric strings parse too
        assert_eq!(aa.prices.get(&ModelId::new("gpt-mini")).unwrap().input, 0.5);
    }

    #[test]
    fn parses_and_normalizes_benchmark_indices() {
        let aa = parse_aa(body()).unwrap();
        let opus = &aa.benchmarks.metrics[&ModelId::new("claude-opus")];
        assert!((opus[DIM_CODING] - 0.62).abs() < 1e-9);
        assert!((opus[DIM_INTELLIGENCE] - 0.70).abs() < 1e-9);
    }

    #[test]
    fn aa_data_feeds_resolve_via_map_benchmarks() {
        let aa = parse_aa(body()).unwrap();
        let profiles = map_benchmarks(&aa.benchmarks, &aa_weights());
        // Debugging = 0.6*coding + 0.4*intelligence for opus = 0.6*0.62 + 0.4*0.70
        let expected = 0.6 * 0.62 + 0.4 * 0.70;
        assert!(
            (profiles[&ModelId::new("claude-opus")].score(SubtaskKind::Debugging) - expected).abs()
                < 1e-9
        );
        // opus should out-score gpt-mini everywhere
        assert!(
            profiles[&ModelId::new("claude-opus")].score(SubtaskKind::DiffEdit)
                > profiles[&ModelId::new("gpt-mini")].score(SubtaskKind::DiffEdit)
        );
    }

    #[test]
    fn aa_weights_cover_all_kinds() {
        let w = aa_weights();
        for kind in SubtaskKind::ALL {
            assert!(w.for_kind(kind).is_some());
        }
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse_aa("nope").is_err());
        assert!(parse_aa(r#"{"data":[]}"#).is_err());
        assert!(parse_aa("{}").is_err());
    }

    #[test]
    fn deterministic() {
        assert_eq!(parse_aa(body()).unwrap(), parse_aa(body()).unwrap());
    }
}
