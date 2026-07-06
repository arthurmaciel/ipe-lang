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
#   only guardian?       → route by CLASS (type-system / runtime / security),
#                          dispatch a class-specialised Opus guardian (worktree) +
#                          a class-specific adversarial review + the FUZZER gate;
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
STREAM="${PROGDEV_STREAM:-}"                  # 1 = agents emit stream-json (live watch.sh); safe (logic is grep/queue-based)
CONTEXT="$REPO/$HERE/context.md"             # operating contract: 6 principles + 2 rules + the seal
GUARDIAN_MODEL="${PROGDEV_GUARDIAN_MODEL:-claude-opus-4-8}"
GATE_TARGET="${MASTER_GATE_TARGET:-$HOME/.cache/master-gate-target}"
QUEUE="docs/architecture/progressive-development-queue.tsv"   # <STATUS>\t<KIND>\t<desc>
DIGEST="docs/architecture/progressive-development-digest.md"
STOP="autopilot.stop"
LOCK=".autopilot.lock"

log()  { printf '%s | autopilot | %s\n' "$(date -Is)" "$*"; }
die()  { log "ABORT: $*"; exit 1; }
# Opus/Sonnet dispatch. EVERY autopilot agent (triage/audit/guardian/review) is
# handed the operating contract via --append-system-prompt-file, so all of them
# obey the 6 principles + 2 rules + the seal (skyc exit-0 ⟹ cargo exit-0) — the
# contract is not optional for any tier.
agent() { # <model> <prompt> ; prints output
    local model="$1" prompt="$2"
    local stream=(); [ -n "$STREAM" ] && stream=(--verbose --output-format stream-json)
    claude --model "$model" --safe-mode --permission-mode auto \
        --append-system-prompt-file "$CONTEXT" "${stream[@]}" \
        --allowedTools 'Bash(cargo *)' 'Bash(git *)' 'Bash(skyc *)' 'Bash(rg *)' \
                       'Bash(cat *)' 'Bash(ls *)' 'Bash(sed *)' 'Bash(diff *)' \
                       'Bash(touch *)' 'Bash(mkdir *)' Edit Write Read Grep Glob \
        -p "$prompt" 2>&1
}

# ── preconditions ────────────────────────────────────────────────────────────
command -v claude >/dev/null || die "claude CLI not found"
[ -f "$CONTEXT" ] || die "missing operating contract $CONTEXT"
[ -x "$FUZZ" ] || die "missing/inexecutable soundness oracle $FUZZ"
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
pending() { # <kind> → PENDING descriptions of that exact kind (newest status wins)
    awk -F'\t' -v k="$1" '{st[$3]=$1; kd[$3]=$2}
        END{for(d in st) if(st[d]=="PENDING" && kd[d]==k) print d}' "$QUEUE"
}
pending_guardian() { # → PENDING guardian-* items as "<kind>\t<desc>" (any class)
    awk -F'\t' '{st[$3]=$1; kd[$3]=$2}
        END{for(d in st) if(st[d]=="PENDING" && kd[d] ~ /^guardian/) print kd[d]"\t"d}' "$QUEUE"
}
mark() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> "$QUEUE"; }   # status kind desc

# ── per-class guardian routing (type-system / runtime / security) ────────────
# Each guardian class gets a focused brief + a class-specific adversarial angle.
# A fuzzer-found panic → guardian-runtime; an inferencer/soundness bug →
# guardian-typesystem; anything on an auth/secrets/crypto/unsafe/FFI surface →
# guardian-security (highest stakes).
guardian_focus() { case "$1" in
  guardian-typesystem) printf '%s' "the HM type inferencer / solver / codegen SOUNDNESS (crates/sky_types, sky_lower, sky_backend_rust). Preserve the parametric-generic + wildcard-any gates. HARD BAN: NEVER introduce \`dyn Any\`, \`Box<dyn Any>\`, \`.downcast\`, or any runtime type-erasure/reflection to paper over a type — the port's guarantee is fully-typed codegen; a fix that reaches for \`Any\` is not a fix, it is a soundness hole. A fix that makes the no-panic fuzzer fail or that accepts an ill-typed program is a regression, not a fix." ;;
  guardian-runtime)    printf '%s' "the EMITTED-CODE runtime behaviour (runtime/src/sky_runtime, the emit in crates/sky_backend_rust). A well-typed program must never panic — add/repair the runtime guard or codegen so the failing case exits cleanly with a typed Error, matching the ../sky reference. Do NOT silence a panic by weakening a check." ;;
  guardian-security)   printf '%s' "a SECURITY-sensitive surface (auth/secrets/crypto/SQL/unsafe/FFI). EXTRA RULES: secrets stay typed and are NEVER logged or fmt-stringified; constant-time compares for anything secret; NO new \`unsafe\` — full stop (a 'justified' unsafe is still a hole; if the change seems to need unsafe, STOP and escalate, do not write it); parse-don't-validate; SQL as typed fragments, never string interpolation. This is the highest-stakes class — prefer a minimal, conservative, heavily-tested change and STOP+escalate rather than guess." ;;
  *)                   printf '%s' "the compiler internals; root-cause only." ;;
