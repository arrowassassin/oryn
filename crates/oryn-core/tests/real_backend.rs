//! End-to-end wiring of the real execution backend onto the deterministic core,
//! with every I/O seam faked.
//!
//! ```text
//! resolve_matrix(seed)
//!   → ProviderRegistry of HarnessProvider (build_invocation → ProcessRunner)
//!   → AdvisorVerifier(OllamaAdvisor over Http)
//!   → Orchestrator::run
//! ```
//!
//! Proves that routing a typed sub-task to the cheapest-capable `(framework,
//! model)` target actually constructs that vendor's headless CLI command, "runs"
//! it (faked spawn), normalizes the output, and gates the result through the local
//! advisor — all without real processes or network.

use std::sync::{Arc, Mutex};

use oryn_core::orchestrator::{
    advisor::{AdvisorVerifier, Http, HttpError, OllamaAdvisor},
    capability::resolve_matrix,
    catalog::CapabilityCatalog,
    harness::AuthMode,
    prefix::CacheStablePrefix,
    provider::{
        AgentFramework, ExecutionTarget, ModelId, ModelKind, ModelSpec, Pricing, ProviderRegistry,
    },
    runner::{HarnessProvider, ProcessOutput, ProcessRunner, RunError},
    scheduler::{Orchestrator, Verdict},
    task::{Mission, Subtask, SubtaskId, SubtaskKind},
};

/// A fake spawn: echoes a final message identifying the model, in the right output
/// shape for the framework (plain text for Ollama, Claude-style JSON otherwise).
struct ScriptedRunner {
    programs_run: Mutex<Vec<String>>,
}

impl ProcessRunner for ScriptedRunner {
    fn run(
        &self,
        inv: &oryn_core::orchestrator::harness::HarnessInvocation,
    ) -> Result<ProcessOutput, RunError> {
        self.programs_run.lock().unwrap().push(inv.program.clone());
        let line = if inv.program == "ollama" {
            // `ollama run <model>` → plain text
            format!(
                "{} completed the task",
                inv.args.get(1).cloned().unwrap_or_default()
            )
        } else {
            // Claude-style stream-json result frame
            r#"{"type":"result","result":"patched the refresh race","usage":{"input_tokens":1200,"output_tokens":300,"cache_read_input_tokens":8000}}"#.to_string()
        };
        Ok(ProcessOutput {
            stdout_lines: vec![line],
            exit_code: 0,
        })
    }
}

/// A fake local model that always passes verification.
struct PassHttp;

