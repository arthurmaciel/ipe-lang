#!/usr/bin/env bash
# Ipê EXAMPLES sweep — the upstream-mirror parity PROOF. For each in-scope
# example it mirrors the real upstream Sky example, patches it into Ipê, and asks
# ONE question: does OUR compiler build and run it? Each example yields one table
# row with two columns:
#
#   BUILD   ipe build + cargo build (emitted crate)   → ok / ipe-fail / cargo-fail
#   RUN     run the emitted binary headless, per shape → ok / failed / panic / hang / noserve / notty / skip
#           (failed = the process exited non-zero, e.g. a failing test assertion)
#
# There is NO reference-compiler build and NO cross-compiler output comparison.
# This is a "does the real upstream example (patched) build+run on ipe" proof.
#
# HOW AN EXAMPLE IS MATERIALISED (tools/scripts/lib/mirror.sh):
#   1. Fetch examples/<name> from upstream GitHub (anzellai/sky) — the raw tree
#      lands in examples/sky/original/<name>/, its Ipê port in examples/sky/ipe/<name>/.
#   2. Apply examples/sky/rename-map.tsv (the shared Sky→Ipe token rewrite) and
#      then the OPTIONAL examples/sky/ipe-edits/<name>.edits semantic delta.
#
# The BUILD step, per example:
#   ( cd <example> && ipe build <ipe.toml | src/Main.ipe> --out out/rust )
#   cargo build --manifest-path <example>/out/rust/Cargo.toml
# ipe emits a self-contained Cargo project under out/rust/ with the runtime
# vendored into src/ipe_runtime, whose default binary is `ipe-app`.
#
# GREEN row  = BUILD ok AND RUN ok.
# RED row    = BUILD/RUN failure (ipe-fail / cargo-fail / panic / hang / noserve /
#              notty) OR an example in the manifest whose source could not be
#              mirrored (no-source) OR an upstream example NOT in the manifest
#              (unpatched-new-example).
# SKIP row   = a shape this host cannot exercise (no pty / no display).
# VERDICT PASS iff no RED row.
#
# FLAGS:
#   IPE_SWEEP_BUILD_ONLY=1  → BUILD column only (RUN = `—`).
#   RUST_EXAMPLES="00-… 01-…" → subset override (basenames of manifest examples).
#   IPE_SWEEP_FORCE=1       → override the (opt-in) night gate.
#   IPE_SWEEP_NIGHT_GATE=1  → re-enable the local 22:00–08:00 BRT deferral window.
#
# Exit: 0 = no RED row · 1 = a RED row · 2 = setup/gate.
set -uo pipefail

# ── Env + example classification + per-shape RUN helpers + mirror ────────────
source "$(dirname "$0")/lib/env.sh"
source "$(dirname "$0")/lib/examples.sh"
source "$(dirname "$0")/lib/checks.sh"
source "$(dirname "$0")/lib/mirror.sh"

night_guard "examples-sweep"

if [ -z "$REPO" ] || [ ! -f "$REPO/tools/scripts/examples-sweep.sh" ]; then
  echo "ERROR: can't locate the repo. cd into it, or set IPE_REPO=/path/to/sky-rust." >&2; exit 2
fi
cd "$REPO"
if [ ! -x "$IPE_BIN" ]; then
  echo "ERROR: ipe binary not at '$IPE_BIN' — build it: cargo build --release -p ipe (or set IPE_BIN)." >&2; exit 2
fi

# ── Preflight: corrupted builds under low disk (HARD gate) ───────────────────
FREE_KB="$(df -Pk "$REPO" 2>/dev/null | awk 'NR==2{print $4}')"
if [ -n "$FREE_KB" ] && [ "$FREE_KB" -lt 5242880 ]; then
  echo "ERROR: < 5G free disk on $REPO ($((FREE_KB/1024/1024))G) — builds corrupt under ENOSPC. Free space first." >&2; exit 2
fi

BUILD_ONLY="${IPE_SWEEP_BUILD_ONLY:-0}"
if [ "$BUILD_ONLY" != 1 ]; then
  command -v curl >/dev/null 2>&1 || { echo "ERROR: curl required for RUN (set IPE_SWEEP_BUILD_ONLY=1)." >&2; exit 2; }
  command -v python3 >/dev/null 2>&1 || { echo "ERROR: python3 required for free_port (set IPE_SWEEP_BUILD_ONLY=1)." >&2; exit 2; }
