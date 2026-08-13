#!/usr/bin/env bash
# tools/scripts/check-sky-parity-visual.sh — visual (screenshot) parity harness.
#
# Parity increment 3: for each in-scope Web/WebView example port, capture
# screenshots from both the Sky and Ipê binaries then compare them with a
# perceptual RMS diff (tools/scripts/lib/visual_diff.py).
#
# Capture strategy:
#   web shape    — sky/ipe binary serves HTTP; Playwright headless Chromium
#                  navigates to localhost and screenshots the initial render.
#   webview shape — ipe-app binary runs under xvfb-run; ImageMagick `import`
#                   captures the virtual display after a settle delay.
#                   Sky webview on Linux (v≤0.16.29) is a no-op stub and
#                   produces no window; those ports are marked SKY-STUB.
#
# Determinism:
#   All captures target the initial render before any Sub.every tick fires.
#   Dynamic ports (animations, live timers) are marked `visual_parity=skip`
#   in the manifest until a clock-freeze strategy is implemented.
#
# Dependencies (Ubuntu):
#   xvfb libwebkit2gtk-4.1-dev libsoup-3.0-dev imagemagick python3-pil
#   npx playwright install chromium
#
# Usage:
#   check-sky-parity-visual.sh [--sky-bin PATH] [--out-dir DIR] [--names N,…]
#
# Exit: 0 all passes/skips  1 one or more fails  2 setup error
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
SETTLE_WEBVIEW="${VISUAL_PARITY_SETTLE:-5}"   # seconds to wait for webview paint
SETTLE_WEB="${VISUAL_PARITY_WEB_SETTLE:-2000}" # ms wait for playwright settle

while [ $# -gt 0 ]; do
  case "$1" in
    --sky-bin)  SKY_BIN="$2";      shift 2 ;;
    --out-dir)  OUT_DIR="$2";      shift 2 ;;
    --names)    FILTER_NAMES="$2"; shift 2 ;;
    *) echo "check-sky-parity-visual: unknown option: $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$OUT_DIR"

# ── Preflight ────────────────────────────────────────────────────────────────
_check_dep() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "check-sky-parity-visual: '$1' not found — $2" >&2; exit 2
  }
}
_check_dep xvfb-run "install: sudo apt-get install -y xvfb"
_check_dep import   "install: sudo apt-get install -y imagemagick"
_check_dep python3  "install python3 + python3-pil"
python3 -c "from PIL import Image" 2>/dev/null || {
  echo "check-sky-parity-visual: python3 Pillow not found — pip3 install Pillow" >&2; exit 2
}
if ! npx playwright screenshot --help >/dev/null 2>&1; then
  echo "check-sky-parity-visual: npx playwright not usable — run: npx playwright install chromium" >&2
  exit 2
fi

DIFF_PY="$SCRIPT_DIR/lib/visual_diff.py"
[ -f "$DIFF_PY" ] || { echo "check-sky-parity-visual: $DIFF_PY not found" >&2; exit 2; }

# ── Port table (name | shape | sky-capture | visual_parity | crop | rms_threshold)
# Maintained here until manifest.toml grows visual_parity fields.
# sky-capture values: "live:<port>" | "webview" | "sky-stub"
# visual_parity:  "initial-render" | "skip:<reason>"
_PORTS="
29-webview-threejs-spike|webview|sky-stub|skip:three.js-animation-not-static||8.0
31-webview-stopwatch-ui|webview|sky-stub|initial-render||8.0
38-composite-ui-multibackend|webview|live:8006|initial-render|0,70,960,720|60.0
"
# Port 38 notes:
#   sky side:  Live.app HTTP server (sky v0.16.29 builds this OK) — Playwright
#              captures at --viewport-size=960,720 to match the ipe webview window.
#   ipe side:  WebView.app window { size = (960, 720) } — xvfb + import crop.
#   crop 0,70: removes the "Day NNNNN" date header (sky = Time.unixMillis today;
#              ipe = fixed todayIndex=20000) to avoid day-boundary false-fail.
#   rms_threshold 60.0: cross-engine baseline (Chromium vs WebKitGTK on same
#              HTML) measures ~46 RMS; 60 gives headroom while catching
#              real regressions (missing element ≥ 80 RMS).

# ── Helpers ──────────────────────────────────────────────────────────────────
_build_ipe_port() {
  local ipe_dir="$1" log="$2"
  local toml="$ipe_dir/ipe.toml"
  [ -f "$toml" ] || { echo "no ipe.toml in $ipe_dir" >&2; return 1; }
  # Each port gets its own target dir so binaries never overwrite each other.
  # Derive a stable dir name from the port path (last two components).
  local port_id
  port_id="$(basename "$(dirname "$ipe_dir")")-$(basename "$ipe_dir")"
  local port_target="${CARGO_TARGET_DIR%-*}-vp-${port_id}"
  mkdir -p "$port_target"
  # Skip emit if out/rust/Cargo.toml already exists (rebuild cargo either way).
  if [ ! -f "$ipe_dir/out/rust/Cargo.toml" ]; then
    timeout "${IPE_SWEEP_BUILD_TIMEOUT:-300}" \
      "$IPE_BIN" build "$toml" --out "$ipe_dir/out/rust" >"$log" 2>&1 || return 1
  fi
  CARGO_TARGET_DIR="$port_target" \
    timeout "${IPE_SWEEP_BUILD_TIMEOUT:-300}" \
    cargo build --manifest-path "$ipe_dir/out/rust/Cargo.toml" >>"$log" 2>&1 || return 2
  # Expose the per-port target dir for resolve_bin.
  _PORT_TARGET_DIR="$port_target"
  return 0
}

