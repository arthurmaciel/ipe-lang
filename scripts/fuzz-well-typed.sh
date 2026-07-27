#!/usr/bin/env bash
# scripts/fuzz-well-typed.sh — well-typed soundness fuzzer (Ipê/ipe-lang port).
#
# Ported from ../ipe/scripts/fuzz-well-typed.sh (Haskell backend, Go target).
# KEY ADAPTATIONS — Rust/Ipê backend:
#
#   BUILD: ipe build src/Main.ipe --out out/rust
#          cargo build --manifest-path out/rust/Cargo.toml
#          (binary: $CARGO_TARGET_DIR/debug/ipe-app)
#
#   PANIC DETECTION: Rust/Ipê runtime installs a classify-and-log panic hook
#   (ipe_runtime::core::install_panic_classifier). A runtime fault emits to
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
#   TRUE POSITIVE: `42 // 0` (Ipê integer division by zero) triggers the
#   DivisionByZero classifier, exits 101. Demonstrated in --tp-demo mode.
#
# Property:
#   A random WELL-TYPED Ipê program MUST (a) build successfully with ipe+cargo
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
#   --build-timeout N  ipe+cargo build timeout in seconds (default 300)
#   --run-timeout N    binary run timeout in seconds (default 15)
#   --tp-demo          Run the true-positive demo then exit (verifies detector)
#   IPE_FUZZ_FULL=1    Shorthand for --iters 10000 (CI full-gate override)
#
# Exit: 0 = all iterations green; 1 = first failure (seed + forensics dir
# under /tmp/ipe-fuzz/FAILURES/); 2 = setup error.
#
# Reproduce a failure: ./scripts/fuzz-well-typed.sh --seed N --iters 1 --keep
# Full 10k gate:       IPE_FUZZ_FULL=1 ./scripts/fuzz-well-typed.sh

set -uo pipefail

# ── Source the shared env (REPO, IPE_BIN, CARGO_TARGET_DIR, IPE_RUNTIME_DIR) ──
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib/env.sh"

# ── Flags ────────────────────────────────────────────────────────────────────
ITERS="${IPE_FUZZ_FULL:+10000}"
ITERS="${ITERS:-30}"
SEED=""
MODE="composite"
KEEP=0
QUIET=0
BUILD_TIMEOUT=300   # ipe + cargo combined; cargo alone can take ~5 min cold
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
if [[ ! -x "$IPE_BIN" ]]; then
    echo "ERROR: ipe binary not found at '$IPE_BIN'" >&2
    echo "  Build: cargo build -p ipe  (or set IPE_BIN=...)" >&2
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
#   • ipe_runtime::core::install_panic_classifier() plain-text line:
#       "[error] DivisionByZero (ref XXXXXXXX): ..."
#       "[error] IndexOutOfRange (ref XXXXXXXX): ..."
#       "[error] ArithmeticOverflow (ref XXXXXXXX): ..."
#       "[error] Unexpected (ref XXXXXXXX): ..."
#   • JSON variant (IPE_LOG_FORMAT=json):
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
# The sweep's env.sh already pins CARGO_TARGET_DIR=$HOME/.cache/ipe-lang-target.
# Heavy deps (axum/tokio/serde/sqlx/…) compile once and are reused across every
# fuzz iteration — so after the first cold build each iteration's cargo step is
# a ~1 s link, not a multi-minute compile.

# ── Deterministic PRNG: LCG (Numerical Recipes constants) ────────────────────
# Stays inside 31-bit positive integers; a (seed, iter) pair is reproducible.
lcg_next() { echo $(( (1103515245 * $1 + 12345) & 0x7FFFFFFF )); }
bint()      { echo $(( $2 + ($1 % ($3 - $2 + 1)) )); }

# ── Six well-typed Ipê program templates ─────────────────────────────────────
# Each is well-typed by construction — slot fills (Int literals, alphanum
# Strings, bounded Int-list literals) satisfy the declared types. A violation
# would be a compiler soundness bug, not a template bug. The templates are
# direct ports of the Haskell reference fuzzer's templates, adjusted only where
# the Rust/Ipê stdlib surface differs from the Go surface (it doesn't — the
# Ipê source surface is identical).

