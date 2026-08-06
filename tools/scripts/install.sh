#!/bin/sh
# Ipê installer — detects your platform, downloads the matching release binary,
# and installs `ipe` (+ `ipe-ffi-inspector`) to a bin dir on your PATH.
#
#   curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/tools/scripts/install.sh | sh
#
# Overrides:  IPE_VERSION=v0.1.0  IPE_INSTALL_DIR=$HOME/.local/bin  sh install.sh
set -eu

REPO="arthurmaciel/ipe-lang"
INSTALL_DIR="${IPE_INSTALL_DIR:-$HOME/.local/bin}"

# ── Palette ──────────────────────────────────────────────────────────────────
# Mirror the CLI (style.rs): a soft Ipê-amarelo (256-colour 222) for the banner,
# a light (bright) yellow (ANSI 93) for a running stage, a soft green (114) for
# success, a plain red (31) for failure, a mid grey (244) for dim hints. Colour
# only when stderr is a terminal and NO_COLOR is unset (per https://no-color.org).
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
  C_YELLOW="$(printf '\033[38;5;222m')"
  C_LYELLOW="$(printf '\033[93m')"
  C_DIM="$(printf '\033[38;5;244m')"
  C_GREEN="$(printf '\033[38;5;114m')"
  C_RED="$(printf '\033[31m')"
  C_BOLD="$(printf '\033[1m')"
  C_RESET="$(printf '\033[0m')"
  IS_TTY=1
else
  C_YELLOW=''; C_LYELLOW=''; C_DIM=''; C_GREEN=''; C_RED=''; C_BOLD=''; C_RESET=''
  IS_TTY=0
fi

# ── Stage progress ───────────────────────────────────────────────────────────
# The streamlined per-stage shape shared with the CLI (src/ipe-cli/src/progress.rs):
# while a stage runs it is one line — a light-yellow spinner + label; on success
# the SAME line is rewritten (carriage return) to a light-green ✓ + message; on
# failure to a light-red ✗ + message. Off a terminal (pipe / CI / NO_COLOR) each
# stage is one plain flush-left line with no spinner, no rewrite, and no ANSI, so
# `curl … | sh` logs stay clean. All chatter goes to stderr so stdout stays clean.
#
# The label of the stage currently in flight, so an outcome can clear a line at
# least as wide as it and leave no tail behind.
STAGE_LABEL=''

# stage_start LABEL — paint the running line (spinner frame 0 + LABEL in light
# yellow) with no trailing newline on a terminal, so the outcome overwrites it.
stage_start() {
  STAGE_LABEL="$1"
  if [ "$IS_TTY" = 1 ]; then
    printf '\r  %s⠋%s %s%s%s\033[0K' \
      "$C_LYELLOW" "$C_RESET" "$C_LYELLOW" "$STAGE_LABEL" "$C_RESET" >&2
  else
    printf '  %s\n' "$STAGE_LABEL" >&2
  fi
}

# stage_ok MSG — settle the running stage as success: rewrite the line to a
# light-green ✓ and MSG (terminal), or emit one plain line (non-terminal).
stage_ok() {
  if [ "$IS_TTY" = 1 ]; then
    printf '\r  %s✓%s %s%s%s\033[0K\n' \
      "$C_GREEN" "$C_RESET" "$C_GREEN" "$1" "$C_RESET" >&2
  else
    printf '  ✓ %s\n' "$1" >&2
  fi
  STAGE_LABEL=''
}

# stage_fail MSG — settle the running stage as failure: rewrite the line to a
# light-red ✗ and MSG (terminal), or emit one plain line (non-terminal). Whether
# the caller then stops or continues is its own decision; this only renders.
stage_fail() {
  if [ "$IS_TTY" = 1 ]; then
    printf '\r  %s✗%s %s%s%s\033[0K\n' \
      "$C_RED" "$C_RESET" "$C_RED" "$1" "$C_RESET" >&2
  else
    printf '  ✗ %s\n' "$1" >&2
  fi
  STAGE_LABEL=''
}

# info — a dimmed, deeper-indented sub-note beneath a stage (a soft skip, a
# secondary fact). Never a stage outcome itself. If a stage is still running on
# a terminal, settle its line first (a soft skip, in dim) so the note lands on
# its own line rather than overwriting the spinner.
info() {
  if [ -n "$STAGE_LABEL" ] && [ "$IS_TTY" = 1 ]; then
    printf '\r  %s•%s %s%s%s\033[0K\n' \
      "$C_DIM" "$C_RESET" "$C_DIM" "$STAGE_LABEL" "$C_RESET" >&2
    STAGE_LABEL=''
  fi
  printf '    %s%s%s\n' "$C_DIM" "$1" "$C_RESET" >&2
}