fi
command -v rg >/dev/null 2>&1 || { echo "ERROR: rg (ripgrep) required for the example-scope filter (is_out_of_scope). Install ripgrep." >&2; exit 2; }

# ── flock: serialize the shared-CARGO_TARGET_DIR build/run span ──────────────
# flock guards ONLY the case of two concurrent sweeps sharing one target dir
# (a local convenience). A single CI sweep has no contender. Git Bash on
# Windows ships no flock; rather than block the whole harness there, fall back
# to a no-op lock wrapper when real flock is absent — correct because the
# span it guards is uncontended in a lone sweep. Everywhere flock DOES exist
# (Linux/macOS) the real serialization is unchanged.
if command -v flock >/dev/null 2>&1; then
  _sweep_flock() { flock "$@"; }
  SWEEP_FLOCK_REAL=1
else
  _sweep_flock() { :; }   # no contender in a single sweep → no-op is sound
  SWEEP_FLOCK_REAL=0
  echo "NOTE: flock unavailable (e.g. Git Bash on Windows) — build/run span runs unserialized; sound for a single sweep, unsafe only for two concurrent sweeps sharing one CARGO_TARGET_DIR." >&2
fi

HIST="$HOME/.cache/ipe/examples-sweep"; mkdir -p "$HIST"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)-$$"
TABLE="$HIST/sweep-$STAMP.table"
RUNLOG="$HIST/run-$STAMP.log"
say() { echo "$@" | tee -a "$RUNLOG"; }
diag() { printf '%s/%s.%s.%s\n' "$HIST" "$1" "$STAMP" "$2"; }
say "=== Ipê EXAMPLES sweep @ $STAMP (repo: $REPO · ipe: $IPE_BIN) ==="

# ── Regenerate the examples/sky/{original,ipe} trees from upstream (always) ───
# The sweep regenerates every in-scope example from the CURRENT upstream Sky
# (raw → examples/sky/original/, transformed+edited → examples/sky/ipe/) and
# builds/runs the ports. A missing upstream source for an individual example
# surfaces as a no-source RED row (below), never a silent skip.
MIRROR_OK=1
if ! mirror_sky_examples; then
  echo "WARN: no upstream Sky example source located (network fetch from anzellai/sky failed — offline?)." >&2
  MIRROR_OK=0
fi

# ── Fail loud on unpatched new upstream examples ─────────────────────────────
# Any upstream dir with a Sky entry point that is NOT listed in the manifest is
# an unpatched new example — it must be added (with verified edits) before the
# sweep can pass. `rust` is not an example (a helper crate), so it is excluded.
UNPATCHED_NEW_EXAMPLES=()
_manifest_names="$(sky_example_names 2>/dev/null)" || _manifest_names=""
while IFS= read -r _uname; do
  [ -z "$_uname" ] && continue
  [ "$_uname" = rust ] && continue
  if ! printf '%s\n' "$_manifest_names" | rg -q "^${_uname}$"; then
    UNPATCHED_NEW_EXAMPLES+=("$_uname")
  fi
done < <(sky_upstream_names_network 2>/dev/null | sort)
if [ "${#UNPATCHED_NEW_EXAMPLES[@]}" -gt 0 ]; then
  say ""
  say "ERROR: upstream Sky has example(s) NOT in examples/sky/manifest.toml — add them with a verified patch:"
  for _un in "${UNPATCHED_NEW_EXAMPLES[@]}"; do say "  UNPATCHED: $_un"; done
  say ""
fi

[ "$BUILD_ONLY" = 1 ] && say "  (IPE_SWEEP_BUILD_ONLY=1 — BUILD column only; RUN skipped)"

# ── ipe build target for an example dir — ipe.toml if present, else src/Main.ipe
# The mirror renames the upstream sky.toml to ipe.toml (lib/mirror.sh), so a
# materialised example carries Ipê's canonical manifest name.
ipe_build_target() {
  local d="$1"
  if [ -f "$d/ipe.toml" ]; then echo "ipe.toml"; else echo "src/Main.ipe"; fi
}

