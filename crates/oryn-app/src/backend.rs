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
use oryn_core::orchestrator::capability::CapabilityProfile;
use oryn_core::orchestrator::catalog::{
    CapabilityCatalog, CapabilitySource, CatalogProvenance, RawBenchmarks, SourceError,
    default_weights, parse_aider_leaderboard,
};
use oryn_core::orchestrator::catalog_store::{
    CatalogBundle, RefreshPolicy, Store, StoreError, load_and_maybe_refresh,
};
use oryn_core::orchestrator::engine::{AdvisorConfig, Engine, EngineConfig};
use oryn_core::orchestrator::harness::{AuthMode, HarnessInvocation};
use oryn_core::orchestrator::listing::{
    ListCommand, build_targets, default_list_command, parse_model_list,
};
use oryn_core::orchestrator::pricing::{PricingSource, PricingTable, parse_openrouter_models};
use oryn_core::orchestrator::provider::{AgentFramework, ModelId, ModelSpec};
use oryn_core::orchestrator::runner::{ProcessRunner, SystemProcessRunner};
use oryn_core::orchestrator::task::{Subtask, SubtaskId, SubtaskKind};
use std::collections::BTreeMap;

use oryn_core::worktree::WorktreeManager;

use crate::launcher::Adapter;
use crate::state::{CatalogSource, PersistedConfig};

// ── persisted UI config ───────────────────────────────────────────────────────

/// Where UI preferences are stored: `ORYN_SETTINGS_PATH`, else `~/.oryn/settings.json`.
pub fn config_path() -> PathBuf {
    std::env::var("ORYN_SETTINGS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".oryn").join("settings.json")
        })
}

/// Load persisted UI config, or `None` if absent/unreadable/corrupt (the app then
/// falls back to environment-derived defaults — never an error).
pub fn load_config() -> Option<PersistedConfig> {
    load_config_from(&config_path())
}

/// Persist UI config. Best-effort: I/O failures are swallowed so a read-only home
/// directory never breaks the UI.
pub fn save_config(cfg: &PersistedConfig) {
    save_config_to(&config_path(), cfg);
}

/// Load config from an explicit path (testable without touching the environment).
pub fn load_config_from(path: &std::path::Path) -> Option<PersistedConfig> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Write config as pretty JSON to an explicit path, creating parent dirs.
pub fn save_config_to(path: &std::path::Path, cfg: &PersistedConfig) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(path, json);
    }
}

/// Capability score assigned to a discovered model we have no benchmark data for
/// yet — enough to keep it routable until real benchmarks arrive.
const DISCOVERY_BASELINE: f64 = 0.6;

/// Current wall-clock seconds since the epoch (the clock lives in the I/O layer).
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── advisor transport ───────────────────────────────────────────────────────

/// Real blocking HTTP client (the production transport for the advisor).
pub struct UreqHttp;

impl Http for UreqHttp {
    fn post_json(&self, url: &str, body: &str) -> Result<String, HttpError> {
        match ureq::post(url)
            .set("Content-Type", "application/json")
            .send_string(body)
        {
            Ok(resp) => resp.into_string().map_err(|_| HttpError::Unreachable),
            Err(ureq::Error::Status(code, _)) => Err(HttpError::Status(code)),
            Err(_) => Err(HttpError::Unreachable),
        }
    }
}

