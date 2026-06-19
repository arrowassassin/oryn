# Oryn Orchestrator — "Route, Don't Race" Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:test-driven-development for every task. Build in `crates/oryn-core` under the `orchestrator` module tree.

**Goal:** The novel core of Oryn — a deterministic, verifier-gated capability cascade over a cache-stable prefix that routes each typed coding sub-task to the cheapest-capable **execution target** (a `(framework, model)` pair the user actually has), reuses a byte-identical prompt prefix to maximize each provider's prompt cache, and escalates only on verification failure. Cheaper + faster + better than naive multi-agent racing.

**The execution atom — `ExecutionTarget { framework, model }`:** Users supply credentials per agent framework (Claude Code, Codex, Cursor, aider, Gemini CLI, local runtime). Oryn **dynamically discovers** the models each framework/key can access — never a hardcoded model list. The deduplicated union of `(framework × accessible models)` is the candidate set. Routing picks a target (framework *and* model); the orchestrator triggers that framework's harness with that model.

**Capability is data, derived & pinned — never hardcoded routing:** Each model has a `CapabilityProfile` (0–1 score per sub-task kind). Defaults ship as a **bundled seed**; a background **catalog** refreshes profiles from a trustworthy benchmark source on an interval into a **pinned, versioned snapshot**. A mission binds to one snapshot and records its provenance, so routing stays deterministic, offline-capable, and replayable. The tier ordering is *computed* at session start from discovered targets + their pinned capability + their real cost — not asserted.

**Pipeline:** `discover targets → decompose → topo-route cheapest-capable-first → run against cache-stable prefix → verify by execution → escalate on fail → deterministic tie-break → synthesize`. All logic is pure/deterministic; network (discovery, model HTTP, catalog fetch) and verification (exec) sit behind traits, TDD-able with fakes.

**Why novel (defensible):** RouteLLM/NotDiamond route one query → one model via a *learned* router; Mixture-of-Agents aggregates all, always; existing orchestrators race all, pick one. Oryn is the first to fuse (1) deterministic rule-routing by coding-sub-task type over discovered `(framework, model)` targets, (2) cache-stable-prefix discipline as a first-class scheduling constraint, and (3) execution-based verify-then-escalate — all reproducible and auditable, with capability sourced from pinned benchmarks rather than baked in.

## Global Constraints (bind every task)
- Rust edition 2024, `unsafe_code = forbid`, clippy `all = warn`. Zero clippy warnings.
- **Determinism mandatory:** no `Date/Instant::now`/`rand` inside logic (timestamps passed in); no order-observable `HashMap` iteration (use `BTreeMap`/sorted `Vec`); all tie-breaks total and explicit (use `f64::total_cmp` for float ordering).
- **TDD; ≥ 87% line coverage per task** (`cargo llvm-cov -p oryn-core --summary-only`). `cargo test -p oryn-core` + `cargo clippy -p oryn-core --all-targets` clean before DONE. NO `Co-Authored-By` trailer.
- Reuse `oryn-core` types: `event::TokenUsage`, `ids::{ArtifactId, EventId}`. No network/fs in this plan — all I/O behind traits with in-test fakes.
- New code under `crates/oryn-core/src/orchestrator/`; add `pub mod <name>;` to `orchestrator/mod.rs` per task.

---

### Task 1: Typed task model (`orchestrator/task.rs`) — ✅ DONE (592228e)
`SubtaskKind` (6 variants, +`Ord`), `SubtaskId`, `Subtask`, `Mission`, `Mission::topo_order()` deterministic + cycle-detecting.

### Task 2: Model provider abstraction (`orchestrator/provider.rs`) — ✅ DONE (d555ff7)
`ModelId`, `ModelKind {Api,Local}`, `Pricing`, `ModelSpec`, `CompletionRequest/Response` (reusing `TokenUsage`), `ModelProvider` trait, `ProviderRegistry`. (Extended in Task 4.)

