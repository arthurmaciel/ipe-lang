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
trap 'chmod -R u+rwX "$fixture_dir" 2>/dev/null; rm -rf "$rg_free_path" "$fixture_dir"' EXIT

# The helpers run only external executables, so the cases name them by path.
ext_true="$(type -P true)"
ext_false="$(type -P false)"
ext_printf="$(type -P printf)"
exit2="$fixture_dir/exit2"
printf '#!/usr/bin/env bash\nexit 2\n' > "$exit2"
partial128="$fixture_dir/partial128"
printf '#!/usr/bin/env bash\nprintf "a\\0"\nexit 128\n' > "$partial128"
chmod +x "$exit2" "$partial128"

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
bash -c "source '$lib'; match_or_fail t -- '$ext_false'" >/dev/null 2>&1
rc=$?
check "match_or_fail: exit 1 (no match) returns 1, doesn't hard-fail" "$rc" 1

rc=0
bash -c "source '$lib'; match_or_fail t -- '$ext_true'" >/dev/null 2>&1
rc=$?
check "match_or_fail: exit 0 (match) returns 0" "$rc" 0

rc=0
bash -c "source '$lib'; match_or_fail t -- '$exit2'" >/dev/null 2>&1
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
got="$(bash -c "source '$lib'; match_capture v t -- '$ext_printf' 'a\nb\n'; printf '%s' \"\$v\"")"
check "match_capture: match captures the command's stdout" "$got" "$(printf 'a\nb')"

rc=0
bash -c "source '$lib'; match_capture v t -- '$ext_false'" >/dev/null 2>&1
rc=$?
check "match_capture: exit 1 (no match) returns 1, doesn't hard-fail" "$rc" 1

rc=0
bash -c "source '$lib'; match_capture v t -- '$ext_true'" >/dev/null 2>&1
rc=$?
check "match_capture: exit 0 (match) returns 0" "$rc" 0

rc=0
bash -c "source '$lib'; match_capture v t -- '$exit2'" >/dev/null 2>&1
rc=$?
check "match_capture: exit 2 hard-exits 2" "$rc" 2

# ── capture_nul / enumerate_files: producer failure or empty set exits 2 ────
rc=0
bash -c "source '$lib'; capture_nul v t -- '$partial128'" >/dev/null 2>&1 || rc=$?
check "capture_nul: a producer exiting 128 after partial output hard-exits 2" "$rc" 2

got="$(bash -c "source '$lib'; capture_nul v t -- '$ext_printf' 'a b\\0c\\0'; printf '%s|' \"\${v[@]}\"")"
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

# ── output-name collisions: a caller name equal to a former helper local
# still receives the value (every helper local is `__`-prefixed) ────────────
for name in rc d desc var tmp out; do
    got="$(bash -c "source '$lib'; capture_nul $name d -- '$ext_printf' 'a\\0'; printf '%s' \"\${#${name}[@]}\"" 2>&1)"
    check "capture_nul: output named '$name' receives the set" "$got" 1
    got="$(bash -c "source '$lib'; match_capture $name d -- '$ext_printf' hit; printf '%s' \"\$$name\"" 2>&1)"
    check "match_capture: output named '$name' receives the text" "$got" hit
done
for name in glob root found sorted tmp out var r; do
    got="$(bash -c "source '$lib'; enumerate_files $name '*.ipe' '$fixture_dir/enum'; printf '%s' \"\${#${name}[@]}\"" 2>&1)"
    check "enumerate_files: output named '$name' receives the set" "$got" 2
done

# ── output-name refusals: the reserved `__` prefix and a non-identifier are
# refused (exit 2) by every helper that writes a caller variable ───────────
reserved_names="__cn_var __cn_desc __cn_tmp __cn_rc __mc_var __mc_desc __mc_out __mc_rc
    __ef_var __ef_glob __ef_root __ef_found __ef_tmp __ef_sorted __ef_out __rsr_files
    __dc_kind __mf_desc __mf_rc __x"
for name in $reserved_names; do
    for call in "capture_nul $name t -- '$ext_printf' 'a\\0'" \
                "match_capture $name t -- '$ext_printf' hit" \
                "enumerate_files $name '*.ipe' '$fixture_dir/enum'"; do
        rc=0
        out="$(bash -c "source '$lib'; $call" 2>&1)" || rc=$?
        check "${call%% *}: reserved output '$name' exits 2" "$rc" 2
        check "${call%% *}: reserved output '$name' is the reported cause" \
            "$(cause_of "$out" "output variable '$name' uses the reserved '__' prefix")" named
    done
done
for name in "''" 1x a-b 'a[0]' "'x y'"; do
    for helper in "capture_nul $name t -- '$ext_printf' 'a\\0'" \
                  "match_capture $name t -- '$ext_printf' hit" \
                  "enumerate_files $name '*.ipe' '$fixture_dir/enum'"; do
        rc=0
        out="$(bash -c "source '$lib'; $helper" 2>&1)" || rc=$?
        check "${helper%% *}: invalid output $name exits 2" "$rc" 2
        check "${helper%% *}: invalid output $name is the reported cause" \
            "$(cause_of "$out" "is not a valid shell identifier")" named
    done
done

