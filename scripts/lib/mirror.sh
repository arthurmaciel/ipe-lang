# shellcheck shell=bash
# scripts/lib/mirror.sh — materialise the upstream Sky examples as buildable Ipê
# examples under examples/sky/<name>/, TRANSIENTLY. SOURCE this (never execute).
#
# The mirror is regenerated on every sweep and git-ignored: examples/sky/<name>/
# never drifts from upstream because it is never committed. Only the control
# surface under examples/sky/ is tracked — manifest.toml, rename-map.tsv,
# ipe-patches/, README.md, BLOCKERS.md.
#
# Each example is transformed from Sky to Ipê in two ordered steps:
#   1. rename-map.tsv (via sky-to-ipe-transform.py) — the shared, drift-resistant
#      token rewrite: Sky.Core.* / Sky.Http.* / Sky.Ffi / Sky.Test / Std.* → Ipe.*
#      plus the .sky→.ipe source-extension rename and the sky.toml→ipe.toml
#      manifest rename (with its `entry` key).
#   2. ipe-patches/<name>.patch — an OPTIONAL per-example unified diff applied on
#      top, for a semantic delta the token rewrite cannot express. Absent for an
#      example whose transform is purely syntactic (the common case).
#
# Provides (FUNCTIONS):
#   sky_upstream_dir              -> path to the local ../sky/examples source, or empty.
#   sky_mirror_one <name> <dst>   -> mirror+transform+patch one example into <dst>. 0=ok.
#   mirror_sky_examples           -> mirror the whole in-scope set into examples/sky/.
#   sky_example_names             -> one in-scope example basename per line (from manifest).

: "${REPO:?mirror.sh: REPO must be set (source lib/env.sh first)}"
SKY_TRANSFORM="${SKY_TRANSFORM:-$REPO/scripts/lib/sky-to-ipe-transform.py}"
SKY_RENAME_MAP="${SKY_RENAME_MAP:-$REPO/examples/sky/rename-map.tsv}"
SKY_PATCH_DIR="${SKY_PATCH_DIR:-$REPO/examples/sky/ipe-patches}"

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

# ── sky_example_names: in-scope example basenames from the manifest ───────────
# Enumerates every [[example]] name in examples/sky/manifest.toml. The sweep's
# is_out_of_scope filter (lib/examples.sh) still excludes Go-FFI examples from
# the BUILD set; listing them here only tells the mirror they are known.
sky_example_names() {
  local manifest="$REPO/examples/sky/manifest.toml"
  if [ -f "$manifest" ]; then
    python3 -c "
import re, sys
with open('$manifest') as f:
    for line in f:
        m = re.match(r'''^\s*name\s*=\s*[\"']([^\"']+)[\"']\s*\$''', line)
        if m:
            print(m.group(1))
"
    return 0
  fi
  local src; src="$(sky_upstream_dir)" || return 1
  local d
  for d in "$src"/[0-9]*/; do
    [ -d "$d" ] || continue
    [ -f "${d}src/Main.sky" ] || [ -f "${d}src/Main.ipe" ] || continue
    basename "$d"
  done
}

# ── _fetch_sky_example_network <name> <dst>: anzellai/sky fallback fetch ──────
# Uses the GitHub tarball for the example subtree. Best-effort: returns non-zero
# (and prints a diagnostic) when offline / curl unavailable, so the caller
# reports the example as skipped-no-source rather than a false red.
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

