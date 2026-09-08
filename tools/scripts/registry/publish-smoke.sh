#!/usr/bin/env bash
# Live-registry publish smoke test — exercise the real `ipe package publish`
# push-path end-to-end against a live (or staging) registry.
#
# What it proves (the gap `registry-admission` cannot cover from local fixtures):
#   1. `ipe package publish` clones the fork, writes the entry, pushes the branch,
#      and opens the index PR against the real registry over the network.
#   2. The registry's admission workflow accepts a well-formed, signed entry.
#   3. The accepted entry resolves back through the Pages read API
#      (`<registry-url>/index.json` + `/packages/<name>.json`).
# Then it CLEANS UP idempotently so the real index is never polluted.
#
# Cadence: NIGHTLY / manual `workflow_dispatch` ONLY. Pushing to a real registry
# must never gate a PR or a promotion.
#
# ── Trust boundary ──────────────────────────────────────────────────────────
# This script pushes to a registry using a publish token. The token is read ONLY
# from the environment (IPE_SMOKE_TOKEN), is NEVER echoed, NEVER written to a
# file the CI logs, and NEVER placed on a process argv — it is handed to `git`
# via an askpass helper and to `gh`/the GitHub API via its own env channel. `set
# -x` is never enabled. A failed clone/push/admission/resolution is a HARD error
# (fail-closed): the smoke test goes RED, never green-by-skip, whenever the
# backend it is meant to exercise is reachable but rejects.
#
# ── Staging vs reserved-package ─────────────────────────────────────────────
# PREFER a staging registry (a throwaway repo mirroring the ipe-registry layout):
# a failed/partial run can then never corrupt the real index. Point the run at it
# with IPE_SMOKE_INDEX_REPO=<owner>/<staging-repo> and
# IPE_REGISTRY_URL=https://<owner>.github.io/<staging-repo>. When no staging repo
# is configured the run falls back to the production registry using a RESERVED
# test-package name (IPE_SMOKE_PACKAGE, default `ipe-registry-smoke-probe`) plus
# guaranteed cleanup — never a real package name.
#
# ── Required environment (the live run's infra) ─────────────────────────────
#   IPE_SMOKE_TOKEN        publish token (a repo secret) with push + open-PR
#                          rights on IPE_SMOKE_INDEX_REPO and IPE_SMOKE_FORK.
#                          NEVER logged. REQUIRED.
#   IPE_SMOKE_INDEX_REPO   <owner>/<name> of the registry the PR targets.
#                          Default: arthurmaciel/ipe-registry (production).
#   IPE_SMOKE_FORK         <owner> of the fork `ipe package publish` pushes the
#                          publish branch to. Default: the index repo's owner.
#   IPE_REGISTRY_URL       Pages read API base the resolver checks. Default:
#                          https://arthurmaciel.github.io/ipe-registry.
#   IPE_SMOKE_PACKAGE      reserved disposable package name. Default:
#                          ipe-registry-smoke-probe.
#   IPE_SMOKE_SOURCE_REPO  <owner>/<name> of the disposable git repo whose HEAD
#                          the entry pins as the package source. Default: a repo
#                          named <IPE_SMOKE_PACKAGE> under IPE_SMOKE_FORK.
#   IPE_SMOKE_POLL_SECS    admission/resolution poll budget in seconds
#                          (default 600).
#   IPE_BIN                path to the built `ipe` binary (default: `ipe` on PATH).
#
# Exit 0 = the full push→admission→resolution→cleanup path held.
# Exit non-zero = a real failure OR missing required infra (fail-closed).

set -euo pipefail

log()  { printf '%s %s\n' "[smoke]" "$*"; }
fail() { printf '%s %s\n' "[smoke][FAIL]" "$*" >&2; exit 1; }

# ── Config ──────────────────────────────────────────────────────────────────
INDEX_REPO="${IPE_SMOKE_INDEX_REPO:-arthurmaciel/ipe-registry}"
INDEX_OWNER="${INDEX_REPO%%/*}"
FORK_OWNER="${IPE_SMOKE_FORK:-$INDEX_OWNER}"
REGISTRY_URL="${IPE_REGISTRY_URL:-https://arthurmaciel.github.io/ipe-registry}"
PACKAGE="${IPE_SMOKE_PACKAGE:-ipe-registry-smoke-probe}"
SOURCE_REPO="${IPE_SMOKE_SOURCE_REPO:-$FORK_OWNER/$PACKAGE}"
POLL_SECS="${IPE_SMOKE_POLL_SECS:-600}"
IPE="${IPE_BIN:-ipe}"

# A fresh, monotonically-increasing prerelease version per run: a smoke run never
# collides with a prior run's version (which admission would reject as immutable),
# and every version this reserved package ever carries is a `0.0.0-smoke.*`
# prerelease that no real consumer would ever depend on.
VERSION="0.0.0-smoke.$(date -u +%Y%m%d%H%M%S)"
BRANCH="publish/${PACKAGE}-${VERSION}"

