#!/usr/bin/env bash
# tools/scripts/check-sky-parity.sh — behavior-parity increment 1.
#
# For each green, run-verify, program/console port (not go_ffi), runs the
# upstream "sky run" and the built "ipe" binary side by side, then compares
# stdout and exit code per the port's parity policy from manifest.toml:
#
#   exact      — byte-identical stdout + exit code required
#   normalized — stdout stripped of known nondeterministic tokens before compare
#   skip       — output is intrinsically nondeterministic; printed with reason
#
# Requires a "sky" binary on PATH or pointed to by SKY_BIN. Install with:
#   tools/scripts/install-sky-toolchain.sh <version>
#
# Usage:
#   check-sky-parity.sh [--sky-bin PATH] [--names name,...] [--diff-lines N]
#
# Exit: 0 all compared ports matched  1 one or more mismatches  2 setup error
set -uo pipefail

source "$(dirname "$0")/lib/env.sh"
source "$(dirname "$0")/lib/checks.sh"

cd "$REPO" || { echo "check-sky-parity: cannot locate repo" >&2; exit 2; }

# ── Disk guard ───────────────────────────────────────────────────────────────
FREE_KB="$(df -Pk "$REPO" 2>/dev/null | awk 'NR==2{print $4}')"
if [ -n "$FREE_KB" ] && [ "$FREE_KB" -lt 5242880 ]; then
  echo "check-sky-parity: < 5G free ($((FREE_KB/1024/1024))G) — aborting." >&2; exit 2
fi

# ── Argument parsing ─────────────────────────────────────────────────────────
SKY_BIN="${SKY_BIN:-sky}"
DIFF_LINES=40
FILTER_NAMES=""

while [ $# -gt 0 ]; do
  case "$1" in
    --sky-bin)    SKY_BIN="$2";      shift 2 ;;
    --names)      FILTER_NAMES="$2"; shift 2 ;;
    --diff-lines) DIFF_LINES="$2";   shift 2 ;;
    *) echo "check-sky-parity: unknown option: $1" >&2; exit 2 ;;
  esac
done

# ── Pre-flight checks ────────────────────────────────────────────────────────
if ! command -v "$SKY_BIN" >/dev/null 2>&1; then
  echo "check-sky-parity: sky binary not found ('$SKY_BIN')" >&2
  echo "  Install with: tools/scripts/install-sky-toolchain.sh <version>" >&2
  echo "  Or set SKY_BIN to a sky binary path." >&2
  exit 2
fi

if [ ! -x "$IPE_BIN" ]; then
  echo "check-sky-parity: ipe binary not found at '$IPE_BIN'" >&2
  echo "  Build it: cargo build --release -p ipe" >&2
  exit 2
fi

SKY_VER="$("$SKY_BIN" --version 2>/dev/null || true)"
IPE_VER="$("$IPE_BIN"  --version 2>/dev/null || true)"
echo "=== check-sky-parity ==="
echo "  sky: ${SKY_VER:-unknown}"
echo "  ipe: ${IPE_VER:-unknown}"
echo ""

# ── Manifest helpers ─────────────────────────────────────────────────────────
_field() {
  local name="$1" field="$2"
  python3 - "$name" "$field" "$REPO/examples/sky/manifest.toml" <<'PYEOF'
import re, sys
name, field, path = sys.argv[1], sys.argv[2], sys.argv[3]
with open(path) as f:
    content = f.read()
for block in re.split(r'\[\[example\]\]', content):
    nm = re.search(r'name\s*=\s*["\']([^"\']+)["\']', block)
    if nm and nm.group(1) == name:
        fv = re.search(r'\b' + re.escape(field) + r'\s*=\s*["\']([^"\']*)["\']', block)
        bv = re.search(r'\b' + re.escape(field) + r'\s*=\s*(true|false)\b', block)
        if fv:
            print(fv.group(1))
        elif bv:
            print(bv.group(1))
        sys.exit(0)
PYEOF
}

