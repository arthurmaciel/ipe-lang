#!/usr/bin/env bash
# ipê EXAMPLES sweep — the cornerstone correctness gate. For each in-scope example
# (build_set, DERIVED in lib/examples.sh: every candidate dir minus Go-FFI) it
# does up to THREE things and emits ONE table row with three columns:
#
#   BUILD   skyc build + cargo build                        → ok / skyc-fail / cargo-fail
#   RUN     run the Rust binary headless, per shape         → ok / panic / hang / noserve / notty / skip
#   EQUIVALENCE   build the Go reference + compare to Rust        → equivalence-* / n/a / DIFFER / go-ref-broken
#
# PORTED from ../sky/runtime-rust/scripts/examples-sweep.sh. KEY ADAPTATION: the
# compiler here is `skyc` (Rust-only cargo workspace), not the Haskell `sky`. The
# BUILD step invokes:
#
#   ( cd <example> && skyc build <sky.toml | src/Main.ipe> [--out sky-out/rust] )
#   cargo build --manifest-path <example>/sky-out/rust/Cargo.toml
#
# skyc has NO `--backend` flag (it only targets Rust); it emits a self-contained
# Cargo project under sky-out/rust/ with the runtime vendored into
# src/sky_runtime, whose default package/binary is `sky-app`. Verified against
# src/ipe-cli/src/lib.rs `run_build` (usage: `skyc build <entry.ipe|project-dir|
# sky.toml> [--out <dir>] [--runtime <dir>]`) + the E2E test in the same file that
# builds sky-out and runs target/debug/sky-app.
#
# PHASED Go-parity: EQUIVALENCE needs a Go reference built by the Haskell `sky`
# compiler, which this repo does NOT have. The FIRST CI iteration runs BUILD+RUN
# only (IPE_SWEEP_NO_EQUIV=1). The EQUIVALENCE column + build_go() below are kept intact
# so parity can be turned on later (see docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md §1).
#
# GREEN row  = BUILD ok AND RUN ok AND EQUIVALENCE ∈ {equivalence-*, n/a, —, amber go-ref-broken}.
# RED row    = BUILD/RUN/EQUIVALENCE failure (*-fail / panic / hang / noserve / notty /
#              DIFFER) — UNLESS EQUIVALENCE = go-ref-broken (AMBER).
# VERDICT PASS iff no RED row.
#
# FLAGS:
#   IPE_SWEEP_BUILD_ONLY=1  → BUILD column only (RUN + EQUIVALENCE = `—`). No `go`.
#   IPE_SWEEP_NO_EQUIV=1    → BUILD + RUN; EQUIVALENCE skipped (`—`).  ← phase-1 default.
#   IPE_SWEEP_FORCE=1       → override the (opt-in) night gate + mem-guard warn.
#   IPE_SWEEP_NIGHT_GATE=1  → re-enable the local 22:00–08:00 BRT deferral window.
#   RUST_EXAMPLES="01-… 19-…" → subset override (paths or basenames).
#
# Exit: 0 = no RED row · 1 = a RED row · 2 = setup/gate.
set -uo pipefail

# ── Env + manifest + shared checks (SINGLE SOURCE OF TRUTH under lib/) ───────
source "$(dirname "$0")/../lib/env.sh"
source "$(dirname "$0")/../lib/examples.sh"
source "$(dirname "$0")/../lib/checks.sh"

# ── Night gate (OPT-IN via IPE_SWEEP_NIGHT_GATE=1; off by default so CI runs) ─
night_guard "examples-sweep"

if [ -z "$REPO" ] || [ ! -f "$REPO/scripts/equivalence-checks/examples-sweep.sh" ]; then
  echo "ERROR: can't locate the repo. cd into it, or set IPE_REPO=/path/to/sky-rust." >&2; exit 2
fi
cd "$REPO"
if [ ! -x "$SKYC_BIN" ]; then
  echo "ERROR: skyc binary not at '$SKYC_BIN' — build it: cargo build --release -p skyc (or set SKYC_BIN)." >&2; exit 2