# die MESSAGE — settle any running stage as a failure, then exit non-zero.
# Mirrors the CLI's failure shape: the actionable message rides the ✗ line.
die() {
  if [ -n "$STAGE_LABEL" ]; then
    stage_fail "$1"
  else
    printf '\n  %s✗%s %s%s%s\n' "$C_RED" "$C_RESET" "$C_RED" "$1" "$C_RESET" >&2
  fi
  exit 1
}

# die_no_prebuilt TAG PLAT CPU — exits 2, a distinct code the `ipe upgrade`
# wrapper uses to show the "still being generated" message instead of generic
# failure text. Exit 2 (not 1) signals "no prebuilt binary" specifically.
die_no_prebuilt() {
  _tag="$1"; _plat="$2"; _cpu="$3"
  printf '\n  %s%s%s No prebuilt binary for %s on %s-%s.\n' \
    "$C_BOLD" "$C_RED" "$C_RESET" "$_tag" "$_plat" "$_cpu" >&2
  printf '      Possibly the binaries for that version are still being generated.\n' >&2
  printf '      If you prefer, build from source:\n' >&2
  printf '          cargo install --git https://github.com/%s ipe\n' "$REPO" >&2
  exit 2
}

banner() {
  printf '\n  %s%sIpê language%s %s- %s%s\n\n' \
    "$C_BOLD" "$C_YELLOW" "$C_RESET" "$C_DIM" "$1" "$C_RESET" >&2
}

# ── Gate the install dir once, at the boundary ───────────────────────────────
# INSTALL_DIR ends up written verbatim into the ~/.ipe env files, which the
# user's shell later sources (executes). Parse, don't validate: reject any path
# carrying shell metacharacters here, ONCE, so every downstream sink can treat
# it as a safe literal. Only ordinary path characters are allowed; a quote,
# backtick, `$`, or whitespace could inject shell code into a sourced file, so we
# refuse rather than try to quote it perfectly at each use. Backslash and the
# drive-letter colon ARE allowed — Windows paths (`D:\...`) need them, and both
# are inert inside the single-quoted `export PATH='…'` the env file emits.
case "$INSTALL_DIR" in
  *[!A-Za-z0-9._/+@:\\-]*)
    die "The install directory contains unsupported characters: $INSTALL_DIR" ;;
  '') die "The install directory is empty." ;;
esac

# ── Detect platform → the release artifact name ──────────────────────────────
os="$(uname -s)"; arch="$(uname -m)"
case "$os" in
  Linux)   plat=linux ;;
  Darwin)  plat=darwin ;;
  FreeBSD) plat=freebsd ;;
  MINGW*|MSYS*|CYGWIN*) plat=windows ;;
  *) die "Unsupported OS: $os" ;;
esac
case "$arch" in
  x86_64|amd64) cpu=x64 ;;
  arm64|aarch64) cpu=arm64 ;;
  *) die "Unsupported architecture: $arch" ;;
esac

# Published matrix (see .github/workflows/release.yml). Reject combos we don't ship.
case "$plat-$cpu" in
  linux-x64|linux-arm64|darwin-arm64|freebsd-x64|windows-x64) : ;;
  *) die "No prebuilt binary for $plat-$cpu — build from source: https://github.com/$REPO" ;;
esac
artifact="ipe-$plat-$cpu"
[ "$plat" = windows ] && ext=zip || ext=tar.gz

# ── Resolve version (default: latest release tag) ────────────────────────────
# Fetch the whole API response into a variable FIRST, then parse it. Piping curl
# straight into `grep -m1`/`head` makes the reader close the pipe early, curl
# hits EPIPE mid-write, and you get `curl: (23) Failed writing body`. Capturing
# the body in full sidesteps SIGPIPE entirely.
if [ -n "${IPE_VERSION:-}" ]; then
  tag="$IPE_VERSION"
  banner "$tag"
else
  banner "latest"
  stage_start "Resolving the latest release…"
  resp="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest")" \
    || die "Could not reach GitHub to resolve the latest release (set IPE_VERSION=vX.Y.Z)."
  # Grep over the captured string — no pipe from curl, so no early-close.
  tag="$(printf '%s\n' "$resp" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
  [ -n "$tag" ] || die "Could not parse the latest release tag (set IPE_VERSION=vX.Y.Z)."
  stage_ok "Found $tag."
fi

# Display version: the release tag may carry an `ipe-` prefix (e.g. ipe-v0.1.2).
# Strip it for the human-facing "vX.Y.Z" while keeping $tag for URLs.
ver="${tag#ipe-}"