/// Blocking HTTP GET, returning the body or a [`SourceError`].
fn http_get(url: &str) -> Result<String, SourceError> {
    match ureq::get(url).call() {
        Ok(resp) => resp
            .into_string()
            .map_err(|e| SourceError::Malformed(e.to_string())),
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
        let path = std::env::var("ORYN_CATALOG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
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
            provenance: CatalogProvenance {
                source: "openrouter".into(),
                fetched_at_unix: now_unix,
                version: "v1".into(),
            },
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
        Ok(resp) => resp
            .into_string()
            .map_err(|e| SourceError::Malformed(e.to_string())),
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
        let key = std::env::var("ARTIFICIALANALYSIS_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())?;
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
            provenance: CatalogProvenance {
                source: "artificial-analysis".into(),
                fetched_at_unix: now_unix,
                version: "v2".into(),
            },
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
    let secs = std::env::var("ORYN_REFRESH_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(24 * 60 * 60);
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
                Ok(table) => format!(
                    "AA key OK · {} models priced + benchmarked",
                    table.prices.len()
                ),
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

/// Detect the project's test command so the engine can verify by execution.
/// `ORYN_TEST_CMD` (whitespace-split) always wins; otherwise it's inferred from
/// the files in `repo_root`. `None` means "no test runner found" — verification
/// then falls back to the advisor.
pub fn detect_test_command(repo_root: &std::path::Path) -> Option<Vec<String>> {
    if let Ok(raw) = std::env::var("ORYN_TEST_CMD") {
        let parts: Vec<String> = raw.split_whitespace().map(str::to_string).collect();
        if !parts.is_empty() {
            return Some(parts);
        }
    }
    let has = |name: &str| repo_root.join(name).exists();
    if has("Cargo.toml") {
        Some(vec!["cargo".into(), "test".into(), "--quiet".into()])
    } else if has("go.mod") {
        Some(vec!["go".into(), "test".into(), "./...".into()])
    } else if has("package.json") {
        Some(vec!["npm".into(), "test".into(), "--silent".into()])
    } else if has("pyproject.toml")
        || has("pytest.ini")
        || has("tox.ini")
        || repo_root.join("tests").is_dir()
    {
        Some(vec!["pytest".into(), "-q".into()])
    } else {
        None
    }
}

/// Promote the winner: apply its worktree's changes onto the repo working tree,
/// then (optionally) tear down the losing worktrees. Blocking — run on a
/// background thread.
pub fn promote_winner(
    repo_root: &std::path::Path,
    winner_session: &str,
    loser_sessions: &[String],
    cleanup: bool,
) -> Result<String, String> {
    let mgr = WorktreeManager::new(repo_root, worktree_base());
    let applied = mgr.promote(winner_session).map_err(|e| e.to_string())?;
    let mut removed = 0;
    if cleanup {
        for s in loser_sessions {
            if mgr.remove(s).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(format!(
        "promoted {} file change(s) into {}{}",
        applied.len(),
        repo_root.display(),
        if cleanup {
            format!(" · tore down {removed} worktree(s)")
        } else {
            String::new()
        },
    ))
}

/// The worktree base directory, from `ORYN_WORKTREE_BASE` or a default.
fn worktree_base() -> PathBuf {
    std::env::var("ORYN_WORKTREE_BASE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".oryn/worktrees"))
}

/// Build a fully-wired engine for the user-chosen advisor `endpoint` + `model`,
/// using the **pinned** `capability` snapshot, the real process runner, and the
/// real HTTP client. Construction does no I/O.
pub fn build_engine(
    endpoint: &str,
    model: &str,
    repo_root: &std::path::Path,
    capability: &CapabilityCatalog,
) -> Engine {
    let config = EngineConfig {
        advisor: AdvisorConfig::new(endpoint, model),
        repo_path: repo_root.to_path_buf(),
        worktree_base: worktree_base(),
        default_auth: AuthMode::Subscription,
        test_command: detect_test_command(repo_root),
    };
    Engine::new(
        config,
        Arc::new(SystemProcessRunner),
        Arc::new(UreqHttp),
        capability.clone(),
    )
}

/// The list command for `framework`: an `ORYN_LIST_<CLI>` env override (whitespace
/// -split), else the documented default. `None` means the framework exposes no
/// listing and contributes no models unless configured.
fn list_command_for(framework: AgentFramework, cli: &str) -> Option<ListCommand> {
    let key = format!("ORYN_LIST_{}", cli.to_uppercase());
    if let Ok(raw) = std::env::var(key) {
        let mut parts = raw.split_whitespace().map(str::to_string);
        if let Some(program) = parts.next() {
            return Some(ListCommand {
                program,
                args: parts.collect(),
            });
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
    build_targets(
        &discovered,
        &bundle.pricing,
        &bundle.capability.profiles,
        DISCOVERY_BASELINE,
    )
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
pub fn check_setup(
    adapters: &[Adapter],
    endpoint: &str,
    model: &str,
    bundle: &CatalogBundle,
) -> String {
    let (specs, _profiles) = discover_specs(adapters, bundle);
    let repo = RepoInfo::detect();
    let engine = build_engine(endpoint, model, &repo.root, &bundle.capability);
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
        Ok(v) => format!(
            "advisor OK ({model}) → passed={} score={:.2}",
            v.passed, v.score
        ),
        Err(e) => format!("advisor unreachable: {e}"),
    }
}

// ── content-addressed artifact store ──────────────────────────────────────────

/// A persistent, content-addressed blob store: each artifact is written once at
/// a path named by the lowercase hex SHA-256 of its bytes. Identical content
/// (e.g. the cache-stable prefix shared across every racing target) is therefore
/// stored exactly once — the dedup primitive behind the Broker view.
pub struct ArtifactStore {
    dir: PathBuf,
}

impl ArtifactStore {
    /// Open the store at `ORYN_STORE_PATH`, else `~/.oryn/store`.
    pub fn open() -> Self {
        let dir = std::env::var("ORYN_STORE_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                PathBuf::from(home).join(".oryn").join("store")
            });
        Self::at(dir)
    }

    /// Open a store rooted at `dir` (used by tests).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Store `bytes`, returning the content id and whether it was newly written
    /// (`false` means an identical blob already existed — a dedup hit).
    pub fn put(&self, bytes: &[u8]) -> (String, bool) {
        use sha2::{Digest, Sha256};
        let id = hex::encode(Sha256::digest(bytes));
        let _ = std::fs::create_dir_all(&self.dir);
        let path = self.dir.join(&id);
        if path.exists() {
            return (id, false);
        }
        let is_new = std::fs::write(&path, bytes).is_ok();
        (id, is_new)
    }

    /// `(artifact_count, total_bytes)` currently on disk.
    pub fn stats(&self) -> (usize, u64) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return (0, 0);
        };
        let mut count = 0;
        let mut bytes = 0;
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                count += 1;
                bytes += meta.len();
            }
        }
        (count, bytes)
    }
}

// ── CLI availability ──────────────────────────────────────────────────────────

/// Whether an executable named `bin` is found on `PATH`. Real detection — checks
/// each `PATH` entry for an existing (executable, on Unix) file.
pub fn is_on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(bin);
        match std::fs::metadata(&candidate) {
            Ok(meta) if meta.is_file() => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    true
                }
            }
            _ => false,
        }
    })
}