fi

# ── Preflight: corrupted builds under low disk (HARD gate) ───────────────────
FREE_KB="$(df -Pk "$REPO" 2>/dev/null | awk 'NR==2{print $4}')"
if [ -n "$FREE_KB" ] && [ "$FREE_KB" -lt 5242880 ]; then
  echo "ERROR: < 5G free disk on $REPO ($((FREE_KB/1024/1024))G) — builds corrupt under ENOSPC. Free space first." >&2; exit 2
fi
# mem-guard is a DEV convenience (macOS-only in the sibling repo). Here it's a
# soft WARN only — never blocks the sweep or CI.
if ! pgrep -f 'mem-guard\.sh' >/dev/null 2>&1; then
  echo "WARN: mem-guard.sh not running — a runaway skyc/cargo can pressure host memory; watch it on a slim box." >&2
fi

# ── Mode flags ───────────────────────────────────────────────────────────────
BUILD_ONLY="${IPE_SWEEP_BUILD_ONLY:-0}"
NO_EQUIV="${IPE_SWEEP_NO_EQUIV:-0}"
[ "$BUILD_ONLY" = 1 ] && NO_EQUIV=1
if [ "$BUILD_ONLY" != 1 ]; then
  command -v curl >/dev/null 2>&1 || { echo "ERROR: curl required for RUN/EQUIVALENCE (set IPE_SWEEP_BUILD_ONLY=1)." >&2; exit 2; }
  command -v python3 >/dev/null 2>&1 || { echo "ERROR: python3 required for free_port (set IPE_SWEEP_BUILD_ONLY=1)." >&2; exit 2; }
fi
if [ "$NO_EQUIV" != 1 ]; then
  command -v go >/dev/null 2>&1 || { echo "ERROR: go required for Go≡Rust EQUIVALENCE (set IPE_SWEEP_NO_EQUIV=1 or IPE_SWEEP_BUILD_ONLY=1)." >&2; exit 2; }
fi
# rg is required by is_out_of_scope (the build_set Go-FFI filter) in EVERY mode.
command -v rg >/dev/null 2>&1 || { echo "ERROR: rg (ripgrep) required for the example-scope filter (is_out_of_scope). Install ripgrep." >&2; exit 2; }
# flock is required UNCONDITIONALLY: the cross-invocation build-serialization
# critical section (#35's race fix, below) and the per-example diagnostic-file
# STAMP-suffixing (#35b) both depend on it. This script has no `set -e`, so a
# missing `flock` would otherwise fail SILENTLY deep inside the per-example
# loop (`flock: command not found`, exit status never checked) and #35's race
# fix would silently no-op with zero signal. Fail loudly up front instead.
# util-linux ships it on Linux; macOS needs `brew install flock`; Windows/
# Git-Bash carries it via MSYS.
command -v flock >/dev/null 2>&1 || { echo "ERROR: flock required to serialize the cross-invocation shared-CARGO_TARGET_DIR build/run/equivalence span (see #35's race fix). Install util-linux (Linux) / 'brew install flock' (macOS)." >&2; exit 2; }

HIST="$HOME/.cache/sky/examples-sweep"; mkdir -p "$HIST"
# PID suffix: two invocations starting in the same UTC second (e.g. two
# concurrent progressive-development lanes sharing $HIST) would otherwise
# collide on rows-$STAMP.tsv/sweep-$STAMP.table and silently merge/interleave
# each other's report rows.
STAMP="$(date -u +%Y%m%dT%H%M%SZ)-$$"
TABLE="$HIST/sweep-$STAMP.table"
RUNLOG="$HIST/run-$STAMP.log"
say() { echo "$@" | tee -a "$RUNLOG"; }

