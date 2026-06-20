# Oryn

**Compile less, test less, trust the results.** A safe, Rust-native developer
tool that runs only the tests your change can affect and scores flaky tests with
real statistics — the things hyperscalers built internally and only paid tools
(Gradle Develocity, CloudBees) sell, packaged free for any Cargo project.

No model in the loop — just classical, deterministic computer science.

## Why

- **`cargo test` runs everything**, even when you changed one crate. `cargo-nextest`
  parallelizes but does **no** change-impact analysis. The only products that
  select tests by impact are paid and platform-locked.
- **Every runner calls a test "flaky" from a naive 2–3 rerun rule.** That's
  statistically wrong: a test that fails 1% of the time needs ~300 reruns to be
  seen failing once with 95% confidence. Nobody reports a flake *rate with a
  confidence interval* or the rerun budget the math actually requires.

Oryn fills both gaps.

## What it does

### 1. Safe test-impact selection (`oryn affected`, `oryn test`)
From a git diff, Oryn works out which workspace crates can possibly be affected —
the changed crates **plus every crate that transitively depends on them** — and
runs tests for only those. A docs-only change runs nothing; a change to a
workspace-level file (`Cargo.lock`, root `Cargo.toml`, toolchain) safely forces a
full run.

This is **safe by construction** at crate granularity: a crate is Cargo's unit of
compilation, so a crate's tests can only change if its own sources or one of its
(transitive) dependencies changed. (The crate-level variant RustyRTS, ICST 2025,
measured this at ~99.99% of failure-revealing tests selected.) It is conservative
— it may over-select, never under-select.

### 2. Statistically-rigorous flaky scoring (`oryn flaky`, `oryn budget`)
Given rerun history, Oryn reports each test's **flake rate with a Wilson
confidence interval** and the **rerun budget** required to confirm it
(`n ≥ ln(1−γ)/ln(1−p)`), instead of a binary guess. It even tells you what a
clean run *doesn't* prove ("20 passes only proves the flake rate is below 16%").

### 3. Honest compile-time tuning (`oryn tune`)
Nobody out-optimizes `rustc`, but the proven wins (fast linker, Cranelift dev
backend, caching, crate splitting) are config most devs never enable. `oryn tune`
detects what's available and tells you exactly what to turn on.

## Quick start

```bash
cargo build --release

# What would a full CI run vs. what's actually affected by my change?
oryn affected                 # working tree vs HEAD
oryn affected --since origin/main   # for a PR

# Run only the affected crates' tests (skips the rest, safely):
oryn test
oryn test --since origin/main --nextest

# Flaky-test statistics from rerun history ({"id","passes","fails"} JSONL):
oryn flaky --input runs.jsonl

# How many reruns to confirm a 1% flake at 95% confidence?
oryn budget --fail-rate 0.01 --confidence 0.95   # -> 299

# Detect compile-time speedups you haven't enabled:
oryn tune
```

`oryn test` exits non-zero if the selected tests fail, so it drops straight into
CI as a faster, safe replacement for `cargo test`.

## Workspace

| Crate | What it is |
|-------|------------|
| [`oryn-core`](crates/oryn-core) | The engine: workspace graph + safe selection (`graph`, `metadata`, `git`, `select`) and statistical flaky scoring (`flaky`, `stats`). Pure, deterministic, unit-tested. |
| [`oryn-cli`](crates/oryn-cli) | The `oryn` binary. |

## Roadmap

- **Function-level selection** (MIR call-graph, RustyRTS-style) for finer skips.
- **Correct content-addressed build/test caching** with early-cutoff and
  hermeticity checks — fixing the unsound blind spots of `sccache`.
- **Safe batching + bisection** for suites too coupled to select.
- Per-test history collection so flaky scoring and prioritization run automatically.

## License

MIT — see [LICENSE](LICENSE).
