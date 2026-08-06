#!/usr/bin/env bash
# tools/scripts/fuzz-ill-typed.sh — ill-typed (rejection) fuzzer for the Ipê/ipe-lang port.
#
# INVERSE of fuzz-well-typed.sh. Property:
#   An ILL-TYPED Ipê program MUST be REJECTED by `ipe` (exit != 0).
#   A false-acceptance — ipe exits 0 on an ill-typed program — is a
#   REAL SOUNDNESS BUG. Any such finding is copied to
#   /tmp/ipe-fuzz-neg/FAILURES/ and the script exits non-zero.
#
# LOAD-BEARING RULE: every mutation is ILL-TYPED BY CONSTRUCTION.
#   Random mutations of valid programs are NOT necessarily ill-typed, so
#   we do NOT assert "any mutant is rejected." Instead, we use a catalogue
#   of nine provably-breaking mutation families:
#
#   Cat 1 — UNDEFINED FIELD ACCESS:
#       Base has record with field `f`. Mutation accesses `.f<SEED>` (absent).
#       Expected code: IPE-T0012 (this record has no such field).
#
#   Cat 2 — UNDEFINED VARIABLE:
#       Reference `undef_<SEED>` — an identifier that provably has no binding
#       anywhere in scope.
#       Expected code: IPE-N0001 (cannot find this value in scope).
#
#   Cat 3 — UNKNOWN QUALIFIED MEMBER:
#       `String.nosuchfn_<SEED> x` — String module has no such member.
#       Expected code: IPE-N0005 (module has no such member).
#
#   Cat 4 — FORCED TYPE MISMATCH:
#       `String.length <int-literal>` — length : String -> Int, not Int -> Int.
#       Also: `if <int-literal> then X else X` — if-condition must be Bool.
#       Expected code: IPE-T0001 (type mismatch).
#
#   Cat 5 — WRONG CONSTRUCTOR ARITY:
#       A 1-payload constructor applied with 0 payloads in a case-pattern,
#       so the case tries to match `Just` where a `Just x` pattern is required.
#       Actually: apply a 0-ary constructor with an extra arg: `Nothing 42`.
#       Expected code: IPE-T0001 (type mismatch / application to non-function).
#
#   Cat 6 — NON-EXHAUSTIVE CASE:
#       An ADT with 3 constructors; the case covers only 2.
#       Expected code: IPE-T0010 (this case does not handle every possibility).
#
#   Cat 7 — SAME-MODULE 2-TYPE USE OF AN UNTYPED HELPER (#66-N canary):
#       `ident x = x` used at Int AND String within its own module. Boundary
#       scheme promotion generalizes at MODULE boundaries only — same-module
#       reuse stays monomorphic (reference parity, class1 spec). A silent
#       acceptance here means the promotion over-generalized.
#       Expected code: IPE-T0001.
#
#   Cat 8 — CROSS-MODULE USE AT AN INCOMPATIBLE INSTANTIATED TYPE
#       (multi-module: writes src/Lib.ipe): `Lib.inc "str"` against the
#       untyped Number-bounded `inc n = n + 1`. The imported scheme's bound
#       must survive fresh instantiation.
#       Expected code: IPE-T0001.
#
# SELF-VALIDATION:
#   - Bases COMPILE CLEAN (proves the harness doesn't reject everything).
#   - Every mutant is REJECTED (0 false-acceptances).
#   - At least one mutant per category is exercised per run.
#
# Flags:
#   --iters N          Iteration count (default 40)
#   --seed N           Start seed (default $RANDOM)
#   --keep             Keep tempdir on success
#   --quiet            Suppress per-iter progress; print summary only
#   --build-timeout N  ipe build timeout in seconds (default 60)
#   --base-sanity      Run base-compile sanity check then exit
#   --cat-demo         Demo one rejected mutant per category then exit
#   IPE_FUZZ_NEG_FULL=1  Shorthand for --iters 1000
#
# Exit: 0 = all iterations green (every mutant rejected, 0 false-acceptances);
#        1 = FALSE ACCEPTANCE found (soundness bug) or harness error;
#        2 = setup error.
#
# Reproduce: ./tools/scripts/fuzz-ill-typed.sh --seed N --iters 1 --keep
# Full gate:  IPE_FUZZ_NEG_FULL=1 ./tools/scripts/fuzz-ill-typed.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib/env.sh"

