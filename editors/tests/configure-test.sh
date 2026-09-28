#!/usr/bin/env bash
# Tests for editors/*/configure.sh and editors/lib/ipe-editors.sh.
#
# Runs every script against a throwaway HOME with stub editors on PATH, so it
# never touches the real configuration. Covers the refusals (too-old editors,
# Doom without a private config, a config Helix rejects) as well as the
# happy-path guarantees (backups, managed-block idempotency, untouched user
# content), plus two drift checks: the Zed grammar pin and ipe-mode's keywords.
#
# Usage: bash editors/tests/configure-test.sh   (from anywhere; needs cc, git)
#
# The Zed wasm32-wasip2 build check additionally needs rustup + cargo with the
# wasm32-wasip2 target installed; it skips with a message when that target is
# absent, except under CI (CI=true), where it is mandatory.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ipe-editors-test.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); printf 'ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL %s\n' "$1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

# A fresh sandbox: HOME + XDG dirs + a stub bin dir first on PATH.
sandbox() {
    SB="$WORK/$1"
    rm -rf "$SB"
    mkdir -p "$SB/home" "$SB/bin"
    export HOME="$SB/home" XDG_CONFIG_HOME="$SB/home/.config" XDG_DATA_HOME="$SB/home/.local/share"
    unset EMACSDIR DOOMDIR
    PATH="$SB/bin:$ORIG_PATH"
    stub ipe 'exit 0'
}
stub() { printf '#!/bin/sh\n%s\n' "$2" > "$SB/bin/$1"; chmod +x "$SB/bin/$1"; }
ORIG_PATH="$PATH"
ORIG_HOME="$HOME"

# --- lib: backups and managed blocks -----------------------------------------------
sandbox lib
(
    set -e
    IPE_SRC_ROOT="$ROOT"
    # shellcheck source=../lib/ipe-editors.sh
    . "$ROOT/editors/lib/ipe-editors.sh"
    f="$HOME/conf"
    printf 'user line 1\nuser line 2\n' > "$f"
    printf 'managed A\n' > "$WORK/a"
    printf 'managed B\n' > "$WORK/b"
    ipe_write_block "$f" "#" "$WORK/a" > /dev/null
    ipe_write_block "$f" "#" "$WORK/a" > /dev/null
    ipe_write_block "$f" "#" "$WORK/b" > /dev/null
    b1="$(ipe_backup "$f")"
    b2="$(ipe_backup "$f")"
    printf '%s\n%s\n' "$b1" "$b2" > "$WORK/backups"
) || bad "lib helpers ran"
f="$HOME/conf"
check "managed block written once, user lines kept" \
    '[ "$(grep -c "^managed " "$f")" = 1 ] && grep -q "^user line 1$" "$f" && grep -q "^user line 2$" "$f"'
check "managed block replaced in place" 'grep -q "^managed B$" "$f" && ! grep -q "^managed A$" "$f"'
check "exactly one begin and one end marker" \
    '[ "$(grep -c ">>> ipe (managed by" "$f")" = 1 ] && [ "$(grep -c "<<< ipe <<<" "$f")" = 1 ]'
check "unchanged rewrite makes no backup (2 changes -> 2 backups + 2 explicit)" \
    '[ "$(ls "$HOME" | grep -c "conf.ipe-backup-")" = 4 ]'
check "same-second backups get distinct names" \
    '[ "$(sed -n 1p "$WORK/backups")" != "$(sed -n 2p "$WORK/backups")" ]'

# --- Helix ----------------------------------------------------------------------------
sandbox helix-old
stub hx 'echo "helix 23.10 (abc)"'
if sh "$ROOT/editors/helix/configure.sh" > "$SB/out" 2>&1; then bad "Helix: refuses 23.10"; else ok "Helix: refuses 23.10"; fi
check "Helix: refusal writes nothing" '[ ! -e "$XDG_CONFIG_HOME/helix" ]'

sandbox helix-health-fails
stub hx 'case "$1" in --version) echo "helix 24.7 (abc)";; --health) echo "Error parsing user language config";; esac'
mkdir -p "$XDG_CONFIG_HOME/helix"
printf '[[language]]\nname = "rust"\n' > "$XDG_CONFIG_HOME/helix/languages.toml"
cp "$XDG_CONFIG_HOME/helix/languages.toml" "$SB/orig"
if sh "$ROOT/editors/helix/configure.sh" > "$SB/out" 2>&1; then
    bad "Helix: failed health check exits non-zero"
else
    ok "Helix: failed health check exits non-zero"
fi
check "Helix: failed health check restores languages.toml" 'cmp -s "$SB/orig" "$XDG_CONFIG_HOME/helix/languages.toml"'
check "Helix: failure never prints the success line" '! grep -q "setup complete" "$SB/out"'

