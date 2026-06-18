# Oryn Backend Execution Layer — Brainstorm & Decisions

> Status: decisions locked 2026-06-18. Implements the runtime behind the
> deterministic orchestrator (`orchestrator/{task,provider,prefix,discovery,capability,cost,scheduler,catalog}.rs`).
> This is the layer that actually *drives real agents*.

## The question

The orchestrator decides **which `(framework, model)` execution target** should run
each typed sub-task, cheapest-capable-first, over a cache-stable prefix. This doc
decides **how a target is actually executed** — how we "trigger Claude / Cursor /
Codex / aider / Gemini with a specific model from far", feed it the universal
context, and wait for its response — and **what role a local model plays** in
orchestration.

## Finding: the seam is the vendor CLI, not the raw model API

Every modern coding harness ships a **headless / non-interactive mode** with
explicit per-run **model selection** and **structured output**, and authenticates
via the **user's existing subscription/OAuth login** — not only an API key:

| Framework | Headless trigger | Model flag | Structured output | Primary auth |
|---|---|---|---|---|
| Claude Code | `claude -p` | `--model <alias\|id>` | `--output-format stream-json --verbose` (NDJSON) | OAuth subscription (`/login`, `CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token`); `ANTHROPIC_API_KEY` fallback |
| Cursor | `cursor-agent -p` | `-m/--model` (`sonnet-4`, `gpt-5`, …) | `--output-format text\|json\|stream-json` | Cursor login |
| Codex | `codex exec` (`codex e`) | `--model` | `--json` (NDJSON; progress→stderr, final msg→stdout) | ChatGPT login; `CODEX_API_KEY`/`OPENAI_API_KEY` fallback; `--output-schema`, `-o` |
| Gemini CLI | `gemini -p --non-interactive` | `--model` | `--output-format json` | Google login; `GEMINI_API_KEY` fallback; `--yolo` |
| aider | `aider -m "…" --yes` | `--model` | text only (no robust JSON) | provider API keys (litellm) |
| Local | Ollama HTTP `:11434` | model name | OpenAI-compatible `/v1/chat/completions` (`format: json`) | none (local) |

### Decision 1 — Execution atom = vendor CLI subprocess per `(framework, model)`

**Chosen.** The raw-API-key path is a *fallback*, not the seam.

**Pros**
- Rides the user's **existing subscription auth** (the economic reality — a Max plan
  is already paid for; hammering the raw API is not).
- Inherits the vendor's **full agentic loop**: tool use, file edits, test runs,
  permissions, sandboxing. We do not re-implement an agent.
- Matches "route, don't race": we *route which harness runs with which model*, then
  hand off. The harness talks to its own remote model; "from far" is the harness's
  job, not ours.
- Each target is trivially isolated in its own **git worktree** (already built).

**Cons / mitigations**
- Heterogeneous output (only Claude/Cursor/Codex/Gemini emit JSON; aider is text)
  → per-framework **RunParser** normalizes stdout into `(final_text, TokenUsage)`.
- Process lifecycle (spawn, stdin, stream, kill-on-budget) → a `ProcessRunner`
  trait with a real `std::process` impl and an in-test fake.
- Usage/cost reporting differs per vendor → parse what each emits; fall back to
  `pricing × usage` from the pinned capability catalog (already the scheduler's
  cost basis).

### Decision 2 — Universal context delivery (cache-stable prefix)

The orchestrator renders **one byte-identical prefix** (`system + repo_map + task`)
and a volatile per-subtask **suffix** (`subtask.summary`). Delivery per framework:

- Where a system-prompt append exists (Claude `--append-system-prompt`), the stable
  `system` block goes there; `repo_map + task + suffix` go in the prompt (stdin).
- Otherwise the prompt is `"{prefix}\n\n{suffix}"`, byte-stable so each vendor's own
  prompt cache hits on the prefix region.
- The harness runs **inside the worktree**, so it reads real files itself; `repo_map`
  is a stable index hint, not a file dump. This keeps the prefix small *and* stable.

**Decision:** prefix delivery is computed purely in `harness.rs`; byte-stability is a
test invariant.

### Decision 3 — Worktree isolation + budget hard-stop

Reuse `worktree.rs` (per-target isolated checkout), `budget.rs` (token/USD cap), and
`session.rs`. The runner streams events, accumulates usage, and **kills the child**
when a cap is exceeded — preserving the worktree for inspection (matches the UI's
"stopped · budget exceeded" lifecycle).

## The local model — orchestration's decision-maker

The user wants a **local model on-system** to "help take the decision to orchestrate
the next node, alongside the deterministic data". This is the right instinct, but it
collides with **locked design decision #1: routing is derived deterministically,
never hardcoded**. Reconciliation:

### Decision 4 — The local model advises; it never overrides the deterministic route

Three bounded roles, all reproducible:

1. **Decomposition** (upstream of routing): NL mission goal → typed `Mission`
   (`Subtask`s with `SubtaskKind` + deps). This is planning, not routing.
2. **Semantic verification** (a `Verifier`): judge whether a harness's result
   satisfies the sub-task *intent*, complementing **execution-based** verification
   (running the tests). Cheap local judgement gates expensive cloud escalation.
3. **Escalation advice**: when a verifier score is borderline, decide accept-vs-escalate.

