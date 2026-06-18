//! Production orchestration glue — wires the whole "route, don't race" runtime
//! together behind **injected** I/O so the core stays network- and process-free.
//!
//! [`Engine`] is the single entrypoint an application calls: given the user's
//! configuration ([`EngineConfig`] — including the **user-chosen advisor endpoint
//! and model**), a [`ProcessRunner`] (real subprocess spawner) and an [`Http`]
//! client (real HTTP to the local model), it
//!
//! 1. resolves the deterministic capability matrix from the available targets +
//!    the pinned catalog,
//! 2. builds a [`ProviderRegistry`] of [`HarnessProvider`]s (one isolated worktree
//!    per `(framework, model)` target),
//! 3. builds the [`AdvisorVerifier`] against the configured local model, and
//! 4. runs the mission through the deterministic [`Orchestrator`].
//!
//! The concrete `ProcessRunner`/`Http` impls live in the app layer; everything
//! here is pure wiring, fully testable with fakes.

use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

use crate::orchestrator::advisor::{AdvisorVerifier, Http, OllamaAdvisor};
use crate::orchestrator::capability::resolve_matrix;
use crate::orchestrator::catalog::CapabilityCatalog;
use crate::orchestrator::harness::AuthMode;
use crate::orchestrator::prefix::CacheStablePrefix;
use crate::orchestrator::provider::{ExecutionTarget, ModelSpec, ProviderRegistry};
use crate::orchestrator::runner::{HarnessProvider, ProcessRunner};
use crate::orchestrator::scheduler::{MissionResult, Orchestrator, OrchestratorError, Verdict};
use crate::orchestrator::task::Mission;

/// Configuration for the local advisor connection — the part the user chooses.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvisorConfig {
    /// Base URL of an OpenAI-compatible endpoint (Ollama, llamafile, llama.cpp).
    /// The user picks this; default `http://localhost:11434`.
    pub endpoint: String,
    /// The local model name the user selects (e.g. `qwen2.5-coder:7b`).
    pub model: String,
    /// Verdict used when the advisor is unreachable or malformed, so a missing
    /// local model degrades gracefully instead of aborting the mission.
    pub fallback: Verdict,
}

impl AdvisorConfig {
    /// A config for `endpoint` + `model` with a deny-by-default fallback (an
    /// unreachable advisor never auto-passes a sub-task).
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            model: model.into(),
            fallback: Verdict {
                passed: false,
                score: 0.0,
            },
        }
    }
}

impl Default for AdvisorConfig {
    fn default() -> Self {
        Self::new("http://localhost:11434", "qwen2.5-coder")
    }
}

/// Top-level engine configuration.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Local advisor connection (user-chosen endpoint + model).
    pub advisor: AdvisorConfig,
    /// Root directory under which each target's isolated worktree is created.
    pub worktree_base: PathBuf,
    /// Default auth mode for harness CLIs (subscription login by default).
    pub default_auth: AuthMode,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            advisor: AdvisorConfig::default(),
            worktree_base: PathBuf::from(".oryn/worktrees"),
            default_auth: AuthMode::Subscription,
        }
    }
}

/// Errors from an engine run.
#[derive(Debug, Error)]
pub enum EngineError {
    /// The deterministic orchestrator failed (cycle, or no candidates).
    #[error(transparent)]
    Orchestrator(#[from] OrchestratorError),
}

/// The production runtime: configuration plus injected I/O.
pub struct Engine {
    config: EngineConfig,
    runner: Arc<dyn ProcessRunner>,
    http: Arc<dyn Http>,
    catalog: CapabilityCatalog,
}

impl Engine {
    /// Construct an engine from `config`, a process `runner`, an `http` client, and
    /// the pinned capability `catalog`.
    pub fn new(
        config: EngineConfig,
        runner: Arc<dyn ProcessRunner>,
        http: Arc<dyn Http>,
        catalog: CapabilityCatalog,
    ) -> Self {
        Self {
            config,
            runner,
            http,
            catalog,
        }
    }

    /// The configured worktree base directory.
    pub fn worktree_base(&self) -> &std::path::Path {
        &self.config.worktree_base
    }

    /// The configured advisor endpoint.
    pub fn advisor_endpoint(&self) -> &str {
        &self.config.advisor.endpoint
    }

    /// The configured advisor model.
    pub fn advisor_model(&self) -> &str {
        &self.config.advisor.model
    }

    /// Filesystem-safe worktree directory for `target`, under the configured base.
    pub fn worktree_for(&self, target: &ExecutionTarget) -> PathBuf {
        let model = target.model.as_str().replace(['/', ':', ' ', '\\'], "-");
        self.config
            .worktree_base
            .join(format!("oryn-{}-{}", target.framework, model))
    }

