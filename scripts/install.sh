#!/bin/sh
# Ipê installer — detects your platform, downloads the matching release binary,
# and installs `ipe` (+ `ipe-ffi-inspector`) to a bin dir on your PATH.
#
#   curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe/main/scripts/install.sh | sh
#
# Overrides:  IPE_VERSION=v0.1.0  IPE_INSTALL_DIR=$HOME/.local/bin  sh install.sh
set -eu

REPO="arthurmaciel/ipe"
INSTALL_DIR="${IPE_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '\033[1mipe-install:\033[0m %s\n' "$1" >&2; }
die() { printf '\033[31mipe-install error:\033[0m %s\n' "$1" >&2; exit 1; }

# ── Detect platform → the release artifact name ──────────────────────────────
os="$(uname -s)"; arch="$(uname -m)"
case "$os" in
  Linux)   plat=linux ;;
  Darwin)  plat=darwin ;;
  FreeBSD) plat=freebsd ;;
  MINGW*|MSYS*|CYGWIN*) plat=windows ;;
  *) die "unsupported OS: $os" ;;
esac
case "$arch" in
  x86_64|amd64) cpu=x64 ;;
  arm64|aarch64) cpu=arm64 ;;
  *) die "unsupported architecture: $arch" ;;
esac

# Published matrix (see .github/workflows/release.yml). Reject combos we don't ship.
case "$plat-$cpu" in
  linux-x64|linux-arm64|darwin-arm64|freebsd-x64|windows-x64) : ;;
  *) die "no prebuilt binary for $plat-$cpu — build from source: https://github.com/$REPO" ;;
esac
artifact="ipe-$plat-$cpu"
[ "$plat" = windows ] && ext=zip || ext=tar.gz

# ── Resolve version (default: latest release tag) ────────────────────────────
if [ -n "${IPE_VERSION:-}" ]; then
  tag="$IPE_VERSION"
else
  say "resolving latest release…"
  tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
         | grep -m1 '"tag_name"' | cut -d'"' -f4)"
  [ -n "$tag" ] || die "could not resolve the latest release tag (set IPE_VERSION=vX.Y.Z)"
fi

url="https://github.com/$REPO/releases/download/$tag/$artifact.$ext"
say "downloading $artifact ($tag)…"

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
curl -fSL --progress-bar "$url" -o "$tmp/pkg.$ext" || die "download failed: $url"

# ── Extract + install ────────────────────────────────────────────────────────
if [ "$ext" = zip ]; then unzip -q "$tmp/pkg.$ext" -d "$tmp"; else tar xzf "$tmp/pkg.$ext" -C "$tmp"; fi
mkdir -p "$INSTALL_DIR"
for b in ipe ipe-ffi-inspector; do
  [ "$plat" = windows ] && b="$b.exe"
  [ -f "$tmp/$b" ] && { install -m 0755 "$tmp/$b" "$INSTALL_DIR/$b" 2>/dev/null || { cp "$tmp/$b" "$INSTALL_DIR/$b"; chmod +x "$INSTALL_DIR/$b"; }; }
done

say "installed ipe $tag → $INSTALL_DIR/ipe"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) "$INSTALL_DIR/ipe" --version || true ;;
  *) say "add it to your PATH:  export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
