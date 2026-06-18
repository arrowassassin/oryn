//! The cascade scheduler — the heart of "route, don't race".
//!
//! [`Orchestrator::run`] walks a [`Mission`] in deterministic topological order
//! and, for each [`Subtask`], climbs the [`CapabilityMatrix`] tier for that
//! subtask's [`SubtaskKind`] **cheapest-capable target first**. Each candidate is
//! a discovered `(framework, model)` [`ExecutionTarget`]; the scheduler resolves
//! it to a [`ModelProvider`] in the [`ProviderRegistry`], runs a completion
//! against the byte-stable [`CacheStablePrefix`], and asks the [`Verifier`] whether
//! the result is acceptable.
//!
//! The cascade **stops at the first passing attempt** — Oryn never races. Only
//! when a cheaper target fails verification does it escalate to the next, pricier
//! tier entry. If every candidate fails, the scheduler still returns a best-effort
//! winner chosen by a **total, deterministic** tie-break so callers always get a
//! reproducible result.
//!
//! # Determinism
//!
//! - Subtasks run in [`Mission::topo_order`] (lexicographic tie-break).
//! - Tier candidates are tried in matrix order; `tier_rank` is the tier index.
//! - Every completion uses `temperature = 0.0` and a [`FIXED_SEED`].
//! - The all-fail winner tie-break is total: `(score desc, tier_rank asc,
//!   ExecutionTarget asc)`, with floats compared via [`f64::total_cmp`].
//!
//! Targets without a registered provider (or whose provider errors) are skipped,
//! never panicked over: a missing credential for one framework must not sink the
//! mission. A subtask with no attemptable candidate at all yields
//! [`OrchestratorError::NoCandidates`].

use std::collections::BTreeMap;

use thiserror::Error;

use crate::event::TokenUsage;
use crate::orchestrator::{
    capability::CapabilityMatrix,
    cost::Spend,
    prefix::CacheStablePrefix,
    provider::{CompletionResponse, ExecutionTarget, ProviderRegistry},
    task::{CycleError, Mission, Subtask, SubtaskId},
};

/// Fixed sampling seed for every completion, so deterministic providers return
/// byte-identical output across runs. The value spells `"ORYN"` in ASCII.
pub const FIXED_SEED: u64 = 0x4F52_594E;

// ── Verifier ────────────────────────────────────────────────────────────────────

/// The outcome of verifying a completion against its subtask.
///
/// `Eq` is not derived because `score` is an `f64`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    /// Whether the completion is acceptable and the cascade may stop.
    pub passed: bool,
    /// A quality score in `0.0..=1.0`, used to pick a best-effort winner when no
    /// attempt passes.
    pub score: f64,
}

/// Decides whether a model's completion satisfies a subtask.
///
/// Object-safe so the orchestrator can take `&dyn Verifier`. Real implementations
/// verify *by execution* (run the tests, apply the diff, type-check); they live in
/// a later segment. The contract here is pure and synchronous.
pub trait Verifier {
    /// Judge `response` against `subtask`.
    fn verify(&self, subtask: &Subtask, response: &CompletionResponse) -> Verdict;
}

// ── Attempt / outcomes ────────────────────────────────────────────────────────

/// One execution of a subtask against a single [`ExecutionTarget`].
#[derive(Debug, Clone, PartialEq)]
pub struct Attempt {
    /// The `(framework, model)` target this attempt ran against.
    pub target: ExecutionTarget,
    /// The target's index within its capability tier (0 = cheapest).
    pub tier_rank: usize,
    /// Token usage the provider reported for this completion.
    pub usage: TokenUsage,
    /// The verifier's judgement of this attempt.
    pub verdict: Verdict,
}

/// The full record of how one subtask was resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtaskOutcome {
    /// The subtask this outcome belongs to.
    pub subtask: SubtaskId,
    /// Every attempt made, cheapest-first, in cascade order.
    pub attempts: Vec<Attempt>,
    /// The winning target: the first attempt to pass, or the best-effort pick
    /// when none passed. `None` is never produced — a subtask with no attempt is
    /// reported as [`OrchestratorError::NoCandidates`] instead.
    pub winner: Option<ExecutionTarget>,
    /// The winning attempt's response text.
    pub response_text: String,
}

/// The result of running a whole [`Mission`].
#[derive(Debug, Clone, PartialEq)]
pub struct MissionResult {
    /// One outcome per subtask, in topological execution order.
    pub outcomes: Vec<SubtaskOutcome>,
    /// Accumulated spend (gross cost + cache savings) across every attempt.
    pub spend: Spend,
}

