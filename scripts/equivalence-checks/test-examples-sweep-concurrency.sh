#!/usr/bin/env bash
# scripts/equivalence-checks/test-examples-sweep-concurrency.sh — automated regression test for #35b.
#
# #35 (commit 6d93e85) fixed a race where two examples-sweep.sh invocations
# sharing the SAME $CARGO_TARGET_DIR could corrupt each other's RUN/EQUIVALENCE
# results because skyc emits a fixed `sky-app` binary name into that shared
# dir. The fix: flock a per-CARGO_TARGET_DIR lock file around the
# build->resolve->run->equivalence critical section, plus PID-suffix the sweep-wide
# rows/table report files.
#
# #35b is the residual gap ONE LAYER DOWN, found by independent review: every
# PER-EXAMPLE diagnostic file ($HIST/$n.skyc.log, $n.cargo.log,
# $n.go.build.log, $n.rust.run.log, $n.go.run.log, $n.diff.txt, $n.equivalence,
# $n.run.log) was still keyed ONLY by bare example name. Two invocations
# pointed at DIFFERENT CARGO_TARGET_DIRs (so #35's flock never contends
# between them) but sharing the SAME $HIST cache dir could still race on
# these bare-named files: both processes open-and-truncate the SAME path at
# overlapping times, genuinely interleaving bytes from both processes into
# one file — not just "last write wins". That can produce a false DIFFER
# (corrupted diff.txt/.equivalence) or a false equivalence pass (corrupted
# go.run.log read back as if it matched). The fix (see scripts/examples-
# sweep.sh's `diag()` helper) STAMP-suffixes every one of these paths with
# the same per-invocation $STAMP (which already carries the PID) used for
# rows-$STAMP.tsv / sweep-$STAMP.table.
#
# THIS TEST proves the fix by actually running examples-sweep.sh TWICE,
# CONCURRENTLY, against the SAME EXAMPLE NAME, sharing the SAME $HIST (via a
# shared fake $HOME) but pointed at DIFFERENT $CARGO_TARGET_DIRs (so #35's
# flock is deliberately NOT in play — this isolates #35b's own protection).
# Each invocation drives a fake `skyc`/Go-reference toolchain (avoids needing
# a real Sky program + Haskell `sky` — this is shell-script-only infra) that
# tags every line of build/run output with a per-invocation MARKER and
# sleeps between lines to maximise any real overlapping-write window. A REAL
# (trivial, zero-dependency) `cargo build` still runs for the Rust half, so
# the test also exercises the genuine BUILD/RUN/EQUIVALENCE control flow, not a
# reimplementation of it.
#
# PASS criteria:
#   1. Each invocation's diagnostic files resolve to a DISTINCT, STAMP/PID-
#      suffixed path under the shared $HIST (not the pre-#35b bare name).
#   2. Every diagnostic file's content carries ONLY its own invocation's
#      marker/path tag — NEVER the other invocation's — proving no
#      interleave-corruption occurred despite the concurrent, overlapping
#      writes.
#   3. A static source-level guard: no `>"$HIST/$n.<ext>"` (unstamped) write
#      site survives in examples-sweep.sh.
#
# Exit: 0 = pass, 1 = a real corruption/regression, 2 = environment/setup issue.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$REPO/scripts/equivalence-checks/examples-sweep.sh"
ORIG_HOME="$HOME"
FAIL=0

fail() { echo "FAIL: $*" >&2; FAIL=1; }
info() { echo "-- $*"; }

for c in bash cargo rg curl python3 flock; do
  command -v "$c" >/dev/null 2>&1 || { echo "SKIP: '$c' not found — cannot run this test." >&2; exit 2; }
done

# ── 0. Static source guard: no unstamped $HIST/$n.<ext> WRITE site remains ──
# (Restricted to the redirect-operator shape so this can't false-positive on
# the explanatory prose comment in examples-sweep.sh itself, which legitimately
# mentions the pre-fix bare filenames in English.)
if rg -n '>\s*"\$HIST/\$n\.' "$SCRIPT" >/tmp/sweep-static-guard.$$.log 2>&1; then
  fail "static guard: unstamped \"\$HIST/\$n.<ext>\" write site(s) still present in $SCRIPT:"
  cat /tmp/sweep-static-guard.$$.log >&2
