#!/usr/bin/env bash
# autopilot.sh — the self-refilling autonomous development loop.
#
#   fix → measure → triage → mechanical-burn → guardian-burn → audit → repeat
#
# It runs until everything mechanical is burned and every guardian item is either
# soundly fixed or waiting on a human decision — then STOPS and reports. It never
# manufactures busy-work.
#
#   mechanical PENDING?  → orchestrate.sh (parallel Sonnet lanes, gate-grade)
#   none?                → audit the landed digest (Opus, adversarial)
#                        → remeasure.sh + no-panic FUZZER (deterministic sweep;
#                          a fuzzer panic = a soundness bug = a new guardian item)
#                        → triage (Opus, CONSERVATIVE): classify new blockers into
#                          the queue — mechanical vs guardian; hard-exclude
#                          security/unsafe/FFI/divergence (never mechanical)
#   new mechanical?      → loop
#   only guardian?       → dispatch Opus guardian (worktree) per item, then an
#                          INDEPENDENT adversarial review + the FUZZER gate before
#                          merging; stuck → escalate + mark BLOCKED
#   nothing actionable?  → TERMINAL: stop, emit the landed digest for human audit
#
# SOUNDNESS NOTE: the gate (cargo test) is a sufficient oracle for MECHANICAL work
# but NOT for type-system/soundness work. Guardian output is therefore verified at
# soundness-grade: an independent adversarial review (a second Opus told to REFUTE
# the fix) AND the no-panic fuzzer (scripts/fuzz-well-typed.sh — proven to catch a
# real panic). The fuzzer is DUAL-ROLE: bug-finder in the measure phase, verifier
# at the guardian gate. Never trust the gate alone for guardian changes. The human
# keeps a LIGHT meta-audit of the guardian tier via the digest.
#
# Config (env): PROGDEV_MAX_CYCLES (6) · PROGDEV_MAX_GUARDIAN (2 per run) ·
#   PROGDEV_LANES (2) · PROGDEV_AUTHOR_MODEL (sonnet) · PROGDEV_GUARDIAN_MODEL /
#   PROGDEV_RECONCILE_MODEL (opus) · touch autopilot.stop to halt after the cycle.
set -uo pipefail
cd "$(dirname "$0")/../.."
REPO="$(pwd)"
HERE="scripts/progressive-development"

MAX_CYCLES="${PROGDEV_MAX_CYCLES:-6}"
MAX_GUARDIAN="${PROGDEV_MAX_GUARDIAN:-2}"
FUZZ_ITERS="${PROGDEV_FUZZ_ITERS:-30}"       # no-panic fuzzer iters (measure sweep + guardian gate)
FUZZ="scripts/fuzz-well-typed.sh"
GUARDIAN_MODEL="${PROGDEV_GUARDIAN_MODEL:-claude-opus-4-8}"
GATE_TARGET="${MASTER_GATE_TARGET:-$HOME/.cache/master-gate-target}"
QUEUE="docs/architecture/progressive-development-queue.tsv"   # <STATUS>\t<KIND>\t<desc>
DIGEST="docs/architecture/progressive-development-digest.md"
STOP="autopilot.stop"
LOCK=".autopilot.lock"

log()  { printf '%s | autopilot | %s\n' "$(date -Is)" "$*"; }
die()  { log "ABORT: $*"; exit 1; }
# Opus/Sonnet dispatch with the standard safe flags + a tools allowlist.
agent() { # <model> <allowed-extra> <prompt> ; prints output
    local model="$1" prompt="$2"
    claude --model "$model" --safe-mode --permission-mode auto \
        --allowedTools 'Bash(cargo *)' 'Bash(git *)' 'Bash(skyc *)' 'Bash(rg *)' \
                       'Bash(cat *)' 'Bash(ls *)' 'Bash(sed *)' 'Bash(diff *)' \
                       'Bash(touch *)' 'Bash(mkdir *)' Edit Write Read Grep Glob \
        -p "$prompt" 2>&1
}