# ── Flags ────────────────────────────────────────────────────────────────────
ITERS="${IPE_FUZZ_NEG_FULL:+1000}"
ITERS="${ITERS:-40}"
SEED=""
KEEP=0
QUIET=0
BUILD_TIMEOUT=60
BASE_SANITY=0
CAT_DEMO=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --iters)         ITERS="$2";         shift 2 ;;
        --seed)          SEED="$2";          shift 2 ;;
        --keep)          KEEP=1;             shift ;;
        --quiet)         QUIET=1;            shift ;;
        --build-timeout) BUILD_TIMEOUT="$2"; shift 2 ;;
        --base-sanity)   BASE_SANITY=1;      shift ;;
        --cat-demo)      CAT_DEMO=1;         shift ;;
        -h|--help)       sed -n '2,70p' "$0"; exit 0 ;;
        *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac
done

[[ -z "$SEED" ]] && SEED=$RANDOM

# ── Preflight ─────────────────────────────────────────────────────────────────
if [[ ! -x "$IPE_BIN" ]]; then
    echo "ERROR: ipe binary not found at '$IPE_BIN'" >&2
    echo "  Build: cargo build -p ipe  (or set IPE_BIN=...)" >&2
    exit 2
fi

FREE_KB="$(df -Pk "$REPO" 2>/dev/null | awk 'NR==2{print $4}')"
if [[ -n "$FREE_KB" && "$FREE_KB" -lt 2097152 ]]; then
    echo "ERROR: <2 GB free disk on $REPO." >&2
    exit 2
fi

if ! pgrep -f 'mem-guard\.sh' >/dev/null 2>&1; then
    echo "WARN: mem-guard.sh not running — a runaway ipe can pressure host memory." >&2
fi

# ── Deterministic PRNG (same LCG as fuzz-well-typed.sh) ──────────────────────
lcg_next() { echo $(( (1103515245 * $1 + 12345) & 0x7FFFFFFF )); }
bint()      { echo $(( $2 + ($1 % ($3 - $2 + 1)) )); }

# ── Project boilerplate ───────────────────────────────────────────────────────
setup_project() {
    local dir=$1
    mkdir -p "$dir/src"
    cat > "$dir/ipe.toml" <<'EOF'
name = "ipe-fuzz-neg"
version = "0.0.0"
entry = "src/Main.ipe"
EOF
}

# ── Run ipe; return 0 iff ipe REJECTED (exit != 0) ─────────────────────────
# Also checks that expected_code appears in stderr when non-empty.
# Output: the ipe stderr (for code-extraction in demo mode).
run_ipe_expect_reject() {
    local dir=$1 expected_code=${2:-""}
    local log="$dir/build.log"
    : > "$log"
    local rc=0
    ( cd "$dir" && timeout "$BUILD_TIMEOUT" \
      "$IPE_BIN" build src/Main.ipe --out out/rust >"$log" 2>&1 ) || rc=$?
    if [[ "$rc" -eq 0 ]]; then
        # FALSE ACCEPTANCE: ipe returned 0 on an ill-typed program.
        echo "FALSE-ACCEPTANCE"
        return 1
    fi
    if [[ "$rc" -eq 124 ]]; then
        echo "TIMEOUT"
        return 1
    fi
    # ipe rejected (rc != 0) — good.
    if [[ -n "$expected_code" ]]; then
        if grep -q "$expected_code" "$log"; then
            echo "REJECTED-WITH-CODE"
        else
            echo "REJECTED-WRONG-CODE"
        fi
    else
        echo "REJECTED"
    fi
    return 0
}

# ── Run ipe; return 0 iff ipe ACCEPTED (exit 0) ────────────────────────────
# Used only for the base sanity check.
run_ipe_expect_accept() {
    local dir=$1
    local log="$dir/build.log"
    : > "$log"
    local rc=0
    ( cd "$dir" && timeout "$BUILD_TIMEOUT" \
      "$IPE_BIN" build src/Main.ipe --out out/rust >"$log" 2>&1 ) || rc=$?
    [[ "$rc" -eq 0 ]]
}

# ═══════════════════════════════════════════════════════════════════════════════
# SIX BASE PROGRAMS — well-typed; MUST compile clean.
# Each base is paired with 1+ mutation strategies that are provably ill-typed.
# ═══════════════════════════════════════════════════════════════════════════════

