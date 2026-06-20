#!/usr/bin/env bash
# Benchmark `oryn` against a real Cargo workspace.
#
#   bench/run.sh [TARGET_REPO_DIR]
#
# With no argument it clones ripgrep into a temp dir. Pass a path to benchmark
# your own workspace (it must be a git repo). Set WITH_FN=1 to also benchmark
# `oryn cover` + `oryn test --fn` (records per-test coverage — minutes on a big
# repo). Results are printed and written to bench-results.txt in the cwd.
#
# Only tracked files are edited-then-reverted (`git checkout --`); a dirty
# target tree is refused so nothing of yours is lost.
set -euo pipefail

RUNS="${RUNS:-5}"
# The cheap oryn-side measurements (cached test, fingerprint) run RUNS times.
# The expensive `cargo test --workspace` baseline re-runs the FULL suite each
# time, so it gets its own, smaller count — on a heavy async suite (tokio) 5×
# full runs is minutes of CI for no extra signal. Override with BASELINE_RUNS.
BASELINE_RUNS="${BASELINE_RUNS:-2}"
WITH_FN="${WITH_FN:-0}"

# --- locate the oryn binary (build it if needed) ---------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ORYN_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ORYN="$ORYN_ROOT/target/release/oryn"
if [ ! -x "$ORYN" ]; then
  echo "building oryn (release)…" >&2
  ( cd "$ORYN_ROOT" && cargo build --release -p oryn >/dev/null )
fi

# --- resolve the target workspace ------------------------------------------
TARGET="${1:-}"
CLONED=""
if [ -z "$TARGET" ]; then
  TARGET="$(mktemp -d)/ripgrep"; CLONED="$TARGET"
  echo "cloning ripgrep into $TARGET …" >&2
  git clone --depth 1 https://github.com/BurntSushi/ripgrep "$TARGET" >/dev/null 2>&1
fi
TARGET="$(cd "$TARGET" && pwd)"
cd "$TARGET"

if [ -n "$(git status --porcelain 2>/dev/null)" ]; then
  echo "error: target tree is dirty — commit/stash first (the bench edits & reverts tracked files)." >&2
  exit 1
fi

# Write results OUTSIDE the target tree so the bench doesn't dirty it (which
# would make [C]'s selection count the root crate).
OUT="${BENCH_OUT:-$(mktemp -t oryn-bench-XXXXXX.txt)}"; : > "$OUT"
log() { echo "$@" | tee -a "$OUT"; }

# best-of-N + mean wall-clock, using date+awk (no bc dependency)
bench() { # name runs -- cmd...
  local name="$1" runs="$2"; shift 3
  local times=() t0 t1
  for _ in $(seq "$runs"); do
    t0=$(date +%s.%N); "$@" >/dev/null 2>&1 || true; t1=$(date +%s.%N)
    times+=("$(awk "BEGIN{print $t1-$t0}")")
  done
  awk -v n="$name" 'BEGIN{min=1e9;sum=0}
    {if($1<min)min=$1; sum+=$1; c++}
    END{printf "  %-44s min=%7.3fs mean=%7.3fs (n=%d)\n", n, min, sum/c, c}' \
    <(printf '%s\n' "${times[@]}") | tee -a "$OUT"
}

cleanup() { git checkout -- . >/dev/null 2>&1 || true; [ -n "$CLONED" ] && rm -rf "$(dirname "$CLONED")" || true; }
trap cleanup EXIT

MEMBERS=$(cargo metadata --no-deps --format-version 1 2>/dev/null \
  | python3 -c "import sys,json;print(len(json.load(sys.stdin)['packages']))" 2>/dev/null || echo '?')
log "=============================================================="
log " oryn benchmark — $(date -u +%FT%TZ)"
log " target:  $TARGET"
log " ~workspace members: $MEMBERS"
log " host:    $(nproc) cores, rustc $(rustc -V | awk '{print $2}')"
log " linkers: mold=$(command -v mold>/dev/null&&echo yes||echo no) sccache=$(command -v sccache>/dev/null&&echo yes||echo no)"
log "=============================================================="

# warm the compile so it isn't in the test loop
cargo build --workspace >/dev/null 2>&1 || true

log ""
log "[A] Warm test loop, NO change — sound cache-skip vs full re-run"
"$ORYN" test --all >/dev/null 2>&1 || true   # seed green
bench "oryn test --all (all crates cached green)" "$RUNS" -- "$ORYN" test --all
bench "cargo test --workspace (re-runs all)"      "$BASELINE_RUNS" -- cargo test --workspace

log ""
log "[B] Fingerprint + selection cost (runs on every oryn invocation)"
bench "oryn affected (parallel fingerprint+select)" "$RUNS" -- "$ORYN" affected

log ""
log "[C] Crate-level selection after editing ONE leaf file"
LEAF=$(git ls-files '*/src/lib.rs' 'src/lib.rs' | head -1)
if [ -n "$LEAF" ]; then
  printf '\n// oryn-bench %s\n' "$(date +%s)" >> "$LEAF"
  SEL=$("$ORYN" affected 2>&1 | grep -iE "changed|affected|test" | head -1)
  log "  edited: $LEAF"
  log "  oryn selects: ${SEL:-<none>}"
  git checkout -- "$LEAF"
else
  log "  (no lib.rs found to edit — skipped)"
fi

log ""
log "[D] Compile: incremental rebuild, default vs line-tables-only (the tune win)"
TOUCH="$LEAF"
if [ -n "$TOUCH" ]; then
  cargo build --workspace >/dev/null 2>&1 || true
  bench "cargo build incr. (default debuginfo)" 3 -- \
    bash -c "printf '\n//b\n' >> '$TOUCH'; cargo build --workspace; git checkout -- '$TOUCH'"
  export CARGO_PROFILE_DEV_DEBUG=line-tables-only
  cargo build --workspace >/dev/null 2>&1 || true
  bench "cargo build incr. (line-tables-only)" 3 -- \
    bash -c "printf '\n//b\n' >> '$TOUCH'; cargo build --workspace; git checkout -- '$TOUCH'"
  unset CARGO_PROFILE_DEV_DEBUG
fi

if [ "$WITH_FN" = "1" ]; then
  log ""
  log "[E] Function-level: parallel coverage + --fn selection"
  git checkout -- . >/dev/null 2>&1 || true
  t0=$(date +%s.%N); "$ORYN" cover >/dev/null 2>&1 || true; t1=$(date +%s.%N)
  log "  $(awk "BEGIN{printf \"oryn cover (parallel): %.1fs\", $t1-$t0}")"
  if [ -n "$LEAF" ]; then
    python3 - "$LEAF" <<'PY' 2>/dev/null || printf '\n//b\n' >> "$LEAF"
import sys
p=sys.argv[1]; s=open(p).read(); i=s.find("{", s.find("fn "))
open(p,"w").write(s[:i+1]+"\n    let _oryn_bench=1;"+s[i+1:] if i>0 else s+"\n//b\n")
PY
    log "  --fn selection after editing one function in $LEAF:"
    "$ORYN" test --fn 2>&1 | grep -E "oryn fn:" | sed 's/^/    /' | tee -a "$OUT"
    git checkout -- "$LEAF"
  fi
fi

log ""
log "results written to $OUT"