# ── diag <example-name> <suffix> → $HIST path for a per-example diagnostic file
# STAMP-suffixed (#35b — same bug class as #35, one layer down). Two sweep
# invocations sharing this $HIST cache dir but pointed at DIFFERENT
# CARGO_TARGET_DIRs (so #35's flock below never contends between them) can
# still process the SAME example concurrently. Before this fix every
# per-example diagnostic file was keyed ONLY by bare example name
# ($HIST/$n.skyc.log, etc.), so two such invocations could open-and-truncate
# the SAME path at overlapping times — genuinely interleaving bytes from both
# processes into one file (not just "last write wins"), producing a false
# DIFFER (corrupted diff.txt/.equivalence) or a false equivalence pass (corrupted
# go.run.log read back as if it matched). $STAMP already carries the PID ($$,
# see above) — reusing it here (rather than a second, independent stamp)
# keeps every artefact from one invocation grouped under the same STAMP
# across rows-$STAMP.tsv / sweep-$STAMP.table / the per-example logs.
diag() { printf '%s/%s.%s.%s\n' "$HIST" "$1" "$STAMP" "$2"; }
say "=== ipê EXAMPLES sweep @ $STAMP (repo: $REPO · skyc: $SKYC_BIN) ==="
[ "$BUILD_ONLY" = 1 ] && say "  (IPE_SWEEP_BUILD_ONLY=1 — BUILD column only; RUN+EQUIVALENCE skipped)"
[ "$BUILD_ONLY" != 1 ] && [ "$NO_EQUIV" = 1 ] && say "  (IPE_SWEEP_NO_EQUIV=1 — BUILD+RUN; EQUIVALENCE skipped — the phase-1 default)"
[ "$NO_EQUIV" = 1 ] || [ "$WEB_OK" = 1 ] || say "  NOTE: browser stack incomplete — scenario equivalence falls back to normalised HTML body comparison (GET / → #sky-root diff via equivalence_normalize_html.py)."

# ── skyc build target for an example dir — sky.toml if present, else src/Main.ipe
# skyc's project build (src/ipe-cli/src/lib.rs build_project) needs a sky.toml to
# discover multi-module projects; the single-file build takes an entry `.ipe`.
# All vendored examples ship sky.toml EXCEPT 26-ui-showcase (which is multi-module
# but has no sky.toml — it will single-file-build and surface a real IPE-N0020 for
# its local `RegressionGates` import until a sky.toml is added upstream). See the
# port doc's TODO(verify).
skyc_build_target() {
  local d="$1"
  if [ -f "$d/sky.toml" ]; then echo "sky.toml"; else echo "src/Main.ipe"; fi
}

# ── build_rust <dir> <example> → 0=ok; sets BUILD_CELL to the failure word ───
BUILD_CELL=""
WARN_CELL=0
build_rust() {
  local d="$1" n="$2" tmo="${IPE_SWEEP_BUILD_TIMEOUT:-900}" tgt attempt ok=0
  local skyclog cargolog; skyclog="$(diag "$n" skyc.log)"; cargolog="$(diag "$n" cargo.log)"
  tgt="$(skyc_build_target "$d")"
  for attempt in 1 2 3 4; do
    # --runtime is left to skyc's auto-resolve (walks up to $REPO/src/runtime/rust/src/
    # sky_runtime); IPE_RUNTIME_DIR is exported by env.sh as a belt-and-braces.
    if ( cd "$d" && timeout "$tmo" "$SKYC_BIN" build "$tgt" --out sky-out/rust >"$skyclog" 2>&1 ); then
      ok=1; break
    fi
    # Transient cargo registry / network flake — back off + retry.
    if [ "$attempt" -lt 4 ] && \
       grep -qiE 'unable to update registry|download of .* failed|curl failed|HTTP2 framing|spurious network error|Connection reset|operation timed out|failed to get response' "$skyclog"; then
      sleep 5; continue
    fi
    break
  done
  if [ "$ok" != 1 ]; then
    BUILD_CELL="skyc-fail"; return 1
  fi
  # cargo build the emitted crate. The vendored runtime carries
  # `#![allow(unused, non_snake_case)]` (generated-code suppression), so a warning
  # that LEAKS PAST that allow is a genuine codegen defect — counted + gated.
  if ( cd "$d" && timeout 900 cargo build --manifest-path sky-out/rust/Cargo.toml >"$cargolog" 2>&1 ); then
    WARN_CELL="$(rg -o 'generated [0-9]+ warning' "$cargolog" 2>/dev/null | rg -o '[0-9]+' | tail -1)"
    : "${WARN_CELL:=0}"
    BUILD_CELL="ok"; return 0
  fi
  BUILD_CELL="cargo-fail"; return 1
}

