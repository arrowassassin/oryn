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
use oryn_core::orchestrator::artificial_analysis::{aa_weights, parse_aa};
use oryn_core::orchestrator::catalog::{
    CapabilityCatalog, CapabilitySource, CatalogProvenance, RawBenchmarks, SourceError,
    default_weights, parse_aider_leaderboard,
};
use oryn_core::orchestrator::catalog_store::{
    CatalogBundle, RefreshPolicy, Store, StoreError, load_and_maybe_refresh,
};
use oryn_core::orchestrator::capability::CapabilityProfile;
use oryn_core::orchestrator::engine::{AdvisorConfig, Engine, EngineConfig};
use oryn_core::orchestrator::harness::{AuthMode, HarnessInvocation};
use oryn_core::orchestrator::listing::{ListCommand, build_targets, default_list_command, parse_model_list};
use oryn_core::orchestrator::pricing::{PricingSource, PricingTable, parse_openrouter_models};
use oryn_core::orchestrator::provider::{AgentFramework, ModelId, ModelSpec};
use oryn_core::orchestrator::runner::{ProcessRunner, SystemProcessRunner};
use oryn_core::orchestrator::task::{Subtask, SubtaskId, SubtaskKind};
use std::collections::BTreeMap;

use crate::launcher::Adapter;
use crate::state::CatalogSource;

/// Capability score assigned to a discovered model we have no benchmark data for
/// yet — enough to keep it routable until real benchmarks arrive.
const DISCOVERY_BASELINE: f64 = 0.6;

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

/// Keyless capability source: the public **Aider polyglot leaderboard** YAML on
/// GitHub raw (no API key, free).
pub struct AiderLeaderboard {
    url: String,
}

impl AiderLeaderboard {
    /// Source from `ORYN_AIDER_URL`, else the aider repo's published leaderboard.
    pub fn from_env() -> Self {
        let url = std::env::var("ORYN_AIDER_URL").unwrap_or_else(|_| {
            "https://raw.githubusercontent.com/Aider-AI/aider/main/aider/website/_data/polyglot_leaderboard.yml".into()
        });
        Self { url }
    }
}

impl CapabilitySource for AiderLeaderboard {
    fn id(&self) -> &str {
        "aider-polyglot"
    }
    fn fetch(&self) -> Result<RawBenchmarks, SourceError> {
        parse_aider_leaderboard(&http_get(&self.url)?)
    }
}

/// HTTP GET with an optional `x-api-key` header.
fn http_get_key(url: &str, key: Option<&str>) -> Result<String, SourceError> {
    let mut req = ureq::get(url);
    if let Some(k) = key {
        req = req.set("x-api-key", k);
    }
    match req.call() {
        Ok(resp) => resp.into_string().map_err(|e| SourceError::Malformed(e.to_string())),
        Err(_) => Err(SourceError::Unavailable),
    }
}

/// Artificial Analysis — the **primary** source: one API for both pricing *and*
/// benchmarks. Requires an API key (`ARTIFICIALANALYSIS_API_KEY`). The same struct
/// serves as both a [`PricingSource`] and a [`CapabilitySource`] (two fetches of
/// the same endpoint on a refresh, which is daily).
pub struct ArtificialAnalysis {
    url: String,
    key: String,
}

impl ArtificialAnalysis {
    /// Configured from `ARTIFICIALANALYSIS_API_KEY` (+ optional `ORYN_AA_URL`).
    /// Returns `None` when no key is set.
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("ARTIFICIALANALYSIS_API_KEY").ok().filter(|s| !s.is_empty())?;
        let url = std::env::var("ORYN_AA_URL")
            .unwrap_or_else(|_| "https://artificialanalysis.ai/api/v2/data/llms/models".into());
        Some(Self { url, key })
    }
}

impl PricingSource for ArtificialAnalysis {
    fn id(&self) -> &str {
        "artificial-analysis"
    }
    fn fetch(&self, now_unix: u64) -> Result<PricingTable, SourceError> {
        let body = http_get_key(&self.url, Some(&self.key))?;
        let aa = parse_aa(&body)?;
        Ok(PricingTable {
            prices: aa.prices,
            provenance: CatalogProvenance { source: "artificial-analysis".into(), fetched_at_unix: now_unix, version: "v2".into() },
        })
    }
}

impl CapabilitySource for ArtificialAnalysis {
    fn id(&self) -> &str {
        "artificial-analysis"
    }
    fn fetch(&self) -> Result<RawBenchmarks, SourceError> {
        let body = http_get_key(&self.url, Some(&self.key))?;
        Ok(parse_aa(&body)?.benchmarks)
    }
}