### Task 3: Cache-stable prefix (`orchestrator/prefix.rs`) — ✅ DONE (35e4344)
`CacheStablePrefix {system,repo_map,task}`, byte-stable `render()`, `handle()` via `ArtifactId`, `repo_map_from` (sorted).

---

### Task 4: Agent frameworks + execution targets + dynamic discovery (`orchestrator/discovery.rs`, modify `provider.rs`)

The routing/execution atom and how the candidate set is discovered from the user's credentials.

- In `provider.rs`: add `PartialOrd, Ord` to `ModelId`; add field `framework: AgentFramework` to `ModelSpec`; add `ModelSpec::target(&self) -> ExecutionTarget`; change `ProviderRegistry` to key lookups by `ExecutionTarget` (`get(&ExecutionTarget) -> Option<&dyn ModelProvider>`) since the same model via two frameworks is two providers. Update Task 2's existing tests accordingly.
- `AgentFramework` enum: `ClaudeCode, Codex, Cursor, Aider, GeminiCli, Local`. Derive `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize`; `Display` with stable kebab strings (`"claude-code"`, `"gemini-cli"`, …).
- `ExecutionTarget { framework: AgentFramework, model: ModelId }`: same derives; `Display` = `"{framework}/{model}"`.
- In `discovery.rs`: `trait ModelDiscovery: Send + Sync { fn framework(&self) -> AgentFramework; fn discover(&self) -> Result<Vec<ModelSpec>, DiscoveryError>; }` — object-safe; real impls (fetch models a key can access) are a later segment. `DiscoveryError` (thiserror): `Unauthorized`, `Unavailable`.
- `fn discover_targets(sources: &[&dyn ModelDiscovery]) -> (Vec<ModelSpec>, Vec<DiscoveryError>)` — run each source; collect specs **deduplicated by `ExecutionTarget`** (first wins) and returned in deterministic `ExecutionTarget` order; a source that errors is skipped and its error collected (missing creds for one framework must not break the rest).

**Tests (crate-local `FakeDiscovery`):** `discover_targets` unions multiple sources, dedups by target, returns deterministic order; an erroring source is skipped + recorded; same model under two frameworks → two distinct targets; `ExecutionTarget`/`AgentFramework` `Display` + serde; registry `get` by `ExecutionTarget` (hit/miss); existing Task 2 tests still pass with the new `framework` field.

### Task 5: Capability resolution over execution targets (`orchestrator/capability.rs`, modify `task.rs`)

Routing is DERIVED deterministically from discovered targets + pinned capability + real cost.

- In `task.rs`: add `pub const ALL: [SubtaskKind; 6]`.
- `CapabilityProfile { scores: BTreeMap<SubtaskKind, f64> }` keyed per **model** (`score(kind) -> f64`, 0 if absent; floats → no `Eq`).
- `default_profiles() -> BTreeMap<ModelId, CapabilityProfile>` — **bundled seed/fallback** (used first-run/offline/source-down; NOT the source of truth — Task 8 refreshes a pinned snapshot). Research rankings as data; scores chosen so the research ordering falls out (e.g. `Debugging`: gpt-5-high 0.95, opus 0.92, sonnet 0.6, local-qwen 0.4; `MechanicalEdit`: sonnet 0.9, local-qwen 0.85, gemini-flash 0.8). Principled, not arbitrary.
- `pub const MIN_CAPABILITY: f64 = 0.5`.
- `CapabilityMatrix { tiers: BTreeMap<SubtaskKind, Vec<ExecutionTarget>> }` (resolved output); `tier(kind) -> &[ExecutionTarget]`; `with(kind, Vec<ExecutionTarget>)` builder for tests/overrides.
- `fn cost_metric(p: &Pricing) -> f64` = `input + output` (local/zero → 0.0 → sorts first).
- `fn framework_rank(f: AgentFramework) -> u8` — deterministic tertiary preference (document as tunable; default e.g. Local=0 then alphabetical) used only as a tie-break.
- `resolve_matrix(available: &[ModelSpec], profiles: &BTreeMap<ModelId, CapabilityProfile>) -> CapabilityMatrix`: for each `SubtaskKind::ALL`, keep specs whose `profiles[model].score(kind) ≥ MIN_CAPABILITY` (no profile ⇒ 0 ⇒ skip); sort by total key `(cost_metric asc, score desc via total_cmp, ModelKind Local-before-Api, framework_rank asc, ExecutionTarget asc)`; map to `ExecutionTarget`; insert if non-empty.

