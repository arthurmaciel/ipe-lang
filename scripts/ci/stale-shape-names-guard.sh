#!/usr/bin/env bash
# Fail-closed guard against stale app-shape names left behind by the
# Live->Web / Webview->WebView shape rename. The current shapes are
# `Ipe.Web` / `Web.app` and `Ipe.WebView` / `WebView.app`; the old spellings
# below no longer resolve (a scaffold or doc that names them is broken or
# misleading). A prior gap of this class shipped a `ipe init` template that
# emitted `Ipe.Live` and would not compile.
#
# Excludes the upstream Sky mirror (examples/sky/), the immutable release
# history (CHANGELOG.md), and this guard's own source (which necessarily names
# the patterns it searches for).
#
# The `TRACKED_BY_OPEN_PR` excludes below are the residual stale-name files that
# were owned by an in-flight PR when this guard landed and so could not be
# swept here. Delete each exclude (and sweep the file) once that PR has merged;
# the goal is an empty exclude list and a fully clean tree.
set -euo pipefail

cd "$(dirname "$0")/../.."

# Stale spellings, as fixed strings. `Live.appRouted` (a historical rejected
# kernel recorded in ADRs) is deliberately NOT matched: the patterns are
# whole-token, so `Live.app` does not match inside `Live.appRouted`.
patterns='Ipe\.Live|(^|[^.[:alnum:]])Live\.app([^a-zA-Z]|$)|Ipe\.Webview|(^|[^.[:alnum:]])Webview\.app([^a-zA-Z]|$)'

hits=$(rg --no-heading --line-number --color never \
  --glob '!examples/sky/**' \
  --glob '!CHANGELOG.md' \
  --glob '!scripts/ci/stale-shape-names-guard.sh' \
  --glob '!AGENTS.md' \
  --glob '!README.md' \
  --glob '!scripts/lib/examples.sh' \
  --glob '!src/compiler/lower/src/lower.rs' \
  --glob '!docs/divergences-from-sky.md' \
  --glob '!docs/architecture/tbd/incremental-compilation-and-watch.md' \
  "$patterns" . || true)

if [ -n "$hits" ]; then
  echo "ERROR: stale shape names found (shape rename left Live/Webview references)." >&2
  echo "Fix to the current names: Ipe.Live->Ipe.Web, Live.app->Web.app," >&2
  echo "Ipe.Webview->Ipe.WebView, Webview.app->WebView.app." >&2
  echo >&2
  echo "$hits" >&2
  exit 1
fi

echo "stale-shape-names guard: clean (no Ipe.Live / Live.app / Ipe.Webview / Webview.app)."
