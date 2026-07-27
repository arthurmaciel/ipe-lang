# shellcheck shell=bash
# scripts/lib/mirror.sh — materialise the upstream Sky examples as buildable Ipê
# examples under examples/sky/<name>/, TRANSIENTLY. SOURCE this (never execute).
#
# The source of truth is the upstream GitHub repo (anzellai/sky): every example
# is fetched fresh from there on each sweep, so the mirror always reflects the
# CURRENT upstream and can never be masked by a stale local checkout. The
# materialised examples/sky/<name>/ trees are regenerated every run and
# git-ignored — only the control surface under examples/sky/ is tracked
# (manifest.toml, rename-map.tsv, ipe-patches/, README.md).
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
#   sky_mirror_one <name> <orig> <ipe> -> write the raw tree to <orig> and the
#                                         transformed+edited Ipê tree to <ipe>. 0=ok.
#   mirror_sky_examples           -> regenerate the whole in-scope set into
#                                    examples/sky/{original,ipe}/.
#   sky_example_names             -> one in-scope example basename per line (from manifest).

: "${REPO:?mirror.sh: REPO must be set (source lib/env.sh first)}"
SKY_TRANSFORM="${SKY_TRANSFORM:-$REPO/scripts/lib/sky-to-ipe-transform.py}"
SKY_EDITS_APPLY="${SKY_EDITS_APPLY:-$REPO/scripts/lib/apply-ipe-edits.py}"
SKY_RENAME_MAP="${SKY_RENAME_MAP:-$REPO/examples/sky/rename-map.tsv}"
SKY_EDITS_DIR="${SKY_EDITS_DIR:-$REPO/examples/sky/ipe-edits}"
# Per-example whole-port override trees (a structural rebuild that replaces the
# transformed port; the edits/rename-map are skipped for an overridden example).
SKY_OVERRIDE_DIR="${SKY_OVERRIDE_DIR:-$REPO/examples/sky/ipe-overrides}"
# Committed output trees: the raw upstream snapshot and the runnable Ipê port.
SKY_ORIGINAL_DIR="${SKY_ORIGINAL_DIR:-$REPO/examples/sky/original}"
SKY_IPE_DIR="${SKY_IPE_DIR:-$REPO/examples/sky/ipe}"

# ── sky_example_names: in-scope example basenames from the manifest ───────────
# Enumerates every [[example]] name in examples/sky/manifest.toml. The sweep's
# is_out_of_scope filter (lib/examples.sh) still excludes Go-FFI examples from
# the BUILD set; listing them here only tells the mirror they are known.
sky_example_names() {
  local manifest="$REPO/examples/sky/manifest.toml"
  [ -f "$manifest" ] || { echo "mirror: manifest not found at $manifest" >&2; return 1; }
  python3 -c "
import re, sys
with open('$manifest') as f:
    for line in f:
        m = re.match(r'''^\s*name\s*=\s*[\"']([^\"']+)[\"']\s*\$''', line)
        if m:
            print(m.group(1))
"
}

# ── sky_upstream_snapshot: fetch+extract anzellai/sky ONCE, echo examples/ path
# The upstream repo tarball is downloaded and extracted a single time per sweep
# (memoised in _SKY_SNAPSHOT_EXAMPLES), then every example is copied from it —
# so the whole sweep pays one ~10 MB fetch, not one per example. Returns
# non-zero (with a diagnostic) when offline / curl unavailable.
_SKY_SNAPSHOT_EXAMPLES=""
sky_upstream_snapshot() {
  if [ -n "$_SKY_SNAPSHOT_EXAMPLES" ] && [ -d "$_SKY_SNAPSHOT_EXAMPLES" ]; then
    printf '%s\n' "$_SKY_SNAPSHOT_EXAMPLES"; return 0
  fi
  command -v curl >/dev/null 2>&1 || { echo "mirror: curl unavailable — cannot fetch upstream" >&2; return 1; }
  local ref="${SKY_UPSTREAM_REF:-main}"
  local url="https://codeload.github.com/anzellai/sky/tar.gz/refs/heads/${ref}"
  local root; root="$(mktemp -d)" || return 1
  if ! curl -fsSL --max-time 120 "$url" -o "$root/sky.tgz" 2>/dev/null ||
     ! tar -xzf "$root/sky.tgz" -C "$root" "sky-${ref}/examples" 2>/dev/null; then
    rm -rf "$root"
    echo "mirror: upstream fetch from anzellai/sky@${ref} failed (offline?)" >&2
    return 1
  fi
  _SKY_SNAPSHOT_EXAMPLES="$root/sky-${ref}/examples"
  printf '%s\n' "$_SKY_SNAPSHOT_EXAMPLES"
}

