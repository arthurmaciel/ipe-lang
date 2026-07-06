#!/usr/bin/env bash
# run.sh (scripts/progressive-development/) — gated, fresh-context autonomous burndown loop for the Ipê port.
#
# A ratchet-and-pawl only advances, never slips back. Each iteration spawns a
# FRESH `claude -p` process (no accumulated conversation) that reads durable
# state from disk (backlog + log + git), lands ONE backlog item as a green
# committed increment, or discards its work and logs why. The tree is always at
# a green commit between iterations. The loop itself is the OUTER safety harness
# (disk / mem-guard / budget / iteration cap / kill-switch / single-writer);
# the per-iteration playbook is scripts/progressive-development/prompt.md.
#
# Usage:
#   scripts/progressive-development/run.sh              # run the loop with defaults
#   scripts/progressive-development/run.sh --once       # run a single iteration (validate first!)
#   touch progressive-development.stop              # kill-switch: clean exit after current iter
#
# Config (env):
#   PROGDEV_MAX_ITERS   (20)     hard cap on iterations
#   PROGDEV_MIN_FREE_GB (15)     abort if free disk drops below this
#   PROGDEV_ITER_TIMEOUT(5400)   per-iteration wall-clock ceiling (s)
#   PROGDEV_COOLDOWN    (20)     seconds between iterations (keep < 300 to hold
#                                the prompt cache warm; see docs/architecture/progressive-development.md)
#   PROGDEV_BRANCH      (auto)   branch to land commits on (default progressive-development/run-<ts>)
#   MASTER_GATE_TARGET  (~/.cache/master-gate-target)  isolated gate target dir
#
# NOTE: pass a timestamp in via the environment for a deterministic branch name;
# the script stamps one if unset.
set -uo pipefail          # NOT -e: iteration failures are handled, not fatal.
cd "$(dirname "$0")/../.."

MAX_ITERS="${PROGDEV_MAX_ITERS:-20}"
MIN_FREE_GB="${PROGDEV_MIN_FREE_GB:-15}"
ITER_TIMEOUT="${PROGDEV_ITER_TIMEOUT:-5400}"
COOLDOWN="${PROGDEV_COOLDOWN:-20}"
STREAM="${PROGDEV_STREAM:-1}"   # DEFAULT ON (watch.sh renders); PROGDEV_STREAM=0 to disable
GATE_TARGET="${MASTER_GATE_TARGET:-$HOME/.cache/master-gate-target}"
STOP_FILE="progressive-development.stop"
PROMPT_FILE="scripts/progressive-development/prompt.md"
LOG="docs/architecture/progressive-development-log.md"
ESC="docs/architecture/progressive-development-escalations.md"
ONCE=0
[ "${1:-}" = "--once" ] && ONCE=1

log() { printf '%s | %s\n' "$(date -Is)" "$*"; }
die() { log "ABORT: $*"; exit 1; }

# ── preconditions (fail fast, before spawning anything) ────────────────────
command -v claude >/dev/null || die "claude CLI not found"
[ -f "$PROMPT_FILE" ] || die "missing $PROMPT_FILE"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not a git repo"
# Only TRACKED changes block a run — untracked scratch (incl. the loop's own
# log/escalation/iter artifacts) must not gate it (else the loop trips over its
# own output, as the second --once test did).
[ -z "$(git status --porcelain --untracked-files=no)" ] || die "tracked working-tree changes present — commit or stash first"
pgrep -f mem-guard.sh >/dev/null || die "mem-guard.sh not running — start it before a progressive-development run"
[ -f "$STOP_FILE" ] && die "kill-switch $STOP_FILE present — remove it to run"

# Single-writer guard: refuse to run if another progressive-development holds the lock.
LOCK=".progressive-development.lock"
if ! ( set -o noclobber; echo "$$" > "$LOCK" ) 2>/dev/null; then
    die "another progressive-development run holds $LOCK (pid $(cat "$LOCK" 2>/dev/null)) — one writer only"
