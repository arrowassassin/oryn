//! Live model pricing — fetched from a real API, pinned, and persisted.
//!
//! Pricing drives the cost tie-break in [`resolve_matrix`](crate::orchestrator::capability::resolve_matrix),
//! so it should reflect *real* per-model rates rather than nominal guesses. This
//! module models a [`PricingTable`] (a pinned snapshot keyed by [`ModelId`], with
//! provenance), a pure parser for the **OpenRouter** `/api/v1/models` shape
//! ([`parse_openrouter_models`]), a [`PricingSource`] trait the app implements over
//! real HTTP, and a bundled [`PricingTable::seed`] fallback for offline/first-run.
//!
//! Rates are normalized to **USD per million tokens** (matching [`Pricing`]); the
//! OpenRouter API reports USD-per-token strings, which this module multiplies up.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::orchestrator::catalog::{CatalogProvenance, SourceError};
use crate::orchestrator::provider::{ModelId, Pricing};

/// Tokens-per-million scale factor (OpenRouter quotes USD per token).
const PER_MILLION: f64 = 1_000_000.0;

/// A pinned snapshot of per-model pricing with provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricingTable {
    /// USD-per-million pricing keyed by model id (the source's id strings).
    pub prices: BTreeMap<ModelId, Pricing>,
    /// Where these prices came from and when.
    pub provenance: CatalogProvenance,
}

impl PricingTable {
    /// Bundled seed pricing — plausible defaults for the well-known logical model
    /// ids, used first-run / offline / when the source is down.
    pub fn seed() -> Self {
        let mut prices = BTreeMap::new();
        let mut put = |id: &str, input: f64, output: f64, cr: f64, cw: f64| {
            prices.insert(ModelId::new(id), Pricing { input, output, cache_read: cr, cache_write: cw });
        };
        put("opus", 15.0, 75.0, 1.5, 18.75);
        put("sonnet", 3.0, 15.0, 0.3, 3.75);
        put("gpt-5-high", 10.0, 30.0, 1.0, 12.5);
        put("gpt-5-mini", 0.5, 1.5, 0.05, 0.625);
        put("gemini-2.5-pro", 1.25, 5.0, 0.125, 1.5625);
        put("gemini-flash", 0.075, 0.30, 0.0075, 0.09375);
        put("local-qwen-coder", 0.0, 0.0, 0.0, 0.0);
        put("local-deepseek", 0.0, 0.0, 0.0, 0.0);
        Self {
            prices,
            provenance: CatalogProvenance {
                source: "bundled-seed".to_string(),
                fetched_at_unix: 0,
                version: "seed".to_string(),
            },
        }
    }

    /// Exact lookup by model id.
    pub fn price(&self, id: &ModelId) -> Option<Pricing> {
        self.prices.get(id).copied()
    }

    /// Tolerant lookup: exact id, else a stored id whose trailing path segment
    /// (after the last `/`) equals `query`, else one that ends with `/query`.
    /// Deterministic — iterates the sorted map and takes the first match.
    pub fn price_fuzzy(&self, query: &str) -> Option<Pricing> {
        if let Some(p) = self.prices.get(&ModelId::new(query)) {
            return Some(*p);
        }
        self.prices
            .iter()
            .find(|(id, _)| {
                let s = id.as_str();
                s == query || s.rsplit('/').next() == Some(query) || s.ends_with(&format!("/{query}"))
            })
            .map(|(_, p)| *p)
    }
}