base="https://github.com/$REPO/releases/download/$tag"
url="$base/$artifact.$ext"

# ── Check prebuilt binary availability ──────────────────────────────
stage_start "Checking for prebuilt binaries…"
have_bin=0
if curl -fsSL -o /dev/null -I --max-time 10 "$url" 2>/dev/null; then
  have_bin=1
  stage_ok "Prebuilt binary available for $plat-$cpu."
else
  info "No prebuilt binary for $tag on $plat-$cpu."
  if [ -n "${IPE_VERSION:-}" ] && [ "$IS_TTY" = 1 ] && [ -r /dev/tty ]; then
    printf '\n    %sNo prebuilt ipe %s binary for %s-%s.%s\n' \
      "$C_BOLD" "$ver" "$plat" "$cpu" "$C_RESET" >&2
    printf '    Install the %slatest%s release instead? [Y/n] ' \
      "$C_BOLD" "$C_RESET" >&2
    ans=''
    if IFS= read -r ans < /dev/tty 2>/dev/null; then
      case "$ans" in
        ''|[Yy]|[Yy][Ee][Ss])
          stage_start "Resolving the latest release…"
          resp="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest")" \
            || die "Could not reach GitHub."
          tag="$(printf '%s\n' "$resp" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
          [ -n "$tag" ] || die "Could not parse latest release tag."
          ver="${tag#ipe-}"
          base="https://github.com/$REPO/releases/download/$tag"
          url="$base/$artifact.$ext"
          if curl -fsSL -o /dev/null -I --max-time 10 "$url" 2>/dev/null; then
            have_bin=1
            stage_ok "Prebuilt binary available for $plat-$cpu."
          else
            info "Latest release also has no binary for $plat-$cpu."
          fi
          ;;
      esac
    fi
  fi
fi

# ── Minimum Rust version (for building from source) ─────────────────
# Kept in sync with the workspace Cargo.toml: edition = "2024" requires
# Rust >= 1.85 (stable since 2025-02-20).
MIN_CARGO_VERSION="1.85.0"

# version_gte MAJ MIN PATCH MAJ MIN PATCH — true when A >= B.
version_gte() {
  a1="$1"; a2="$2"; a3="$3"
  b1="$4"; b2="$5"; b3="$6"
  [ "$a1" -gt "$b1" ] && return 0
  [ "$a1" -lt "$b1" ] && return 1
  [ "$a2" -gt "$b2" ] && return 0
  [ "$a2" -lt "$b2" ] && return 1
  [ "$a3" -ge "$b3" ]
}

# ── Check Rust toolchain ────────────────────────────────────────────
stage_start "Checking Rust toolchain…"
cargo_ok=0
if command -v cargo >/dev/null 2>&1; then
  cargo_ver_raw="$(cargo version 2>/dev/null | cut -d' ' -f2)"
  cargo_ver="${cargo_ver_raw%%-*}"
  cargo_major="$(printf '%s\n' "$cargo_ver" | cut -d. -f1)"
  cargo_minor="$(printf '%s\n' "$cargo_ver" | cut -d. -f2)"
  cargo_patch="$(printf '%s\n' "$cargo_ver" | cut -d. -f3)"
  cargo_patch="${cargo_patch:-0}"
  min_major="$(printf '%s\n' "$MIN_CARGO_VERSION" | cut -d. -f1)"
  min_minor="$(printf '%s\n' "$MIN_CARGO_VERSION" | cut -d. -f2)"
  min_patch="$(printf '%s\n' "$MIN_CARGO_VERSION" | cut -d. -f3)"
  if version_gte "$cargo_major" "$cargo_minor" "$cargo_patch" \
                  "$min_major" "$min_minor" "$min_patch"; then
    cargo_ok=1
    stage_ok "Found cargo $cargo_ver_raw (>= $MIN_CARGO_VERSION)."
  else
    info "Found cargo $cargo_ver_raw (< required $MIN_CARGO_VERSION)."
  fi
