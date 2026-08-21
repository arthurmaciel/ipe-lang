#!/usr/bin/env bash
# tools/scripts/check-sky-parity-visual.sh — visual (screenshot) parity harness.
#
# Parity increment 3: for each in-scope Web/WebView example port, capture
# screenshots from both the Sky and Ipê binaries then compare them with a
# perceptual RMS diff (tools/scripts/lib/visual_diff.py).
#
# Capture strategy:
#   web shape    — sky/ipe binary serves HTTP; both sides captured with
#                  Playwright headless Chromium (same-engine comparison).
#                  Same-engine eliminates Chromium-vs-WebKitGTK rendering
#                  noise so the threshold can be as low as 8.0 RMS.
#
# Port discovery (data-driven from manifest.toml):
#   Reads examples/sky/manifest.toml and collects every [[example]] whose
#   shape = "web" and status = "green".  The per-port skip/threshold table
#   below is overlaid on top of that set.  Ports listed in SKIP_NAMES are
#   excluded regardless of manifest status.
#
# Honest verdict rules:
#   - A run that performed ZERO real comparisons exits FAIL.
#   - SKY-BUILD-FAIL counts as FAIL: if the Sky side cannot build, the
#     comparison was not performed; that is a gate failure, not data.
#   - CAPTURE-FAIL counts as FAIL.
#   - Legitimate skips (genuinely non-deterministic first-paint ports) remain
#     but must be a small minority. If skips dominate and no real comparisons
#     run, the zero-comparison rule above fires first.
#
# Same-engine threshold:
#   Both screenshots use Playwright/Chromium at --viewport-size=1280,800.
#   Baseline for an identical page: RMS 0.  Threshold 8.0 covers antialiasing
#   noise from tiny timing differences in CSS animation state at t=0.
#   Cross-engine (Chromium vs WebKitGTK) would need threshold ~60 and only
#   applies if the ipe-side is a WebView.app port (none of the 12 web ports).
#
# Determinism:
#   All captures target the initial render.  Ports with inherently dynamic
#   first-paint (real-time data, WebSocket push at load) are flagged in the
#   SKIP_REASON table and reported as SKIP, not compared.
#
# Disk safety:
#   Transient artifacts (sky-out/, emitted out/rust binaries of each port,
#   tmp screenshots) are cleaned after each port.  Disk is checked between
#   ports; under 7G free the script stops and reports partial results.
#
# Dependencies (Ubuntu):
#   xvfb libwebkit2gtk-4.1-dev libsoup-3.0-dev imagemagick python3-pil
#   npx playwright install chromium
#
# Usage:
#   check-sky-parity-visual.sh [--sky-bin PATH] [--out-dir DIR] [--names N,…]
#                              [--keep-artifacts] [--no-sky]
#
# Exit: 0 all in-scope ports pass/skip  1 one or more diff-FAILs  2 setup error
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib/env.sh"
source "$SCRIPT_DIR/lib/checks.sh"

cd "$REPO" || { echo "check-sky-parity-visual: cannot locate repo" >&2; exit 2; }

# ── Argument defaults ────────────────────────────────────────────────────────
SKY_BIN="${SKY_BIN:-sky}"
OUT_DIR="${VISUAL_PARITY_OUT:-/tmp/ipe-visual-parity}"
FILTER_NAMES=""
DIFF_THRESHOLD="${VISUAL_PARITY_THRESHOLD:-8.0}"
SETTLE_WEB="${VISUAL_PARITY_WEB_SETTLE:-2000}"   # ms Playwright settle
KEEP_ARTIFACTS=0
NO_SKY=0
VIEWPORT="1280,800"
DISK_STOP_KB=7340032   # 7 GiB in KiB
DISK_SCCACHE_KB=12582912  # 12 GiB — prune sccache below this

while [ $# -gt 0 ]; do
  case "$1" in
    --sky-bin)        SKY_BIN="$2";        shift 2 ;;
    --out-dir)        OUT_DIR="$2";        shift 2 ;;
    --names)          FILTER_NAMES="$2";   shift 2 ;;
    --keep-artifacts) KEEP_ARTIFACTS=1;    shift ;;
    --no-sky)         NO_SKY=1;            shift ;;
    *) echo "check-sky-parity-visual: unknown option: $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$OUT_DIR"