sandbox helix-ok
stub hx 'case "$1" in --version) echo "helix 24.7 (abc)";; --health) printf "Configured language servers:\nHighlight queries: ✓\n";; esac'
mkdir -p "$XDG_CONFIG_HOME/helix"
printf '# mine\n[[language]]\nname = "rust"\n' > "$XDG_CONFIG_HOME/helix/languages.toml"
sh "$ROOT/editors/helix/configure.sh" > "$SB/out" 2>&1 && ok "Helix: happy path" || bad "Helix: happy path"
sh "$ROOT/editors/helix/configure.sh" > "$SB/out2" 2>&1 || true
L="$XDG_CONFIG_HOME/helix/languages.toml"
check "Helix: grammar + queries installed" \
    '[ -s "$XDG_CONFIG_HOME/helix/runtime/grammars/ipe.so" ] && [ -s "$XDG_CONFIG_HOME/helix/runtime/queries/ipe/highlights.scm" ]'
check "Helix: user content kept, one ipe-lsp table" \
    'grep -q "^# mine$" "$L" && [ "$(grep -c "^\[language-server.ipe-lsp\]" "$L")" = 1 ]'
check "Helix: re-run is a no-op" 'grep -q "already up to date" "$SB/out2" && [ "$(ls "$XDG_CONFIG_HOME/helix" | grep -c ipe-backup)" = 1 ]'

sandbox helix-handwritten
stub hx 'case "$1" in --version) echo "helix 24.7 (abc)";; --health) printf "Highlight queries: ✓\n";; esac'
mkdir -p "$XDG_CONFIG_HOME/helix"
printf '[language-server.ipe-lsp]\ncommand = "ipe"\n' > "$XDG_CONFIG_HOME/helix/languages.toml"
cp "$XDG_CONFIG_HOME/helix/languages.toml" "$SB/orig"
sh "$ROOT/editors/helix/configure.sh" > "$SB/out" 2>&1 || true
check "Helix: hand-written Ipê definition left untouched" 'cmp -s "$SB/orig" "$XDG_CONFIG_HOME/helix/languages.toml"'

# --- Neovim ---------------------------------------------------------------------------
sandbox nvim-old
stub nvim 'echo "NVIM v0.10.4"'
if sh "$ROOT/editors/neovim/configure.sh" > "$SB/out" 2>&1; then bad "Neovim: refuses 0.10"; else ok "Neovim: refuses 0.10"; fi
check "Neovim: refusal writes nothing" '[ ! -e "$XDG_DATA_HOME/nvim" ]'

sandbox nvim-check-fails
stub nvim 'case "$1" in --version) echo "NVIM v0.11.4";; *) echo "parser not loadable" >&2; exit 1;; esac'
if sh "$ROOT/editors/neovim/configure.sh" > "$SB/out" 2>&1; then
    bad "Neovim: failed headless check exits non-zero"
else
    ok "Neovim: failed headless check exits non-zero"
fi
check "Neovim: failure never prints the success line" '! grep -q "setup complete" "$SB/out"'

# --- Emacs ----------------------------------------------------------------------------
sandbox emacs-old
stub emacs 'echo 28'
if sh "$ROOT/editors/emacs/configure.sh" > "$SB/out" 2>&1; then bad "Emacs: refuses 28"; else ok "Emacs: refuses 28"; fi

sandbox emacs-doom-unconfigured
stub emacs 'echo 30'
mkdir -p "$XDG_CONFIG_HOME/emacs/lisp"
: > "$XDG_CONFIG_HOME/emacs/.doom"
if sh "$ROOT/editors/emacs/configure.sh" > "$SB/out" 2>&1; then
    bad "Emacs: Doom without private config refused"
else
    ok "Emacs: Doom without private config refused"
fi
check "Emacs: Doom refusal writes nothing" '[ ! -e "$XDG_DATA_HOME/ipe" ] && [ ! -e "$XDG_CONFIG_HOME/emacs/init.el" ]'

if command -v emacs > /dev/null 2>&1 && [ "$(emacs -Q --batch --eval '(princ emacs-major-version)' 2>/dev/null || echo 0)" -ge 29 ]; then
    REAL_EMACS="$(command -v emacs)"
    sandbox emacs-doom
    ln -s "$REAL_EMACS" "$SB/bin/emacs"
    mkdir -p "$XDG_CONFIG_HOME/emacs/lisp" "$XDG_CONFIG_HOME/doom"
    : > "$XDG_CONFIG_HOME/emacs/.doom"
    printf ';; my doom config\n' > "$XDG_CONFIG_HOME/doom/config.el"
    sh "$ROOT/editors/emacs/configure.sh" > "$SB/out" 2>&1 && ok "Emacs: Doom happy path" || { bad "Emacs: Doom happy path"; cat "$SB/out"; }
    check "Emacs: Doom config.el gets the block, keeps user lines" \
        'grep -q "^;; my doom config$" "$XDG_CONFIG_HOME/doom/config.el" && grep -qF "(require '"'"'ipe-mode)" "$XDG_CONFIG_HOME/doom/config.el"'
else
    printf 'skip Emacs 29+ not available for the Doom happy path\n'
fi

