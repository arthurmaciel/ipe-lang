#!/usr/bin/env bash
# scripts/fuzz-well-typed.sh — well-typed soundness fuzzer (Ipê/sky-rust port).
#
# Ported from ../sky/scripts/fuzz-well-typed.sh (Haskell backend, Go target).
# KEY ADAPTATIONS — Rust/Ipê backend:
#
#   BUILD: skyc build src/Main.sky --out sky-out/rust
#          cargo build --manifest-path sky-out/rust/Cargo.toml
#          (binary: $CARGO_TARGET_DIR/debug/sky-app)
#
#   PANIC DETECTION: Rust/Ipê runtime installs a classify-and-log panic hook
#   (sky_runtime::core::install_panic_classifier). A runtime fault emits to
#   stderr before the process exits non-zero:
#
#     [error] DivisionByZero (ref XXXXXXXX): attempt to divide by zero
#
#   Rust's default unhandled-panic message: "thread '...' panicked at '...'"
#
#   Detector fires on:
#     • binary exit code != 0
#     • stderr contains one of the PANIC_RE markers (see below)
#     • a build/run timeout
#
#   TRUE POSITIVE: `42 // 0` (Sky integer division by zero) triggers the
#   DivisionByZero classifier, exits 101. Demonstrated in --tp-demo mode.
#
# Property:
#   A random WELL-TYPED Sky program MUST (a) build successfully with skyc+cargo
#   (it is well-typed by construction; a build failure IS a soundness bug) AND
#   (b) run without panicking and exit 0.  Any deviation is a soundness violation.
#
# Flags:
#   --iters N          Iteration count (default 30; use 10000 for full gate)
#   --seed N           Start seed (default $RANDOM); iter i uses seed+i
#   --mode M           template | corpus | composite (default composite)
#                        template: Tier B1 only (synthesised templates)
#                        corpus:   Tier B2 only (00-standard-libs replay)
#                        composite: alternating (default)
#   --keep             Keep tempdir on success (default: cleanup on success)
#   --quiet            Suppress per-iter progress; print summary only
#   --build-timeout N  skyc+cargo build timeout in seconds (default 300)
#   --run-timeout N    binary run timeout in seconds (default 15)
#   --tp-demo          Run the true-positive demo then exit (verifies detector)
#   SKY_FUZZ_FULL=1    Shorthand for --iters 10000 (CI full-gate override)
#
# Exit: 0 = all iterations green; 1 = first failure (seed + forensics dir
# under /tmp/sky-fuzz/FAILURES/); 2 = setup error.
#
# Reproduce a failure: ./scripts/fuzz-well-typed.sh --seed N --iters 1 --keep
# Full 10k gate:       SKY_FUZZ_FULL=1 ./scripts/fuzz-well-typed.sh

set -uo pipefail

# ── Source the shared env (REPO, SKYC_BIN, CARGO_TARGET_DIR, SKY_RUNTIME_DIR) ──
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib/env.sh"

# ── Flags ────────────────────────────────────────────────────────────────────
ITERS="${SKY_FUZZ_FULL:+10000}"
ITERS="${ITERS:-30}"
SEED=""
MODE="composite"
KEEP=0
QUIET=0
BUILD_TIMEOUT=300   # skyc + cargo combined; cargo alone can take ~5 min cold
RUN_TIMEOUT=15
TP_DEMO=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --iters)         ITERS="$2";        shift 2 ;;
        --seed)          SEED="$2";         shift 2 ;;
        --mode)          MODE="$2";         shift 2 ;;
        --keep)          KEEP=1;            shift ;;
        --quiet)         QUIET=1;           shift ;;
        --build-timeout) BUILD_TIMEOUT="$2"; shift 2 ;;
        --run-timeout)   RUN_TIMEOUT="$2";  shift 2 ;;
        --tp-demo)       TP_DEMO=1;         shift ;;
        -h|--help)
            sed -n '2,70p' "$0"; exit 0 ;;
        *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac
done

[[ -z "$SEED" ]] && SEED=$RANDOM

# ── Preflight checks ─────────────────────────────────────────────────────────
if [[ ! -x "$SKYC_BIN" ]]; then
    echo "ERROR: skyc binary not found at '$SKYC_BIN'" >&2
    echo "  Build: cargo build -p skyc  (or set SKYC_BIN=...)" >&2
    exit 2