fi
rm -f "/tmp/sweep-static-guard.$$.log"

# ── 1. Scaffold two independent "lanes" sharing one fake $HOME (→ $HIST) ────
TMPROOT="$(mktemp -d "${TMPDIR:-/tmp}/sweep-concurrency-test.XXXXXX")"
TEST_HOME="$TMPROOT/home"; mkdir -p "$TEST_HOME"
EXAMPLE_NAME="corruption-probe-35b"
TARGET_A="$HOME/.cache/sky-rust-target-lane-a35b-A"
TARGET_B="$HOME/.cache/sky-rust-target-lane-a35b-B"
mkdir -p "$TARGET_A" "$TARGET_B"

cleanup() {
  if [ "$FAIL" = 0 ]; then
    rm -rf "$TMPROOT" "$TARGET_A" "$TARGET_B" 2>/dev/null || true
  else
    echo "-- FAIL: leaving artifacts for inspection: $TMPROOT (+ $TARGET_A, $TARGET_B)" >&2
  fi
}
trap cleanup EXIT

for lane in A B; do
  d="$TMPROOT/lane-$lane/examples/$EXAMPLE_NAME"
  mkdir -p "$d/src"
  # Never actually parsed (SKYC_BIN is faked below) — just needs to exist so
  # examples-sweep.sh's `[ -f "$d/src/Main.sky" ]` gate and example_shape's
  # regex scan (no Tui/Webview/Live/Server keywords here → shape=cli) pass.
  cat >"$d/src/Main.sky" <<'EOF'
module Main exposing (main)
-- #35b concurrency-corruption probe fixture — never actually compiled by skyc
-- (SKYC_BIN is faked out by the test harness).
main = ()
EOF
done

# ── 2. Fake toolchain: tags every output line with $TEST_MARKER + sleeps to
# widen the window for any genuine concurrent-write interleaving. A REAL,
# zero-dependency `cargo build` still runs for the Rust half.
mkdir -p "$TMPROOT/bin"
FAKE_SKYC="$TMPROOT/bin/fake-skyc"
FAKE_GO="$TMPROOT/bin/fake-go"

cat >"$FAKE_SKYC" <<'FAKESKYC'
#!/usr/bin/env bash
set -euo pipefail
# examples-sweep.sh invokes: fake-skyc build <target> --out <outdir>
shift || true
target="${1:-src/Main.sky}"; shift || true
outdir="sky-out/rust"
while [ $# -gt 0 ]; do
  case "$1" in
    --out) outdir="$2"; shift 2 ;;
    *) shift ;;
  esac
done
marker="${TEST_MARKER:-UNSET}"
for i in $(seq 1 40); do
  printf 'SKYC-BUILD marker=%s pid=%s i=%02d target=%s\n' "$marker" "$$" "$i" "$target"
  sleep 0.02
done
mkdir -p "$outdir/src"
cat >"$outdir/Cargo.toml" <<EOF
[package]
name = "sky-app"
version = "0.1.0"
edition = "2021"
EOF
cat >"$outdir/src/main.rs" <<EOF
fn main() { println!("RUN marker=$marker"); }
EOF
FAKESKYC
chmod +x "$FAKE_SKYC"

cat >"$FAKE_GO" <<'FAKEGO'
#!/usr/bin/env bash
set -euo pipefail
# examples-sweep.sh invokes (cwd = example dir): fake-go build src/Main.sky
marker="${TEST_MARKER:-UNSET}"
for i in $(seq 1 40); do
  printf 'GO-BUILD marker=%s pid=%s i=%02d\n' "$marker" "$$" "$i"
  sleep 0.02
done
mkdir -p sky-out
cat >sky-out/app <<EOF
#!/usr/bin/env bash
echo "RUN marker=$marker"
EOF
chmod +x sky-out/app
FAKEGO
chmod +x "$FAKE_GO"

