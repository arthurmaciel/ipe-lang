# shellcheck shell=bash
# scripts/lib/sky_mirror.sh — mirror upstream Sky examples into examples/sky/<name>/
# and apply the syntactic Sky->Ipe patch. SOURCE this (never execute it).
#
# The mirror is materialised at sweep time, NOT vendored: examples/sky/<name>/
# trees are regenerated each run (git-ignored) so they never drift from upstream.
# Only the rename map + README + manifest are checked in.
#
# Provides (FUNCTIONS):
#   sky_upstream_dir              -> path to the local ../sky/examples source, or empty.
#   sky_mirror_one <name> <dst>   -> mirror+patch one upstream example into <dst>. 0=ok.
#   mirror_sky_examples           -> mirror the whole upstream set into examples/sky/.
#   sky_example_names             -> one upstream example basename per line.

# Resolve the transform + rename map relative to REPO (set by env.sh).
: "${REPO:?sky_mirror.sh: REPO must be set (source lib/env.sh first)}"
SKY_TRANSFORM="${SKY_TRANSFORM:-$REPO/scripts/equivalence-checks/sky-to-ipe-transform.py}"
SKY_RENAME_MAP="${SKY_RENAME_MAP:-$REPO/examples/sky/rename-map.tsv}"

# ── sky_upstream_dir: the local sibling Sky checkout's examples/, if present ──
# Resolution order: $SKY_EXAMPLES_DIR override -> a `sky` checkout beside the
# MAIN repo root -> a few common sibling layouts. Empty output => not found
# locally (caller falls back to the network fetch).
#
# WORKTREE-AWARE: when the sweep runs inside a git worktree, $REPO is the
# worktree path (`.../.claude/worktrees/agent-*`), so `$REPO/../sky` is wrong.
# The upstream `sky` checkout sits beside the MAIN working tree, which is the
# parent of the shared `--git-common-dir` (`<main>/.git`). Derive that first.
sky_upstream_dir() {
  if [ -n "${SKY_EXAMPLES_DIR:-}" ] && [ -d "$SKY_EXAMPLES_DIR" ]; then
    printf '%s\n' "$SKY_EXAMPLES_DIR"; return 0
  fi
  local main_root="" common
  common="$(git -C "$REPO" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)"
  [ -n "$common" ] && main_root="$(dirname "$common")"
  local cand
  for cand in \
      ${main_root:+"$main_root/../sky/examples"} \
      "$REPO/../sky/examples" \
      "$REPO/../../sky/examples" \
      "$HOME/sky/examples"; do
    [ -d "$cand" ] && { printf '%s\n' "$(cd "$cand" && pwd -P)"; return 0; }
  done
  return 1
}

# ── sky_example_names: the upstream example basenames (numbered dirs) ─────────
# Only the NN-name example dirs — skips the upstream `rust/`, `simple/`,
# `test_pkg/` helper trees (not part of the canonical Sky example set).
sky_example_names() {
  local src; src="$(sky_upstream_dir)" || return 1
  local d
  for d in "$src"/[0-9]*/; do
    [ -d "$d" ] || continue
    [ -f "${d}src/Main.sky" ] || [ -f "${d}src/Main.ipe" ] || continue
    basename "$d"
  done
}

# ── _fetch_sky_example_network <name> <dst>: anzellai/sky fallback fetch ──────
# Uses the GitHub tarball API for the example subtree. Best-effort: returns
# non-zero (and prints a diagnostic) when offline / gh unavailable, so the
# caller reports the example as skipped-no-source rather than a false red.
_fetch_sky_example_network() {
  local name="$1" dst="$2" ref="${SKY_UPSTREAM_REF:-main}"
  command -v curl >/dev/null 2>&1 || return 1
  local url="https://codeload.github.com/anzellai/sky/tar.gz/refs/heads/${ref}"
  local tmp; tmp="$(mktemp -d)" || return 1
  if curl -fsSL --max-time 120 "$url" -o "$tmp/sky.tgz" 2>/dev/null &&
     tar -xzf "$tmp/sky.tgz" -C "$tmp" "sky-${ref}/examples/${name}" 2>/dev/null; then
    mkdir -p "$dst"
    cp -rf "$tmp/sky-${ref}/examples/${name}/." "$dst/"
    rm -rf "$tmp"
    return 0
  fi
  rm -rf "$tmp"
  return 1
}