**Tests:** `SubtaskKind::ALL` = 6 unique. `resolve_matrix` over 3–4 fake specs (mix local-zero-cost + priced api, across ≥2 frameworks) → per kind only capability-clearing targets, cheapest-first (a local/zero-cost target precedes a pricier equal-or-lower-score one; a sub-bar model excluded; a model absent from `available` never appears even if profiled). Deterministic (resolve twice → identical). `default_profiles()` resolved against all known ids reproduces the research orderings.

### Task 6: Cost model with cache economics (`orchestrator/cost.rs`) — ✅ DONE (3a09dfc)
`cost_usd`, `baseline_usd`, `cache_savings_usd` (clamped ≥0), `Spend {gross_usd, saved_usd}` with `add`/`baseline_usd`/`fraction_saved`. 24 tests, 100% coverage.

- `cost_usd(usage, p)` = `(input*p.input + output*p.output + cache_read*p.cache_read + cache_write*p.cache_write)/1e6` (per-million).
- `baseline_usd(usage, p)` = no-cache cost: `((input+cache_read+cache_write)*p.input + output*p.output)/1e6`.
- `cache_savings_usd = baseline_usd - cost_usd` (≥0).
- `Spend { gross_usd, saved_usd }` with `add(usage, pricing)` and `fraction_saved()` (guard /0).

**Tests:** known Anthropic-like numbers (input 3.0/out 15.0/cache_read 0.30/cache_write 3.75 per M); cache_read ≈0.1× input; savings ≥0; zero-usage→0; `Spend` accumulates; `fraction_saved` guards zero. Float asserts via `(a-b).abs() < 1e-9`.

### Task 7: The cascade scheduler (`orchestrator/scheduler.rs`) — heart; depends on Tasks 1–6 — ✅ DONE (03ccb1b)
`Verifier`/`Verdict`/`Attempt`/`SubtaskOutcome`/`MissionResult`, `Orchestrator::run` (topo → tier cheapest-first → complete vs prefix at temp 0 / `FIXED_SEED` → verify → stop on pass / escalate on fail; missing+erroring providers skipped; `NoCandidates` when none attemptable; all-fail best-effort winner by total tie-break). 13 tests, ~100% coverage.

- `trait Verifier { fn verify(&self, subtask: &Subtask, response: &CompletionResponse) -> Verdict; }`
- `Verdict { passed: bool, score: f64 }` (no `Eq`).
- `Attempt { target: ExecutionTarget, tier_rank: usize, usage: TokenUsage, verdict: Verdict }`.
- `SubtaskOutcome { subtask: SubtaskId, attempts: Vec<Attempt>, winner: Option<ExecutionTarget>, response_text: String }`.
- `MissionResult { outcomes: Vec<SubtaskOutcome>, spend: Spend }`.
- `Orchestrator::run(mission, registry, matrix, prefix, verifier) -> Result<MissionResult, OrchestratorError>`: `topo_order`; per subtask walk `matrix.tier(kind)` cheap→frontier (tier_rank=index), resolve provider from registry by `ExecutionTarget` (skip+record if absent), build `CompletionRequest { prefix: prefix.render(), suffix: subtask.summary, temperature: 0.0, seed: Some(FIXED_SEED) }`, `complete`, `verify`, accumulate `spend` from usage+pricing; **stop at first `passed`**; else select best by total tie-break `(score desc, tier_rank asc, ExecutionTarget asc)`. `OrchestratorError` (thiserror) wraps `CycleError`; `NoCandidates(SubtaskId)`. `const FIXED_SEED: u64`.