# ── build_go <dir> <example> → 0=ok (binary at $d/sky-out/app), 1=fail ──────
# PHASED: only reachable when NO_EQUIV=0. Go reference = the Haskell `sky`.
# Resolution order: $IPE_GO_BIN → pinned $REPO/tools/oracle/bin/sky (the
# v0.17.3 sky-linux-x64 release, fetched not committed) → `sky` on PATH.
# The pinned binary keeps the reference at the PORT-TARGET version so we never
# read v0.16↔v0.17 stdlib skew as a parity failure.
build_go() {
  local d="$1" n="$2" go_bin="${IPE_GO_BIN:-}"
  if [ -z "$go_bin" ]; then
    if [ -x "$REPO/tools/oracle/bin/sky" ]; then go_bin="$REPO/tools/oracle/bin/sky"; else go_bin="sky"; fi
  fi
  command -v "$go_bin" >/dev/null 2>&1 || return 1
  # IPE_RUNTIME_DIR is a skyc-ONLY knob (env.sh exports it so skyc's --runtime
  # auto-resolve is CWD-independent). The Haskell `sky` Go reference ALSO honours
  # IPE_RUNTIME_DIR — and would vendor the REPO's *Rust* runtime tree as its Go
  # `rt/` package, yielding `undefined: rt.SetPortDefault` (every rt.* symbol) at
  # `go build`. `sky` has its own TH-embedded Go runtime, so the reference build
  # MUST run with IPE_RUNTIME_DIR unset. `env -u` scopes the unset to this child.
  ( cd "$d" && env -u IPE_RUNTIME_DIR timeout 300 "$go_bin" build src/Main.ipe >"$(diag "$n" go.build.log)" 2>&1 )
  sync
  [ -x "$d/sky-out/app" ]
}

norm() { grep -v '^[[:space:]]*$' "$1" 2>/dev/null | head -200; }

go_stdout_deterministic() {
  local gb="$1" l="$2"
  exercise_cli "$gb" "$l.1" >/dev/null 2>&1 || true
  exercise_cli "$gb" "$l.2" >/dev/null 2>&1 || true
  diff <(norm "$l.1") <(norm "$l.2") >/dev/null 2>&1
}