/// Mark which adapters' CLIs are actually installed, so Launch shows real
/// availability instead of a static hint.
pub fn mark_cli_availability(adapters: &mut [Adapter]) {
    for a in adapters.iter_mut() {
        a.installed = is_on_path(a.cli);
    }
}

// ── user identity ─────────────────────────────────────────────────────────────

/// The local developer identity, read from real sources: the repo's git config,
/// then the global `~/.gitconfig`, then the OS user. No invented account data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserIdentity {
    pub name: String,
    pub email: String,
    /// Up-to-two-letter initials for the avatar.
    pub initials: String,
}

impl UserIdentity {
    /// Detect the identity for the repository at `repo_root`.
    pub fn detect(repo_root: &std::path::Path) -> Self {
        let (mut name, mut email) = git_user(&repo_root.join(".git").join("config"));
        if name.is_none() || email.is_none() {
            let home = std::env::var("HOME").unwrap_or_default();
            let (gn, ge) = git_user(&PathBuf::from(home).join(".gitconfig"));
            name = name.or(gn);
            email = email.or(ge);
        }
        let os_user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "developer".into());
        let name = name.unwrap_or_else(|| os_user.clone());
        let email = email.unwrap_or_else(|| format!("{os_user}@localhost"));
        let initials = initials_of(&name);
        Self {
            name,
            email,
            initials,
        }
    }
}

/// Parse `name`/`email` from the `[user]` section of a git-config-format file.
fn git_user(path: &std::path::Path) -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    let mut in_user = false;
    let (mut name, mut email) = (None, None);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_user = line.starts_with("[user");
            continue;
        }
        if !in_user {
            continue;
        }
        if let Some(v) = line
            .strip_prefix("name")
            .and_then(|r| r.trim_start().strip_prefix('='))
        {
            name = Some(v.trim().trim_matches('"').to_string());
        } else if let Some(v) = line
            .strip_prefix("email")
            .and_then(|r| r.trim_start().strip_prefix('='))
        {
            email = Some(v.trim().trim_matches('"').to_string());
        }
    }
    (
        name.filter(|s| !s.is_empty()),
        email.filter(|s| !s.is_empty()),
    )
}

