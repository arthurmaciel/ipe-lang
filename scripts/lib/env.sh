# shellcheck shell=bash
# scripts/lib/env.sh — SINGLE SOURCE OF TRUTH for the ipê examples-sweep command
# env. SOURCE this (never execute it): `source "$(dirname "$0")/lib/env.sh"`.
#
# PORTED from ../sky/runtime-rust/scripts/lib/env.sh and ADAPTED for this repo:
# the compiler here is `skyc` (a Rust cargo workspace), NOT the Haskell `sky`.
# There is no GHC/cabal, no `sky-out/sky`; the binary is built by cargo and lives
# in the (possibly global) cargo target dir. This file defines REPO + SKYC_BIN and
# does NOT cd (callers `cd "$REPO"` themselves so the failure path stays theirs).
#
# It is idempotent: safe to source even when the caller has already cd'd into the
# repo or pre-set CARGO_TARGET_DIR / RUSTC_WRAPPER / SKYC_BIN (all `${VAR:-…}`).

# ── PATH: prepend the canonical dev dirs, PRESERVE the inherited PATH ────────
# cargo (and go, kept for the PHASED Go≡Rust equiv step) resolve from their
# canonical local dirs first. The trailing `$PATH` is LOAD-BEARING on CI:
# GitHub's setup-go / setup-node put `go` / `node` / `curl` on the runner PATH at
# non-canonical locations — clobbering it would abort the sweep at its
# `command -v go` / `curl` preflight.
export PATH="$HOME/.cargo/bin:/usr/local/go/bin:/usr/local/bin:/usr/bin:/bin:$PATH"

# ── Shared cargo target + sccache + CARGO_INCREMENTAL=0 ─────────────────────
# A shared CARGO_TARGET_DIR compiles the heavy deps (axum/tokio/serde/sqlx/…)
# ONCE and persists across each example's `rm -rf sky-out`. This repo's global
# ~/.cargo/config.toml already pins `target-dir = ~/.cache/sky-rust-target`, so
# this default AGREES with where a bare `cargo build` of the workspace lands
# skyc — the same dir the sweep's per-example `cargo build` reuses. Override
# CARGO_TARGET_DIR to relocate; we honour a pre-existing value.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/ipe/ipe-target}"
mkdir -p "$CARGO_TARGET_DIR" || {
  echo "env.sh: could not create CARGO_TARGET_DIR='$CARGO_TARGET_DIR' (perms/ENOSPC?)" >&2
  return 1 2>/dev/null || exit 1
}

# sccache (RUSTC_WRAPPER) caches each rustc by content hash — the big LOCAL win,
# coupled to CARGO_INCREMENTAL=0 (sccache caches NOTHING with incremental=true).
# SKY_NO_SCCACHE=1 force-disables it; CI sets that (GitHub retired the v1
# Actions-Cache API sccache's GHA backend depends on) and relies on actions/cache
# of CARGO_TARGET_DIR + ~/.cargo instead — leaving CARGO_INCREMENTAL at cargo's
# default so a persisted target dir does incremental rebuilds.
if [ -z "${SKY_NO_SCCACHE:-}" ] && command -v sccache >/dev/null 2>&1; then
    export RUSTC_WRAPPER="${RUSTC_WRAPPER:-sccache}"
    export CARGO_INCREMENTAL=0
fi

# ── Repo-root detection → REPO ───────────────────────────────────────────────
# Honour an explicit SKY_REPO; else detect via this file's location (works from
# any subdir of any clone). Don't cd — that's the caller's. Only SKY_REPO seeds
# the root; REPO is a common var other tooling exports, so trusting an inherited
# value would poison every "$REPO/…" path.
REPO="${SKY_REPO:-}"
[ -z "$REPO" ] && [ -f "$PWD/scripts/lib/examples.sh" ] && REPO="$PWD"
if [ -z "$REPO" ]; then
  _env_sh_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd)"
  if [ -n "$_env_sh_dir" ]; then
    REPO="$(git -C "$_env_sh_dir" rev-parse --show-toplevel 2>/dev/null)"
  fi
  unset _env_sh_dir
fi
if [ -z "$REPO" ]; then
  echo "env.sh: could not locate repo root (set SKY_REPO to override)" >&2
  return 1 2>/dev/null || exit 1
fi
export REPO

# ── SKYC_BIN detection ───────────────────────────────────────────────────────
# The compiler binary is produced by `cargo build [--release] -p skyc`. Because
# this repo's ~/.cargo/config.toml pins a GLOBAL target-dir, the binary lands in
# $CARGO_TARGET_DIR/{release,debug}/skyc — NOT $REPO/target — so probe the shared
# target FIRST, then the in-repo target/ (the layout on a checkout WITHOUT a
# global target-dir), then PATH. Release is preferred over debug (faster sweep).
# SKYC_BIN honoured verbatim if the caller pre-set it.
if [ -z "${SKYC_BIN:-}" ]; then
  for _cand in \
    "$CARGO_TARGET_DIR/release/skyc" \
    "$CARGO_TARGET_DIR/debug/skyc" \
    "$REPO/target/release/skyc" \
    "$REPO/target/debug/skyc"; do
    if [ -x "$_cand" ]; then SKYC_BIN="$_cand"; break; fi
  done
  # Last resort: a skyc on PATH.
  [ -z "${SKYC_BIN:-}" ] && command -v skyc >/dev/null 2>&1 && SKYC_BIN="$(command -v skyc)"
  unset _cand
fi
export SKYC_BIN="${SKYC_BIN:-$CARGO_TARGET_DIR/release/skyc}"

# ── Vendored runtime dir (skyc --runtime) ────────────────────────────────────
# skyc's build vendors the runtime module tree into each emitted crate. Left
# UNSET it auto-resolves by walking up to `$REPO/runtime/src/sky_runtime`
# (resolve_runtime() in crates/skyc/src/lib.rs). We export the explicit path so
# the sweep is independent of the invocation CWD; callers may override.
export SKY_RUNTIME_DIR="${SKY_RUNTIME_DIR:-$REPO/runtime/src/sky_runtime}"
