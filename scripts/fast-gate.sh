#!/usr/bin/env bash
# fast-gate.sh — the canonical cheap gate. ONE script a developer runs locally and
# CI runs on every pull request, so `local == CI` by construction: each gate below
# invokes the SAME command the corresponding CI job uses.
#
# Order is cheapest-first and fail-fast: the first failing gate exits non-zero and
# no later gate runs, so the fastest signal (a formatting slip) surfaces before the
# minutes-long ones. Every gate here targets minutes, not the heavy nightly SEAL
# (full e2e / seal-slice / miri / tier2 jails) which lives off this path.
#
# Usage:
#   scripts/fast-gate.sh                 # nextest over the whole workspace
#   scripts/fast-gate.sh ipe ipe_types   # nextest only these crates (changed set)
#
# Any positional args are treated as a changed-crate set: the nextest gate then
# runs `-p <crate>` for each instead of `--workspace`. Everything else always runs
# over the whole tree (the drift/floor/leak gates are cheap and tree-wide by
# nature).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

# The runtime source the compiler emits against; several gates (explain/doc
# examples, first-party floor) need it CWD-independently.
export IPE_RUNTIME_DIR="${IPE_RUNTIME_DIR:-$REPO/src/runtime/rust/src}"

step() { printf '\n=== fast-gate: %s ===\n' "$1"; }

# ── fmt (ci.yml: fmt) ────────────────────────────────────────────────────────
step "cargo fmt --all -- --check"
cargo fmt --all -- --check

# ── clippy (ci.yml: clippy) ──────────────────────────────────────────────────
step "cargo clippy --all-targets --workspace -- -D warnings"
cargo clippy --all-targets --workspace -- -D warnings

# ── compile floor (ci.yml: quick-check) ──────────────────────────────────────
step "cargo check --workspace"
cargo check --workspace

# ── nextest, changed-crate set or whole workspace (ci.yml: test) ─────────────
step "cargo nextest (unit/integration)"
if [ "$#" -gt 0 ]; then
  crate_args=()
  for c in "$@"; do crate_args+=(-p "$c"); done
  echo "changed-crate set: $*"
  cargo nextest run --profile ci "${crate_args[@]}"
else
  echo "no crate set given — whole workspace"
  cargo nextest run --profile ci --workspace
fi

# ── supply chain (security.yml: cargo-deny) ──────────────────────────────────
step "cargo deny check advisories bans licenses sources"
cargo deny check advisories bans licenses sources

# ── clone-bloat guard (ci.yml: artifact-guard) ───────────────────────────────
step "artifact-guard"
./tools/scripts/ci/artifact-guard.sh

# ── reference-impl leak guard (ci.yml: no-reference-impl-leak) ────────────────
step "no-reference-impl-leak"
bash tools/scripts/no-reference-impl-leak.sh

# ── diagnostic tone (ci.yml: diagnostic-tone) ────────────────────────────────
step "lint-diagnostic-tone"
bash tools/scripts/lint-diagnostic-tone.sh

# ── authored-panic scan (panic-scan.yml: panic-scan) ─────────────────────────
step "panic-scan"
cargo build --release --manifest-path tools/panic-scan/Cargo.toml
mapfile -t panic_files < <(find src -name '*.rs' -not -path '*/tests/*' -not -path '*/templates/*')
tools/panic-scan/target/release/panic-scan "${panic_files[@]}"

# ── build the compiler ONCE for the ipe-driven gates below ───────────────────
step "build ipe (release) for ipe-driven gates"
cargo build --release -p ipe
export IPE_BIN="$REPO/target/release/ipe"

# ── first-party type-check floor (ci.yml: first-party-check-floor) ───────────
step "first-party-check-floor"
bash tools/scripts/first-party-check-floor.sh

# ── explain-page examples (ci.yml: explain-examples) ─────────────────────────
step "check-explain-examples"
bash tools/scripts/check-explain-examples.sh

# ── stdlib doc-string examples (ci.yml: doc-examples) ────────────────────────
step "check-doc-examples"
bash tools/scripts/check-doc-examples.sh

# ── generated-docs drift gates (ci.yml: *-docs-drift) ────────────────────────
# Each regenerates the committed reference and fails on any diff.
step "stdlib-docs-drift"
cargo run -p ipe_docs --bin gen-stdlib-docs -- --repo-root "$REPO"
git diff --exit-code docs/reference/stdlib.md docs/reference/stdlib/

step "env-docs-drift"
cargo run -p ipe_docs --bin gen-env-docs -- --repo-root "$REPO"
git diff --exit-code docs/reference/env.md

step "capabilities-docs-drift"
cargo run -p ipe_docs --bin gen-capabilities-docs -- --repo-root "$REPO"
git diff --exit-code docs/reference/capabilities.md

# ── wasm floor (ci.yml: wasm-floor) ──────────────────────────────────────────
step "wasm-floor"
cargo build -p ipe-runtime-rust --target wasm32-unknown-unknown
cargo build -p ipe-runtime-rust --target wasm32-unknown-unknown --features json
cargo build -p ipe-runtime-rust --target wasm32-unknown-unknown --no-default-features --features wasm-client,debugger

printf '\n=== fast-gate: all gates passed ===\n'