**Tests (crate-local `FakeProvider`+`FakeVerifier`):** cheap tier passes → one attempt, winner=tier-0 target (cheaper); cheap fails→next passes (escalation recorded); all fail→tie-break winner asserted exactly; spend accumulates; same mission twice → identical `MissionResult`; missing provider for a tier target skipped not panicked.

### Task 8: Capability catalog + benchmark sources (`orchestrator/catalog.rs`) — ✅ DONE (fda908f)
`CatalogProvenance`, `RawBenchmarks`, `DimensionWeights`/`default_weights`, `map_benchmarks` (normalized weighted clamped sum), `CapabilitySource`/`SourceError`, `CapabilityCatalog::{seed, refreshed}`. 18 tests, 100% coverage.

Refresh capability from a trusted benchmark source on an interval into a pinned snapshot; missions bind + record provenance. Network fetch deferred behind a trait.

- `CatalogProvenance { source: String, fetched_at_unix: u64, version: String }` (timestamp passed in, never read from a clock).
- `CapabilityCatalog { profiles: BTreeMap<ModelId, CapabilityProfile>, provenance: CatalogProvenance }`; `seed()` wraps `default_profiles()` with provenance `{source:"bundled-seed", fetched_at_unix:0, version:"seed"}`.
- `trait CapabilitySource { fn id(&self)->&str; fn fetch(&self)->Result<RawBenchmarks, SourceError>; }` (object-safe; Aider/SWE-bench/OpenRouter impls later). `RawBenchmarks { metrics: BTreeMap<ModelId, BTreeMap<String,f64>> }`.
- `DimensionWeights` mapping `(SubtaskKind, dimension) -> weight`; `default_weights()` (DiffEdit←aider-polyglot; Debugging/Refactor←swe-bench; LargeContext←long-context).
- `map_benchmarks(raw, weights) -> BTreeMap<ModelId, CapabilityProfile>` — deterministic normalized weighted sum, clamped 0..=1.
- `CapabilityCatalog::refreshed(source, weights, now_unix) -> Result<CapabilityCatalog, SourceError>` — fetch→map→wrap provenance. Document: caller falls back to `seed()` on error; refresh-interval policy lives in the app layer; missions pin one snapshot, never fetch mid-run.
- `SourceError` (thiserror): `Unavailable`, `Malformed(String)`.

**Tests (crate-local `FakeSource`):** `seed()` provenance + profiles == `default_profiles()`; `map_benchmarks` deterministic & expected per-kind ordering (aider-strong model outranks aider-weak for DiffEdit); `refreshed()` provenance `fetched_at_unix == now_unix` passed in, profiles reflect metrics; `refreshed()` surfaces `SourceError` on fake failure; `resolve_matrix(specs, &catalog.profiles)` integrates.

---

### Post-Task-8 review fixes
- **discovery wired (2aae718):** `orchestrator/discovery.rs` (Task 4) was committed but never added to `mod.rs`, so its 15 tests never compiled. Wired in + corrected a stale framework-ordering test.
- **framework_rank (5d28e41):** the plan's documented tunable framework tie-break (Task 5) was missing — framework ordering fell out implicitly of enum declaration order. Made it an explicit, auditable `framework_rank` (Local preferred, then alphabetical) between the ModelKind and ExecutionTarget sort stages.
- **integration smoke (ac64ad7):** `crates/oryn-core/tests/orchestrator_pipeline.rs` exercises discover → resolve(seed) → 3-subtask `Orchestrator::run` end-to-end.

## Verification (whole feature)
- `cargo test -p oryn-core` green; `cargo clippy -p oryn-core --all-targets` clean; coverage ≥ 87% overall and per new module.
- Determinism test: `Orchestrator::run` twice on same inputs → equal results.
- Integration smoke: `discover_targets([fakes])` → `resolve_matrix(specs, &CapabilityCatalog::seed().profiles)` → a 3-subtask mission through `Orchestrator::run`, asserting topo order, escalation on the hard node, the winning `ExecutionTarget`s, and positive `spend.saved_usd`.