# ── sky_upstream_names_network: every upstream example basename with an entry ──
# Lists the upstream repo's example dirs that carry a Sky entry point, so the
# sweep can flag any upstream example missing from the manifest. `rust` is a
# helper crate, not an example, and is excluded by the caller.
sky_upstream_names_network() {
  local ex; ex="$(sky_upstream_snapshot)" || return 1
  local d
  for d in "$ex"/*/; do
    [ -d "$d" ] || continue
    [ -f "${d}src/Main.sky" ] || [ -f "${d}src/Main.ipe" ] || continue
    basename "$d"
  done
}

# ── _fetch_sky_example_network <name> <dst>: copy one example from the snapshot
# Copies the upstream example subtree from the one-time snapshot into <dst>.
# Returns non-zero when the fetch failed or the example is absent upstream, so
# the caller reports it as skipped-no-source rather than a false red.
_fetch_sky_example_network() {
  local name="$1" dst="$2" ex
  ex="$(sky_upstream_snapshot)" || return 1
  [ -d "$ex/$name" ] || return 1
  mkdir -p "$dst"
  cp -rf "$ex/$name/." "$dst/"
}

# ── sky_mirror_one <name> <orig> <ipe>: raw snapshot + transformed Ipê port ───
# Fetches the upstream tree from GitHub (anzellai/sky) into <orig> verbatim (the
# committed raw snapshot), then derives the Ipê port in <ipe>. Returns 0 on
# success, 1 when the upstream fetch fails, 2 when an edit fails to apply.
sky_mirror_one() {
  local name="$1" orig="$2" ipe="$3"
  rm -rf "$orig"
  # Fetch from the upstream GitHub repo — the sole source, so the snapshot always
  # holds the CURRENT upstream and a stale local checkout can never mask it.
  if ! _fetch_sky_example_network "$name" "$orig"; then
    echo "mirror: no source for '$name' (upstream fetch from anzellai/sky failed — offline or curl unavailable)" >&2
    return 1
  fi
  # Drop any build/cache artefacts that rode along in the upstream tree, so the
  # committed raw snapshot is source-only.
  rm -rf "$orig/sky-out" "$orig/out" "$orig/.ipe" "$orig/.ipecache" "$orig/.ipedeps" \
         "$orig/.skydeps" "$orig/.skyache" "$orig/target" 2>/dev/null
  sky_transform_one "$name" "$orig" "$ipe"
}

# ── sky_transform_one <name> <orig-src> <ipe-dst>: raw tree -> Ipê port ───────
# Derives the Ipê port in <ipe-dst> from an already-materialised raw tree
# <orig-src> (no network). Two mutually-exclusive modes:
#
#   • OVERRIDE — if examples/sky/ipe-overrides/<name>/ exists, the port IS that
#     tree, copied wholesale; the token rewrite + edits are skipped entirely.
#     This is for a port that is a structural REBUILD of the upstream example
#     rather than a transform of it (e.g. a Go-FFI example reimplemented on
#     Rust-crate FFI), where a per-line edit cannot express the delta and files
#     are added/removed. The raw upstream stays in examples/sky/original/<name>/
#     as the reference the rebuild diverges from.
#   • TRANSFORM (default) — rename every *.sky -> *.ipe, rename sky.toml ->
#     ipe.toml (rewriting its `entry` key), apply the rename-map token rewrite,
#     then the optional content-anchored ipe-edits/<name>.edits semantic delta.
#
# Returns 0 on success, 2 when an edit fails to apply (a real regression to
# surface, never ignore). Offline + deterministic — the basis of `--check`.
sky_transform_one() {
  local name="$1" orig="$2" ipe="$3"
  rm -rf "$ipe"
  mkdir -p "$ipe"

  # OVERRIDE mode: the committed override tree is the port verbatim.
  local override="$SKY_OVERRIDE_DIR/$name"
  if [ -d "$override" ]; then
    cp -rf "$override/." "$ipe/"
    return 0
  fi

  # TRANSFORM mode: the Ipê port starts as a copy of the raw snapshot.
  cp -rf "$orig/." "$ipe/"

  # *.sky -> *.ipe (source extension rename).
  local f
  while IFS= read -r f; do
    mv -f "$f" "${f%.sky}.ipe"
  done < <(find "$ipe" -type f -name '*.sky' -print 2>/dev/null)

  # Project manifest: sky.toml -> ipe.toml (Ipê's canonical manifest name), with
  # the `entry` key's src/Main.sky -> src/Main.ipe rewrite. Renaming the file (not
  # just editing it) is load-bearing: every consumer of the port
  # (ipe_build_target, is_wasm_example, resolve_bin) keys off `ipe.toml`, so the
  # port must be indistinguishable from a native Ipê project.
  if [ -f "$ipe/sky.toml" ]; then
    sed -i.bak -E 's/^([[:space:]]*entry[[:space:]]*=[[:space:]]*")([^"]*)\.sky(")/\1\2.ipe\3/' "$ipe/sky.toml"
    rm -f "$ipe/sky.toml.bak"
    mv -f "$ipe/sky.toml" "$ipe/ipe.toml"
  fi

  # Bare stdlib imports (`import System` -> `import Ipe.System`): the set of
  # top-level Ipê stdlib module names (Ipe/<X>.ipe) that this example bare-imports
  # AND does NOT shadow with a local <X>.ipe. Local-first, mirroring Sky's own
  # import resolution.
  local bare bareflag=(); bare="$(_sky_bare_stdlib_set "$ipe")"
  [ -n "$bare" ] && bareflag=(--bare-stdlib "$bare")

  # Step 1 — the shared token rewrite (code-only; strings/comments preserved).
  find "$ipe" -type f -name '*.ipe' -print0 2>/dev/null \
    | xargs -0 -r python3 "$SKY_TRANSFORM" "${bareflag[@]}" "$SKY_RENAME_MAP"

  # Step 2 — the optional per-example content-anchored edits. An edit whose
  # `find` text no longer occurs (upstream genuinely changed the target) fails
  # loud: return non-zero so the sweep surfaces it as a RED row rather than
  # shipping a half-edited port. Unlike a line-numbered diff, an anchored edit
  # is immune to the line shifts upstream makes between releases.
  local edits="$SKY_EDITS_DIR/$name.edits"
  if [ -f "$edits" ] && [ -s "$edits" ]; then
    if ! python3 "$SKY_EDITS_APPLY" "$edits" "$ipe"; then
      echo "mirror: ipe-edits/$name.edits failed to apply cleanly to '$name'" >&2
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

# ── mirror_sky_examples: regenerate the whole in-scope set ────────────────────
# Writes examples/sky/original/<name>/ (raw) and examples/sky/ipe/<name>/
# (transformed+edited) for every manifest example. Returns 0 if at least one
# regenerated; prints the count. Non-fatal per example (a missing-source example
# is skipped). An edit-apply failure (rc 2) is counted separately and printed,
# but does not abort the whole regeneration.
mirror_sky_examples() {
  local names ok=0 fail=0 editfail=0 name rc
  names="$(sky_example_names)" || {
    echo "mirror: cannot enumerate examples (examples/sky/manifest.toml missing or unreadable)" >&2
    return 1
  }
  while IFS= read -r name; do
    [ -z "$name" ] && continue
    sky_mirror_one "$name" "$SKY_ORIGINAL_DIR/$name" "$SKY_IPE_DIR/$name"; rc=$?
    case "$rc" in
      0) ok=$((ok+1)) ;;
      2) editfail=$((editfail+1)) ;;
      *) fail=$((fail+1)) ;;
    esac
  done <<< "$names"
  echo "mirror: regenerated $ok example(s)${fail:+, $fail without source}${editfail:+, $editfail with a failing edit}"
  [ "$ok" -gt 0 ]
}