// ── OrchestratorError ────────────────────────────────────────────────────────

/// Errors that abort a mission run.
#[derive(Debug, Error)]
pub enum OrchestratorError {
    /// The mission's dependency graph contains a cycle.
    #[error(transparent)]
    Cycle(#[from] CycleError),
    /// A subtask had no attemptable execution target — its tier was empty, or
    /// every tier target was missing a provider or failed to respond.
    #[error("no capable execution target for subtask {0}")]
    NoCandidates(SubtaskId),
}

// ── Orchestrator ────────────────────────────────────────────────────────────────

/// The stateless cascade scheduler.
pub struct Orchestrator;

impl Orchestrator {
    /// Run `mission` to completion.
    ///
    /// Subtasks are executed in [`Mission::topo_order`]. For each, the capability
    /// `matrix` tier is walked cheapest-first; the first target with a working
    /// provider whose completion passes verification wins and stops the cascade.
    /// Spend accumulates across every attempt that actually ran.
    ///
    /// # Errors
    ///
    /// - [`OrchestratorError::Cycle`] if the mission has a dependency cycle.
    /// - [`OrchestratorError::NoCandidates`] if a subtask cannot be attempted by
    ///   any target.
    pub fn run(
        mission: &Mission,
        registry: &ProviderRegistry,
        matrix: &CapabilityMatrix,
        prefix: &CacheStablePrefix,
        verifier: &dyn Verifier,
    ) -> Result<MissionResult, OrchestratorError> {
        let order = mission.topo_order()?;

        // Index subtasks by id for O(log n) lookup during the walk.
        let by_id: BTreeMap<&SubtaskId, &Subtask> =
            mission.subtasks.iter().map(|s| (&s.id, s)).collect();

        let rendered_prefix = prefix.render();
        let mut spend = Spend::ZERO;
        let mut outcomes = Vec::with_capacity(order.len());

        for id in &order {
            let subtask = by_id.get(id).expect("topo_order id is a mission subtask");
            let outcome = Self::run_subtask(
                subtask,
                registry,
                matrix,
                &rendered_prefix,
                verifier,
                &mut spend,
            )?;
            outcomes.push(outcome);
        }

        Ok(MissionResult { outcomes, spend })
    }