# ── Base for cats 1/4: record with a field `value` ───────────────────────────
# Has record type alias Point { x : Int, y : Int, value : Int }.
# Used for:
#   Cat 1 — access .value<SEED> (no such field)
#   Cat 4a — String.length <int-literal> (type mismatch)
base_record() {
    cat <<'EOF'
module Main exposing (main)

import Ipe.Log exposing (println)

type alias Point =
    { x : Int, y : Int, value : Int }

showPoint : Point -> String
showPoint p =
    "x=" ++ String.fromInt p.x ++ " y=" ++ String.fromInt p.y ++ " v=" ++ String.fromInt p.value

main =
    let
        pt = { x = 10, y = 20, value = 42 }
    in
    println (showPoint pt)
EOF
}

# ── Base for cat 2: simple arith with a let-bound variable ───────────────────
# Used for:
#   Cat 2 — reference undef_<SEED>
base_arith() {
    cat <<'EOF'
module Main exposing (main)

import Ipe.Log exposing (println)

main =
    let
        x = 5
        y = 10
    in
    println (String.fromInt (x + y))
EOF
}

# ── Base for cat 3: uses String module ───────────────────────────────────────
# Used for:
#   Cat 3 — String.nosuchfn_<SEED> x
base_string_ops() {
    cat <<'EOF'
module Main exposing (main)

import Ipe.Log exposing (println)

main =
    let
        s = "hello"
        n = String.length s
    in
    println (String.fromInt n)
EOF
}

# ── Base for cat 4b: if expression ───────────────────────────────────────────
# Used for:
#   Cat 4b — if <int-literal> then ... else ... (int is not Bool)
base_if_expr() {
    cat <<'EOF'
module Main exposing (main)

import Ipe.Log exposing (println)

main =
    let
        flag = True
        result = if flag then "yes" else "no"
    in
    println result
EOF
}

