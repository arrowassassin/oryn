# Example: a small LLM evaluation flow

A complete, runnable walkthrough of Oryn on a tiny scenario: you want to replace
**model A** with **model B** on a 12-question QA benchmark, and you suspect some
questions leaked from the training data. Every number below is produced by the
`oryn` CLI from the files in this directory — no model judges another model.

## Files

| File | What it is |
|------|------------|
| `benchmark.jsonl` | 12 eval questions (`{id, text}`) |
| `training_corpus.jsonl` | a snapshot of the model's training data (`q3` is present verbatim; `q7` appears paraphrased) |
| `model_a.jsonl` / `model_b.jsonl` | grader scores (`{id, score}`, 0/1) for each model on the same ids |
| `determinism_probe.json` | the same prompt sent 5× at temperature 0 |

## Run it

```bash
cargo build -p oryn-cli
BIN=./target/debug/oryn

# 1) Is the benchmark contaminated by the training corpus?
$BIN scan --corpus examples/flow/training_corpus.jsonl \
          --eval   examples/flow/benchmark.jsonl --ngram 4

# 2) Eval each model with confidence intervals + required-N.
$BIN eval --run examples/flow/model_a.jsonl --name model_a
$BIN eval --run examples/flow/model_b.jsonl --name model_b

# 3) Paired regression gate (exits non-zero, blocks CI, if B regressed).
$BIN gate --baseline examples/flow/model_a.jsonl \
          --candidate examples/flow/model_b.jsonl

# 4) Is inference even reproducible?
$BIN determinism --runs examples/flow/determinism_probe.json
```

## What you get (and the math behind it)

**Step 1 — contamination.** `q3` is verbatim in the corpus → every 4-gram is in
the corpus set (`overlap = 1.00`) and MinHash `jaccard = 1.00` → **flagged**,
closest source `web-2`. `q7` is a paraphrase: it shares ~33% of exact 4-grams but
the MinHash Jaccard stays below the 0.8 threshold, so it is *not* flagged — a real
limitation of strict thresholds on short text (lower `--jaccard-threshold` to
catch softer paraphrases). Output includes the **clean held-out split** of the 11
non-leaked items — the set you should actually report on.

```
12 items, 1 contaminated (8.3%), 11 clean held out
```

**Step 2 — eval with error bars.** A = 10/12 = 0.83, B = 5/12 = 0.42. The `±` is
the Wilson interval half-width (`SE = sqrt(p(1-p)/n)`). The CIs barely overlap,
and `required-N for d=0.20: 197` warns that 12 items is tiny.

```
model_a: mean=0.8333 ±0.2005  (95% CI [0.5520, 0.9530])
model_b: mean=0.4167 ±0.2436  (95% CI [0.1933, 0.6805])
```

**Step 3 — regression gate.** Paired per-question differences `d = B - A`, mean
`Δ = -0.42`, `z = Δ/SE(d)`, two-sided `p = 0.0051 < 0.05` → significant
regression, the gate **blocks** (exit code 2).

```
Regressed (Δ=-0.4167, p=0.0051, n=12) — BLOCKED
```

**Step 4 — determinism.** The same prompt gave "…mars" 4× and "…venus" 1×:
2 distinct outputs (BLAKE3 fingerprints), first divergence at token 4.

```
2/5 unique outputs (NONDETERMINISTIC); first divergence at token 4
```

## Verdict

Don't ship B (significant regression), re-score on the clean 11-item split (q3
leaked), and note the model is nondeterministic so any single run is partly luck.
This is string-set math (step 1) + classical statistics (steps 2–3) + hashing
(step 4) — all deterministic, no model in the loop.