# --- Zed ------------------------------------------------------------------------------
sandbox zed
stub zed 'exit 0'
stub rustup 'exit 0'
stub cargo 'exit 0'
mkdir -p "$XDG_CONFIG_HOME/zed"
printf '// my settings\n{ "languages": { "Ipê": {} }, }\n' > "$XDG_CONFIG_HOME/zed/settings.json"
cp "$XDG_CONFIG_HOME/zed/settings.json" "$SB/orig"
sh "$ROOT/editors/zed/configure.sh" > "$SB/out" 2>&1 && ok "Zed: happy path" || bad "Zed: happy path"
check "Zed: settings.json never edited" 'cmp -s "$SB/orig" "$XDG_CONFIG_HOME/zed/settings.json"'
check "Zed: legacy settings keys reported" 'grep -q "older setup" "$SB/out"'
check "Zed: extension assembled" \
    '[ -s "$XDG_DATA_HOME/ipe/zed-ipe/extension.toml" ] && [ -s "$XDG_DATA_HOME/ipe/zed-ipe/languages/ipe/highlights.scm" ] && [ -s "$XDG_DATA_HOME/ipe/zed-ipe/src/lib.rs" ]'

# The stubbed rustup/cargo above prove only that configure.sh assembles the
# right files. Zed itself then runs `cargo build --release --target
# wasm32-wasip2` inside that assembled directory with RUSTC_WRAPPER cleared
# (WASI builds hang under sccache) — the step nothing here proved before,
# letting a Cargo.lock/Cargo.toml/target-rustflags regression ship unbuilt.
# Mandatory in CI (GitHub Actions sets CI=true, and the workflow installs the
# target); skips with a message when wasm32-wasip2 is not installed locally.
ZED_ASSEMBLED="$XDG_DATA_HOME/ipe/zed-ipe"
PATH="$ORIG_PATH"
# The sandboxed HOME hides the real toolchain; point rustup and cargo back at it.
export RUSTUP_HOME="${RUSTUP_HOME:-$ORIG_HOME/.rustup}" CARGO_HOME="${CARGO_HOME:-$ORIG_HOME/.cargo}"
if command -v rustup > /dev/null 2>&1 && command -v cargo > /dev/null 2>&1 \
    && (cd "$ZED_ASSEMBLED" && rustup target list --installed 2> /dev/null | grep -qx wasm32-wasip2); then
    WASM_TARGET_DIR="$WORK/zed-wasm-build"
    if (cd "$ZED_ASSEMBLED" && RUSTC_WRAPPER= cargo build --release --target wasm32-wasip2 \
        --target-dir "$WASM_TARGET_DIR") > "$WORK/zed-wasm-build.log" 2>&1
    then
        ok "Zed: extension compiles for wasm32-wasip2"
    else
        bad "Zed: extension compiles for wasm32-wasip2"
        cat "$WORK/zed-wasm-build.log"
    fi
    check "Zed: wasm artifact produced" '[ -s "$WASM_TARGET_DIR/wasm32-wasip2/release/zed_ipe.wasm" ]'
elif [ "${CI:-}" = "true" ]; then
    bad "Zed: wasm32-wasip2 target missing — the CI workflow must install it"
else
    printf 'skip Zed wasm32-wasip2 build (target not installed; run: rustup target add wasm32-wasip2)\n'
fi

sandbox zed-missing
if sh "$ROOT/editors/zed/configure.sh" > "$SB/out" 2>&1; then bad "Zed: refuses without zed"; else ok "Zed: refuses without zed"; fi

# --- drift checks ---------------------------------------------------------------------------
PATH="$ORIG_PATH"
rev="$(sed -n 's/^rev = "\([0-9a-f]\{40\}\)"$/\1/p' "$ROOT/editors/zed-ipe/extension.toml")"
if [ -z "$rev" ]; then
    bad "Zed: extension.toml pins the grammar to a commit"
elif ! git -C "$ROOT" cat-file -e "$rev^{commit}" 2> /dev/null; then
    bad "Zed: pinned grammar commit $rev is not in this clone (fetch it or fix the pin)"
else
    check "Zed: pinned grammar commit matches this checkout's grammar" \
        'git -C "$ROOT" diff --quiet "$rev" HEAD -- editors/tree-sitter-ipe/src editors/tree-sitter-ipe/queries/highlights.scm'
fi

hl_keywords="$(awk '/^\[/{k=1; buf=""; next} k && /^\] @keyword/{print buf; k=0; next} k{buf=buf" "$0}' \
    "$ROOT/editors/tree-sitter-ipe/queries/highlights.scm" | tr -s ' "' '\n' | grep -E '^[a-z]+$' | sort -u)"
el_keywords="$(sed -n '/(defconst ipe-keywords/,/)$/p' "$ROOT/editors/emacs/ipe-mode.el" \
    | grep -o '"[a-z]*"' | tr -d '"' | sort -u)"
check "ipe-mode keywords match the grammar's keyword captures" '[ -n "$hl_keywords" ] && [ "$hl_keywords" = "$el_keywords" ]'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" = 0 ]