else
  info "Rust is not installed."
  if [ -f "$HOME/.cargo/env" ] && [ "$IS_TTY" = 1 ] && [ -r /dev/tty ]; then
    printf '\n    %sRust seems installed but not on your %sPATH%s.%s\n' \
      "$C_BOLD" "$C_RESET" "$C_BOLD" "$C_RESET" >&2
    printf '    Source %s~/.cargo/env%s now? [Y/n] ' \
      "$C_BOLD" "$C_RESET" >&2
    ans=''
    if IFS= read -r ans < /dev/tty 2>/dev/null; then
      case "$ans" in
        ''|[Yy]|[Yy][Ee][Ss])
          # shellcheck source=/dev/null
          . "$HOME/.cargo/env"
          if command -v cargo >/dev/null 2>&1; then
            cargo_ok=1
            stage_ok "Rust now available ($(cargo version | cut -d' ' -f2))."
          fi
          ;;
      esac
    fi
  fi
  if [ "$cargo_ok" != 1 ]; then
    case "$plat" in
      linux|darwin|freebsd)
        rustup_cmd="curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        ;;
      windows)
        rustup_url="https://win.rustup.rs/x86_64"
        ;;
    esac
    if [ "$IS_TTY" = 1 ] && [ -r /dev/tty ]; then
      printf '\n    %sRust is not installed.%s\n' "$C_BOLD" "$C_RESET" >&2
      case "$plat" in
        linux|darwin|freebsd)
          printf '    Install Rust via %srustup%s? [Y/n] ' \
            "$C_BOLD" "$C_RESET" >&2
          ;;
        windows)
          printf '    Download the Rust installer from:\n      %s%s%s\n' \
            "$C_DIM" "$rustup_url" "$C_RESET" >&2
          printf '    Open the link and run the installer? [Y/n] ' >&2
          ;;
      esac
      ans=''
      if IFS= read -r ans < /dev/tty 2>/dev/null; then
        case "$ans" in
          ''|[Yy]|[Yy][Ee][Ss])
            case "$plat" in
              linux|darwin|freebsd)
                stage_start "Installing Rust via rustup…"
                if eval "$rustup_cmd" -y 2>/dev/null; then
                  if [ -f "$HOME/.cargo/env" ]; then
                    # shellcheck source=/dev/null
                    . "$HOME/.cargo/env"
                  fi
                  if command -v cargo >/dev/null 2>&1; then
                    cargo_ok=1
                    info "Rust installed ($(cargo version | cut -d' ' -f2))."
                  fi
                else
                  info "rustup exited non-zero — check https://rustup.rs for manual install."
                fi
                ;;
              windows)
                info "Download the installer from: $rustup_url"
                ;;
            esac
            ;;
        esac
      fi
    fi
    if [ "$cargo_ok" != 1 ]; then
      info "Install Rust later: https://rustup.rs"
    fi
  fi
fi

# ── Bail out when no binary is available ────────────────────────────
if [ "$have_bin" != 1 ]; then
  die_no_prebuilt "$tag" "$plat" "$cpu"
fi

# ── Download the binary with a friendly progress display ─────────────────────
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
pkg="$tmp/pkg.$ext"

stage_start "Downloading ipe $ver for ${plat}-${cpu}…"

# download_with_progress URL DEST
# On a TTY, run curl in the background writing to DEST, and animate a spinner +
# percent + bar + size + ETA by polling DEST's on-disk size against the known
# total. Off a TTY (pipe/CI), fall back to a couple of plain lines — no ANSI,
# no animation, no SIGPIPE.
download_with_progress() {
  dl_url="$1"; dl_dest="$2"

  # Total size (best-effort; 0 ⇒ unknown, bar degrades to a byte counter).
  # A one-byte range GET is more reliable than HEAD across GitHub's signed CDN
  # redirect: it always lands on a 206 carrying `Content-Range: bytes 0-0/TOTAL`.
  # Parse from a captured string, never a live pipe (no SIGPIPE).
  total=0
  range_resp="$(curl -fsSL -r 0-0 -D - -o /dev/null "$dl_url" 2>/dev/null || true)"
  if [ -n "$range_resp" ]; then
    total="$(printf '%s\n' "$range_resp" \
      | tr -d '\r' \
      | sed -n 's#^[Cc]ontent-[Rr]ange: *bytes [0-9]*-[0-9]*/\([0-9][0-9]*\).*#\1#p' \
      | tail -n1)"
    [ -n "$total" ] || total=0
  fi

  if [ "$IS_TTY" != 1 ]; then
    # Non-terminal: quiet download, single plain status line, no animation.
    curl -fsSL "$dl_url" -o "$dl_dest" \
      || die "Download failed: $dl_url"
    got="$(wc -c < "$dl_dest" 2>/dev/null || echo 0)"
    stage_ok "Downloaded $(human "$got")."
    return 0
  fi

  # Terminal: curl in background (its own subshell so `set -e` can't trip on the
  # spinner loop); poll DEST size for the animation. Pre-create DEST so the
  # poller's `< DEST` never hits a not-yet-opened file mid-race.
  : > "$dl_dest"
  ( curl -fsSL "$dl_url" -o "$dl_dest"; echo $? > "$tmp/curl.rc" ) &
  dl_pid=$!

  si=0
  start="$(date +%s 2>/dev/null || echo 0)"
  printf '\033[?25l' >&2   # hide cursor

  while kill -0 "$dl_pid" 2>/dev/null; do
    got="$(wc -c < "$dl_dest" 2>/dev/null || echo 0)"
    render_progress "$(spin_glyph "$si")" "$got" "$total" "$start"
    si=$(( (si + 1) % 10 ))
    sleep 0.1 2>/dev/null || sleep 1
  done
  wait "$dl_pid" 2>/dev/null || true

  printf '\033[?25h' >&2   # show cursor
  printf '\r\033[K' >&2    # clear the progress line

  rc="$(cat "$tmp/curl.rc" 2>/dev/null || echo 1)"
  [ "$rc" = 0 ] || die "Download failed: $dl_url"

  got="$(wc -c < "$dl_dest" 2>/dev/null || echo 0)"
  stage_ok "Downloaded $(human "$got")."
}