fi
trap 'rm -f "$LOCK"; log "loop exit"' EXIT

# ── suppress CLAUDE.md via the CLI, inject the lean contract ─────────────────
# An auto-loaded CLAUDE.md CANNOT be "ignored" by a prompt instruction — it is
# in the fresh agent's context before it acts. So we stop it loading at the CLI:
#   --safe-mode  disables CLAUDE.md + skills + hooks (keeps normal OAuth/keychain
#                auth) — the robust default for a subscription login.
#   --bare       leaner (keeps skills, disables hooks + CLAUDE.md + auto-memory)
#                but needs ANTHROPIC_API_KEY (OAuth/keychain are not read).
# Either way the lean contract is injected with --append-system-prompt-file, and
# hooks-off means the stop-hook can't interfere with the loop. Override the whole
# flag set with PROGDEV_CLAUDE_ARGS.
#
# --permission-mode auto: the iteration runs bash/git/edits UNATTENDED via the
# auto-approval classifier, which still gates genuinely dangerous ops. NOT
# acceptEdits — that only auto-approves file edits and would STALL on the first
# `cargo`/`git` bash call (no human to approve in -p mode). If `auto` proves too
# conservative and blocks a routine command, add an allowlist, e.g. append
#   --allowedTools 'Bash(cargo *) Bash(git *) Bash(skyc *) Edit Write Read'
# via PROGDEV_CLAUDE_ARGS. `dontAsk`/`bypassPermissions` are more permissive but
# weaken the safety the guardrails depend on — avoid unless sandboxed.
CONTEXT="$(pwd)/scripts/progressive-development/context.md"
[ -f "$CONTEXT" ] || die "missing lean contract $CONTEXT"
# Flag set as an ARRAY — allowlist entries like 'Bash(cargo *)' contain spaces, so
# a word-split string would mangle them. Default = --safe-mode (no CLAUDE.md) +
# auto approval, PLUS an explicit allowlist so the gate (cargo), git commit/reset,
# skyc, and edits ALWAYS flow unattended. (The --once test wrote a full 8-file wire
# but exited with no verdict and never committed — the classifier alone was the
# suspected block on the gate/commit step; the allowlist removes that failure mode.)
if [ -n "${PROGDEV_CLAUDE_ARGS:-}" ]; then
    read -r -a CLAUDE_ARGS <<< "$PROGDEV_CLAUDE_ARGS"
else
    CLAUDE_ARGS=(--safe-mode --permission-mode auto
        --allowedTools 'Bash(cargo *)' 'Bash(git *)' 'Bash(skyc *)' 'Bash(touch *)'
                       'Bash(cat *)' 'Bash(ls *)' 'Bash(rg *)' 'Bash(sed *)' 'Bash(awk *)'
                       'Bash(mkdir *)' 'Bash(cp *)' 'Bash(mv *)' 'Bash(rm *)' 'Bash(df *)'
                       Edit Write Read Grep Glob)
fi
STOP_PATH="$(pwd)/$STOP_FILE"

# ── dedicated branch (human fast-forwards to master after reviewing the run) ─
BRANCH="${PROGDEV_BRANCH:-progressive-development/run-${PROGDEV_TS:-manual}}"
BASE="$(git rev-parse --abbrev-ref HEAD)"
git switch -c "$BRANCH" 2>/dev/null || git switch "$BRANCH" || die "cannot create branch $BRANCH"
mkdir -p "$(dirname "$LOG")"; touch "$LOG" "$ESC"
log "progressive-development start: branch=$BRANCH base=$BASE claude_args='${CLAUDE_ARGS[*]}' max_iters=$MAX_ITERS gate=$GATE_TARGET"

