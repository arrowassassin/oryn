# Oryn

**Compile less, test less, trust the results.** A safe, Rust-native developer
tool that makes the Cargo edit→build→test loop dramatically faster — *correctly* —
and scores flaky tests with real statistics. The parts hyperscalers built
internally and only paid tools (Gradle Develocity, CloudBees) sell, packaged free
for any Cargo project.

No model in the loop — classical, deterministic computer science, grounded in
current research (RustyRTS ICST 2025; Gruber et al. ICST 2021; *Build Systems à
la Carte*; Meta predictive test selection).

```text
$ oryn test          # run #1 — runs the affected crates, records them green
$ oryn test          # run #2 — nothing changed → all cached green, 0.05s ✓
$ # edit one crate
$ oryn test          # runs only that crate; everything else stays cached
```

## How it gets fast — without being wrong

Speed and correctness pull against each other, so Oryn is deliberate about each:

1. **Test only what changed (safe selection).** From a git diff, Oryn tests the
   changed crates **plus every crate that transitively depends on them**, and
   skips the rest. Safe *by construction* at crate granularity — a crate is
   Cargo's unit of compilation, so its tests can only change if its own sources
   or a (transitive) dependency changed. Conservative: it may over-select, never
   under-select. A docs-only change tests nothing; a `Cargo.lock`/toolchain
   change forces a full run.

2. **Don't re-run known-green tests (sound result cache).** Oryn computes a
   **Merkle fingerprint** of each crate's entire dependency-closure of sources
   plus the exact `rustc` version. If a crate's fingerprint matches a recorded
   green run, its tests *cannot* have a different outcome, so they're skipped.
   This is what makes a warm `oryn test` return in milliseconds.

3. **Stand on a correct compile cache, don't reinvent it.** A *subtly wrong*
   compile cache silently miscompiles — the worst bug there is. The conditions
   for a sound shared cache (pinned toolchain, full-flag keying, path remapping,
   refusing un-captured inputs) are exactly what `sccache` already does, so
   `oryn tune` wires it up rather than risk a buggy reimplementation.

4. **Auto-apply the proven fast path.** `oryn tune` detects and configures the
   wins most devs never enable: fast linker (note: `rust-lld` is already the
   default on x86_64-linux since Rust 1.90), `sccache`, dependency optimization,
   and `split-debuginfo`.

## Trust the results — real statistics

Every runner labels a test "flaky" from a naive 2–3 rerun rule. That's
statistically wrong: a test that fails 1% of the time needs **~300 reruns** to be
seen failing once with 95% confidence. Oryn instead:

- estimates each test's **flake rate with both a frequentist Wilson interval and
  a Bayesian (Jeffreys-prior) credible interval**,
- tells you the **rerun budget** the statistics actually require
  (`n ≥ ln(1−γ)/ln(1−p)`),
- and tells you what a clean run *doesn't* prove ("20 passes only proves the
  flake rate is below 16%").

It builds this from per-test history collected automatically (run `oryn setup`
once to enable nextest's JUnit output), and orders tests **fail-fast**
(recently-failed first, then fastest) — the heuristic that beats ML in the
literature (Cheng et al., ISSTA 2024).

## Commands

```bash
oryn affected [--since <ref>]      # what a change can affect (safe selection)
oryn test     [--since <ref>] [--all] [--no-cache]   # run only affected, skip cached-green
oryn build    [--since <ref>] [--all]                # build only affected crates
oryn flaky    [--input runs.jsonl] [--json]          # flaky scoring (Wilson + Bayes + rerun budget)
oryn budget   --fail-rate 0.01 --confidence 0.95     # -> "run each test 299 times"
oryn setup                          # enable per-test history (nextest JUnit profile)
oryn tune                           # detect & configure compile-time speedups
oryn info                           # versions + detected tooling
```

`oryn test` exits non-zero when the selected tests fail, so it drops into CI as a
faster, safe replacement for `cargo test`. For PRs, use `--since origin/main`.

## Workspace

| Crate | What it is |
|-------|------------|
| [`oryn-core`](crates/oryn-core) | The engine. Selection (`graph`, `metadata`, `git`, `select`), sound result caching (`fingerprint`, `store`, `runner`), test collection (`junit`), and the statistical framework (`stats` Wilson/bootstrap, `bayes` Beta-Binomial, `flaky`, `prioritize`). Pure, deterministic, exhaustively unit-tested. |
| [`oryn-cli`](crates/oryn-cli) | The `oryn` binary — orchestration over cargo/nextest/git. |

## Soundness notes

- The green cache fingerprint captures **source closure + `rustc` version**. Tests
  that depend on un-captured runtime state (network, wall-clock, ambient env) are
  not perfectly hermetic; such tests should be quarantined — the flaky subsystem
  surfaces them, and `--no-cache` forces a full re-run.
- Crate-level selection is the safe default. Function-level (MIR) selection,
  batching/bisection, and a content-addressed build cache are the roadmap.

## License

MIT — see [LICENSE](LICENSE).
