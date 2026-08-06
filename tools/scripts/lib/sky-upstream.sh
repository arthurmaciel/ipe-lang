# shellcheck shell=bash
# tools/scripts/lib/sky-upstream.sh — install the upstream Sky compiler for LIVE
# reference comparison. SOURCE this (never execute).
#
# The sweep's parity proof is "does ipe build+run the real upstream examples?".
# When SKY comparison is on, it goes one step further: run the SAME upstream
# example through the released upstream `sky` compiler and compare its output to
# ipe's, so a behavioural divergence surfaces against the live reference rather
# than a frozen cached oracle.
#
# Upstream Sky is a Haskell compiler that lowers to Go, so `sky run` needs Go on
# PATH. We install the RELEASED binary (no GHC/cabal build) from the
# anzellai/sky GitHub release — the sky-lang.org install endpoint is not relied
# on (it has been observed returning Cloudflare 522). Provides:
#   sky_upstream_bin        -> path to an installed `sky` binary, installing on demand. empty+rc1 if unavailable.
#   sky_run_capture <dir> <outfile> -> `sky run` the example in <dir>, capture stdout to <outfile>. rc = sky's.

: "${REPO:?sky-upstream.sh: REPO must be set (source lib/env.sh first)}"

# Pin the reference release. A bare "latest" would let an upstream release drift
# silently change the comparison; pin it and bump deliberately.
SKY_UPSTREAM_RELEASE="${SKY_UPSTREAM_RELEASE:-v0.17.10}"
SKY_UPSTREAM_CACHE="${SKY_UPSTREAM_CACHE:-$HOME/.cache/ipe/sky-upstream}"

# ── _sky_release_asset: the release asset name for this host ──────────────────
_sky_release_asset() {
  local os arch
  case "$(uname -s)" in
    Linux)  os=linux ;;
    Darwin) os=darwin ;;
    *) return 1 ;;
  esac
  case "$(uname -m)" in
    x86_64|amd64) arch=x64 ;;
    arm64|aarch64) arch=arm64 ;;
    *) return 1 ;;
  esac
  # Upstream ships x64 for linux/windows and arm64 for darwin/linux.
  printf 'sky-%s-%s.tar.gz\n' "$os" "$arch"
}

# ── sky_upstream_bin: install-on-demand, print the binary path ────────────────
# Idempotent: a cached binary is reused. Downloads the pinned release asset via
# the GitHub API (`gh` if authenticated, else curl to the public release URL),
# extracts it, and locates the `sky*` executable. Empty output + rc 1 when the
# host is unsupported or the download fails (caller degrades to no-compare, never
# a false red).
sky_upstream_bin() {
  local bin
  # The release tarball ships TWO binaries: the compiler `sky-<os>-<arch>` and
  # the FFI inspector `sky-ffi-inspect-sky-<os>-<arch>`. Select the COMPILER —
  # exclude the inspector, which would emit FFI-inspection JSON, not a compile.
  bin="$(find "$SKY_UPSTREAM_CACHE" -maxdepth 1 -type f -name 'sky-*-*' \
         ! -name 'sky-ffi-inspect-*' ! -name '*.tar.gz' ! -name '*.zip' 2>/dev/null | head -1)"
  if [ -n "$bin" ] && [ -x "$bin" ]; then printf '%s\n' "$bin"; return 0; fi

  local asset; asset="$(_sky_release_asset)" || { echo "sky-upstream: unsupported host" >&2; return 1; }
  mkdir -p "$SKY_UPSTREAM_CACHE"
  local tgz="$SKY_UPSTREAM_CACHE/$asset"
  if command -v gh >/dev/null 2>&1 && \
     gh release download "$SKY_UPSTREAM_RELEASE" -R anzellai/sky -p "$asset" -D "$SKY_UPSTREAM_CACHE" --clobber 2>/dev/null; then
    :
  else
    local url="https://github.com/anzellai/sky/releases/download/$SKY_UPSTREAM_RELEASE/$asset"
    command -v curl >/dev/null 2>&1 || { echo "sky-upstream: no gh and no curl" >&2; return 1; }
    curl -fsSL --max-time 180 "$url" -o "$tgz" 2>/dev/null || { echo "sky-upstream: download failed ($url)" >&2; return 1; }
  fi
  tar -xzf "$tgz" -C "$SKY_UPSTREAM_CACHE" 2>/dev/null || { echo "sky-upstream: extract failed" >&2; return 1; }
  # The release tarball ships TWO binaries: the compiler `sky-<os>-<arch>` and
  # the FFI inspector `sky-ffi-inspect-sky-<os>-<arch>`. Select the COMPILER —
  # exclude the inspector, which would emit FFI-inspection JSON, not a compile.
  bin="$(find "$SKY_UPSTREAM_CACHE" -maxdepth 1 -type f -name 'sky-*-*' \
         ! -name 'sky-ffi-inspect-*' ! -name '*.tar.gz' ! -name '*.zip' 2>/dev/null | head -1)"
  [ -n "$bin" ] || { echo "sky-upstream: no sky binary in release asset" >&2; return 1; }
  chmod +x "$bin"
  printf '%s\n' "$bin"
}

# ── sky_run_capture <dir> <outfile>: run the upstream example, capture stdout ─
# `sky run` in <dir> (which holds the RAW upstream .sky tree + sky.toml). Its
# stdout goes to <outfile>. `sky run` compiles to Go and executes, so Go must be
# on PATH; a missing Go is reported (rc 3) so the caller degrades to no-compare.
sky_run_capture() {
  local dir="$1" out="$2" bin
  bin="$(sky_upstream_bin)" || return 2
  command -v go >/dev/null 2>&1 || { echo "sky-upstream: go not on PATH (sky lowers to Go)" >&2; return 3; }
  ( cd "$dir" && timeout "${SKY_RUN_TIMEOUT:-180}" "$bin" run ) >"$out" 2>&1
}
