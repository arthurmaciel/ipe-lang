#!/usr/bin/env bash
# install-hooks.sh — point this repo's git hooks at scripts/hooks/ so the
# post-commit / post-merge index refresh runs locally. One command, idempotent:
#
#   scripts/install-hooks.sh
#
# It sets `core.hooksPath` to scripts/hooks (repo-local git config only — never
# touches global config or other repos). Undo with:
#
#   git config --unset core.hooksPath
#
# The hooks themselves are non-blocking: they refresh the ipe-index and never
# fail a commit or merge (see scripts/hooks/post-commit).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

chmod +x scripts/hooks/post-commit scripts/hooks/post-merge
git config core.hooksPath scripts/hooks

echo "Installed: core.hooksPath -> scripts/hooks"
echo "post-commit + post-merge will refresh the ipe-index (non-blocking)."
