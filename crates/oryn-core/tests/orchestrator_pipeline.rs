//! Whole-feature integration smoke test for the "route, don't race" pipeline.
//!
//! Exercises the full chain end-to-end through the public API only:
//!
//! ```text
//! discover_targets([fakes])
//!   → resolve_matrix(specs, CapabilityCatalog::seed().profiles)
//!   → Orchestrator::run(3-subtask mission)
//! ```
//!
//! and asserts the topological execution order, escalation on the hard node,
//! the winning `ExecutionTarget`s, and a positive cache saving.

use oryn_core::event::TokenUsage;
use oryn_core::orchestrator::{
    capability::resolve_matrix,
    catalog::CapabilityCatalog,
    discovery::{DiscoveryError, ModelDiscovery, discover_targets},
    prefix::CacheStablePrefix,
    provider::{
        AgentFramework, CompletionRequest, CompletionResponse, ExecutionTarget, ModelId, ModelKind,
        ModelProvider, ModelSpec, Pricing, ProviderError, ProviderRegistry,
    },
    scheduler::{Orchestrator, Verdict, Verifier},
    task::{Mission, Subtask, SubtaskId, SubtaskKind},
};

// ── fakes (public-API only) ──────────────────────────────────────────────────

/// A discovery source returning a fixed set of specs for one framework.
struct FakeDiscovery {
    framework: AgentFramework,
    specs: Vec<ModelSpec>,
}

impl ModelDiscovery for FakeDiscovery {
    fn framework(&self) -> AgentFramework {
        self.framework
    }
    fn discover(&self) -> Result<Vec<ModelSpec>, DiscoveryError> {
        Ok(self.specs.clone())
    }
}

/// A provider that echoes its target id and reports cache-heavy usage.
struct FakeProvider {
    spec: ModelSpec,
    usage: TokenUsage,
}

impl ModelProvider for FakeProvider {
    fn spec(&self) -> &ModelSpec {
        &self.spec
    }
    fn complete(&self, _req: &CompletionRequest) -> Result<CompletionResponse, ProviderError> {
        Ok(CompletionResponse {
            text: self.spec.target().to_string(),
            usage: self.usage,
        })
    }
}

/// Passes only when the completion text is in a fixed accept-set; this lets us
/// force the cheap Debugging candidate to fail and the next to pass.
struct AcceptVerifier {
    passing: Vec<String>,
}