# ── Base for cat 5: Maybe ADT usage ──────────────────────────────────────────
# Used for:
#   Cat 5 — Nothing applied to an extra arg (`Nothing 42`)
# NOTE: Ipê's Rust backend does not support multi-clause pattern matching on
# function params (IPE-P0030 / top-level pattern dispatch). Use a single
# clause with a case expression instead.
base_maybe_usage() {
    cat <<'EOF'
module Main exposing (main)

import Ipe.Log exposing (println)

safeDiv : Int -> Int -> Maybe Int
safeDiv a b =
    if b == 0 then
        Nothing
    else
        Just (a // b)

main =
    let
        result = safeDiv 10 2
        val = Maybe.withDefault 0 result
    in
    println (String.fromInt val)
EOF
}

# ── Base for cat 6: 3-constructor ADT ────────────────────────────────────────
# Used for:
#   Cat 6 — case covers only 2 of 3 constructors (non-exhaustive)
base_adt_exhaustive() {
    cat <<'EOF'
module Main exposing (main)

import Ipe.Log exposing (println)

type Shape
    = Circle
    | Square
    | Triangle

describeShape : Shape -> String
describeShape s =
    case s of
        Circle -> "circle"
        Square -> "square"
        Triangle -> "triangle"

main =
    println (describeShape Circle ++ " " ++ describeShape Square ++ " " ++ describeShape Triangle)
EOF
}

# ═══════════════════════════════════════════════════════════════════════════════
# SIX MUTATION STRATEGIES
# Each function writes an ILL-TYPED Ipê program to stdout.
# ═══════════════════════════════════════════════════════════════════════════════

# Cat 1 — undefined field access (.value<SEED> doesn't exist on Point)
mutant_undef_field() {
    local seed=$1
    local suffix; suffix=$(printf '%04x' $(( seed & 0xFFFF )))
    cat <<EOF
module Main exposing (main)

import Ipe.Log exposing (println)

type alias Point =
    { x : Int, y : Int, value : Int }

main =
    let
        pt = { x = 10, y = 20, value = 42 }
    in
    println (String.fromInt pt.value${suffix})
EOF
    # Why ill-typed: `pt` is `Point` which has no field `value${suffix}` — IPE-T0012.
}

# Cat 2 — undefined variable (undef_<SEED> has no binding in scope)
mutant_undef_var() {
    local seed=$1
    local suffix; suffix=$(printf '%04x' $(( seed & 0xFFFF )))
    cat <<EOF
module Main exposing (main)

import Ipe.Log exposing (println)

main =
    let
        x = 5
        y = 10
    in
    println (String.fromInt (x + y + undef_${suffix}))
EOF
    # Why ill-typed: `undef_${suffix}` is not in scope — IPE-N0001.
}

# Cat 3 — unknown qualified member (String.<typo>_<SEED>)
mutant_unknown_member() {
    local seed=$1
    local suffix; suffix=$(printf '%04x' $(( seed & 0xFFFF )))
    cat <<EOF
module Main exposing (main)

import Ipe.Log exposing (println)

main =
    let
        s = "hello"
    in
    println (String.fromInt (String.nosuchfn_${suffix} s))
EOF
    # Why ill-typed: String module has no member `nosuchfn_${suffix}` — IPE-N0005.
}

# Cat 4a — type mismatch via String.length on an Int literal
mutant_type_mismatch_strlen() {
    local seed=$1
    local n; n=$(bint $(lcg_next "$seed") 1 9999)
    cat <<EOF
module Main exposing (main)

import Ipe.Log exposing (println)

main =
    println (String.fromInt (String.length $n))
EOF
    # Why ill-typed: String.length : String -> Int, but we pass Int literal $n — IPE-T0001.
}

# Cat 4b — type mismatch via if-condition that is Int, not Bool
mutant_type_mismatch_if() {
    local seed=$1
    local n; n=$(bint $(lcg_next "$seed") 1 9999)
    cat <<EOF
module Main exposing (main)

import Ipe.Log exposing (println)

main =
    let
        result = if $n then "yes" else "no"
    in
    println result
EOF
    # Why ill-typed: if-condition must be Bool; $n is an Int literal — IPE-T0001.
}

# Cat 5 — wrong constructor arity: Nothing applied to an extra Int arg
mutant_ctor_arity() {
    local seed=$1
    local n; n=$(bint $(lcg_next "$seed") 1 9999)
    cat <<EOF
module Main exposing (main)

import Ipe.Log exposing (println)

main =
    let
        val = Maybe.withDefault 0 (Nothing $n)
    in
    println (String.fromInt val)
EOF
    # Why ill-typed: Nothing : Maybe a has arity 0; applying it to $n is an
    # application of a non-function. Expect IPE-T0001 (type mismatch on
    # application: Maybe a is not a function).
}

# Cat 6 — non-exhaustive case (missing Triangle arm)
mutant_nonexhaustive_case() {
    # seed parameter unused (mutation is deterministic for this base)
    local _seed=$1
    cat <<'EOF'
module Main exposing (main)

import Ipe.Log exposing (println)

type Shape
    = Circle
    | Square
    | Triangle

describeShape : Shape -> String
describeShape s =
    case s of
        Circle -> "circle"
        Square -> "square"

main =
    println (describeShape Circle)
EOF
    # Why ill-typed: Shape has 3 constructors; the case covers only 2 (missing
    # Triangle). Expect IPE-T0010 (this case does not handle every possibility).
}

# Cat 7 — same-module 2-type use of an UNTYPED helper. The class-1 boundary
# scheme promotion generalizes untyped defs at MODULE boundaries only;
# same-module reuse at two types stays REJECTED (reference-parity semantics —
# see docs/architecture/class1-inference-fix-spec-2026-07-09.md). This is the
# #66-N false-acceptance canary: if the promotion ever over-generalizes to
# same-module uses, this mutant is silently ACCEPTED and the fuzzer flags it.
mutant_same_module_2type() {
    local seed=$1
    local n; n=$(( (seed % 90) + 1 ))
    cat <<EOF
module Main exposing (main)

import Ipe.Log exposing (println)

ident x = x

main =
    println (String.fromInt (ident $n) ++ ident "s$n")
EOF
    # Why ill-typed (by current semantics): `ident` is untyped and used at Int
    # AND String within its own module — same-module reuse is monomorphic.
    # Expect IPE-T0001 (empirically verified 2026-07-11).
}

# Cat 8 — CROSS-MODULE use of an untyped Number-bounded helper at an
# incompatible instantiated type (`Lib.inc "str"` where `inc n = n + 1`).
# The boundary scheme promotion must instantiate the imported scheme fresh
# AND still carry the Number bound — a String argument is an ordinary
# IPE-T0001, never an acceptance. Writes a Lib.ipe sibling (multi-module).
mutant_cross_module_bad_inst() {
    local seed=$1 libdst=$2
    local n; n=$(( (seed % 90) + 1 ))
    cat > "$libdst" <<'EOF'
module Lib exposing (inc)


inc n = n + 1
EOF
    cat <<EOF
module Main exposing (main)

import Lib
import Ipe.Log exposing (println)

main =
    println (String.fromInt (Lib.inc "oops$n"))
EOF
    # Why ill-typed: Lib.inc's promoted scheme is Number-bounded (n + 1);
    # instantiating it at String violates the bound — IPE-T0001
    # (empirically verified 2026-07-11).
}

# ═══════════════════════════════════════════════════════════════════════════════
# CATALOGUE: list of (mutant_fn, expected_code, category_label)
# ═══════════════════════════════════════════════════════════════════════════════
#
#  Index  Mutant fn                   Expected code  Category label
#  0      undef_field                 IPE-T0012      cat1
#  1      undef_var                   IPE-N0001      cat2
#  2      unknown_member              IPE-N0005      cat3
#  3      type_mismatch_strlen        IPE-T0001      cat4a
#  4      type_mismatch_if            IPE-T0001      cat4b
#  5      ctor_arity                  IPE-T0001      cat5
#  6      nonexhaustive_case          IPE-T0010      cat6
#  7      same_module_2type           IPE-T0001      cat7 (#66-N canary)
#  8      cross_module_bad_inst       IPE-T0001      cat8 (multi-module)
#
# Total: 9 entries in the catalogue.

CATALOGUE_SIZE=9

# catalogue_fn idx seed srcdir — writes Main.ipe to stdout; multi-module
# entries (cat 8) additionally write "$srcdir/Lib.ipe" as a side effect.
catalogue_fn() {
    local idx=$1 seed=$2 srcdir=$3
    case $idx in
        0) mutant_undef_field       "$seed" ;;
        1) mutant_undef_var         "$seed" ;;
        2) mutant_unknown_member    "$seed" ;;
        3) mutant_type_mismatch_strlen "$seed" ;;
        4) mutant_type_mismatch_if  "$seed" ;;
        5) mutant_ctor_arity        "$seed" ;;
        6) mutant_nonexhaustive_case "$seed" ;;
        7) mutant_same_module_2type "$seed" ;;
        8) mutant_cross_module_bad_inst "$seed" "$srcdir/Lib.ipe" ;;
    esac
}

