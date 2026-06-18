//! Production backend wiring — the real I/O the [`oryn_core`] engine runs on:
//! the advisor HTTP transport, the subprocess runner, the on-disk catalog store,
//! and the live pricing / benchmark sources.
//!
//! The core is network/process/clock-free; everything that touches the outside
//! world lives here.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oryn_core::orchestrator::advisor::{Http, HttpError, LocalAdvisor, OllamaAdvisor};
use oryn_core::orchestrator::catalog::{
    CapabilityCatalog, CapabilitySource, CatalogProvenance, RawBenchmarks, SourceError,
    default_weights, parse_scored_list,
};
use oryn_core::orchestrator::catalog_store::{
    CatalogBundle, RefreshPolicy, Store, StoreError, load_and_maybe_refresh,
};
use oryn_core::orchestrator::engine::{AdvisorConfig, Engine, EngineConfig};
use oryn_core::orchestrator::harness::AuthMode;
use oryn_core::orchestrator::pricing::{PricingSource, PricingTable, parse_openrouter_models};
use oryn_core::orchestrator::provider::{AgentFramework, ModelId, ModelKind, ModelSpec, Pricing};
use oryn_core::orchestrator::runner::SystemProcessRunner;
use oryn_core::orchestrator::task::{Subtask, SubtaskId, SubtaskKind};

use crate::launcher::Adapter;

/// Current wall-clock seconds since the epoch (the clock lives in the I/O layer).
pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ── advisor transport ───────────────────────────────────────────────────────

/// Real blocking HTTP client (the production transport for the advisor).
pub struct UreqHttp;

impl Http for UreqHttp {
    fn post_json(&self, url: &str, body: &str) -> Result<String, HttpError> {
        match ureq::post(url).set("Content-Type", "application/json").send_string(body) {
            Ok(resp) => resp.into_string().map_err(|_| HttpError::Unreachable),
            Err(ureq::Error::Status(code, _)) => Err(HttpError::Status(code)),
            Err(_) => Err(HttpError::Unreachable),
        }
    }
}

/// Blocking HTTP GET, returning the body or a [`SourceError`].
fn http_get(url: &str) -> Result<String, SourceError> {
    match ureq::get(url).call() {
        Ok(resp) => resp.into_string().map_err(|e| SourceError::Malformed(e.to_string())),
        Err(_) => Err(SourceError::Unavailable),
    }
}

// ── catalog store + live sources ─────────────────────────────────────────────

/// Filesystem-backed [`Store`] that parks the catalog bundle at a JSON path.
pub struct FsStore {
    path: PathBuf,
}

impl FsStore {
    /// Store at `ORYN_CATALOG_PATH`, else `~/.oryn/catalog.json`.
    pub fn from_env() -> Self {
        let path = std::env::var("ORYN_CATALOG_PATH").map(PathBuf::from).unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".oryn").join("catalog.json")
        });
        Self { path }
    }
}

impl Store for FsStore {
    fn read(&self) -> Option<String> {
        std::fs::read_to_string(&self.path).ok()
    }
    fn write(&self, json: &str) -> Result<(), StoreError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Write(e.to_string()))?;
        }
        std::fs::write(&self.path, json).map_err(|e| StoreError::Write(e.to_string()))
    }
}

/// Live pricing from an OpenRouter-compatible `/api/v1/models` endpoint.
pub struct OpenRouterPricing {
    url: String,
}

impl OpenRouterPricing {
    /// Source at `ORYN_PRICING_URL`, else OpenRouter's public models endpoint.
    pub fn from_env() -> Self {
        let url = std::env::var("ORYN_PRICING_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1/models".into());
        Self { url }
    }
}

impl PricingSource for OpenRouterPricing {
    fn id(&self) -> &str {
        "openrouter"
    }
    fn fetch(&self, now_unix: u64) -> Result<PricingTable, SourceError> {
        let body = http_get(&self.url)?;
        let prices = parse_openrouter_models(&body)?;
        Ok(PricingTable {
            prices,
            provenance: CatalogProvenance { source: "openrouter".into(), fetched_at_unix: now_unix, version: "v1".into() },
        })
    }
}

/// Live capability scores from a benchmark leaderboard JSON URL (`ORYN_BENCHMARK_URL`).
/// When no URL is configured it reports unavailable, so the parked/seed capability
/// is kept unchanged.
pub struct HttpBenchmarkSource {
    url: Option<String>,
    dimension: String,
}

impl HttpBenchmarkSource {
    /// Source from `ORYN_BENCHMARK_URL` (+ `ORYN_BENCHMARK_DIMENSION`, default
    /// `aider-polyglot`).
    pub fn from_env() -> Self {
        Self {
            url: std::env::var("ORYN_BENCHMARK_URL").ok().filter(|s| !s.is_empty()),
            dimension: std::env::var("ORYN_BENCHMARK_DIMENSION").unwrap_or_else(|_| "aider-polyglot".into()),
        }
    }
}