esac; }
reviewer_angle() { case "$1" in
  guardian-typesystem) printf '%s' "construct a program the change now WRONGLY accepts (unsoundness) or wrongly rejects (regression); probe the parametric-generic gate + numeric/collection defaulting. AUTO-REJECT if the diff introduces ANY \`dyn Any\` / \`.downcast\` / runtime type-erasure." ;;
  guardian-runtime)    printf '%s' "find an input that still panics, or a case where the new guard changes observable behaviour vs the ../sky reference." ;;
  guardian-security)   printf '%s' "ATTACK it, assume malice: can a secret leak via logs/errors/timing? is any compare non-constant-time? does it weaken an existing gate or open an injection? AUTO-REJECT if the diff adds ANY new \`unsafe\`." ;;
  *)                   printf '%s' "try to find any unsoundness, behaviour change, or disguised hack." ;;
esac; }

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
        mark PENDING guardian-runtime "SOUNDNESS: the no-panic fuzzer built a well-typed Sky program that PANICKED at runtime — a codegen/runtime soundness bug. Repro artifacts: ${fdir:-/tmp/autopilot-fuzz-c$cycle.log} (src + emitted Rust + run.log). Root-cause it; verify the fix with $FUZZ. HIGHEST priority — this is an 'if-it-compiles-it-works' violation."
    fi
    log "triage (Opus, conservative)"
    agent "$GUARDIAN_MODEL" "You are TRIAGING the Ipê compiler backlog to refill the autonomous work queue. Read docs/architecture/remeasure-snapshot.tsv (current per-example blockers) and the repo. For each blocker NOT already resolved, decide its class and append ONE line per item to $QUEUE in the exact format '<STATUS>\t<KIND>\t<one-line description>' (tab-separated), STATUS=PENDING.