catalogue_code() {
    local idx=$1
    case $idx in
        0) echo "IPE-T0012" ;;
        1) echo "IPE-N0001" ;;
        2) echo "IPE-N0005" ;;
        3) echo "IPE-T0001" ;;
        4) echo "IPE-T0001" ;;
        5) echo "IPE-T0001" ;;
        6) echo "IPE-T0010" ;;
        7) echo "IPE-T0001" ;;
        8) echo "IPE-T0001" ;;
    esac
}

catalogue_label() {
    local idx=$1
    case $idx in
        0) echo "cat1-undef-field" ;;
        1) echo "cat2-undef-var" ;;
        2) echo "cat3-unknown-member" ;;
        3) echo "cat4a-type-mismatch-strlen" ;;
        4) echo "cat4b-type-mismatch-if" ;;
        5) echo "cat5-ctor-arity" ;;
        6) echo "cat6-nonexhaustive-case" ;;
        7) echo "cat7-same-module-2type-canary" ;;
        8) echo "cat8-cross-module-bad-inst" ;;
    esac
}

# ── Directories ───────────────────────────────────────────────────────────────
FUZZ_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ipe-fuzz-neg.XXXXXX")"
FAILURES_DIR="/tmp/ipe-fuzz-neg/FAILURES"
mkdir -p "$FAILURES_DIR"

cleanup() {
    if [[ "$KEEP" -eq 0 ]]; then rm -rf "$FUZZ_DIR"; fi
}
trap cleanup EXIT