# ── EQUIVALENCE for one example (PHASED — dormant while NO_EQUIV=1) ─────────────────
equivalence_for() {
  local d="$1" n="$2" mode="$3" rbin="$4"
  local rsl gol; rsl="$(diag "$n" rust.run.log)"; gol="$(diag "$n" go.run.log)"
  case "$mode" in
    none) printf 'n/a\t%s\n' "$(equivalence_override_reason "$d")"; return 0 ;;
  esac
  case "$mode" in
    stdout)
      exercise_cli "$rbin" "$rsl" >/dev/null 2>&1 || true
      build_go "$d" "$n" || { printf 'go-ref-broken\tGo build failed (no Haskell sky? — see port doc)\n'; return 0; }
      if ! go_stdout_deterministic "$d/sky-out/app" "$gol"; then
        printf 'n/a\tnondeterministic Go stdout (auto-probe)\n'; return 0
      fi
      local difftxt; difftxt="$(diag "$n" diff.txt)"
      if diff <(norm "$gol.1") <(norm "$rsl") >"$difftxt" 2>&1; then
        printf 'equivalence-stdout\t\n'
      else
        printf 'DIFFER\tstdout differs (see %s)\n' "$(basename "$difftxt")"
      fi
      ;;
    body)
      build_go "$d" "$n" || { printf 'go-ref-broken\tGo build failed\n'; return 0; }
      local res equivalencelog; equivalencelog="$(diag "$n" equivalence)"
      res="$(exercise_server_equiv "$d/sky-out/app" "$rbin" "$d" "$equivalencelog")"
      reap
      case "$res" in
        equivalence-body\ *) printf '%s\t\n' "$res" ;;
        equivalence-serve)   printf 'equivalence-serve\t0 comparable GET routes — both boot\n' ;;
        go-ref-broken) printf 'go-ref-broken\tGo reference did not boot+serve\n' ;;
        rust-broken)   printf 'DIFFER\tRust did not boot+serve where Go did\n' ;;
        DIFFER)        printf 'DIFFER\troute body differs (see %s)\n' "$(basename "$equivalencelog")" ;;
        *)             printf 'go-ref-broken\tequiv probe inconclusive (%s)\n' "$res" ;;
      esac
      ;;
    serve)
      local rok=1 gok=1
      exercise_server "$rbin" "$(free_port)" "$rsl" || rok=0; reap
      build_go "$d" "$n" || { printf 'go-ref-broken\tGo build failed\n'; return 0; }
      exercise_server "$d/sky-out/app" "$(free_port)" "$gol" || gok=0; reap
      if [ "$gok" = 0 ]; then printf 'go-ref-broken\tGo did not boot+serve\n'
      elif [ "$rok" = 1 ]; then printf 'equivalence-serve\t\n'
      else printf 'DIFFER\tRust did not boot+serve where Go did\n'; fi
      ;;
    pty)
      local rok=1 gok=1 rrc
      exercise_tui "$rbin" "$rsl"; rrc=$?
      if [ "$rrc" = "$EXERCISE_SKIP_RC" ]; then printf 'n/a\t%s\n' "$(grep -m1 '^SKIP' "$rsl" | sed 's/^SKIP //')"; return 0; fi
      [ "$rrc" = 0 ] || rok=0
      build_go "$d" "$n" || { printf 'go-ref-broken\tGo build failed\n'; return 0; }
      exercise_tui "$d/sky-out/app" "$gol" || gok=0
      if [ "$gok" = 0 ]; then printf 'go-ref-broken\tGo TUI panicked\n'
      elif [ "$rok" = 1 ]; then printf 'equivalence-pty\tboth drive runtime (NOT cell-identical)\n'
      else printf 'DIFFER\tRust TUI panicked where Go did not\n'; fi
      ;;
    scenario)
      local scen rok=1 gok=1
      if ! browser_drivable "$d"; then
        exercise_server "$rbin" "$(free_port)" "$rsl" || rok=0; reap
        build_go "$d" "$n" || { printf 'go-ref-broken\tGo build failed\n'; return 0; }
        exercise_server "$d/sky-out/app" "$(free_port)" "$gol" || gok=0; reap
        if [ "$gok" = 0 ]; then printf 'go-ref-broken\tGo did not boot+serve\n'
        elif [ "$rok" = 1 ]; then printf 'equivalence-serve\tdriver cannot locate dir — boot-both floor\n'
        else printf 'DIFFER\tRust did not boot+serve where Go did\n'; fi
        return 0
      fi
      scen="$(scenario_for "$n")"
      if [ "$WEB_OK" = 1 ]; then
        # Full browser scenario — the gold standard for Sky.Live equivalence.
        exercise_live "$rbin" "$n" "$(free_port)" "$scen" "$rsl" || rok=0; reap
        build_go "$d" "$n" || { printf 'go-ref-broken\tGo build failed\n'; return 0; }
        exercise_live "$d/sky-out/app" "$n" "$(free_port)" "$scen" "$gol" || gok=0; reap
        if [ "$gok" = 0 ] && [ "$rok" = 0 ]; then printf 'go-ref-broken\tboth fail scenario\n'
        elif [ "$gok" = 0 ]; then printf 'go-ref-broken\tGo fails scenario %s\n' "$scen"
        elif [ "$rok" = 1 ]; then printf 'equivalence-scenario\t(scenario %s; APP-behaviour)\n' "$scen"
        else printf 'DIFFER\tRust fails scenario %s where Go passes\n' "$scen"; fi
      else
        # No browser stack — fall back to normalised HTML body comparison.
        # Sky.Live serves a full HTML page at GET / with id="sky-root"; the
        # normaliser collapses implementation-freedom surface (sky-id format,
        # attr order, event encoding, style delivery) so the diff is
        # behaviourally meaningful. Same exercise_server_equiv path used for
        # Sky.Http.Server body mode, but driven against a Live server.
        build_go "$d" "$n" || { printf 'go-ref-broken\tGo build failed\n'; return 0; }
        local res equivalencelog; equivalencelog="$(diag "$n" equivalence)"
        res="$(exercise_server_equiv "$d/sky-out/app" "$rbin" "$d" "$equivalencelog")"
        reap
        case "$res" in
          equivalence-body\ *) printf '%s\t(HTML-norm; no browser stack)\n' "$res" ;;
          equivalence-serve)   printf 'equivalence-serve\t0 comparable GET routes — both boot (no browser)\n' ;;
          go-ref-broken) printf 'go-ref-broken\tGo reference did not boot+serve\n' ;;
          rust-broken)   printf 'DIFFER\tRust did not boot+serve where Go did\n' ;;
          DIFFER)        printf 'DIFFER\tHTML body differs after normalisation (see %s)\n' "$(basename "$equivalencelog")" ;;
          *)             printf 'go-ref-broken\tequiv probe inconclusive (%s)\n' "$res" ;;
        esac
      fi
      ;;
    *) printf 'n/a\tunknown mode %s\n' "$mode" ;;
  esac
}

