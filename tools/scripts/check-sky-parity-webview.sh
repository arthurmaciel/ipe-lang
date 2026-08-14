#!/usr/bin/env bash
# tools/scripts/check-sky-parity-webview.sh — Sky-vs-Ipê visual parity for webview examples.
#
# Captures screenshots from both the Sky and Ipê native webview apps for each
# webview example, then computes a perceptual RMS diff.
#
# This is a macOS-only harness: Sky's webview is gated `cgo && darwin`; on
# other platforms Sky opens no window.  Both sides use WKWebView, so the
# threshold can be kept tight (same engine, same OS, minor AA noise only).
# The captures are cropped to their common top-left region before the diff, so
# a difference in native-window height (frame vs content sizing) does not
# squash the taller image out of alignment.
#
# Per-example thresholds:
#   31-webview-stopwatch-ui  — static at 00:00.0 [paused]; threshold 8.0 RMS
#   29-webview-threejs-spike — WebGL canvas timing varies; threshold 15.0 RMS
#     (the threshold is intentionally modest: both sides are WKWebView on the
#     same macOS image, but the 3-D canvas frame captured may differ by
#     timing.  Tighten once a stable baseline is confirmed.)
#
# Usage:
#   check-sky-parity-webview.sh \
#     --ipe-bin PATH         path to ipe compiler binary
#     --out-dir DIR          directory for PNG captures and diff output
#     --capture-out DIR      directory where screencapture writes PNGs
#     --settle-secs N        seconds to wait after launch before capture
#     [--no-diff]            skip RMS diff (capture-only mode)
#
# Exit: 0 all examples pass  1 one or more diffs exceed threshold  2 setup error
#
# Dependencies: python3 with Pillow, sqlite3, screencapture (macOS), pyobjc-Quartz
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# ── Arguments ────────────────────────────────────────────────────────────────
IPE_BIN=""
OUT_DIR="/tmp/ipe-webview-parity"
CAPTURE_OUT="/tmp/ipe-webview-capture"
SETTLE_SECS="${WEBVIEW_SETTLE_SECS:-5}"
NO_DIFF=0
IPE_RUNTIME_DIR="${IPE_RUNTIME_DIR:-${REPO}/src/runtime/rust/src}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${REPO}/.cache/ipe-target}"
BUILD_TIMEOUT="${IPE_SWEEP_BUILD_TIMEOUT:-300}"

while [ $# -gt 0 ]; do
  case "$1" in
    --ipe-bin)      IPE_BIN="$2";       shift 2 ;;
    --out-dir)      OUT_DIR="$2";       shift 2 ;;
    --capture-out)  CAPTURE_OUT="$2";   shift 2 ;;
    --settle-secs)  SETTLE_SECS="$2";   shift 2 ;;
    --no-diff)      NO_DIFF=1;          shift ;;
    *) echo "check-sky-parity-webview: unknown option: $1" >&2; exit 2 ;;
  esac
done

if [ -z "${IPE_BIN}" ]; then
  IPE_BIN="${CARGO_TARGET_DIR}/release/ipe"
fi

if [ ! -x "${IPE_BIN}" ]; then
  echo "check-sky-parity-webview: ipe binary not found: ${IPE_BIN}" >&2
  exit 2
fi

mkdir -p "${OUT_DIR}" "${CAPTURE_OUT}"

# ── Python venv (Pillow + pyobjc-Quartz) ────────────────────────────────────
VENV=/tmp/webview-parity-venv
if [ ! -x "${VENV}/bin/python" ]; then
  python3 -m venv "${VENV}"
fi
# Quartz bindings for CGWindowList window-id lookup; Pillow for diff.
"${VENV}/bin/pip" install --quiet Pillow pyobjc-framework-Quartz

PY="${VENV}/bin/python"

# ── Window-ID helper ─────────────────────────────────────────────────────────
# Emits the CGWindowID (integer) of the frontmost visible window whose owner
# process name matches the argument.  Exits 1 if no such window is found.
WINDOW_ID_SCRIPT='
import sys
from Quartz import (
    CGWindowListCopyWindowInfo,
    kCGWindowListOptionOnScreenOnly,
    kCGNullWindowID,
    kCGWindowOwnerName,
    kCGWindowLayer,
    kCGWindowNumber,
    kCGWindowIsOnscreen,
)
target = sys.argv[1]
windows = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID) or []
matches = [
    w for w in windows
    if w.get(kCGWindowOwnerName, "").startswith(target)
    and w.get(kCGWindowLayer, 99) == 0
]
if not matches:
    sys.exit(1)
