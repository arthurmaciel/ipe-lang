# shellcheck shell=bash
# scripts/lib/examples.sh — SINGLE SOURCE OF TRUTH for the example manifest.
# SOURCE this (never execute it).
#
# PORTED from ../sky/runtime-rust/scripts/lib/examples.sh. The Go-FFI EXCLUSION
# LOGIC (is_out_of_scope / build_set) is preserved VERBATIM — it is the authority
# on which examples belong to the Rust backend. Only two paths are adapted for
# this repo: the sky-stdlib index scan (this repo's stdlib lives under
# src/stdlib, not sky-stdlib/) and the equivalence-classification overrides
# path (scripts/, not runtime-rust/scripts/).
#
# DERIVED, NOT HARDCODED. Every set is computed at call time from the example
# dirs on disk + their Ipê source. The ONLY thing that excludes an example is
# Go-FFI, because the Rust backend does not bind Go packages. A greenfield
# example that has never built on Rust SURFACES as a real failure rather than
# being silently filtered out.
#
# THE GO-FFI SIGNAL IS THE IMPORT, NOT `[go.dependencies]`. The real Go-FFI tell
# is a Ipê `import` of a Go-PACKAGE module — one that resolves to neither a Ipê
# stdlib module nor a local project `.ipe` file (`Github.Com.…`, `Net.Http`,
# `Fyne.…`).
#
# Provides (all FUNCTIONS — call them, don't read arrays):
#   all_examples            → every candidate example dir, one per line (no trailing /).
#   is_out_of_scope <dir>   → exit 0 IFF Go-FFI (imports an unresolvable Go-pkg module).
#   is_web_example  <dir>   → exit 0 IFF Ipe.Live / Ipe.Http.Server (browser-drivable).
#   example_shape   <dir>   → tui|webview|fyne|server|live|cli
#   build_set               → all_examples − Go-FFI (the BUILD sweep set).
#   run_set / perf_set      → == build_set.
#   equivalence_mode <dir>        → none|stdout|body|scenario|pty (DERIVED, overrides on top).

# ── all_examples: every candidate dir on disk, trailing slash stripped ───────
all_examples() {
  local d
  for d in examples/[0-9]*/ examples/simple/ examples/test_pkg/ examples/rust/*/; do
    [ -d "$d" ] || continue
    d="${d%/}"
    [ -f "$d/src/Main.ipe" ] || continue
    printf '%s\n' "$d"
  done
}

# ── _build_stdlib_index: ONE-TIME in-memory index of stdlib module paths ──────
# ADAPTED: this repo's stdlib source lives under src/stdlib (and, once it
# lands, a top-level sky-stdlib/). IPE_STDLIB_DIRS is a space-separated list of
# roots to index; both are scanned so a bare/partial stdlib import (`import
# System` → Ipe.System) resolves regardless of which tree owns it. Every
# `/`-delimited suffix of each module path (minus `.ipe`) is recorded as an O(1)
# key. The BUILT flag is the idempotency guard across re-sources.
IPE_STDLIB_DIRS="${IPE_STDLIB_DIRS:-src/stdlib sky-stdlib}"
declare -gA _SKY_STDLIB_INDEX
_build_stdlib_index() {
  [ -n "${_SKY_STDLIB_INDEX_BUILT:-}" ] && return 0
  local f rest root
  for root in $IPE_STDLIB_DIRS; do
    [ -d "$root" ] || continue
    while IFS= read -r f; do
      # index keys are relative to the stdlib ROOT (so Ipê/Core/String.ipe →
      # Ipê/Core/String, Core/String, String), mirroring the source layout.
      rest="${f#"$root"/}"; rest="${rest%.ipe}"
      while :; do
        _SKY_STDLIB_INDEX["$rest"]=1
        case "$rest" in
          */*) rest="${rest#*/}" ;;
          *)   break ;;
        esac
      done
    done < <(find "$root" -type f -name '*.ipe' 2>/dev/null)
  done
  _SKY_STDLIB_INDEX_BUILT=1
}

# ── is_out_of_scope <dir>: the ONLY exclusion is Go-FFI (IMPORT signal) ──────
# Return 0 (exclude) IFF the example imports a Go-PACKAGE module: a Ipê `import`
# whose module name resolves to NEITHER a Ipê stdlib module NOR a local project
# `.ipe` file. The recursive `.ipe` walk is load-bearing: some examples hide
# their `Github.Com.…`/`Net.Http`/`Fyne.…` imports inside Lib.* submodules.
#   • prefix Ipê. / Ipe.  → Ipê stdlib          → IN scope
#   • prefix Rust.        → Rust-FFI wrapper crate → IN scope
#   • dotted name suffix-matches the stdlib index → IN scope
#   • dotted name resolves to a `.ipe` under the project → IN (local mod)
#   • otherwise (`Github.Com.…`, `Net.Http`, `Fyne.…`) → Go-FFI → OUT
is_out_of_scope() {
  local dir="$1" m rel localpaths localdone=""
  # Explicit out-of-scope: skyshop-rs is the heavyweight real-world FFI proof
  # (firestore + async-stripe via wrapper crates). Verified separately, not in
  # the per-commit gate. (Not vendored into this repo; the case-guard is kept for
  # parity with upstream so a future re-sync stays consistent.)
  case "$dir" in */skyshop-rs) return 0 ;; esac
  _build_stdlib_index
  while read -r m; do
    [ -z "$m" ] && continue
    case "$m" in Ipê.*|Ipe.*|Rust.*) continue ;; esac # Ipê stdlib / Rust-FFI wrapper → in scope
    rel="${m//.//}"
    [ -n "${_SKY_STDLIB_INDEX[$rel]:-}" ] && continue
    if [ -z "$localdone" ]; then
      localpaths=$'\n'"$(find "$dir" -type f -name '*.ipe' 2>/dev/null)"$'\n'
      localdone=1
    fi
    case "$localpaths" in *"/${rel}.ipe"$'\n'*) continue ;; esac
    return 0                                          # unresolvable → Go-package → OUT
  done < <(find "$dir/src" -type f -name '*.ipe' -exec \
             rg --no-filename -No '^[[:space:]]*import[[:space:]]+([A-Za-z0-9_.]+)' -r '$1' {} + 2>/dev/null)
  return 1                                            # every import resolved → in scope
}