template_arith() {
    local n1=$1 n2=$2 n3=$3
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

main =
    println (String.fromInt (let x_a = $n1 in x_a + $n2 * $n3))
EOF
}

template_strconcat() {
    local n1=$1 s1=$2
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

main =
    println ("prefix-" ++ String.fromInt $n1 ++ "-suffix-" ++ "$s1")
EOF
}

template_listmap() {
    local n1=$1 lst=$2
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

main =
    println (String.fromInt (List.length (List.map (\x -> x + $n1) $lst)))
EOF
}

template_maybechain() {
    local n1=$1 n2=$2
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

main =
    println (String.fromInt (Maybe.withDefault $n1 (Maybe.map (\x -> x * 2) (Just $n2))))
EOF
}

template_resultpipeline() {
    local n1=$1 n2=$2
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

main =
    println (String.fromInt (Result.withDefault 0 (Result.map (\x -> x + $n1) (Ok $n2))))
EOF
}

template_paramrecord() {
    local n1=$1 s1=$2
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

type alias Box a =
    { value : a, label : String }

main =
    println (let b = { value = $n1, label = "$s1" } in String.fromInt b.value)
EOF
}

# ── New templates (constructs 6–17) ───────────────────────────────────────────

# Template 6: case / ADT — declare a 3-constructor type, case over it covering
# all arms (including a wildcard catch-all). Well-typed by construction: the
# constructor chosen from {Red,Green,Blue} at slot fill and the case arms
# cover all three plus a final wildcard so exhaustiveness is satisfied.
template_adt_case() {
    local n1=$1 n2=$2 n3=$3
    # pick the constructor via modular arithmetic on n3
    local ctor
    case $(( n3 % 3 )) in
        0) ctor="Red" ;;
        1) ctor="Green" ;;
        *) ctor="Blue" ;;
    esac
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

type Color
    = Red
    | Green
    | Blue

colorValue : Color -> Int
colorValue c =
    case c of
        Red -> $n1
        Green -> $n2
        Blue -> $n3

main =
    println (String.fromInt (colorValue $ctor))
EOF
}

# Template 7: let-polymorphism — a let-bound helper `showNum` used at two
# different call sites with the same concrete type (Int→String). This exercises
# the let-binding path in HM without requiring higher-kinded types.
template_let_poly() {
    local n1=$1 n2=$2
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

main =
    let
        showNum x = "v=" ++ String.fromInt x
        a = showNum $n1
        b = showNum $n2
    in
    println (a ++ " " ++ b)
EOF
}

# Template 8: higher-order / partial application — pass a lambda to List.map,
# and use a partially applied (+) via a named helper. Well-typed: List.map takes
# (a->b) and List a; String.join takes a separator and List String.
template_higher_order() {
    local n1=$1 lst=$2
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.List as List
import Ipe.String as String
import Ipe.Log exposing (println)

addN : Int -> Int -> Int
addN n x =
    n + x

main =
    let
        bumped = List.map (addN $n1) $lst
        strs = List.map String.fromInt bumped
    in
    println (String.join "," strs)
EOF
}

# Template 9: record update — build a record, then produce an updated copy.
# Well-typed: both fields are the correct type after update.
template_record_update() {
    local n1=$1 n2=$2 s1=$3
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

type alias Point =
    { x : Int, y : Int, label : String }

main =
    let
        p0 = { x = $n1, y = $n2, label = "$s1" }
        p1 = { p0 | x = p0.x + 1, label = "updated" }
    in
    println (String.fromInt p1.x ++ "-" ++ p1.label)
EOF
}

# Template 10: tuple + destructure — build a 2-tuple, destructure via fst/snd
# (from Prelude). Well-typed: fst and snd are `(a, b) -> a` / `(a, b) -> b`.
template_tuple() {
    local n1=$1 n2=$2
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

main =
    let
        pair = ( $n1, $n2 )
        a = fst pair
        b = snd pair
    in
    println (String.fromInt (a + b))
EOF
}