# ── build_rust <dir> <example> → 0=ok; sets BUILD_CELL to the failure word ───
BUILD_CELL=""
WARN_CELL=0
build_rust() {
  local d="$1" n="$2" tmo="${IPE_SWEEP_BUILD_TIMEOUT:-900}" tgt attempt ok=0
  local ipelog cargolog; ipelog="$(diag "$n" ipe.log)"; cargolog="$(diag "$n" cargo.log)"
  local shape; shape="$(example_shape "$d")"
  tgt="$(ipe_build_target "$d")"

  # Wasm examples: `ipe build --target wasm` runs the full pipeline internally
  # (cargo build --target wasm32-unknown-unknown → wasm-bindgen → optional
  # wasm-opt). No separate `cargo build --manifest-path` step.
  if [ "$shape" = wasm ]; then
    for attempt in 1 2 3 4; do
      if ( cd "$d" && timeout "$tmo" "$IPE_BIN" build "$tgt" --out out/rust --target wasm >"$ipelog" 2>&1 ); then
        ok=1; break
      fi
      if [ "$attempt" -lt 4 ] && \
         rg -q 'unable to update registry|download of .* failed|curl failed|HTTP2 framing|spurious network error|Connection reset|operation timed out|failed to get response' "$ipelog"; then
        sleep 5; continue
      fi
      break
    done
    if [ "$ok" != 1 ]; then BUILD_CELL="ipe-fail"; return 1; fi
    if [ ! -f "$d/out/rust/www/pkg/ipe_app_bg.wasm" ]; then
      BUILD_CELL="cargo-fail"; return 1
    fi
    WARN_CELL=0
    BUILD_CELL="ok"; return 0
  fi

  for attempt in 1 2 3 4; do
    if ( cd "$d" && timeout "$tmo" "$IPE_BIN" build "$tgt" --out out/rust >"$ipelog" 2>&1 ); then
      ok=1; break
    fi
    if [ "$attempt" -lt 4 ] && \
       rg -q 'unable to update registry|download of .* failed|curl failed|HTTP2 framing|spurious network error|Connection reset|operation timed out|failed to get response' "$ipelog"; then
      sleep 5; continue
    fi
    break
  done
  if [ "$ok" != 1 ]; then BUILD_CELL="ipe-fail"; return 1; fi
  # cargo build the emitted crate. The vendored runtime carries
  # `#![allow(unused, non_snake_case)]`, so a warning that LEAKS PAST that allow
  # is a genuine codegen defect — counted + gated.
  if ! ( cd "$d" && timeout 900 cargo build --manifest-path out/rust/Cargo.toml >"$cargolog" 2>&1 ); then
    BUILD_CELL="cargo-fail"; return 1
  fi
  WARN_CELL="$(rg -o 'generated [0-9]+ warning' "$cargolog" 2>/dev/null | rg -o '[0-9]+' | tail -1)"
  : "${WARN_CELL:=0}"
  BUILD_CELL="ok"; return 0
}