# ── RUN for one example → echoes the RUN cell + NOTE (tab-separated) ─────────
run_for() {
  local n="$1" shape="$2" bin="$3" rl; rl="$(diag "$n" run.log)"
  case "$shape" in
    cli)
      if exercise_cli "$bin" "$rl"; then printf 'ok\t\n'
      elif grep -qiE "$PANIC_RE" "$rl"; then printf 'panic\tcli panicked\n'
      elif is_live_network_cli "$n"; then printf 'skip\tcli makes a live external HTTP call — network-dependent RUN; not a Rust defect\n'
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
      if [ "$WEB_OK" = 1 ] && browser_drivable "$DCUR" && is_web_example "$DCUR"; then
        local scen; scen="$(scenario_for "$n")"
        if exercise_live "$bin" "$n" "$port" "$scen" "$rl"; then printf 'ok\t(browser round-trip, scenario %s)\n' "$scen"
        else printf 'noserve\tlive browser: %s\n' "$(grep -m1 '^FAIL' "$rl" | sed 's/^FAIL [^ ]* — //')"; fi
      else
        local lrc; exercise_server "$bin" "$port" "$rl"; lrc=$?
        if   [ "$lrc" = 0 ]; then printf 'ok\t(serves :%s)\n' "$port"
        elif [ "$lrc" = "$EXERCISE_SKIP_RC" ]; then printf 'skip\tlive bound but unreachable (macOS loopback-probe limitation)\n'
        elif grep -qiE "$PANIC_RE" "$rl"; then printf 'panic\tlive panicked\n'
        else printf 'noserve\tlive did not serve\n'; fi
      fi
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

# ── Build the example list (build_set, or RUST_EXAMPLES override) ────────────
EXAMPLES=()
if [ -n "${RUST_EXAMPLES:-}" ]; then
  for e in $RUST_EXAMPLES; do
    if [ -d "$e" ]; then EXAMPLES+=("${e%/}"); else e="examples/${e#examples/}"; EXAMPLES+=("${e%/}"); fi
  done
else
  while IFS= read -r d; do EXAMPLES+=("$d"); done < <(build_set)
fi

# ── Sweep: one row per example, columns BUILD·RUN·EQUIVALENCE (+ NOTE) ─────────────
say ""; say ">>> EXAMPLES SWEEP  (build_set DERIVED in lib/examples.sh; equivalence modes DERIVED + overrides in equivalence-classification.tsv)"
ROWS="$HIST/rows-$STAMP.tsv"; : >"$ROWS"
WARNS="$HIST/warnings-$STAMP.tsv"; : >"$WARNS"
DCUR=""