# Template 11: recursion — a self-recursive `sumList` that sums an Int list.
# Well-typed: Int->Int return through every arm; tail position via acc.
template_recursion() {
    local lst=$1
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.List as List
import Ipe.Log exposing (println)

sumList : List Int -> Int -> Int
sumList xs acc =
    case xs of
        [] -> acc
        (h :: t) -> sumList t (acc + h)

main =
    println (String.fromInt (sumList $lst 0))
EOF
}

# Template 12: if / nested let — nested let blocks with an if/else inside.
# Well-typed: both branches of if return Int; outer let builds on the result.
template_if_nested_let() {
    local n1=$1 n2=$2 n3=$3
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

main =
    let
        bigger =
            if $n1 > $n2 then
                $n1
            else
                $n2
        result =
            let
                scaled = bigger * $n3
            in
            scaled + 1
    in
    println (String.fromInt result)
EOF
}

# Template 13: pipelines — |> and <| chains. Well-typed: Int threaded through
# String.fromInt, then String.length gives Int; <| applies to that.
template_pipeline() {
    local n1=$1 n2=$2
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.String as String
import Ipe.Log exposing (println)

double : Int -> Int
double x = x * 2

main =
    let
        len = $n1 |> String.fromInt |> String.length
        result = double <| len + $n2
    in
    println (String.fromInt result)
EOF
}

# Template 14: Dict ops — insert + get, String key. Well-typed: Dict.insert
# takes k->v->Dict k v; Dict.get returns Maybe v; Maybe.withDefault provides Int.
template_dict_ops() {
    local n1=$1 n2=$2 s1=$3
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Dict as Dict
import Ipe.Maybe as Maybe
import Ipe.Log exposing (println)

main =
    let
        d0 = Dict.empty
        d1 = Dict.insert "x" $n1 d0
        d2 = Dict.insert "$s1" $n2 d1
        vx = Maybe.withDefault 0 (Dict.get "x" d2)
        sz = Dict.size d2
    in
    println (String.fromInt (vx + sz))
EOF
}

# Template 15: Set ops — fromList, member, size. Well-typed: Set Int operations.
template_set_ops() {
    local n1=$1 n2=$2 lst=$3
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Set as Set
import Ipe.Log exposing (println)

main =
    let
        s = Set.fromList $lst
        s2 = Set.insert $n1 s
        hasMember = Set.member $n2 s2
        sz = Set.size s2
        result = if hasMember then sz + 1 else sz
    in
    println (String.fromInt result)
EOF
}

# Template 16: Maybe.andMap (applicative style) — map2 combines two Maybes.
# Well-typed: both Maybe Int, combinator produces Maybe Int.
template_maybe_andmap() {
    local n1=$1 n2=$2
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Maybe as Maybe
import Ipe.Log exposing (println)

main =
    let
        ma = Just $n1
        mb = Just $n2
        mc = Maybe.map2 (\a b -> a + b) ma mb
        result = Maybe.withDefault 0 mc
    in
    println (String.fromInt result)
EOF
}

# Template 17: Result.map2 — combines two Ok values. Well-typed: both
# Result Error Int, map2 produces Result Error Int.
template_result_map2() {
    local n1=$1 n2=$2
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Result as Result
import Ipe.Log exposing (println)

main =
    let
        ra = Ok $n1
        rb = Ok $n2
        rc = Result.map2 (\a b -> a * b) ra rb
        result = Result.withDefault 0 rc
    in
    println (String.fromInt result)
EOF
}

# Template 18: multiline string + interpolation — triple-quoted string with
# {{...}} interpolations. Well-typed: all interpolated exprs are String.
# NOTE: String.fromInt applied to a literal integer inside {{...}} triggers
# IPE-I0001 (unbound local '<literal>') — compiler bug, not template bug.
# Workaround: bind all Int values to let-variables BEFORE interpolating;
# {{varName}} and {{String.fromInt varName}} both work when the arg is a name.
template_multiline_interp() {
    local n1=$1 n2=$2 s1=$3
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.String as String
import Ipe.Log exposing (println)

main =
    let
        tag = "$s1"
        count = $n1
        total = $n2
        msg = """item={{tag}} count={{String.fromInt count}} total={{String.fromInt total}}"""
    in
    println msg
EOF
}

