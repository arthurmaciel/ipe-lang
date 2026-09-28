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
# cause_of <output> <substring>: "named" when the output carries the expected
# cause, else the output itself — so a refusal is proven to fire for the reason
# under test, not an unrelated earlier exit 2.
cause_of() {
    case "$1" in *"$2"*) echo named ;; *) printf '%s' "$1" ;; esac
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
# The gate scans explain_fixture too (jargon + jargon_cased), so
# require_scan_root needs a matching *.md file in it from the start — an
# emptied scan root must fail closed (exit 2), so every gate invocation below
# needs a real file present in BOTH scanned dirs, not just the one under test.
printf '# IPE-T0001\n\nno jargon here either\n' > "$explain_fixture/IPE-T0001.md"
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
rm -f "$goldens_fixture/violation.txt"

# ── require_scan_root: a missing or emptied scan root must fail closed (exit
# 2), never read as "clean" because rg then simply finds nothing ───────────
nonexistent_dir="$fixture_dir/does-not-exist"
rc=0
GOLDENS_DIR="$nonexistent_dir" EXPLAIN_DIR="$explain_fixture" \
    bash "$repo_root/tools/scripts/lint-diagnostic-tone.sh" >/dev/null 2>&1 || rc=$?
check "diagnostic-tone gate exits 2 when GOLDENS_DIR does not exist" "$rc" 2

empty_goldens="$fixture_dir/empty-goldens"
mkdir -p "$empty_goldens"
rc=0
GOLDENS_DIR="$empty_goldens" EXPLAIN_DIR="$explain_fixture" \
    bash "$repo_root/tools/scripts/lint-diagnostic-tone.sh" >/dev/null 2>&1 || rc=$?
check "diagnostic-tone gate exits 2 when GOLDENS_DIR has no matching file" "$rc" 2
rmdir "$empty_goldens"

rc=0
bash -c "source '$lib'; require_scan_root '$nonexistent_dir' '*.txt'" >/dev/null 2>&1 || rc=$?
check "require_scan_root exits 2 when the dir is missing" "$rc" 2

mkdir -p "$fixture_dir/empty-root"
rc=0
bash -c "source '$lib'; require_scan_root '$fixture_dir/empty-root' '*.txt'" >/dev/null 2>&1 || rc=$?
check "require_scan_root exits 2 when the dir has no matching file" "$rc" 2
rmdir "$fixture_dir/empty-root"

rc=0
bash -c "source '$lib'; require_scan_root '$goldens_fixture' '*.txt'" >/dev/null 2>&1 || rc=$?
check "require_scan_root exits 0 when the dir has a matching file" "$rc" 0

# ── match_capture: mirrors match_or_fail, plus captures the matched text ───
got="$(bash -c "source '$lib'; match_capture v t -- printf 'a\nb\n'; printf '%s' \"\$v\"")"
check "match_capture: match captures the command's stdout" "$got" "$(printf 'a\nb')"

rc=0
bash -c "source '$lib'; match_capture v t -- false" >/dev/null 2>&1
rc=$?
check "match_capture: exit 1 (no match) returns 1, doesn't hard-fail" "$rc" 1

rc=0
bash -c "source '$lib'; match_capture v t -- true" >/dev/null 2>&1
rc=$?
check "match_capture: exit 0 (match) returns 0" "$rc" 0

rc=0
bash -c "source '$lib'; match_capture v t -- bash -c 'exit 2'" >/dev/null 2>&1
rc=$?
check "match_capture: exit 2 hard-exits 2" "$rc" 2

# ── direct-command rule: a shell function (which can hide a pipeline's failing
# first stage behind a later stage's "no match") is refused outright ────────
for helper in "match_or_fail t" "match_capture v t" "capture_nul v t"; do
    rc=0
    bash -c "source '$lib'; wrapped() { false | true; }; $helper -- wrapped" >/dev/null 2>&1 || rc=$?
    check "${helper%% *}: refuses a shell-function command (exit 2)" "$rc" 2
done

# ── capture_nul / enumerate_files: producer failure or empty set exits 2 ────
rc=0
bash -c "source '$lib'; capture_nul v t -- bash -c 'printf \"a\\0\"; exit 128'" >/dev/null 2>&1 || rc=$?
check "capture_nul: a producer exiting 128 after partial output hard-exits 2" "$rc" 2

got="$(bash -c "source '$lib'; capture_nul v t -- printf 'a b\\0c\\0'; printf '%s|' \"\${v[@]}\"")"
check "capture_nul: loads NUL-delimited records intact" "$got" "a b|c|"