# ── direct-command refusals: builtins (eval, source, ., command, builtin,
# exec, false), keywords, aliases, shells, and launchers can each run a
# masked pipeline, so every helper refuses them (exit 2) ──────────────────
for helper in "match_or_fail t" "match_capture v t" "capture_nul v t"; do
    for cmd in "eval 'false | rg foo'" "source /dev/null" ". /dev/null" \
               "command rg foo" "builtin echo" "exec rg foo" "false"; do
        rc=0
        out="$(bash -c "source '$lib'; $helper -- $cmd" 2>&1)" || rc=$?
        check "${helper%% *}: refuses builtin '${cmd%% *}' (exit 2)" "$rc" 2
        check "${helper%% *}: builtin '${cmd%% *}' is the reported cause" \
            "$(cause_of "$out" "'${cmd%% *}' is builtin")" named
    done
    rc=0
    out="$(bash -c "source '$lib'; $helper -- time rg foo" 2>&1)" || rc=$?
    check "${helper%% *}: refuses a keyword (exit 2)" "$rc" 2
    check "${helper%% *}: the keyword is the reported cause" \
        "$(cause_of "$out" "'time' is keyword")" named
    rc=0
    out="$(bash -c "source '$lib'; shopt -s expand_aliases; alias al='false | true'
$helper -- al" 2>&1)" || rc=$?
    check "${helper%% *}: refuses an alias (exit 2)" "$rc" 2
    check "${helper%% *}: the alias is the reported cause" \
        "$(cause_of "$out" "'al' is alias")" named
    rc=0
    out="$(bash -c "source '$lib'; wrapped() { false | true; }; $helper -- wrapped" 2>&1)" || rc=$?
    check "${helper%% *}: refuses a shell function (exit 2)" "$rc" 2
    check "${helper%% *}: the function is the reported cause" \
        "$(cause_of "$out" "'wrapped' is function")" named
    for cmd in "bash -c 'false | rg foo'" "sh -c 'false | rg foo'" \
               "$(type -P bash) -c 'false | rg foo'" "env rg foo" "xargs rg"; do
        rc=0
        out="$(bash -c "source '$lib'; $helper -- $cmd" 2>&1)" || rc=$?
        check "${helper%% *}: refuses launcher '${cmd%% *}' (exit 2)" "$rc" 2
        check "${helper%% *}: launcher '${cmd%% *}' is the reported cause" \
            "$(cause_of "$out" "runs another command line")" named
    done
done

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

# ── examples.sh: an unreadable source file fails the shape scan closed ─────
shape_ex="$fixture_dir/shape-example"
mkdir -p "$shape_ex/src"
printf 'module Main exposing (main)\nimport Ipe.Tea.Tui\n' > "$shape_ex/src/Main.ipe"
got="$(bash -c "source '$ex_lib'; example_shape '$shape_ex'" 2>&1)"
check "example_shape: a readable Tui source classifies as tui" "$got" tui
printf 'module Hidden exposing (x)\n' > "$shape_ex/src/Hidden.ipe"
chmod 000 "$shape_ex/src/Hidden.ipe"
rc=0
out="$(bash -c "source '$ex_lib'; example_shape '$shape_ex'" 2>&1)" || rc=$?
check "example_shape: an unreadable source file exits 2, never a partial scan" "$rc" 2
check "example_shape: the unreadable file is the reported cause" \
    "$(cause_of "$out" "cannot open $shape_ex/src/Hidden.ipe")" named
chmod 644 "$shape_ex/src/Hidden.ipe"

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
# function, an eval/source/exec-style builtin, or a `sh -c` line as its
# command (the runtime refusal's static twin) ─────────────────────────────
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
            while ($line =~ /\b(match_or_fail|match_capture|capture_nul)\b[^#]*?\s--\s+([^\s;|&)]+)(?:\s+(\S+))?/g) {
                push @calls, [$f, $1, $2, $3 // ""];
            }
        }
    }
    my %opaque = map { $_ => 1 } qw(eval source . command builtin exec env xargs);
    for my $c (@calls) {
        my ($f, $h, $cmd, $next) = @$c;
        (my $base = $cmd) =~ s{.*/}{};
        my $shell_c = $base =~ /^(?:ba|da|z|k|mk)?sh$/ && $next =~ /^-[A-Za-z]*c/;
        print "$f: $h -- $cmd\n" if $defined{$cmd} || $opaque{$cmd} || $shell_c;
    }
'
offenders="$(cd "$repo_root" && perl -e "$scan_pl" "${sh_files[@]}")" \
    || { echo "FAIL: structural call-site scan errored" >&2; fail=1; }
check "no match_or_fail/match_capture/capture_nul call site passes an opaque command" "$offenders" ""
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
for shape in "eval 'false | rg foo'" "bash -c 'false | rg foo'" "sh -c 'false | rg foo'" \
             "/bin/bash -ec 'false | rg foo'" "source ./gate.sh" ". ./gate.sh" \
             "command rg foo" "builtin echo" "exec rg foo"; do
    printf 'match_or_fail "t" -- %s\n' "$shape" > "$bad_sh"
    got="$(perl -e "$scan_pl" "$bad_sh")" || got="scan errored"
    check "structural scan flags a '${shape%% *}' call site" \
        "$(cause_of "$got" "match_or_fail -- ${shape%% *}")" named
done
printf 'match_or_fail "t" -- rg -q foo bar\n' > "$bad_sh"
got="$(perl -e "$scan_pl" "$bad_sh")" || got="scan errored"
check "structural scan passes a direct rg call site" "$got" ""

if [ "$fail" -ne 0 ]; then
    echo "test-require-tool: FAILED" >&2
    exit 1
fi
echo "test-require-tool: all cases pass"
