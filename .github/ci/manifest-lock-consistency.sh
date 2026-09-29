#!/usr/bin/env bash
# Cargo.lock pins every workspace member that inherits the workspace version to
# the version Cargo.toml declares, and release-please's release commit bumps
# both files together (see manifest_lock_consistency.py). A desynced tree fails
# `cargo build --locked` after a release; this gate refuses to merge one.
# Stdlib Python, no toolchain, so it stays on the fast required PR path.
set -euo pipefail
exec python3 "$(dirname "$0")/manifest_lock_consistency.py"