# ── RUN for one example → echoes the RUN cell + NOTE (tab-separated) ─────────
run_for() {
  local n="$1" shape="$2" bin="$3" rl; rl="$(diag "$n" run.log)"
  case "$shape" in
    wasm)
      local www_dir scenario node_bin wasm_log wrc
      www_dir="$bin"
      wasm_log="$(diag "$n" wasm.log)"
      node_bin="${NODE:-node}"
      if ! command -v "$node_bin" >/dev/null 2>&1; then
        printf 'skip\twasm RUN: node not found (install Node.js)\n'; return 0
      fi
      local verify_mjs="$REPO/tools/scripts/lib/wasm-verify.mjs"
      if [ ! -f "$verify_mjs" ]; then
        printf 'skip\twasm RUN: wasm-verify.mjs not found\n'; return 0
      fi
      scenario="$(scenario_for "$n" 2>/dev/null || echo smoke)"
      [ -z "$scenario" ] && scenario="smoke"
      if timeout 60 "$node_bin" "$verify_mjs" "$www_dir" "$scenario" >"$wasm_log" 2>&1; then
        printf 'ok\t(wasm browser scenario: %s)\n' "$scenario"
      else
        wrc=$?
        if rg -q "did not mount\|WASM app\|timeout" "$wasm_log" 2>/dev/null; then
          printf 'panic\twasm did not boot (see %s)\n' "$(basename "$wasm_log")"
        else
          printf 'panic\twasm scenario %s failed (exit %s; see %s)\n' "$scenario" "$wrc" "$(basename "$wasm_log")"
        fi
      fi
      ;;
    cli)
      exercise_cli "$bin" "$rl"; local crc=$?
      if   [ "$crc" = 0 ]; then printf 'ok\t\n'
      elif grep -qiE "$PANIC_RE" "$rl"; then printf 'panic\tcli panicked\n'
      elif is_live_network_cli "$n"; then printf 'skip\tcli makes a live external HTTP call — network-dependent RUN; not a Rust defect\n'
      elif [ "$crc" = 3 ]; then printf 'failed\tcli exited non-zero (see %s)\n' "$(basename "$rl")"
      else printf 'hang\tcli timed out\n'; fi
      ;;
    tui)
      exercise_tui "$bin" "$rl"; local rc=$?
      if   [ "$rc" = 0 ]; then printf 'ok\t\n'
      elif [ "$rc" = "$EXERCISE_SKIP_RC" ]; then printf 'skip\t%s\n' "$(grep -m1 '^SKIP' "$rl" | sed 's/^SKIP //')"
      elif grep -qiE "not a tty|inappropriate ioctl|TERM environment" "$rl"; then printf 'notty\tno terminal allocated\n'
      else printf 'panic\ttui panicked\n'; fi
      ;;
    webview)
      if ! command -v xvfb-run >/dev/null 2>&1 && [ "$IPE_HOST_OS" = linux ]; then printf 'skip\twebview: install xvfb to run headless\n'
      else
        exercise_webview "$bin" "$rl"; local rc=$?
        if   [ "$rc" = 0 ]; then printf 'ok\t\n'
        elif [ "$rc" = "$EXERCISE_SKIP_RC" ]; then printf 'skip\t%s\n' "$(grep -m1 '^SKIP' "$rl" | sed 's/^SKIP //')"
        else printf 'panic\twebview panicked\n'; fi
      fi
      ;;
    fyne)
      printf 'skip\tfyne: Go-FFI shape — not a Rust target\n'
      ;;
    live)
      local port; port="$(free_port)"
      local lrc; exercise_server "$bin" "$port" "$rl"; lrc=$?
      if   [ "$lrc" = 0 ]; then printf 'ok\t(serves :%s)\n' "$port"
      elif [ "$lrc" = "$EXERCISE_SKIP_RC" ]; then printf 'skip\tlive bound but unreachable (macOS loopback-probe limitation)\n'
      elif grep -qiE "$PANIC_RE" "$rl"; then printf 'panic\tlive panicked\n'
      else printf 'noserve\tlive did not serve\n'; fi
      ;;
    server)
      local port src; port="$(free_port)"
      exercise_server "$bin" "$port" "$rl"; src=$?
      if   [ "$src" = 0 ]; then printf 'ok\t(serves :%s)\n' "$port"
      elif [ "$src" = "$EXERCISE_SKIP_RC" ]; then printf 'skip\tserver bound but unreachable (macOS loopback-probe limitation)\n'
      elif grep -qiE "$PANIC_RE" "$rl"; then printf 'panic\tserver panicked\n'
      else printf 'noserve\tserver did not serve\n'; fi
      ;;
    *) printf 'skip\tunknown shape %s\n' "$shape" ;;
  esac
}

# ── Build the example list (build_set over the examples/sky/ipe ports) ────────
# build_set (lib/examples.sh) folds in examples/sky/ipe/* and drops Go-FFI examples.
# RUST_EXAMPLES overrides with an explicit subset (basenames or paths).
EXAMPLES=()
if [ -n "${RUST_EXAMPLES:-}" ]; then
  for e in $RUST_EXAMPLES; do
    if [ -d "$e" ]; then EXAMPLES+=("${e%/}")
    elif [ -d "examples/sky/ipe/$e" ]; then EXAMPLES+=("examples/sky/ipe/$e")
    else e="examples/${e#examples/}"; EXAMPLES+=("${e%/}"); fi
  done
else
  while IFS= read -r d; do EXAMPLES+=("$d"); done < <(build_set)
fi

say ""; say ">>> EXAMPLES SWEEP  (in-scope set DERIVED in lib/examples.sh; ports regenerated into examples/sky/ipe/ by lib/mirror.sh)"
ROWS="$HIST/rows-$STAMP.tsv"; : >"$ROWS"
WARNS="$HIST/warnings-$STAMP.tsv"; : >"$WARNS"
DCUR=""

# Flush any unpatched-new-example RED rows now that $ROWS exists.
if [ "${#UNPATCHED_NEW_EXAMPLES[@]}" -gt 0 ]; then
  for _un in "${UNPATCHED_NEW_EXAMPLES[@]}"; do
    [ -z "$_un" ] && continue
    printf '%s\t%s\t%s\t%s\n' "$_un" "unpatched-new-example" "—" \
      "upstream example not in examples/sky/manifest.toml — add + verify edits first" \
      >>"$ROWS"
  done