mkdir -p "$fixture_dir/enum/sub" "$fixture_dir/enum-empty"
printf 'x\n' > "$fixture_dir/enum/sub/b.ipe"
printf 'x\n' > "$fixture_dir/enum/a.ipe"
got="$(bash -c "source '$lib'; enumerate_files v '*.ipe' '$fixture_dir/enum'; printf '%s|' \"\${v[@]#$fixture_dir/enum/}\"")"
check "enumerate_files: sorted set of every matching file" "$got" "a.ipe|sub/b.ipe|"

rc=0
bash -c "source '$lib'; enumerate_files v '*.ipe' '$fixture_dir/enum-empty'" >/dev/null 2>&1 || rc=$?
check "enumerate_files: an empty set exits 2" "$rc" 2

rc=0
bash -c "source '$lib'; enumerate_files v '*.ipe' '$fixture_dir/enum' '$fixture_dir/nope'" >/dev/null 2>&1 || rc=$?
check "enumerate_files: any missing root exits 2" "$rc" 2

# ── stub producers: a PATH dir whose tool exits with a chosen error code ─────
real_git="$(command -v git)"
stub_bin="$fixture_dir/stub-bin"
mkdir -p "$stub_bin"
# git that fails `ls-files` (the tracked-set producer) and passes through the rest.
cat > "$stub_bin/git" <<EOF
#!/usr/bin/env bash
for a in "\$@"; do [ "\$a" = ls-files ] && exit 128; done
exec "$real_git" "\$@"
EOF
chmod +x "$stub_bin/git"
rg_stub_home="$fixture_dir/stub-home"
mkdir -p "$rg_stub_home/.cargo/bin"
printf '#!/usr/bin/env bash\nexit 2\n' > "$rg_stub_home/.cargo/bin/rg"
chmod +x "$rg_stub_home/.cargo/bin/rg"

new_repo() {
    local r="$1"
    mkdir -p "$r"
    "$real_git" -C "$r" init -q
    printf 'ok\n' > "$r/README"
    "$real_git" -C "$r" add README
}

# ── artifact-guard: producer failure, forbidden artifact, clean tree ─────────
guard="$repo_root/.github/ci/artifact-guard.sh"
new_repo "$fixture_dir/repo-clean"
rc=0
(cd "$fixture_dir/repo-clean" && bash "$guard") >/dev/null 2>&1 || rc=$?
check "artifact-guard: a clean tracked set passes" "$rc" 0

rc=0
out="$(cd "$fixture_dir/repo-clean" && PATH="$stub_bin:$PATH" bash "$guard" 2>&1)" || rc=$?
check "artifact-guard: git ls-files exiting 128 fails closed (exit 2), never clean" "$rc" 2
check "artifact-guard: the git failure is the reported cause" \
    "$(cause_of "$out" "producer exited 128")" named

new_repo "$fixture_dir/repo-target"
mkdir -p "$fixture_dir/repo-target/x/target"
printf 'blob\n' > "$fixture_dir/repo-target/x/target/y"
"$real_git" -C "$fixture_dir/repo-target" add x/target/y
rc=0
(cd "$fixture_dir/repo-target" && bash "$guard") >/dev/null 2>&1 || rc=$?
check "artifact-guard: a tracked x/target/y fails (exit 1)" "$rc" 1

new_repo "$fixture_dir/repo-ext"
printf 'blob\n' > "$fixture_dir/repo-ext/lib.rlib"
"$real_git" -C "$fixture_dir/repo-ext" add lib.rlib
rc=0
(cd "$fixture_dir/repo-ext" && bash "$guard") >/dev/null 2>&1 || rc=$?
check "artifact-guard: a tracked .rlib fails (exit 1)" "$rc" 1

# ── examples.sh: an rg error while classifying is a hard failure ─────────────
ex_lib="$repo_root/tools/scripts/lib/examples.sh"
ffi_ex="$fixture_dir/ffi-example"
mkdir -p "$ffi_ex/src"
printf 'module Main exposing (main)\n' > "$ffi_ex/src/Main.ipe"
printf 'Package.rustDependencies []\n' > "$ffi_ex/package.ipe"
rc=0
bash -c "source '$ex_lib'; needs_ffi_install '$ffi_ex'" >/dev/null 2>&1 || rc=$?
check "needs_ffi_install: detects a rust-dependencies manifest (exit 0)" "$rc" 0
rc=0
out="$(PATH="$rg_stub_home/.cargo/bin:$PATH" bash -c "source '$ex_lib'; needs_ffi_install '$ffi_ex'" 2>&1)" || rc=$?
check "needs_ffi_install: rg exiting 2 hard-exits 2, never reads as no-FFI" "$rc" 2
check "needs_ffi_install: the rg error is the reported cause" \
    "$(cause_of "$out" "command exited 2")" named

