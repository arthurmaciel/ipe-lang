#!/usr/bin/env bash
# tools/scripts/check-sky-parity-nonvisual.sh — Increment 2: non-visual Sky↔Ipê parity.
#
# For each green, non-visual (program | console | server) port not excluded by go_ffi,
# runs both the upstream sky toolchain and the local ipe build on identical input,
# then compares output per the port's parity policy from manifest.toml:
#
#   program / console
#     Run to exit; byte-compare stdout and exit code per parity policy:
#       exact      — byte-identical stdout + exit code required
#       normalized — stdout stripped of known nondeterministic tokens before compare
#       skip       — output is intrinsically nondeterministic; printed with reason
#
#   server (verify=serve)
#     Start each toolchain on its own port; send the scripted request set from
#     examples/sky/ipe/<name>/verify.json (or e2e.json if verify.json is absent);
#     compare HTTP status code and response body for each request.
#     Sky→Ipê brand tokens ("Sky " / "sky-") are normalized before body compare.
#     A mismatch in status code or normalized body is a DIFF.
#
# Requires a "sky" binary on PATH or pointed to by SKY_BIN. Install with:
#   tools/scripts/install-sky-toolchain.sh <version>
# When the sky binary is absent the sky-side of the comparison is skipped and the
# script still exercises the ipe side, printing SKIP (no sky) per example. The
# ipe half and the diff / compare logic are always exercised regardless.
#
# Usage:
#   check-sky-parity-nonvisual.sh [--sky-bin PATH] [--names name,...] [--diff-lines N]
#                                  [--diff-dir DIR] [--ipe-only]
#
#   --sky-bin PATH   path or command name of the sky binary (default: $SKY_BIN or "sky")
#   --names list     comma-separated example names to limit the run (default: all in scope)
#   --diff-lines N   max lines of diff to print inline (default: 40)
#   --diff-dir DIR   write per-example diff files here for artifact upload (default: none)
#   --ipe-only       skip the sky side entirely; exercise ipe + compare/diff logic only
#
# Exit: 0 all compared examples matched  1 one or more mismatches  2 setup error
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib/env.sh"
source "$SCRIPT_DIR/lib/checks.sh"

cd "$REPO" || { echo "check-sky-parity-nonvisual: cannot locate repo" >&2; exit 2; }

# ── Disk guard ───────────────────────────────────────────────────────────────
FREE_KB="$(df -Pk "$REPO" 2>/dev/null | awk 'NR==2{print $4}')"
if [ -n "$FREE_KB" ] && [ "$FREE_KB" -lt 5242880 ]; then
  echo "check-sky-parity-nonvisual: < 5G free ($((FREE_KB/1024/1024))G) — aborting." >&2
  exit 2
fi

# ── Argument parsing ─────────────────────────────────────────────────────────
SKY_BIN="${SKY_BIN:-sky}"
DIFF_LINES=40
FILTER_NAMES=""
DIFF_DIR=""
IPE_ONLY=0

while [ $# -gt 0 ]; do
  case "$1" in
    --sky-bin)    SKY_BIN="$2";    shift 2 ;;
    --names)      FILTER_NAMES="$2"; shift 2 ;;
    --diff-lines) DIFF_LINES="$2"; shift 2 ;;
    --diff-dir)   DIFF_DIR="$2";   shift 2 ;;
    --ipe-only)   IPE_ONLY=1;      shift   ;;
    *) echo "check-sky-parity-nonvisual: unknown option: $1" >&2; exit 2 ;;
  esac
done

# ── Pre-flight checks ────────────────────────────────────────────────────────
SKY_AVAILABLE=0
if [ "$IPE_ONLY" -eq 0 ] && command -v "$SKY_BIN" >/dev/null 2>&1; then
  SKY_AVAILABLE=1
fi

if [ ! -x "$IPE_BIN" ]; then
  echo "check-sky-parity-nonvisual: ipe binary not found at '$IPE_BIN'" >&2
  echo "  Build it: cargo build --release -p ipe" >&2
  exit 2