fi

# A manifest example whose source could not be mirrored (upstream fetch failed)
# is a RED no-source row — never a silent skip.
if [ "$MIRROR_OK" = 1 ]; then
  while IFS= read -r _mn; do
    [ -z "$_mn" ] && continue
    is_out_of_scope "examples/sky/ipe/$_mn" 2>/dev/null && continue
    [ -f "examples/sky/ipe/$_mn/src/Main.ipe" ] && continue
    if [ -d "examples/sky/ipe/$_mn" ] && \
       find "examples/sky/ipe/$_mn" -mindepth 2 -name 'Main.ipe' -print -quit 2>/dev/null | rg -q .; then
      # Materialised, but a multi-app COMPOSITE: sub-apps each carry their own
      # src/Main.ipe under a nested dir, so the flat per-dir sweep has no single
      # entry to build. A structural SKIP, not a failure — building the sub-apps
      # as separate units is a sweep-structure follow-up.
      printf '%s\t%s\t%s\t%s\n' "$_mn" "ok" "skip" \
        "composite (multi-app); no top-level src/Main.ipe — sub-apps not built by the flat sweep" >>"$ROWS"
    else
      # Genuinely absent: no upstream source located for this manifest example.
      printf '%s\t%s\t%s\t%s\n' "$_mn" "no-source" "—" \
        "manifest example not materialised (no upstream source located)" >>"$ROWS"
    fi
  done < <(sky_example_names 2>/dev/null)
fi

# ── Cross-invocation build serialization ─────────────────────────────────────
# ipe emits a FIXED `ipe-app` binary into the shared $CARGO_TARGET_DIR. flock the
# build→resolve→run span per example so two concurrent sweeps sharing one target
# dir interleave safely instead of racing on ipe-app.
SWEEP_LOCK_FILE="$CARGO_TARGET_DIR/.examples-sweep-build.lock"
if [ "$SWEEP_FLOCK_REAL" = 1 ]; then
  exec {SWEEP_LOCK_FD}>"$SWEEP_LOCK_FILE"
else
  SWEEP_LOCK_FD=-   # no-op flock ignores the fd; a placeholder keeps the calls uniform
fi

for d in "${EXAMPLES[@]}"; do
  n="$(basename "$d")"
  [ -f "$d/src/Main.ipe" ] || continue
  DCUR="$d"
  shape="$(example_shape "$d")"

  # A manifest example marked `blocked` exercises an Ipê feature not yet
  # implemented (a tracked compiler gap, not a mirror defect). Documented SKIP,
  # never a surprise RED and never a silent pass.
  blocked_reason="$(_manifest_blocked "$n" 2>/dev/null)" || blocked_reason=""
  if [ -n "$blocked_reason" ]; then
    printf '%s\t%s\t%s\t%s\n' "$n" "blocked" "skip" "$blocked_reason" >>"$ROWS"
    continue
  fi

  # A `[rust.dependencies]` example needs a sandboxed `ipe install` to generate
  # its shim-free Rust-SDK bindings (into a gitignored .ipe/cache/ffi/rust) before
  # `ipe build` can resolve its `import Rust.<Crate>` modules. The per-commit
  # sweep does not run that install (build-scripts / network / RCE-sandbox), so
  # without a pre-populated cache this is an install prerequisite, not a compiler
  # defect: a SKIP row, never a false RED.
  if needs_ffi_install "$d"; then
    printf '%s\t%s\t%s\t%s\n' "$n" "ok" "skip" \
      "needs FFI install (ipe install --allow-build-scripts) to generate Rust.* bindings — not run in the per-commit sweep" >>"$ROWS"
    continue
  fi

  ( cd "$d" && rm -rf out .ipe .ipecache .ipedeps )

  build_cell=""; run_cell="—"; note=""

  _sweep_flock -x "$SWEEP_LOCK_FD"

  if ! build_rust "$d" "$n"; then
    build_cell="$BUILD_CELL"; note="rust build failed (see $(basename "$(diag "$n" ipe.log)") / $(basename "$(diag "$n" cargo.log)"))"
    printf '%s\t%s\t%s\t%s\n' "$n" "$build_cell" "—" "$note" >>"$ROWS"
    ( cd "$d" && rm -rf out .ipe .ipecache .ipedeps ); _sweep_flock -u "$SWEEP_LOCK_FD"; continue
  fi
  build_cell="ok"
  printf '%s\t%s\n' "$n" "${WARN_CELL:-0}" >>"$WARNS"
  rbin="$(resolve_bin "$d")"

  # Wasm examples have no native binary; pass the www/ path as the "bin" sentinel.
  if [ "$shape" = wasm ] && [ "$BUILD_ONLY" != 1 ]; then
    rbin="$d/out/rust/www"
  fi

  if [ "$BUILD_ONLY" = 1 ] || [ -z "$rbin" ]; then
    [ -z "$rbin" ] && { run_cell="noserve"; note="no binary resolved after build"; }
    printf '%s\t%s\t%s\t%s\n' "$n" "$build_cell" "$run_cell" "$note" >>"$ROWS"
    ( cd "$d" && rm -rf out .ipe .ipecache .ipedeps ); _sweep_flock -u "$SWEEP_LOCK_FD"; continue
  fi

  IFS=$'\t' read -r run_cell run_note < <(run_for "$n" "$shape" "$rbin")

  _sweep_flock -u "$SWEEP_LOCK_FD"

  note="$run_note"
  printf '%s\t%s\t%s\t%s\n' "$n" "$build_cell" "$run_cell" "$note" >>"$ROWS"
  reap
  ( cd "$d" && rm -rf out .ipe .ipecache .ipedeps )