# ── the loop ───────────────────────────────────────────────────────────────
landed=0
for i in $(seq 1 "$MAX_ITERS"); do
    [ -f "$STOP_PATH" ] && { log "kill-switch tripped — clean exit"; break; }

    free_gb="$(df -BG --output=avail / | tail -1 | tr -dc '0-9')"
    [ "${free_gb:-0}" -lt "$MIN_FREE_GB" ] && { log "low disk (${free_gb}G < ${MIN_FREE_GB}G) — abort loop"; break; }
    pgrep -f mem-guard.sh >/dev/null || { log "mem-guard died — abort loop"; break; }

    log "── iteration $i/$MAX_ITERS ──"
    # The full iteration output goes to a per-iteration log so a no-verdict /
    # failed run is diagnosable (the --once test proved this is essential).
    iterlog="docs/architecture/progressive-development-iter-$i.log"
    if [ "$STREAM" != 0 ]; then
        # LIVE mode: stream-json + verbose writes every step (reasoning, tool
        # calls, gate output) to the iter-log AS IT HAPPENS. Watch it with
        # `scripts/progressive-development/watch.sh` or `tail -f $iterlog | jq`.
        # Raw NDJSON in the file; verdict is grepped from the streamed text.
        timeout "$ITER_TIMEOUT" claude "${CLAUDE_ARGS[@]}" --verbose --output-format stream-json \
            --append-system-prompt-file "$CONTEXT" -p "$(cat "$PROMPT_FILE")" >"$iterlog" 2>&1
        rc=$?
        verdict="$(grep -oE 'PROGDEV: (LANDED|FAILED|ESCALATED|ABORT|DRY)[^"\\]*' "$iterlog" | tail -1)"
    else
        # Default (text) mode: cheaper, but claude -p buffers → the iter-log only
        # fills when the iteration ENDS (post-mortem view, not live).
        out="$(timeout "$ITER_TIMEOUT" claude "${CLAUDE_ARGS[@]}" --append-system-prompt-file "$CONTEXT" -p "$(cat "$PROMPT_FILE")" 2>&1 | tee "$iterlog")"; rc=${PIPESTATUS[0]}
        verdict="$(printf '%s\n' "$out" | grep -oE 'PROGDEV: (LANDED|FAILED|ESCALATED|ABORT|DRY)[^\n]*' | tail -1)"
    fi
    log "iteration $i verdict: ${verdict:-<none> (rc=$rc)} (full output: $iterlog)"

    # Enforce the pawl in the HARNESS — never trust the iteration to have reset
    # itself. ANY non-LANDED outcome (failed / no-verdict / abort) hard-discards
    # whatever tracked changes it left, so a dirty tree can never leak onto the
    # branch (or, after the final switch, onto the base). This is the fix for the
    # --once test where a no-verdict iteration's uncommitted wire reached master.
    if [[ "$verdict" != *LANDED* ]] && [ -n "$(git status --porcelain --untracked-files=no)" ]; then
        log "non-LANDED iteration left tracked changes — hard-resetting to hold the pawl (kept in $iterlog)"
        git reset --hard HEAD >/dev/null 2>&1 || true
    fi

    case "$verdict" in
        *LANDED*)    landed=$((landed+1)) ;;
        *DRY*)       log "no eligible mechanical work left — stopping"; break ;;
        *ABORT*)     log "iteration aborted on a safety precondition — stopping"; break ;;
        "")          log "iteration produced no verdict (rc=$rc) — treating as failure, stopping to avoid thrash"; break ;;
    esac

    # Belt-and-braces: tracked tree must be clean between iterations (the pawl).
    [ -z "$(git status --porcelain --untracked-files=no)" ] || { log "tree still dirty after reset — stopping"; break; }

    [ "$ONCE" -eq 1 ] && { log "--once: single iteration done"; break; }
    sleep "$COOLDOWN"
done

git switch "$BASE" 2>/dev/null || true   # restore the checkout to its starting branch
log "progressive-development done: $landed landed on $BRANCH (checkout restored to $BASE)."
log "  review:  git log --oneline $BASE..$BRANCH"
log "  land:    git merge --ff-only $BRANCH   (CLAUDE.md never loaded — branch has only work commits)"