fi

[ -n "$DIFF_DIR" ] && mkdir -p "$DIFF_DIR"

SKY_VER="${SKY_BIN:-sky}"
if [ "$SKY_AVAILABLE" -eq 1 ]; then
  SKY_VER="$("$SKY_BIN" --version 2>/dev/null || true)"
fi
IPE_VER="$("$IPE_BIN" --version 2>/dev/null || true)"

echo "=== check-sky-parity-nonvisual ==="
echo "  ipe: ${IPE_VER:-unknown}"
if [ "$SKY_AVAILABLE" -eq 1 ]; then
  echo "  sky: ${SKY_VER:-unknown}"
elif [ "$IPE_ONLY" -eq 1 ]; then
  echo "  sky: (skipped — --ipe-only)"
else
  echo "  sky: (absent — sky-side skipped, ipe-side + compare logic still exercised)"
fi
echo ""

# ── Manifest helpers ─────────────────────────────────────────────────────────
# _field <name> <field>: extract a string or bool field from the manifest for a named example.
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

# _nonvisual_names: names of green, non-go_ffi examples with shape in (program|console|server).
# server examples with verify=serve are included; program examples with verify=build are excluded
# (build-only means no run output to compare).
_nonvisual_names() {
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
    shape  = field("shape")
    status = field("status")
    verify = field("verify")
    go_ffi = field("go_ffi", "false")
    if (status == "green"
            and go_ffi != "true"
            and shape in ("program", "console", "server")):
        # program/console: only run-verify (build-only has no run output to compare)
        # server: only serve-verify
        if (shape in ("program", "console") and verify == "run") \
                or (shape == "server" and verify == "serve"):
            print(nm.group(1))
PYEOF
}

# ── Normalizers ──────────────────────────────────────────────────────────────
# _normalize_stdout: strip ISO 8601 timestamps and 13-digit epoch ms from CLI output.
_normalize_stdout() {
  sed -E \
    -e 's/[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?(Z|[+-][0-9:]+)?/<TIMESTAMP>/g' \
    -e 's/\b[0-9]{13}\b/<EPOCH_MS>/g'
}

# _normalize_body: strip Sky↔Ipe branding differences before body comparison.
# "Sky " → "Brand " and "sky-" → "brand-" in a case-insensitive pass so that
# a route returning "Sky HTTP Server" on the sky side and "Ipe HTTP Server" on
# the ipe side do not produce a spurious DIFF.
_normalize_body() {
  sed \
    -e 's/Sky HTTP/Brand HTTP/gI' \
    -e 's/Sky Mux/Brand Mux/gI' \
    -e 's/Ipe HTTP/Brand HTTP/gI' \
    -e 's/Ipe Mux/Brand Mux/gI' \
    -e 's/\bSky\b/Brand/gI' \
    -e 's/\bIpe\b/Brand/gI' \
    -e 's/sky-/brand-/gI' \
    -e 's/ipe-/brand-/gI'
}

# ── Build helpers ─────────────────────────────────────────────────────────────
# _build_ipe <ipe-dir> <logfile>: emit + cargo build the ipe port.
# Returns 0 on success, 1 on ipe-build failure, 2 on cargo-build failure.
_build_ipe() {
  local ipe_dir="$1" log="$2"
  local entry="$ipe_dir/ipe.toml"
  [ -f "$entry" ] || entry="$ipe_dir/src/Main.ipe"
  rm -rf "$ipe_dir/out" 2>/dev/null
  timeout "${IPE_SWEEP_BUILD_TIMEOUT:-300}" \
    "$IPE_BIN" build "$entry" --out "$ipe_dir/out/rust" >"$log" 2>&1 || return 1
  timeout "${IPE_SWEEP_BUILD_TIMEOUT:-300}" \
    cargo build --manifest-path "$ipe_dir/out/rust/Cargo.toml" >>"$log" 2>&1 || return 2
  return 0
}