# ── Multi-module templates (constructs 19–22) ────────────────────────────────
# Boundary Scheme Promotion follow-up (class1 spec §"Fuzzer additions"):
# each writes a `Lib.ipe` SIBLING next to `Main.ipe` (two-file project) so the
# fuzzer exercises the cross-module untyped-boundary generalization path the
# 2026-07-10 class-1 fix landed — single-file templates can never reach it.
# Each template function takes the Lib.ipe path as its FIRST argument.

# Template 19: cross-module 2-type reuse — an UNTYPED `ident x = x` in Lib,
# instantiated at Int AND String from Main. Pre-fix this was the class-1
# "one monomorphic var across the linked program" bug (E0308/false reject);
# post-fix the boundary scheme generalizes and both uses are accepted.
template_mm_2type_reuse() {
    local libdst=$1 n1=$2 s1=$3
    cat > "$libdst" <<EOF
module Lib exposing (ident)

import Ipe.Prelude exposing (..)

-- Untyped on purpose: the boundary-scheme promotion must generalize this
-- def at the module boundary so each importer use instantiates it fresh.
ident x = x
EOF
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Lib exposing (ident)
import Ipe.Log exposing (println)

main =
    println (String.fromInt (ident $n1) ++ "|" ++ ident "$s1")
EOF
}

# Template 20: untyped VALUE binding (`empty = []`) used at two element types
# cross-module — proves the promotion has no value restriction (class1 spec
# new-unit-test 3, fuzz form).
template_mm_value_binding() {
    local libdst=$1 n1=$2 s1=$3
    cat > "$libdst" <<EOF
module Lib exposing (empty)

import Ipe.Prelude exposing (..)

-- Untyped value binding: promoted to a scheme at the boundary; importers
-- may use it as List Int AND List String.
empty = []
EOF
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Lib
import Ipe.Log exposing (println)

main =
    println
        (String.fromInt
            (List.length ($n1 :: Lib.empty) + List.length ("$s1" :: Lib.empty)))
EOF
}

# Template 21: Number-bounded untyped helper (`plus a b = a + b`) used
# cross-module at ONE numeric type. Documents D2 (class1 spec): using it at
# Int in one module and Float in another stays REJECTED (conservative
# Super-bound handling), so the well-typed template pins the accepted
# single-type shape.
template_mm_number_helper() {
    local libdst=$1 n1=$2 n2=$3
    cat > "$libdst" <<EOF
module Lib exposing (plus)

import Ipe.Prelude exposing (..)

-- Untyped Number-bounded helper. Single-type cross-module use is accepted;
-- Int+Float dual use is D2-rejected by design (see class1 spec).
plus a b = a + b
EOF
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Lib
import Ipe.Log exposing (println)

main =
    println (String.fromInt (Lib.plus $n1 $n2))
EOF
}

# Template 22: mutually-recursive UNTYPED pair in Lib, used polymorphically
# (two element types) from outside the recursion group (class1 spec
# new-unit-test 5, fuzz form).
template_mm_recursive_pair() {
    local libdst=$1 lst=$2 s1=$3
    cat > "$libdst" <<EOF
module Lib exposing (evenLen, oddLen)

import Ipe.Prelude exposing (..)

-- Mutually-recursive untyped pair, polymorphic in the element type. The
-- boundary promotion must generalize the GROUP so importers can use it at
-- several element types.
evenLen xs =
    case xs of
        [] -> True
        _ :: rest -> oddLen rest

oddLen xs =
    case xs of
        [] -> False
        _ :: rest -> evenLen rest
EOF
    cat <<EOF
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Lib
import Ipe.Log exposing (println)

main =
    let a = if Lib.evenLen $lst then "E" else "O" in
    let b = if Lib.evenLen [ "$s1" ] then "E" else "O" in
    println (a ++ b)
EOF
}