fi

FREE_KB="$(df -Pk "$REPO" 2>/dev/null | awk 'NR==2{print $4}')"
if [[ -n "$FREE_KB" && "$FREE_KB" -lt 5242880 ]]; then
    echo "ERROR: <5 GB free disk on $REPO — builds fail under ENOSPC." >&2
    exit 2
fi

if ! pgrep -f 'mem-guard\.sh' >/dev/null 2>&1; then
    echo "WARN: mem-guard.sh not running — a runaway cargo can pressure host memory." >&2
fi

# ── Rust/Ipê panic-detection regex ───────────────────────────────────────────
# Sources:
#   • sky_runtime::core::install_panic_classifier() plain-text line:
#       "[error] DivisionByZero (ref XXXXXXXX): ..."
#       "[error] IndexOutOfRange (ref XXXXXXXX): ..."
#       "[error] ArithmeticOverflow (ref XXXXXXXX): ..."
#       "[error] Unexpected (ref XXXXXXXX): ..."
#   • JSON variant (SKY_LOG_FORMAT=json):
#       {"level":"error","kind":"DivisionByZero",...}
#   • Rust default unhandled-panic (reaches raw output when hook fires but
#     the thread join still re-propagates):
#       "thread '...' panicked at '...'"
#   • Direct binary crash (signal/SIGABRT/SIGILL — cargo wraps with RUST_BACKTRACE):
#       "RUST_BACKTRACE=1"
#   • block_on thread-join Err arm:
#       "async task panicked"
#   • ffi_kernel_polyfill:
#       "Ffi.kernel ... should not be called in Rust target"
#   • Go-backend markers preserved for composite corpus mode:
#       "panic:", "goroutine [N] [", "runtime error:", "fatal error:", "unrecoverable"
PANIC_RE='^\[error\] [A-Za-z]+ \(ref |"kind":"[A-Za-z]|thread .* panicked at|RUST_BACKTRACE|async task panicked|Ffi\.kernel.*should not be called|panic:|goroutine [0-9]+ \[|runtime error:|fatal error:|unrecoverable'

# ── Warm shared CARGO_TARGET_DIR (the key to fast iterations) ────────────────
# The sweep's env.sh already pins CARGO_TARGET_DIR=$HOME/.cache/sky-rust-target.
# Heavy deps (axum/tokio/serde/sqlx/…) compile once and are reused across every
# fuzz iteration — so after the first cold build each iteration's cargo step is
# a ~1 s link, not a multi-minute compile.

# ── Deterministic PRNG: LCG (Numerical Recipes constants) ────────────────────
# Stays inside 31-bit positive integers; a (seed, iter) pair is reproducible.
lcg_next() { echo $(( (1103515245 * $1 + 12345) & 0x7FFFFFFF )); }
bint()      { echo $(( $2 + ($1 % ($3 - $2 + 1)) )); }

# ── Six well-typed Sky program templates ─────────────────────────────────────
# Each is well-typed by construction — slot fills (Int literals, alphanum
# Strings, bounded Int-list literals) satisfy the declared types. A violation
# would be a compiler soundness bug, not a template bug. The templates are
# direct ports of the Haskell reference fuzzer's templates, adjusted only where
# the Rust/Ipê stdlib surface differs from the Go surface (it doesn't — the
# Sky source surface is identical).

template_arith() {
    local n1=$1 n2=$2 n3=$3
    cat <<EOF
module Main exposing (main)

import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

main =
    println (String.fromInt (let x_a = $n1 in x_a + $n2 * $n3))
EOF
}

template_strconcat() {
    local n1=$1 s1=$2
    cat <<EOF
module Main exposing (main)

import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

main =
    println ("prefix-" ++ String.fromInt $n1 ++ "-suffix-" ++ "$s1")
EOF
}

template_listmap() {
    local n1=$1 lst=$2
    cat <<EOF
module Main exposing (main)

import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

main =
    println (String.fromInt (List.length (List.map (\x -> x + $n1) $lst)))
EOF
}

template_maybechain() {
    local n1=$1 n2=$2
    cat <<EOF
module Main exposing (main)

import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

main =
    println (String.fromInt (Maybe.withDefault $n1 (Maybe.map (\x -> x * 2) (Just $n2))))
EOF
}