# ── is_live_network_cli <name>: a cli whose RUN makes a LIVE EXTERNAL call ───
# A cli that issues a real HTTP request to a third-party host has a
# non-deterministic, network-dependent RUN that can HANG on a CI runner with
# flaky egress. That is a host/network artifact, NOT a Rust defect, so its
# RUN-hang degrades to SKIP. EXPLICIT, documented set — never a heuristic.
is_live_network_cli() {
  case "$1" in
    02-go-stdlib) return 0 ;;
  esac
  return 1
}

# ── is_web_example <dir>: Ipe.Live OR Ipe.Http.Server (browser-drivable) ─────
is_web_example() {
  _shape_match "$1/src" 'Ipe\.Live|Live\.app|Server\.listen|Ipe\.Http\.Server'
}

# ── example_shape <dir>: tui|webview|fyne|server|live|cli ────────────────────
# `_shape_match` strips Ipê line comments (`--…`) from every matching line before
# re-testing, so a doc comment naming a backend can't misclassify the example.
_shape_match() { # $1=src dir  $2=regex
  rg --no-filename -e "$2" "$1" 2>/dev/null | sed 's/--.*$//' | rg -q -e "$2" 2>/dev/null
}
example_shape() {
  local s="$1/src"
  if   _shape_match "$s" 'Ipe\.Tui|Tui\.app';               then echo tui
  elif _shape_match "$s" 'Ipe\.Webview|Webview\.app';        then echo webview
  elif _shape_match "$s" 'Fyne';                             then echo fyne
  elif _shape_match "$s" 'Ipe\.Live|Live\.app';              then echo live
  elif _shape_match "$s" 'Server\.listen|Ipe\.Http\.Server'; then echo server
  else echo cli; fi
}

# ── build_set: all_examples minus Go-FFI (unresolvable-import examples) ──────
build_set() {
  if [ -n "${_SKY_BUILD_SET+x}" ]; then printf '%s' "$_SKY_BUILD_SET"; return 0; fi
  local d out=""
  while IFS= read -r d; do
    is_out_of_scope "$d" && continue
    out+="$d"$'\n'
  done < <(all_examples)
  _SKY_BUILD_SET="$out"
  printf '%s' "$out"
}

run_set()  { build_set; }
perf_set() { build_set; }

# ── equivalence_mode <dir>: DERIVE the Go≡Rust equivalence mode from the shape ─────
#   Go-FFI / out-of-scope → none · cli → stdout · server → body ·
#   live → scenario · tui → pty · webview → none · fyne → none.
# An OVERRIDE from equivalence-classification.tsv (keyed by basename) wins if present.
equivalence_mode() {
  local dir="$1" base over
  base="$(basename "$dir")"
  if [ -f "$EQUIVALENCE_TSV" ]; then
    over="$(awk -v k="$base" '!/^#/ && $1==k {print $2; exit}' "$EQUIVALENCE_TSV" 2>/dev/null)"
    [ -n "$over" ] && { printf '%s\n' "$over"; return 0; }
  fi
  if is_out_of_scope "$dir"; then printf 'none\n'; return 0; fi
  case "$(example_shape "$dir")" in
    cli)     printf 'stdout\n'   ;;
    server)  printf 'body\n'     ;;
    live)    printf 'scenario\n' ;;
    tui)     printf 'pty\n'      ;;
    webview) printf 'none\n'     ;;
    fyne)    printf 'none\n'     ;;
    *)       printf 'none\n'     ;;
  esac
}

# equivalence_override_reason <dir> → the .tsv reason column for an overridden example.
equivalence_override_reason() {
  local base; base="$(basename "$1")"
  [ -f "$EQUIVALENCE_TSV" ] || return 0
  awk -v k="$base" '!/^#/ && $1==k {$1="";$2="";sub(/^[[:space:]]+/,"");print;exit}' "$EQUIVALENCE_TSV" 2>/dev/null
}

# The overrides file (overrides-on-top-of-derived), resolved relative to REPO.
# ADAPTED path: scripts/, not runtime-rust/scripts/.
EQUIVALENCE_TSV="${EQUIVALENCE_TSV:-$REPO/scripts/equivalence-checks/equivalence-classification.tsv}"