# ── preconditions ────────────────────────────────────────────────────────────
command -v claude >/dev/null || die "claude CLI not found"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not a git repo"
[ -z "$(git status --porcelain --untracked-files=no)" ] || die "tracked changes present — commit/stash first"
pgrep -f mem-guard.sh >/dev/null || die "mem-guard.sh not running"
[ -f "$STOP" ] && die "kill-switch $STOP present"
if ! ( set -o noclobber; echo "$$" > "$LOCK" ) 2>/dev/null; then die "another autopilot holds $LOCK"; fi
trap 'rm -f "$LOCK"; log "exit"' EXIT

BASE="$(git rev-parse --abbrev-ref HEAD)"
START_SHA="$(git rev-parse HEAD)"
mkdir -p "$(dirname "$QUEUE")"; touch "$QUEUE"
log "start: base=$BASE start=$START_SHA max_cycles=$MAX_CYCLES max_guardian=$MAX_GUARDIAN"

# queue helpers (queue is append-only history; latest status per desc wins)
pending() { # <kind> → prints PENDING descriptions of that kind, newest status wins
    awk -F'\t' -v k="$1" '{st[$3]=$1; kd[$3]=$2}
        END{for(d in st) if(st[d]=="PENDING" && kd[d]==k) print d}' "$QUEUE"
}
mark() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> "$QUEUE"; }   # status kind desc

# ── the cycle ────────────────────────────────────────────────────────────────
for cycle in $(seq 1 "$MAX_CYCLES"); do
    [ -f "$STOP" ] && { log "kill-switch — stopping"; break; }
    pgrep -f mem-guard.sh >/dev/null || { log "mem-guard died — stopping"; break; }
    log "════ cycle $cycle/$MAX_CYCLES ════"

    # 1 ── mechanical batch ──────────────────────────────────────────────────
    mapfile -t mech < <(pending mechanical)
    if [ "${#mech[@]}" -gt 0 ]; then
        batch=("${mech[@]:0:${PROGDEV_LANES:-2}}")
        log "mechanical batch: ${#batch[@]} item(s) → orchestrate.sh"
        PROGDEV_TS="auto-c$cycle" "$HERE/orchestrate.sh" "${batch[@]}" 2>&1 | sed 's/^/    /'
        for d in "${batch[@]}"; do mark ATTEMPTED mechanical "$d"; done  # sweep re-surfaces genuine misses
        continue
    fi

    # 2 ── no mechanical: audit what landed, then remeasure + triage ──────────
    if [ "$(git rev-parse HEAD)" != "$START_SHA" ]; then
        log "digest audit: adversarial review of landed commits (Opus)"
        landed="$(git log --oneline "$START_SHA"..HEAD)"
        agent "$GUARDIAN_MODEL" "You are AUDITING autonomous commits for soundness. Review the diffs of these commits on the current branch and answer: did ANY of them land a HACK rather than a root-cause fix — e.g. editing a reference-identical example fixture to satisfy our type-checker, weakening/removing a gate or soundness check, adding a \`_ =>\` catch-all to dodge exhaustiveness, or a \`#[allow]\`/\`unwrap\` that hides a contract violation? Commits:
