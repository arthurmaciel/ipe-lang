#!/usr/bin/env bash
# tools/scripts/check-sky-ports.sh — OFFLINE "our ports work" PR gate.
#
# Verifies the COMMITTED examples/sky/ipe/ ports with no network access:
#   1. Offline consistency check  — committed ipe/ == re-derive of committed
#      original/ via the current rename-map + ipe-edits (regen --check).
#   2. ipe build — build every in-scope (non-go_ffi) committed port.
#   3. ipe run  — run it per its manifest verify policy:
#        verify=run    → run to exit 0
#        verify=build  → build only (run skipped)
#        verify=serve  → build only this increment (serving needs heavy infra)
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

# Read per-example verify policy from manifest.  Default: build.
# Returns via stdout: run | build | serve
_verify_policy() {
  local name="$1"
  python3 -c "
import re, sys
name = '$name'
with open('$REPO/examples/sky/manifest.toml') as f:
    content = f.read()
# Split into [[example]] blocks
blocks = re.split(r'\[\[example\]\]', content)
for block in blocks:
    nm = re.search(r'name\s*=\s*[\"\'](.*?)[\"\']\s', block)
    vf = re.search(r'verify\s*=\s*[\"\'](.*?)[\"\']\s', block)
    if nm and nm.group(1) == name:
        print(vf.group(1) if vf else 'build')
        sys.exit(0)
print('build')
" 2>/dev/null
}

# Read go_ffi flag from manifest.
_is_go_ffi() {
  local name="$1"
  python3 -c "
import re, sys
name = '$name'
with open('$REPO/examples/sky/manifest.toml') as f:
    content = f.read()
blocks = re.split(r'\[\[example\]\]', content)
for block in blocks:
    nm = re.search(r'name\s*=\s*[\"\'](.*?)[\"\']\s', block)
    gf = re.search(r'go_ffi\s*=\s*(true|false)', block)
    if nm and nm.group(1) == name:
        sys.exit(0 if (gf and gf.group(1) == 'true') else 1)
sys.exit(1)
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

# ── Step 2+3: build + run per-policy ─────────────────────────────────────────
echo "=== check-sky-ports: step 2+3 — build + run ==="
names="$(sky_example_names)" || { echo "check-sky-ports: cannot read manifest" >&2; exit 2; }

built=0 ran=0 build_only=0 failed=0 skipped=0

for name in $names; do
  [ -z "$name" ] && continue
  d="$REPO/examples/sky/ipe/$name"

  # Skip go_ffi examples (no Rust-build path).
  if _is_go_ffi "$name"; then
    echo "  SKIP $name (go_ffi)"
    skipped=$((skipped+1)); continue
  fi

  if [ ! -d "$d" ]; then
    echo "  SKIP $name (ipe/ dir missing — run regen-sky-examples.sh)"
    skipped=$((skipped+1)); continue
  fi

  # Needs FFI install (rust.dependencies without generated bindings) → skip.
  if rg -q '^\[rust\.dependencies\]' "$d/ipe.toml" 2>/dev/null && [ ! -d "$d/.ipe/cache/ffi/rust" ]; then
    echo "  SKIP $name (needs ipe install --allow-build-scripts for rust.dependencies)"
    skipped=$((skipped+1)); continue
  fi

  # Composite examples without a top-level src/Main.ipe → build-only skip.
  if [ ! -f "$d/src/Main.ipe" ]; then
    echo "  SKIP $name (no top-level src/Main.ipe — composite root)"
    skipped=$((skipped+1)); continue
  fi

  policy="$(_verify_policy "$name")"

  # Build.
  rm -rf "$d/out" 2>/dev/null
  build_log="$(mktemp /tmp/ipe-port-build.XXXXXX)"
  ipe_entry="$d/ipe.toml"; [ ! -f "$ipe_entry" ] && ipe_entry="$d/src/Main.ipe"
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
  built=$((built+1))
  echo "  BUILD ok $name"

  # Run (per policy).
  case "$policy" in
    run)
      bin="$(resolve_bin "$d")" || { echo "  FAIL $name — built binary not found"; failed=$((failed+1)); continue; }
      run_log="$(mktemp /tmp/ipe-port-run.XXXXXX)"
      if exercise_cli "$bin" "$run_log" 30; then
        echo "  RUN  ok $name"
        ran=$((ran+1))
      else
        echo "  FAIL $name — run failed (exit/hang/panic)"
        sed 's/^/    /' "$run_log" >&2
        failed=$((failed+1))
      fi
      rm -f "$run_log"
      ;;
    serve)
      # Serving needs headless infra not available in this increment — treat
      # as build-only so the gate still exercises the build path.
      echo "  BUILD-ONLY $name (verify=serve — run deferred to later increment)"
      build_only=$((build_only+1))
      ;;
    build|*)
      echo "  BUILD-ONLY $name (verify=build)"
      build_only=$((build_only+1))
      ;;
  esac

  reap 2>/dev/null
done

echo ""
echo "=== check-sky-ports: RESULTS ==="
echo "  built:      $built"
echo "  ran:        $ran"
echo "  build-only: $build_only"
echo "  skipped:    $skipped"
echo "  failed:     $failed"
echo ""

if [ "$failed" -gt 0 ]; then
  echo "VERDICT: FAIL ($failed failure(s))" >&2; exit 1
fi
echo "VERDICT: PASS"