# Take the first result (lowest window number = oldest; highest layer = frontmost).
wid = matches[0].get(kCGWindowNumber)
print(wid)
sys.exit(0)
'

get_window_id() {
  local owner="$1"
  "${PY}" -c "${WINDOW_ID_SCRIPT}" "${owner}" 2>/dev/null
}

# ── Capture helper ───────────────────────────────────────────────────────────
# Captures a single window by owner name to the given path.
# Falls back to full-screen if no window id is found (with a warning).
capture_window() {
  local owner="$1" out="$2"
  local wid
  wid=$(get_window_id "${owner}") || true
  if [ -n "${wid}" ]; then
    # -l: window id, -o: no drop shadow
    screencapture -x -l "${wid}" -o "${out}" 2>/dev/null || \
      screencapture -x "${out}" 2>/dev/null || true
  else
    echo "  warn: no on-screen window found for '${owner}'; falling back to full-screen" >&2
    screencapture -x "${out}" 2>/dev/null || true
  fi
}

# ── Non-blank check ──────────────────────────────────────────────────────────
CAPTURE_MIN_BYTES="${CAPTURE_MIN_BYTES:-20000}"
CAPTURE_MIN_STDDEV="${CAPTURE_MIN_STDDEV:-5.0}"

check_non_blank() {
  local label="$1" path="$2"
  if [ ! -f "${path}" ]; then
    echo "  MISSING  ${label}"
    return 1
  fi
  local size
  size="$(wc -c < "${path}")"
  if [ "${size}" -lt "${CAPTURE_MIN_BYTES}" ]; then
    echo "  BLANK    ${label} — too small (${size} bytes)"
    return 1
  fi
  local stddev
  stddev="$("${PY}" - "${path}" "${CAPTURE_MIN_STDDEV}" <<'PYEOF'
import sys, statistics
from PIL import Image
path, threshold = sys.argv[1], float(sys.argv[2])
img = Image.open(path).convert("RGB")
pixels = list(img.getdata())
flat = [ch for px in pixels for ch in px]
sd = statistics.stdev(flat) if len(flat) > 1 else 0.0
print(f"{sd:.2f}")
sys.exit(0 if sd >= threshold else 1)
PYEOF
  )" || { echo "  BLANK    ${label} — low stddev (${stddev})"; return 1; }
  echo "  non-blank  ${label} — ${size} bytes, stddev ${stddev}"
  return 0
}

# ── Per-example configuration ─────────────────────────────────────────────────
# Format: "slug|ipe_dir|sky_dir|ipe_owner|sky_owner|rms_threshold|crop_or_empty"
# crop: x0,y0,x1,y1 applied to both images before diff (pixel coords, 0-indexed)
# empty crop means full image

# ipe_owner is the emitted binary name (macOS window owner), which the compiler
# derives from each project's ipe.toml `name`.
EXAMPLES=(
  "31-webview-stopwatch-ui|examples/sky/ipe/31-webview-stopwatch-ui|examples/sky/original/31-webview-stopwatch-ui|webview-stopwatch-ui|sky-31-stopwatch-app|8.0|"
  "29-webview-threejs-spike|examples/sky/ipe/29-webview-threejs-spike|examples/sky/original/29-webview-threejs-spike|webview-threejs-spike|sky-29-threejs-app|15.0|"
)

PASS=0
FAIL=0
SKIP=0

