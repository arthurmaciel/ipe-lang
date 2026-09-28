#!/usr/bin/env bash
# Self-test for tools/scripts/lib/require-tool.sh — proves the fail-closed
# refusals a missing-tool gate must take, so the mechanism can't silently
# regress to "if rg …; then" reading exit 127 as "no match" (issue #3036).
#
# Exit 0 when every case behaves; prints the failing case(s) and exits 1
# otherwise.
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
lib="$repo_root/tools/scripts/lib/require-tool.sh"

fail=0
check() {
    local desc="$1" got="$2" want="$3"
    if [ "$got" != "$want" ]; then
        echo "FAIL: $desc — got '$got', want '$want'" >&2
        fail=1
    else
        echo "ok: $desc"
    fi
}

# ── an rg-free PATH: every ordinary coreutils/bash/dirname/etc. tool stays
# reachable so the harness itself keeps working, but `rg` specifically is
# absent — isolating "rg is missing" from "PATH is empty". ─────────────────
rg_free_path="$(mktemp -d)"
for d in /usr/local/bin /usr/bin /bin; do
    [ -d "$d" ] || continue
    for f in "$d"/*; do
        [ -e "$f" ] || continue
        b="$(basename "$f")"
        [ "$b" = rg ] && continue
        [ -e "$rg_free_path/$b" ] && continue
        ln -s "$f" "$rg_free_path/$b" 2>/dev/null
    done
done

fixture_dir="$(mktemp -d)"
trap 'rm -rf "$rg_free_path" "$fixture_dir"' EXIT

# ── require_tool: a missing tool exits 2, a present one exits 0 ─────────────
rc=0
PATH="$rg_free_path" bash -c "source '$lib'; require_tool rg" >/dev/null 2>&1 || rc=$?
check "require_tool exits 2 when the tool is missing" "$rc" 2

rc=0
bash -c "source '$lib'; require_tool bash" >/dev/null 2>&1 || rc=$?
check "require_tool exits 0 when the tool is present" "$rc" 0

# ── rg_status: exit 1 is no-match, exit 2 (or any non-0/1) is error ─────────
got="$(bash -c "source '$lib'; rg_status 0")"
check "rg_status 0 -> match" "$got" match
got="$(bash -c "source '$lib'; rg_status 1")"
check "rg_status 1 -> no-match" "$got" no-match
got="$(bash -c "source '$lib'; rg_status 2")"
check "rg_status 2 -> error" "$got" error
got="$(bash -c "source '$lib'; rg_status 127")"
check "rg_status 127 (command not found) -> error" "$got" error

# ── match_or_fail: mirrors rg_status through a real command's exit code ─────
rc=0
bash -c "source '$lib'; match_or_fail t -- false" >/dev/null 2>&1
rc=$?
check "match_or_fail: exit 1 (no match) returns 1, doesn't hard-fail" "$rc" 1

rc=0
bash -c "source '$lib'; match_or_fail t -- true" >/dev/null 2>&1
rc=$?
check "match_or_fail: exit 0 (match) returns 0" "$rc" 0

rc=0
bash -c "source '$lib'; match_or_fail t -- bash -c 'exit 2'" >/dev/null 2>&1
rc=$?
check "match_or_fail: exit 2 (rg error) hard-exits 2" "$rc" 2

rc=0
PATH="$rg_free_path" bash -c "source '$lib'; match_or_fail t -- rg foo bar" >/dev/null 2>&1
rc=$?
check "match_or_fail: command-not-found (127) hard-exits, never reads as no-match" "$rc" 2

# ── the actual gate: fails on a fixture with a known tone violation ─────────
goldens_fixture="$fixture_dir/render_goldens"
explain_fixture="$fixture_dir/explain"
mkdir -p "$goldens_fixture" "$explain_fixture"
printf 'IPE-T0001: type mismatch\n\nsee salsa for details\n' > "$goldens_fixture/violation.txt"

rc=0
GOLDENS_DIR="$goldens_fixture" EXPLAIN_DIR="$explain_fixture" \
    bash "$repo_root/tools/scripts/lint-diagnostic-tone.sh" >/dev/null 2>&1 || rc=$?
check "diagnostic-tone gate fails on a fixture jargon violation" "$rc" 1

rm -f "$goldens_fixture/violation.txt"
printf 'IPE-T0001: type mismatch\n\nno jargon here\n' > "$goldens_fixture/clean.txt"
rc=0
GOLDENS_DIR="$goldens_fixture" EXPLAIN_DIR="$explain_fixture" \
    bash "$repo_root/tools/scripts/lint-diagnostic-tone.sh" >/dev/null 2>&1 || rc=$?
check "diagnostic-tone gate passes on a clean fixture" "$rc" 0

# ── the actual gate: hard-fails (never vacuously passes) when rg is absent ──
rc=0
printf 'IPE-T0001: type mismatch\n\nsee salsa for details\n' > "$goldens_fixture/violation.txt"
GOLDENS_DIR="$goldens_fixture" EXPLAIN_DIR="$explain_fixture" PATH="$rg_free_path" \
    bash "$repo_root/tools/scripts/lint-diagnostic-tone.sh" >/dev/null 2>&1 || rc=$?
check "diagnostic-tone gate exits 2 (not 0) when rg is missing from PATH" "$rc" 2

if [ "$fail" -ne 0 ]; then
    echo "test-require-tool: FAILED" >&2
    exit 1
fi
echo "test-require-tool: all cases pass"