# spin_glyph IDX → the IDXth braille spinner frame (0-9). Selected by `case`
# rather than `cut -c`, which counts BYTES in a C locale and would slice a
# multi-byte UTF-8 frame into garbage.
spin_glyph() {
  case "$1" in
    0) printf '⠋' ;; 1) printf '⠙' ;; 2) printf '⠹' ;; 3) printf '⠸' ;;
    4) printf '⠼' ;; 5) printf '⠴' ;; 6) printf '⠦' ;; 7) printf '⠧' ;;
    8) printf '⠇' ;; *) printf '⠏' ;;
  esac
}

# render_progress GLYPH GOT TOTAL START — one animated line to stderr.
render_progress() {
  glyph="$1"; rp_got="$2"; rp_total="$3"; rp_start="$4"
  [ -n "$glyph" ] || glyph='*'

  now="$(date +%s 2>/dev/null || echo "$rp_start")"
  elapsed=$(( now - rp_start ))
  [ "$elapsed" -lt 0 ] && elapsed=0

  bar_w=24
  if [ "$rp_total" -gt 0 ] 2>/dev/null; then
    pct=$(( rp_got * 100 / rp_total ))
    [ "$pct" -gt 100 ] && pct=100
    filled=$(( rp_got * bar_w / rp_total ))
    [ "$filled" -gt "$bar_w" ] && filled="$bar_w"
    bar=''
    i=0
    while [ "$i" -lt "$bar_w" ]; do
      if [ "$i" -lt "$filled" ]; then bar="$bar#"; else bar="$bar-"; fi
      i=$(( i + 1 ))
    done
    # ETA from the running average rate.
    eta='--:--'
    if [ "$rp_got" -gt 0 ] && [ "$elapsed" -gt 0 ]; then
      rate=$(( rp_got / elapsed ))
      if [ "$rate" -gt 0 ]; then
        remain=$(( (rp_total - rp_got) / rate ))
        [ "$remain" -lt 0 ] && remain=0
        eta="$(fmt_eta "$remain")"
      fi
    fi
    printf '\r  %s%s%s  %s%3d%%%s [%s%s%s]  %s / %s  %sETA %s%s\033[K' \
      "$C_YELLOW" "$glyph" "$C_RESET" \
      "$C_BOLD" "$pct" "$C_RESET" \
      "$C_YELLOW" "$bar" "$C_RESET" \
      "$(human "$rp_got")" "$(human "$rp_total")" \
      "$C_DIM" "$eta" "$C_RESET" >&2
  else
    # Unknown total: spinner + downloaded bytes + elapsed.
    printf '\r  %s%s%s  %s downloaded  %s%ds elapsed%s\033[K' \
      "$C_YELLOW" "$glyph" "$C_RESET" \
      "$(human "$rp_got")" \
      "$C_DIM" "$elapsed" "$C_RESET" >&2
  fi
}

# human BYTES → e.g. "1.4 MB". Integer math only (POSIX sh has no floats).
human() {
  h_b="${1:-0}"
  if [ "$h_b" -lt 1024 ] 2>/dev/null; then
    printf '%d B' "$h_b"
  elif [ "$h_b" -lt 1048576 ]; then
    printf '%d.%d KB' "$(( h_b / 1024 ))" "$(( (h_b % 1024) * 10 / 1024 ))"
  else
    printf '%d.%d MB' "$(( h_b / 1048576 ))" "$(( (h_b % 1048576) * 10 / 1048576 ))"
  fi
}

# fmt_eta SECONDS → "M:SS" (or "H:MM:SS" past an hour).
fmt_eta() {
  e_s="${1:-0}"
  if [ "$e_s" -ge 3600 ]; then
    printf '%d:%02d:%02d' "$(( e_s / 3600 ))" "$(( (e_s % 3600) / 60 ))" "$(( e_s % 60 ))"
  else
    printf '%d:%02d' "$(( e_s / 60 ))" "$(( e_s % 60 ))"
  fi
}