# _build_sky <orig-dir> <work-dir> <logfile>: build the sky original in a temp copy.
# Populates <work-dir>/sky-out/app (or first executable under sky-out/).
# Returns 0 on success, 1 on failure, sets SKY_APP_PATH to the binary.
SKY_APP_PATH=""
_build_sky() {
  local orig_dir="$1" work_dir="$2" log="$3"
  cp -R "$orig_dir/." "$work_dir/"
  if ( cd "$work_dir" && exec timeout 300 "$SKY_BIN" build ) >"$log" 2>&1; then
    SKY_APP_PATH="$work_dir/sky-out/app"
    [ -x "$SKY_APP_PATH" ] || \
      SKY_APP_PATH="$(find "$work_dir/sky-out" -maxdepth 1 -type f -perm -u+x 2>/dev/null | head -1)"
    [ -x "$SKY_APP_PATH" ] && return 0
    echo "  (sky build produced no runnable binary)" >&2; return 1
  fi
  return 1
}

# ── Program/console parity ───────────────────────────────────────────────────
# _run_program <bin> <run-dir> <output-file>: run a CLI binary to completion.
# Returns the exit code of the binary (not the wrapper).
_run_program() {
  local bin="$1" run_dir="$2" out="$3" rc=0
  ( cd "$run_dir" && exec timeout 30 "$bin" ) >"$out" 2>/dev/null || rc=$?
  return "$rc"
}

_compare_programs() {
  local name="$1" parity="$2" sky_out="$3" sky_rc="$4" ipe_out="$5" ipe_rc="$6"

  if [ "$parity" = "skip" ]; then
    local reason
    reason="$(_field "$name" parity_skip_reason)"
    echo "  SKIP $name (program) — ${reason:-nondeterministic output}"
    n_skip=$((n_skip+1))
    rm -f "$sky_out" "$ipe_out" 2>/dev/null
    return
  fi

  local cmp_sky="$sky_out" cmp_ipe="$ipe_out"
  local norm_sky="" norm_ipe=""
  if [ "$parity" = "normalized" ]; then
    norm_sky="$(mktemp /tmp/parity-norm-sky.XXXXXX)"
    norm_ipe="$(mktemp /tmp/parity-norm-ipe.XXXXXX)"
    _normalize_stdout <"$sky_out" >"$norm_sky"
    _normalize_stdout <"$ipe_out" >"$norm_ipe"
    cmp_sky="$norm_sky"; cmp_ipe="$norm_ipe"
  fi

  local exit_ok=1 stdout_ok=1
  [ "$sky_rc" != "$ipe_rc" ] && exit_ok=0
  diff -q "$cmp_sky" "$cmp_ipe" >/dev/null 2>&1 || stdout_ok=0

  if [ "$exit_ok" -eq 1 ] && [ "$stdout_ok" -eq 1 ]; then
    echo "  OK $name (program, $parity)"
    [ "$parity" = "normalized" ] && n_normalized=$((n_normalized+1)) || n_exact=$((n_exact+1))
  else
    echo "  MISMATCH $name (program, $parity)"
    [ "$exit_ok" -eq 0 ] && echo "    exit: sky=$sky_rc  ipe=$ipe_rc"
    if [ "$stdout_ok" -eq 0 ]; then
      echo "    stdout diff (first $DIFF_LINES lines):"
      diff -u "$cmp_sky" "$cmp_ipe" | head -"$DIFF_LINES" | sed 's/^/      /'
      if [ -n "$DIFF_DIR" ]; then
        diff -u "$cmp_sky" "$cmp_ipe" >"$DIFF_DIR/${name}.stdout.diff" 2>/dev/null || true
      fi
    fi
    failed=$((failed+1))
  fi

  rm -f "$sky_out" "$ipe_out" 2>/dev/null
  [ -n "$norm_sky" ] && rm -f "$norm_sky" "$norm_ipe" 2>/dev/null || true
}