    /// Build a provider registry: one [`HarnessProvider`] per available spec, each
    /// in its own worktree, sharing the injected runner.
    pub fn build_registry(&self, specs: &[ModelSpec]) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        for spec in specs {
            let workdir = self.worktree_for(&spec.target());
            registry.register(Box::new(HarnessProvider::new(
                spec.clone(),
                workdir,
                self.config.default_auth.clone(),
                self.runner.clone(),
            )));
        }
        registry
    }

    /// The verifier bound to the user-configured local advisor.
    fn verifier(&self) -> AdvisorVerifier<OllamaAdvisor> {
        let advisor = OllamaAdvisor::new(
            self.config.advisor.endpoint.clone(),
            self.config.advisor.model.clone(),
            self.http.clone(),
        );
        AdvisorVerifier::new(advisor, self.config.advisor.fallback)
    }

    /// Run `mission` against the `available` targets over the cache-stable `prefix`.
    ///
    /// Deterministically routes each typed sub-task to the cheapest-capable target,
    /// executes its harness CLI, and gates the result through the local advisor.
    ///
    /// # Errors
    ///
    /// [`EngineError::Orchestrator`] on a dependency cycle or a sub-task with no
    /// attemptable target.
    pub fn run_mission(
        &self,
        mission: &Mission,
        available: &[ModelSpec],
        prefix: &CacheStablePrefix,
    ) -> Result<MissionResult, EngineError> {
        self.run_mission_with(mission, available, &self.catalog.profiles, prefix)
    }

