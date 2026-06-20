# Oryn

**Compile less, test less, trust the results.** A safe, Rust-native accelerator
for Cargo workspaces: it runs only the tests your change can affect, skips the
ones it can *prove* are unchanged, scores flaky tests with real statistics, and
tunes your build — all on stable Rust, with no risk of a wrong result.

No AI, no nightly, no homegrown compile cache — just classical, deterministic CS.

## Install

```bash
cargo install oryn
```

Optional: `cargo install cargo-nextest` (richer per-test history) and
`rustup component add llvm-tools-preview` (for function-level `oryn test --fn`).

## Quick start

```bash
oryn test            # test only the crates your change affects; skip the rest
oryn test            # nothing changed → all cached green, in milliseconds ✓
oryn flaky           # rank flaky tests with confidence intervals + rerun budget
oryn tui             # live dashboard
```

In CI, `oryn test --since origin/main` is a faster, safe drop-in for `cargo test`.

## Benchmarks

Warm `oryn test` (nothing changed) vs `cargo test --workspace`, on a 4-core CI runner:

| Repo | Test suite | `cargo test` | `oryn test` | Speedup |
|------|-----------|-------------|-------------|---------|
| [ripgrep](https://github.com/BurntSushi/ripgrep) | 1,139 tests | 5.7 s | 0.063 s | **~91×** |
| [tokio](https://github.com/tokio-rs/tokio) | full suite | 92.1 s | 0.074 s | **~1,245×** |

The win scales with suite size: Oryn's cached path is roughly constant (fingerprint,
then skip), while `cargo test` re-runs everything. Reproduce with [`bench/run.sh`](bench).

## Features

- **Safe test selection.** From a git diff, run the changed crates plus every
  crate that depends on them — never fewer. Function-level mode (`oryn cover` +
  `oryn test --fn`) narrows further to the exact tests whose recorded coverage
  touches your change (a one-function edit on ripgrep selected ~250 of 1,139).
- **Sound result cache.** A BLAKE3 Merkle fingerprint of each crate's full file
  closure + `Cargo.lock` + `rustc` version. Matching fingerprint ⇒ the outcome
  can't have changed ⇒ skip. A `cargo update` or any file edit invalidates it.
- **Rigorous flaky scoring.** Wilson + Jeffreys credible intervals and the rerun
  budget the statistics actually require (`n ≥ ln(1−γ)/ln(1−p)`) — instead of the
  naive "fail-then-pass in 3 reruns" rule. Fail-fast ordering, O(log n) bisection.
- **Build doctor.** `oryn tune --apply` writes only sound, stable config
  (`debug = "line-tables-only"`, `split-debuginfo`, the right linker for your
  target) and wraps `sccache` rather than reinventing a compile cache.
- **Terminal UI.** `oryn tui` — selection, cache state, crates, and flaky stats.

## Commands

```
oryn test   [--since <ref>] [--all] [--cache] [--fn]    run only affected, skip cached-green
oryn cover  [--since <ref>]                             record per-test coverage for --fn
oryn build  [--since <ref>] [--all] [--tests] [--cache] build only affected crates
oryn affected [--since <ref>] [--json]                  show what a change affects
oryn flaky  [--input runs.jsonl] [--json]               Wilson + Bayes + rerun budget
oryn budget --fail-rate 0.01 --confidence 0.95          reruns needed to catch a flake
oryn tune [--apply]                                     detect/apply sound compile speedups
oryn tui / setup / cache / info
```

## Soundness & limits

The cache captures **source closure + `Cargo.lock` + `rustc` version**. Tests that
depend on un-captured state — network, wall-clock, ambient env, or dependencies
outside the workspace via `path`/`patch` — aren't perfectly hermetic; the flaky
subsystem surfaces such tests, and `--no-cache` forces a full re-run. Crate-level
selection is the safe default; function-level selection needs `llvm-tools-preview`
and a clean working tree.

## License

MIT — see [LICENSE](LICENSE).