KIND is exactly one of: 'mechanical' | 'guardian-typesystem' | 'guardian-runtime' | 'guardian-security'. Use 'mechanical' ONLY if it is a clean, reference-backed wire with no design or soundness decision (a missing kernel to wire with an existing template, a module to register, a fixture parse issue). Otherwise pick the guardian CLASS: 'guardian-security' if it touches auth/secrets/crypto/SQL/\`unsafe\`/FFI (HARD RULE — such an item is NEVER mechanical); 'guardian-runtime' for a runtime panic / emitted-code behaviour bug / oracle DIVERGENCE in runtime output; 'guardian-typesystem' for an inferencer/solver/codegen soundness or any other type-checking bug. When UNSURE mechanical-vs-guardian → guardian (a wrong 'mechanical' tag lets an unattended lane hack it). When unsure WHICH guardian class → 'guardian-typesystem'. Each mechanical description must be self-contained enough for a lane to execute (name the kernel/site + the reference template). Do NOT do the work; only classify + append. Print TRIAGE: <n> mechanical, <m> guardian appended." 2>&1 | tee /tmp/autopilot-triage-c$cycle.log | sed 's/^/    /'

    mapfile -t mech2 < <(pending mechanical)
    [ "${#mech2[@]}" -gt 0 ] && { log "triage produced ${#mech2[@]} mechanical item(s) — looping"; continue; }

    # 3 ── guardian tier: dispatch + adversarial review ──────────────────────
    mapfile -t guard < <(pending_guardian)   # each entry: "<guardian-class>\t<desc>"
    if [ "${#guard[@]}" -eq 0 ]; then log "no mechanical AND no guardian items — TERMINAL"; break; fi
    log "no mechanical left; ${#guard[@]} guardian item(s). Dispatching up to $MAX_GUARDIAN this run."
    done_guard=0
    for g in "${guard[@]}"; do
        [ "$done_guard" -ge "$MAX_GUARDIAN" ] && break
        [ -f "$STOP" ] && break
        class="${g%%$'\t'*}"; gdesc="${g#*$'\t'}"
        [ -z "$class" ] && class="guardian-typesystem"
        done_guard=$((done_guard+1))
        log "guardian [$class]: $gdesc"
        gbr="progressive-development/guardian-c$cycle-$done_guard"
        gwt="$REPO/.progressive-development-wt/guardian-$done_guard"
        rm -rf "$gwt"; git worktree add --quiet -b "$gbr" "$gwt" "$BASE" || { mark BLOCKED "$class" "$gdesc"; continue; }
        ( cd "$gwt"; MASTER_GATE_TARGET="$HOME/.cache/guardian-target" \
            agent "$GUARDIAN_MODEL" "You are a compiler guardian specialising in $class. Obey the operating contract in your system prompt IN FULL — the 6 principles + 2 rules + the seal (skyc exit-0 ⟹ cargo exit-0: NEVER emit codegen that type-checks but fails cargo). Root-cause and FIX this item, ROOT-CAUSE ONLY (never a hack/fixture-edit/gate-weakening). FOCUS: $(guardian_focus "$class"). Item: $gdesc . Boundary: the Rust-port crates + runtime; ../sky is READ-ONLY reference. Gate on CARGO_TARGET_DIR=\$HOME/.cache/guardian-target (cargo test --workspace + clippy --all-targets -D warnings, both green — MATCH the master gate exactly), AND the no-panic fuzzer must stay clean (\`scripts/fuzz-well-typed.sh --iters 30\`). Add a regression test. Commit on this branch. If genuinely multi-session, commit partial progress + print GUARDIAN: PARTIAL <what remains>; else GUARDIAN: DONE." ) >/tmp/autopilot-guardian-c$cycle-$done_guard.log 2>&1
        ahead="$(git rev-list --count "$BASE..$gbr" 2>/dev/null || echo 0)"
        if [ "$ahead" -eq 0 ]; then
            log "guardian [$class] made no commit — leaving for human"; mark ESCALATED "$class" "$gdesc"
            git worktree remove --force "$gwt" 2>/dev/null; git branch -D "$gbr" 2>/dev/null; continue
        fi
        log "guardian [$class] committed; INDEPENDENT adversarial review — soundness-grade, not just the gate"
        review="$(agent "$GUARDIAN_MODEL" "You are an ADVERSARIAL reviewer of a $class change on branch $gbr (diff: git diff $BASE..$gbr). REFUTE it — $(reviewer_angle "$class"). Read the diff + the added tests. If you find ANY unsoundness / behaviour change vs the ../sky reference / disguised hack, print REVIEW: REJECT <why>. Only if you cannot break it after genuine effort, print REVIEW: ACCEPT. Default to REJECT when uncertain. (The no-panic fuzzer is a hard gate below; YOUR job is the reasoning it can't do.)")"
        if printf '%s' "$review" | rg -q "REVIEW: ACCEPT"; then
            git switch "$BASE" >/dev/null 2>&1
            if git merge --no-ff -m "autopilot: $class fix — $gdesc" "$gbr" >/dev/null 2>&1 \
               && ( touch runtime/tests/*.rs crates/skyc/tests/*.rs 2>/dev/null; CARGO_TARGET_DIR="$GATE_TARGET" timeout 3000 cargo test --workspace >/tmp/autopilot-gate.log 2>&1 && CARGO_TARGET_DIR="$GATE_TARGET" timeout 1200 cargo clippy --workspace --all-targets -- -D warnings >>/tmp/autopilot-gate.log 2>&1 ) \
               && ( "$FUZZ" --iters "$FUZZ_ITERS" --quiet >>/tmp/autopilot-gate.log 2>&1 ); then
                log "guardian [$class] ACCEPTED + gate-green + fuzz-clean — landed"; mark LANDED "$class" "$gdesc"
            else
                log "guardian [$class] failed merge/gate/fuzz — reverting"; git merge --abort 2>/dev/null; git reset --hard HEAD >/dev/null 2>&1; mark BLOCKED "$class" "$gdesc"
            fi
        else
            log "adversarial review [$class] REJECTED — not landing (see /tmp/autopilot-guardian-c$cycle-$done_guard.log)"; mark ESCALATED "$class" "$gdesc"
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