# ── Save a false-acceptance for forensics ─────────────────────────────────────
save_false_acceptance() {
    local seed=$1 iterdir=$2 label=$3 code=$4
    local ts; ts=$(date +%s)
    local dst="$FAILURES_DIR/seed-${seed}-${ts}"
    mkdir -p "$dst"
    cp -rf "$iterdir/src"      "$dst/"     2>/dev/null || true
    cp -f  "$iterdir/ipe.toml" "$dst/"     2>/dev/null || true
    cp -f  "$iterdir/build.log" "$dst/"    2>/dev/null || true
    printf 'seed=%s label=%s expected_code=%s\n' \
        "$seed" "$label" "$code" > "$dst/SUMMARY"
    printf '\n=== FALSE ACCEPTANCE BUG REPORT ===\n'
    printf 'ipe accepted an ILL-TYPED program:\n'
    printf '  seed:   %s\n' "$seed"
    printf '  label:  %s\n' "$label"
    printf '  expect: %s\n' "$code"
    printf '  src:    %s/src/Main.ipe\n' "$dst"
    printf '  log:    %s/build.log\n' "$dst"
    printf 'This is a real compiler soundness bug — please report it.\n'
    printf '=====================================\n\n'
}

# ═══════════════════════════════════════════════════════════════════════════════
# BASE SANITY CHECK — prove every base compiles clean
# ═══════════════════════════════════════════════════════════════════════════════
run_base_sanity() {
    echo "=== BASE SANITY CHECK ==="
    echo "Verifying all 6 base programs compile clean (proves the harness"
    echo "is not falsely rejecting everything)."
    echo ""

    local bases=(
        "record"
        "arith"
        "string_ops"
        "if_expr"
        "maybe_usage"
        "adt_exhaustive"
    )
    local labels=(
        "base_record (cat1/4 base)"
        "base_arith (cat2 base)"
        "base_string_ops (cat3 base)"
        "base_if_expr (cat4b base)"
        "base_maybe_usage (cat5 base)"
        "base_adt_exhaustive (cat6 base)"
    )

    local all_ok=1
    for i in "${!bases[@]}"; do
        local name="${bases[$i]}"
        local label="${labels[$i]}"
        local bdir="$FUZZ_DIR/base-$name"
        setup_project "$bdir"
        "base_$name" > "$bdir/src/Main.ipe"
        if run_ipe_expect_accept "$bdir"; then
            printf '  OK:   %s\n' "$label"
        else
            printf '  FAIL: %s — base did not compile (harness bug or compiler regression)\n' "$label"
            printf '        Log:\n'
            if [[ -f "$bdir/build.log" ]]; then
                sed 's/^/        /' "$bdir/build.log"
            fi
            all_ok=0
        fi
    done

    echo ""
    if [[ "$all_ok" -eq 1 ]]; then
        echo "BASE SANITY: ALL 6 BASES COMPILE CLEAN."
        return 0
    else
        echo "BASE SANITY: FAILED — at least one base rejected (harness or compiler bug)."
        return 1
    fi
}

# ═══════════════════════════════════════════════════════════════════════════════
# CATEGORY DEMO — run one mutant per category, show the diagnostic code
# ═══════════════════════════════════════════════════════════════════════════════
run_cat_demo() {
    echo "=== CATEGORY DEMO ==="
    echo "One mutant per catalogue entry. Each MUST be rejected."
    echo ""

    local demo_seed=12345
    local demo_ok=1

    for idx in $(seq 0 $(( CATALOGUE_SIZE - 1 )) ); do
        local label; label=$(catalogue_label "$idx")
        local code;  code=$(catalogue_code "$idx")
        local ddir="$FUZZ_DIR/demo-$idx"
        setup_project "$ddir"
        catalogue_fn "$idx" "$demo_seed" "$ddir/src" > "$ddir/src/Main.ipe"

        printf '[cat %d] %-34s expect %s\n' "$idx" "$label" "$code"

        local log="$ddir/build.log"
        : > "$log"
        local rc=0
        ( cd "$ddir" && timeout "$BUILD_TIMEOUT" \
          "$IPE_BIN" build src/Main.ipe --out out/rust >"$log" 2>&1 ) || rc=$?

        if [[ "$rc" -eq 0 ]]; then
            printf '  RESULT: FALSE ACCEPTANCE (ipe exit 0) — SOUNDNESS BUG!\n'
            printf '  src: %s/src/Main.ipe\n' "$ddir"
            demo_ok=0
        elif [[ "$rc" -eq 124 ]]; then
            printf '  RESULT: TIMEOUT (ipe hung)\n'
            demo_ok=0
        else
            if grep -q "$code" "$log" 2>/dev/null; then
                printf '  RESULT: REJECTED with %s (correct)\n' "$code"
            else
                # Still a valid rejection — code might have a different pattern on stderr.
                # Extract any IPE- code from the log for reporting.
                local found_code; found_code=$(grep -oP 'IPE-[A-Z][0-9]+' "$log" | head -1)
                if [[ -n "$found_code" ]]; then
                    printf '  RESULT: REJECTED with %s (expected %s — check catalogue)\n' \
                        "$found_code" "$code"
                else
                    printf '  RESULT: REJECTED (exit %d, no IPE- code in log)\n' "$rc"
                fi
            fi
            # Show first 5 lines of log with any IPE- code for documentation.
            local first_line; first_line=$(grep -m1 'IPE-\|error\[' "$log" 2>/dev/null || true)
            if [[ -n "$first_line" ]]; then
                printf '  diag:  %s\n' "$first_line"
            fi
        fi
        echo ""
    done

    if [[ "$demo_ok" -eq 1 ]]; then
        echo "CAT DEMO: ALL $CATALOGUE_SIZE MUTANTS REJECTED."
        return 0
    else
        echo "CAT DEMO: FAILURES — see above."
        return 1
    fi
}