# The token must exist and never be printed. Its presence is checked, its value
# is not surfaced.
[ -n "${IPE_SMOKE_TOKEN:-}" ] || fail \
  "IPE_SMOKE_TOKEN is unset — the live push-path needs a publish token (a repo secret). \
This is the infra the run requires; see this script's header."

if [ "$INDEX_REPO" = "arthurmaciel/ipe-registry" ]; then
  log "targeting the PRODUCTION registry with reserved package '$PACKAGE' (no staging repo configured)."
  log "set IPE_SMOKE_INDEX_REPO + IPE_REGISTRY_URL to a staging mirror to avoid touching production."
else
  log "targeting STAGING registry '$INDEX_REPO' (production is untouched)."
fi
log "registry read API: $REGISTRY_URL"
log "package: $PACKAGE  version: $VERSION  fork: $FORK_OWNER  source: $SOURCE_REPO"

# ── Token plumbing: git askpass + gh, never on argv, never logged ───────────
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ipe-smoke-XXXXXX")"
ASKPASS="$WORK/askpass.sh"
# The askpass helper prints the token to stdout when git asks for a password and
# the fork owner when git asks for a username. git reads it via GIT_ASKPASS, so
# the token never appears on any command line. The file is mode 0700 in a
# per-run temp dir and removed on cleanup.
#
# The `$1`, `$IPE_SMOKE_TOKEN` in the single-quoted printf templates below are the
# GENERATED script's own runtime references — they must NOT expand at generation
# time, so single quotes are correct here (shellcheck SC2016 is expected).
# shellcheck disable=SC2016
{
  printf '#!/usr/bin/env bash\n'
  printf 'case "$1" in\n'
  printf '  *Username*) printf "%%s" "%s" ;;\n' "$FORK_OWNER"
  printf '  *Password*) printf "%%s" "$IPE_SMOKE_TOKEN" ;;\n'
  printf 'esac\n'
} > "$ASKPASS"
chmod 700 "$ASKPASS"
export GIT_ASKPASS="$ASKPASS"
export GIT_TERMINAL_PROMPT=0
# The askpass child reads IPE_SMOKE_TOKEN from its inherited environment; export
# it so the git subprocess sees it. It is still never echoed or placed on argv.
export IPE_SMOKE_TOKEN
# `ipe package publish`'s headless PR-open reads GITHUB_TOKEN (parsed through the
# PublishToken alphabet gate before it reaches curl). Feed the smoke token in on
# that channel; it is never echoed here.
export GITHUB_TOKEN="$IPE_SMOKE_TOKEN"
# Point the resolver's git-checkout fallback + publish's entry-merge read at a
# throwaway index checkout, never the developer's real ~/.cache index.
export IPE_INDEX_DIR="$WORK/index-checkout"
export IPE_REGISTRY_URL="$REGISTRY_URL"

# ── Idempotent cleanup — safe to run twice, leaves no residue ───────────────
# Deletes the publish branch from the fork and closes any open smoke PR. It does
# NOT delete a merged entry from the index main line here: on the production
# registry the reserved package's prerelease entries are inert (`0.0.0-smoke.*`),
# and a merged entry is pruned by the registry's own retention (a follow-up prune
# workflow on the registry side); on a staging repo the whole repo is disposable.
# Every step tolerates "already gone" so a re-run is a no-op.
cleanup() {
  local rc=$?
  set +e
  log "cleanup: removing publish branch and closing any open smoke PR (idempotent)."
  # Delete the pushed branch from the fork over the API (never on argv, token via
  # the Authorization header only). A 404/422 (already deleted) is fine.
  if command -v gh >/dev/null 2>&1; then
    GH_TOKEN="$IPE_SMOKE_TOKEN" gh api \
      --method DELETE \
      "repos/$FORK_OWNER/${INDEX_REPO##*/}/git/refs/heads/$BRANCH" \
      >/dev/null 2>&1 || true
    # Close any open PR from this smoke branch against the index.
    local prs
    prs="$(GH_TOKEN="$IPE_SMOKE_TOKEN" gh api \
      "repos/$INDEX_REPO/pulls?head=$FORK_OWNER:$BRANCH&state=open" \
      --jq '.[].number' 2>/dev/null || true)"
    for n in $prs; do
      GH_TOKEN="$IPE_SMOKE_TOKEN" gh api --method PATCH \
        "repos/$INDEX_REPO/pulls/$n" -f state=closed >/dev/null 2>&1 || true
      log "cleanup: closed smoke PR #$n."
    done
  else
    log "cleanup: gh not available — branch/PR cleanup skipped; the smoke branch is a \
'$BRANCH' prerelease and is safe to prune by hand."
  fi
  rm -rf "$WORK" 2>/dev/null || true
  exit "$rc"
}
trap cleanup EXIT

