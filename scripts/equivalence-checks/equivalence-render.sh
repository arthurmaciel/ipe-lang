#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Go≡Rust RENDERED-OUTPUT equivalence for the heavy UI shapes — Sky.Live (HTML)
# and Sky.Tui (terminal). The cornerstone examples-sweep proves cli/stdout +
# server/body equivalence; its `live` (scenario-boot) and `pty` (tui boot-both)
# modes are deliberately WEAK and miss render regressions (a blank textarea, a
# stacked grid). This runner closes that gap with a STRICT, normalised render
# diff:
#
#   live <ex>  : build both backends, serve, GET /, extract the #sky-root view,
#                normalise away the legitimate implementation-detail differences
#                (sky-id separators, attribute order, event wire-encoding,
#                pseudo/mq/anim/tr style-delivery, masked SVG chart coords), and
#                byte-diff. Empty diff = behavioural render parity.
#   tui <ex>   : build both backends, capture the initial frame in a fixed-size
#                pty, render through a terminal emulator (pyte) to a STYLED cell
#                grid (char + fg/bg/bold/italic/underline), and diff. Catches
#                layout (grid/border/wrap) AND styling (typography/input-bg).
#
# PORTED from ../sky/runtime-rust/scripts/equiv-render.sh (#110 item 2, spec
# docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md §3.2). ADAPTED for
# this repo the same way examples-sweep.sh was (examples-sweep-port.md): the
# reference drives ONE compiler with `--backend go|rust`; here there are TWO
# binaries — the pinned Go-reference oracle (`IPE_GO_BIN`, default
# tools/oracle/bin/sky) and `ipe` (env.sh's IPE_BIN) + a `cargo build` of the
# emitted crate. Deliberately NOT inlined into examples-sweep.sh's `pty` EQUIVALENCE
# case — tui-grid equivalence lives in this standalone driver by the reference's
# own architecture; the main sweep's `pty` mode stays at the boot-both floor.
#
# Normalisation rationale lives in lib/equivalence_normalize_html.py + lib/equivalence_tui_grid.py.
# These compare BEHAVIOUR (what the user sees), not implementation — the backends
# are committed to behavioural parity, not byte-identical emission.
#
# Usage:  equivalence-render.sh live 26-ui-showcase
#         equivalence-render.sh tui  24-tui-kitchen-sink [rows]
#         equivalence-render.sh all                      # every wired example
# Exit:   0 = equivalence · 1 = DIFFER (regression) · 2 = setup/build error.
# ─────────────────────────────────────────────────────────────────────────────
set -uo pipefail

_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd -P)"
# shellcheck source=lib/env.sh
source "$_dir/../lib/env.sh"
[ -n "${REPO:-}" ] && cd "$REPO" || { echo "ERROR: run from the ipê repo (set IPE_REPO)." >&2; exit 2; }
# shellcheck source=lib/checks.sh
source "$_dir/../lib/checks.sh"   # resolve_bin (the freshest ipe-emitted binary)

GO_ORACLE="${IPE_GO_BIN:-$REPO/tools/oracle/bin/sky}"
[ -x "$GO_ORACLE" ] || { echo "ERROR: Go-reference oracle not found at $GO_ORACLE (set IPE_GO_BIN; see tools/oracle/README.md)." >&2; exit 2; }
[ -x "$IPE_BIN" ] || { echo "ERROR: ipe binary not at '$IPE_BIN' — build it: cargo build --release -p ipe (or set IPE_BIN)." >&2; exit 2; }
NORM_HTML="$_dir/../lib/equivalence_normalize_html.py"
NORM_TUI="$_dir/../lib/equivalence_tui_grid.py"
HIST="${TMPDIR:-/tmp}/sky-equivalence-render"; mkdir -p -m 700 "$HIST" 2>/dev/null || true
# Multi-user /tmp clobber guard: refuse a pre-existing symlink or a dir not owned
# by the current user (a predictable shared path is otherwise symlink-attackable).
if [ -L "$HIST" ] || [ ! -d "$HIST" ] || [ ! -O "$HIST" ]; then
  echo "ERROR: $HIST is not a directory owned by the current user (refusing to use)." >&2; exit 2