/// How often (seconds) to consider the parked catalog stale (`ORYN_REFRESH_SECS`,
/// default 24h).
fn refresh_policy() -> RefreshPolicy {
    let secs = std::env::var("ORYN_REFRESH_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(24 * 60 * 60);
    RefreshPolicy::new(secs)
}

/// Load the parked catalog and refresh it from the live sources if stale,
/// re-parking the result. Offline-safe: keeps parked/seed data when a source is
/// down. Safe on a background thread.
///
/// The user chooses the [`CatalogSource`]:
/// - [`CatalogSource::ArtificialAnalysis`] — pricing **and** benchmarks from one
///   API (needs `ARTIFICIALANALYSIS_API_KEY`); falls back to keyless if no key.
/// - [`CatalogSource::Keyless`] — OpenRouter pricing + the public Aider polyglot
///   leaderboard. No key, free.
pub fn load_catalog(source: CatalogSource) -> CatalogBundle {
    let store = FsStore::from_env();
    let policy = refresh_policy();
    let now = now_unix();
    match source {
        CatalogSource::ArtificialAnalysis => {
            if let Some(aa) = ArtificialAnalysis::from_env() {
                return load_and_maybe_refresh(&store, policy, &aa, &aa_weights(), &aa, now);
            }
            // No key → keyless fallback.
            keyless_refresh(&store, policy, now)
        }
        CatalogSource::Keyless => keyless_refresh(&store, policy, now),
    }
}

fn keyless_refresh(store: &FsStore, policy: RefreshPolicy, now: u64) -> CatalogBundle {
    let benchmark = AiderLeaderboard::from_env();
    let pricing = OpenRouterPricing::from_env();
    load_and_maybe_refresh(store, policy, &benchmark, &default_weights(), &pricing, now)
}

/// Make a **real** call to the chosen source and report what came back — the
/// "verify" the UI triggers. For Artificial Analysis this validates the API key.
pub fn verify_source(source: CatalogSource) -> String {
    let now = now_unix();
    match source {
        CatalogSource::ArtificialAnalysis => match ArtificialAnalysis::from_env() {
            None => "Artificial Analysis: no ARTIFICIALANALYSIS_API_KEY set".to_string(),
            Some(aa) => match PricingSource::fetch(&aa, now) {
                Ok(table) => format!("AA key OK · {} models priced + benchmarked", table.prices.len()),
                Err(e) => format!("AA error (check key/host): {e}"),
            },
        },
        CatalogSource::Keyless => {
            let pricing = OpenRouterPricing::from_env();
            let bench = AiderLeaderboard::from_env();
            let p = match PricingSource::fetch(&pricing, now) {
                Ok(t) => format!("OpenRouter {} prices", t.prices.len()),
                Err(e) => format!("OpenRouter err ({e})"),
            };
            let b = match CapabilitySource::fetch(&bench) {
                Ok(r) => format!("Aider {} models", r.metrics.len()),
                Err(e) => format!("Aider err ({e})"),
            };
            format!("{p} · {b}")
        }
    }
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

/// The list command for `framework`: an `ORYN_LIST_<CLI>` env override (whitespace
/// -split), else the documented default. `None` means the framework exposes no
/// listing and contributes no models unless configured.
fn list_command_for(framework: AgentFramework, cli: &str) -> Option<ListCommand> {
    let key = format!("ORYN_LIST_{}", cli.to_uppercase());
    if let Ok(raw) = std::env::var(key) {
        let mut parts = raw.split_whitespace().map(str::to_string);
        if let Some(program) = parts.next() {
            return Some(ListCommand { program, args: parts.collect() });
        }
    }
    default_list_command(framework)
}

/// Ask each selected framework's CLI which models it can access right now, by
/// running its list command via the real process runner and parsing the output.
/// No hardcoded model names — whatever the CLI reports is what we get. Frameworks
/// whose CLI is missing or exposes no listing simply contribute nothing.
pub fn discover_targets(adapters: &[Adapter]) -> Vec<(AgentFramework, ModelId)> {
    let runner = SystemProcessRunner;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut out = Vec::new();
    for adapter in adapters.iter().filter(|a| a.enabled) {
        let framework = framework_for(adapter.cli);
        let Some(cmd) = list_command_for(framework, adapter.cli) else {
            continue;
        };
        let inv = HarnessInvocation {
            program: cmd.program,
            args: cmd.args,
            env: vec![],
            stdin: None,
            cwd: cwd.clone(),
        };
        if let Ok(output) = runner.run(&inv) {
            for model in parse_model_list(framework, &output.stdout_lines) {
                out.push((framework, model));
            }
        }
    }
    out
}

/// Discover models from the CLIs and enrich them into routable specs + capability
/// profiles using the pinned catalog (live pricing + benchmarks, baseline otherwise).
pub fn discover_specs(
    adapters: &[Adapter],
    bundle: &CatalogBundle,
) -> (Vec<ModelSpec>, BTreeMap<ModelId, CapabilityProfile>) {
    let discovered = discover_targets(adapters);
    build_targets(&discovered, &bundle.pricing, &bundle.capability.profiles, DISCOVERY_BASELINE)
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

/// A real readiness check: discovers models live from the CLIs, prices them from
/// the pinned snapshot, constructs the engine, and makes a **live** advisor
/// round-trip.
pub fn check_setup(adapters: &[Adapter], endpoint: &str, model: &str, bundle: &CatalogBundle) -> String {
    let (specs, _profiles) = discover_specs(adapters, bundle);
    let engine = build_engine(endpoint, model, &bundle.capability);
    let advisor_status = probe_advisor(endpoint, model);
    format!(
        "{} model(s) discovered · pricing {} · worktrees {} · {}",
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
