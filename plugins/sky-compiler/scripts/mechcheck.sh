#!/usr/bin/env bash
# mechcheck — the deterministic soundness gate for Sky-compiler / Rust-backend /
# Rust-runtime development. Runs the mechanical checks FAIL-FAST (stops at the first
# failure), tees each step to a log, and prints the failing step + log path so a
# cheap (Sonnet) fix agent can be dispatched against it. All-green => exit 0, then
# the caller runs the expensive (Opus) adversarial guardian review.
#
# Usage:  mechcheck.sh [WORKSPACE_DIR] [--miri] [--e2e] [--parity] [--all]
#   WORKSPACE_DIR  cargo workspace to check (default: $PWD) — works for sky-rust
#                  (compiler+backend) and runtime-rust alike.
#   --miri    also run `cargo +nightly miri test --workspace` (slow)
#   --e2e     also run the golden E2E (IPE_E2E=1) — sky-rust only; runs BOUNDED
#             (nested cargo builds are CPU/mem/disk heavy)
#   --parity  also run ./scripts/parity-sweep.sh if present (Go-vs-Rust) — sky-rust only
#   --fmt-fix APPLY `cargo fmt --all` instead of `--check` — for the dev/agent loop
#             so formatting is fixed mechanically, never a gate failure a fix agent
#             must burn tokens on. CI omits it (keeps --check to enforce formatting).
#   --all     --miri --e2e --parity
#
# Test runner: uses `cargo nextest run` (global cross-binary parallel pool) when
# installed, else falls back to `cargo test`. The E2E step is thread-capped because
# each E2E test shells out to its own `cargo build` (oversubscription/ENOSPC risk).
#
# Exit codes: 0 = all green; 1 = a check failed (see printed log); 2 = bad invocation.
set -uo pipefail

WS="${PWD}"
case "${1:-}" in ""|-*) : ;; *) WS="$1"; shift ;; esac
RUN_MIRI=0; RUN_E2E=0; RUN_PARITY=0; FMT_FIX=0
for a in "$@"; do
  case "$a" in
    --miri) RUN_MIRI=1 ;;
    --e2e) RUN_E2E=1 ;;
    --parity) RUN_PARITY=1 ;;
    --fmt-fix) FMT_FIX=1 ;;
    --all) RUN_MIRI=1; RUN_E2E=1; RUN_PARITY=1 ;;
    *) echo "mechcheck: unknown flag '$a'"; exit 2 ;;
  esac
done

cd "$WS" 2>/dev/null || { echo "mechcheck: no such workspace: $WS"; exit 2; }
LOGDIR="${TMPDIR:-/tmp}/mechcheck"; mkdir -p "$LOGDIR"

# Disk guard before heavy builds (CLAUDE.md ENOSPC trap): reclaim if <15 GB free.
free_gb="$(df -P / | awk 'NR==2 { print int($4 / 1024 / 1024) }')"
if [ "${free_gb:-99}" -lt 15 ]; then
  echo "mechcheck: disk low (${free_gb}G) — reclaiming go-build + cargo temp"
  rm -rf /tmp/go-build* /tmp/skyc_* 2>/dev/null
  command -v go >/dev/null && go clean -cache 2>/dev/null
fi

# Cross-binary parallel test runner if available.
HAS_NEXTEST=0
command -v cargo-nextest >/dev/null 2>&1 && HAS_NEXTEST=1

# step NAME CMD...  — run a check, fail-fast with the log on failure.
step() {
  local name="$1"; shift
  local log="$LOGDIR/$name.log"
  printf '→ %-8s ' "$name"
  if "$@" >"$log" 2>&1; then
    echo "ok"
  else
    echo "FAILED"
    echo "mechcheck: '$name' failed in $WS"
    echo "  log: $log"
    echo "  --- tail ---"
    tail -n 25 "$log"
    echo "mechcheck: FAIL-FAST. Dispatch a Sonnet fix against the log above, then re-run mechcheck."
    exit 1
  fi
}

echo "mechcheck: $WS  (nextest=$HAS_NEXTEST, ${free_gb}G free)"
# Formatting is mechanical + idempotent: with --fmt-fix (dev/agent loop) APPLY it
# so it never becomes a gate failure a fix agent must waste tokens on; the default
# (--check, for CI) enforces that contributors formatted.
if [ "$FMT_FIX" = 1 ]; then
  step fmt cargo fmt --all
else
  step fmt cargo fmt --all -- --check
fi
step build  cargo build --workspace --all-targets
step clippy cargo clippy --all-targets -- -D warnings

# Fast unit/integration tests — wide parallelism (nextest global pool when present).
if [ "$HAS_NEXTEST" = 1 ]; then
  step test cargo nextest run --workspace
  step doctest cargo test --doc --workspace   # nextest does not run doctests
else
  step test cargo test --workspace
fi

[ "$RUN_MIRI" = 1 ] && step miri cargo +nightly miri test --workspace

# E2E: each test shells out to its own `cargo build` — cap concurrency.
if [ "$RUN_E2E" = 1 ]; then
  export IPE_E2E=1
  if [ "$HAS_NEXTEST" = 1 ]; then
    step e2e cargo nextest run --workspace --test-threads 2
  else
    step e2e cargo test --workspace   # cargo runs test binaries serially => inherently bounded
  fi
  unset IPE_E2E
fi

if [ "$RUN_PARITY" = 1 ]; then
  if [ -x ./scripts/parity-sweep.sh ]; then
    step parity ./scripts/parity-sweep.sh
  else
    echo "→ parity   skipped (no ./scripts/parity-sweep.sh in $WS)"
  fi
fi

echo "mechcheck: ALL GREEN ($WS)"
exit 0