# ═══════════════════════════════════════════════════════════════════════════════
# ONE FUZZ ITERATION
# ═══════════════════════════════════════════════════════════════════════════════
# Returns:
#   0  = mutant correctly rejected
#   1  = false acceptance (soundness bug) or timeout (also a bug to investigate)
run_iter() {
    local seed=$1 iterdir=$2

    # Pick a catalogue entry deterministically from the seed.
    local idx; idx=$(( seed % CATALOGUE_SIZE ))
    local label; label=$(catalogue_label "$idx")
    local code;  code=$(catalogue_code "$idx")

    setup_project "$iterdir"
    catalogue_fn "$idx" "$seed" "$iterdir/src" > "$iterdir/src/Main.ipe"

    local log="$iterdir/build.log"
    : > "$log"
    local rc=0
    ( cd "$iterdir" && timeout "$BUILD_TIMEOUT" \
      "$IPE_BIN" build src/Main.ipe --out out/rust >"$log" 2>&1 ) || rc=$?

    if [[ "$rc" -eq 0 ]]; then
        echo "FALSE-ACCEPTANCE label=$label expected=$code"
        return 1
    fi
    if [[ "$rc" -eq 124 ]]; then
        echo "TIMEOUT label=$label expected=$code"
        return 1
    fi

    # ipe rejected — correct. Report how closely the code matched.
    if grep -q "$code" "$log" 2>/dev/null; then
        echo "REJECTED-OK label=$label code=$code"
    else
        local found; found=$(grep -oP 'IPE-[A-Z][0-9]+' "$log" | head -1 || true)
        if [[ -n "$found" ]]; then
            echo "REJECTED-DIFF-CODE label=$label expected=$code got=$found"
        else
            echo "REJECTED-NO-CODE label=$label expected=$code"
        fi
    fi
    return 0
}

# ── Mode: base sanity ─────────────────────────────────────────────────────────
if [[ "$BASE_SANITY" -eq 1 ]]; then
    run_base_sanity
    exit $?
fi

# ── Mode: cat demo ────────────────────────────────────────────────────────────
if [[ "$CAT_DEMO" -eq 1 ]]; then
    run_cat_demo
    exit $?
fi

# ═══════════════════════════════════════════════════════════════════════════════
# MAIN LOOP
# ═══════════════════════════════════════════════════════════════════════════════
echo "ipe-fuzz-neg: mode=ill-typed iters=$ITERS start_seed=$SEED"
echo "ipe-fuzz-neg: ipe=$IPE_BIN"
echo "ipe-fuzz-neg: catalogue_size=$CATALOGUE_SIZE build_timeout=${BUILD_TIMEOUT}s"
echo "ipe-fuzz-neg: failures_dir=$FAILURES_DIR"
echo "ipe-fuzz-neg: property: every ill-typed mutant must be REJECTED (exit != 0)"
echo ""

# Track per-category coverage.
declare -A cat_hits
for idx in $(seq 0 $(( CATALOGUE_SIZE - 1 ))); do
    cat_hits[$idx]=0