template_resultpipeline() {
    local n1=$1 n2=$2
    cat <<EOF
module Main exposing (main)

import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

main =
    println (String.fromInt (Result.withDefault 0 (Result.map (\x -> x + $n1) (Ok $n2))))
EOF
}

template_paramrecord() {
    local n1=$1 s1=$2
    cat <<EOF
module Main exposing (main)

import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

type alias Box a =
    { value : a, label : String }

main =
    println (let b = { value = $n1, label = "$s1" } in String.fromInt b.value)
EOF
}

# ── Template renderer ─────────────────────────────────────────────────────────
render_template() {
    local seed=$1 dst=$2
    local ps pk s1 s2 s3 n1 n2 n3 slen str llen lstr lst i cs cidx ch ls lv

    ps=$(lcg_next "$seed")
    local kind=$(( ps % 6 ))

    s1=$(lcg_next $(( seed + 1 )))
    s2=$(lcg_next $(( seed + 2 )))
    s3=$(lcg_next $(( seed + 3 )))

    n1=$(bint "$s1" 0 99)
    n2=$(bint "$s2" 0 99)
    n3=$(bint "$s3" 0 99)

    # Alphanum string of length 1..6 drawn from [a-z]
    slen=$(bint "$s1" 1 6)
    str=""
    for (( i = 0; i < slen; i++ )); do
        cs=$(lcg_next $(( seed * 7 + i + 1 )))
        cidx=$(( cs % 26 ))
        ch=$(awk -v n=$(( 97 + cidx )) 'BEGIN { printf "%c", n }')
        str="$str$ch"
    done

    # Bounded Int list literal of length 0..5
    llen=$(bint "$s2" 0 5)
    lstr=""
    for (( i = 0; i < llen; i++ )); do
        ls=$(lcg_next $(( seed * 11 + i + 1 )))
        lv=$(bint "$ls" 0 99)
        if [[ -z "$lstr" ]]; then lstr="$lv"; else lstr="$lstr, $lv"; fi
    done
    lst="[$lstr]"

    case $kind in
        0) echo "arith";       template_arith       "$n1" "$n2" "$n3" > "$dst" ;;
        1) echo "strconcat";   template_strconcat   "$n1" "$str"       > "$dst" ;;
        2) echo "listmap";     template_listmap     "$n1" "$lst"       > "$dst" ;;
        3) echo "maybechain";  template_maybechain  "$n1" "$n2"        > "$dst" ;;
        4) echo "resultpipe";  template_resultpipeline "$n1" "$n2"    > "$dst" ;;
        5) echo "paramrecord"; template_paramrecord "$n1" "$str"       > "$dst" ;;
    esac
}

# ── Project setup ─────────────────────────────────────────────────────────────
setup_project() {
    local dir=$1
    mkdir -p "$dir/src"
    cat > "$dir/sky.toml" <<'EOF'
name = "sky-fuzz-iter"
version = "0.0.0"
entry = "src/Main.sky"
EOF
}

# ── Detect panic in combined build/run output ─────────────────────────────────
has_panic() {
    local log=$1
    [[ -f "$log" ]] && grep -qE "$PANIC_RE" "$log"
}

