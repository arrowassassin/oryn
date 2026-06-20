# Benchmarks

`bench/run.sh` benchmarks `oryn` against a real Cargo workspace and prints a
results table.

```bash
# clone ripgrep and benchmark it (default)
bench/run.sh

# benchmark your own workspace (must be a clean git repo)
bench/run.sh /path/to/workspace

# also measure parallel coverage + function-level selection (slow)
WITH_FN=1 bench/run.sh /path/to/workspace
```

It builds `target/release/oryn` if needed, refuses a dirty target tree, and only
edits-then-reverts tracked files. Results are also written to `$BENCH_OUT`
(a temp file by default).

## What it measures

| Section | What |
|---|---|
| **[A]** | Warm test loop, no change: `oryn test --all` (sound cache-skip) vs `cargo test --workspace` (full re-run). The headline. |
| **[B]** | Fingerprint + selection cost on every `oryn` invocation (parallel hashing of the whole closure). |
| **[C]** | Crate-level selection after editing one leaf file (how many of N crates run). |
| **[D]** | Incremental rebuild: default debuginfo vs `debug = "line-tables-only"` (the `oryn tune` win). |
| **[E]** (`WITH_FN=1`) | Parallel `oryn cover` time, then `oryn test --fn` selection after a one-function edit. |

## In CI

`.github/workflows/benchmark.yml` runs this on demand (Actions → *benchmark* →
*Run workflow*) against any repo URL, with `mold`, `clang`, and
`llvm-tools-preview` installed so the linker and coverage levers are measurable.
Results land in the run summary and a `bench-results` artifact.

## Reference numbers (ripgrep, 10 crates, 1,139 tests, 4-core CI-class box)

- **[A]** `oryn test --all` ~0.06s vs `cargo test --workspace` ~6.1s → **~95×** on the warm loop.
- **[B]** ~0.03s for the full 10-crate fingerprint + selection.
- **[C]** editing one leaf crate selects only it + its direct dependents.
- **[E]** parallel `oryn cover` ~2.5 min (a serial pass does not finish in 6);
  one-function edit selects ~250 of 1,139 tests, soundly (including dependent
  crates' tests that execute the changed code).

Absolute seconds depend on the machine and repo; the **ratios** are the point.