/// Initials from a display name: first letters of the first two words, uppercased.
fn initials_of(name: &str) -> String {
    let mut out = String::new();
    for word in name.split_whitespace().take(2) {
        if let Some(c) = word.chars().next() {
            out.extend(c.to_uppercase());
        }
    }
    if out.is_empty() {
        out.push('?');
    }
    out
}

/// Human-readable worktree base directory (`ORYN_WORKTREE_BASE` or the default).
pub fn worktree_base_display() -> String {
    worktree_base().display().to_string()
}

// ── repository detection ──────────────────────────────────────────────────────

/// Real, dependency-free snapshot of the git repository the app is launched in:
/// the worktree root, current branch, short HEAD sha, and a bounded list of
/// source files used to build the cache-stable repo map. Everything here is read
/// straight from the filesystem — no mockups.
#[derive(Debug, Clone)]
pub struct RepoInfo {
    /// Absolute path to the repository (or the cwd when no `.git` is found).
    pub root: PathBuf,
    /// Short display label, e.g. `acme/web-platform`.
    pub label: String,
    /// Current branch name (or `detached`/`unknown`).
    pub branch: String,
    /// Short HEAD commit sha (best-effort; empty if unavailable).
    pub head_short: String,
    /// Bounded, sorted list of tracked-looking source files (for the repo map).
    pub files: Vec<String>,
}

impl RepoInfo {
    /// Detect the repository containing `start` (defaulting to the current dir),
    /// walking up to the first ancestor that contains a `.git` entry.
    pub fn detect() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let root = find_git_root(&cwd).unwrap_or_else(|| cwd.clone());
        let label = repo_label(&root);
        let (branch, head_short) = git_head(&root);
        let files = list_source_files(&root, 400);
        Self {
            root,
            label,
            branch,
            head_short,
            files,
        }
    }

    /// `branch@shortsha`, e.g. `main@4f2ab1c`.
    pub fn base_ref(&self) -> String {
        if self.head_short.is_empty() {
            self.branch.clone()
        } else {
            format!("{}@{}", self.branch, self.head_short)
        }
    }
}

/// Walk up from `start` to the first directory containing a `.git`.
fn find_git_root(start: &std::path::Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// `<parent>/<dir>` label for the repo, falling back to the final path segment.
fn repo_label(root: &std::path::Path) -> String {
    let name = root.file_name().and_then(|s| s.to_str()).unwrap_or("repo");
    match root
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
    {
        Some(parent) if !parent.is_empty() => format!("{parent}/{name}"),
        _ => name.to_string(),
    }
}

/// Read the current branch + short HEAD sha straight from `.git`. Best-effort and
/// dependency-free: parses `.git/HEAD`, then the matching loose ref (packed refs
/// are not resolved — the sha is simply omitted in that case).
fn git_head(root: &std::path::Path) -> (String, String) {
    let head_path = root.join(".git").join("HEAD");
    let Ok(head) = std::fs::read_to_string(&head_path) else {
        return ("unknown".into(), String::new());
    };
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref: ") {
        let branch = reference
            .rsplit('/')
            .next()
            .unwrap_or("unknown")
            .to_string();
        let sha = std::fs::read_to_string(root.join(".git").join(reference))
            .ok()
            .map(|s| s.trim().chars().take(7).collect::<String>())
            .unwrap_or_default();
        (branch, sha)
    } else {
        // Detached HEAD: the file holds the sha directly.
        ("detached".into(), head.chars().take(7).collect())
    }
}

/// Bounded recursive walk collecting source-looking files (repo-relative paths),
/// skipping VCS/build/vendor directories. Sorted and capped at `limit`.
fn list_source_files(root: &std::path::Path, limit: usize) -> Vec<String> {
    const SKIP: &[&str] = &[
        ".git",
        "target",
        "node_modules",
        ".oryn",
        "dist",
        "build",
        ".venv",
        "__pycache__",
    ];
    const EXT: &[&str] = &[
        "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "rb", "php", "cs", "c", "h",
        "cpp", "hpp", "swift", "scala", "md", "toml", "yaml", "yml", "json", "css", "html", "sh",
    ];
    let mut out: Vec<String> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= limit {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') && name != ".gitignore" {
                continue;
            }
            if path.is_dir() {
                if !SKIP.contains(&name.as_ref()) {
                    stack.push(path);
                }
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| EXT.contains(&e))
                && let Ok(rel) = path.strip_prefix(root)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out.truncate(limit);
    out
}