# ── 3. Launch both invocations concurrently, no gap between them ───────────
# `exec` (not a plain call) is load-bearing: `run_lane ... &` backgrounds this
# function in a subshell, and $! captures THAT subshell's PID. Without `exec`,
# `bash "$SCRIPT"` would fork as a CHILD of the subshell — a different PID
# than $!, and examples-sweep.sh's own $$-derived STAMP would then never match
# what this test looks for. `exec` replaces the subshell's process image with
# `bash "$SCRIPT"` in place (no fork), so $! == the script's own $$.
run_lane() {
  local lane="$1" dir="$2" target="$3" logfile="$4"
  HOME="$TEST_HOME" \
  CARGO_HOME="$ORIG_HOME/.cargo" \
  RUSTUP_HOME="$ORIG_HOME/.rustup" \
  CARGO_TARGET_DIR="$target" \
  SKYC_BIN="$FAKE_SKYC" \
  SKY_GO_BIN="$FAKE_GO" \
  RUST_EXAMPLES="$dir" \
  TEST_MARKER="$lane" \
  SKY_SWEEP_FORCE=1 \
  SKY_REPO="$REPO" \
  exec bash "$SCRIPT" >"$logfile" 2>&1
}

LOG_A="$TMPROOT/out-A.log"; LOG_B="$TMPROOT/out-B.log"
run_lane A "$TMPROOT/lane-A/examples/$EXAMPLE_NAME" "$TARGET_A" "$LOG_A" &
pidA=$!
run_lane B "$TMPROOT/lane-B/examples/$EXAMPLE_NAME" "$TARGET_B" "$LOG_B" &
pidB=$!

info "lane A pid=$pidA · lane B pid=$pidB — waiting (180s ceiling)"
elapsed=0
while kill -0 "$pidA" 2>/dev/null || kill -0 "$pidB" 2>/dev/null; do
  sleep 1; elapsed=$((elapsed + 1))
  if [ "$elapsed" -ge 180 ]; then
    fail "lanes did not finish within 180s — killing"
    kill -KILL "$pidA" "$pidB" 2>/dev/null || true
    break
  fi
done
wait "$pidA" 2>/dev/null; rcA=$?
wait "$pidB" 2>/dev/null; rcB=$?
info "lane A exit=$rcA · lane B exit=$rcB"
[ "$rcA" = 0 ] || { fail "lane A (examples-sweep.sh) exited non-zero ($rcA) — see $LOG_A"; tail -n 40 "$LOG_A" >&2; }
[ "$rcB" = 0 ] || { fail "lane B (examples-sweep.sh) exited non-zero ($rcB) — see $LOG_B"; tail -n 40 "$LOG_B" >&2; }

# ── 4. Locate + verify each lane's STAMP/PID-suffixed diagnostic files ──────
HIST="$TEST_HOME/.cache/sky/examples-sweep"

find_one() { # find_one <glob> — echoes the single match or fails loudly
  local matches=()
  while IFS= read -r f; do matches+=("$f"); done < <(compgen -G "$1" 2>/dev/null || true)
  case "${#matches[@]}" in
    1) printf '%s\n' "${matches[0]}"; return 0 ;;
    0) fail "no file matched glob: $1 (STAMP-suffixing missing/regressed?)"; return 1 ;;
    *) fail "AMBIGUOUS: ${#matches[@]} files matched glob: $1 -- ${matches[*]}"; return 1 ;;
  esac
}

# must_contain_only_own_marker <file> <own-marker> <other-marker>
must_contain_only_own_marker() {
  local f="$1" own="$2" other="$3"
  [ -f "$f" ] || { fail "missing diagnostic file: $f"; return 1; }
  rg -q "marker=$own" "$f" || fail "$f: does not contain its own marker=$own (build never ran through it?)"
  if rg -q "marker=$other" "$f"; then
    fail "$f: CONTAINS THE OTHER LANE's marker=$other — interleave-corruption reproduced!"
  fi
}

