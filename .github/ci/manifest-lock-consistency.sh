#!/usr/bin/env bash
# Cargo.lock must pin the first-party crates to the same version Cargo.toml
# declares. release-please bumps `[workspace.package] version` in Cargo.toml on a
# release; if Cargo.lock is not synced in lockstep, `cargo build --locked` fails
# after the release and the next plain build dirties the tree. This gate refuses
# to merge a version-desynced tree, so a release can never ship a stale lock even
# when the release-please `sync-cargo-lock` job loses its force-push race — the
# job (or a local `cargo update --workspace`) is the fixer; this is the standing
# guard. Pure grep + sed (no ripgrep, no toolchain) so it stays on the fast
# required PR path — the runner for this job installs nothing.
set -euo pipefail

# The version SSOT is the single marked line in Cargo.toml.
manifest_version="$(sed -n 's/^version = "\([^"]*\)" # x-release-please-version.*/\1/p' Cargo.toml | head -n1)"
if [ -z "$manifest_version" ]; then
  echo "manifest-lock-consistency: could not read the x-release-please-version marker from Cargo.toml" >&2
  exit 1
fi

# The first-party crate `ipe` inherits the workspace version; its Cargo.lock entry
# is the canonical lockstep check. The `name = "ipe"` line is exact-anchored so it
# never matches a sibling crate (ipe_kernels, ipe_lsp_server, …).
lock_version="$(grep -A2 '^name = "ipe"$' Cargo.lock | sed -n 's/^version = "\([^"]*\)".*/\1/p' | head -n1)"
if [ -z "$lock_version" ]; then
  echo "manifest-lock-consistency: could not read the 'ipe' version from Cargo.lock" >&2
  exit 1
fi

if [ "$manifest_version" != "$lock_version" ]; then
  echo "manifest-lock-consistency: Cargo.lock is out of sync with Cargo.toml." >&2
  echo "  Cargo.toml [workspace.package] version = $manifest_version" >&2
  echo "  Cargo.lock 'ipe' version              = $lock_version" >&2
  echo "Run 'cargo update --workspace' and commit the updated Cargo.lock." >&2
  exit 1
fi

echo "manifest-lock-consistency: OK ($manifest_version)"