// ── live mission run ──────────────────────────────────────────────────────────

/// One real execution attempt the orchestrator made: a `(framework, model)`
/// target run against a subtask, with the tokens it reported, the cost computed
/// from the pinned pricing, and the advisor's verdict. `won` marks the attempt
/// the cascade selected for that subtask.
#[derive(Debug, Clone)]
pub struct LiveAttempt {
    pub subtask: String,
    pub framework: String,
    pub model: String,
    pub tier_rank: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub passed: bool,
    pub score: f64,
    pub won: bool,
    pub response: String,
    /// Files changed in this target's worktree (winners only; 0 otherwise).
    pub files_changed: usize,
    /// Lines added / removed in the worktree diff (winners only).
    pub added: usize,
    pub removed: usize,
    /// The worktree session id for this target (used to promote/clean up).
    pub worktree_session: String,
}

/// The full, real result of running a mission through the engine — what the UI
/// renders. No simulated fields: every number comes from the orchestrator.
#[derive(Debug, Clone)]
pub struct LiveReport {
    pub goal: String,
    pub repo_label: String,
    pub base_ref: String,
    pub advisor: String,
    /// Number of `(framework, model)` targets discovered from the CLIs.
    pub discovered: usize,
    pub subtasks: usize,
    pub attempts: Vec<LiveAttempt>,
    pub gross_usd: f64,
    pub saved_usd: f64,
    /// Human-readable headline (e.g. a setup hint when nothing was discovered).
    pub note: String,
    /// Content-addressed store stats: total unique artifacts + bytes on disk.
    pub store_artifacts: usize,
    pub store_bytes: u64,
    /// Context bytes this run offered to the store vs. uniquely stored — the real
    /// dedup the shared cache-stable prefix produces (offered ≥ stored).
    pub ctx_offered_bytes: u64,
    pub ctx_stored_bytes: u64,
}

impl LiveReport {
    /// Dedup ratio for this run's context bytes (offered / stored), ≥ 1.0.
    pub fn dedup_ratio(&self) -> f64 {
        if self.ctx_stored_bytes == 0 {
            1.0
        } else {
            self.ctx_offered_bytes as f64 / self.ctx_stored_bytes as f64
        }
    }
}

impl LiveReport {
    /// Total tokens (input + output) across every attempt.
    pub fn total_tokens(&self) -> u64 {
        self.attempts
            .iter()
            .map(|a| a.input_tokens + a.output_tokens)
            .sum()
    }
}