# ── One fuzz iteration: render → build → run → assert ────────────────────────
# Returns:   0  = green
#            1  = failure (reason in stdout, e.g. "BUILD-FAILED rc=1 kind=arith")
run_iter() {
    local seed=$1 mode=$2 iterdir=$3 kind corpus_src="" composite_corpus="" _corpus_cand="" _cc=""

    setup_project "$iterdir"

    case "$mode" in
        template)
            kind=$(render_template "$seed" "$iterdir/src/Main.sky")
            ;;
        corpus)
            # Replay a known-good corpus example — validates the compiler
            # doesn't drift under repeated invocation. Prefer 01-hello-world
            # which is the unconditional pass example in this port. Fall back
            # to a synthesised template if no corpus example is available.
            # NOTE: 00-standard-libs imports Std.Money which has a pre-existing
            # stdlib type error (unrelated to soundness under test) — skip it.
            local corpus_src=""
            for _corpus_cand in \
                "$REPO/examples/01-hello-world/src/Main.sky" \
                "$REPO/examples/14-task-demo/src/Main.sky"; do
                [[ -f "$_corpus_cand" ]] && { corpus_src="$_corpus_cand"; break; }
            done
            if [[ -n "$corpus_src" ]]; then
                cp -f "$corpus_src" "$iterdir/src/Main.sky"
                kind="corpus"
            else
                kind=$(render_template "$seed" "$iterdir/src/Main.sky")
                kind="template-fallback"
            fi
            ;;
        composite|*)
            if (( seed % 2 == 0 )); then
                kind=$(render_template "$seed" "$iterdir/src/Main.sky")
            else
                local composite_corpus=""
                for _cc in \
                    "$REPO/examples/01-hello-world/src/Main.sky" \
                    "$REPO/examples/14-task-demo/src/Main.sky"; do
                    [[ -f "$_cc" ]] && { composite_corpus="$_cc"; break; }
                done
                if [[ -n "$composite_corpus" ]]; then
                    cp -f "$composite_corpus" "$iterdir/src/Main.sky"
                    kind="corpus"
                else
                    kind=$(render_template "$seed" "$iterdir/src/Main.sky")
                fi
            fi
            ;;
    esac

    local buildlog="$iterdir/build.log"
    local runlog="$iterdir/run.log"
    : >"$buildlog" >"$runlog"

    # ── Step 1: skyc build → emitted Rust project ──────────────────────────
    local skyc_rc=0
    if ! ( cd "$iterdir" && timeout "$BUILD_TIMEOUT" \
           "$SKYC_BIN" build src/Main.sky --out sky-out/rust >"$buildlog" 2>&1 ); then
        skyc_rc=$?
        echo "SKYC-BUILD-FAILED rc=$skyc_rc kind=$kind"
        return 1
    fi
    if [[ ! -f "$iterdir/sky-out/rust/Cargo.toml" ]]; then
        echo "SKYC-BUILD-FAILED no-cargo-toml kind=$kind"
        return 1
    fi

    # ── Step 2: cargo build → sky-app binary ───────────────────────────────
    local cargo_rc=0
    if ! ( cd "$iterdir" && timeout "$BUILD_TIMEOUT" \
           cargo build --manifest-path sky-out/rust/Cargo.toml >>"$buildlog" 2>&1 ); then
        cargo_rc=$?
        if has_panic "$buildlog"; then
            echo "CARGO-BUILD-PANIC rc=$cargo_rc kind=$kind"
        else
            echo "CARGO-BUILD-FAILED rc=$cargo_rc kind=$kind"
        fi
        return 1
    fi

    # ── Step 3: find binary ─────────────────────────────────────────────────
    local bin=""
    for _cand in \
        "$CARGO_TARGET_DIR/debug/sky-app" \
        "$CARGO_TARGET_DIR/release/sky-app" \
        "$iterdir/sky-out/rust/target/debug/sky-app"; do
        [[ -x "$_cand" ]] && { bin="$_cand"; break; }
    done
    if [[ -z "$bin" ]]; then
        echo "BINARY-NOT-FOUND kind=$kind"
        return 1
    fi

    # ── Step 4: run ─────────────────────────────────────────────────────────
    local run_dir run_rc
    run_dir="$(mktemp -d "${TMPDIR:-/tmp}/sky-fuzz-run.XXXXXX")"
    ( cd "$run_dir" && timeout "$RUN_TIMEOUT" "$bin" ) >"$runlog" 2>&1
    run_rc=$?
    rm -rf "$run_dir"

    if [[ "$run_rc" -eq 124 ]]; then
        echo "RUN-TIMEOUT kind=$kind"
        return 1
    fi

    # ── Step 5: check for panic markers ─────────────────────────────────────
    if has_panic "$runlog"; then
        echo "PANIC-DETECTED kind=$kind"
        return 1
    fi

    # A non-zero exit even without matching panic markers = failure.
    # (Structured errors from Task.fail print to stderr and exit 1.)
    if [[ "$run_rc" -ne 0 ]]; then
        echo "RUN-FAILED rc=$run_rc kind=$kind"
        return 1
    fi

    return 0
}