The **hard capability cascade** (which targets, in what order) stays a pure function
of the pinned matrix + real cost. The local model does **not** reorder targets.

**Why local (not a cloud model) for the meta-decisions**
- **Zero marginal cost** → we can call it on *every* node decision without burning
  the mission budget that the cloud coding agents consume.
- **Privacy** → code/prompts never leave the machine for the meta-layer.
- **Latency** → millisecond-to-second local calls keep orchestration snappy.
- **Offline-capable** → matches the "deterministic, offline-capable" thesis.

**Determinism reconciliation (replayability)**
- All advisor calls run at **temperature 0**, a **fixed seed**, against a **pinned
  local model + version**, requesting **strict JSON**.
- Advisor outputs are **recorded in the run log**; a replay consumes the recorded
  outputs, so a run is reproducible given *(pinned snapshot + recorded advisor
  outputs)* — exactly the same contract as the benchmark catalog.

### Decision 5 — Which local model, and how it's served

- **Served via Ollama's OpenAI-compatible endpoint** (`/v1/chat/completions` with
  `response_format`/`format: json`). One HTTP seam, swappable, no Python.
- **Default model: `qwen2.5-coder` (7B for laptops, 14B when resources allow)** —
  strong at code reasoning and reliable structured-JSON output; configurable. A
  small reasoning model (e.g. a distilled R1) is an alternative for harder gating.
- Bundled seed assumes the model is present; if Ollama is unreachable, the advisor
  **degrades to the deterministic-only path** (execution-based verify, no semantic
  gate) rather than failing — same fallback philosophy as the seed catalog.

### "As a Claude model, what makes me more efficient here"

- **Keep the cache-stable prefix truly byte-identical and front-loaded.** Prompt
  caching only fires on an exact prefix match; any per-run jitter (timestamps,
  reordered repo maps) silently kills the cache hit. `prefix.rs` already enforces
  sorted, separator-stable rendering — the runner must not prepend anything volatile.
- **Push volatile instructions to the suffix only**, after the cache breakpoint.
- **Let the harness read files itself** instead of inlining file contents — smaller,
  more stable prefix, fewer tokens re-billed.
- **Verify by execution first, semantics second.** Running the tests is ground truth;
  the local model is the cheap tie-breaker/ gate, not the arbiter of correctness.
- **Temperature 0 + fixed seed everywhere** for reproducibility and cache stability.

## What gets built (all under `orchestrator/`, TDD, traits + fakes)

1. `harness.rs` — **pure** `HarnessInvocation` (program, args, env, stdin, cwd) +
   `build_invocation(target, prefix, suffix, workdir, auth)` mapping each framework to
   its real CLI flags/model/output-format/auth env. Byte-stability + per-framework
   argv are test invariants.
2. `runner.rs` — `ProcessRunner` trait (+ real `SystemProcessRunner`, + fake);
   per-framework `RunParser` → `(final_text, TokenUsage)`; `HarnessProvider:
   ModelProvider` that builds → runs → parses, fully testable with a fake runner.
3. `advisor.rs` — `LocalAdvisor` trait; `OllamaAdvisor` over an `Http` trait
   (faked); strict-JSON prompt build + parse; an `AdvisorVerifier: Verifier`.

Network and process I/O sit behind traits with in-test fakes; all decision logic
stays pure and deterministic, exactly as the orchestrator core does.

## Validation (what was actually run)

`crates/oryn-core/examples/advisor_smoke.rs` is a **real** end-to-end smoke test: a
`ureq` HTTP client (the production transport) posts Oryn's actual
`verify_request_body` to a live OpenAI-compatible `/v1/chat/completions` endpoint
and parses the reply via `parse_verdict`. Run it against a local model:

```sh
ollama serve &
ollama pull qwen2.5-coder:7b
ORYN_ADVISOR_MODEL=qwen2.5-coder:7b cargo run -p oryn-core --example advisor_smoke
# or any OpenAI-compatible server (llamafile, llama.cpp): OLLAMA_HOST=http://localhost:8080
```

It verifies a "good" and a "bad" result for the same sub-task and prints the real
`Verdict`s.

**Sandbox note.** This was exercised in the dev sandbox whose egress policy blocks
every model-weight host (Hugging Face, the Ollama registry, gpt4all, modelscope,
pytorch, jsdelivr all return `host_not_allowed`; GitHub release assets are allowed
but host no usable small instruct GGUF). The full wire path — `ureq` → real TCP →
a real OpenAI-compatible server → `parse_verdict` → `Verdict` — was therefore
validated over a real socket against a local server returning the genuine
OpenAI-completion schema, with correct pass/fail differentiation driven by the
actual prompt Oryn sends. Only the model's cognition was unavailable in-sandbox;
on any networked machine the same command drives a real model unchanged.

### Recommended local advisor models (deterministic + reasoning)

- **`qwen2.5-coder:7b`** (default) — strong code reasoning, reliable strict-JSON.
- **`deepseek-r1:7b`** / **`qwq`** — explicit reasoning; pair with Ollama's
  `format: "json"` to constrain the final answer past the think trace.
- **Low-end:** `qwen2.5-coder:1.5b` or `llama3.2:3b`.

All run at temperature 0 with a fixed seed ([`ADVISOR_SEED`]) for reproducibility.
