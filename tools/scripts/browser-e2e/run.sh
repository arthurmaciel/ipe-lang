#!/usr/bin/env bash
# Run the geo-clipboard Playwright spec against a locally built binary.
#
# Usage:
#   bash tools/scripts/browser-e2e/run.sh [PORT]
#
# PORT defaults to 18080.  The script:
#   1. Builds the ipe compiler (cargo build -p ipe --release).
#   2. Compiles the geo-clipboard example via `ipe build`.
#   3. Cargo-builds the emitted Rust project.
#   4. Spawns the binary on PORT.
#   5. Runs the Playwright spec.
#   6. Kills the binary on exit.
#
# Prerequisites: node, npx, Rust toolchain, IPE_RUNTIME_DIR set (or run
# from the repo root where scripts/ipe-index wakeup auto-discovers it).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
SPEC_DIR="$(cd "$(dirname "$0")" && pwd)"
PORT="${1:-18080}"
OUT_DIR="${TMPDIR:-/tmp}/geo-clipboard-browser-e2e"

export IPE_GEO_CLIPBOARD_PORT="$PORT"
export IPE_RUNTIME_DIR="${IPE_RUNTIME_DIR:-$REPO_ROOT/src/runtime/rust/src}"

echo "==> Building ipe compiler..."
cargo build --release -p ipe --manifest-path "$REPO_ROOT/Cargo.toml"
IPE="$REPO_ROOT/target/release/ipe"

echo "==> Compiling geo-clipboard example..."
rm -rf "$OUT_DIR"
"$IPE" build "$REPO_ROOT/examples/shapes/web/geo-clipboard/package.ipe" --out "$OUT_DIR"

echo "==> Cargo-building emitted project..."
cargo build --release --manifest-path "$OUT_DIR/Cargo.toml"
BINARY="$OUT_DIR/target/release/geo-clipboard"
if [ ! -f "$BINARY" ]; then
  # Emitted binary name may differ; find it.
  BINARY="$(find "$OUT_DIR/target/release" -maxdepth 1 -type f -perm /111 ! -name "*.d" | head -1)"
fi

echo "==> Spawning geo-clipboard server on port $PORT..."
IPE_WEB_PORT="$PORT" IPE_CSRF=off IPE_CONSOLE_EMBED=off "$BINARY" &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

echo "==> Waiting for server readiness..."
for i in $(seq 1 20); do
  if curl -sf "http://127.0.0.1:$PORT/" >/dev/null 2>&1; then
    echo "   server ready (attempt $i)"
    break
  fi
  sleep 0.5
done

echo "==> Installing Playwright + Chromium (if not cached)..."
cd "$SPEC_DIR"
npm install --save-dev @playwright/test 2>/dev/null || true
npx playwright install chromium 2>/dev/null || true

echo "==> Running Playwright spec..."
npx playwright test --config playwright.config.mjs

echo "==> Done. Screenshots in $SPEC_DIR/artifacts/"