download_with_progress "$url" "$pkg"

# ── Verify checksum (when the release ships SHA256SUMS) ───────────────────────
# Opportunistic: if the release publishes SHA256SUMS and we have a sha256 tool,
# verify the artifact. A present-but-mismatched sum is fatal; a missing sums
# file or missing tool is a soft skip (older releases, minimal environments).
stage_start "Verifying the download…"
sums="$(curl -fsSL "$base/SHA256SUMS" 2>/dev/null || true)"
if [ -n "$sums" ]; then
  want="$(printf '%s\n' "$sums" \
    | sed -n "s/^\\([0-9a-fA-F][0-9a-fA-F]*\\) [ *]*$artifact\\.$ext\$/\\1/p" \
    | head -n1)"
  if [ -n "$want" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      got_sum="$(sha256sum "$pkg" | cut -d' ' -f1)"
    elif command -v shasum >/dev/null 2>&1; then
      got_sum="$(shasum -a 256 "$pkg" | cut -d' ' -f1)"
    else
      got_sum=''
    fi
    if [ -z "$got_sum" ]; then
      info "No sha256 tool found — skipping checksum verification."
    elif [ "$got_sum" = "$want" ]; then
      stage_ok "Checksum verified."
    else
      die "Checksum mismatch for $artifact.$ext — refusing to install (expected $want, got $got_sum)."
    fi
  else
    info "No checksum listed for $artifact.$ext — skipping verification."
  fi
else
  info "This release ships no SHA256SUMS — skipping checksum verification."
fi

# ── Extract + install ────────────────────────────────────────────────────────
# Brace the name: on a non-UTF-8 `/bin/sh` (macOS bash in POSIX mode) an
# unbraced `$INSTALL_DIR` followed by the multibyte `…` slurps the ellipsis's
# first byte into the variable name → an "unbound variable" abort under set -u.
stage_start "Installing to ${INSTALL_DIR}…"
if [ "$ext" = zip ]; then
  # `unzip` isn't guaranteed (e.g. Git Bash on Windows); bsdtar (`tar`) reads
  # zips on Windows and macOS, so fall back to it.
  if command -v unzip >/dev/null 2>&1; then
    unzip -q "$pkg" -d "$tmp" || die "Could not unzip the download."
  else
    tar -xf "$pkg" -C "$tmp" || die "Could not extract the download."
  fi
else
  tar xzf "$pkg" -C "$tmp" || die "Could not extract the download."
fi
mkdir -p "$INSTALL_DIR"
installed=0
for b in ipe ipe-ffi-inspector; do
  [ "$plat" = windows ] && b="$b.exe"
  if [ -f "$tmp/$b" ]; then
    install -m 0755 "$tmp/$b" "$INSTALL_DIR/$b" 2>/dev/null \
      || { cp "$tmp/$b" "$INSTALL_DIR/$b"; chmod +x "$INSTALL_DIR/$b"; }
    installed=$(( installed + 1 ))
  fi
done
[ "$installed" -gt 0 ] || die "The archive contained no ipe binaries."
stage_ok "Installed ipe $ver to $INSTALL_DIR/ipe"

# ── PATH setup ────────────────────────────────────────────────────────────────
# ipe is installed, but a bin dir is only useful once it is on PATH. We make
# that painless without ever silently editing a shell file. Following rustup, we
# own a managed env file under ~/.ipe (env for POSIX shells, env.fish for fish)
# that exports INSTALL_DIR onto PATH, and we add ONE attributable line to the
# login shell's rc that sources it. The user sees the exact file and line, and
# consents on the real terminal (/dev/tty — never the piped installer on stdin)
# before we touch a dotfile. Because the PATH mechanics live in our own file, a
# future update or uninstall rewrites or removes it cleanly, and the rc keeps a
# single stable `. "$HOME/.ipe/env"` line.

IPE_HOME="$HOME/.ipe"
ENV_POSIX="$IPE_HOME/env"
ENV_FISH="$IPE_HOME/env.fish"

# on_path — succeed when INSTALL_DIR is already a PATH entry.
on_path() {
  case ":${PATH:-}:" in
    *":$INSTALL_DIR:"*) return 0 ;;
    *) return 1 ;;
  esac
}

# canonical_home — $HOME with any symlinks and `..` resolved to a physical path,
# computed once. The prefix test below compares physical paths, so a link whose
# textual target begins with "$HOME/" but resolves elsewhere cannot slip past.
canonical_home="$(cd -P "$HOME" 2>/dev/null && pwd -P)" || canonical_home="$HOME"