# ── Server parity ─────────────────────────────────────────────────────────────
# _load_requests <ipe-dir>: print a JSON array of request objects.
# Reads verify.json (preferred, present in most ipe/ ports) then e2e.json.
# Normalizes both schemas to a unified array of {method, path, expectStatus, expectBody[]}.
_load_requests() {
  local ipe_dir="$1"
  local vf="" ef=""
  [ -f "$ipe_dir/verify.json" ] && vf="$ipe_dir/verify.json"
  [ -f "$ipe_dir/e2e.json" ]    && ef="$ipe_dir/e2e.json"
  python3 - "$vf" "$ef" <<'PYEOF'
import json, sys

vf, ef = sys.argv[1], sys.argv[2]

def load_requests(path):
    with open(path) as f:
        data = json.load(f)
    # verify.json schema: {"requests": [...], each has method, path, expectStatus, expectBody}
    if "requests" in data:
        out = []
        for r in data["requests"]:
            out.append({
                "method":       r.get("method", "GET"),
                "path":         r["path"],
                "expectStatus": r.get("expectStatus", 200),
                "expectBody":   r.get("expectBody", []),
            })
        return out
    # e2e.json schema: {"kind": "server", "steps": [...], each has name, method, path, expectStatus, expectBodyContains}
    if "steps" in data:
        out = []
        for r in data["steps"]:
            out.append({
                "method":       r.get("method", "GET"),
                "path":         r["path"],
                "expectStatus": r.get("expectStatus", 200),
                "expectBody":   r.get("expectBodyContains", []),
            })
        return out
    return []

reqs = []
if vf:
    reqs = load_requests(vf)
elif ef:
    reqs = load_requests(ef)
print(json.dumps(reqs))
PYEOF
}

# _send_request <port> <method> <path> <output-file>: send one HTTP request, write body to <output-file>.
# Stdout: the HTTP status code. Returns 0 when curl succeeds (even on non-200).
_send_request() {
  local port="$1" method="$2" path="$3" out="$4"
  curl -s -m 15 -X "$method" \
    -o "$out" \
    -w '%{http_code}' \
    "http://127.0.0.1:${port}${path}" 2>/dev/null
}

# _run_server_requests <port> <requests-json> <result-dir>:
# Iterates the requests JSON array and writes per-step files:
#   <result-dir>/<N>.status  — numeric HTTP status code
#   <result-dir>/<N>.body    — raw response body
# Returns 0 when every request produced an HTTP response (regardless of status).
_run_server_requests() {
  local port="$1" requests="$2" result_dir="$3"
  mkdir -p "$result_dir"
  python3 - "$port" "$requests" "$result_dir" <<'PYEOF'
import json, subprocess, sys, os

port, reqs_json, result_dir = sys.argv[1], sys.argv[2], sys.argv[3]
reqs = json.loads(reqs_json)

for i, req in enumerate(reqs):
    body_file = os.path.join(result_dir, f"{i}.body")
    status_file = os.path.join(result_dir, f"{i}.status")
    method = req["method"]
    path   = req["path"]
    url    = f"http://127.0.0.1:{port}{path}"
    result = subprocess.run(
        ["curl", "-s", "-m", "15", "-X", method, "-o", body_file, "-w", "%{http_code}", url],
        capture_output=True, text=True
    )
    status = result.stdout.strip() if result.returncode == 0 else "000"
    with open(status_file, "w") as f:
        f.write(status + "\n")
PYEOF
}

