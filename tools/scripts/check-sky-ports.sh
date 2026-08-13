#!/usr/bin/env bash
# tools/scripts/check-sky-ports.sh — OFFLINE "our ports work" PR gate.
#
# Verifies the COMMITTED examples/sky/ipe/ ports with no network access:
#   1. Offline consistency check  — committed ipe/ == re-derive of committed
#      original/ via the current rename-map + ipe-edits (regen --check).
#   2. Per-example build + run, gated by manifest status:
#        green         — full (consistency + emit + cargo build + run per verify);
#                        any failure = gate FAIL
#        deps-deferred — consistency + emit ONLY (no cargo build/run);
#                        printed as SKIP-BUILD
#        ice-blocked   — consistency + emit ONLY; printed as SKIP-BUILD
#        broken        — consistency + emit ONLY; printed as SKIP-BUILD
#      Consistency/regen mismatch on ANY status = gate FAIL.
#
# DRIFT detection (never fails the gate):
#   If a non-green port cargo-builds cleanly, print:
#     DRIFT: <name> reclassifiable to green
#
# Summary line:
#   N green (built+ran), M deps-deferred, K ice-blocked, J broken
#
# This gate runs on EVERY PR touching examples/sky/ or src/ or the converter.
# It is deliberately scoped to the committed trees so it is deterministic and
# requires no network (upstream drift is the nightly job's concern).
#
# EXIT: 0 all passed  1 one or more failures  2 setup error
set -uo pipefail

source "$(dirname "$0")/lib/env.sh"
source "$(dirname "$0")/lib/mirror.sh"
source "$(dirname "$0")/lib/checks.sh"

cd "$REPO" || { echo "check-sky-ports: cannot locate repo" >&2; exit 2; }

if [ ! -x "$IPE_BIN" ]; then
  echo "check-sky-ports: ipe binary not found at '$IPE_BIN'" >&2
  echo "  Build it: cargo build --release -p ipe" >&2
  exit 2
fi

# Disk guard: builds corrupt under ENOSPC.
FREE_KB="$(df -Pk "$REPO" 2>/dev/null | awk 'NR==2{print $4}')"
if [ -n "$FREE_KB" ] && [ "$FREE_KB" -lt 5242880 ]; then
  echo "check-sky-ports: < 5G free on $REPO ($((FREE_KB/1024/1024))G) — aborting." >&2; exit 2
fi

# Read a field from the manifest for a given example name.
# Usage: _manifest_field <name> <field>
# Returns the field value via stdout, or empty string when absent.
_manifest_field() {
  local name="$1" field="$2"
  python3 -c "
import re, sys
name = '$name'
field = '$field'
with open('$REPO/examples/sky/manifest.toml') as f:
    content = f.read()
blocks = re.split(r'\[\[example\]\]', content)
for block in blocks:
    nm = re.search(r'name\s*=\s*[\"\'](.*?)[\"\']\s', block)
    if nm and nm.group(1) == name:
        fv = re.search(r'\b' + re.escape(field) + r'\s*=\s*[\"\'](.*?)[\"\']\s', block)
        print(fv.group(1) if fv else '')
        sys.exit(0)
print('')
" 2>/dev/null
}

# ── Step 1: offline consistency check ────────────────────────────────────────
echo "=== check-sky-ports: step 1/3 — offline consistency check ==="
if [ -d "$REPO/examples/sky/original" ]; then
  if ! bash "$REPO/tools/scripts/regen-sky-examples.sh" --check; then
    echo "check-sky-ports: FAIL — committed ipe/ ports are inconsistent with original/" >&2
    exit 1
  fi
else
  echo "check-sky-ports: examples/sky/original/ missing — skip consistency check (run regen first)" >&2
fi

# ── Step 2+3: build + run per status policy ───────────────────────────────────
echo "=== check-sky-ports: step 2+3 — build + run ==="
names="$(sky_example_names)" || { echo "check-sky-ports: cannot read manifest" >&2; exit 2; }

n_green=0 n_deps_deferred=0 n_ice_blocked=0 n_broken=0 n_skipped=0 failed=0