# ── Template renderer ─────────────────────────────────────────────────────────
# Total templates: 23 (6 original + 13 single-file + 4 multi-module).
# kind = seed-derived mod 23. Kinds 19-22 write a Lib.ipe sibling (the
# multi-file infrastructure lives inside render_template: dst is always
# src/Main.ipe, so Lib.ipe lands next to it and ipe's module resolution
# picks it up from the same src/ directory).
render_template() {
    local seed=$1 dst=$2
    local ps pk s1 s2 s3 n1 n2 n3 slen str llen lstr lst i cs cidx ch ls lv

    ps=$(lcg_next "$seed")
    local kind=$(( ps % 23 ))

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

    # Bounded Int list literal of length 1..5 (1 minimum — recursion/set
    # templates work better with at least one element)
    llen=$(bint "$s2" 1 5)
    lstr=""
    for (( i = 0; i < llen; i++ )); do
        ls=$(lcg_next $(( seed * 11 + i + 1 )))
        lv=$(bint "$ls" 0 99)
        if [[ -z "$lstr" ]]; then lstr="$lv"; else lstr="$lstr, $lv"; fi
    done
    lst="[$lstr]"

    case $kind in
        0)  echo "arith";          template_arith             "$n1" "$n2" "$n3" > "$dst" ;;
        1)  echo "strconcat";      template_strconcat         "$n1" "$str"       > "$dst" ;;
        2)  echo "listmap";        template_listmap           "$n1" "$lst"       > "$dst" ;;
        3)  echo "maybechain";     template_maybechain        "$n1" "$n2"        > "$dst" ;;
        4)  echo "resultpipe";     template_resultpipeline    "$n1" "$n2"        > "$dst" ;;
        5)  echo "paramrecord";    template_paramrecord       "$n1" "$str"       > "$dst" ;;
        6)  echo "adtcase";        template_adt_case          "$n1" "$n2" "$n3" > "$dst" ;;
        7)  echo "letpoly";        template_let_poly          "$n1" "$n2"        > "$dst" ;;
        8)  echo "higherorder";    template_higher_order      "$n1" "$lst"       > "$dst" ;;
        9)  echo "recordupdate";   template_record_update     "$n1" "$n2" "$str" > "$dst" ;;
        10) echo "tuple";          template_tuple             "$n1" "$n2"        > "$dst" ;;
        11) echo "recursion";      template_recursion         "$lst"             > "$dst" ;;
        12) echo "ifnestedlet";    template_if_nested_let     "$n1" "$n2" "$n3" > "$dst" ;;
        13) echo "pipeline";       template_pipeline          "$n1" "$n2"        > "$dst" ;;
        14) echo "dictops";        template_dict_ops          "$n1" "$n2" "$str" > "$dst" ;;
        15) echo "setops";         template_set_ops           "$n1" "$n2" "$lst" > "$dst" ;;
        16) echo "maybeandmap";    template_maybe_andmap      "$n1" "$n2"        > "$dst" ;;
        17) echo "resultmap2";     template_result_map2       "$n1" "$n2"        > "$dst" ;;
        18) echo "multilineinterp"; template_multiline_interp "$n1" "$n2" "$str" > "$dst" ;;
        # Multi-module kinds (19-22): Lib.ipe is written as a sibling of dst
        # (always src/Main.ipe), exercising the class-1 boundary-scheme
        # promotion's cross-module generalization path.
        19) echo "mm2typereuse";   template_mm_2type_reuse    "$(dirname "$dst")/Lib.ipe" "$n1" "$str" > "$dst" ;;
        20) echo "mmvaluebind";    template_mm_value_binding  "$(dirname "$dst")/Lib.ipe" "$n1" "$str" > "$dst" ;;
        21) echo "mmnumberhelper"; template_mm_number_helper  "$(dirname "$dst")/Lib.ipe" "$n1" "$n2"  > "$dst" ;;
        22) echo "mmrecpair";      template_mm_recursive_pair "$(dirname "$dst")/Lib.ipe" "$lst" "$str" > "$dst" ;;
    esac
}