    /// Like [`run_mission`](Self::run_mission) but with an explicit capability
    /// `profiles` map — used when models are **discovered dynamically** and keyed
    /// by ids that differ from the bundled catalog (see
    /// [`listing::build_targets`](crate::orchestrator::listing::build_targets)).
    ///
    /// # Errors
    ///
    /// [`EngineError::Orchestrator`] on a dependency cycle or a sub-task with no
    /// attemptable target.
    pub fn run_mission_with(
        &self,
        mission: &Mission,
        available: &[ModelSpec],
        profiles: &std::collections::BTreeMap<
            crate::orchestrator::provider::ModelId,
            crate::orchestrator::capability::CapabilityProfile,
        >,
        prefix: &CacheStablePrefix,
    ) -> Result<MissionResult, EngineError> {
        let matrix = resolve_matrix(available, profiles);
        let registry = self.build_registry(available);
        let verifier = self.verifier();
        Ok(Orchestrator::run(
            mission, &registry, &matrix, prefix, &verifier,
        )?)
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::advisor::HttpError;
    use crate::orchestrator::harness::HarnessInvocation;
    use crate::orchestrator::provider::{AgentFramework, ModelId, ModelKind, Pricing};
    use crate::orchestrator::runner::{ProcessOutput, RunError};
    use crate::orchestrator::task::{Subtask, SubtaskId, SubtaskKind};
    use std::sync::Mutex;

    /// Records the worktrees it was launched in; replays canned stdout per program.
    struct FakeRunner {
        cwds: Mutex<Vec<PathBuf>>,
    }
    impl ProcessRunner for FakeRunner {
        fn run(&self, inv: &HarnessInvocation) -> Result<ProcessOutput, RunError> {
            self.cwds.lock().unwrap().push(inv.cwd.clone());
            let line = if inv.program == "ollama" {
                "local result".to_string()
            } else {
                r#"{"type":"result","result":"cloud result","usage":{"input_tokens":10,"output_tokens":5}}"#.to_string()
            };
            Ok(ProcessOutput {
                stdout_lines: vec![line],
                exit_code: 0,
            })
        }
    }

    struct PassHttp;
    impl Http for PassHttp {
        fn post_json(&self, _url: &str, _body: &str) -> Result<String, HttpError> {
            Ok(r#"{"choices":[{"message":{"content":"{\"passed\":true,\"score\":0.9}"}}]}"#.into())
        }
    }

    struct DownHttp;
    impl Http for DownHttp {
        fn post_json(&self, _url: &str, _body: &str) -> Result<String, HttpError> {
            Err(HttpError::Unreachable)
        }
    }

    fn spec(framework: AgentFramework, id: &str, pricing: Pricing) -> ModelSpec {
        let kind = if pricing == Pricing::ZERO {
            ModelKind::Local {
                endpoint: "http://localhost:11434".into(),
            }
        } else {
            ModelKind::Api {
                provider: "anthropic".into(),
            }
        };
        ModelSpec {
            id: ModelId::new(id),
            kind,
            pricing,
            context_window: 200_000,
            framework,
        }
    }

    fn specs() -> Vec<ModelSpec> {
        vec![
            spec(AgentFramework::Local, "local-qwen-coder", Pricing::ZERO),
            spec(
                AgentFramework::ClaudeCode,
                "opus",
                Pricing {
                    input: 15.0,
                    output: 75.0,
                    cache_read: 1.5,
                    cache_write: 18.75,
                },
            ),
        ]
    }

    fn prefix() -> CacheStablePrefix {
        CacheStablePrefix::builder()
            .system("sys")
            .repo_map("map")
            .task("task")
            .build()
    }

    fn mission() -> Mission {
        Mission {
            id: "m".into(),
            goal: "g".into(),
            subtasks: vec![Subtask {
                id: SubtaskId::new("e"),
                kind: SubtaskKind::MechanicalEdit,
                summary: "edit".into(),
                deps: vec![],
            }],
        }
    }

    fn engine(http: Arc<dyn Http>) -> Engine {
        Engine::new(
            EngineConfig {
                advisor: AdvisorConfig::new("http://localhost:11434", "qwen2.5-coder"),
                worktree_base: PathBuf::from("/wt"),
                default_auth: AuthMode::Subscription,
            },
            Arc::new(FakeRunner {
                cwds: Mutex::new(Vec::new()),
            }),
            http,
            CapabilityCatalog::seed(),
        )
    }

    #[test]
    fn worktree_for_sanitizes_model_name() {
        let e = engine(Arc::new(PassHttp));
        let wt = e.worktree_for(&ExecutionTarget::new(
            AgentFramework::ClaudeCode,
            ModelId::new("opus:4.6"),
        ));
        assert_eq!(wt, PathBuf::from("/wt/oryn-claude-code-opus-4.6"));
    }

    #[test]
    fn build_registry_makes_one_provider_per_spec() {
        let e = engine(Arc::new(PassHttp));
        let reg = e.build_registry(&specs());
        assert_eq!(reg.specs().len(), 2);
        // each resolvable by target
        assert!(
            reg.get(&ExecutionTarget::new(
                AgentFramework::Local,
                ModelId::new("local-qwen-coder")
            ))
            .is_some()
        );
    }

    #[test]
    fn run_mission_routes_to_cheapest_and_passes() {
        let e = engine(Arc::new(PassHttp));
        let result = e.run_mission(&mission(), &specs(), &prefix()).unwrap();
        let outcome = &result.outcomes[0];
        assert_eq!(outcome.attempts.len(), 1, "advisor passed the cheap tier");
        assert_eq!(
            outcome.winner.as_ref().unwrap(),
            &ExecutionTarget::new(AgentFramework::Local, ModelId::new("local-qwen-coder"))
        );
        assert_eq!(outcome.response_text, "local result");
    }

    #[test]
    fn run_mission_is_deterministic() {
        let a = engine(Arc::new(PassHttp))
            .run_mission(&mission(), &specs(), &prefix())
            .unwrap();
        let b = engine(Arc::new(PassHttp))
            .run_mission(&mission(), &specs(), &prefix())
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn unreachable_advisor_degrades_to_fallback_not_panic() {
        // With a down advisor every verify returns the deny fallback → no tier
        // passes, but the run still completes with a best-effort winner.
        let e = engine(Arc::new(DownHttp));
        let result = e.run_mission(&mission(), &specs(), &prefix()).unwrap();
        let outcome = &result.outcomes[0];
        assert!(outcome.attempts.iter().all(|a| !a.verdict.passed));
        assert!(
            outcome.winner.is_some(),
            "best-effort winner chosen even when all fail"
        );
    }

    #[test]
    fn worktrees_are_under_the_configured_base() {
        let runner = Arc::new(FakeRunner {
            cwds: Mutex::new(Vec::new()),
        });
        let e = Engine::new(
            EngineConfig {
                advisor: AdvisorConfig::default(),
                worktree_base: PathBuf::from("/custom/base"),
                default_auth: AuthMode::Subscription,
            },
            runner.clone(),
            Arc::new(PassHttp),
            CapabilityCatalog::seed(),
        );
        e.run_mission(&mission(), &specs(), &prefix()).unwrap();
        let cwds = runner.cwds.lock().unwrap();
        assert!(!cwds.is_empty());
        assert!(cwds.iter().all(|p| p.starts_with("/custom/base")));
    }

    #[test]
    fn config_getters_expose_user_choices() {
        let e = Engine::new(
            EngineConfig {
                advisor: AdvisorConfig::new("http://host:9999", "my-model"),
                worktree_base: PathBuf::from("/wt"),
                default_auth: AuthMode::Subscription,
            },
            Arc::new(FakeRunner {
                cwds: Mutex::new(Vec::new()),
            }),
            Arc::new(PassHttp),
            CapabilityCatalog::seed(),
        );
        assert_eq!(e.advisor_endpoint(), "http://host:9999");
        assert_eq!(e.advisor_model(), "my-model");
        assert_eq!(e.worktree_base(), std::path::Path::new("/wt"));
    }

    #[test]
    fn advisor_config_defaults_are_sane() {
        let c = AdvisorConfig::default();
        assert_eq!(c.endpoint, "http://localhost:11434");
        assert_eq!(c.model, "qwen2.5-coder");
        assert!(!c.fallback.passed, "unreachable advisor must not auto-pass");
    }
}
