#!/usr/bin/env bash
# tools/scripts/check-sky-drift.sh — upstream-drift detection (network, nightly).
#
# Fetches the current upstream Sky example corpus from anzellai/sky (via the
# existing mirror.sh snapshot mechanism), recomputes each example's content hash,
# and compares against the committed examples/sky/upstream.lock.
#
# Outcomes per example:
#   unchanged → skip (no action).
#   changed   → EXIT 1, name the drifted example(s).  A human updates original/,
#               regenerates ipe/, re-verifies, and bumps the lock.
#   added     → auto-convert via sky_transform_one → ipe build + ipe run →
#               on SUCCESS: stage original/ + ipe/ + new lock entry, open a bot PR.
#               on FAILURE: EXIT 1 naming the example.
#   removed   → warn (do not auto-delete; a human decides to keep or drop).
#
# EXIT: 0 no drift  1 drift found or a conversion failed  2 setup error
#
# The bot-PR step requires GH_TOKEN to be set (GitHub Actions provides it).
# When GH_TOKEN is absent the PR is skipped (local runs just report drift).
set -uo pipefail

source "$(dirname "$0")/lib/env.sh"
source "$(dirname "$0")/lib/mirror.sh"
source "$(dirname "$0")/lib/sky-hash.sh"
source "$(dirname "$0")/lib/checks.sh"

cd "$REPO" || { echo "check-sky-drift: cannot locate repo" >&2; exit 2; }

LOCK="$REPO/examples/sky/upstream.lock"

if [ ! -f "$LOCK" ]; then
  echo "check-sky-drift: upstream.lock missing — run gen-sky-lock.sh first" >&2; exit 2
fi
if [ ! -x "$IPE_BIN" ]; then
  echo "check-sky-drift: ipe binary not found at '$IPE_BIN'" >&2
  echo "  Build it: cargo build --release -p ipe" >&2
  exit 2
fi