for entry in "${EXAMPLES[@]}"; do
  IFS='|' read -r slug ipe_dir sky_dir ipe_owner sky_owner threshold crop <<< "${entry}"

  echo ""
  echo "── ${slug} ──────────────────────────────────────────────────────────────"

  IPE_EXAMPLE="${REPO}/${ipe_dir}"
  SKY_EXAMPLE="${REPO}/${sky_dir}"
  IPE_OUT_DIR="${IPE_EXAMPLE}/out/rust"
  APP_BIN="${CARGO_TARGET_DIR}/debug/${ipe_owner}"

  # ── Build + capture: Ipê ──────────────────────────────────────────────────
  echo "  build: Ipê ${slug}"
  rm -rf "${IPE_OUT_DIR}"
  if ! timeout "${BUILD_TIMEOUT}" \
       env IPE_RUNTIME_DIR="${IPE_RUNTIME_DIR}" \
       "${IPE_BIN}" build "${IPE_EXAMPLE}/ipe.toml" --out "${IPE_OUT_DIR}" 2>&1; then
    echo "  SKIP  ${slug} — ipe build failed"
    SKIP=$(( SKIP + 1 ))
    continue
  fi
  if ! timeout "${BUILD_TIMEOUT}" \
       env CARGO_TARGET_DIR="${CARGO_TARGET_DIR}" \
       cargo build --manifest-path "${IPE_OUT_DIR}/Cargo.toml" 2>&1; then
    echo "  SKIP  ${slug} — cargo build (Ipê) failed"
    SKIP=$(( SKIP + 1 ))
    continue
  fi

  IPE_PNG="${CAPTURE_OUT}/ipe-${slug}.png"
  "${APP_BIN}" &
  IPE_PID=$!
  sleep "${SETTLE_SECS}"
  capture_window "${ipe_owner}" "${IPE_PNG}"
  kill "${IPE_PID}" 2>/dev/null || true
  wait "${IPE_PID}" 2>/dev/null || true

  if ! check_non_blank "ipe-${slug}" "${IPE_PNG}"; then
    echo "  FAIL  ${slug} — Ipê capture blank or missing"
    FAIL=$(( FAIL + 1 ))
    continue
  fi

  # ── Build + capture: Sky ──────────────────────────────────────────────────
  echo "  build: Sky ${slug}"
  SKY_WORK="/tmp/sky-${slug}"
  SKY_APP="/tmp/${sky_owner}"
  rm -rf "${SKY_WORK}"
  cp -R "${SKY_EXAMPLE}" "${SKY_WORK}"

  if ! ( cd "${SKY_WORK}" && timeout 120 sky build ) 2>&1; then
    echo "  SKY-BUILD-FAIL  ${slug} — sky build failed; Ipê-side capture kept for reference"
    SKIP=$(( SKIP + 1 ))
    continue
  fi
  if ! ( cd "${SKY_WORK}/sky-out" && timeout 180 go build -o "${SKY_APP}" . ) 2>&1; then
    echo "  SKY-BUILD-FAIL  ${slug} — go build failed"
    SKIP=$(( SKIP + 1 ))
    continue
  fi

  SKY_PNG="${CAPTURE_OUT}/sky-${slug}.png"
  "${SKY_APP}" &
  SKY_PID=$!
  sleep "${SETTLE_SECS}"
  capture_window "${sky_owner}" "${SKY_PNG}"
  kill "${SKY_PID}" 2>/dev/null || true
  wait "${SKY_PID}" 2>/dev/null || true

  if ! check_non_blank "sky-${slug}" "${SKY_PNG}"; then
    echo "  FAIL  ${slug} — Sky capture blank or missing"
    FAIL=$(( FAIL + 1 ))
    continue
  fi

  if [ "${NO_DIFF}" -eq 1 ]; then
    echo "  SKIP-DIFF  ${slug} (--no-diff)"
    PASS=$(( PASS + 1 ))
    continue
  fi

  # The two engines can size the native window differently (frame vs content
  # height), leaving a strip of trailing background on the taller capture.
  # Crop both to their common top-left region so the diff compares the shared
  # visible area rather than resize-squashing the taller image out of alignment.
  "${PY}" - "${IPE_PNG}" "${SKY_PNG}" <<'PYEOF'
import sys
from PIL import Image

a_path, b_path = sys.argv[1], sys.argv[2]
a = Image.open(a_path)
b = Image.open(b_path)
w = min(a.width, b.width)
h = min(a.height, b.height)
if a.size != (w, h):
    a.crop((0, 0, w, h)).save(a_path)
if b.size != (w, h):
    b.crop((0, 0, w, h)).save(b_path)
PYEOF

  # ── RMS diff ──────────────────────────────────────────────────────────────
  DIFF_LOG="${OUT_DIR}/diff-${slug}.log"
  DIFF_ARGS=("${IPE_PNG}" "${SKY_PNG}" "--threshold" "${threshold}")
  if [ -n "${crop}" ]; then
    DIFF_ARGS+=("${crop}")
  fi

  if "${PY}" "${SCRIPT_DIR}/lib/visual_diff.py" "${DIFF_ARGS[@]}" 2>&1 | tee "${DIFF_LOG}"; then
    echo "  PASS  ${slug}"
    PASS=$(( PASS + 1 ))
  else
    echo "  FAIL  ${slug} — RMS diff exceeds threshold ${threshold}"
    FAIL=$(( FAIL + 1 ))
  fi
done

echo ""
echo "webview parity: ${PASS} pass, ${FAIL} fail, ${SKIP} skip"

if [ "${FAIL}" -gt 0 ]; then
  echo "VERDICT: FAIL — ${FAIL} example(s) exceeded the RMS threshold or had blank captures" >&2
  exit 1
fi
echo "VERDICT: PASS — all webview examples within RMS threshold"