impl CapabilitySource for HttpBenchmarkSource {
    fn id(&self) -> &str {
        "http-benchmark"
    }
    fn fetch(&self) -> Result<RawBenchmarks, SourceError> {
        let url = self.url.as_deref().ok_or(SourceError::Unavailable)?;
        let body = http_get(url)?;
        parse_scored_list(&body, &self.dimension)
    }
}

/// How often (seconds) to consider the parked catalog stale (`ORYN_REFRESH_SECS`,
/// default 24h).
fn refresh_policy() -> RefreshPolicy {
    let secs = std::env::var("ORYN_REFRESH_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(24 * 60 * 60);
    RefreshPolicy::new(secs)
}

/// Load the parked catalog and refresh it from the live sources if it is stale,
/// re-parking the result. Offline-safe: keeps parked/seed data when a source is
/// down. This is the "check and refresh for the model you're loading, keep it
/// parked" entrypoint — safe to call on a background thread.
pub fn load_catalog() -> CatalogBundle {
    let store = FsStore::from_env();
    let benchmark = HttpBenchmarkSource::from_env();
    let pricing = OpenRouterPricing::from_env();
    load_and_maybe_refresh(&store, refresh_policy(), &benchmark, &default_weights(), &pricing, now_unix())
}

// ── engine wiring ─────────────────────────────────────────────────────────────

/// The worktree base directory, from `ORYN_WORKTREE_BASE` or a default.
fn worktree_base() -> PathBuf {
    std::env::var("ORYN_WORKTREE_BASE").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(".oryn/worktrees"))
}

/// Build a fully-wired engine for the user-chosen advisor `endpoint` + `model`,
/// using the **pinned** `capability` snapshot, the real process runner, and the
/// real HTTP client. Construction does no I/O.
pub fn build_engine(endpoint: &str, model: &str, capability: &CapabilityCatalog) -> Engine {
    let config = EngineConfig {
        advisor: AdvisorConfig::new(endpoint, model),
        worktree_base: worktree_base(),
        default_auth: AuthMode::Subscription,
    };
    Engine::new(config, Arc::new(SystemProcessRunner), Arc::new(UreqHttp), capability.clone())
}

/// Map the user's selected adapters into routable [`ModelSpec`]s, pricing each from
/// the **pinned pricing snapshot** (fuzzy-matched by model id); local models are
/// free, and anything unpriced falls back to zero so it still routes.
pub fn specs_from_adapters(adapters: &[Adapter], pricing: &PricingTable) -> Vec<ModelSpec> {
    adapters
        .iter()
        .filter(|a| a.enabled)
        .map(|a| {
            let framework = framework_for(a.cli);
            let (kind, default_price) = if framework == AgentFramework::Local {
                (ModelKind::Local { endpoint: "http://localhost:11434".into() }, Pricing::ZERO)
            } else {
                (ModelKind::Api { provider: a.cli.into() }, Pricing::ZERO)
            };
            let price = pricing.price_fuzzy(a.tag).unwrap_or(default_price);
            ModelSpec { id: ModelId::new(a.tag), kind, pricing: price, context_window: 200_000, framework }
        })
        .collect()
}

fn framework_for(cli: &str) -> AgentFramework {
    match cli {
        "claude" => AgentFramework::ClaudeCode,
        "codex" => AgentFramework::Codex,
        "cursor" => AgentFramework::Cursor,
        "aider" => AgentFramework::Aider,
        "gemini" => AgentFramework::GeminiCli,
        _ => AgentFramework::Local,
    }
}

/// A real readiness check: counts configured targets (priced from the pinned
/// snapshot), constructs the engine, and makes a **live** advisor round-trip.
pub fn check_setup(adapters: &[Adapter], endpoint: &str, model: &str, bundle: &CatalogBundle) -> String {
    let specs = specs_from_adapters(adapters, &bundle.pricing);
    let engine = build_engine(endpoint, model, &bundle.capability);
    let advisor_status = probe_advisor(endpoint, model);
    format!(
        "{} target(s) · pricing {} · worktrees {} · {}",
        specs.len(),
        bundle.pricing.provenance.source,
        engine.worktree_base().display(),
        advisor_status,
    )
}

/// Make a real advisor verification call as a connectivity probe.
fn probe_advisor(endpoint: &str, model: &str) -> String {
    let advisor = OllamaAdvisor::new(endpoint, model, Arc::new(UreqHttp));
    let probe = Subtask {
        id: SubtaskId::new("probe"),
        kind: SubtaskKind::Debugging,
        summary: "Confirm the change makes the failing test pass.".into(),
        deps: vec![],
    };
    match advisor.verify(&probe, "Applied the fix; the test suite now passes 14/14.") {
        Ok(v) => format!("advisor OK ({model}) → passed={} score={:.2}", v.passed, v.score),
        Err(e) => format!("advisor unreachable: {e}"),
    }
}