# Resolve the ipe-app binary in the per-port target dir set by _build_ipe_port.
_resolve_port_bin() {
  local exe=""
  [ "${IPE_HOST_OS:-}" = windows ] && exe=".exe"
  for b in \
    "${_PORT_TARGET_DIR:-}/debug/ipe-app$exe" \
    "${_PORT_TARGET_DIR:-}/release/ipe-app$exe"; do
    [ -x "$b" ] && { echo "$b"; return 0; }
  done
  return 1
}

_free_port() {
  python3 -c 'import socket;s=socket.socket();s.bind(("",0));p=s.getsockname()[1];s.close();print(p)'
}

_wait_port() {
  local host="127.0.0.1" port="$1" deadline
  deadline=$(( $(date +%s) + 8 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    python3 -c "import socket,sys; s=socket.socket(); s.settimeout(0.5); sys.exit(0 if s.connect_ex(('$host',$port))==0 else 1)" 2>/dev/null \
      && return 0
    sleep 0.3
  done
  return 1
}

# screenshot_web <url> <out.png> [WxH]  — Playwright headless Chromium.
# The viewport size MUST match the ipe webview window size so both sides
# lay out identically before the pixel diff; default 960x720.
screenshot_web() {
  local url="$1" out="$2" vp="${3:-960,720}"
  npx playwright screenshot \
    --browser=chromium \
    --viewport-size="$vp" \
    --wait-for-timeout="$SETTLE_WEB" \
    "$url" "$out" >/dev/null 2>&1
}

# screenshot_webview <bin> <out.png>  — xvfb-run + ImageMagick import
screenshot_webview() {
  local bin="$1" out="$2"
  local settle="$SETTLE_WEBVIEW"
  local inner
  inner="import subprocess,time,os
app=subprocess.Popen(['$bin'],stdout=open('/tmp/vp-app.log','w'),stderr=subprocess.STDOUT)
time.sleep($settle)
subprocess.run(['import','-window','root','$out'],stderr=subprocess.DEVNULL)
app.terminate()
try: app.wait(timeout=3)
except: app.kill()"
  xvfb-run -a -w 2 python3 -c "$inner" >/dev/null 2>&1
  [ -f "$out" ] && [ -s "$out" ]
}

# ── Main loop ────────────────────────────────────────────────────────────────
echo "=== check-sky-parity-visual ==="
echo "  out-dir:   $OUT_DIR"
echo "  threshold: $DIFF_THRESHOLD"
echo ""

n_pass=0 n_fail=0 n_skip=0 n_stub=0 n_build_fail=0

while IFS='|' read -r name shape sky_cap vis_par crop port_thresh; do
  [ -z "$name" ] && continue
  # Strip leading/trailing whitespace
  name="${name#"${name%%[![:space:]]*}"}"
  [ -z "$name" ] && continue

  if [ -n "$FILTER_NAMES" ]; then
    found=0
    IFS=',' read -ra wanted <<< "$FILTER_NAMES"
    for w in "${wanted[@]}"; do [ "$name" = "$w" ] && { found=1; break; }; done
    [ "$found" -eq 0 ] && continue
  fi

  ipe_dir="$REPO/examples/sky/ipe/$name"
  [ -d "$ipe_dir" ] || { echo "  SKIP $name — ipe port not found"; n_skip=$((n_skip+1)); continue; }

  # visual_parity=skip
  case "$vis_par" in
    skip:*)
      reason="${vis_par#skip:}"
      echo "  SKIP $name — ${reason//-/ }"
      n_skip=$((n_skip+1)); continue ;;
  esac

  thresh="${port_thresh:-$DIFF_THRESHOLD}"

  # Build ipe port
  build_log="$OUT_DIR/${name}-build.log"
  _build_ipe_port "$ipe_dir" "$build_log"
  build_rc=$?
  if [ "$build_rc" -ne 0 ]; then
    label="ipe emit failed"; [ "$build_rc" -eq 2 ] && label="cargo build failed"
    echo "  BUILD-FAIL $name — $label (see $build_log)"
    n_build_fail=$((n_build_fail+1)); continue
  fi

  ipe_bin="$(_resolve_port_bin)" || {
    echo "  BUILD-FAIL $name — built binary not found"
    n_build_fail=$((n_build_fail+1)); continue
  }

  ipe_png="$OUT_DIR/${name}-ipe.png"
  sky_png="$OUT_DIR/${name}-sky.png"

  # Capture ipe screenshot (always webview for now — expand to web when needed)
  if ! screenshot_webview "$ipe_bin" "$ipe_png"; then
    echo "  CAPTURE-FAIL $name — ipe webview screenshot failed"
    n_fail=$((n_fail+1)); continue
  fi

  # Capture sky screenshot
  case "$sky_cap" in
    sky-stub)
      echo "  SKY-STUB $name — sky ≤0.16.29 webview is a no-op on Linux; ipe captured at $ipe_png"
      n_stub=$((n_stub+1)); continue ;;

    live:*)
      sky_port="${sky_cap#live:}"
      # Find and start the sky binary
      orig_dir="$REPO/examples/sky/original/$name"
      if [ ! -d "$orig_dir" ]; then
        echo "  SKIP $name — sky original/ not found"
        n_skip=$((n_skip+1)); continue
      fi
      if ! command -v "$SKY_BIN" >/dev/null 2>&1; then
        echo "  SKIP $name — sky binary not found ('$SKY_BIN')"
        n_skip=$((n_skip+1)); continue
      fi
      # Use a stable cache dir so sky emit + go build are not repeated each run.
      sky_run_dir="$OUT_DIR/${name}-sky-src"
      sky_app="$OUT_DIR/${name}-sky-app"
      sky_build_log="$OUT_DIR/${name}-sky-build.log"
      if [ ! -d "$sky_run_dir" ]; then
        cp -R "$orig_dir/." "$sky_run_dir/"
      fi
      # sky build emits Go source into sky-out/; skip if already emitted.
      sky_emit_ok=0
      if [ -f "$sky_run_dir/sky-out/main.go" ]; then
        sky_emit_ok=1
      else
        ( cd "$sky_run_dir" && timeout 300 "$SKY_BIN" build >"$sky_build_log" 2>&1 ) && sky_emit_ok=1
      fi
      # Compile emitted Go source; skip if binary already exists.
      sky_go_ok=0
      if [ -x "$sky_app" ]; then
        sky_go_ok=1
      elif [ "$sky_emit_ok" -eq 1 ]; then
        ( cd "$sky_run_dir/sky-out" && timeout 300 go build -o "$sky_app" . >>"$sky_build_log" 2>&1 ) && sky_go_ok=1
      fi
      if [ "$sky_go_ok" -eq 0 ] || [ ! -x "$sky_app" ]; then
        echo "  SKIP $name — sky build/go-compile failed (see $sky_build_log)"
        n_skip=$((n_skip+1)); continue
      fi
      # Start server on free port
      listen_port="$(_free_port)"
      sky_log="$OUT_DIR/${name}-sky-server.log"
      python3 -c "