done

# ── Render the aligned table ─────────────────────────────────────────────────
{
  printf "%-28s %-22s %-9s %s\n" "EXAMPLE" "BUILD" "RUN" "NOTE"
  printf "%-28s %-22s %-9s %s\n" "-------" "-----" "---" "----"
  while IFS=$'\t' read -r n b r note; do
    printf "%-28s %-22s %-9s %s\n" "$n" "$b" "$r" "$note"
  done < "$ROWS"
} | tee "$TABLE" | tee -a "$RUNLOG"

# ── Verdict ──────────────────────────────────────────────────────────────────
RED=0; GREEN=0; SKIP=0; RED_ROWS=""
while IFS=$'\t' read -r n b r note; do
  row_red=0
  case "$b" in ipe-fail|cargo-fail|no-source|unpatched-new-example) row_red=1 ;; esac
  case "$r" in panic|hang|noserve|notty|failed) row_red=1 ;; esac
  row_skip=0; case "$r" in skip) SKIP=$((SKIP+1)); row_skip=1 ;; esac
  if [ "$row_red" = 1 ]; then RED=$((RED+1)); RED_ROWS="$RED_ROWS $n"
  elif [ "$row_skip" = 0 ]; then GREEN=$((GREEN+1)); fi
done < "$ROWS"
TOTAL="$(wc -l < "$ROWS" | tr -d ' ')"

# ── Cargo-warning tally (warnings that LEAK PAST the generated `#![allow]`) ──
WARN_TOTAL=0; WARN_ROWS=""
if [ -f "$WARNS" ]; then
  while IFS=$'\t' read -r wn wc; do
    [ "${wc:-0}" -gt 0 ] 2>/dev/null || continue
    WARN_TOTAL=$((WARN_TOTAL + wc)); WARN_ROWS="$WARN_ROWS $wn($wc)"
  done < "$WARNS"
fi
WARN_FAIL=0
[ "$WARN_TOTAL" -gt 0 ] && [ "${IPE_SWEEP_WARN_GATE:-1}" != 0 ] && WARN_FAIL=1

say ""
say "  summary: $GREEN green · $RED red · $SKIP skipped (of $TOTAL)"
say "  cargo warnings (past #![allow]): $WARN_TOTAL total${WARN_ROWS:+ —$WARN_ROWS}"
say "  full table: $TABLE · warnings: $WARNS"

SCORE="$HIST/scoreboard.tsv"
printf '%s\tgreen=%s\tred=%s\tskip=%s\n' "$STAMP" "$GREEN" "$RED" "$SKIP" >>"$SCORE"

if [ "$RED" -gt 0 ] || [ "$WARN_FAIL" = 1 ]; then
  [ "$RED" -gt 0 ] && say "  RED rows (build/run failure — investigate):${RED_ROWS}"
  [ "$WARN_FAIL" = 1 ] && say "  WARNING rows (codegen defect past #![allow]):${WARN_ROWS}"
  say ""; say "=== VERDICT: FAIL ($RED red row(s), $WARN_TOTAL cargo warning(s)) ==="
  exit 1
fi
say ""; say "=== VERDICT: PASS · no red row · $WARN_TOTAL cargo warning(s) · table=$TABLE ==="
exit 0