# ── sky_mirror_one <name> <dst>: mirror + patch one example ──────────────────
# Copies the upstream tree (local sibling preferred, network fallback), renames
# every *.sky -> *.ipe, rewrites the sky.toml `entry` key, and applies the
# rename-map transform to every .ipe file. Returns 0 on success.
sky_mirror_one() {
  local name="$1" dst="$2" src
  rm -rf "$dst"
  if src="$(sky_upstream_dir)" && [ -d "$src/$name" ]; then
    mkdir -p "$dst"
    cp -rf "$src/$name/." "$dst/"
  elif ! _fetch_sky_example_network "$name" "$dst"; then
    echo "sky_mirror: no source for '$name' (no ../sky sibling and network fetch failed)" >&2
    return 1
  fi
  # Drop any stale build/cache artefacts copied from the source tree.
  rm -rf "$dst/sky-out" "$dst/.ipeache" "$dst/.skydeps" "$dst/target" 2>/dev/null

  # Preserve the PRISTINE upstream Sky source (still `.sky`, still `Sky.Core.*`/
  # `Std.*`) under .sky-original/ BEFORE the rename+patch. The Go-oracle reference
  # build (Haskell `sky`) MUST compile the original Sky source — it does not know
  # the `Ipe.*` namespaces the patch introduces — so the equivalence path builds
  # the Go binary from here while the Rust build uses the patched tree.
  rm -rf "$dst/.sky-original"; mkdir -p "$dst/.sky-original"
  cp -rf "$dst/." "$dst/.sky-original/" 2>/dev/null
  rm -rf "$dst/.sky-original/.sky-original" 2>/dev/null

  # *.sky -> *.ipe (source extension rename). The `.sky-original/` pristine copy
  # is pruned from EVERY rename/transform walk below so it stays untouched Sky.
  local f
  while IFS= read -r f; do
    mv -f "$f" "${f%.sky}.ipe"
  done < <(find "$dst" -path "$dst/.sky-original" -prune -o -type f -name '*.sky' -print 2>/dev/null)

  # sky.toml `entry` key: src/Main.sky -> src/Main.ipe (only the entry line;
  # never a blind global — an entry value is the sole .sky path in the toml).
  if [ -f "$dst/sky.toml" ]; then
    sed -i.bak -E 's/^([[:space:]]*entry[[:space:]]*=[[:space:]]*")([^"]*)\.sky(")/\1\2.ipe\3/' "$dst/sky.toml"
    rm -f "$dst/sky.toml.bak"
  fi

  # Bare stdlib imports (`import System` -> `import Ipe.System`): the set of
  # top-level Ipê stdlib module names (Ipe/<X>.ipe) that this example bare-imports
  # AND does NOT shadow with a local <X>.ipe. A composite's local `Server`/`Head`/
  # `Auth` module keeps its bare import; a genuine stdlib `System`/`Io` gets the
  # `Ipe.` prefix. Local-first, mirroring Sky's own import resolution.
  local bare bareflag=(); bare="$(_sky_bare_stdlib_set "$dst")"
  [ -n "$bare" ] && bareflag=(--bare-stdlib "$bare")

  # Apply the syntactic rename map to every .ipe file (code-only; strings/
  # comments preserved by the transform), plus the bare-stdlib import prefixing.
  # The flag pair is passed as an ARRAY so `--bare-stdlib` and its value stay two
  # separate argv words (a `${x:+--flag "$x"}` expansion would collapse them into
  # one, mis-parsed as the rename-map path).
  find "$dst" -path "$dst/.sky-original" -prune -o -type f -name '*.ipe' -print0 2>/dev/null \
    | xargs -0 -r python3 "$SKY_TRANSFORM" "${bareflag[@]}" "$SKY_RENAME_MAP"
}

# ── _sky_bare_stdlib_set <exampledir> -> comma list of bare stdlib names ──────
# A name qualifies iff (a) `Ipe/<Name>.ipe` is a top-level stdlib module and
# (b) no `<Name>.ipe` exists anywhere under the example (a local module shadows
# the stdlib one and must keep its bare import).
_sky_bare_stdlib_set() {
  local dir="$1" stdlib="$REPO/src/stdlib/Ipe" name out=""
  [ -d "$stdlib" ] || return 0
  # Bare imports appearing in this example's sources.
  local bares; bares="$(rg --no-filename -o '^[[:space:]]*import[[:space:]]+([A-Z][A-Za-z0-9_]*)([[:space:]]|$)' -r '$1' \
    "$dir"/src/*.ipe "$dir"/src/**/*.ipe 2>/dev/null | sort -u)"
  while IFS= read -r name; do
    [ -z "$name" ] && continue
    [ -f "$stdlib/$name.ipe" ] || continue                     # not a top-level stdlib module
    # shadowed by a LOCAL module? (ignore the .sky-original pristine copy)
    find "$dir" -path "$dir/.sky-original" -prune -o -type f -name "$name.ipe" -print 2>/dev/null | rg -q . && continue
    out="${out:+$out,}$name"
  done <<< "$bares"
  printf '%s' "$out"
}

# ── mirror_sky_examples: mirror the whole upstream set into examples/sky/ ─────
# Returns 0 if at least one example mirrored; prints the count. Non-fatal per
# example (a missing-source example is skipped, not a hard error).
mirror_sky_examples() {
  local names ok=0 fail=0 name dst
  names="$(sky_example_names)" || {
    echo "sky_mirror: no upstream example source located (set SKY_EXAMPLES_DIR or ensure ../sky exists)" >&2
    return 1
  }
  while IFS= read -r name; do
    [ -z "$name" ] && continue
    dst="$REPO/examples/sky/$name"
    if sky_mirror_one "$name" "$dst"; then ok=$((ok+1)); else fail=$((fail+1)); fi
  done <<< "$names"
  echo "sky_mirror: mirrored $ok example(s)${fail:+, $fail without source}"
  [ "$ok" -gt 0 ]
}
