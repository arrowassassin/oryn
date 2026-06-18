//! Production backend wiring — the real I/O the [`oryn_core`] engine runs on.
//!
//! The core is network/process-free; this module supplies the concrete
//! [`Http`] client ([`UreqHttp`]) and builds a fully-wired [`Engine`] from the
//! user's configuration (the advisor endpoint + model they chose), using the real
//! [`SystemProcessRunner`] to spawn the vendor CLIs.

use std::path::PathBuf;
use std::sync::Arc;

use oryn_core::orchestrator::advisor::{Http, HttpError, LocalAdvisor, OllamaAdvisor};
use oryn_core::orchestrator::catalog::CapabilityCatalog;
use oryn_core::orchestrator::engine::{AdvisorConfig, Engine, EngineConfig};
use oryn_core::orchestrator::harness::AuthMode;
use oryn_core::orchestrator::provider::{AgentFramework, ModelId, ModelKind, ModelSpec, Pricing};
use oryn_core::orchestrator::runner::SystemProcessRunner;
use oryn_core::orchestrator::task::{Subtask, SubtaskId, SubtaskKind};

use crate::launcher::Adapter;

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

/// The worktree base directory, from `ORYN_WORKTREE_BASE` or a sensible default.
fn worktree_base() -> PathBuf {
    std::env::var("ORYN_WORKTREE_BASE").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(".oryn/worktrees"))
}

/// Build a fully-wired engine for the user-chosen advisor `endpoint` + `model`,
/// using the real process runner and HTTP client. Construction does no I/O.
pub fn build_engine(endpoint: &str, model: &str) -> Engine {
    let config = EngineConfig {
        advisor: AdvisorConfig::new(endpoint, model),
        worktree_base: worktree_base(),
        default_auth: AuthMode::Subscription,
    };
    Engine::new(config, Arc::new(SystemProcessRunner), Arc::new(UreqHttp), CapabilityCatalog::seed())
}

/// Map the user's selected adapters into routable [`ModelSpec`]s. Pricing is
/// nominal until a real discovery/pricing source is wired; local models are free.
pub fn specs_from_adapters(adapters: &[Adapter]) -> Vec<ModelSpec> {
    adapters
        .iter()
        .filter(|a| a.enabled)
        .map(|a| {
            let framework = framework_for(a.cli);
            let (kind, pricing) = if framework == AgentFramework::Local {
                (ModelKind::Local { endpoint: "http://localhost:11434".into() }, Pricing::ZERO)
            } else {
                (
                    ModelKind::Api { provider: a.cli.into() },
                    // Nominal per-million pricing; replaced by the pinned catalog/
                    // discovery once real price data is sourced.
                    Pricing { input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 },
                )
            };
            ModelSpec { id: ModelId::new(a.tag), kind, pricing, context_window: 200_000, framework }
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

/// A real, one-shot readiness check the UI can trigger: constructs the engine
/// (no I/O), counts configured targets, and makes a **real** advisor round-trip to
/// the chosen endpoint+model. Returns a human-readable status line.
pub fn check_setup(adapters: &[Adapter], endpoint: &str, model: &str) -> String {
    let specs = specs_from_adapters(adapters);
    let engine = build_engine(endpoint, model);
    let advisor_status = probe_advisor(endpoint, model);
    format!(
        "{} target(s) · worktrees {} · {}",
        specs.len(),
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