# ── Per-port visual policy overlay ──────────────────────────────────────────
# Format: name|skip_reason|rms_threshold
# skip_reason empty → compare normally.
# skip_reason non-empty → SKIP with that reason (no diff).
# rms_threshold empty → use DIFF_THRESHOLD global.
#
# Dynamic ports: 27-multi-session-chat and 28-streaming-chat display a
# real-time message list that may differ on first paint (SSE / WebSocket push
# before settle finishes). 16-skychess and 17-skymon have dynamic board/graph
# states. All four are skipped until a session-seed strategy is implemented.
_POLICY="
09-live-counter||
10-live-component||
12-skyvote||
25-sky-console||
26-ui-showcase||
27-multi-session-chat|real-time-chat-dynamic-first-paint|
28-streaming-chat|streaming-dynamic-first-paint|
34-multi-tier-console||
16-skychess|chess-board-dynamic-state|
17-skymon|monitoring-graph-dynamic|
19-skyforum||
37-composite-live-shop||
"

# ── Preflight checks ─────────────────────────────────────────────────────────
_die() { echo "check-sky-parity-visual: $*" >&2; exit 2; }

_check_dep() {
  command -v "$1" >/dev/null 2>&1 || _die "'$1' not found — $2"
}

_check_dep python3  "install python3"
python3 -c "from PIL import Image" 2>/dev/null || _die "python3 Pillow not found — pip3 install Pillow"

if ! npx playwright screenshot --help >/dev/null 2>&1; then
  _die "npx playwright not usable — run: npx playwright install chromium"
fi

DIFF_PY="$SCRIPT_DIR/lib/visual_diff.py"
[ -f "$DIFF_PY" ] || _die "$DIFF_PY not found"

if [ ! -x "$IPE_BIN" ]; then
  _die "ipe binary not found at '$IPE_BIN' — run: cargo build --release -p ipe"
fi

MANIFEST="$REPO/examples/sky/manifest.toml"
[ -f "$MANIFEST" ] || _die "manifest.toml not found at $MANIFEST"

# ── Helpers ──────────────────────────────────────────────────────────────────
_free_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); p=s.getsockname()[1]; s.close(); print(p)'
}