# _start_server <bin> <hint-port> <logfile>: start a server binary in the background.
# Waits up to 15 s for it to accept HTTP on either the hint port or the port the
# binary logged in a "listening on …:<PORT>" line (servers whose port is hardcoded
# in their source ignore the PORT env var). Returns 0 on success, 1 on timeout.
# Sets _SERVER_PID to the background PID; sets _SERVER_ACTUAL_PORT to the port
# that responded.
_SERVER_PID=""
_SERVER_ACTUAL_PORT=""
_start_server() {
  local bin="$1" hint_port="$2" log="$3" run_dir i code lp code2 ok="" abin
  abin="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")"
  run_dir="$(mktemp -d "${TMPDIR:-/tmp}/ipe-srv.XXXXXX")"
  ( cd "$run_dir" && exec env IPE_LIVE_PORT="$hint_port" PORT="$hint_port" "$abin" ) \
    >"$log" 2>&1 </dev/null &
  _SERVER_PID=$!
  _SERVER_ACTUAL_PORT="$hint_port"
  for i in $(seq 1 30); do
    kill -0 "$_SERVER_PID" 2>/dev/null || { _SERVER_PID=""; _SERVER_ACTUAL_PORT=""; return 1; }
    code="$(curl -s -o /dev/null -m 1 -w '%{http_code}' \
      "http://127.0.0.1:${_SERVER_ACTUAL_PORT}/" 2>/dev/null || true)"
    case "$code" in [1-5][0-9][0-9]) ok=1; break ;; esac
    # Detect the port the server actually bound to from its startup log line.
    lp="$(grep -iE "listening on" "$log" 2>/dev/null \
          | grep -oE ":[0-9]+" | tail -1 | tr -d ':')"
    if [ -n "$lp" ] && [ "$lp" != "$_SERVER_ACTUAL_PORT" ]; then
      code2="$(curl -s -o /dev/null -m 1 -w '%{http_code}' \
        "http://127.0.0.1:$lp/" 2>/dev/null || true)"
      case "$code2" in
        [1-5][0-9][0-9]) _SERVER_ACTUAL_PORT="$lp"; ok=1; break ;;
      esac
    fi
    sleep 0.5
  done
  rm -rf "$run_dir" 2>/dev/null
  [ -n "$ok" ] && return 0
  kill -TERM "$_SERVER_PID" 2>/dev/null; _SERVER_PID=""; _SERVER_ACTUAL_PORT=""
  return 1
}

# _stop_server <pid>: terminate a background server.
_stop_server() {
  local pid="$1"
  [ -z "$pid" ] && return
  kill -TERM "$pid" 2>/dev/null; sleep 0.3; kill -KILL "$pid" 2>/dev/null
}

# _compare_server_results <name> <sky-result-dir> <ipe-result-dir> <requests-json>:
# Compare per-step status codes and normalized bodies.
_compare_server_results() {
  local name="$1" sky_dir="$2" ipe_dir="$3" reqs="$4" ok=1 diff_text=""
  python3 - "$name" "$sky_dir" "$ipe_dir" "$reqs" "$DIFF_DIR" "$DIFF_LINES" <<'PYEOF'
import json, sys, os, re

name, sky_dir, ipe_dir, reqs_json, diff_dir, diff_lines = \
    sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5], int(sys.argv[6])

reqs = json.loads(reqs_json)

def _normalize(text):
    # Strip Sky/Ipe branding so "Sky HTTP Server" == "Ipe HTTP Server"
    text = re.sub(r'\b(Sky|Ipe)\s+HTTP\b', 'Brand HTTP', text, flags=re.IGNORECASE)
    text = re.sub(r'\b(Sky|Ipe)\s+Mux\b', 'Brand Mux', text, flags=re.IGNORECASE)
    text = re.sub(r'\b(Sky|Ipe)\b', 'Brand', text, flags=re.IGNORECASE)
    text = re.sub(r'(sky|ipe)-', 'brand-', text, flags=re.IGNORECASE)
    return text