# ── Cross-invocation build serialization ─────────────────────────────────────
# skyc emits a FIXED `sky-app` binary name into the shared $CARGO_TARGET_DIR
# (see resolve_bin's comment above: "each example's cargo build writes
# $CARGO_TARGET_DIR/{debug,release}/sky-app"). If two examples-sweep.sh
# invocations race against the SAME CARGO_TARGET_DIR (e.g. two
# progressive-development lanes sharing ~/.cache/sky-rust-target), one's cargo
# build can overwrite sky-app between another's build_rust and its
# resolve_bin/RUN/EQUIVALENCE step — RUN then silently executes the WRONG example's
# binary (verified empirically: concurrent lanes made 01-hello-world's RUN
# print an unrelated example's stdout). flock the build→resolve→run→equivalence span
# per example so concurrent sweeps interleave safely instead of racing.
SWEEP_LOCK_FILE="$CARGO_TARGET_DIR/.examples-sweep-build.lock"
exec {SWEEP_LOCK_FD}>"$SWEEP_LOCK_FILE"

for d in "${EXAMPLES[@]}"; do
  n="$(basename "$d")"
  [ -f "$d/src/Main.ipe" ] || continue
  DCUR="$d"
  shape="$(example_shape "$d")"
  mode="$(equivalence_mode "$d")"
  ( cd "$d" && rm -rf sky-out .skycache .skydeps )

  build_cell=""; run_cell="—"; equivalence_cell="—"; note=""

  flock -x "$SWEEP_LOCK_FD"

  if ! build_rust "$d" "$n"; then
    build_cell="$BUILD_CELL"; note="rust build failed (see $(basename "$(diag "$n" skyc.log)") / $(basename "$(diag "$n" cargo.log)"))"
    printf '%s\t%s\t%s\t%s\t%s\n' "$n" "$build_cell" "—" "—" "$note" >>"$ROWS"
    ( cd "$d" && rm -rf sky-out .skycache .skydeps ); flock -u "$SWEEP_LOCK_FD"; continue
  fi
  build_cell="ok"
  printf '%s\t%s\n' "$n" "${WARN_CELL:-0}" >>"$WARNS"
  rbin="$(resolve_bin "$d")"

  if [ "$BUILD_ONLY" = 1 ] || [ -z "$rbin" ]; then
    [ -z "$rbin" ] && { run_cell="noserve"; note="no binary resolved after build"; }
    printf '%s\t%s\t%s\t%s\t%s\n' "$n" "$build_cell" "$run_cell" "$equivalence_cell" "$note" >>"$ROWS"
    ( cd "$d" && rm -rf sky-out .skycache .skydeps ); flock -u "$SWEEP_LOCK_FD"; continue
  fi

  IFS=$'\t' read -r run_cell run_note < <(run_for "$n" "$shape" "$rbin")

  if [ "$NO_EQUIV" = 1 ]; then
    equivalence_cell="—"; equivalence_note=""
  else
    IFS=$'\t' read -r equivalence_cell equivalence_note < <(equivalence_for "$d" "$n" "$mode" "$rbin")
  fi

  flock -u "$SWEEP_LOCK_FD"

  note="$run_note"; [ -n "$equivalence_note" ] && note="${note:+$note; }$equivalence_note"
  printf '%s\t%s\t%s\t%s\t%s\n' "$n" "$build_cell" "$run_cell" "$equivalence_cell" "$note" >>"$ROWS"
  reap
  ( cd "$d" && rm -rf sky-out .skycache .skydeps )
done

# ── Render the aligned table ─────────────────────────────────────────────────
{
  printf "%-28s %-10s %-9s %-16s %s\n" "EXAMPLE" "BUILD" "RUN" "EQUIVALENCE" "NOTE"
  printf "%-28s %-10s %-9s %-16s %s\n" "-------" "-----" "---" "-----" "----"
  while IFS=$'\t' read -r n b r e note; do
    printf "%-28s %-10s %-9s %-16s %s\n" "$n" "$b" "$r" "$e" "$note"
  done < "$ROWS"
} | tee "$TABLE" | tee -a "$RUNLOG"