impl Http for PassHttp {
    fn post_json(&self, _url: &str, _body: &str) -> Result<String, HttpError> {
        Ok(r#"{"choices":[{"message":{"role":"assistant","content":"{\"passed\":true,\"score\":0.9}"}}]}"#.to_string())
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

fn prefix() -> CacheStablePrefix {
    CacheStablePrefix::builder()
        .system("oryn orchestrator")
        .repo_map("src/auth/refreshQueue.ts")
        .task("fix the token-refresh race")
        .build()
}

#[test]
fn deterministic_route_drives_real_harness_command_and_advisor() {
    // Two discovered targets whose ids match the seed catalog so the matrix
    // resolves: a free local model and a priced cloud model.
    let local = spec(AgentFramework::Local, "local-qwen-coder", Pricing::ZERO);
    let opus = spec(
        AgentFramework::ClaudeCode,
        "opus",
        Pricing {
            input: 15.0,
            output: 75.0,
            cache_read: 1.5,
            cache_write: 18.75,
        },
    );
    let available = vec![local.clone(), opus.clone()];

    let catalog = CapabilityCatalog::seed();
    let matrix = resolve_matrix(&available, &catalog.profiles);

    // For MechanicalEdit the local model clears the bar and is free → it leads the
    // tier; the cascade should stop on it.
    let tier = matrix.tier(SubtaskKind::MechanicalEdit);
    assert_eq!(
        tier[0],
        ExecutionTarget::new(AgentFramework::Local, ModelId::new("local-qwen-coder"))
    );

    // Build a registry of *real* HarnessProviders, each backed by the scripted
    // runner (a faked subprocess). Each gets its own worktree path.
    let runner = Arc::new(ScriptedRunner {
        programs_run: Mutex::new(Vec::new()),
    });
    let mut registry = ProviderRegistry::new();
    for s in [&local, &opus] {
        registry.register(Box::new(HarnessProvider::new(
            s.clone(),
            std::path::PathBuf::from(format!("/work/oryn-{}", s.id.as_str())),
            AuthMode::Subscription,
            runner.clone(),
        )));
    }

    // The verifier is the local advisor (faked HTTP), with a deny fallback.
    let verifier = AdvisorVerifier::new(
        OllamaAdvisor::new(
            "http://localhost:11434",
            "qwen2.5-coder",
            Arc::new(PassHttp),
        ),
        Verdict {
            passed: false,
            score: 0.0,
        },
    );

    let mission = Mission {
        id: "m".into(),
        goal: "fix the race".into(),
        subtasks: vec![Subtask {
            id: SubtaskId::new("edit"),
            kind: SubtaskKind::MechanicalEdit,
            summary: "apply the single-flight guard".into(),
            deps: vec![],
        }],
    };

    let result = Orchestrator::run(&mission, &registry, &matrix, &prefix(), &verifier).unwrap();

    // Routed to the cheapest-capable target, one attempt (advisor passed), winner
    // is the local model — and the runner actually launched `ollama`.
    let outcome = &result.outcomes[0];
    assert_eq!(
        outcome.attempts.len(),
        1,
        "advisor passed the cheap tier → no escalation"
    );
    assert_eq!(
        outcome.winner.as_ref().unwrap(),
        &ExecutionTarget::new(AgentFramework::Local, ModelId::new("local-qwen-coder"))
    );
    assert_eq!(outcome.response_text, "local-qwen-coder completed the task");

    let programs = runner.programs_run.lock().unwrap().clone();
    assert_eq!(
        programs,
        vec!["ollama".to_string()],
        "the local harness CLI was invoked"
    );

    // Determinism: same inputs → identical result (fresh runner, same script).
    let runner2 = Arc::new(ScriptedRunner {
        programs_run: Mutex::new(Vec::new()),
    });
    let mut registry2 = ProviderRegistry::new();
    for s in [&local, &opus] {
        registry2.register(Box::new(HarnessProvider::new(
            s.clone(),
            std::path::PathBuf::from(format!("/work/oryn-{}", s.id.as_str())),
            AuthMode::Subscription,
            runner2.clone(),
        )));
    }
    let result2 = Orchestrator::run(&mission, &registry2, &matrix, &prefix(), &verifier).unwrap();
    assert_eq!(result, result2);
}

#[test]
fn advisor_failure_escalates_to_the_next_target() {
    // An advisor that fails the local model but passes the cloud model forces the
    // cascade to escalate from the cheap tier to the next.
    struct GatedHttp;
    impl Http for GatedHttp {
        fn post_json(&self, _url: &str, body: &str) -> Result<String, HttpError> {
            // The advisor prompt embeds the agent's result text; pass only when it
            // came from the cloud model.
            let passed = body.contains("patched the refresh race");
            Ok(format!(
                r#"{{"choices":[{{"message":{{"role":"assistant","content":"{{\"passed\":{passed},\"score\":0.8}}"}}}}]}}"#
            ))
        }
    }

    let local = spec(AgentFramework::Local, "local-qwen-coder", Pricing::ZERO);
    let opus = spec(
        AgentFramework::ClaudeCode,
        "opus",
        Pricing {
            input: 15.0,
            output: 75.0,
            cache_read: 1.5,
            cache_write: 18.75,
        },
    );
    let available = vec![local.clone(), opus.clone()];
    let matrix = resolve_matrix(&available, &CapabilityCatalog::seed().profiles);

    let runner = Arc::new(ScriptedRunner {
        programs_run: Mutex::new(Vec::new()),
    });
    let mut registry = ProviderRegistry::new();
    for s in [&local, &opus] {
        registry.register(Box::new(HarnessProvider::new(
            s.clone(),
            std::path::PathBuf::from("/work"),
            AuthMode::Subscription,
            runner.clone(),
        )));
    }
    let verifier = AdvisorVerifier::new(
        OllamaAdvisor::new(
            "http://localhost:11434",
            "qwen2.5-coder",
            Arc::new(GatedHttp),
        ),
        Verdict {
            passed: false,
            score: 0.0,
        },
    );

    let mission = Mission {
        id: "m".into(),
        goal: "g".into(),
        subtasks: vec![Subtask {
            id: SubtaskId::new("edit"),
            kind: SubtaskKind::MechanicalEdit,
            summary: "apply the guard".into(),
            deps: vec![],
        }],
    };
    let result = Orchestrator::run(&mission, &registry, &matrix, &prefix(), &verifier).unwrap();
    let outcome = &result.outcomes[0];
    assert_eq!(outcome.attempts.len(), 2, "local fails → escalate to cloud");
    assert_eq!(
        outcome.winner.as_ref().unwrap().framework,
        AgentFramework::ClaudeCode
    );
    // Both harness CLIs were launched, cheapest first.
    assert_eq!(
        runner.programs_run.lock().unwrap().clone(),
        vec!["ollama".to_string(), "claude".to_string()]
    );
}
