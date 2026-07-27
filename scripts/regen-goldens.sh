#!/usr/bin/env bash
# Regenerate every checked-in golden `main.rs` from its `Main.ipe` using the
# freshly-built `ipe` compiler. A golden dir that ships a `main.rs` is a
# byte-diff golden; rebuild it and copy the emitted `src/main.rs` over the
# stored one. Dirs with only a `Main.ipe` (SEAL/run/exit-0 goldens) are left
# untouched — there is nothing to byte-diff.
#
# Usage:
#   source scripts/lib/env.sh        # exports IPE_BIN + IPE_RUNTIME_DIR
#   scripts/regen-goldens.sh [--check]
#
# --check regenerates into a scratch dir and DIFFS instead of overwriting,
# exiting non-zero on any drift (a dry-run gate).

set -euo pipefail

REPO="$(git rev-parse --show-toplevel)"
GOLDEN_ROOT="$REPO/tests/golden"
CHECK=0
[[ "${1:-}" == "--check" ]] && CHECK=1

: "${IPE_BIN:?source scripts/lib/env.sh first (IPE_BIN unset)}"
: "${IPE_RUNTIME_DIR:?source scripts/lib/env.sh first (IPE_RUNTIME_DIR unset)}"

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

regenerated=0
drift=0
skipped=0

for dir in "$GOLDEN_ROOT"/*/; do
    name="$(basename "$dir")"
    main_ipe="$dir/Main.ipe"
    golden_main_rs="$dir/main.rs"
    # Only regenerate dirs that already ship a byte-diff `main.rs`.
    [[ -f "$main_ipe" && -f "$golden_main_rs" ]] || { skipped=$((skipped + 1)); continue; }

    out="$scratch/$name"
    rm -rf "$out"
    # Build from the golden's own dir so `--out` is relative-clean; the emitted
    # project lands under "$out/src/main.rs".
    if ! ( cd "$dir" && timeout 120 "$IPE_BIN" build "Main.ipe" --out "$out" ) \
            > "$scratch/$name.log" 2>&1; then
        echo "SKIP $name: ipe build failed (not a plain-emit golden?)" >&2
        skipped=$((skipped + 1))
        continue
    fi

    emitted="$out/src/main.rs"
    [[ -f "$emitted" ]] || { echo "SKIP $name: no emitted src/main.rs" >&2; skipped=$((skipped + 1)); continue; }

    if [[ "$CHECK" == "1" ]]; then
        if ! diff -q "$golden_main_rs" "$emitted" > /dev/null 2>&1; then
            echo "DRIFT $name"
            drift=$((drift + 1))
        fi
    else
        cp -f "$emitted" "$golden_main_rs"
        regenerated=$((regenerated + 1))
    fi
done

if [[ "$CHECK" == "1" ]]; then
    echo "checked goldens; drift=$drift skipped=$skipped"
    [[ "$drift" == "0" ]]
else
    echo "regenerated $regenerated golden main.rs; skipped=$skipped"
fi