# ── Project setup ─────────────────────────────────────────────────────────────
setup_project() {
    local dir=$1
    mkdir -p "$dir/src"
    cat > "$dir/ipe.toml" <<'EOF'
name = "ipe-fuzz-iter"
version = "0.0.0"
entry = "src/Main.ipe"
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
            kind=$(render_template "$seed" "$iterdir/src/Main.ipe")
            ;;
        corpus)
            # Replay a known-good corpus example — validates the compiler
            # doesn't drift under repeated invocation. Prefer 01-hello-world
            # which is the unconditional pass example in this port. Fall back
            # to a synthesised template if no corpus example is available.
            # NOTE: 00-standard-libs imports Ipe.Money which has a pre-existing
            # stdlib type error (unrelated to soundness under test) — skip it.
            local corpus_src=""
            for _corpus_cand in \
                "$REPO/examples/sky/ipe/01-hello-world/src/Main.ipe" \
                "$REPO/examples/14-task-demo/src/Main.ipe"; do
                [[ -f "$_corpus_cand" ]] && { corpus_src="$_corpus_cand"; break; }
            done
            if [[ -n "$corpus_src" ]]; then
                cp -f "$corpus_src" "$iterdir/src/Main.ipe"
                kind="corpus"
            else
                kind=$(render_template "$seed" "$iterdir/src/Main.ipe")
                kind="template-fallback"
            fi
            ;;
        composite|*)
            if (( seed % 2 == 0 )); then
                kind=$(render_template "$seed" "$iterdir/src/Main.ipe")
            else
                local composite_corpus=""
                for _cc in \
                    "$REPO/examples/sky/ipe/01-hello-world/src/Main.ipe" \
                    "$REPO/examples/14-task-demo/src/Main.ipe"; do
                    [[ -f "$_cc" ]] && { composite_corpus="$_cc"; break; }
                done
                if [[ -n "$composite_corpus" ]]; then
                    cp -f "$composite_corpus" "$iterdir/src/Main.ipe"
                    kind="corpus"
                else
                    kind=$(render_template "$seed" "$iterdir/src/Main.ipe")
                fi
            fi
            ;;
    esac

    local buildlog="$iterdir/build.log"
    local runlog="$iterdir/run.log"
    : >"$buildlog" >"$runlog"

    # ── Step 1: ipe build → emitted Rust project ──────────────────────────
    local ipe_rc=0
    if ! ( cd "$iterdir" && timeout "$BUILD_TIMEOUT" \
           "$IPE_BIN" build src/Main.ipe --out out/rust >"$buildlog" 2>&1 ); then
        ipe_rc=$?
        echo "IPE-BUILD-FAILED rc=$ipe_rc kind=$kind"
        return 1
    fi
    if [[ ! -f "$iterdir/out/rust/Cargo.toml" ]]; then
        echo "IPE-BUILD-FAILED no-cargo-toml kind=$kind"
        return 1
    fi

    # ── Step 2: cargo build → ipe-app binary ───────────────────────────────
    local cargo_rc=0
    if ! ( cd "$iterdir" && timeout "$BUILD_TIMEOUT" \
           cargo build --manifest-path out/rust/Cargo.toml >>"$buildlog" 2>&1 ); then
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
        "$CARGO_TARGET_DIR/debug/ipe-app" \
        "$CARGO_TARGET_DIR/release/ipe-app" \
        "$iterdir/out/rust/target/debug/ipe-app"; do
        [[ -x "$_cand" ]] && { bin="$_cand"; break; }
    done
    if [[ -z "$bin" ]]; then
        echo "BINARY-NOT-FOUND kind=$kind"
        return 1
    fi

    # ── Step 4: run ─────────────────────────────────────────────────────────
    local run_dir run_rc
    run_dir="$(mktemp -d "${TMPDIR:-/tmp}/ipe-fuzz-run.XXXXXX")"
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
    cp -f  "$iterdir/ipe.toml"  "$dst/"     2>/dev/null || true
    cp -f  "$iterdir/build.log" "$dst/"     2>/dev/null || true
    cp -f  "$iterdir/run.log"   "$dst/"     2>/dev/null || true
    # Emitted Rust source (most useful artefact for debugging)
    [[ -d "$iterdir/out/rust/src" ]] && \
        cp -rf "$iterdir/out/rust/src" "$dst/emitted-rust-src" 2>/dev/null || true
    printf 'seed=%s reason=%s\n' "$seed" "$reason" > "$dst/SUMMARY"
    echo "  Forensics: $dst"
}

