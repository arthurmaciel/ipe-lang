#!/usr/bin/env bash
# scripts/sky-compare.sh — LIVE upstream-Sky parity comparison.
#
# For each named cli example it runs the SAME upstream example two ways and
# compares the program's stdout:
#   • upstream reference — the released `sky run` on the raw upstream .sky tree
#     (installed by scripts/lib/sky-upstream.sh; Sky lowers to Go, so Go must be
#     on PATH).
#   • ipe — `ipe build` + run the mirrored+transformed tree.
#
# This is the LIVE reference the retired cached `expected_go.txt` oracle used to
# stand in for: a behavioural divergence surfaces against the current upstream
# compiler, not a frozen snapshot. It is deliberately SCOPED to deterministic
# `cli` examples (stdout is the whole observable behaviour); server/live/tui/
# webview shapes have no single comparable stdout and are out of scope here.
#
# Sky prints its build log to the SAME stdout as the program, with a stable set
# of prefixes; `_strip_sky_log` removes them so only program output remains.
#
# Exit: 0 = every compared example matched · 1 = a divergence · 2 = setup/skip
# (Sky or Go unavailable — a degraded environment, not a parity failure).
set -uo pipefail

source "$(dirname "$0")/lib/env.sh"
source "$(dirname "$0")/lib/mirror.sh"
source "$(dirname "$0")/lib/sky-upstream.sh"

# Deterministic-stdout cli examples. Kept explicit (not derived): the comparison
# is only meaningful where the whole observable behaviour is a stable stdout.
SKY_COMPARE_EXAMPLES="${SKY_COMPARE_EXAMPLES:-01-hello-world}"

# ── _strip_sky_log <file>: drop Sky's build-log lines, keep program output ────
# Sky interleaves its build log with program stdout. The log lines have a stable
# shape: a `-- ` section header, a 3-space-indented detail, or one of a small set
# of fixed status lines. Everything else is the program's own output.
_strip_sky_log() {
  rg -v '^-- |^   |^Running go build\.\.\.$|^Compilation successful$|^Build complete, running\.\.\.$|^Sky lowering succeeded$|^\[DCE\]' "$1" 2>/dev/null
}

need_bin() { [ -x "$IPE_BIN" ] || { echo "ERROR: ipe binary not at '$IPE_BIN' (cargo build --release -p ipe)"; exit 2; }; }

main() {
  cd "$REPO"
  need_bin
  export SKY_SWEEP_COMPARE=1   # tell the mirror to preserve the raw .sky tree

  local sky_bin
  if ! sky_bin="$(sky_upstream_bin)"; then
    echo "SKIP: upstream Sky binary unavailable on this host — live comparison not run (not a parity failure)."
    exit 2
  fi
  if ! command -v go >/dev/null 2>&1; then
    echo "SKIP: Go not on PATH — \`sky run\` lowers to Go, cannot run the reference (not a parity failure)."
    exit 2
  fi
  echo "Using upstream Sky: $sky_bin ($("$sky_bin" --version 2>/dev/null))"

  local workroot; workroot="$(mktemp -d "${TMPDIR:-/tmp}/ipe-sky-compare.XXXXXX")"
  trap 'rm -rf "$workroot"' EXIT
  local fails=0 ran=0 name

  for name in $SKY_COMPARE_EXAMPLES; do
    local dst="$workroot/$name"
    if ! sky_mirror_one "$name" "$dst"; then
      echo "  $name: SKIP (no upstream source)"; continue
    fi
    [ -d "$dst/.sky-src" ] || { echo "  $name: SKIP (raw .sky tree not preserved)"; continue; }

    # Reference: sky run on the RAW upstream tree.
    local sky_out="$workroot/$name.sky.out"
    if ! sky_run_capture "$dst/.sky-src" "$sky_out"; then
      echo "  $name: SKIP (sky run failed — see below)"; sed 's/^/      sky: /' "$sky_out" | head -8; continue
    fi

    # ipe: build + run the transformed tree.
    local tgt ipe_out="$workroot/$name.ipe.out"
    tgt="ipe.toml"; [ -f "$dst/ipe.toml" ] || tgt="src/Main.ipe"
    if ! ( cd "$dst" && timeout 600 "$IPE_BIN" build "$tgt" --out out/rust ) >"$workroot/$name.ipebuild.log" 2>&1; then
      echo "  $name: FAIL (ipe build)"; sed 's/^/      ipe: /' "$workroot/$name.ipebuild.log" | tail -8; fails=$((fails+1)); continue
    fi
    if ! ( cd "$dst/out/rust" && timeout 600 cargo run -q ) >"$ipe_out" 2>&1; then
      echo "  $name: FAIL (ipe run)"; sed 's/^/      ipe: /' "$ipe_out" | tail -8; fails=$((fails+1)); continue
    fi

    # Compare program output (Sky log stripped from the reference).
    local sky_prog="$workroot/$name.sky.prog" ipe_prog="$ipe_out"
    _strip_sky_log "$sky_out" >"$sky_prog"
    ran=$((ran+1))
    if diff -u "$sky_prog" "$ipe_prog" >"$workroot/$name.diff" 2>&1; then
      echo "  $name: OK (sky == ipe): $(tr '\n' '|' <"$sky_prog")"
    else
      echo "  $name: DIVERGENCE (sky vs ipe):"; sed 's/^/      /' "$workroot/$name.diff" | head -20
      fails=$((fails+1))
    fi
  done

  echo ""
  echo "=== sky-compare: $ran compared, $fails divergence(s) ==="
  [ "$ran" -eq 0 ] && { echo "SKIP: no example compared."; exit 2; }
  [ "$fails" -gt 0 ] && exit 1
  exit 0
}

main "$@"