    /// Resolve a single subtask by climbing its capability tier.
    fn run_subtask(
        subtask: &Subtask,
        registry: &ProviderRegistry,
        matrix: &CapabilityMatrix,
        rendered_prefix: &str,
        verifier: &dyn Verifier,
        spend: &mut Spend,
    ) -> Result<SubtaskOutcome, OrchestratorError> {
        use crate::orchestrator::provider::CompletionRequest;

        let mut attempts: Vec<Attempt> = Vec::new();
        // Response text kept in lockstep with `attempts` so we can recover the
        // winning attempt's text without storing it on every `Attempt`.
        let mut texts: Vec<String> = Vec::new();
        let mut winner_idx: Option<usize> = None;

        for (tier_rank, target) in matrix.tier(subtask.kind).iter().enumerate() {
            // A target with no registered provider is skipped, not fatal.
            let Some(provider) = registry.get(target) else {
                continue;
            };

            let req = CompletionRequest {
                prefix: rendered_prefix.to_string(),
                suffix: subtask.summary.clone(),
                temperature: 0.0,
                seed: Some(FIXED_SEED),
            };

            // A provider that errors is also skipped — try the next tier entry.
            let Ok(response) = provider.complete(&req) else {
                continue;
            };

            spend.add(&response.usage, &provider.spec().pricing);
            let verdict = verifier.verify(subtask, &response);

            attempts.push(Attempt {
                target: target.clone(),
                tier_rank,
                usage: response.usage,
                verdict,
            });
            texts.push(response.text);

            if verdict.passed {
                winner_idx = Some(attempts.len() - 1);
                break;
            }
        }

        if attempts.is_empty() {
            return Err(OrchestratorError::NoCandidates(subtask.id.clone()));
        }

        // No attempt passed: pick the best by a total, deterministic tie-break of
        // (score desc, tier_rank asc, ExecutionTarget asc).
        let chosen = winner_idx.unwrap_or_else(|| best_attempt(&attempts));

        Ok(SubtaskOutcome {
            subtask: subtask.id.clone(),
            winner: Some(attempts[chosen].target.clone()),
            response_text: texts[chosen].clone(),
            attempts,
        })
    }
}

/// Index of the best attempt by `(score desc, tier_rank asc, ExecutionTarget asc)`.
///
/// `attempts` must be non-empty.
fn best_attempt(attempts: &[Attempt]) -> usize {
    (0..attempts.len())
        .max_by(|&a, &b| {
            let x = &attempts[a];
            let y = &attempts[b];
            // Higher score wins.
            x.verdict
                .score
                .total_cmp(&y.verdict.score)
                // Lower tier_rank wins → reverse so it ranks "greater" for max_by.
                .then(y.tier_rank.cmp(&x.tier_rank))
                // Lower ExecutionTarget wins → reverse likewise.
                .then(y.target.cmp(&x.target))
        })
        .expect("best_attempt called on non-empty attempts")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::provider::{
        AgentFramework, CompletionRequest, ModelId, ModelKind, ModelProvider, ModelSpec, Pricing,
        ProviderError,
    };
    use crate::orchestrator::task::SubtaskKind;

    // ── fakes ───────────────────────────────────────────────────────────────────

    /// A provider that returns fixed text + usage, or always errors.
    struct FakeProvider {
        spec: ModelSpec,
        text: String,
        usage: TokenUsage,
        errors: bool,
    }

    impl FakeProvider {
        fn new(framework: AgentFramework, id: &str, pricing: Pricing, usage: TokenUsage) -> Self {
            Self {
                spec: ModelSpec {
                    id: ModelId::new(id),
                    kind: ModelKind::Api { provider: "test".into() },
                    pricing,
                    context_window: 128_000,
                    framework,
                },
                // Text identifies the target so the verifier can decide per-target.
                text: format!("{framework}/{id}"),
                usage,
                errors: false,
            }
        }

        fn erroring(framework: AgentFramework, id: &str) -> Self {
            let mut p = Self::new(framework, id, Pricing::ZERO, TokenUsage::default());
            p.errors = true;
            p
        }
    }

    impl ModelProvider for FakeProvider {
        fn spec(&self) -> &ModelSpec {
            &self.spec
        }
        fn complete(&self, _req: &CompletionRequest) -> Result<CompletionResponse, ProviderError> {
            if self.errors {
                Err(ProviderError::Unavailable)
            } else {
                Ok(CompletionResponse { text: self.text.clone(), usage: self.usage })
            }
        }
    }

    /// Maps a response text to `(passed, score)`; absent text → `(false, 0.0)`.
    struct FakeVerifier {
        verdicts: BTreeMap<String, (bool, f64)>,
    }

    impl FakeVerifier {
        fn new(entries: &[(&str, bool, f64)]) -> Self {
            let verdicts = entries
                .iter()
                .map(|&(t, p, s)| (t.to_string(), (p, s)))
                .collect();
            Self { verdicts }
        }
    }

    impl Verifier for FakeVerifier {
        fn verify(&self, _subtask: &Subtask, response: &CompletionResponse) -> Verdict {
            let (passed, score) =
                self.verdicts.get(&response.text).copied().unwrap_or((false, 0.0));
            Verdict { passed, score }
        }
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn target(framework: AgentFramework, id: &str) -> ExecutionTarget {
        ExecutionTarget::new(framework, ModelId::new(id))
    }

    fn usage_with_cache() -> TokenUsage {
        TokenUsage { input: 1_000, output: 500, cache_read: 4_000, cache_write: 200 }
    }

    fn anthropic() -> Pricing {
        Pricing { input: 3.0, output: 15.0, cache_read: 0.30, cache_write: 3.75 }
    }

    fn prefix() -> CacheStablePrefix {
        CacheStablePrefix::builder()
            .system("you are oryn")
            .repo_map("src/lib.rs")
            .task("do the thing")
            .build()
    }

    fn subtask(id: &str, kind: SubtaskKind) -> Subtask {
        Subtask { id: SubtaskId::new(id), kind, summary: format!("work {id}"), deps: vec![] }
    }

    fn mission(subtasks: Vec<Subtask>) -> Mission {
        Mission { id: "m".into(), goal: "g".into(), subtasks }
    }

    // ── happy path: cheapest passes ───────────────────────────────────────────

    #[test]
    fn cheapest_tier_passes_in_one_attempt() {
        let cheap = target(AgentFramework::Local, "qwen");
        let pricey = target(AgentFramework::ClaudeCode, "opus");

        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(FakeProvider::new(
            AgentFramework::Local,
            "qwen",
            Pricing::ZERO,
            usage_with_cache(),
        )));
        reg.register(Box::new(FakeProvider::new(
            AgentFramework::ClaudeCode,
            "opus",
            anthropic(),
            usage_with_cache(),
        )));

        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::Debugging, vec![cheap.clone(), pricey]);
        // The cheap target's text passes.
        let verifier = FakeVerifier::new(&[("local/qwen", true, 0.7)]);

        let result =
            Orchestrator::run(&mission(vec![subtask("s", SubtaskKind::Debugging)]), &reg, &matrix, &prefix(), &verifier)
                .unwrap();

        assert_eq!(result.outcomes.len(), 1);
        let o = &result.outcomes[0];
        assert_eq!(o.attempts.len(), 1, "must stop at first pass — no racing");
        assert_eq!(o.winner.as_ref(), Some(&cheap));
        assert_eq!(o.attempts[0].tier_rank, 0);
        assert!(o.attempts[0].verdict.passed);
        assert_eq!(o.response_text, "local/qwen");
    }