# ── first-party floor: producer error and empty set both exit 2 ──────────────
floor="$repo_root/tools/scripts/first-party-check-floor.sh"
floor_repo="$fixture_dir/floor-repo"
mkdir -p "$floor_repo/tools/scripts"
: > "$floor_repo/tools/scripts/first-party-check-floor.sh"
run_floor() { # $1 = HOME for the run (its .cargo/bin leads env.sh's PATH)
    IPE_REPO="$floor_repo" IPE_BIN="$(type -P true)" HOME="$1" \
        CARGO_TARGET_DIR="$fixture_dir/floor-target" IPE_NO_SCCACHE=1 \
        bash "$floor" 2>&1
}
out="$(run_floor "$fixture_dir/plain-home")"; rc=$?
check "first-party floor: an empty example set exits 2" "$rc" 2
check "first-party floor: the empty set is the reported cause" \
    "$(cause_of "$out" "enumerated zero examples")" named

mkdir -p "$floor_repo/examples/shapes/cli/demo/src"
printf 'module Main exposing (main)\n' > "$floor_repo/examples/shapes/cli/demo/src/Main.ipe"
printf 'Package.name "demo"\n' > "$floor_repo/examples/shapes/cli/demo/package.ipe"
out="$(run_floor "$fixture_dir/plain-home")"; rc=$?
check "first-party floor: a one-example set whose check passes exits 0" "$rc" 0
out="$(run_floor "$rg_stub_home")"; rc=$?
check "first-party floor: the set producer failing (rg exit 2) exits 2" "$rc" 2
check "first-party floor: the producer failure is the reported cause" \
    "$(cause_of "$out" "failed to enumerate")" named

# ── tree-sitter parity: a missing or empty scan root exits 2 before any parse ─
parity="$repo_root/editors/tree-sitter-ipe/scripts/parity-check.sh"
out="$(PARITY_SCAN_ROOTS="$fixture_dir/enum:$fixture_dir/nope" bash "$parity" 2>&1)"; rc=$?
check "parity-check: a missing scan root exits 2" "$rc" 2
check "parity-check: the missing root is the reported cause" \
    "$(cause_of "$out" "missing scan root")" named
out="$(PARITY_SCAN_ROOTS="$fixture_dir/enum:$fixture_dir/enum-empty" bash "$parity" 2>&1)"; rc=$?
check "parity-check: an empty scan root exits 2" "$rc" 2
check "parity-check: the empty root is the reported cause" \
    "$(cause_of "$out" "nothing to scan")" named

# ── structural: no fail-closed helper call site in the tree passes a shell
# function as its command (the runtime refusal's static twin) ─────────────
sh_files=()
(cd "$repo_root" && git ls-files -z -- '*.sh' > "$fixture_dir/sh-files") \
    || { echo "FAIL: git ls-files over *.sh failed" >&2; fail=1; }
mapfile -d '' -t sh_files < "$fixture_dir/sh-files"
# shellcheck disable=SC2016  # perl source, expanded by perl, not the shell
scan_pl='
    my (%defined, @calls);
    for my $f (@ARGV) {
        next if $f eq "tools/scripts/tests/test-require-tool.sh";
        open my $fh, "<", $f or die "open $f: $!";
        local $/; my $src = <$fh>; close $fh;
        $src =~ s/\\\n/ /g;
        for my $line (split /\n/, $src) {
            next if $line =~ /^\s*#/;
            $defined{$1} = 1 if $line =~ /^\s*(?:function\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(\)\s*\{/;
            while ($line =~ /\b(match_or_fail|match_capture|capture_nul)\b[^#]*?\s--\s+([^\s;|&)]+)/g) {
                push @calls, [$f, $1, $2];
            }
        }
    }
    for my $c (@calls) {
        print "$c->[0]: $c->[1] -- $c->[2]\n" if $defined{$c->[2]};
    }
'
offenders="$(cd "$repo_root" && perl -e "$scan_pl" "${sh_files[@]}")" \
    || { echo "FAIL: structural call-site scan errored" >&2; fail=1; }
check "no match_or_fail/match_capture/capture_nul call site passes a shell function" "$offenders" ""
# The scan must fire on the shape it exists to forbid.
bad_sh="$fixture_dir/bad-gate.sh"
cat > "$bad_sh" <<'EOF'
_git_scan() { git ls-files | rg -e "$1"; }
match_capture hits "target scan" -- \
  _git_scan '(^|/)target/'
EOF
got="$(perl -e "$scan_pl" "$bad_sh")" || got="scan errored"
check "structural scan flags a function-wrapped pipeline call site" \
    "$(cause_of "$got" "match_capture -- _git_scan")" named

if [ "$fail" -ne 0 ]; then
    echo "test-require-tool: FAILED" >&2
    exit 1
fi
echo "test-require-tool: all cases pass"
