#!/usr/bin/env bash
#
# migrate-golden-harness.sh — mechanically migrate `crates/skyc/tests/golden_*.rs`
# byte-diff tests from the ad hoc `read_to_string(<out>/src/main.rs)` +
# `assert_eq!(..)` pattern to the shared
# `support::assert_emitted_project_matches_golden_dir` helper, and replace each
# file's local `fn repo_root() -> PathBuf { .. }` duplicate with an import of the
# shared `support::repo_root`.
#
# This is the codemod referenced by the phase-5 emit_rust_file design (§2.4 step 4
# / Task 8a). It is DELIBERATELY CONSERVATIVE: it only rewrites a file when the
# file matches the CANONICAL shape exactly (the shape `golden_m0.rs` /
# `golden_m1_tuples.rs` carried), and otherwise leaves the file byte-for-byte
# untouched and reports it as "skipped (nonstandard)". Nonstandard shapes
# (`.expect("main.rs")`, `.ok()`, `.unwrap_or_default()`, renamed locals, files
# with no `assert_eq!` at all) are left for hand migration in a later batch —
# silently mangling them would be worse than skipping them.
#
# Idempotent: re-running on an already-migrated file is a no-op (the canonical
# `read_to_string`/`assert_eq!` block is gone, so nothing matches).
#
# Usage:
#   scripts/migrate-golden-harness.sh [FILE ...]
# With no arguments, operates on every `crates/skyc/tests/golden_*.rs` except
# `golden_m0.rs` (already migrated by hand). Pass explicit files to scope a batch.
#
# After running, ALWAYS:
#   rustfmt --edition 2024 <changed files>
#   cargo test -p skyc --test <changed test> ...   # prove each batch green
#
set -euo pipefail

repo_root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tests_dir="$repo_root_dir/crates/skyc/tests"

if [[ $# -gt 0 ]]; then
    files=("$@")
else
    files=()
    for f in "$tests_dir"/golden_*.rs; do
        # `golden_m0.rs` is migrated by hand — never touch it here.
        [[ "$f" == */golden_m0.rs ]] && continue
        files+=("$f")
    done
fi

migrated=0
skipped=0

for file in "${files[@]}"; do
    # NOTE: `set -e` is in force. The per-file python heredoc uses its EXIT CODE
    # as a signal (0 = migrated, 2 = skip/nonstandard, other = hard error), so we
    # MUST NOT let a non-zero exit abort the whole batch before `rc=$?` is read.
    # Guard the invocation with `|| rc=$?`: on exit 0 the `||` short-circuits and
    # `rc` is set to 0 below; on any non-zero exit `rc` captures it and the loop
    # continues to classify it. Without this guard, the first nonstandard golden
    # (exit 2) killed the entire script before a single canonical file after it
    # could be migrated, and the summary counters were never reached.
    rc=0
    python3 - "$file" <<'PY' || rc=$?
import re
import sys

path = sys.argv[1]
with open(path, "r", encoding="utf-8") as fh:
    src = fh.read()
original = src

# 1. The canonical byte-diff block:
#
#     let emitted = std::fs::read_to_string(out.join("src").join("main.rs"));
#     let want = std::fs::read_to_string(&golden);
#     assert!(emitted.is_ok() && want.is_ok(), "both files must read");
#     assert_eq!(
#         emitted.ok(),
#         want.ok(),
#         "emitted main.rs must equal the golden byte-for-byte"
#     );
#
# `out` and `golden` (the variable names) and the whole structural block must
# match exactly — only the human-readable assert MESSAGE is allowed to vary
# (some goldens run this block from a `assert_byte_identical(name)` helper and
# word the message `... for {name} ...`). The message is a diagnostic string,
# not load-bearing for the assertion's meaning, so matching it verbatim is
# needless brittleness; matching its stable `emitted main.rs ... byte-for-byte`
# skeleton keeps the rewrite exact where it matters (the compared operands) and
# tolerant where it does not. Anything that diverges structurally (different
# variable names, `.expect()` instead of `.ok()`, `assert_eq!(emitted, want)`
# on raw strings, more than one such block) is still refused for hand migration.
block_re = re.compile(
    r'''[ \t]*let\ emitted\ =\ std::fs::read_to_string\(out\.join\("src"\)\.join\("main\.rs"\)\);\n'''
    r'''[ \t]*let\ want\ =\ std::fs::read_to_string\(&golden\);\n'''
    r'''[ \t]*assert!\(emitted\.is_ok\(\)\ &&\ want\.is_ok\(\),\ "both\ files\ must\ read"\);\n'''
    r'''[ \t]*assert_eq!\(\n'''
    r'''[ \t]*emitted\.ok\(\),\n'''
    r'''[ \t]*want\.ok\(\),\n'''
    r'''[ \t]*"emitted\ main\.rs\ [^"\n]*byte-for-byte",?\n'''
    r'''[ \t]*\);'''
)

replacement = (
    '    // Directory-diff the emitted project against the golden dir (byte-compares\n'
    '    // the emitted `src/main.rs` against the golden `main.rs`). Replaces the\n'
    '    // former hand-rolled `read_to_string` + `assert_eq!` pair with the shared\n'
    '    // harness helper.\n'
    '    support::assert_emitted_project_matches_golden_dir(\n'
    '        &out,\n'
    '        golden.parent().expect("golden has a parent dir"),\n'
    '    );'
)

new_src, n = block_re.subn(replacement, src)
if n == 0:
    print(f"SKIP {path} (no canonical byte-diff block)")
    sys.exit(2)
src = new_src

# 2. Replace the local `fn repo_root() -> PathBuf { .. }` with a shared import.
#    Only the canonical two-line body is matched; a divergent body is left in
#    place (and the file still compiles, just keeping its local helper).
localfn_re = re.compile(
    r'''(?:/// [^\n]*\n)?'''
    r'''fn repo_root\(\) -> PathBuf \{\n'''
    r'''[ \t]*let joined = Path::new\(env!\("CARGO_MANIFEST_DIR"\)\)\.join\("\.\."\)\.join\("\.\."\);\n'''
    r'''[ \t]*std::fs::canonicalize\(&joined\)\.unwrap_or\(joined\)\n'''
    r'''\}\n'''
)

def repl_localfn(_m):
    return "use support::repo_root;\n"

src, m = localfn_re.subn(repl_localfn, src, count=1)
# If the local fn was present, we swapped in `use support::repo_root;` where it
# stood. If it was NOT present, the file already relies on the shared import or a
# differently-shaped helper — either way we leave imports as-is.

if src != original:
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(src)
    print(f"MIGRATED {path}")
    sys.exit(0)
else:
    print(f"SKIP {path} (unchanged)")
    sys.exit(2)
PY
    # `rc` was captured by the `|| rc=$?` guard on the heredoc invocation above
    # (0 when python exited 0 and the `||` was not taken; the python exit code
    # otherwise). Do NOT re-read `$?` here — the `|| rc=$?` compound always
    # succeeds, so `$?` would be 0 and clobber a genuine skip/error signal.
    if [[ $rc -eq 0 ]]; then
        migrated=$((migrated + 1))
    elif [[ $rc -eq 2 ]]; then
        skipped=$((skipped + 1))
    else
        echo "ERROR processing $file (python exit $rc)" >&2
        exit 1
    fi
done

echo "---"
echo "migrated: $migrated   skipped (nonstandard/unchanged): $skipped"
echo "Now run: rustfmt --edition 2024 <migrated files> && cargo test -p skyc --test <...>"