    // ── escalation ────────────────────────────────────────────────────────────

    #[test]
    fn cheap_fails_then_escalates_to_next_tier() {
        let cheap = target(AgentFramework::Local, "qwen");
        let pricey = target(AgentFramework::ClaudeCode, "opus");

        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(FakeProvider::new(
            AgentFramework::Local,
            "qwen",
            Pricing::ZERO,
            usage_with_cache(),
        )));
        reg.register(Box::new(FakeProvider::new(
            AgentFramework::ClaudeCode,
            "opus",
            anthropic(),
            usage_with_cache(),
        )));

        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::Debugging, vec![cheap.clone(), pricey.clone()]);
        // Cheap fails, pricey passes → escalation.
        let verifier =
            FakeVerifier::new(&[("local/qwen", false, 0.3), ("claude-code/opus", true, 0.9)]);

        let result =
            Orchestrator::run(&mission(vec![subtask("s", SubtaskKind::Debugging)]), &reg, &matrix, &prefix(), &verifier)
                .unwrap();

        let o = &result.outcomes[0];
        assert_eq!(o.attempts.len(), 2, "escalation must be recorded");
        assert_eq!(o.attempts[0].target, cheap);
        assert!(!o.attempts[0].verdict.passed);
        assert_eq!(o.attempts[1].target, pricey);
        assert!(o.attempts[1].verdict.passed);
        assert_eq!(o.winner.as_ref(), Some(&pricey));
        assert_eq!(o.response_text, "claude-code/opus");
    }

    // ── all fail → deterministic tie-break ────────────────────────────────────

    #[test]
    fn all_fail_picks_best_by_tiebreak() {
        // Three targets, all fail. Tie-break = (score desc, tier_rank asc, target asc).
        // a: score 0.2 rank 0; b: score 0.5 rank 1; c: score 0.5 rank 2.
        // b and c tie on score → lower tier_rank wins → b.
        let a = target(AgentFramework::Local, "a");
        let b = target(AgentFramework::Local, "b");
        let c = target(AgentFramework::Local, "c");

        let mut reg = ProviderRegistry::new();
        for id in ["a", "b", "c"] {
            reg.register(Box::new(FakeProvider::new(
                AgentFramework::Local,
                id,
                Pricing::ZERO,
                usage_with_cache(),
            )));
        }

        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::Refactor, vec![a, b.clone(), c]);
        let verifier = FakeVerifier::new(&[
            ("local/a", false, 0.2),
            ("local/b", false, 0.5),
            ("local/c", false, 0.5),
        ]);

        let result =
            Orchestrator::run(&mission(vec![subtask("s", SubtaskKind::Refactor)]), &reg, &matrix, &prefix(), &verifier)
                .unwrap();

        let o = &result.outcomes[0];
        assert_eq!(o.attempts.len(), 3, "all candidates tried when none pass");
        assert_eq!(o.winner.as_ref(), Some(&b), "tie-break must select local/b");
        assert_eq!(o.response_text, "local/b");
    }

    #[test]
    fn all_fail_tiebreak_uses_target_when_score_and_rank_tie() {
        // Same score, but we craft equal scores across two targets where the
        // distinguishing factor is the ExecutionTarget ordering. Since tier_rank
        // differs by index, force it via separate kinds is overkill — instead test
        // the target tie-break directly through best_attempt.
        let lo = Attempt {
            target: target(AgentFramework::ClaudeCode, "a"),
            tier_rank: 0,
            usage: TokenUsage::default(),
            verdict: Verdict { passed: false, score: 0.5 },
        };
        let hi = Attempt {
            target: target(AgentFramework::Codex, "a"),
            tier_rank: 0,
            usage: TokenUsage::default(),
            verdict: Verdict { passed: false, score: 0.5 },
        };
        // ClaudeCode < Codex, so the ClaudeCode attempt (index 0) must win.
        assert_eq!(best_attempt(&[lo, hi]), 0);
    }

    // ── spend accumulates ──────────────────────────────────────────────────────

    #[test]
    fn spend_accumulates_across_attempts_and_subtasks() {
        let cheap = target(AgentFramework::Local, "qwen");
        let pricey = target(AgentFramework::ClaudeCode, "opus");

        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(FakeProvider::new(
            AgentFramework::Local,
            "qwen",
            Pricing::ZERO,
            usage_with_cache(),
        )));
        reg.register(Box::new(FakeProvider::new(
            AgentFramework::ClaudeCode,
            "opus",
            anthropic(),
            usage_with_cache(),
        )));

        // Two subtasks; the first escalates (two attempts), the second passes cheap.
        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::Debugging, vec![cheap.clone(), pricey])
            .with(SubtaskKind::TestGen, vec![cheap]);
        // For the Debugging mission, local fails and opus passes (escalation).
        let verifier = FakeVerifier::new(&[
            ("local/qwen", false, 0.3),
            ("claude-code/opus", true, 0.9),
        ]);
        // For the TestGen mission, the cheap local target passes outright.
        let verifier_pass = FakeVerifier::new(&[("local/qwen", true, 0.8)]);

        // Debugging mission (escalates): both providers charged.
        let r1 = Orchestrator::run(
            &mission(vec![subtask("s", SubtaskKind::Debugging)]),
            &reg,
            &matrix,
            &prefix(),
            &verifier,
        )
        .unwrap();
        // Two attempts ran: local (free) + opus (priced) → positive gross + savings.
        assert!(r1.spend.gross_usd > 0.0);
        assert!(r1.spend.saved_usd > 0.0, "cache_write/read imply positive savings");

        // TestGen mission (cheap passes): only the free local provider charged.
        let r2 = Orchestrator::run(
            &mission(vec![subtask("t", SubtaskKind::TestGen)]),
            &reg,
            &matrix,
            &prefix(),
            &verifier_pass,
        )
        .unwrap();
        assert_eq!(r2.spend.gross_usd, 0.0, "local-only attempt is free");
    }

    // ── determinism ──────────────────────────────────────────────────────────

    #[test]
    fn same_inputs_produce_identical_result() {
        let cheap = target(AgentFramework::Local, "qwen");
        let pricey = target(AgentFramework::ClaudeCode, "opus");

        let build_reg = || {
            let mut reg = ProviderRegistry::new();
            reg.register(Box::new(FakeProvider::new(
                AgentFramework::Local,
                "qwen",
                Pricing::ZERO,
                usage_with_cache(),
            )));
            reg.register(Box::new(FakeProvider::new(
                AgentFramework::ClaudeCode,
                "opus",
                anthropic(),
                usage_with_cache(),
            )));
            reg
        };

        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::Debugging, vec![cheap, pricey]);
        let verifier =
            FakeVerifier::new(&[("local/qwen", false, 0.3), ("claude-code/opus", true, 0.9)]);
        let m = mission(vec![
            subtask("b", SubtaskKind::Debugging),
            subtask("a", SubtaskKind::Debugging),
        ]);

        let r1 = Orchestrator::run(&m, &build_reg(), &matrix, &prefix(), &verifier).unwrap();
        let r2 = Orchestrator::run(&m, &build_reg(), &matrix, &prefix(), &verifier).unwrap();
        assert_eq!(r1, r2);
        // And topo order is respected: "a" before "b".
        assert_eq!(r1.outcomes[0].subtask, SubtaskId::new("a"));
        assert_eq!(r1.outcomes[1].subtask, SubtaskId::new("b"));
    }

    // ── missing provider skipped, not panicked ────────────────────────────────

    #[test]
    fn missing_provider_for_tier_target_is_skipped() {
        let missing = target(AgentFramework::Cursor, "ghost");
        let real = target(AgentFramework::ClaudeCode, "opus");

        let mut reg = ProviderRegistry::new();
        // Only `real` is registered; `missing` has no provider.
        reg.register(Box::new(FakeProvider::new(
            AgentFramework::ClaudeCode,
            "opus",
            anthropic(),
            usage_with_cache(),
        )));

        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::Debugging, vec![missing, real.clone()]);
        let verifier = FakeVerifier::new(&[("claude-code/opus", true, 0.9)]);

        let result =
            Orchestrator::run(&mission(vec![subtask("s", SubtaskKind::Debugging)]), &reg, &matrix, &prefix(), &verifier)
                .unwrap();

        let o = &result.outcomes[0];
        assert_eq!(o.attempts.len(), 1, "missing target produces no attempt");
        assert_eq!(o.winner.as_ref(), Some(&real));
        // tier_rank is the tier index (1), not the attempt index (0).
        assert_eq!(o.attempts[0].tier_rank, 1);
    }

    #[test]
    fn erroring_provider_is_skipped() {
        let bad = target(AgentFramework::Local, "broken");
        let good = target(AgentFramework::ClaudeCode, "opus");

        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(FakeProvider::erroring(AgentFramework::Local, "broken")));
        reg.register(Box::new(FakeProvider::new(
            AgentFramework::ClaudeCode,
            "opus",
            anthropic(),
            usage_with_cache(),
        )));

        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::Debugging, vec![bad, good.clone()]);
        let verifier = FakeVerifier::new(&[("claude-code/opus", true, 0.9)]);

        let result =
            Orchestrator::run(&mission(vec![subtask("s", SubtaskKind::Debugging)]), &reg, &matrix, &prefix(), &verifier)
                .unwrap();

        let o = &result.outcomes[0];
        assert_eq!(o.attempts.len(), 1, "erroring provider makes no attempt");
        assert_eq!(o.winner.as_ref(), Some(&good));
    }

    // ── no candidates ──────────────────────────────────────────────────────────

    #[test]
    fn empty_tier_yields_no_candidates() {
        let reg = ProviderRegistry::new();
        let matrix = CapabilityMatrix::new(); // no tier for any kind
        let verifier = FakeVerifier::new(&[]);

        let err = Orchestrator::run(
            &mission(vec![subtask("s", SubtaskKind::Debugging)]),
            &reg,
            &matrix,
            &prefix(),
            &verifier,
        )
        .unwrap_err();
        match err {
            OrchestratorError::NoCandidates(id) => assert_eq!(id, SubtaskId::new("s")),
            other => panic!("expected NoCandidates, got {other:?}"),
        }
    }

    #[test]
    fn all_targets_missing_yields_no_candidates() {
        let reg = ProviderRegistry::new(); // nothing registered
        let matrix = CapabilityMatrix::new()
            .with(SubtaskKind::Debugging, vec![target(AgentFramework::Local, "x")]);
        let verifier = FakeVerifier::new(&[]);

        let err = Orchestrator::run(
            &mission(vec![subtask("s", SubtaskKind::Debugging)]),
            &reg,
            &matrix,
            &prefix(),
            &verifier,
        )
        .unwrap_err();
        assert!(matches!(err, OrchestratorError::NoCandidates(_)));
    }

    // ── cycle ────────────────────────────────────────────────────────────────

    #[test]
    fn cycle_is_propagated() {
        let reg = ProviderRegistry::new();
        let matrix = CapabilityMatrix::new();
        let verifier = FakeVerifier::new(&[]);
        let m = Mission {
            id: "m".into(),
            goal: "g".into(),
            subtasks: vec![
                Subtask {
                    id: SubtaskId::new("a"),
                    kind: SubtaskKind::Debugging,
                    summary: "a".into(),
                    deps: vec![SubtaskId::new("b")],
                },
                Subtask {
                    id: SubtaskId::new("b"),
                    kind: SubtaskKind::Debugging,
                    summary: "b".into(),
                    deps: vec![SubtaskId::new("a")],
                },
            ],
        };
        let err = Orchestrator::run(&m, &reg, &matrix, &prefix(), &verifier).unwrap_err();
        assert!(matches!(err, OrchestratorError::Cycle(_)));
    }

    // ── struct ergonomics ────────────────────────────────────────────────────

    #[test]
    fn fixed_seed_value_is_oryn() {
        assert_eq!(FIXED_SEED, 0x4F52_594E);
    }

    #[test]
    fn verdict_is_copy_and_eq_by_value() {
        let v = Verdict { passed: true, score: 0.5 };
        let w = v;
        assert_eq!(v, w);
    }
}
