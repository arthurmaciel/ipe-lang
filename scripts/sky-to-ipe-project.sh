#!/usr/bin/env bash
# scripts/sky-to-ipe-project.sh — convert ANY Sky project into an Ipê project.
#
# Generalises the example-mirror transform (scripts/lib/mirror.sh's
# sky_transform_one) into a converter for an arbitrary Sky project directory, not
# only the committed example set. Given a Sky project tree it applies every known
# transform — the rename-map token rewrite, the stdlib member moves, Pure
# desugaring, bare-stdlib qualifier injection, Cmd/Sub shape re-home, the
# .sky->.ipe + sky.toml->ipe.toml renames — AND translates the manifest's
# `["go.dependencies"]` into `[rust.dependencies]` via the reviewed Go->Rust
# crate map (scripts/lib/go-to-rust-crates.tsv).
#
# USAGE
#   scripts/sky-to-ipe-project.sh <sky-project-dir> [--out <dir>]
#
#   <sky-project-dir>  a directory holding a Sky project (its sky.toml + src/).
#                      A bare NAME with no slash is resolved against the committed
#                      raw snapshots (examples/sky/original/<NAME>) as a
#                      convenience, so `... 13-skyshop` converts that example.
#   --out <dir>        output directory. Default: ipe/<basename-of-input>/,
#                      relative to the current directory.
#
# The input tree is never modified; the transformed Ipê project is written to the
# output dir (created fresh). Exit: 0 ok · 1 bad input / transform failed · 2 setup.
set -uo pipefail

source "$(dirname "$0")/lib/env.sh"

usage() { sed -n '2,22p' "$0"; }

SRC=""
OUT=""
while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --out) OUT="${2:-}"; shift ;;
    -*) echo "sky-to-ipe-project: unknown flag '$1'" >&2; usage >&2; exit 2 ;;
    *) if [ -z "$SRC" ]; then SRC="$1"; else echo "sky-to-ipe-project: unexpected arg '$1'" >&2; exit 2; fi ;;
  esac
  shift
done

[ -n "$SRC" ] || { echo "sky-to-ipe-project: missing <sky-project-dir>" >&2; usage >&2; exit 2; }

# A bare name (no slash) is resolved against the committed raw snapshots as a
# convenience, so `sky-to-ipe-project.sh 13-skyshop` just works.
if [ ! -d "$SRC" ] && [[ "$SRC" != */* ]] && [ -d "$REPO/examples/sky/original/$SRC" ]; then
  SRC="$REPO/examples/sky/original/$SRC"
fi
[ -d "$SRC" ] || { echo "sky-to-ipe-project: '$SRC' is not a directory" >&2; exit 1; }

name="$(basename "$SRC")"
[ -n "$OUT" ] || OUT="ipe/$name"

# Enable the Go->Rust FFI dependency translation for the general converter (the
# committed-mirror regen leaves it off to keep its ports byte-stable).
export SKY_TRANSLATE_GO_DEPS=1

source "$(dirname "$0")/lib/mirror.sh"

# The converter is offline + deterministic: it transforms an already-materialised
# raw tree, exactly the `--check` path. sky_transform_one drops build artefacts,
# renames sources + manifest, runs the token rewrite + edits, and (with the flag
# set above) the go.dependencies -> rust.dependencies translation.
mkdir -p "$(dirname "$OUT")" || { echo "sky-to-ipe-project: cannot create parent of '$OUT'" >&2; exit 2; }
if sky_transform_one "$name" "$SRC" "$OUT"; then
  echo "sky-to-ipe-project: wrote Ipê project to '$OUT/' (from '$SRC')"
  exit 0
fi
echo "sky-to-ipe-project: conversion failed for '$name'" >&2
exit 1