_parity_names() {
  python3 - "$REPO/examples/sky/manifest.toml" <<'PYEOF'
import re, sys
with open(sys.argv[1]) as f:
    content = f.read()
for block in re.split(r'\[\[example\]\]', content):
    nm = re.search(r'name\s*=\s*["\']([^"\']+)["\']', block)
    if not nm:
        continue
    def field(f, default=""):
        m = re.search(r'\b' + re.escape(f) + r'\s*=\s*["\']([^"\']*)["\']', block)
        b = re.search(r'\b' + re.escape(f) + r'\s*=\s*(true|false)\b', block)
        return m.group(1) if m else (b.group(1) if b else default)
    if (field("status") == "green"
            and field("verify") == "run"
            and field("shape") in ("program", "console")
            and field("go_ffi", "false") != "true"):
        print(nm.group(1))
PYEOF
}

# ── Build the ipe port for one example (emit + cargo) ────────────────────────
_build_ipe() {
  local ipe_dir="$1" log="$2"
  local entry="$ipe_dir/ipe.toml"
  [ -f "$entry" ] || entry="$ipe_dir/src/Main.ipe"
  rm -rf "$ipe_dir/out" 2>/dev/null
  timeout "${IPE_SWEEP_BUILD_TIMEOUT:-300}" \
    "$IPE_BIN" build "$entry" --out "$ipe_dir/out/rust" >"$log" 2>&1 || return 1
  timeout "${IPE_SWEEP_BUILD_TIMEOUT:-300}" \
    cargo build --manifest-path "$ipe_dir/out/rust/Cargo.toml" >>"$log" 2>&1|| return 2
  return 0
}

# ── Normalize stdout (strips ISO 8601 timestamps + 13-digit epoch ms) ────────
_normalize() {
  sed -E \
    -e 's/[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?(Z|[+-][0-9:]+)?/<TIMESTAMP>/g' \
    -e 's/\b[0-9]{13}\b/<EPOCH_MS>/g'
}

# ── Main loop ────────────────────────────────────────────────────────────────
all_names="$(_parity_names)"

if [ -n "$FILTER_NAMES" ]; then
  filtered=""
  IFS=',' read -ra wanted <<< "$FILTER_NAMES"
  for n in $all_names; do
    for w in "${wanted[@]}"; do
      [ "$n" = "$w" ] && { filtered="$filtered $n"; break; }
    done
  done
  all_names="${filtered# }"
fi

n_exact=0 n_normalized=0 n_skip=0 n_build_fail=0 failed=0