fi

# Wired examples (extend as render-heavy examples are added).
LIVE_EXAMPLES=(26-ui-showcase)
TUI_EXAMPLES=(24-tui-kitchen-sink)

free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'; }

# build_both <example-dir> -> sets GO_BIN + RUST_BIN (Go copied out; the rust build
# rm -rf's sky-out which would clobber it). Returns 1 on either build failure.
build_both() {
  local d="$1" tgt
  # Clear caches before EACH backend — building go then rust in the same dir
  # otherwise lets a stale .ipeache short-circuit poison the second build.
  ( cd "$d" && rm -rf sky-out .ipeache .skydeps ) >/dev/null 2>&1
  # env -u IPE_RUNTIME_DIR: the exported Rust-runtime dir would leak into the Go
  # reference's build and read as go-ref-broken (same fix as examples-sweep.sh).
  ( cd "$d" && env -u IPE_RUNTIME_DIR timeout 300 "$GO_ORACLE" build src/Main.ipe ) >"$HIST/go-build.log" 2>&1 || { echo "  go build failed (see $HIST/go-build.log)"; return 1; }
  GO_BIN="$HIST/$(basename "$d").gobin"; cp -f "$d/sky-out/app" "$GO_BIN" || return 1
  ( cd "$d" && rm -rf sky-out .ipeache .skydeps ) >/dev/null 2>&1
  # ipe: project build when a sky.toml exists (multi-module discovery), else the
  # single-file entry (same dispatch as examples-sweep.sh's ipe_build_target).
  if [ -f "$d/sky.toml" ]; then tgt="sky.toml"; else tgt="src/Main.ipe"; fi
  ( cd "$d" && timeout 600 "$IPE_BIN" build "$tgt" --out sky-out/rust ) >"$HIST/rust-build.log" 2>&1 || { echo "  ipe build failed (see $HIST/rust-build.log)"; return 1; }
  ( cd "$d" && timeout 900 cargo build --manifest-path sky-out/rust/Cargo.toml ) >>"$HIST/rust-build.log" 2>&1 || { echo "  cargo build failed (see $HIST/rust-build.log)"; return 1; }
  RUST_BIN="$(resolve_bin "$d")"
  [ -n "$RUST_BIN" ] && [ -x "$RUST_BIN" ] || { echo "  rust binary not found"; return 1; }
}

# serve_and_get <binary> <out.html> — serve in an isolated cwd, GET /, kill.
serve_and_get() {
  local bin="$1" out="$2" p; p="$(free_port)"
  local rd; rd="$(mktemp -d "$HIST/serve.XXXXXX")"
  ( cd "$rd" && exec env IPE_LIVE_PORT="$p" PORT="$p" \
      IPE_CONSOLE_EMBED=off \
      IPE_AUTH_TOKEN_SECRET="sky-equivalence-render-test-secret-0123456789-abcdef" \
      "$bin" ) >"$HIST/serve.log" 2>&1 &
  local pid=$!
  local i
  for i in $(seq 1 30); do
    curl -s -o "$out" "http://127.0.0.1:$p/" 2>/dev/null && [ -s "$out" ] && grep -q 'sky-root' "$out" && break
    sleep 0.4
  done
  kill -9 "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  rm -rf "$rd" 2>/dev/null
  [ -s "$out" ] && grep -q 'sky-root' "$out"
}