# ── sky_mirror_one <name> <dst>: mirror + transform + patch one example ───────
# Copies the upstream tree (local sibling preferred, network fallback), renames
# every *.sky -> *.ipe, renames sky.toml -> ipe.toml (rewriting its `entry` key),
# applies the
# rename-map transform to every .ipe file, then applies the optional
# ipe-patches/<name>.patch semantic delta. Returns 0 on success, 2 when the
# per-example patch fails to apply (a real regression to surface, never ignore).
sky_mirror_one() {
  local name="$1" dst="$2" src
  rm -rf "$dst"
  # Network FIRST — the point of the mirror is to hold the CURRENT upstream, so
  # each refresh fetches from anzellai/sky. A local ../sky sibling is only the
  # offline fallback (network failure), so a stale local checkout never masks a
  # fresh upstream.
  if _fetch_sky_example_network "$name" "$dst"; then
    :
  elif src="$(sky_upstream_dir)" && [ -d "$src/$name" ]; then
    mkdir -p "$dst"
    cp -rf "$src/$name/." "$dst/"
  else
    echo "mirror: no source for '$name' (upstream fetch failed and no local ../sky fallback)" >&2
    return 1
  fi
  # Drop any stale build/cache artefacts copied from the source tree.
  rm -rf "$dst/sky-out" "$dst/out" "$dst/.ipe" "$dst/.ipecache" "$dst/.ipedeps" \
         "$dst/.skydeps" "$dst/.skyache" "$dst/target" 2>/dev/null

  # Preserve the RAW upstream .sky tree in a sibling `.sky-src/` before the
  # rewrites, so the live-Sky comparison (SKY_SWEEP_COMPARE) can `sky run` the
  # unmodified upstream. Skipped when comparison is off, to keep the mirror lean.
  # Copy each top-level entry individually — `cp "$dst/." "$dst/.sky-src/"` would
  # race on the just-created target dir and can copy nothing on some `cp`s.
  if [ "${SKY_SWEEP_COMPARE:-0}" = 1 ]; then
    local _e
    rm -rf "$dst/.sky-src"
    mkdir -p "$dst/.sky-src"
    for _e in "$dst"/* "$dst"/.[!.]*; do
      [ -e "$_e" ] || continue
      case "$(basename "$_e")" in .sky-src) continue ;; esac
      cp -rf "$_e" "$dst/.sky-src/" 2>/dev/null
    done
  fi

  # *.sky -> *.ipe (source extension rename). The `.sky-src/` raw-preservation
  # tree is pruned from every walk below so it keeps its original .sky files.
  local f
  while IFS= read -r f; do
    mv -f "$f" "${f%.sky}.ipe"
  done < <(find "$dst" -path "$dst/.sky-src" -prune -o -type f -name '*.sky' -print 2>/dev/null)

  # Project manifest: sky.toml -> ipe.toml (Ipê's canonical manifest name), with
  # the `entry` key's src/Main.sky -> src/Main.ipe rewrite. Renaming the file (not
  # just editing it) is load-bearing: every consumer of the mirrored tree
  # (ipe_build_target, is_wasm_example, resolve_bin) keys off `ipe.toml`, so the
  # mirrored tree must be indistinguishable from a native Ipê project.
  if [ -f "$dst/sky.toml" ]; then
    sed -i.bak -E 's/^([[:space:]]*entry[[:space:]]*=[[:space:]]*")([^"]*)\.sky(")/\1\2.ipe\3/' "$dst/sky.toml"
    rm -f "$dst/sky.toml.bak"
    mv -f "$dst/sky.toml" "$dst/ipe.toml"
  fi

  # Bare stdlib imports (`import System` -> `import Ipe.System`): the set of
  # top-level Ipê stdlib module names (Ipe/<X>.ipe) that this example bare-imports
  # AND does NOT shadow with a local <X>.ipe. Local-first, mirroring Sky's own
  # import resolution.
  local bare bareflag=(); bare="$(_sky_bare_stdlib_set "$dst")"
  [ -n "$bare" ] && bareflag=(--bare-stdlib "$bare")

  # Step 1 — the shared token rewrite (code-only; strings/comments preserved).
  find "$dst" -path "$dst/.sky-src" -prune -o -type f -name '*.ipe' -print0 2>/dev/null \
    | xargs -0 -r python3 "$SKY_TRANSFORM" "${bareflag[@]}" "$SKY_RENAME_MAP"

  # Step 2 — the optional per-example semantic-delta patch. A patch that fails
  # to apply is a real problem (upstream drifted, or the patch is stale): return
  # non-zero so the sweep surfaces it as a RED row rather than building a
  # half-patched tree.
  local patch="$SKY_PATCH_DIR/$name.patch"
  if [ -f "$patch" ] && [ -s "$patch" ]; then
    if ! ( cd "$dst" && patch -p1 --forward --silent <"$patch" ); then
      echo "mirror: ipe-patches/$name.patch failed to apply cleanly to '$name'" >&2
      return 2
    fi
  fi
  return 0
}

# ── _sky_bare_stdlib_set <exampledir> -> comma list of bare stdlib names ──────
# A name qualifies iff (a) `Ipe/<Name>.ipe` is a top-level stdlib module and
# (b) no `<Name>.ipe` exists anywhere under the example (a local module shadows
# the stdlib one and must keep its bare import).
_sky_bare_stdlib_set() {
  local dir="$1" stdlib="$REPO/src/stdlib/Ipe" name out=""
  [ -d "$stdlib" ] || return 0
  local bares; bares="$(rg --no-filename -o '^[[:space:]]*import[[:space:]]+([A-Z][A-Za-z0-9_]*)([[:space:]]|$)' -r '$1' \
    "$dir"/src/*.ipe "$dir"/src/**/*.ipe 2>/dev/null | sort -u)"
  while IFS= read -r name; do
    [ -z "$name" ] && continue
    [ -f "$stdlib/$name.ipe" ] || continue
    find "$dir" -type f -name "$name.ipe" -print 2>/dev/null | rg -q . && continue
    out="${out:+$out,}$name"
  done <<< "$bares"
  printf '%s' "$out"
}

# ── mirror_sky_examples: mirror the whole in-scope set into examples/sky/ ──────
# Returns 0 if at least one example mirrored; prints the count. Non-fatal per
# example (a missing-source example is skipped). A patch-apply failure (rc 2) is
# counted separately and printed, but does not abort the whole mirror.
mirror_sky_examples() {
  local names ok=0 fail=0 patchfail=0 name dst rc
  names="$(sky_example_names)" || {
    echo "mirror: no upstream example source located (set SKY_EXAMPLES_DIR or ensure ../sky exists)" >&2
    return 1
  }
  while IFS= read -r name; do
    [ -z "$name" ] && continue
    dst="$REPO/examples/sky/$name"
    sky_mirror_one "$name" "$dst"; rc=$?
    case "$rc" in
      0) ok=$((ok+1)) ;;
      2) patchfail=$((patchfail+1)) ;;
      *) fail=$((fail+1)) ;;
    esac
  done <<< "$names"
  echo "mirror: materialised $ok example(s)${fail:+, $fail without source}${patchfail:+, $patchfail with a failing patch}"
  [ "$ok" -gt 0 ]
}