impl Verifier for AcceptVerifier {
    fn verify(
        &self,
        _target: &ExecutionTarget,
        _subtask: &Subtask,
        response: &CompletionResponse,
    ) -> Verdict {
        let passed = self.passing.iter().any(|t| t == &response.text);
        Verdict {
            passed,
            score: if passed { 0.9 } else { 0.3 },
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

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

fn cache_usage() -> TokenUsage {
    // Cache-heavy: most of the prompt is served from cache → positive savings.
    TokenUsage {
        input: 800,
        output: 600,
        cache_read: 9_000,
        cache_write: 1_200,
    }
}

fn subtask(id: &str, kind: SubtaskKind, deps: &[&str]) -> Subtask {
    Subtask {
        id: SubtaskId::new(id),
        kind,
        summary: format!("do {id}"),
        deps: deps.iter().map(|d| SubtaskId::new(*d)).collect(),
    }
}

// ── the smoke test ──────────────────────────────────────────────────────────

#[test]
fn full_pipeline_routes_escalates_and_saves() {
    // Pricing: sonnet cheaper than opus; local is free.
    let sonnet_pricing = Pricing {
        input: 3.0,
        output: 15.0,
        cache_read: 0.30,
        cache_write: 3.75,
    };
    let opus_pricing = Pricing {
        input: 15.0,
        output: 75.0,
        cache_read: 1.50,
        cache_write: 18.75,
    };

    // 1. Discover targets from two framework sources. Model ids match the seed
    //    catalog so resolve_matrix produces real tiers.
    let local_src = FakeDiscovery {
        framework: AgentFramework::Local,
        specs: vec![spec(
            AgentFramework::Local,
            "local-qwen-coder",
            Pricing::ZERO,
        )],
    };
    let claude_src = FakeDiscovery {
        framework: AgentFramework::ClaudeCode,
        specs: vec![
            spec(AgentFramework::ClaudeCode, "opus", opus_pricing),
            spec(AgentFramework::ClaudeCode, "sonnet", sonnet_pricing),
        ],
    };
    let (specs, errors) = discover_targets(&[
        &local_src as &dyn ModelDiscovery,
        &claude_src as &dyn ModelDiscovery,
    ]);
    assert!(errors.is_empty());
    assert_eq!(
        specs.len(),
        3,
        "three distinct (framework, model) targets discovered"
    );

    // 2. Resolve the capability matrix against the bundled seed catalog.
    let catalog = CapabilityCatalog::seed();
    let matrix = resolve_matrix(&specs, &catalog.profiles);

    // Sanity: Debugging excludes the weak local model (seed 0.40 < MIN_CAPABILITY)
    // and orders sonnet (cheaper) before opus.
    let dbg_tier = matrix.tier(SubtaskKind::Debugging);
    assert_eq!(
        dbg_tier.len(),
        2,
        "only the two API models clear the Debugging bar"
    );
    assert_eq!(
        dbg_tier[0].model,
        ModelId::new("sonnet"),
        "cheaper sonnet leads the tier"
    );
    assert_eq!(dbg_tier[1].model, ModelId::new("opus"));

    // 3. Build a registry from the discovered specs.
    let mut registry = ProviderRegistry::new();
    for s in &specs {
        registry.register(Box::new(FakeProvider {
            spec: s.clone(),
            usage: cache_usage(),
        }));
    }

    let prefix = CacheStablePrefix::builder()
        .system("oryn orchestrator")
        .repo_map("crates/oryn-core/src/lib.rs")
        .task("ship the cascade")
        .build();

    // A 3-node chain: a → b → c, with b the hard (Debugging) node.
    let mission = Mission {
        id: "smoke".into(),
        goal: "exercise the pipeline".into(),
        subtasks: vec![
            subtask("a", SubtaskKind::MechanicalEdit, &[]),
            subtask("b", SubtaskKind::Debugging, &["a"]),
            subtask("c", SubtaskKind::TestGen, &["b"]),
        ],
    };

    // Accept the cheap local target (for a and c) and opus (for b after the
    // cheaper sonnet fails) — this forces exactly one escalation, on b.
    let verifier = AcceptVerifier {
        passing: vec!["local/local-qwen-coder".into(), "claude-code/opus".into()],
    };

    let result = Orchestrator::run(&mission, &registry, &matrix, &prefix, &verifier).unwrap();

    // Topological order: a, b, c.
    let ids: Vec<&str> = result.outcomes.iter().map(|o| o.subtask.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);

    // a: cheap local passes immediately (one attempt, no race).
    let a = &result.outcomes[0];
    assert_eq!(a.attempts.len(), 1);
    assert_eq!(
        a.winner.as_ref().unwrap().to_string(),
        "local/local-qwen-coder"
    );

    // b: cheap sonnet fails, escalate to opus (two attempts).
    let b = &result.outcomes[1];
    assert_eq!(b.attempts.len(), 2, "the hard node escalates exactly once");
    assert_eq!(b.attempts[0].target.model, ModelId::new("sonnet"));
    assert!(!b.attempts[0].verdict.passed);
    assert_eq!(b.attempts[1].target.model, ModelId::new("opus"));
    assert!(b.attempts[1].verdict.passed);
    assert_eq!(b.winner.as_ref().unwrap().to_string(), "claude-code/opus");

    // c: cheapest capable is the free local model again.
    let c = &result.outcomes[2];
    assert_eq!(c.attempts.len(), 1);
    assert_eq!(
        c.winner.as_ref().unwrap().to_string(),
        "local/local-qwen-coder"
    );

    // The priced attempts on b carry cache tokens → positive cache savings.
    assert!(
        result.spend.gross_usd > 0.0,
        "priced attempts incurred cost"
    );
    assert!(
        result.spend.saved_usd > 0.0,
        "cache-stable prefix produced real savings"
    );
    assert!((0.0..=1.0).contains(&result.spend.fraction_saved()));

    // Determinism: identical inputs → identical result.
    let mut registry2 = ProviderRegistry::new();
    for s in &specs {
        registry2.register(Box::new(FakeProvider {
            spec: s.clone(),
            usage: cache_usage(),
        }));
    }
    let result2 = Orchestrator::run(&mission, &registry2, &matrix, &prefix, &verifier).unwrap();
    assert_eq!(result, result2);
}
