#!/usr/bin/env bash
# tools/scripts/install-sky-toolchain.sh — downloads + installs the sky binary.
#
# Downloads the prebuilt sky binary for the current OS/arch from the upstream
# GitHub release and places it on PATH (default: ~/.local/bin/sky).
#
# Usage:
#   install-sky-toolchain.sh [VERSION] [--dest DIR]
#
# Arguments:
#   VERSION    sky release tag, e.g. "v0.19.15" (default: latest upstream release)
#   --dest DIR installation directory (default: ~/.local/bin)
#
# After this script completes, `sky --version` should print the installed version.
set -uo pipefail

# ── Resolve version ───────────────────────────────────────────────────────────
# Default: resolve the latest release tag from the GitHub API so the parity gate
# always compares against the current Sky release rather than a stale pin.
# Pass an explicit VERSION argument to override (e.g. for bisecting a regression).
_resolve_latest_version() {
  if command -v gh >/dev/null 2>&1; then
    gh api repos/anzellai/sky/releases/latest --jq .tag_name 2>/dev/null && return
  fi
  # Fallback: curl + sed (no gh CLI needed in CI with a GITHUB_TOKEN).
  curl -fsSL \
    -H "Accept: application/vnd.github+json" \
    ${GITHUB_TOKEN:+-H "Authorization: Bearer $GITHUB_TOKEN"} \
    "https://api.github.com/repos/anzellai/sky/releases/latest" \
    2>/dev/null \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
    | head -1
}

# ── Argument parsing ─────────────────────────────────────────────────────────
VERSION="${1:-}"
DEST_DIR="$HOME/.local/bin"
shift 2>/dev/null || true
while [ $# -gt 0 ]; do
  case "$1" in
    --dest) DEST_DIR="$2"; shift 2 ;;
    *) echo "install-sky-toolchain: unknown option: $1" >&2; exit 1 ;;
  esac
done

if [ -z "$VERSION" ]; then
  echo "install-sky-toolchain: resolving latest Sky release from GitHub API..."
  VERSION="$(_resolve_latest_version)"
  if [ -z "$VERSION" ]; then
    echo "install-sky-toolchain: could not resolve latest Sky version from GitHub API" >&2
    echo "  set GITHUB_TOKEN or pass a VERSION argument explicitly" >&2
    exit 1
  fi
  echo "install-sky-toolchain: resolved latest release: $VERSION"
fi

# ── OS/arch detection ────────────────────────────────────────────────────────
OS=""
ARCH=""
case "$(uname -s)" in
  Linux)  OS="linux" ;;
  Darwin) OS="darwin" ;;
  *) echo "install-sky-toolchain: unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) ARCH="x64" ;;
  aarch64|arm64) ARCH="arm64" ;;
  *) echo "install-sky-toolchain: unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac

ASSET="sky-${OS}-${ARCH}.tar.gz"
URL="https://github.com/anzellai/sky/releases/download/${VERSION}/${ASSET}"

echo "install-sky-toolchain: fetching $VERSION ($OS/$ARCH)"
echo "  url:  $URL"
echo "  dest: $DEST_DIR/sky"

mkdir -p "$DEST_DIR"

TMP="$(mktemp -d "${TMPDIR:-/tmp}/sky-install.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

curl -fsSL --retry 5 --retry-delay 2 -o "$TMP/$ASSET" "$URL"
tar -xzf "$TMP/$ASSET" -C "$TMP"

# The tarball entries are named e.g. "sky-linux-x64" (no extension).
# Try exact "sky" first, then the platform-suffixed name.
SKY_BIN="$(find "$TMP" -maxdepth 2 -name 'sky' -type f | head -1)"
if [ -z "$SKY_BIN" ]; then
  SKY_BIN="$(find "$TMP" -maxdepth 2 -name "sky-${OS}-${ARCH}" -type f | head -1)"
fi
if [ -z "$SKY_BIN" ]; then
  echo "install-sky-toolchain: could not find sky binary in $ASSET" >&2
  ls "$TMP" >&2
  exit 1
fi

chmod +x "$SKY_BIN"
cp "$SKY_BIN" "$DEST_DIR/sky"

echo ""
echo "Installed: $("$DEST_DIR/sky" --version)"
echo "Path:      $DEST_DIR/sky"
echo ""
echo "If $DEST_DIR is not on your PATH, add it:"
echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