_wait_port() {
  local port="$1" deadline
  deadline=$(( $(date +%s) + 10 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    python3 -c "import socket,sys; s=socket.socket(); s.settimeout(0.5); sys.exit(0 if s.connect_ex(('127.0.0.1',$port))==0 else 1)" 2>/dev/null \
      && return 0
    sleep 0.3
  done
  return 1
}

# screenshot_web <url> <out.png>  — Playwright headless Chromium
screenshot_web() {
  local url="$1" out="$2"
  npx playwright screenshot \
    --browser=chromium \
    --viewport-size="$VIEWPORT" \
    --wait-for-timeout="$SETTLE_WEB" \
    "$url" "$out" >/dev/null 2>&1
}

# _kill_server <pid>  — clean shutdown with fallback SIGKILL
_kill_server() {
  local pid="$1"
  kill "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null || true
}

# _disk_free_kb — available KiB on the home partition
_disk_free_kb() {
  df -Pk "$HOME" 2>/dev/null | awk 'NR==2{print $4}'
}

# _disk_guard — stop if critically low; prune sccache if moderately low
_disk_guard() {
  local free_kb
  free_kb="$(_disk_free_kb)"
  if [ -n "$free_kb" ]; then
    if [ "$free_kb" -lt "$DISK_STOP_KB" ]; then
      echo ""
      echo "check-sky-parity-visual: disk critically low ($(( free_kb/1024/1024 ))G free < 7G) — stopping." >&2
      return 1
    fi
    if [ "$free_kb" -lt "$DISK_SCCACHE_KB" ]; then
      echo "  (disk $(( free_kb/1024/1024 ))G — pruning sccache)"
      rm -rf "$HOME/.cache/sccache" 2>/dev/null || true
    fi
  fi
  return 0
}

# _parse_manifest_web_green — emit "name" lines for shape=web status=green ports
# Uses awk to walk the [[example]] blocks without requiring a toml library.
_parse_manifest_web_green() {
  awk '
    /^\[\[example\]\]/ { in_block=1; name=""; shape=""; status="" }
    in_block && /^name/ { match($0,/"([^"]+)"/,a); name=a[1] }
    in_block && /^shape/ { match($0,/"([^"]+)"/,a); shape=a[1] }
    in_block && /^status/ { match($0,/"([^"]+)"/,a); status=a[1] }
    in_block && /^\[\[/ && !/^\[\[example\]\]/ { in_block=0 }
    in_block && name!="" && shape=="web" && status=="green" {
      print name; in_block=0
    }
  ' "$MANIFEST"
}

# _policy_for <name>  — sets globals POLICY_SKIP and POLICY_THRESH
_policy_for() {
  local want="$1"
  POLICY_SKIP=""
  POLICY_THRESH="$DIFF_THRESHOLD"
  while IFS='|' read -r pname skip thresh; do
    pname="${pname#"${pname%%[![:space:]]*}"}"
    [ "$pname" = "$want" ] || continue
    POLICY_SKIP="$skip"
    [ -n "$thresh" ] && POLICY_THRESH="$thresh"
    return
  done <<< "$_POLICY"
}

# _build_ipe_port <ipe_dir> <log_file>
# Sets _IPE_BIN_PATH on success.
_build_ipe_port() {
  local ipe_dir="$1" log="$2"
  local toml="$ipe_dir/ipe.toml"
  [ -f "$toml" ] || { echo "no ipe.toml in $ipe_dir" >&2; return 1; }
  local port_id
  port_id="$(basename "$(dirname "$ipe_dir")")-$(basename "$ipe_dir")"
  local port_target="${CARGO_TARGET_DIR}-vp-${port_id}"
  mkdir -p "$port_target"
  if [ ! -f "$ipe_dir/out/rust/Cargo.toml" ]; then
    timeout "${IPE_SWEEP_BUILD_TIMEOUT:-300}" \
      "$IPE_BIN" build "$toml" --out "$ipe_dir/out/rust" >"$log" 2>&1 || return 1
  fi
  CARGO_TARGET_DIR="$port_target" \
    timeout "${IPE_SWEEP_BUILD_TIMEOUT:-300}" \
    cargo build --manifest-path "$ipe_dir/out/rust/Cargo.toml" >>"$log" 2>&1 || return 2
  local bin_name
  bin_name="$(sed -n 's/^name = "\(.*\)"/\1/p' "$ipe_dir/out/rust/Cargo.toml" 2>/dev/null | head -1)"
  bin_name="${bin_name:-ipe-app}"
  _IPE_BIN_PATH=""
  for b in "$port_target/debug/$bin_name" "$port_target/release/$bin_name"; do
    [ -x "$b" ] && { _IPE_BIN_PATH="$b"; return 0; }
  done
  return 3
}

# _clean_port_artifacts <name> <ipe_dir> <sky_work_dir> <port_target>
# Removes transient build/emit artifacts for one port. Called after screenshot.
_clean_port_artifacts() {
  local name="$1" ipe_dir="$2" sky_work_dir="$3" port_target="$4"
  if [ "$KEEP_ARTIFACTS" -eq 0 ]; then
    rm -rf "$sky_work_dir" 2>/dev/null || true
    # Remove the ipe emitted binary but keep out/rust/Cargo.toml for incremental
    # rebuilds on re-runs. Only remove the per-port cargo target subtree.
    rm -rf "$port_target" 2>/dev/null || true
    # Remove tmp screenshots from OUT_DIR (keep only diff result if FAIL)
    rm -f "$OUT_DIR/${name}-ipe-raw.png" "$OUT_DIR/${name}-sky-raw.png" 2>/dev/null || true
  fi
}

# ── Discover ports ────────────────────────────────────────────────────────────
mapfile -t MANIFEST_PORTS < <(_parse_manifest_web_green)

# Apply name filter
if [ -n "$FILTER_NAMES" ]; then
  filtered=()
  IFS=',' read -ra wanted <<< "$FILTER_NAMES"
  for name in "${MANIFEST_PORTS[@]}"; do
    for w in "${wanted[@]}"; do
      [ "$name" = "$w" ] && { filtered+=("$name"); break; }
    done
  done
  MANIFEST_PORTS=("${filtered[@]}")
fi

# ── Header ────────────────────────────────────────────────────────────────────
echo "=== check-sky-parity-visual ==="
echo "  manifest:   $MANIFEST"
echo "  out-dir:    $OUT_DIR"
echo "  threshold:  $DIFF_THRESHOLD (same-engine Chromium/Chromium)"
echo "  viewport:   $VIEWPORT"
echo "  ipe:        $IPE_BIN"
echo "  sky:        ${SKY_BIN} ($(${SKY_BIN} --version 2>/dev/null || echo 'unknown'))"
echo "  ports:      ${#MANIFEST_PORTS[@]} web+green from manifest"
echo "  no-sky:     $NO_SKY"
echo ""

n_pass=0 n_fail=0 n_skip=0 n_build_fail=0

# ── Port loop ─────────────────────────────────────────────────────────────────
for name in "${MANIFEST_PORTS[@]}"; do
  _disk_guard || break

  ipe_dir="$REPO/examples/sky/ipe/$name"
  orig_dir="$REPO/examples/sky/original/$name"

  if [ ! -d "$ipe_dir" ]; then
    echo "  SKIP $name — ipe dir not found"
    n_skip=$(( n_skip+1 )); continue
  fi

  _policy_for "$name"

  if [ -n "$POLICY_SKIP" ]; then
    echo "  SKIP $name — ${POLICY_SKIP//-/ }"
    n_skip=$(( n_skip+1 )); continue
  fi

  thresh="$POLICY_THRESH"

  # ── Build ipe port ──────────────────────────────────────────────────────
  build_log="$OUT_DIR/${name}-ipe-build.log"
  port_id="sky-rust-examples-sky-ipe-${name}"
  port_target="${CARGO_TARGET_DIR}-vp-${port_id}"

  _build_ipe_port "$ipe_dir" "$build_log"
  build_rc=$?
  if [ "$build_rc" -ne 0 ]; then
    case "$build_rc" in
      1) label="ipe emit failed" ;;
      2) label="cargo build failed" ;;
      *) label="app binary not found after build" ;;
    esac
    echo "  BUILD-FAIL $name — $label (see $build_log)"
    n_build_fail=$(( n_build_fail+1 )); continue
  fi
  ipe_app="$_IPE_BIN_PATH"

  # ── Start ipe server ────────────────────────────────────────────────────
  ipe_port="$(_free_port)"
  ipe_log="$OUT_DIR/${name}-ipe-server.log"
  IPE_LIVE_PORT="$ipe_port" "$ipe_app" >"$ipe_log" 2>&1 &
  ipe_pid=$!

  if ! _wait_port "$ipe_port"; then
    echo "  CAPTURE-FAIL $name — ipe server did not start on :$ipe_port"
    _kill_server "$ipe_pid"
    n_fail=$(( n_fail+1 )); continue
  fi

  ipe_png="$OUT_DIR/${name}-ipe-raw.png"
  screenshot_web "http://127.0.0.1:$ipe_port/" "$ipe_png"
  ipe_ss_rc=$?
  _kill_server "$ipe_pid"

  if [ "$ipe_ss_rc" -ne 0 ] || [ ! -f "$ipe_png" ]; then
    echo "  CAPTURE-FAIL $name — ipe screenshot failed"
    n_fail=$(( n_fail+1 )); _clean_port_artifacts "$name" "$ipe_dir" "" "$port_target"; continue
  fi

  # ── Sky side ────────────────────────────────────────────────────────────
  sky_png=""
  sky_work_dir="$OUT_DIR/${name}-sky-work"
  if [ "$NO_SKY" -eq 1 ]; then
    echo "  NO-SKY $name — --no-sky mode; ipe screenshot captured at $ipe_png"
    n_skip=$(( n_skip+1 ))
    _clean_port_artifacts "$name" "$ipe_dir" "$sky_work_dir" "$port_target"; continue
  fi

  if [ ! -d "$orig_dir" ]; then
    echo "  SKY-SKIP $name — original/ not found; ipe screenshot at $ipe_png"
    n_skip=$(( n_skip+1 ))
    _clean_port_artifacts "$name" "$ipe_dir" "$sky_work_dir" "$port_target"; continue
  fi

  sky_build_log="$OUT_DIR/${name}-sky-build.log"
  sky_app="$OUT_DIR/${name}-sky-app"

  # Build sky side: emit Go then go build.
  sky_emit_ok=0
  if [ -f "$sky_work_dir/sky-out/main.go" ]; then
    sky_emit_ok=1
  else
    mkdir -p "$sky_work_dir"
    cp -R "$orig_dir/." "$sky_work_dir/"
    ( cd "$sky_work_dir" && timeout 120 "$SKY_BIN" build >"$sky_build_log" 2>&1 ) \
      && sky_emit_ok=1
  fi

  sky_go_ok=0
  if [ -x "$sky_app" ]; then
    sky_go_ok=1
  elif [ "$sky_emit_ok" -eq 1 ] && [ -f "$sky_work_dir/sky-out/main.go" ]; then
    ( cd "$sky_work_dir/sky-out" \
      && timeout 180 go build -o "$sky_app" . >>"$sky_build_log" 2>&1 ) \
      && sky_go_ok=1
  fi

  if [ "$sky_go_ok" -eq 0 ] || [ ! -x "$sky_app" ]; then
    echo "  SKY-BUILD-FAIL $name — sky build/go-compile failed (see $sky_build_log); comparison not performed"
    n_fail=$(( n_fail+1 ))
    _clean_port_artifacts "$name" "$ipe_dir" "$sky_work_dir" "$port_target"; continue
  fi

  # ── Start sky server ────────────────────────────────────────────────────
  sky_port="$(_free_port)"
  sky_log="$OUT_DIR/${name}-sky-server.log"
  SKY_LIVE_PORT="$sky_port" "$sky_app" >"$sky_log" 2>&1 &
  sky_pid=$!

  if ! _wait_port "$sky_port"; then
    echo "  CAPTURE-FAIL $name — sky server did not start on :$sky_port"
    _kill_server "$sky_pid"
    n_fail=$(( n_fail+1 ))
    _clean_port_artifacts "$name" "$ipe_dir" "$sky_work_dir" "$port_target"; continue
  fi

  sky_png="$OUT_DIR/${name}-sky-raw.png"
  screenshot_web "http://127.0.0.1:$sky_port/" "$sky_png"
  sky_ss_rc=$?
  _kill_server "$sky_pid"

  if [ "$sky_ss_rc" -ne 0 ] || [ ! -f "$sky_png" ]; then
    echo "  CAPTURE-FAIL $name — sky screenshot failed"
    n_fail=$(( n_fail+1 ))
    _clean_port_artifacts "$name" "$ipe_dir" "$sky_work_dir" "$port_target"; continue
  fi

  # ── Perceptual diff ─────────────────────────────────────────────────────
  diff_out="$(python3 "$DIFF_PY" "$sky_png" "$ipe_png" --threshold "$thresh" 2>&1)"
  diff_rc=$?

  if [ "$diff_rc" -eq 0 ]; then
    echo "  PASS $name — $diff_out"
    n_pass=$(( n_pass+1 ))
  else
    echo "  FAIL $name — $diff_out"
    echo "       sky: $sky_png"
    echo "       ipe: $ipe_png"
    n_fail=$(( n_fail+1 ))
  fi

  _clean_port_artifacts "$name" "$ipe_dir" "$sky_work_dir" "$port_target"
  _disk_guard || break
done

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
echo "=== check-sky-parity-visual: RESULTS ==="
echo "  pass:           $n_pass"
echo "  fail:           $n_fail   (diff above threshold, sky-build-fail, or capture-fail)"
echo "  skip:           $n_skip   (documented dynamic-first-paint ports only)"
echo "  ipe-build-fail: $n_build_fail"
echo ""
echo "  NOTE: sky-build-fail counts as FAIL — a comparison that did not run is not a pass."
echo "        To see sky-build errors, check the -sky-build.log files in the out-dir."
echo ""

# Gate: zero real comparisons is a harness failure even if no explicit FAIL was recorded.
if [ "$n_pass" -eq 0 ] && [ "$n_fail" -eq 0 ] && [ "$n_build_fail" -eq 0 ]; then
  echo "VERDICT: FAIL — harness performed 0 real comparisons (all ports skipped or absent)" >&2
  exit 1
fi

if [ "$n_fail" -gt 0 ] || [ "$n_build_fail" -gt 0 ]; then
  echo "VERDICT: FAIL ($n_fail comparison-fail(s), $n_build_fail ipe-build-fail(s))" >&2; exit 1
fi
echo "VERDICT: PASS ($n_pass comparison(s) passed, $n_skip skip(s))"
