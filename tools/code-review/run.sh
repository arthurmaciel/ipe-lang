#!/usr/bin/env bash
# Launch the code-review app against this checkout's ipe-index database.
#
# A bare `ipe run` from this directory fails three ways; this wrapper fixes all:
#
#   1. DB path — the app defaults IPE_INDEX_DB to `.ipe-index/index.db` relative
#      to the current directory, but the index lives at the repo root. A missing
#      file surfaces as SQLite `database error [14]` (SQLITE_CANTOPEN). We pin
#      IPE_INDEX_DB / IPE_INDEX_ROOT to absolute repo-root paths.
#
#   2. Runtime version — from inside a compiler checkout, `ipe` auto-discovers the
#      vendored `src/runtime/rust` snapshot, which can lag the installed compiler
#      and fail the build with a version-mismatch error. We pin IPE_RUNTIME_DIR to
#      the installed compiler's own materialized runtime.
#
#   3. `ipe run` binary-name mismatch — the emit names the crate `ipe-app`, but
#      `ipe run` looks for a binary named after the project (`code-review`) and
#      dies with `io error ... No such file or directory`. We `ipe build` and then
#      exec the emitted `ipe-app` binary directly, locating it via cargo metadata
#      so a shared `target-dir` is respected.
#
# Usage:
#   tools/code-review/run.sh                 # build + serve on http://localhost:8000
#   tools/code-review/run.sh build           # pass any other subcommand through to ipe
#   tools/code-review/run.sh type-check
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/../.." && pwd)"

# --- shared env: DB location + version-matched runtime ----------------------
index_db="$repo_root/.ipe-index/index.db"
if [[ ! -f "$index_db" ]]; then
  echo "error: index DB not found at $index_db" >&2
  echo "build it first from the repo root:  tools/scripts/ipe-index index" >&2
  exit 1
fi

ver="$(ipe version | awk '{for (i=1;i<=NF;i++) if ($i ~ /^v?[0-9]+\.[0-9]+\.[0-9]+$/) {sub(/^v/,"",$i); print $i}}')"
if [[ -z "$ver" ]]; then
  echo "error: could not parse a version from 'ipe version' — is ipe on PATH?" >&2
  exit 1
fi
runtime_dir="$HOME/.ipe/runtime/$ver/rust"
if [[ ! -f "$runtime_dir/Cargo.toml" ]]; then
  echo "error: no materialized runtime for compiler $ver at $runtime_dir" >&2
  echo "materialize it once from outside this checkout:" >&2
  echo "  (cd /tmp && env -u IPE_RUNTIME_DIR ipe health)" >&2
  exit 1
fi

export IPE_INDEX_DB="$index_db"
export IPE_INDEX_ROOT="$repo_root"
export IPE_RUNTIME_DIR="$runtime_dir"

cd "$here"

# --- pass-through: any explicit subcommand other than the default run -------
if [[ $# -gt 0 && "$1" != "run" && "$1" != "serve" ]]; then
  exec ipe "$@"
fi

# --- default: build, then exec the emitted server binary directly -----------
ipe build

out_manifest="$here/out/rust/Cargo.toml"
if [[ ! -f "$out_manifest" ]]; then
  echo "error: emitted crate not found at $out_manifest (did 'ipe build' succeed?)" >&2
  exit 1
fi

# Ask cargo where the artifacts landed (honours a user-level target-dir pin).
target_dir="$(cargo metadata --manifest-path "$out_manifest" --format-version 1 --no-deps 2>/dev/null \
  | grep -o '"target_directory":"[^"]*"' | head -1 | sed 's/.*:"//; s/"$//')"
if [[ -z "$target_dir" ]]; then
  echo "error: could not determine cargo target directory via 'cargo metadata'" >&2
  exit 1
fi

bin="$target_dir/debug/ipe-app"
if [[ ! -x "$bin" ]]; then
  echo "error: emitted binary not found at $bin" >&2
  exit 1
fi

echo "serving code-review on http://localhost:8000  (Ctrl-C to stop)"
exec "$bin"