mismatches = []
for i, req in enumerate(reqs):
    sky_body_f  = os.path.join(sky_dir, f"{i}.body")
    sky_stat_f  = os.path.join(sky_dir, f"{i}.status")
    ipe_body_f  = os.path.join(ipe_dir, f"{i}.body")
    ipe_stat_f  = os.path.join(ipe_dir, f"{i}.status")

    def read(p):
        try:
            with open(p) as f: return f.read().strip()
        except OSError:
            return ""

    sky_status = read(sky_stat_f) or "000"
    ipe_status = read(ipe_stat_f) or "000"
    sky_body   = _normalize(read(sky_body_f))
    ipe_body   = _normalize(read(ipe_body_f))
    path       = req["path"]
    method     = req["method"]

    step_ok = True
    notes = []
    if sky_status != ipe_status:
        notes.append(f"status: sky={sky_status} ipe={ipe_status}")
        step_ok = False
    if sky_body != ipe_body:
        # Produce a line-based diff
        from difflib import unified_diff
        sky_lines = sky_body.splitlines(keepends=True)
        ipe_lines = ipe_body.splitlines(keepends=True)
        diff = list(unified_diff(sky_lines, ipe_lines,
                                 fromfile=f"sky/{name}:{path}",
                                 tofile=f"ipe/{name}:{path}"))
        notes.append(f"body diff ({min(len(diff), diff_lines)} lines):")
        notes += ["  " + l.rstrip('\n') for l in diff[:diff_lines]]
        step_ok = False

    if not step_ok:
        mismatches.append(f"    step {i}: {method} {path}")
        mismatches += [f"      {n}" for n in notes]

if mismatches:
    print("\n".join(mismatches))
    if diff_dir:
        os.makedirs(diff_dir, exist_ok=True)
        with open(os.path.join(diff_dir, f"{name}.server.diff"), "w") as f:
            f.write(f"# server parity diff: {name}\n")
            f.write("\n".join(mismatches) + "\n")
    sys.exit(1)
sys.exit(0)
PYEOF
}

# ── Main loop ────────────────────────────────────────────────────────────────
all_names="$(_nonvisual_names)"

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

n_exact=0 n_normalized=0 n_skip=0 n_build_fail=0 n_ipe_only=0 failed=0

