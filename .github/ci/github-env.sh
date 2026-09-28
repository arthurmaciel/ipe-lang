#!/usr/bin/env bash
# The one sanctioned writer of the runner env file: `github-env.sh KEY VALUE`
# appends `KEY=VALUE` for every later step of the job. KEY must be
# `CI_JOB_[A-Z0-9_]+` and an exact line of `github-env-allowlist.txt`; VALUE
# must be a single line (a newline would smuggle a second KEY=VALUE line past
# the allowlist). `verify-manifest.py`
# refuses every other textual reference to the env or path files in a workflow,
# and every call whose KEY is not a bare allowlisted literal.
set -euo pipefail

fail() {
  echo "github-env.sh: $*" >&2
  exit 1
}

[ "$#" -eq 2 ] || fail "usage: github-env.sh KEY VALUE (got $# arguments)"
key=$1
value=$2
allowlist="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/github-env-allowlist.txt"

[[ $key =~ ^CI_JOB_[A-Z0-9_]+$ ]] || fail "key '$key' is not a CI_JOB_-prefixed upper-case identifier"
grep -Fxq -- "$key" "$allowlist" || fail "key '$key' is not in $allowlist"
case $value in
  *$'\n'* | *$'\r'*) fail "value for '$key' spans more than one line" ;;
esac
[ -n "${GITHUB_ENV:-}" ] || fail "GITHUB_ENV is unset (not running under GitHub Actions)"

printf '%s=%s\n' "$key" "$value" >> "$GITHUB_ENV"