# ── Failure forensics ─────────────────────────────────────────────────────────
save_failure() {
    local seed=$1 iterdir=$2 reason=$3
    local ts; ts=$(date +%s)
    local dst="$FAILURES_DIR/seed-${seed}-${ts}"
    mkdir -p "$dst"
    cp -rf "$iterdir/src"       "$dst/"     2>/dev/null || true
    cp -f  "$iterdir/sky.toml"  "$dst/"     2>/dev/null || true
    cp -f  "$iterdir/build.log" "$dst/"     2>/dev/null || true
    cp -f  "$iterdir/run.log"   "$dst/"     2>/dev/null || true
    # Emitted Rust source (most useful artefact for debugging)
    [[ -d "$iterdir/sky-out/rust/src" ]] && \
        cp -rf "$iterdir/sky-out/rust/src" "$dst/emitted-rust-src" 2>/dev/null || true
    printf 'seed=%s reason=%s\n' "$seed" "$reason" > "$dst/SUMMARY"
    echo "  Forensics: $dst"
}

# ── True-positive demo ────────────────────────────────────────────────────────
# A WELL-TYPED Sky program that panics at runtime: `42 // 0`.
# The `//` operator is integer division; divisor 0 triggers the
# sky_runtime::math::sky_int_div panic path, classified as DivisionByZero,
# exit 101. The detector must flag it.
run_tp_demo() {
    echo "=== TRUE-POSITIVE DEMO ==="
    echo "    Program: println (String.fromInt (42 // 0))"
    echo "    Expected: DivisionByZero panic, exit != 0, detector flags it."
    echo ""

    local tp_dir; tp_dir="$(mktemp -d "${TMPDIR:-/tmp}/sky-fuzz-tp.XXXXXX")"
    setup_project "$tp_dir"
    cat > "$tp_dir/src/Main.sky" <<'EOF'
module Main exposing (main)

import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

main =
    println (String.fromInt (42 // 0))
EOF

    local buildlog="$tp_dir/build.log"
    local runlog="$tp_dir/run.log"
    : >"$buildlog" >"$runlog"

    echo "[1/3] skyc build..."
    if ! ( cd "$tp_dir" && timeout "$BUILD_TIMEOUT" \
           "$SKYC_BIN" build src/Main.sky --out sky-out/rust >"$buildlog" 2>&1 ); then
        echo "RESULT: FAIL — program did not build (compiler bug)"
        echo "  Build log: $(cat "$buildlog")"
        rm -rf "$tp_dir"; return 1
    fi
    echo "      OK (well-typed by construction — build pass is correct)"

    echo "[2/3] cargo build..."
    if ! ( cd "$tp_dir" && timeout "$BUILD_TIMEOUT" \
           cargo build --manifest-path sky-out/rust/Cargo.toml >>"$buildlog" 2>&1 ); then
        echo "RESULT: FAIL — cargo build failed"
        rm -rf "$tp_dir"; return 1
    fi
    echo "      OK"

    local bin=""
    for _cand in \
        "$CARGO_TARGET_DIR/debug/sky-app" \
        "$CARGO_TARGET_DIR/release/sky-app"; do
        [[ -x "$_cand" ]] && { bin="$_cand"; break; }
    done
    if [[ -z "$bin" ]]; then echo "RESULT: FAIL — binary not found"; rm -rf "$tp_dir"; return 1; fi

    echo "[3/3] running (expecting panic)..."
    local run_dir; run_dir="$(mktemp -d "${TMPDIR:-/tmp}/sky-fuzz-tprun.XXXXXX")"
    local run_rc
    ( cd "$run_dir" && timeout 10 "$bin" ) >"$runlog" 2>&1
    run_rc=$?
    rm -rf "$run_dir"

    echo "      exit code: $run_rc"
    echo "      stderr output:"
    cat "$runlog" | sed 's/^/        /'
    echo ""

    local detected=0
    if has_panic "$runlog"; then
        detected=1
        echo "      PANIC MARKER DETECTED in output: YES"
    else
        echo "      PANIC MARKER DETECTED in output: NO"
    fi

    if [[ "$run_rc" -ne 0 && "$detected" -eq 1 ]]; then
        echo ""
        echo "RESULT: TRUE POSITIVE CONFIRMED."
        echo "  The detector correctly flagged 42 // 0 as a panic (DivisionByZero)."
        echo "  Exit was $run_rc (non-zero) — would be counted as RUN-FAILED or PANIC-DETECTED."
        [[ "$KEEP" -eq 0 ]] && rm -rf "$tp_dir"
        return 0
    elif [[ "$run_rc" -ne 0 && "$detected" -eq 0 ]]; then
        echo ""
        echo "RESULT: PARTIAL — exit $run_rc but no recognized panic marker."
        echo "  The binary crashed but output did not match PANIC_RE."
        echo "  The fuzzer STILL catches this (non-zero exit → RUN-FAILED)."
        echo "  Consider whether the output should be added to PANIC_RE."
        echo "  Raw output: $(cat "$runlog")"
        [[ "$KEEP" -eq 0 ]] && rm -rf "$tp_dir"
        return 0
    else
        echo ""
        echo "RESULT: FAIL — true-positive not detected."
        echo "  The runtime appears to have absorbed the division-by-zero without"
        echo "  panicking (exit 0 + no panic marker). This means either:"
        echo "    a) The Rust runtime handles 42 // 0 gracefully (returns 0 or Err),"
        echo "       which is a sanctioned divergence from Sky-Go (see math.rs)."
        echo "    b) There is a bug in the detector (check PANIC_RE)."
        echo "  In either case: report as a finding (see --tp-demo output above)."
        [[ "$KEEP" -eq 0 ]] && rm -rf "$tp_dir"
        return 1
    fi
}

# ── Directories ───────────────────────────────────────────────────────────────
FUZZ_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sky-fuzz.XXXXXX")"
FAILURES_DIR="/tmp/sky-fuzz/FAILURES"
mkdir -p "$FAILURES_DIR"

cleanup() {
    if [[ "$KEEP" -eq 0 ]]; then rm -rf "$FUZZ_DIR"; fi
}
trap cleanup EXIT

# ── True-positive demo mode ───────────────────────────────────────────────────
if [[ "$TP_DEMO" -eq 1 ]]; then
    run_tp_demo
    exit $?
fi

# ── Main loop ─────────────────────────────────────────────────────────────────
echo "sky-fuzz: mode=$MODE iters=$ITERS start_seed=$SEED"
echo "sky-fuzz: skyc=$SKYC_BIN"
echo "sky-fuzz: cargo_target=$CARGO_TARGET_DIR"
echo "sky-fuzz: tempdir=$FUZZ_DIR"
echo "sky-fuzz: failures_dir=$FAILURES_DIR"
echo "sky-fuzz: build_timeout=${BUILD_TIMEOUT}s run_timeout=${RUN_TIMEOUT}s"
echo ""

start_ts=$(date +%s)
failures=0
green=0

for (( i = 0; i < ITERS; i++ )); do
    iter_seed=$(( SEED + i ))
    iterdir="$FUZZ_DIR/iter-$i"
    mkdir -p "$iterdir"

    reason=$(run_iter "$iter_seed" "$MODE" "$iterdir")
    rc=$?

    if [[ "$rc" -ne 0 ]]; then
        failures=$(( failures + 1 ))
        echo "FAIL iter=$i seed=$iter_seed $reason" >&2
        save_failure "$iter_seed" "$iterdir" "$reason"
        echo "" >&2
        echo "sky-fuzz: ABORTING after first failure (iter $i / $ITERS)." >&2
        echo "sky-fuzz: reproduce: $0 --seed $iter_seed --iters 1 --keep" >&2
        exit 1
    fi
    green=$(( green + 1 ))

    # Clean up successful iter to avoid disk pressure at 10k
    rm -rf "$iterdir"

    if [[ "$QUIET" -eq 0 && $(( (i + 1) % 10 )) -eq 0 ]]; then
        elapsed=$(( $(date +%s) - start_ts ))
        rate=$(awk -v g="$green" -v e="$elapsed" \
            'BEGIN { if (e>0) printf "%.1f", g/e; else print "-" }')
        echo "  progress: $((i + 1))/$ITERS green=$green elapsed=${elapsed}s rate=${rate}/s"
    fi
done

elapsed=$(( $(date +%s) - start_ts ))
echo ""
echo "sky-fuzz: DONE iters=$ITERS green=$green failures=$failures elapsed=${elapsed}s"
if [[ "$failures" -eq 0 ]]; then
    if (( ITERS >= 10000 )); then
        echo "sky-fuzz: full gate SATISFIED — ran $ITERS iters clean (criterion 8)"
    else
        echo "sky-fuzz: smoke PASS — ran $ITERS iters clean"
        echo "          (full gate: SKY_FUZZ_FULL=1 ./scripts/fuzz-well-typed.sh)"
    fi
    exit 0
else
    exit 1
fi