/// Parse the OpenRouter `/api/v1/models` response into per-model [`Pricing`].
///
/// Expects `{"data": [{"id": "...", "pricing": {"prompt": "0.000003",
/// "completion": "0.000015", ...}}]}`. `prompt`/`completion` are USD per token
/// (strings) and are scaled to USD-per-million. Optional `input_cache_read` /
/// `input_cache_write` are honoured when present, else zero.
///
/// # Errors
///
/// [`SourceError::Malformed`] if the top-level `data` array is missing.
pub fn parse_openrouter_models(body: &str) -> Result<BTreeMap<ModelId, Pricing>, SourceError> {
    let root: Value = serde_json::from_str(body).map_err(|e| SourceError::Malformed(e.to_string()))?;
    let data = root
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| SourceError::Malformed("missing `data` array".into()))?;

    let mut out = BTreeMap::new();
    for entry in data {
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(pricing) = entry.get("pricing") else {
            continue;
        };
        let per_tok = |key: &str| {
            pricing
                .get(key)
                .and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64()))
                .unwrap_or(0.0)
                * PER_MILLION
        };
        out.insert(
            ModelId::new(id),
            Pricing {
                input: per_tok("prompt"),
                output: per_tok("completion"),
                cache_read: per_tok("input_cache_read"),
                cache_write: per_tok("input_cache_write"),
            },
        );
    }
    if out.is_empty() {
        return Err(SourceError::Malformed("no models parsed".into()));
    }
    Ok(out)
}

/// A live pricing provider. Object-safe; the app implements it over real HTTP.
pub trait PricingSource {
    /// Stable identifier recorded in provenance.
    fn id(&self) -> &str;

    /// Fetch the current pricing, stamping `now_unix` into provenance.
    ///
    /// # Errors
    ///
    /// [`SourceError`] when the source is unreachable or malformed.
    fn fetch(&self, now_unix: u64) -> Result<PricingTable, SourceError>;
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_has_known_models_and_free_local() {
        let t = PricingTable::seed();
        assert!(t.price(&ModelId::new("opus")).is_some());
        assert_eq!(t.price(&ModelId::new("local-qwen-coder")).unwrap(), Pricing::ZERO);
        assert_eq!(t.provenance.version, "seed");
    }

    #[test]
    fn parse_openrouter_scales_to_per_million() {
        let body = r#"{"data":[
            {"id":"anthropic/claude-opus","pricing":{"prompt":"0.000015","completion":"0.000075","input_cache_read":"0.0000015"}},
            {"id":"openai/gpt-5","pricing":{"prompt":"0.00001","completion":"0.00003"}}
        ]}"#;
        let prices = parse_openrouter_models(body).unwrap();
        let opus = prices.get(&ModelId::new("anthropic/claude-opus")).unwrap();
        assert!((opus.input - 15.0).abs() < 1e-6);
        assert!((opus.output - 75.0).abs() < 1e-6);
        assert!((opus.cache_read - 1.5).abs() < 1e-6);
        assert_eq!(opus.cache_write, 0.0);
        assert!((prices.get(&ModelId::new("openai/gpt-5")).unwrap().input - 10.0).abs() < 1e-6);
    }

    #[test]
    fn parse_openrouter_rejects_malformed() {
        assert!(parse_openrouter_models("{}").is_err());
        assert!(parse_openrouter_models("not json").is_err());
        assert!(parse_openrouter_models(r#"{"data":[]}"#).is_err());
    }

    #[test]
    fn fuzzy_lookup_matches_trailing_segment() {
        let body = r#"{"data":[{"id":"anthropic/claude-3.7-sonnet","pricing":{"prompt":"0.000003","completion":"0.000015"}}]}"#;
        let prices = parse_openrouter_models(body).unwrap();
        let table = PricingTable {
            prices,
            provenance: CatalogProvenance { source: "openrouter".into(), fetched_at_unix: 1, version: "v1".into() },
        };
        // exact full slug
        assert!(table.price_fuzzy("anthropic/claude-3.7-sonnet").is_some());
        // trailing segment
        assert!(table.price_fuzzy("claude-3.7-sonnet").is_some());
        // miss
        assert!(table.price_fuzzy("opus").is_none());
    }

    #[test]
    fn parse_is_deterministic() {
        let body = r#"{"data":[{"id":"a/b","pricing":{"prompt":"0.000001","completion":"0.000002"}}]}"#;
        assert_eq!(parse_openrouter_models(body).unwrap(), parse_openrouter_models(body).unwrap());
    }
}
