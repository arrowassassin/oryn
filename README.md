# Oryn

**The reproducibility & evaluation-integrity layer for AI — a non-AI tool that
makes AI results you can reproduce, trust, and audit.**

There is **no model in the loop anywhere in Oryn**. It is classical computer
science — n-gram/MinHash matching, statistics, floating-point numerics, BLAKE3 +
Ed25519 — applied to the one question every AI generator dodges: *is this result
real, or is it noise, nondeterminism, or benchmark contamination?*

Three structural problems Oryn answers deterministically:

1. **Evals don't replicate.** The same model on the same benchmark is reported
   with wildly different scores; most evals ship with no error bars at all.
   Oryn attaches confidence intervals, statistical power, and a paired
   **regression gate**.
2. **Benchmarks are contaminated.** Test items leak into training corpora and
   inflate scores. Oryn scans for verbatim (n-gram) and near-duplicate
   (MinHash/LSH) contamination and emits a **clean held-out split**.
3. **Inference isn't deterministic.** At temperature 0, batch size / tiling
   changes the floating-point reduction order and flips tokens. Oryn analyzes
   repeated generations and ships **batch-invariant kernels** (real CUDA, with a
   tested CPU reference) that make the reduction order — and the output —
   reproducible.

Every report can be sealed into a tamper-evident, **Ed25519-signed hash chain**
for audit (EU AI Act Art. 12/19 record-keeping).

## Workspace

| Crate | What it is |
|-------|------------|
| [`oryn-core`](crates/oryn-core) | The engine: contamination scanning, statistically-rigorous evals + regression gate, determinism analysis, signed attestations, and the aggregate integrity report. Deterministic; no I/O hidden inside. |
| [`oryn-cuda`](crates/oryn-cuda) | Batch-invariant numerical kernels. Real CUDA (`kernels/batch_invariant.cu`) compiled and linked when the `cuda` feature is set and `nvcc` is present; otherwise a **tested CPU reference** with identical semantics. |
| [`oryn-server`](crates/oryn-server) | A UI-agnostic JSON HTTP API over the engine (axum) — a library, served via `oryn serve`, so any frontend can drive it. |
| [`oryn-cli`](crates/oryn-cli) | The `oryn` command line — the single binary: `scan`, `eval`, `gate`, `determinism`, `keygen`, `attest`, `serve`, `info`. |

## Quick start

```bash
# Build everything (CPU path; no GPU required).
cargo build --release

# Contamination-scan an eval set against a corpus.
oryn scan --corpus examples/corpus.jsonl --eval examples/eval.jsonl --ngram 6

# Eval with error bars + required sample size.
oryn eval --run examples/baseline.jsonl

# Paired regression gate (exits non-zero if the candidate regressed).
oryn gate --baseline examples/baseline.jsonl --candidate examples/candidate.jsonl

# Determinism analysis of repeated generations.
oryn determinism --runs examples/generations.json

# Run the HTTP API for a UI to call.
oryn serve --addr 127.0.0.1:8787
```

### Data formats

* **Documents** (`scan`): JSONL or a JSON array of `{"id","text"}`.
* **Eval runs** (`eval`, `gate`): JSONL or a JSON array of `{"id","score"}`
  (use `0`/`1` for accuracy-style scores; any real value works).
* **Generations** (`determinism`): a JSON array of strings, or one per line.

## HTTP API

`oryn serve` exposes:

| Method | Path | Body → Response |
|--------|------|-----------------|
| GET | `/api/health` | → `{status}` |
| GET | `/api/info` | → versions + compute backend |
| POST | `/api/scan` | `{corpus, eval, config?}` → contamination report |
| POST | `/api/duplicates` | `{docs, config?}` → intra-set near-duplicates |
| POST | `/api/eval` | `{run, config?}` → eval report (CI, power) |
| POST | `/api/gate` | `{baseline, candidate, level?}` → regression gate |
| POST | `/api/determinism` | `{outputs}` → determinism report |
| POST | `/api/integrity` | integrity report → `{verdict, report}` |
| POST | `/api/keygen` | → `{secret_hex, public_hex}` |
| POST | `/api/attest/seal` | `{seed_hex?, entries}` → signed chain |
| POST | `/api/attest/verify` | signed chain → `{ok, entries, error?}` |

CORS is permissive so a browser UI can call it directly. A dedicated UI is
designed separately and built against this API.

## Building the CUDA kernels

```bash
# Requires nvcc on PATH and a CUDA-capable GPU at runtime.
cargo build --release -p oryn-cuda --features cuda
oryn info   # compute backend will report "cuda"
```

Without `nvcc`, the build prints a warning and uses the CPU reference path; the
public API and results are identical (the reference mirrors the kernels'
fixed-reduction-order semantics).

## Design principles

* **Deterministic by construction** — same input, same bytes out, on any
  machine. Hashing is fixed-seeded; the bootstrap is seeded; map iteration is
  sorted before output.
* **No model in the loop** — verification you can trust must not reintroduce the
  failure mode (a hallucinating judge) it is trying to catch.
* **Auditable** — every result can be signed and chained.

## License

MIT — see [LICENSE](LICENSE).
