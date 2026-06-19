# oryn

**A local-first, vendor-neutral control plane for AI coding agents.** Oryn
discovers the models each coding CLI you have installed actually exposes, decomposes
a task into typed subtasks, and runs the deterministic **route, don't race**
cascade — sending each subtask to the cheapest-capable `(framework, model)` target
first and escalating only when a local advisor model rejects the result.

This repo is a Rust workspace:

| Crate | What it is |
|-------|------------|
| [`oryn-core`](crates/oryn-core) | The headless engine: typed mission model, deterministic decomposition, capability matrix, the cascade scheduler, cost accounting, the cache-stable prefix, live model discovery, and the catalog store. Network-, process- and clock-free — all I/O is injected. |
| [`oryn-app`](crates/oryn-app) | A [GPUI](https://www.gpui.rs) desktop frontend. Launching a run drives the **real** engine on a background thread and renders the real result — no simulation. |

## How a run works (the real path)

1. **Discover** — for each selected framework (`claude`, `codex`, `gemini`,
   `aider`, …) Oryn runs the CLI's own list command via a real subprocess and
   parses exactly the models it reports. No hardcoded model names.
2. **Decompose** — the free-text task becomes a typed `Mission` of `Subtask`s with
   dependency edges ([`decompose`](crates/oryn-core/src/orchestrator/decompose.rs)),
   deterministically, so a run is reproducible.
3. **Share context** — one byte-identical, content-addressed **cache-stable
   prefix** (system + repo map + task) is built once and reused across every
   target, so providers serve it from their prompt cache and only the volatile
   per-subtask suffix is re-billed.
4. **Cascade** — the scheduler climbs each subtask's capability tier
   cheapest-first, creating a **real isolated git worktree** per target and running
   the harness CLI there. Progress streams live (`subtask N/M`).
5. **Verify by execution** — each result is gated by running the project's test
   command *in that target's worktree* and checking the exit code (auto-detected:
   Cargo/Go/npm/pytest, override with `ORYN_TEST_CMD`); the local **advisor**
   (an OpenAI-compatible endpoint such as Ollama) is the fallback when there's no
   test runner. The cascade stops at the first attempt that genuinely passes.
6. **Report & promote** — the UI renders the real `MissionResult`: which
   `(framework, model)` won each subtask, reported tokens, cost from live pricing,
   the verdict, the actual diff (`+`/`−` lines), and total spend. Promoting a
   winner applies its worktree's changes onto your repo and tears down the losers.

When no coding CLI is installed, discovery honestly returns zero targets and the
app says so — it never invents results.

## More

- **Command palette** (top-bar search / ⌘K) — fuzzy command search, keyboard-driven.
- **Cancel** an in-flight run; **persisted** preferences across launches.
- **Context Broker** — a real content-addressed artifact store; the shared
  cache-stable prefix is stored once across targets (real dedup numbers).
- **CLI detection** — Launch shows which coding CLIs are actually installed.
- **CI** — `.github/workflows/ci.yml` enforces fmt, clippy (`-D warnings`),
  build, and tests on every push/PR.

## Run it

```sh
cargo run -p oryn          # opens the desktop app (needs a display)
cargo test                 # 320+ unit + integration tests
```

### Configuration (environment)

| Variable | Purpose | Default |
|----------|---------|---------|
| `ORYN_ADVISOR_ENDPOINT` | OpenAI-compatible advisor endpoint | `http://localhost:11434` |
| `ORYN_ADVISOR_MODEL` | Local advisor model | `qwen2.5-coder:7b` |
| `ORYN_WORKTREE_BASE` | Where per-target worktrees are created | `.oryn/worktrees` |
| `ORYN_CATALOG_PATH` | Parked catalog (pricing + benchmarks) | `~/.oryn/catalog.json` |
| `ARTIFICIALANALYSIS_API_KEY` | Use Artificial Analysis for pricing+benchmarks | _(keyless OpenRouter + Aider leaderboard if unset)_ |

## License

MIT — see [LICENSE](LICENSE).