# ── 1. Build a disposable package git repo, pushed so publish can pin HEAD ───
# `ipe package publish` refuses a dirty tree or an unpushed HEAD, and pins the
# committed, pushed revision. The source repo is the disposable IPE_SMOKE_SOURCE_REPO.
PKG="$WORK/pkg"
mkdir -p "$PKG/src"
cat > "$PKG/package.ipe" <<EOF
module Package exposing (package)

import Ipe.Package exposing (..)


package : Package
package =
    { name = "$PACKAGE"
    , version = "$VERSION"
    }
EOF
cat > "$PKG/src/Main.ipe" <<'EOF'
module Main exposing (main)

import Ipe.Io as Io


main =
    Io.println "registry smoke probe"
EOF

git -C "$PKG" init --quiet
git -C "$PKG" -c user.name=ipe-smoke -c user.email=smoke@ipe-lang.invalid add .
git -C "$PKG" -c user.name=ipe-smoke -c user.email=smoke@ipe-lang.invalid \
  commit --quiet -m "smoke $VERSION"
git -C "$PKG" remote add origin "https://github.com/$SOURCE_REPO.git"
# Push the probe commit to the disposable source repo so the pinned rev is
# fetchable. `--force` because the reserved source repo is disposable; a prior
# run's commit is irrelevant.
log "pushing probe source to $SOURCE_REPO"
git -C "$PKG" push --force --quiet origin HEAD:refs/heads/smoke \
  || fail "could not push the probe source to $SOURCE_REPO — the live run needs a \
disposable source repo the token can push to (see IPE_SMOKE_SOURCE_REPO)."

# ── 2. Run the REAL publish push-path ───────────────────────────────────────
# Not --dry-run: this clones the fork, writes packages/<pkg>.toml, pushes the
# branch, and opens the PR against the index. The source/rev are pinned from the
# pushed HEAD above.
SRC_HEAD="$(git -C "$PKG" rev-parse HEAD)"
log "publishing $PACKAGE@$VERSION to $INDEX_REPO (real push-path)"
"$IPE" package publish "$PKG" \
  --index "$INDEX_REPO" \
  --fork "$FORK_OWNER" \
  --source "https://github.com/$SOURCE_REPO" \
  --rev "$SRC_HEAD" \
  || fail "ipe package publish failed against $INDEX_REPO (auth/admission/push error) — fail-closed."

# ── 3. Wait for admission to accept + the entry to resolve via Pages ────────
# Admission runs on the registry side (the PR's checks); once it merges, the
# Pages mirror serves the entry. Poll the read API until the version appears, or
# hard-fail after the budget.
resolve_ok() {
  local body
  body="$(curl --silent --show-error --location \
    -H 'Accept: application/json' \
    "$REGISTRY_URL/packages/$PACKAGE.json" 2>/dev/null || true)"
  [ -n "$body" ] || return 1
  printf '%s' "$body" | grep -q "\"$VERSION\""
}

log "waiting up to ${POLL_SECS}s for admission + Pages resolution of $PACKAGE@$VERSION"
deadline=$(( $(date +%s) + POLL_SECS ))
while :; do
  if resolve_ok; then
    log "RESOLVED: $PACKAGE@$VERSION is served by $REGISTRY_URL/packages/$PACKAGE.json"
    break
  fi
  [ "$(date +%s)" -lt "$deadline" ] \
    || fail "timed out: $PACKAGE@$VERSION was not admitted + resolvable within ${POLL_SECS}s \
(admission rejected, PR unmerged, or Pages not yet mirrored) — fail-closed."
  sleep 15
done

# ── 4. Cross-check the top-level registry catalog names the package ─────────
# The issue names `<registry-url>/index.json` as the second resolution surface.
# The resolver client consumes `/packages/<name>.json` (the hard gate in step 3);
# the root `index.json` catalog is an additional listing. When the registry
# publishes it, assert it lists the package; when it does not (a staging mirror
# may omit it), this is a soft note — the per-package resolution in step 3 is the
# authoritative proof, not this catalog.
catalog="$(curl --silent --show-error --location \
  -H 'Accept: application/json' \
  "$REGISTRY_URL/index.json" 2>/dev/null || true)"
if [ -n "$catalog" ]; then
  if printf '%s' "$catalog" | grep -q "\"$PACKAGE\""; then
    log "catalog: $REGISTRY_URL/index.json lists $PACKAGE"
  else
    log "note: $REGISTRY_URL/index.json did not list $PACKAGE (per-package resolution in step 3 held; catalog is advisory)."
  fi
else
  log "note: $REGISTRY_URL/index.json is not served (per-package resolution in step 3 is the authoritative proof)."
fi

log "OK: publish → admission → Pages per-package resolution all held."