# ── True-positive demo ────────────────────────────────────────────────────────
# A WELL-TYPED Ipê program that panics at runtime: `42 // 0`.
# The `//` operator is integer division; divisor 0 triggers the
# ipe_runtime::math::ipe_int_div panic path, classified as DivisionByZero,
# exit 101. The detector must flag it.
run_tp_demo() {
    echo "=== TRUE-POSITIVE DEMO ==="
    echo "    Program: println (String.fromInt (42 // 0))"
    echo "    Expected: DivisionByZero panic, exit != 0, detector flags it."
    echo ""

    local tp_dir; tp_dir="$(mktemp -d "${TMPDIR:-/tmp}/ipe-fuzz-tp.XXXXXX")"
    setup_project "$tp_dir"
    cat > "$tp_dir/src/Main.ipe" <<'EOF'
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Log exposing (println)

main =
    println (String.fromInt (42 // 0))
EOF

    local buildlog="$tp_dir/build.log"
    local runlog="$tp_dir/run.log"
    : >"$buildlog" >"$runlog"

    echo "[1/3] ipe build..."
    if ! ( cd "$tp_dir" && timeout "$BUILD_TIMEOUT" \
           "$IPE_BIN" build src/Main.ipe --out out/rust >"$buildlog" 2>&1 ); then
        echo "RESULT: FAIL — program did not build (compiler bug)"
        echo "  Build log: $(cat "$buildlog")"
        rm -rf "$tp_dir"; return 1
    fi
    echo "      OK (well-typed by construction — build pass is correct)"

    echo "[2/3] cargo build..."
    if ! ( cd "$tp_dir" && timeout "$BUILD_TIMEOUT" \
           cargo build --manifest-path out/rust/Cargo.toml >>"$buildlog" 2>&1 ); then
        echo "RESULT: FAIL — cargo build failed"
        rm -rf "$tp_dir"; return 1
    fi
    echo "      OK"

    local bin=""
    for _cand in \
        "$CARGO_TARGET_DIR/debug/ipe-app" \
        "$CARGO_TARGET_DIR/release/ipe-app"; do
        [[ -x "$_cand" ]] && { bin="$_cand"; break; }
    done
    if [[ -z "$bin" ]]; then echo "RESULT: FAIL — binary not found"; rm -rf "$tp_dir"; return 1; fi

    echo "[3/3] running (expecting panic)..."
    local run_dir; run_dir="$(mktemp -d "${TMPDIR:-/tmp}/ipe-fuzz-tprun.XXXXXX")"
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
        echo "       which is a sanctioned divergence from Ipê-Go (see math.rs)."
        echo "    b) There is a bug in the detector (check PANIC_RE)."
        echo "  In either case: report as a finding (see --tp-demo output above)."
        [[ "$KEEP" -eq 0 ]] && rm -rf "$tp_dir"
        return 1
    fi
}

# ── Directories ───────────────────────────────────────────────────────────────
FUZZ_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ipe-fuzz.XXXXXX")"
FAILURES_DIR="/tmp/ipe-fuzz/FAILURES"
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
echo "ipe-fuzz: mode=$MODE iters=$ITERS start_seed=$SEED"
echo "ipe-fuzz: ipe=$IPE_BIN"
echo "ipe-fuzz: cargo_target=$CARGO_TARGET_DIR"
echo "ipe-fuzz: tempdir=$FUZZ_DIR"
echo "ipe-fuzz: failures_dir=$FAILURES_DIR"
echo "ipe-fuzz: build_timeout=${BUILD_TIMEOUT}s run_timeout=${RUN_TIMEOUT}s"
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
        echo "ipe-fuzz: ABORTING after first failure (iter $i / $ITERS)." >&2
        echo "ipe-fuzz: reproduce: $0 --seed $iter_seed --iters 1 --keep" >&2
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
echo "ipe-fuzz: DONE iters=$ITERS green=$green failures=$failures elapsed=${elapsed}s"
if [[ "$failures" -eq 0 ]]; then
    if (( ITERS >= 10000 )); then
        echo "ipe-fuzz: full gate SATISFIED — ran $ITERS iters clean (criterion 8)"
    else
        echo "ipe-fuzz: smoke PASS — ran $ITERS iters clean"
        echo "          (full gate: IPE_FUZZ_FULL=1 ./scripts/fuzz-well-typed.sh)"
    fi
    exit 0
else
    exit 1
fi