/// Run `goal` against the selected `adapters` through the **real** engine: live
/// CLI model discovery, deterministic decomposition, the route-don't-race
/// cascade, and the local advisor as the verifier. Blocking and side-effecting —
/// call it on a background thread.
pub fn run_live(
    adapters: &[Adapter],
    endpoint: &str,
    model: &str,
    bundle: &CatalogBundle,
    repo: &RepoInfo,
    goal: &str,
    progress: &mut dyn FnMut(usize, usize),
) -> LiveReport {
    use oryn_core::orchestrator::cost::cost_usd;
    use oryn_core::orchestrator::decompose::decompose;
    use oryn_core::orchestrator::prefix::{CacheStablePrefix, repo_map_from};
    use oryn_core::orchestrator::provider::ExecutionTarget;

    let advisor = format!("{model} @ {endpoint}");
    let (specs, profiles) = discover_specs(adapters, bundle);

    // Pricing lookup by target, so per-attempt cost is real (not estimated).
    let pricing: BTreeMap<ExecutionTarget, _> =
        specs.iter().map(|s| (s.target(), s.pricing)).collect();

    if specs.is_empty() {
        return LiveReport {
            goal: goal.to_string(),
            repo_label: repo.label.clone(),
            base_ref: repo.base_ref(),
            advisor,
            discovered: 0,
            subtasks: 0,
            attempts: vec![],
            gross_usd: 0.0,
            saved_usd: 0.0,
            note: "No models discovered. Install & sign in to a coding CLI (claude, codex, gemini, aider) and select it in Launch — Oryn lists exactly the models each CLI reports."
                .to_string(),
            store_artifacts: { let (n, _) = ArtifactStore::open().stats(); n },
            store_bytes: { let (_, b) = ArtifactStore::open().stats(); b },
            ctx_offered_bytes: 0,
            ctx_stored_bytes: 0,
        };
    }

    let mission = decompose(format!("mission-{}", now_unix()), goal);
    let prefix = CacheStablePrefix::builder()
        .system("You are an expert software engineer working in an isolated git worktree. Make the smallest change that fully satisfies the task and keep the build and tests green.")
        .repo_map(repo_map_from(&repo.files))
        .task(goal)
        .build();

    let engine = build_engine(endpoint, model, &repo.root, &bundle.capability);
    match engine.run_mission_in_worktrees(&mission, &specs, &profiles, &prefix, progress) {
        Ok(artifacts) => {
            let result = &artifacts.result;
            let mut attempts = Vec::new();
            for outcome in &result.outcomes {
                for attempt in &outcome.attempts {
                    let cost = pricing
                        .get(&attempt.target)
                        .map(|p| cost_usd(&attempt.usage, p))
                        .unwrap_or(0.0);
                    let won = outcome.winner.as_ref() == Some(&attempt.target);
                    // Real diff stats for the winning target's worktree.
                    let (files_changed, added, removed) = if won {
                        artifacts
                            .diffs
                            .get(&attempt.target)
                            .map(|d| {
                                let (a, r) = d.line_stats();
                                (d.file_count(), a, r)
                            })
                            .unwrap_or((0, 0, 0))
                    } else {
                        (0, 0, 0)
                    };
                    attempts.push(LiveAttempt {
                        subtask: outcome.subtask.to_string(),
                        framework: attempt.target.framework.to_string(),
                        model: attempt.target.model.to_string(),
                        tier_rank: attempt.tier_rank,
                        input_tokens: attempt.usage.input + attempt.usage.cache_read,
                        output_tokens: attempt.usage.output,
                        cost_usd: cost,
                        passed: attempt.verdict.passed,
                        score: attempt.verdict.score,
                        won,
                        response: if won {
                            outcome.response_text.clone()
                        } else {
                            String::new()
                        },
                        files_changed,
                        added,
                        removed,
                        worktree_session: Engine::session_id(&attempt.target),
                    });
                }
            }
            let passed = attempts.iter().filter(|a| a.won && a.passed).count();
            let note = format!(
                "{} subtask(s) routed across {} discovered target(s) · {}/{} verified by the advisor",
                result.outcomes.len(),
                specs.len(),
                passed,
                result.outcomes.len(),
            );

            // Persist artifacts content-addressed; the cache-stable prefix is the
            // same bytes for every target, so it's offered N times but stored once
            // — real dedup, surfaced in the Broker.
            let store = ArtifactStore::open();
            let mut seen = std::collections::BTreeSet::new();
            let mut offered = 0u64;
            let mut stored = 0u64;
            let mut offer = |bytes: &[u8]| {
                if bytes.is_empty() {
                    return;
                }
                offered += bytes.len() as u64;
                let (id, _) = store.put(bytes);
                if seen.insert(id) {
                    stored += bytes.len() as u64;
                }
            };
            let prefix_bytes = prefix.render().into_bytes();
            for _ in 0..specs.len() {
                offer(&prefix_bytes);
            }
            for outcome in &result.outcomes {
                if let Some(winner) = &outcome.winner {
                    offer(outcome.response_text.as_bytes());
                    if let Some(d) = artifacts.diffs.get(winner) {
                        offer(d.raw().as_bytes());
                    }
                }
            }
            let (store_artifacts, store_bytes) = store.stats();

            LiveReport {
                goal: goal.to_string(),
                repo_label: repo.label.clone(),
                base_ref: repo.base_ref(),
                advisor,
                discovered: specs.len(),
                subtasks: result.outcomes.len(),
                attempts,
                gross_usd: result.spend.gross_usd,
                saved_usd: result.spend.saved_usd,
                note,
                store_artifacts,
                store_bytes,
                ctx_offered_bytes: offered,
                ctx_stored_bytes: stored,
            }
        }
        Err(e) => LiveReport {
            goal: goal.to_string(),
            repo_label: repo.label.clone(),
            base_ref: repo.base_ref(),
            advisor,
            discovered: specs.len(),
            subtasks: 0,
            attempts: vec![],
            gross_usd: 0.0,
            saved_usd: 0.0,
            note: format!("Run could not start: {e}"),
            store_artifacts: 0,
            store_bytes: 0,
            ctx_offered_bytes: 0,
            ctx_stored_bytes: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::Adapter;

    #[test]
    fn run_live_with_no_clis_reports_setup_note() {
        // In a clean environment no coding CLIs are installed, so live discovery
        // finds nothing and the engine never runs — the report must say so
        // honestly rather than fabricate attempts.
        let bundle = CatalogBundle::seed();
        let repo = RepoInfo::detect();
        let report = run_live(
            &Adapter::available(),
            "http://localhost:11434",
            "qwen2.5-coder:7b",
            &bundle,
            &repo,
            "Fix the flaky token-refresh race and add a test",
            &mut |_, _| {},
        );
        assert_eq!(report.discovered, 0);
        assert!(report.attempts.is_empty());
        assert!(report.note.contains("No models discovered"));
        // Repo detection produced a real label + base ref.
        assert!(!report.repo_label.is_empty());
        assert!(report.advisor.contains("qwen2.5-coder:7b"));
    }

    #[test]
    fn git_user_parsed_from_config_and_initials() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git").join("config"),
            "[core]\n\trepositoryformatversion = 0\n[user]\n\tname = Ada Lovelace\n\temail = ada@example.com\n",
        )
        .unwrap();
        let id = UserIdentity::detect(dir.path());
        assert_eq!(id.name, "Ada Lovelace");
        assert_eq!(id.email, "ada@example.com");
        assert_eq!(id.initials, "AL");
    }

    #[test]
    fn identity_falls_back_without_git_config() {
        let dir = tempfile::tempdir().unwrap();
        let id = UserIdentity::detect(dir.path());
        // Falls back to OS user; never empty.
        assert!(!id.name.is_empty());
        assert!(!id.email.is_empty());
        assert!(!id.initials.is_empty());
    }

    #[test]
    fn config_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        let mut r = crate::Root::headless();
        r.settings.accent_idx = 2;
        r.settings.telemetry = true;
        r.advisor.model = "deepseek-r1:7b".into();
        r.catalog_source = CatalogSource::ArtificialAnalysis;
        let cfg = r.to_config();

        assert!(load_config_from(&path).is_none(), "absent file → None");
        save_config_to(&path, &cfg);
        let loaded = load_config_from(&path).expect("config reloads");
        assert_eq!(loaded, cfg);
        // The transient advisor status is never persisted.
        assert!(loaded.advisor.status.is_none());
    }

    #[test]
    fn artifact_store_dedups_identical_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::at(dir.path());
        let (id1, new1) = store.put(b"shared prefix bytes");
        let (id2, new2) = store.put(b"shared prefix bytes");
        assert_eq!(id1, id2, "same content → same id");
        assert!(new1, "first write is new");
        assert!(!new2, "identical content is a dedup hit");
        store.put(b"a different artifact");
        let (count, bytes) = store.stats();
        assert_eq!(count, 2, "two unique artifacts stored");
        assert!(bytes > 0);
    }

    #[test]
    fn is_on_path_finds_a_ubiquitous_binary() {
        // `sh` exists on any unix CI runner; a nonsense name never does.
        #[cfg(unix)]
        assert!(is_on_path("sh"));
        assert!(!is_on_path("oryn-definitely-not-a-real-binary-xyz"));
    }

    #[test]
    fn mark_cli_availability_matches_path_lookup() {
        let mut ads = Adapter::available();
        mark_cli_availability(&mut ads);
        // Each flag reflects the real PATH lookup for that CLI.
        assert!(ads.iter().all(|a| a.installed == is_on_path(a.cli)));
    }

    #[test]
    fn detect_test_command_infers_from_manifest() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            detect_test_command(dir.path()),
            None,
            "empty dir → no runner"
        );
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        assert_eq!(
            detect_test_command(dir.path()),
            Some(vec!["cargo".into(), "test".into(), "--quiet".into()])
        );
        let go = tempfile::tempdir().unwrap();
        std::fs::write(go.path().join("go.mod"), "module x\n").unwrap();
        assert_eq!(detect_test_command(go.path()).unwrap()[0], "go");
    }

    #[test]
    fn repo_detect_finds_this_repository() {
        let repo = RepoInfo::detect();
        // We are inside the oryn git repo when tests run.
        assert!(!repo.label.is_empty());
        assert!(
            repo.files.iter().any(|f| f.ends_with(".rs")),
            "should list rust sources"
        );
    }
}