$landed
For each, \`git show <sha>\`. If you find a violation, print AUDIT: VIOLATION <sha> <why> and STOP (do not fix). If all are genuine root-cause work, print AUDIT: CLEAN. Be adversarial; err toward flagging." | tee /tmp/autopilot-audit-c$cycle.log | sed 's/^/    /'
        if rg -q "AUDIT: VIOLATION" /tmp/autopilot-audit-c$cycle.log; then
            log "AUDIT FLAGGED A VIOLATION — stopping for human review (see /tmp/autopilot-audit-c$cycle.log)"; break
        fi
    fi

    log "remeasure (sweep)"; "$HERE/remeasure.sh" 2>&1 | tail -3 | sed 's/^/    /'
    # Fuzz-in-measure: the no-panic fuzzer is a BUG-FINDER here — a well-typed
    # program that panics is a soundness bug, and it becomes a guardian item.
    log "fuzz (soundness sweep, $FUZZ_ITERS iters)"
    if "$FUZZ" --iters "$FUZZ_ITERS" --quiet >/tmp/autopilot-fuzz-c$cycle.log 2>&1; then
        log "fuzz clean"
    else
        fdir="$(rg -o '/tmp/sky-fuzz/FAILURES/[^ ]+' /tmp/autopilot-fuzz-c$cycle.log 2>/dev/null | tail -1)"
        log "FUZZ FOUND A SOUNDNESS BUG → filing a guardian item (artifacts: ${fdir:-see /tmp/autopilot-fuzz-c$cycle.log})"
        mark PENDING guardian "SOUNDNESS: the no-panic fuzzer built a well-typed Sky program that PANICKED at runtime — a codegen/runtime/type-system soundness bug. Repro artifacts: ${fdir:-/tmp/autopilot-fuzz-c$cycle.log} (src + emitted Rust + run.log). Root-cause it; verify the fix with $FUZZ. HIGHEST priority — this is an 'if-it-compiles-it-works' violation."
    fi
    log "triage (Opus, conservative)"
    agent "$GUARDIAN_MODEL" "You are TRIAGING the Ipê compiler backlog to refill the autonomous work queue. Read docs/architecture/remeasure-snapshot.tsv (current per-example blockers) and the repo. For each blocker NOT already resolved, decide its class and append ONE line per item to $QUEUE in the exact format '<STATUS>\t<KIND>\t<one-line description>' (tab-separated), STATUS=PENDING.
KIND is 'mechanical' ONLY if it is a clean, reference-backed wire with no design or soundness decision: a missing kernel to wire (template exists), a module to register, or a fixture parse issue. KIND is 'guardian' for ANYTHING else. HARD RULE — never 'mechanical' if it touches: auth/secrets/crypto, \`unsafe\`/FFI, the type inferencer/soundness, or is an oracle DIVERGENCE from the ../sky reference. When UNSURE → 'guardian'. Be conservative: a wrong 'mechanical' tag lets an unattended lane hack it. Each mechanical description must be self-contained enough for a lane to execute (name the kernel/site + the reference template). Do NOT do the work; only classify + append. Print TRIAGE: <n> mechanical, <m> guardian appended." 2>&1 | tee /tmp/autopilot-triage-c$cycle.log | sed 's/^/    /'

    mapfile -t mech2 < <(pending mechanical)
    [ "${#mech2[@]}" -gt 0 ] && { log "triage produced ${#mech2[@]} mechanical item(s) — looping"; continue; }

    # 3 ── guardian tier: dispatch + adversarial review ──────────────────────
    mapfile -t guard < <(pending guardian)
    if [ "${#guard[@]}" -eq 0 ]; then log "no mechanical AND no guardian items — TERMINAL"; break; fi
    log "no mechanical left; ${#guard[@]} guardian item(s). Dispatching up to $MAX_GUARDIAN this run."
    done_guard=0
    for gdesc in "${guard[@]}"; do
        [ "$done_guard" -ge "$MAX_GUARDIAN" ] && break
        [ -f "$STOP" ] && break
        done_guard=$((done_guard+1))
        log "guardian: $gdesc"
        gbr="progressive-development/guardian-c$cycle-$done_guard"
        gwt="$REPO/.progressive-development-wt/guardian-$done_guard"
        rm -rf "$gwt"; git worktree add --quiet -b "$gbr" "$gwt" "$BASE" || { mark BLOCKED guardian "$gdesc"; continue; }
        ( cd "$gwt"; MASTER_GATE_TARGET="$HOME/.cache/guardian-target" \
            agent "$GUARDIAN_MODEL" "You are a compiler guardian. Root-cause and FIX this item, ROOT-CAUSE ONLY (never a hack/fixture-edit/gate-weakening). Item: $gdesc . Boundary: the Rust-port crates + runtime; ../sky is READ-ONLY reference. Gate on CARGO_TARGET_DIR=\$HOME/.cache/guardian-target (cargo test --workspace + clippy -D warnings, both green), AND the no-panic fuzzer must stay clean (\`scripts/fuzz-well-typed.sh --iters 30\` — a well-typed program must never panic; if your change makes it panic OR fail to build, that is a soundness regression, not a fix). Add a regression test. Commit on this branch. If it is genuinely multi-session, commit partial progress + print GUARDIAN: PARTIAL <what remains>; else GUARDIAN: DONE." ) >/tmp/autopilot-guardian-c$cycle-$done_guard.log 2>&1
        ahead="$(git rev-list --count "$BASE..$gbr" 2>/dev/null || echo 0)"
        if [ "$ahead" -eq 0 ]; then
            log "guardian made no commit — leaving PENDING for human"; mark ESCALATED guardian "$gdesc"
            git worktree remove --force "$gwt" 2>/dev/null; git branch -D "$gbr" 2>/dev/null; continue
        fi
        log "guardian committed; INDEPENDENT adversarial review (Opus) — soundness oracle, not just the gate"
        review="$(agent "$GUARDIAN_MODEL" "You are an ADVERSARIAL reviewer of a compiler type-system/soundness change on branch $gbr (diff: git diff $BASE..$gbr). Your job is to REFUTE it: try to find a program the change now WRONGLY accepts or rejects, a soundness hole, or a weakened gate. Read the diff + the added tests. If you can construct or identify ANY unsoundness or the fix is a disguised hack, print REVIEW: REJECT <why>. Only if you cannot break it after genuine effort, print REVIEW: ACCEPT. Default to REJECT when uncertain. (The no-panic fuzzer scripts/fuzz-well-typed.sh mechanically checks that well-typed programs don't panic AND is run as a hard gate below; YOUR job is the adversarial reasoning it can't do — the corner case, the leaked var, the weakened invariant.)")"
        if printf '%s' "$review" | rg -q "REVIEW: ACCEPT"; then
            git switch "$BASE" >/dev/null 2>&1
            if git merge --no-ff -m "autopilot: guardian fix — $gdesc" "$gbr" >/dev/null 2>&1 \
               && ( touch runtime/tests/*.rs crates/skyc/tests/*.rs 2>/dev/null; CARGO_TARGET_DIR="$GATE_TARGET" timeout 3000 cargo test --workspace >/tmp/autopilot-gate.log 2>&1 && CARGO_TARGET_DIR="$GATE_TARGET" timeout 1200 cargo clippy --workspace --all-targets -- -D warnings >>/tmp/autopilot-gate.log 2>&1 ) \
               && ( "$FUZZ" --iters "$FUZZ_ITERS" --quiet >>/tmp/autopilot-gate.log 2>&1 ); then
                log "guardian fix ACCEPTED + gate-green + fuzz-clean — landed"; mark LANDED guardian "$gdesc"
            else
                log "guardian fix failed merge/gate — reverting"; git merge --abort 2>/dev/null; git reset --hard HEAD >/dev/null 2>&1; mark BLOCKED guardian "$gdesc"
            fi
        else
            log "adversarial review REJECTED — not landing (see /tmp/autopilot-guardian-c$cycle-$done_guard.log)"; mark ESCALATED guardian "$gdesc"
        fi
        git worktree remove --force "$gwt" 2>/dev/null; git branch -D "$gbr" 2>/dev/null
    done
    git worktree prune
    [ "$done_guard" -eq 0 ] && break   # nothing dispatched → terminal
done

# ── landed digest for the human meta-audit ───────────────────────────────────
{
    echo "# Autopilot run digest — $(date -Is)"
    echo ""; echo "Base $START_SHA → $(git rev-parse --short HEAD).  Review before trusting the guardian-tier commits."
    echo ""; echo "## Landed this run"; git log --oneline "$START_SHA"..HEAD | sed 's/^/- /'
    echo ""; echo "## Queue tail"; tail -15 "$QUEUE"
} > "$DIGEST"
log "done. digest → $DIGEST (audit the guardian-tier commits). queue → $QUEUE"