for name in $all_names; do
  orig_dir="$REPO/examples/sky/original/$name"
  ipe_dir="$REPO/examples/sky/ipe/$name"
  shape="$(_field "$name" shape)"
  parity="$(_field "$name" parity)"; [ -z "$parity" ] && parity="exact"

  if [ ! -d "$ipe_dir" ]; then
    echo "  SKIP $name — ipe/ missing (run regen-sky-examples.sh)"
    n_skip=$((n_skip+1)); continue
  fi

  # ── Build the ipe port ───────────────────────────────────────────────────
  build_log="$(mktemp /tmp/parity-build.XXXXXX)"
  _build_ipe "$ipe_dir" "$build_log"
  build_rc=$?
  if [ "$build_rc" -ne 0 ]; then
    label="ipe build failed"; [ "$build_rc" -eq 2 ] && label="cargo build failed"
    echo "  BUILD-FAIL $name — $label"
    sed 's/^/    /' "$build_log" >&2
    rm -f "$build_log"; n_build_fail=$((n_build_fail+1)); continue
  fi
  rm -f "$build_log"

  # ── Program / console shape ──────────────────────────────────────────────
  if [ "$shape" = "program" ] || [ "$shape" = "console" ]; then

    ipe_bin="$(resolve_bin "$ipe_dir")" || {
      echo "  BUILD-FAIL $name — built ipe binary not found"
      n_build_fail=$((n_build_fail+1)); continue
    }

    ipe_out="$(mktemp /tmp/parity-ipe.XXXXXX)"
    ipe_run_dir="$(mktemp -d "${TMPDIR:-/tmp}/ipe-run.XXXXXX")"
    ipe_rc=0
    _run_program "$ipe_bin" "$ipe_run_dir" "$ipe_out" || ipe_rc=$?
    rm -rf "$ipe_run_dir" 2>/dev/null

    if [ "$SKY_AVAILABLE" -eq 0 ]; then
      echo "  IPE-ONLY $name ($parity) — sky binary absent, ipe ran (exit=$ipe_rc)"
      n_ipe_only=$((n_ipe_only+1))
      rm -f "$ipe_out" 2>/dev/null; continue
    fi

    if [ ! -d "$orig_dir" ]; then
      echo "  SKIP $name — original/ missing (run regen-sky-examples.sh)"
      n_skip=$((n_skip+1)); rm -f "$ipe_out" 2>/dev/null; continue
    fi

    if [ "$parity" = "skip" ]; then
      local_reason="$(_field "$name" parity_skip_reason)"
      echo "  SKIP $name ($parity) — ${local_reason:-nondeterministic output}"
      n_skip=$((n_skip+1)); rm -f "$ipe_out" 2>/dev/null; continue
    fi

    sky_run_dir="$(mktemp -d "${TMPDIR:-/tmp}/sky-run.XXXXXX")"
    sky_build_log="$(mktemp /tmp/parity-sky-build.XXXXXX)"
    sky_app_path=""
    sky_rc=0
    SKY_APP_PATH=""
    if _build_sky "$orig_dir" "$sky_run_dir" "$sky_build_log"; then
      sky_app_path="$SKY_APP_PATH"
    else
      echo "  SKIP $name — sky build failed"
      sed 's/^/    sky-build: /' "$sky_build_log" >&2
      rm -f "$sky_build_log"; rm -rf "$sky_run_dir" 2>/dev/null
      rm -f "$ipe_out" 2>/dev/null; n_skip=$((n_skip+1)); continue
    fi
    rm -f "$sky_build_log"

    sky_out="$(mktemp /tmp/parity-sky.XXXXXX)"
    sky_run_dir2="$(mktemp -d "${TMPDIR:-/tmp}/sky-run2.XXXXXX")"
    _run_program "$sky_app_path" "$sky_run_dir2" "$sky_out" || sky_rc=$?
    rm -rf "$sky_run_dir2" "$sky_run_dir" 2>/dev/null

    _compare_programs "$name" "$parity" "$sky_out" "$sky_rc" "$ipe_out" "$ipe_rc"
    reap 2>/dev/null
    continue
  fi

  # ── Server shape ─────────────────────────────────────────────────────────
  if [ "$shape" = "server" ]; then
    ipe_bin="$(resolve_bin "$ipe_dir")" || {
      echo "  BUILD-FAIL $name — built ipe server binary not found"
      n_build_fail=$((n_build_fail+1)); continue
    }

    requests="$(_load_requests "$ipe_dir")"
    if [ -z "$requests" ] || [ "$requests" = "[]" ]; then
      echo "  SKIP $name (server) — no verify.json or e2e.json found"
      n_skip=$((n_skip+1)); continue
    fi

    ipe_port="$(free_port)"
    if [ -z "$ipe_port" ]; then
      echo "  SKIP $name (server) — could not allocate a free port"
      n_skip=$((n_skip+1)); continue
    fi

    ipe_srv_log="$(mktemp /tmp/parity-ipe-srv.XXXXXX)"
    ipe_result_dir="$(mktemp -d /tmp/parity-ipe-res.XXXXXX)"
    _start_server "$ipe_bin" "$ipe_port" "$ipe_srv_log"
    ipe_srv_pid="$_SERVER_PID"
    ipe_actual_port="$_SERVER_ACTUAL_PORT"

    if [ -z "$ipe_srv_pid" ]; then
      echo "  BUILD-FAIL $name (server) — ipe server did not come up"
      sed 's/^/    /' "$ipe_srv_log" >&2
      rm -f "$ipe_srv_log"; rm -rf "$ipe_result_dir" 2>/dev/null
      n_build_fail=$((n_build_fail+1)); continue
    fi

    _run_server_requests "$ipe_actual_port" "$requests" "$ipe_result_dir"
    _stop_server "$ipe_srv_pid"; ipe_srv_pid=""
    rm -f "$ipe_srv_log" 2>/dev/null

    if [ "$SKY_AVAILABLE" -eq 0 ]; then
      echo "  IPE-ONLY $name (server) — sky binary absent, ipe server exercised"
      n_ipe_only=$((n_ipe_only+1))
      rm -rf "$ipe_result_dir" 2>/dev/null; continue
    fi

    if [ ! -d "$orig_dir" ]; then
      echo "  SKIP $name (server) — original/ missing (run regen-sky-examples.sh)"
      n_skip=$((n_skip+1)); rm -rf "$ipe_result_dir" 2>/dev/null; continue
    fi

    sky_port="$(free_port)"
    if [ -z "$sky_port" ]; then
      echo "  SKIP $name (server) — could not allocate a second free port"
      n_skip=$((n_skip+1)); rm -rf "$ipe_result_dir" 2>/dev/null; continue
    fi

    sky_build_dir="$(mktemp -d "${TMPDIR:-/tmp}/sky-srv-build.XXXXXX")"
    sky_build_log="$(mktemp /tmp/parity-sky-srv-build.XXXXXX)"
    SKY_APP_PATH=""
    if ! _build_sky "$orig_dir" "$sky_build_dir" "$sky_build_log"; then
      echo "  SKIP $name (server) — sky build failed"
      sed 's/^/    sky-build: /' "$sky_build_log" >&2
      rm -f "$sky_build_log"; rm -rf "$sky_build_dir" "$ipe_result_dir" 2>/dev/null
      n_skip=$((n_skip+1)); continue
    fi
    rm -f "$sky_build_log"
    sky_bin="$SKY_APP_PATH"

    sky_srv_log="$(mktemp /tmp/parity-sky-srv.XXXXXX)"
    sky_result_dir="$(mktemp -d /tmp/parity-sky-res.XXXXXX)"
    _start_server "$sky_bin" "$sky_port" "$sky_srv_log"
    sky_srv_pid="$_SERVER_PID"
    sky_actual_port="$_SERVER_ACTUAL_PORT"

    if [ -z "$sky_srv_pid" ]; then
      echo "  SKIP $name (server) — sky server did not come up"
      sed 's/^/    /' "$sky_srv_log" >&2
      rm -f "$sky_srv_log"; rm -rf "$sky_build_dir" "$sky_result_dir" "$ipe_result_dir" 2>/dev/null
      n_skip=$((n_skip+1)); continue
    fi

    _run_server_requests "$sky_actual_port" "$requests" "$sky_result_dir"
    _stop_server "$sky_srv_pid"
    rm -f "$sky_srv_log" 2>/dev/null; rm -rf "$sky_build_dir" 2>/dev/null

    if _compare_server_results "$name" "$sky_result_dir" "$ipe_result_dir" "$requests"; then
      echo "  OK $name (server)"
      n_exact=$((n_exact+1))
    else
      echo "  MISMATCH $name (server)"
      failed=$((failed+1))
    fi

    rm -rf "$sky_result_dir" "$ipe_result_dir" 2>/dev/null
    reap 2>/dev/null
    continue
  fi

  echo "  SKIP $name — shape '$shape' not handled by this script"
  n_skip=$((n_skip+1))
done

echo ""
echo "=== check-sky-parity-nonvisual: RESULTS ==="
echo "  matched (exact):      $n_exact"
echo "  matched (normalized): $n_normalized"
echo "  skipped:              $n_skip"
echo "  ipe-only:             $n_ipe_only"
echo "  build failures:       $n_build_fail"
echo "  mismatches:           $failed"
echo ""
if [ "$n_ipe_only" -gt 0 ] && [ "$SKY_AVAILABLE" -eq 0 ]; then
  echo "NOTE: sky binary was absent; $n_ipe_only example(s) exercised ipe-only."
  echo "      Full sky↔ipe comparison requires the sky toolchain (install-sky-toolchain.sh)."
  echo ""
fi

if [ "$failed" -gt 0 ]; then
  echo "VERDICT: FAIL ($failed mismatch(es))" >&2; exit 1
fi
echo "VERDICT: PASS"