done

start_ts=$(date +%s)
false_acceptances=0
timeouts=0
rejected=0
rejected_ok=0

for (( i = 0; i < ITERS; i++ )); do
    iter_seed=$(( SEED + i ))
    iterdir="$FUZZ_DIR/iter-$i"
    mkdir -p "$iterdir"

    reason=$(run_iter "$iter_seed" "$iterdir")
    rc=$?

    # Track category coverage.
    local_idx=$(( iter_seed % CATALOGUE_SIZE ))
    cat_hits[$local_idx]=$(( ${cat_hits[$local_idx]:-0} + 1 ))

    if [[ "$rc" -ne 0 ]]; then
        # Failure: false acceptance or timeout.
        if [[ "$reason" == FALSE-ACCEPTANCE* ]]; then
            false_acceptances=$(( false_acceptances + 1 ))
            label_part="${reason#FALSE-ACCEPTANCE label=}"
            label_name="${label_part%% *}"
            code_part="${label_part##* expected=}"
            save_false_acceptance "$iter_seed" "$iterdir" "$label_name" "$code_part"
            echo "FAIL iter=$i seed=$iter_seed $reason" >&2
            echo "" >&2
            echo "ipe-fuzz-neg: ABORTING — soundness bug found." >&2
            echo "ipe-fuzz-neg: reproduce: $0 --seed $iter_seed --iters 1 --keep" >&2
            exit 1
        else
            # Timeout: also a bug to investigate (but don't abort the run,
            # just count it and warn).
            timeouts=$(( timeouts + 1 ))
            echo "WARN iter=$i seed=$iter_seed $reason" >&2
        fi
    else
        rejected=$(( rejected + 1 ))
        if [[ "$reason" == REJECTED-OK* ]]; then
            rejected_ok=$(( rejected_ok + 1 ))
        fi
    fi

    # Clean up successful iters to save disk.
    rm -rf "$iterdir"

    if [[ "$QUIET" -eq 0 && $(( (i + 1) % 10 )) -eq 0 ]]; then
        elapsed=$(( $(date +%s) - start_ts ))
        rate=$(awk -v g="$rejected" -v e="$elapsed" \
            'BEGIN { if (e>0) printf "%.1f", g/e; else print "-" }')
        echo "  progress: $((i + 1))/$ITERS rejected=$rejected timeouts=$timeouts fa=$false_acceptances elapsed=${elapsed}s rate=${rate}/s"
    fi
done

elapsed=$(( $(date +%s) - start_ts ))

echo ""
echo "ipe-fuzz-neg: DONE"
echo "  iters=$ITERS rejected=$rejected rejected_with_correct_code=$rejected_ok timeouts=$timeouts false_acceptances=$false_acceptances elapsed=${elapsed}s"
echo ""
echo "  Category coverage:"
for idx in $(seq 0 $(( CATALOGUE_SIZE - 1 ))); do
    printf '    %s: %d iters\n' "$(catalogue_label "$idx")" "${cat_hits[$idx]:-0}"
done
echo ""

if [[ "$false_acceptances" -gt 0 ]]; then
    echo "ipe-fuzz-neg: FAILED — $false_acceptances false acceptance(s) found."
    echo "  Forensics: $FAILURES_DIR"
    exit 1
fi

if [[ "$timeouts" -gt 0 ]]; then
    echo "ipe-fuzz-neg: WARNING — $timeouts timeout(s). Investigate: ipe hung on some ill-typed programs."
fi

if [[ "$rejected" -eq "$ITERS" ]]; then
    echo "ipe-fuzz-neg: PASS — all $ITERS mutants correctly rejected."
    echo "  0 false acceptances (soundness property holds for this run)."
    if (( ITERS >= 1000 )); then
        echo "  Full gate SATISFIED — 1000+ iters clean."
    else
        echo "  Smoke PASS. Full gate: IPE_FUZZ_NEG_FULL=1 ./tools/scripts/fuzz-ill-typed.sh"
    fi
    exit 0
else
    # Some iters had timeouts but no false acceptances.
    echo "ipe-fuzz-neg: PARTIAL PASS — $rejected/$ITERS rejected cleanly ($timeouts timeouts, 0 false acceptances)."
    if [[ "$timeouts" -gt 0 ]]; then
        exit 1
    fi
    exit 0
fi
