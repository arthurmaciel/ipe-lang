# shellcheck shell=bash
# tools/scripts/lib/sky-hash.sh — stable content hash for a committed
# examples/sky/original/<name>/ tree.
#
# Hash algorithm: SHA-256 over a deterministic byte stream built from every
# regular file under the tree, sorted by repo-relative path (LC_ALL=C):
#
#   <repo-relative-path>\n<sha256-of-file-contents>\n
#   (repeated for every file, sorted)
#
# Then SHA-256 the concatenated stream.  Order-independent across platforms,
# path-sensitive, content-sensitive.  An empty tree hashes the empty string.
#
# Provides:
#   sky_hash_one <name> [original-root]
#       Print the 64-char hex hash for example <name>.
#       <original-root> defaults to examples/sky/original.
#       Returns 1 when the directory is absent.
#
# Requires: sha256sum (Linux coreutils) OR shasum -a 256 (macOS).
# SOURCE this (never execute).

: "${REPO:?sky-hash.sh: REPO must be set (source lib/env.sh first)}"

# _sha256_file <path>  →  64-char hex digest
_sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -c1-64
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -c1-64
  else
    echo "sky-hash: sha256sum/shasum not found" >&2; return 1
  fi
}

# _sha256_stdin  →  64-char hex digest of stdin
_sha256_stdin() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum | cut -c1-64
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 | cut -c1-64
  else
    echo "sky-hash: sha256sum/shasum not found" >&2; return 1
  fi
}

# sky_hash_one <name> [original-root]
sky_hash_one() {
  local name="$1" root="${2:-$REPO/examples/sky/original}"
  local dir="$root/$name"
  if [ ! -d "$dir" ]; then
    echo "sky-hash: original/$name not found at $dir" >&2; return 1
  fi
  local f relpath digest stream=""
  while IFS= read -r f; do
    relpath="${f#"$dir"/}"
    digest="$(_sha256_file "$f")" || return 1
    stream="${stream}${relpath}"$'\n'"${digest}"$'\n'
  done < <(LC_ALL=C find "$dir" -type f | LC_ALL=C sort)
  printf '%s' "$stream" | _sha256_stdin
}