# ── pty capture of the initial Tui frame at a fixed winsize ──────────────────
capture_tui() { # <binary> <out.raw> <rows> <cols>
  python3 - "$1" "$2" "$3" "$4" <<'PY'
import os, pty, sys, time, fcntl, termios, struct, select
binp, outp, rows, cols = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
pid, fd = pty.fork()
if pid == 0:
    os.execv(binp, [binp])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
buf, t0 = b"", time.time()
while time.time() - t0 < 1.2:
    r, _, _ = select.select([fd], [], [], 0.1)
    if r:
        try:
            d = os.read(fd, 65536)
        except OSError:
            break
        if not d:
            break
        buf += d
open(outp, "wb").write(buf)
try:
    os.kill(pid, 9)
except OSError:
    pass
os.waitpid(pid, 0)
PY
}

equivalence_live() { # <example>
  local ex="$1" d="examples/$1"
  echo "live  $ex"
  [ -f "$d/src/Main.ipe" ] || { echo "  no such example"; return 2; }
  build_both "$d" || return 2
  serve_and_get "$GO_BIN"   "$HIST/$ex.go.html"   || { echo "  go did not serve"; return 2; }
  serve_and_get "$RUST_BIN" "$HIST/$ex.rust.html" || { echo "  rust did not serve"; return 2; }
  python3 "$NORM_HTML" "$HIST/$ex.go.html"   > "$HIST/$ex.go.norm"   || return 2
  python3 "$NORM_HTML" "$HIST/$ex.rust.html" > "$HIST/$ex.rust.norm" || return 2
  if diff "$HIST/$ex.go.norm" "$HIST/$ex.rust.norm" > "$HIST/$ex.diff" 2>&1; then
    echo "  ✓ equivalence (normalised #sky-root identical)"; return 0
  else
    echo "  ✗ DIFFER — $(rg -c '^[<>]' "$HIST/$ex.diff" || echo '?') lines (see $HIST/$ex.diff)"
    head -20 "$HIST/$ex.diff"; return 1
  fi
}

equivalence_tui() { # <example> [rows]
  local ex="$1" rows="${2:-50}" cols=80 d="examples/$1"
  echo "tui   $ex (${cols}x${rows})"
  [ -f "$d/src/Main.ipe" ] || { echo "  no such example"; return 2; }
  python3 -c "import pyte" 2>/dev/null || { echo "  SKIP (pyte not installed)"; return 0; }
  build_both "$d" || return 2
  capture_tui "$GO_BIN"   "$HIST/$ex.go.raw"   "$rows" "$cols"
  capture_tui "$RUST_BIN" "$HIST/$ex.rust.raw" "$rows" "$cols"
  python3 "$NORM_TUI" "$HIST/$ex.go.raw"   "$rows" > "$HIST/$ex.go.grid"   || return 2
  python3 "$NORM_TUI" "$HIST/$ex.rust.raw" "$rows" > "$HIST/$ex.rust.grid" || return 2
  if diff "$HIST/$ex.go.grid" "$HIST/$ex.rust.grid" > "$HIST/$ex.tdiff" 2>&1; then
    echo "  ✓ equivalence (styled cell grid identical)"; return 0
  else
    echo "  ✗ DIFFER — $(rg -c '^[<>]' "$HIST/$ex.tdiff" || echo '?') lines (see $HIST/$ex.tdiff)"
    head -20 "$HIST/$ex.tdiff"; return 1
  fi
}

fail=0
case "${1:-}" in
  live) equivalence_live "${2:?usage: equivalence-render.sh live <example>}" || fail=$? ;;
  tui)  equivalence_tui  "${2:?usage: equivalence-render.sh tui <example> [rows]}" "${3:-}" || fail=$? ;;
  all)
    for e in "${LIVE_EXAMPLES[@]}"; do equivalence_live "$e" || fail=1; done
    for e in "${TUI_EXAMPLES[@]}";  do equivalence_tui  "$e" || fail=1; done ;;
  *) echo "usage: equivalence-render.sh {live <ex> | tui <ex> [rows] | all}" >&2; exit 2 ;;
esac
exit "$fail"