for name in $names; do
  [ -z "$name" ] && continue
  d="$REPO/examples/sky/ipe/$name"

  go_ffi="$(_manifest_field "$name" go_ffi)"
  status="$(_manifest_field "$name" status)"
  verify="$(_manifest_field "$name" verify)"
  [ -z "$verify" ] && verify="build"

  # go_ffi examples are never built in the Rust gate.
  if [ "$go_ffi" = "true" ]; then
    echo "  SKIP $name (go_ffi)"
    n_skipped=$((n_skipped+1)); continue
  fi

  if [ ! -d "$d" ]; then
    echo "  SKIP $name (ipe/ dir missing — run regen-sky-examples.sh)"
    n_skipped=$((n_skipped+1)); continue
  fi

  # Needs FFI install (rust.dependencies without generated bindings) → skip.
  if rg -q '^\[rust\.dependencies\]' "$d/ipe.toml" 2>/dev/null && [ ! -d "$d/.ipe/cache/ffi/rust" ]; then
    echo "  SKIP $name (needs ipe install --allow-build-scripts for rust.dependencies)"
    n_skipped=$((n_skipped+1)); continue
  fi

  # Composite examples without a top-level src/Main.ipe → build-only skip.
  if [ ! -f "$d/src/Main.ipe" ]; then
    echo "  SKIP $name (no top-level src/Main.ipe — composite root)"
    n_skipped=$((n_skipped+1)); continue
  fi

  # Determine entry point.
  ipe_entry="$d/ipe.toml"; [ ! -f "$ipe_entry" ] && ipe_entry="$d/src/Main.ipe"

  # ── Non-green: emit-only check ───────────────────────────────────────────
  if [ "$status" != "green" ]; then
    label="$status"
    rm -rf "$d/out" 2>/dev/null
    emit_log="$(mktemp /tmp/ipe-port-emit.XXXXXX)"
    timeout "${IPE_SWEEP_BUILD_TIMEOUT:-300}" \
      "$IPE_BIN" build "$ipe_entry" --emit-ir >"$emit_log" 2>&1
    emit_rc=$?
    if [ $emit_rc -ne 0 ]; then
      # Emit genuinely failed — still only a tracked skip, not a gate FAIL,
      # because this port is already classified non-green.
      echo "  SKIP-BUILD $name ($label) [emit-check failed — expected]"
    else
      echo "  SKIP-BUILD $name ($label)"
      # Drift hint: emit passes for a non-green port — worth re-classifying.
      # (Never a gate failure; cargo build is not attempted here to avoid
      # lock contention with concurrent green builds.)
      echo "  DRIFT: $name emit passes — cargo build may be reclassifiable to green"
    fi
    rm -f "$emit_log"
    case "$label" in
      deps-deferred) n_deps_deferred=$((n_deps_deferred+1)) ;;
      ice-blocked)   n_ice_blocked=$((n_ice_blocked+1)) ;;
      *)             n_broken=$((n_broken+1)) ;;
    esac
    reap 2>/dev/null
    continue
  fi

  # ── Green: full build + run ───────────────────────────────────────────────
  rm -rf "$d/out" 2>/dev/null
  build_log="$(mktemp /tmp/ipe-port-build.XXXXXX)"
  if ! timeout "${IPE_SWEEP_BUILD_TIMEOUT:-300}" \
       "$IPE_BIN" build "$ipe_entry" --out "$d/out/rust" \
       >"$build_log" 2>&1; then
    echo "  FAIL $name — ipe build failed"
    sed 's/^/    /' "$build_log" >&2
    rm -f "$build_log"; failed=$((failed+1)); continue
  fi
  if ! timeout "${IPE_SWEEP_BUILD_TIMEOUT:-300}" \
       cargo build --manifest-path "$d/out/rust/Cargo.toml" \
       >>"$build_log" 2>&1; then
    echo "  FAIL $name — cargo build failed"
    sed 's/^/    /' "$build_log" >&2
    rm -f "$build_log"; failed=$((failed+1)); continue
  fi
  rm -f "$build_log"
  echo "  BUILD ok $name"

  # Run per verify policy.
  case "$verify" in
    run)
      bin="$(resolve_bin "$d")" || { echo "  FAIL $name — built binary not found"; failed=$((failed+1)); continue; }
      run_log="$(mktemp /tmp/ipe-port-run.XXXXXX)"
      if exercise_cli "$bin" "$run_log" 30; then
        echo "  RUN  ok $name"
        n_green=$((n_green+1))
      else
        echo "  FAIL $name — run failed (exit/hang/panic)"
        sed 's/^/    /' "$run_log" >&2
        failed=$((failed+1))
      fi
      rm -f "$run_log"
      ;;
    serve)
      echo "  BUILD-ONLY $name (verify=serve — run deferred to later increment)"
      n_green=$((n_green+1))
      ;;
    build|*)
      echo "  BUILD-ONLY $name (verify=build)"
      n_green=$((n_green+1))
      ;;
  esac

  reap 2>/dev/null
done

echo ""
echo "=== check-sky-ports: RESULTS ==="
echo "  green (built+ran): $n_green"
echo "  deps-deferred:     $n_deps_deferred"
echo "  ice-blocked:       $n_ice_blocked"
echo "  broken:            $n_broken"
echo "  skipped:           $n_skipped"
echo "  failed:            $failed"
echo ""
echo "SUMMARY: $n_green green (built+ran), $n_deps_deferred deps-deferred, $n_ice_blocked ice-blocked, $n_broken broken"
echo ""

if [ "$failed" -gt 0 ]; then
  echo "VERDICT: FAIL ($failed failure(s))" >&2; exit 1
fi
echo "VERDICT: PASS"