verify_lane() {
  local lane="$1" other="$2" pid="$3" other_dir_tag="$4"

  local skycA cargoA gobuildA runA rustrunA gorun1 gorun2 difftxt
  skycA="$(find_one "$HIST/$EXAMPLE_NAME.*-$pid.skyc.log")" || return 0
  cargoA="$(find_one "$HIST/$EXAMPLE_NAME.*-$pid.cargo.log")" || return 0
  gobuildA="$(find_one "$HIST/$EXAMPLE_NAME.*-$pid.go.build.log")" || return 0
  runA="$(find_one "$HIST/$EXAMPLE_NAME.*-$pid.run.log")" || return 0
  rustrunA="$(find_one "$HIST/$EXAMPLE_NAME.*-$pid.rust.run.log")" || return 0
  gorun1="$(find_one "$HIST/$EXAMPLE_NAME.*-$pid.go.run.log.1")" || return 0
  gorun2="$(find_one "$HIST/$EXAMPLE_NAME.*-$pid.go.run.log.2")" || return 0
  difftxt="$(find_one "$HIST/$EXAMPLE_NAME.*-$pid.diff.txt")" || return 0

  info "lane $lane ($pid): skyc=$(basename "$skycA") cargo=$(basename "$cargoA") go.build=$(basename "$gobuildA")"

  must_contain_only_own_marker "$skycA" "$lane" "$other"
  must_contain_only_own_marker "$gobuildA" "$lane" "$other"
  must_contain_only_own_marker "$runA" "$lane" "$other"
  must_contain_only_own_marker "$rustrunA" "$lane" "$other"
  must_contain_only_own_marker "$gorun1" "$lane" "$other"
  must_contain_only_own_marker "$gorun2" "$lane" "$other"

  # cargo doesn't know about $TEST_MARKER — it tags output with the manifest
  # DIR path instead, which embeds the lane tag (lane-A / lane-B).
  [ -s "$cargoA" ] || fail "$cargoA: empty — cargo build never ran / never captured"
  if rg -q "$other_dir_tag" "$cargoA"; then
    fail "$cargoA: CONTAINS the other lane's dir tag ($other_dir_tag) — interleave-corruption reproduced!"
  fi

  # equivalence-stdout is engineered to match (fake go/rust binaries emit identical
  # text) — diff.txt should exist and be empty; still confirm it's THIS
  # lane's own file (no content check needed beyond existence/emptiness).
  [ -f "$difftxt" ] || fail "$difftxt: missing"
  [ -s "$difftxt" ] && fail "$difftxt: non-empty (expected equivalence-stdout — engineered fixture mismatch, investigate fake toolchain output)"
}

verify_lane A B "$pidA" lane-B
verify_lane B A "$pidB" lane-A

# ── 5. Cross-check: the two lanes' resolved file sets are entirely disjoint ─
mapfile -t all_a < <(compgen -G "$HIST/$EXAMPLE_NAME.*-$pidA.*" 2>/dev/null || true)
mapfile -t all_b < <(compgen -G "$HIST/$EXAMPLE_NAME.*-$pidB.*" 2>/dev/null || true)
if [ "${#all_a[@]}" -eq 0 ] || [ "${#all_b[@]}" -eq 0 ]; then
  fail "one or both lanes produced zero diagnostic files under \$HIST — setup problem, not a corruption result"
else
  for f in "${all_a[@]}"; do
    for g in "${all_b[@]}"; do
      [ "$f" = "$g" ] && fail "lane A and lane B resolved to the SAME diagnostic file: $f"
    done
  done
  info "lane A: ${#all_a[@]} diagnostic files · lane B: ${#all_b[@]} diagnostic files · zero overlap"
fi

if [ "$FAIL" = 0 ]; then
  echo ""
  echo "=== PASS: #35b — two concurrent examples-sweep.sh invocations (same \$HIST,"
  echo "    different \$CARGO_TARGET_DIR, same example name) produced fully disjoint,"
  echo "    uncorrupted per-example diagnostic files. ==="
  exit 0
else
  echo ""
  echo "=== FAIL: see above — #35b regression reproduced (or environment issue) ===" >&2
  echo "    lane A log: $LOG_A"
  echo "    lane B log: $LOG_B"
  exit 1
fi