# refuse_symlink_escape PATH — die if PATH is a symlink whose target resolves
# outside $HOME. A managed env file or dotfile must stay within the user's home;
# we never follow a link that would let us write elsewhere. The whole symlink
# chain and every `..` are resolved (readlink -f where available, else a
# physical-cd fallback) before the comparison, so a relative `../` target or a
# multi-hop chain cannot escape.
refuse_symlink_escape() {
  rse_path="$1"
  [ -L "$rse_path" ] || return 0

  # Resolve the link fully. readlink -f follows the chain and normalizes `..`;
  # the fallback resolves the parent physically and appends the basename, which
  # collapses a `..` that would otherwise escape.
  rse_real="$(readlink -f "$rse_path" 2>/dev/null || true)"
  if [ -z "$rse_real" ]; then
    rse_dir="$(dirname "$rse_path")"
    rse_base="$(basename "$rse_path")"
    rse_pdir="$(cd -P "$rse_dir" 2>/dev/null && pwd -P)" || rse_pdir="$rse_dir"
    rse_real="$rse_pdir/$rse_base"
  fi

  case "$rse_real/" in
    "$canonical_home"/*) return 0 ;;
    *) die "$rse_path is a symlink pointing outside your home directory — refusing to edit it." ;;
  esac
}

# resolve_shell_rc — set SH_NAME and RC_FILE (the login shell's startup file),
# RC_SOURCE_LINE (the one line we append to rc to source our env file), and
# PATH_NOW / SOURCE_NOW (the two commands the user can run in the current shell
# to activate PATH immediately — put ipe on PATH directly, or source the edited
# rc's env file).
resolve_shell_rc() {
  SH_NAME="$(basename "${SHELL:-sh}")"
  case "$SH_NAME" in
    zsh)  RC_FILE="${ZDOTDIR:-$HOME}/.zshrc" ;;
    bash)
      # Prefer an existing file; else .bash_profile on macOS (login shells),
      # .bashrc elsewhere.
      if   [ -f "$HOME/.bashrc" ];       then RC_FILE="$HOME/.bashrc"
      elif [ -f "$HOME/.bash_profile" ]; then RC_FILE="$HOME/.bash_profile"
      elif [ "$plat" = darwin ];         then RC_FILE="$HOME/.bash_profile"
      else RC_FILE="$HOME/.bashrc"; fi
      ;;
    fish) RC_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ;;
    ksh)  RC_FILE="$HOME/.kshrc" ;;
    *)    RC_FILE="$HOME/.profile" ;;
  esac
  if [ "$SH_NAME" = fish ]; then
    RC_SOURCE_LINE="source \"$ENV_FISH\""
    PATH_NOW="fish_add_path $INSTALL_DIR"
    SOURCE_NOW="source \"$ENV_FISH\""
  else
    RC_SOURCE_LINE=". \"$ENV_POSIX\""
    # shellcheck disable=SC2016  # $PATH must stay literal so it expands per-shell
    PATH_NOW="export PATH=\"$INSTALL_DIR:\$PATH\""
    SOURCE_NOW=". \"$ENV_POSIX\""
  fi
}

# write_env_files — (re)write our managed env files with the current PATH line.
# These are entirely ours, so overwriting them on every run keeps them correct
# after a move or version change. POSIX and fish both get one, so a user who
# switches shells still has the right file to source.
# write_managed FILE CONTENT — write CONTENT to FILE by rendering to a fresh
# temp file inside the validated $IPE_HOME and `mv`-ing it into place. `mv`
# replaces a symlink at FILE rather than following it, so a final-component
# symlink swapped in after our check cannot redirect the write outside $HOME
# (closing the check-then-write TOCTOU that a plain `>` redirect leaves open).
write_managed() {
  wm_dest="$1"; wm_body="$2"
  wm_tmp="$IPE_HOME/.env.$$.tmp"
  printf '%s' "$wm_body" > "$wm_tmp" || die "Could not write $wm_dest."
  mv -f "$wm_tmp" "$wm_dest" || { rm -f "$wm_tmp"; die "Could not write $wm_dest."; }
}

write_env_files() {
  refuse_symlink_escape "$IPE_HOME"
  mkdir -p "$IPE_HOME" 2>/dev/null || die "Could not create $IPE_HOME."
  refuse_symlink_escape "$ENV_POSIX"
  refuse_symlink_escape "$ENV_FISH"
  # INSTALL_DIR is gated to path characters at the boundary, so single-quoting
  # it here yields an inert literal even though the file is later sourced.
  # shellcheck disable=SC2016  # $PATH must stay literal for per-shell expansion
  write_env_files_posix="# Managed by the Ipê installer — puts ipe on your PATH.
case \":\${PATH}:\" in *\":$INSTALL_DIR:\"*) ;; *) export PATH='$INSTALL_DIR':\"\$PATH\" ;; esac
"
  write_env_files_fish="# Managed by the Ipê installer — puts ipe on your PATH.
if not contains '$INSTALL_DIR' \$PATH
    fish_add_path '$INSTALL_DIR'
end
"
  write_managed "$ENV_POSIX" "$write_env_files_posix"
  write_managed "$ENV_FISH" "$write_env_files_fish"
}

# activation_hint — the two commands to activate PATH in the CURRENT shell: run
# ipe onto PATH directly, or source the env file the edited rc now loads.
activation_hint() {
  printf '    To use %sipe%s right now, run:\n' "$C_BOLD" "$C_RESET" >&2
  printf '        %s%s%s\n' "$C_DIM" "$PATH_NOW" "$C_RESET" >&2
  printf '    or reload the updated startup file with:\n' >&2
  printf '        %s%s%s\n\n' "$C_DIM" "$SOURCE_NOW" "$C_RESET" >&2
}

# manual_path_hint — the do-it-yourself fallback when we did not edit the rc:
# source our env file (already written) from the shell's startup file yourself.
manual_path_hint() {
  printf '    Add ipe to your PATH by putting this line in %s%s%s:\n' \
    "$C_YELLOW" "$RC_FILE" "$C_RESET" >&2
  printf '        %s%s%s\n\n' "$C_DIM" "$RC_SOURCE_LINE" "$C_RESET" >&2
}

# persist_path — put INSTALL_DIR on PATH for good: write our managed env files,
# then append one attributable line to the login shell's rc that sources the
# right one, after showing the exact file + line and getting a yes on the real
# terminal. Idempotent (never double-adds), consented (never silent), and
# attributable (under a fixed marker). A non-interactive run prints the manual
# hint instead of editing anything.
persist_path() {
  resolve_shell_rc
  write_env_files

  # Already sourcing our env file from the rc — nothing to add.
  if [ -f "$RC_FILE" ] && grep -Fq "$RC_SOURCE_LINE" "$RC_FILE" 2>/dev/null; then
    stage_ok "ipe is on your PATH (via $RC_FILE)."
    return 0
  fi

  # No terminal to ask on (piped into a non-interactive shell, CI): never edit a
  # file unasked — print the manual hint and stop.
  if [ "$IS_TTY" != 1 ] || [ ! -r /dev/tty ]; then
    manual_path_hint
    return 0
  fi

  printf '\n  %s%s is not on your PATH yet.%s\n' "$C_BOLD" "$INSTALL_DIR" "$C_RESET" >&2
  printf '    Add it to %s%s%s so every shell finds %sipe%s?\n' \
    "$C_YELLOW" "$RC_FILE" "$C_RESET" "$C_BOLD" "$C_RESET" >&2
  printf '        %s%s%s\n' "$C_DIM" "$RC_SOURCE_LINE" "$C_RESET" >&2
  printf '    Update it now? [Y/n] ' >&2

  # A successful read of an empty line (bare ENTER) means yes; a failed read
  # (closed tty / EOF) is NOT consent — default to leaving the file untouched.
  ans=''
  if ! IFS= read -r ans < /dev/tty; then
    ans=n
  fi
  case "$ans" in
    ''|[Yy]|[Yy][Ee][Ss])
      mkdir -p "$(dirname "$RC_FILE")" 2>/dev/null || true
      refuse_symlink_escape "$RC_FILE"
      { printf '\n# Added by the Ipê installer\n'
        printf '%s\n' "$RC_SOURCE_LINE"
      } >> "$RC_FILE" || die "Could not write to $RC_FILE."
      stage_ok "Added ipe to your PATH (via $RC_FILE)."
      activation_hint
      ;;
    *)
      info "Left $RC_FILE untouched."
      manual_path_hint
      ;;
  esac
}

# ── Done + next steps ────────────────────────────────────────────────────────
if ! on_path; then
  # Put INSTALL_DIR on PATH for this run (live immediately when the installer is
  # sourced), then persist it for future shells.
  export PATH="$INSTALL_DIR:$PATH"
  persist_path
fi

# Success banner: the green word carries the good news; the footer mirrors the
# CLI's "report bugs" line (kept in sync with the style SSOT by a drift test).
printf '\n  Ipê %s was %ssuccessfully%s installed!\n' \
  "$ver" "$C_GREEN" "$C_RESET" >&2
printf '\n  If you find any bugs, please report them at https://github.com/%s/issues.\n\n' \
  "$REPO" >&2