# ── Verdict ──────────────────────────────────────────────────────────────────
RED=0; GREEN=0; SKIP=0; AMBER=0; RED_ROWS=""
declare -A EQ_COUNT=()
while IFS=$'\t' read -r n b r e note; do
  row_red=0
  case "$b" in skyc-fail|cargo-fail) row_red=1 ;; esac
  case "$r" in panic|hang|noserve|notty) row_red=1 ;; esac
  case "$e" in DIFFER) row_red=1 ;; esac
  if [ "$e" = go-ref-broken ]; then AMBER=$((AMBER+1)); row_red=0; fi
  case "$e" in
    equivalence-stdout)   EQ_COUNT[stdout]=$(( ${EQ_COUNT[stdout]:-0} + 1 )) ;;
    equivalence-body*)    EQ_COUNT[body]=$(( ${EQ_COUNT[body]:-0} + 1 )) ;;
    equivalence-serve)    EQ_COUNT[serve]=$(( ${EQ_COUNT[serve]:-0} + 1 )) ;;
    equivalence-scenario) EQ_COUNT[scenario]=$(( ${EQ_COUNT[scenario]:-0} + 1 )) ;;
    equivalence-pty)      EQ_COUNT[pty]=$(( ${EQ_COUNT[pty]:-0} + 1 )) ;;
    n/a)            EQ_COUNT[na]=$(( ${EQ_COUNT[na]:-0} + 1 )) ;;
    go-ref-broken)  EQ_COUNT[goref]=$(( ${EQ_COUNT[goref]:-0} + 1 )) ;;
  esac
  row_skip=0; case "$r" in skip) SKIP=$((SKIP+1)); row_skip=1 ;; esac
  if [ "$row_red" = 1 ]; then RED=$((RED+1)); RED_ROWS="$RED_ROWS $n"
  elif [ "$row_skip" = 0 ]; then GREEN=$((GREEN+1)); fi
done < "$ROWS"
TOTAL="$(wc -l < "$ROWS" | tr -d ' ')"

EQ_BREAK="stdout=${EQ_COUNT[stdout]:-0} body=${EQ_COUNT[body]:-0} scenario=${EQ_COUNT[scenario]:-0} serve=${EQ_COUNT[serve]:-0} pty=${EQ_COUNT[pty]:-0} n/a=${EQ_COUNT[na]:-0} go-ref-broken=${EQ_COUNT[goref]:-0}"

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
say "  summary: $GREEN green · $RED red · $SKIP skipped (of $TOTAL) · amber go-ref-broken=$AMBER"
say "  equivalence-mode breakdown: $EQ_BREAK"
say "  cargo warnings (past #![allow]): $WARN_TOTAL total${WARN_ROWS:+ —$WARN_ROWS}"
say "  full table: $TABLE · warnings: $WARNS"

SCORE="$HIST/scoreboard.tsv"
printf '%s\tgreen=%s\tred=%s\tskip=%s\tamber=%s\t%s\n' "$STAMP" "$GREEN" "$RED" "$SKIP" "$AMBER" "$EQ_BREAK" >>"$SCORE"

if [ "$RED" -gt 0 ] || [ "$WARN_FAIL" = 1 ]; then
  [ "$RED" -gt 0 ] && say "  RED rows (build/run/equivalence failure — investigate):${RED_ROWS}"
  [ "$WARN_FAIL" = 1 ] && say "  WARNING rows (codegen defect past #![allow]):${WARN_ROWS}"
  say ""; say "=== VERDICT: FAIL ($RED red row(s), $WARN_TOTAL cargo warning(s)) ==="
  exit 1
fi
say ""; say "=== VERDICT: PASS · no red row · $WARN_TOTAL cargo warning(s) · table=$TABLE ==="
exit 0