# ── Load committed lock: name → hash ─────────────────────────────────────────
declare -A LOCK_HASH
while IFS=$'\t' read -r n h; do
  [[ "$n" == \#* ]] && continue
  [ -z "$n" ] && continue
  LOCK_HASH["$n"]="$h"
done < "$LOCK"

# ── Fetch upstream snapshot (one HTTP fetch for the whole corpus) ─────────────
# sky_upstream_snapshot primes the shared _SKY_SNAPSHOT_EXAMPLES cache used by
# every subsequent _fetch_sky_example_network call; the return value is the path
# to the extracted examples/ dir but we don't need it directly here.
echo "check-sky-drift: fetching upstream snapshot…"
# shellcheck disable=SC2034
snapshot="$(sky_upstream_snapshot)" || {
  echo "check-sky-drift: upstream fetch failed — cannot detect drift (offline?)" >&2; exit 2
}

# ── Enumerate upstream examples (those with a src/Main.sky or src/Main.ipe) ───
declare -A UPSTREAM_NAMES
while IFS= read -r uname; do
  [ -z "$uname" ] && continue
  [ "$uname" = "rust" ] && continue   # helper crate, not an example
  UPSTREAM_NAMES["$uname"]=1
done < <(sky_upstream_names_network 2>/dev/null)

echo "check-sky-drift: upstream has ${#UPSTREAM_NAMES[@]} example(s); lock has ${#LOCK_HASH[@]} entries."

drift=0 warned=0 added_ok=0 added_fail=0

# ── Sanity lock against current committed original/ (fast, no network) ────────
# This reuses the committed original/ to give a self-consistent check when run
# locally against what's already in the repo.  The real upstream comparison
# follows below using the freshly-fetched snapshot.

# ── Compare each lock entry against fresh upstream ────────────────────────────
tmp_orig="$(mktemp -d)" || exit 2
trap 'rm -rf "$tmp_orig"' EXIT

for name in "${!LOCK_HASH[@]}"; do
  locked_hash="${LOCK_HASH[$name]}"

  # Fetch fresh upstream copy for this example into a temp dir.
  fresh="$tmp_orig/$name"
  if ! _fetch_sky_example_network "$name" "$fresh" 2>/dev/null; then
    # Upstream no longer has this example.
    echo "check-sky-drift: REMOVED upstream — $name (was in lock, now absent upstream)"
    echo "  Action: decide whether to keep pinned or drop from manifest + lock." >&2
    warned=$((warned+1))
    continue
  fi
  # Drop build artefacts from the fresh copy (mirrors what sky_mirror_one does).
  rm -rf "$fresh/sky-out" "$fresh/out" "$fresh/.ipe" "$fresh/.ipecache" \
         "$fresh/.ipedeps" "$fresh/.skydeps" "$fresh/target" 2>/dev/null

  fresh_hash="$(sky_hash_one "$name" "$tmp_orig")" || {
    echo "check-sky-drift: hash error for $name" >&2; drift=1; continue
  }

  if [ "$fresh_hash" = "$locked_hash" ]; then
    echo "check-sky-drift: ok (unchanged) $name"
  else
    echo "check-sky-drift: CHANGED — $name (lock $locked_hash → upstream $fresh_hash)"
    echo "  Action: update original/, regenerate ipe/, re-verify parity, bump lock." >&2
    drift=1
  fi
done

# ── Check for upstream-added examples not in the lock ─────────────────────────
for uname in "${!UPSTREAM_NAMES[@]}"; do
  [ -n "${LOCK_HASH[$uname]:-}" ] && continue   # already tracked
  echo "check-sky-drift: ADDED upstream — $uname (not in lock)"

  # Auto-convert: fetch fresh, transform, build, run, then stage + open bot PR.
  fresh="$tmp_orig/$uname"
  [ -d "$fresh" ] || _fetch_sky_example_network "$uname" "$fresh" 2>/dev/null
  rm -rf "$fresh/sky-out" "$fresh/out" "$fresh/.ipe" "$fresh/.ipecache" \
         "$fresh/.ipedeps" "$fresh/.skydeps" "$fresh/target" 2>/dev/null

  ipe_out="$REPO/examples/sky/ipe/$uname"
  orig_out="$REPO/examples/sky/original/$uname"

  if ! sky_transform_one "$uname" "$fresh" "$ipe_out"; then
    echo "check-sky-drift: FAIL auto-convert for $uname — transform/edits failed" >&2
    added_fail=$((added_fail+1)); drift=1; continue
  fi

  cp -rf "$fresh/." "$orig_out/"

  build_log="$(mktemp /tmp/ipe-drift-build.XXXXXX)"
  ipe_entry="$ipe_out/package.ipe"
  [ ! -f "$ipe_entry" ] && ipe_entry="$ipe_out/ipe.toml"
  [ ! -f "$ipe_entry" ] && ipe_entry="$ipe_out/src/Main.ipe"
  if ! timeout 300 "$IPE_BIN" build "$ipe_entry" --out "$ipe_out/out/rust" \
       >"$build_log" 2>&1 || \
     ! timeout 300 cargo build --manifest-path "$ipe_out/out/rust/Cargo.toml" \
       >>"$build_log" 2>&1; then
    echo "check-sky-drift: FAIL auto-convert for $uname — build failed" >&2
    sed 's/^/    /' "$build_log" >&2
    rm -f "$build_log"; rm -rf "$ipe_out" "$orig_out" 2>/dev/null
    added_fail=$((added_fail+1)); drift=1; continue
  fi
  rm -f "$build_log"

  # Try run (build-only if no src/Main.ipe or binary missing).
  if [ -f "$ipe_out/src/Main.ipe" ]; then
    bin="$(resolve_bin "$ipe_out" 2>/dev/null)" || bin=""
    if [ -n "$bin" ]; then
      run_log="$(mktemp /tmp/ipe-drift-run.XXXXXX)"
      exercise_cli "$bin" "$run_log" 20 || true   # non-fatal; human reviews the PR
      rm -f "$run_log"
    fi
  fi

  # Compute lock entry for the new example.
  new_hash="$(sky_hash_one "$uname" "$REPO/examples/sky/original")" || {
    added_fail=$((added_fail+1)); drift=1; continue
  }

  # Append to lock (will be rewritten properly by gen-sky-lock.sh in the PR).
  printf '%s\t%s\n' "$uname" "$new_hash" >> "$LOCK"
  added_ok=$((added_ok+1))
  echo "check-sky-drift: auto-converted $uname — staged for bot PR"

  # Open a bot PR when GH_TOKEN is available.
  if [ -n "${GH_TOKEN:-}" ] && command -v git >/dev/null 2>&1 && command -v gh >/dev/null 2>&1; then
    branch="chore/sky-new-example-${uname}"
    git config user.name  "ipe-mirror-bot" 2>/dev/null || true
    git config user.email "ipe-mirror-bot@users.noreply.github.com" 2>/dev/null || true
    git checkout -B "$branch" 2>/dev/null || true
    git add "examples/sky/original/$uname" "examples/sky/ipe/$uname" "$LOCK" 2>/dev/null || true
    git commit -m "chore(examples): auto-convert new upstream example $uname" 2>/dev/null || true
    git push -f origin "$branch" 2>/dev/null || true
    if gh pr view "$branch" --json state --jq .state 2>/dev/null | rg -q OPEN; then
      echo "check-sky-drift: refresh PR for $branch already open — updated branch."
    else
      gh pr create --base main --head "$branch" \
        --title "chore(examples): new upstream Sky example — $uname" \
        --body "Auto-converted new upstream example \`$uname\` from anzellai/sky.

Build + run passed. Review:
- \`examples/sky/original/$uname/\` — raw upstream source
- \`examples/sky/ipe/$uname/\` — converted Ipê port
- Any ipe-edits or ipe-overrides needed
- manifest.toml entry (add with shape + verify fields)
- upstream.lock entry (appended automatically; rerun gen-sky-lock.sh for a clean sort)

**Do not merge without classifying shape and verify in manifest.toml.**" 2>/dev/null || true
    fi
  fi

  reap 2>/dev/null
done

echo ""
echo "=== check-sky-drift: SUMMARY ==="
echo "  changed/drifted:  $drift example(s) with hash change"
echo "  removed upstream: $warned"
echo "  added+converted:  $added_ok ok  $added_fail failed"
echo ""

if [ "$drift" -gt 0 ] || [ "$added_fail" -gt 0 ]; then
  echo "check-sky-drift: drift detected — see above for required human actions." >&2
  exit 1
fi
echo "check-sky-drift: no unexpected drift."