for name in $all_names; do
  orig_dir="$REPO/examples/sky/original/$name"
  ipe_dir="$REPO/examples/sky/ipe/$name"

  parity="$(_field "$name" parity)"
  [ -z "$parity" ] && parity="exact"

  if [ "$parity" = "skip" ]; then
    reason="$(_field "$name" parity_skip_reason)"
    echo "  SKIP $name — ${reason:-nondeterministic output}"
    n_skip=$((n_skip+1))
    continue
  fi

  if [ ! -d "$orig_dir" ]; then
    echo "  SKIP $name — original/ missing (run regen-sky-examples.sh)"
    n_skip=$((n_skip+1)); continue
  fi
  if [ ! -d "$ipe_dir" ]; then
    echo "  SKIP $name — ipe/ missing (run regen-sky-examples.sh)"
    n_skip=$((n_skip+1)); continue
  fi

  # Build ipe port.
  build_log="$(mktemp /tmp/parity-build.XXXXXX)"
  _build_ipe "$ipe_dir" "$build_log"
  build_rc=$?
  if [ "$build_rc" -ne 0 ]; then
    label="ipe build failed"
    [ "$build_rc" -eq 2 ] && label="cargo build failed"
    echo "  BUILD-FAIL $name — $label"
    sed 's/^/    /' "$build_log" >&2
    rm -f "$build_log"
    n_build_fail=$((n_build_fail+1)); continue
  fi
  rm -f "$build_log"

  ipe_bin="$(resolve_bin "$ipe_dir")" || {
    echo "  BUILD-FAIL $name — built binary not found"
    n_build_fail=$((n_build_fail+1)); continue
  }

  # Build + run upstream sky in an isolated copy of the project. sky resolves
  # modules from the working directory (not a path argument) and its `run`
  # interleaves build trace onto stdout, so build first, then execute the
  # emitted binary for clean program output — symmetric with the ipe side.
  sky_out="$(mktemp /tmp/parity-sky.XXXXXX)"
  sky_run_dir="$(mktemp -d "${TMPDIR:-/tmp}/sky-run.XXXXXX")"
  cp -R "$orig_dir/." "$sky_run_dir/" 2>/dev/null
  sky_rc=0
  sky_build_log="$(mktemp /tmp/parity-sky-build.XXXXXX)"
  if ( cd "$sky_run_dir" && exec timeout 300 "$SKY_BIN" build ) >"$sky_build_log" 2>&1; then
    sky_app="$sky_run_dir/sky-out/app"
    [ -x "$sky_app" ] || sky_app="$(find "$sky_run_dir/sky-out" -maxdepth 1 -type f -perm -u+x 2>/dev/null | head -1)"
    if [ -n "$sky_app" ] && [ -x "$sky_app" ]; then
      ( cd "$sky_run_dir" && exec timeout 30 "$sky_app" ) >"$sky_out" 2>/dev/null || sky_rc=$?
    else
      echo "  (sky build produced no runnable binary for $name)" >&2
      sky_rc=127
    fi
  else
    sky_rc=1
    sed 's/^/    sky-build: /' "$sky_build_log" >&2
  fi
  rm -f "$sky_build_log"
  rm -rf "$sky_run_dir" 2>/dev/null

  # Run the built ipe binary.
  ipe_out="$(mktemp /tmp/parity-ipe.XXXXXX)"
  ipe_run_dir="$(mktemp -d "${TMPDIR:-/tmp}/ipe-run.XXXXXX")"
  ipe_rc=0
  ( cd "$ipe_run_dir" && exec timeout 30 "$ipe_bin" ) \
    >"$ipe_out" 2>/dev/null || ipe_rc=$?
  rm -rf "$ipe_run_dir" 2>/dev/null

  # Apply normalization when requested.
  cmp_sky="$sky_out"
  cmp_ipe="$ipe_out"
  norm_sky="" norm_ipe=""
  if [ "$parity" = "normalized" ]; then
    norm_sky="$(mktemp /tmp/parity-norm-sky.XXXXXX)"
    norm_ipe="$(mktemp /tmp/parity-norm-ipe.XXXXXX)"
    _normalize <"$sky_out" >"$norm_sky"
    _normalize <"$ipe_out" >"$norm_ipe"
    cmp_sky="$norm_sky"
    cmp_ipe="$norm_ipe"
  fi

  exit_ok=1; [ "$sky_rc" != "$ipe_rc" ] && exit_ok=0
  stdout_ok=1; diff -q "$cmp_sky" "$cmp_ipe" >/dev/null 2>&1 || stdout_ok=0

  if [ "$exit_ok" -eq 1 ] && [ "$stdout_ok" -eq 1 ]; then
    echo "  OK $name ($parity)"
    [ "$parity" = "normalized" ] && n_normalized=$((n_normalized+1)) \
                                  || n_exact=$((n_exact+1))
  else
    echo "  MISMATCH $name ($parity)"
    [ "$exit_ok" -eq 0 ] && echo "    exit: sky=$sky_rc  ipe=$ipe_rc"
    if [ "$stdout_ok" -eq 0 ]; then
      echo "    stdout diff (first $DIFF_LINES lines):"
      diff -u "$cmp_sky" "$cmp_ipe" | head -"$DIFF_LINES" | sed 's/^/      /'
    fi
    failed=$((failed+1))
  fi

  rm -f "$sky_out" "$ipe_out" 2>/dev/null
  [ -n "$norm_sky" ] && rm -f "$norm_sky" "$norm_ipe" 2>/dev/null || true
  reap 2>/dev/null
done

echo ""
echo "=== check-sky-parity: RESULTS ==="
echo "  exact matched:      $n_exact"
echo "  normalized matched: $n_normalized"
echo "  skipped:            $n_skip"
echo "  build failures:     $n_build_fail"
echo "  mismatches:         $failed"
echo ""
echo "SUMMARY: $n_exact exact, $n_normalized normalized, $n_skip skipped"

if [ "$failed" -gt 0 ]; then
  echo "VERDICT: FAIL ($failed mismatch(es))" >&2; exit 1
fi
echo "VERDICT: PASS"