import subprocess, time, os, sys
p = subprocess.Popen(
    ['$sky_app'],
    cwd='$sky_run_dir',
    env={**os.environ, 'SKY_LIVE_PORT': '$listen_port', 'PORT': '$listen_port'},
    stdout=open('$sky_log', 'w'), stderr=subprocess.STDOUT
)
time.sleep(12)
p.terminate()
try: p.wait(timeout=3)
except: p.kill()
" &
      server_pid=$!
      if ! _wait_port "$listen_port"; then
        echo "  SKIP $name — sky server did not start on :$listen_port"
        kill "$server_pid" 2>/dev/null; wait "$server_pid" 2>/dev/null
        n_skip=$((n_skip+1)); continue
      fi
      # Viewport must match ipe webview window size (default 960x720 per port table).
      screenshot_web "http://127.0.0.1:$listen_port/" "$sky_png" "960,720"
      sky_rc=$?
      kill "$server_pid" 2>/dev/null; wait "$server_pid" 2>/dev/null
      if [ "$sky_rc" -ne 0 ] || [ ! -f "$sky_png" ]; then
        echo "  CAPTURE-FAIL $name — sky web screenshot failed"
        n_fail=$((n_fail+1)); continue
      fi
      ;;

    *)
      echo "  SKIP $name — unknown sky-cap '$sky_cap'"
      n_skip=$((n_skip+1)); continue ;;
  esac

  # Run perceptual diff
  crop_arg=""; [ -n "$crop" ] && crop_arg="$crop"
  diff_out="$(python3 "$DIFF_PY" "$sky_png" "$ipe_png" ${crop_arg:+"$crop_arg"} --threshold "$thresh" 2>&1)"
  diff_rc=$?

  if [ "$diff_rc" -eq 0 ]; then
    echo "  PASS $name — $diff_out"
    n_pass=$((n_pass+1))
  else
    echo "  FAIL $name — $diff_out"
    echo "       sky:  $sky_png"
    echo "       ipe:  $ipe_png"
    n_fail=$((n_fail+1))
  fi

done <<< "$_PORTS"

echo ""
echo "=== check-sky-parity-visual: RESULTS ==="
echo "  pass:         $n_pass"
echo "  fail:         $n_fail"
echo "  sky-stub:     $n_stub"
echo "  skip:         $n_skip"
echo "  build-fail:   $n_build_fail"
echo ""

if [ "$n_fail" -gt 0 ] || [ "$n_build_fail" -gt 0 ]; then
  echo "VERDICT: FAIL ($n_fail fail(s), $n_build_fail build-fail(s))" >&2; exit 1
fi
echo "VERDICT: PASS"
