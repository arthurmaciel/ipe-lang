#!/usr/bin/env bash
# Roll a group of jobs up into one required status context, failing closed.
#
# A job skipped because an upstream failed or was cancelled reports `skipped`,
# and branch protection counts `skipped` as passing — so an aggregator that
# accepted every `skipped` would turn an upstream failure into a green. Each
# argument names a job's `needs.<job>.result` and the role it plays:
#
#   must-pass:<job>=<result>  An upstream with no path filter (the path-filter
#                             job itself, quick-check). Only `success` passes.
#   may-skip:<job>=<result>   A path-filtered job. `success` passes; `skipped`
#                             passes vacuously. A `skipped` here is its own
#                             path filter, never an upstream failure, because
#                             every upstream it depends on is also listed and
#                             must pass (`verify-manifest.py` asserts the
#                             aggregator lists the full upstream closure).
#
# Any other result (`failure`, `cancelled`, empty, unknown) and any malformed
# argument fails the gate.
#
# Usage: gate-aggregate.sh <label> <role>:<job>=<result>...
set -euo pipefail

if [ "$#" -lt 2 ]; then
  echo "gate-aggregate: usage: gate-aggregate.sh <label> <role>:<job>=<result>..." >&2
  exit 1
fi

label="$1"
shift

failed=0
ran=0
for arg in "$@"; do
  role="${arg%%:*}"
  rest="${arg#*:}"
  job="${rest%%=*}"
  result="${rest#*=}"
  if [ "$role" = "$arg" ] || [ "$job" = "$rest" ] || [ -z "$job" ]; then
    echo "gate-aggregate: $label: malformed argument '$arg' (want <role>:<job>=<result>)" >&2
    exit 1
  fi
  case "$role:$result" in
    must-pass:success | may-skip:success)
      ran=1
      echo "$label: $job=success"
      ;;
    may-skip:skipped)
      echo "$label: $job=skipped (path filter)"
      ;;
    must-pass:* | may-skip:*)
      echo "$label FAILED: $job=${result:-<empty>} ($role)" >&2
      failed=1
      ;;
    *)
      echo "gate-aggregate: $label: unknown role '$role' in '$arg'" >&2
      exit 1
      ;;
  esac
done

if [ "$failed" -ne 0 ]; then
  exit 1
fi
if [ "$ran" -eq 0 ]; then
  echo "gate-aggregate: $label: no job reported success" >&2
  exit 1
fi
echo "$label: PASSED"
