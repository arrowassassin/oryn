# Oryn

**Compile less, test less, trust the results.** A safe, Rust-native developer
tool that makes the Cargo edit→build→test loop dramatically faster — *correctly* —
scores flaky tests with real statistics, and visualizes it all in a terminal UI.
The parts hyperscalers built internally and only paid tools (Gradle Develocity,
CloudBees) sell, packaged free for any Cargo project.

No model in the loop — classical, deterministic computer science, grounded in
current research (RustyRTS ICST 2025; Gruber et al. ICST 2021; *Build Systems à
la Carte*; Meta predictive test selection; Beheshtian et al. EMSE 2024).

```text
$ oryn test          # run #1 — runs the affected crates, records them green
$ oryn test          # run #2 — nothing changed → all cached green, ~0.05s ✓
$ # edit one crate
$ oryn test          # runs only that crate; everything else stays cached
$ oryn tui           # dashboard: selection, cache, crates, flaky stats
```

## How it gets fast — without being wrong

Speed and correctness pull against each other, so each lever is deliberate:

1. **Test only what changed (safe selection).** From a git diff, Oryn tests the
   changed crates **plus every crate that transitively depends on them**, and
   skips the rest. Safe *by construction* at crate granularity — a crate is
   Cargo's unit of compilation. Conservative: may over-select, never
   under-select. Docs-only change → nothing; `Cargo.lock`/toolchain change →
   full run.

2. **Don't re-run known-green tests (sound result cache).** Oryn computes a
   **Merkle fingerprint** of each crate's entire dependency-closure of sources
   plus the exact `rustc` version. Matching fingerprint ⇒ the tests *cannot* have
   a different outcome ⇒ skip them. This is the millisecond warm loop.

3. **Stand on a correct compile cache, don't reinvent it.** A subtly-wrong
   compile cache silently miscompiles. `oryn build --cache` / `oryn test --cache`
   drive **sccache** (the correct, battle-tested shared cache) — Oryn does not
   ship a homegrown one. `oryn cache` shows hit/miss stats.

4. **Auto-apply the proven fast path.** `oryn tune` detects and configures the
   wins most devs never enable: fast linker (note: `rust-lld` is already default
   on x86_64-linux since Rust 1.90), sccache, dependency optimization,
   `split-debuginfo`.

## Trust the results — real statistics

Every runner labels a test "flaky" from a naive 2–3 rerun rule. That's
statistically wrong: a test that fails 1% of the time needs **~300 reruns** to be
seen failing once with 95% confidence. Oryn instead:

- estimates each test's flake rate with **both** a frequentist **Wilson** interval
  and a Bayesian **Jeffreys-prior credible** interval,
- reports the **rerun budget** the statistics require (`n ≥ ln(1−γ)/ln(1−p)`),
- tells you what a clean run *doesn't* prove ("20 passes only proves the flake
  rate is below 16%"),
- orders tests **fail-fast** (recently-failed, then fastest — the heuristic that
  beats ML, Cheng et al. ISSTA 2024),
- and provides O(log n) **bisection** to isolate the culprit in a failing batch.

Per-test history is collected automatically — run `oryn setup` once to enable
nextest's JUnit output.

## The terminal UI

`oryn tui` is a live dashboard:

- **Overview** — counts, selection reason, and a cache-hit gauge.
- **Selection** — what your current diff affects (changed / affected / skipped).
- **Crates** — fingerprints, cache state, recorded test counts and time.
- **Flaky** — Wilson + Bayesian intervals visualized as bars, with rerun budgets.

Navigate with `1–5`/arrows/`jk`, `r` to refresh, `q` to quit.

## Commands

```bash
oryn affected [--since <ref>]                 # what a change affects (safe selection)
oryn test     [--since <ref>] [--all] [--no-cache] [--cache]   # run only affected, skip cached-green
oryn build    [--since <ref>] [--all] [--cache]                # build only affected crates
oryn tui      [--since <ref>]                 # terminal dashboard
oryn flaky    [--input runs.jsonl] [--json]   # Wilson + Bayes + rerun budget
oryn budget   --fail-rate 0.01 --confidence 0.95     # -> "run each test 299 times"
oryn setup                                    # enable per-test history (nextest JUnit)
oryn tune                                     # detect & configure compile-time speedups
oryn cache                                    # sccache hit/miss stats
oryn info                                     # versions + detected tooling
```

`oryn test` exits non-zero when the selected tests fail, so it drops into CI as a
faster, safe replacement for `cargo test` (see
[`.github/workflows/selective-tests.yml`](.github/workflows/selective-tests.yml)).
For PRs, use `--since origin/main`.

## Workspace

| Crate | What it is |
|-------|------------|
| [`oryn-core`](crates/oryn-core) | The engine. Selection (`graph`, `metadata`, `git`, `select`), sound result caching (`fingerprint`, `store`, `runner`), test collection (`junit`), the statistical framework (`stats` Wilson/bootstrap, `bayes` Beta-Binomial, `flaky`, `prioritize`, `bisect`), and the render-agnostic `dashboard`. Pure, deterministic, exhaustively unit-tested. |
| [`oryn-cli`](crates/oryn-cli) | The `oryn` binary — orchestration over cargo/nextest/git/sccache, and the ratatui `tui`. |

## Soundness notes

- The green-cache fingerprint captures **source closure + `rustc` version**. Tests
  depending on un-captured runtime state (network, wall-clock, ambient env) are
  not perfectly hermetic; the flaky subsystem surfaces such tests, and
  `--no-cache` forces a full re-run.
- Crate-level selection is the safe default.

## Roadmap

- **Function-level (MIR) selection** for finer skips — this requires nightly
  rustc internals (the approach RustyRTS uses) and is deliberately *not* faked
  here; crate-level is the safe, stable default today.
- **Content-addressed build cache** with early-cutoff + hermeticity checks, as an
  alternative to sccache — only if it can be made provably sound.
- **Merge-queue batching** built on the `bisect` primitive.

## License

MIT — see [LICENSE](LICENSE).
