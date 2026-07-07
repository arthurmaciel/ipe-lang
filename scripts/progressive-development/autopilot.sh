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
# Config (env): PROGDEV_MAX_CYCLES (100 backstop) · PROGDEV_MAX_GUARDIAN (2/cycle) ·
#   PROGDEV_LANES (2) · PROGDEV_AUTHOR_MODEL (sonnet) · PROGDEV_GUARDIAN_MODEL /
#   PROGDEV_RECONCILE_MODEL (opus) · touch autopilot.stop to halt after the cycle.
set -uo pipefail
cd "$(dirname "$0")/../.."
REPO="$(pwd)"
HERE="scripts/progressive-development"

MAX_CYCLES="${PROGDEV_MAX_CYCLES:-100}"   # runaway backstop only; real stop = 2-dry-pass convergence
MAX_GUARDIAN="${PROGDEV_MAX_GUARDIAN:-2}"
FUZZ_ITERS="${PROGDEV_FUZZ_ITERS:-30}"       # no-panic fuzzer iters (measure sweep + guardian gate)
FUZZ="scripts/fuzz-well-typed.sh"
STREAM="${PROGDEV_STREAM:-1}"                 # DEFAULT ON (watch.sh renders it); PROGDEV_STREAM=0 to disable. Safe: logic is grep/queue-based
WATCH="${PROGDEV_WATCH:-1}"                   # 1 = auto-launch watch.sh alongside (one terminal); 0 / --no-watch disables
CONTEXT="$REPO/$HERE/context.md"             # operating contract: 6 principles + 2 rules + the seal
GUARDIAN_MODEL="${PROGDEV_GUARDIAN_MODEL:-claude-opus-4-8}"
AUTHOR_MODEL="${PROGDEV_AUTHOR_MODEL:-claude-sonnet-4-6}"   # v4: Sonnet implements the Opus design
GATE_TARGET="${MASTER_GATE_TARGET:-$HOME/.cache/master-gate-target}"
QUEUE="docs/architecture/progressive-development-queue.tsv"   # <STATUS>\t<KIND>\t<desc>
RESUME_DIR="docs/architecture/progressive-development-resume"  # gitignored guardian resume artifacts
GUARDIAN_ATTEMPTS="${PROGDEV_GUARDIAN_ATTEMPTS:-2}"  # v4: 2 resuming attempts, then phase-4 (review-dominated failures → 3rd try low-yield)
DIGEST="docs/architecture/progressive-development-digest.md"
STOP="autopilot.stop"
LOCK=".autopilot.lock"
GUARDIAN_TARGET="$HOME/.cache/guardian-target"
DISK_FLOOR="${PROGDEV_DISK_FLOOR_GB:-20}"    # reclaim when free drops below this
DISK_CRIT="${PROGDEV_DISK_CRIT_GB:-10}"      # graceful-stop when still below this after reclaim
# The loop's ENTIRE cargo-target footprint — everything else under ~/.cache is
# reclaimable. Bounds disk to these two + the shared dev target.
KEEP_TARGETS="$(basename "$GATE_TARGET") $(basename "$GUARDIAN_TARGET") sky-rust-target"
LEDGER="docs/architecture/progdev-cost-ledger.tsv"   # persistent per-cycle agent-cost ledger (survives /tmp overwrite across runs)
SHIMDIR="/tmp/autopilot-shims"                        # rg-enforcement: grep/egrep/fgrep shims prepended to the agent PATH

log()  { printf '%s | autopilot | %s\n' "$(date +%H:%M)" "$*"; }
die()  { log "ABORT: $*"; exit 1; }

# v4 status header — current task / phase / model, rewritten on each transition.
# watch.sh shows it fixed at top; `cat docs/architecture/progdev-status.txt` any time.
STATUS="docs/architecture/progdev-status.txt"
set_task() { CUR_TYPE="$1"; CUR_TASK="$(printf '%.110s' "$2")"; CUR_START="$(date +%H:%M)"; CUR_ATT="${3:-·}"; }
phase() {  # <phase-name> <model>
    { printf 'task    %s\n' "${CUR_TASK:-·}"
      printf 'type    %s   attempt %s   started %s\n' "${CUR_TYPE:-·}" "${CUR_ATT:-·}" "${CUR_START:-·}"
      printf 'phase   %-11s model %-8s now %s\n' "$1" "${2:-·}" "$(date +%H:%M)"; } > "$STATUS" 2>/dev/null
    log "· $1 (${2:-·})"
}

usage() {
    cat <<'EOF'
autopilot.sh — self-refilling autonomous development loop
  fix → measure(remeasure + no-panic fuzzer) → triage → mechanical-burn →
  guardian-burn → audit → repeat. Runs until only human-decision work remains,
  then STOPS and reports. Never manufactures busy-work.

Usage: scripts/progressive-development/autopilot.sh [--no-watch] [-h|--help]

Flags:
  --no-watch      don't auto-launch the live monitor (watch.sh) in this terminal
  -h, --help      show this help and exit

Env vars (defaults):
  PROGDEV_MAX_CYCLES      (100)    runaway backstop only (real stop = converge: 2 passes, no new findings)
  PROGDEV_MAX_GUARDIAN    (2)      guardian items dispatched per run
  PROGDEV_LANES           (2)      parallel mechanical lanes (this box: 2)
  PROGDEV_FUZZ_ITERS      (30)     no-panic fuzzer iters (measure sweep + guardian gate)
  PROGDEV_STREAM          (1)      agents emit stream-json for the live view; 0 = plain-text logs
  PROGDEV_WATCH           (1)      auto-launch watch.sh alongside (one terminal); 0 = don't (== --no-watch)
  PROGDEV_AUTHOR_MODEL    (claude-sonnet-4-6)   mechanical-lane model
  PROGDEV_GUARDIAN_MODEL  (claude-opus-4-8)     guardian / triage / audit / review model
  PROGDEV_RECONCILE_MODEL (claude-opus-4-8)     merge-conflict reconcile model (via orchestrate.sh)
  MASTER_GATE_TARGET      (~/.cache/master-gate-target)   isolated gate target dir

Control:  touch autopilot.stop  → halt cleanly after the current cycle
Monitor:  watch.sh runs automatically (unless --no-watch); or run it in another terminal.
EOF
}
for arg in "$@"; do case "$arg" in
    -h|--help)   usage; exit 0 ;;
    --no-watch)  WATCH=0 ;;
    *)           die "unknown argument: $arg (try --help)" ;;
esac; done

# mem-guard.sh (memory kill-switch) is REQUIRED, so autopilot DISPATCHES it when
# it isn't already up — rather than aborting on the caller. A runaway skyc /
# cargo / rustc can pressure the host into an OOM; mem-guard is the backstop.
# It's a host-protection daemon (other tooling relies on it too), so we start it
# and LEAVE it running on exit — unlike watch.sh, which is autopilot-scoped.
ensure_mem_guard() {
    pgrep -f mem-guard.sh >/dev/null 2>&1 && return 0
    [ -x scripts/mem-guard.sh ] || die "scripts/mem-guard.sh missing/not executable — cannot start the memory kill-switch"
    log "mem-guard.sh not running — dispatching it (memory kill-switch)"
    nohup ./scripts/mem-guard.sh >/tmp/mem-guard.out 2>&1 & disown
    for _ in 1 2 3 4 5; do
        pgrep -f mem-guard.sh >/dev/null 2>&1 && { log "mem-guard.sh up (pid $(pgrep -f mem-guard.sh | head -1))"; return 0; }
        sleep 1
    done
    die "mem-guard.sh failed to start within 5s (see /tmp/mem-guard.out)"
}

# Opus/Sonnet dispatch. EVERY autopilot agent (triage/audit/guardian/review) is
# handed the operating contract via --append-system-prompt-file, so all of them
# obey the 6 principles + 2 rules + the seal (skyc exit-0 ⟹ cargo exit-0) — the
# contract is not optional for any tier.
agent() { # <model> <prompt> ; prints output
    local model="$1" prompt="$2"
    local stream=(); [ "$STREAM" != 0 ] && stream=(--verbose --output-format stream-json)
    # rg enforcement: headless `claude -p` IGNORES PreToolUse hooks (verified), so
    # the interactive rg hook doesn't reach agents. Shadow grep/egrep/fgrep with a
    # PATH shim instead (git grep still works — it doesn't exec the grep binary).
    PATH="$SHIMDIR:$PATH" claude --model "$model" --safe-mode --permission-mode auto \
        --append-system-prompt-file "$CONTEXT" "${stream[@]}" \
        --allowedTools 'Bash(cargo *)' 'Bash(git *)' 'Bash(skyc *)' 'Bash(rg *)' \
                       'Bash(cat *)' 'Bash(ls *)' 'Bash(sed *)' 'Bash(diff *)' \
                       'Bash(touch *)' 'Bash(mkdir *)' Edit Write Read Grep Glob \
        -p "$prompt" 2>&1
}
# Render an agent's tee'd output to the heartbeat so it stays HUMAN-READABLE
# (raw stream-json would otherwise flood stdout when no watch.sh is attached).
# The LOG file still gets the raw json for watch.sh; this only shapes stdout.
show_agent() { if [ "$STREAM" != 0 ]; then "$HERE/render-stream.sh" "$1"; else sed "s/^/  [$1] /"; fi; }

# ── disk safety (ENOSPC mid-build corrupts git state — prevent, don't crash) ──
disk_free_gb() { df -BG --output=avail / | tail -1 | tr -dc '0-9'; }
reclaim_disk() {
    log "disk reclaim (free $(disk_free_gb)G) — keeping only [$KEEP_TARGETS]"
    for d in "$HOME"/.cache/*target*; do
        [ -d "$d" ] || continue
        local keep=0 k; for k in $KEEP_TARGETS; do [ "$(basename "$d")" = "$k" ] && keep=1; done
        [ "$keep" -eq 0 ] && { log "  rm $(basename "$d") ($(du -sh "$d" 2>/dev/null | cut -f1))"; rm -rf "$d"; }
    done
    git worktree prune 2>/dev/null
    rm -rf /tmp/sky-fuzz.* /tmp/sky-fuzz-neg.* /tmp/orch-*.log 2>/dev/null
    log "  free now $(disk_free_gb)G"
}
# reclaim if below floor; return 1 (caller stops) if still critical afterwards.
ensure_disk() {
    [ "$(disk_free_gb)" -ge "$DISK_FLOOR" ] && return 0
    log "low disk ($(disk_free_gb)G < ${DISK_FLOOR}G)"; reclaim_disk
    [ "$(disk_free_gb)" -lt "$DISK_CRIT" ] && return 1 || return 0
}

# ── preconditions ────────────────────────────────────────────────────────────
command -v claude >/dev/null || die "claude CLI not found"
[ -f "$CONTEXT" ] || die "missing operating contract $CONTEXT"
[ -x "$FUZZ" ] || die "missing/inexecutable soundness oracle $FUZZ"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not a git repo"
[ -z "$(git status --porcelain --untracked-files=no)" ] || die "tracked changes present — commit/stash first"
ensure_mem_guard
# rg-enforcement shims for agents (grep→error+exit; git grep unaffected).
mkdir -p "$SHIMDIR"
for _g in grep egrep fgrep; do
    printf '#!/usr/bin/env bash\necho "grep is disabled for autopilot agents — use rg (ripgrep): rg -n PATTERN / rg -l / rg -c." >&2\nexit 2\n' > "$SHIMDIR/$_g"
    chmod +x "$SHIMDIR/$_g"
done
mkdir -p "$(dirname "$LEDGER")"; [ -f "$LEDGER" ] || printf 'stamp\trun\tcycle\tcum_cost\n' > "$LEDGER"
ensure_disk || die "disk critically low even after reclaim ($(disk_free_gb)G < ${DISK_CRIT}G) — free space and retry"
[ -f "$STOP" ] && die "kill-switch $STOP present"
if ! ( set -o noclobber; echo "$$" > "$LOCK" ) 2>/dev/null; then die "another autopilot holds $LOCK"; fi

# One-terminal DX: auto-launch the live monitor as a child (tty only), killed on
# exit. The heartbeat below + watch.sh's rendered agent stream interleave in this
# terminal — distinguishable by prefix (`| autopilot |` vs indented `↳[tag]`).
# Opt out with --no-watch / PROGDEV_WATCH=0 (headless/CI, or a separate terminal).
watch_pid=""
if [ "$WATCH" != 0 ] && [ -t 1 ] && [ -x "$HERE/watch.sh" ]; then
    "$HERE/watch.sh" & watch_pid=$!
    log "live monitor: watch.sh pid $watch_pid (one terminal; --no-watch / PROGDEV_WATCH=0 to disable)"
fi
trap 'rm -f "$LOCK"; [ -n "$watch_pid" ] && { kill "$watch_pid" 2>/dev/null; pkill -P "$watch_pid" 2>/dev/null; }; log "exit"' EXIT

BASE="$(git rev-parse --abbrev-ref HEAD)"
START_SHA="$(git rev-parse HEAD)"
mkdir -p "$(dirname "$QUEUE")"; touch "$QUEUE"
log "start: base=$BASE start=$START_SHA max_cycles=$MAX_CYCLES max_guardian=$MAX_GUARDIAN"

# queue helpers (append-only history; newest status per desc wins). Actionability:
# a MECHANICAL item is dropped after 2 attempts (it must not spin the loop). A
# GUARDIAN item gets up to GUARDIAN_ATTEMPTS *thoughtful* tries — each one RESUMES
# from the prior attempt's saved artifact rather than restarting cold — and is
# ESCALATED to a human only after exhausting them. ESCALATED = final (suppressed).
# This is what lets the loop converge while still giving hard items a real effort.
pending() { # mechanical: 2 attempts, then suppressed
    awk -F'\t' -v k="$1" '
        { st[$3]=$1; kd[$3]=$2
          if($1=="ESCALATED"||$1=="BLOCKED") dead[$3]=1
          if($1=="ATTEMPTED") att[$3]++ }
        END{for(d in st) if(st[d]=="PENDING" && kd[d]==k && !(d in dead) && att[d]<2) print d}' "$QUEUE"
}
pending_guardian() { # guardian: up to GUARDIAN_ATTEMPTS resuming tries, then ESCALATED
    awk -F'\t' -v maxa="$GUARDIAN_ATTEMPTS" '
        { st[$3]=$1; kd[$3]=$2
          if($1=="ESCALATED") dead[$3]=1
          if($1=="ATTEMPTED"||$1=="BLOCKED") att[$3]++ }
        END{for(d in st) if(st[d]=="PENDING" && kd[d] ~ /^guardian/ && !(d in dead) && att[d]<maxa) print kd[d]"\t"d}' "$QUEUE"
}
mark() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> "$QUEUE"; }   # status kind desc
attempts_of() { awk -F'\t' -v d="$1" '$3==d && ($1=="ATTEMPTED"||$1=="BLOCKED"){n++} END{print n+0}' "$QUEUE"; }
slug_of()     { printf '%s' "$1" | tr -cs 'A-Za-z0-9' '-' | sed 's/^-//;s/-*$//' | cut -c1-64; }
# Save what a guardian attempt tried (diff + why it didn't land) so the NEXT
# attempt resumes instead of restarting cold. Gitignored → survives git reset.
save_resume() { # <class> <desc> <branch> <reason>
    mkdir -p "$RESUME_DIR"
    local f="$RESUME_DIR/$(slug_of "$2").md"
    { echo "# Resume: $2"
      echo "class=$1  attempts_used=$(( $(attempts_of "$2") + 1 ))/$GUARDIAN_ATTEMPTS  saved=$(date -Is)"
      echo; echo "## Why the last attempt did not land"; printf '%s\n' "$4" | head -40
      echo; echo "## Prior attempt diff (a STARTING point — the reviewer may have refuted it; do NOT blindly re-apply)"
      echo '```diff'; git diff "$BASE".."$3" 2>/dev/null | head -500; echo '```'
    } > "$f"
    log "  resume saved → $f"
}
resume_hint() { # <desc> → a prompt fragment if a prior attempt exists (ABSOLUTE path:
    # the guardian runs in a worktree, but the artifact is gitignored in the main checkout)
    local f="$RESUME_DIR/$(slug_of "$1").md"
    [ -f "$f" ] && printf ' A PRIOR ATTEMPT exists at %s — READ it (its diff + why it failed) and CONTINUE from there; do NOT restart cold and do NOT repeat a refuted approach.' "$REPO/$f"
}
# One guardian attempt failed: save it for resume, then re-queue (PENDING) if
# attempts remain, else ESCALATE to a human (artifact preserved).
reason_of() { printf '%s' "$1" | "$HERE/render-stream.sh" 2>/dev/null | tr '\n' ' ' | cut -c1-600; }  # agent stream-json → readable one-liner (resume reasons/logs)
guardian_failed() { # <class> <desc> <branch> <reason>
    save_resume "$1" "$2" "$3" "$4"
    local n=$(( $(attempts_of "$2") + 1 ))
    mark ATTEMPTED "$1" "$2"
    if [ "$n" -ge "$GUARDIAN_ATTEMPTS" ]; then
        mark ESCALATED "$1" "$2"; log "guardian [$1] exhausted $GUARDIAN_ATTEMPTS attempts — ESCALATED to human (resume at $RESUME_DIR/$(slug_of "$2").md)"
    else
        mark PENDING "$1" "$2"; log "guardian [$1] attempt $n/$GUARDIAN_ATTEMPTS failed — re-queued to RESUME next pass"
    fi
}   # status kind desc

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

# ── the loop: run until DONE (2 passes, no new tractable findings) ────────────
# Converged = 2 consecutive cycles where nothing landed AND nothing actionable
# remains (all items resolved, or escalated/blocked/2x-attempted → suppressed).
# MAX_CYCLES is a pure runaway backstop, NEVER the normal stop.
prev_head=""; dry=0; cycle=0; last_audit=""
while :; do
    cycle=$((cycle+1))
    [ -f "$STOP" ] && { log "kill-switch — stopping"; break; }
    pgrep -f mem-guard.sh >/dev/null || { log "mem-guard died — stopping"; break; }
    ensure_disk || { log "disk still critical after reclaim ($(disk_free_gb)G) — graceful stop before any build (no mid-build ENOSPC)"; break; }
    [ "$cycle" -gt "$MAX_CYCLES" ] && { log "runaway backstop ($MAX_CYCLES cycles) — stopping; raise PROGDEV_MAX_CYCLES if legit"; break; }

    # convergence: did the PREVIOUS cycle make progress? (HEAD advanced, or work remains)
    cur_head="$(git rev-parse HEAD)"
    act="$({ pending mechanical; pending_guardian; } | wc -l | tr -d ' ')"
    if [ -n "$prev_head" ] && [ "$cur_head" = "$prev_head" ] && [ "$act" -eq 0 ]; then
        dry=$((dry+1)); log "no-progress pass $dry/2 (nothing landed, no tractable findings)"
        [ "$dry" -ge 2 ] && { log "converged — 2 dry passes; work done or human-blocked. stopping."; break; }
    else
        dry=0
    fi
    prev_head="$cur_head"
    log "cycle $cycle (dry=$dry, actionable=$act)"

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
    if [ "$(git rev-parse HEAD)" != "$START_SHA" ] && [ "$(git rev-parse HEAD)" != "$last_audit" ]; then
        last_audit="$(git rev-parse HEAD)"
        log "digest audit: adversarial review of landed commits (Opus)"
        landed="$(git log --oneline "$START_SHA"..HEAD)"
        agent "$GUARDIAN_MODEL" "You are AUDITING autonomous commits for soundness. Review the diffs of these commits on the current branch and answer: did ANY of them land a HACK rather than a root-cause fix — e.g. editing a reference-identical example fixture to satisfy our type-checker, weakening/removing a gate or soundness check, adding a \`_ =>\` catch-all to dodge exhaustiveness, or a \`#[allow]\`/\`unwrap\` that hides a contract violation? Commits:
$landed
For each, \`git show <sha>\`. If you find a violation, print AUDIT: VIOLATION <sha> <why> and STOP (do not fix). If all are genuine root-cause work, print AUDIT: CLEAN. Be adversarial; err toward flagging." | tee /tmp/autopilot-audit-c$cycle.log | show_agent audit
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
KIND is exactly one of: 'mechanical' | 'guardian-typesystem' | 'guardian-runtime' | 'guardian-security'. Use 'mechanical' ONLY if it is a clean, reference-backed wire with no design or soundness decision (a missing kernel to wire with an existing template, a module to register, a fixture parse issue). Otherwise pick the guardian CLASS: 'guardian-security' if it touches auth/secrets/crypto/SQL/\`unsafe\`/FFI (HARD RULE — such an item is NEVER mechanical); 'guardian-runtime' for a runtime panic / emitted-code behaviour bug / oracle DIVERGENCE in runtime output; 'guardian-typesystem' for an inferencer/solver/codegen soundness or any other type-checking bug. When UNSURE mechanical-vs-guardian → guardian (a wrong 'mechanical' tag lets an unattended lane hack it). When unsure WHICH guardian class → 'guardian-typesystem'. Each mechanical description must be self-contained enough for a lane to execute (name the kernel/site + the reference template). Do NOT do the work; only classify + append. Print TRIAGE: <n> mechanical, <m> guardian appended." 2>&1 | tee /tmp/autopilot-triage-c$cycle.log | show_agent triage

    mapfile -t mech2 < <(pending mechanical)
    [ "${#mech2[@]}" -gt 0 ] && { log "triage produced ${#mech2[@]} mechanical item(s) — looping"; continue; }

    # 3 ── guardian tier: dispatch + adversarial review ──────────────────────
    mapfile -t guard < <(pending_guardian)   # each entry: "<guardian-class>\t<desc>"
    if [ "${#guard[@]}" -eq 0 ]; then log "nothing actionable this pass"; continue; fi
    log "no mechanical left; ${#guard[@]} guardian item(s). Dispatching up to $MAX_GUARDIAN this run."
    done_guard=0
    for g in "${guard[@]}"; do
        [ "$done_guard" -ge "$MAX_GUARDIAN" ] && break
        [ -f "$STOP" ] && break
        class="${g%%$'\t'*}"; gdesc="${g#*$'\t'}"
        [ -z "$class" ] && class="guardian-typesystem"
        done_guard=$((done_guard+1))
        set_task "$class" "$gdesc" "$(( $(attempts_of "$gdesc") + 1 ))/$GUARDIAN_ATTEMPTS"
        log "guardian [$class] · $gdesc"
        gbr="progressive-development/guardian-c$cycle-$done_guard"
        gwt="$REPO/.progressive-development-wt/guardian-$done_guard"
        glog="/tmp/autopilot-guardian-c$cycle-$done_guard.log"
        dlog="/tmp/autopilot-guardian-design-c$cycle-$done_guard.log"   # tee'd so watch.sh follows the design stream live
        rlog="/tmp/autopilot-guardian-review-c$cycle-$done_guard.log"   # tee'd so watch.sh follows the review stream live
        rm -rf "$gwt"; git worktree add --quiet -b "$gbr" "$gwt" "$BASE" || { mark BLOCKED "$class" "$gdesc"; continue; }

        # ── v4 stage 1: DESIGN — Opus on the FIRST attempt; REUSED from the saved
        # plan on a RETRY. A REVIEW: REJECT usually means the impl was wrong, not the
        # plan, and re-designing every attempt was the single biggest cost lane. On a
        # retry the impl gets the rejection reason (resume_hint) so it fixes the
        # specific defect against the SAME plan. (If the plan itself was wrong, the
        # 2nd attempt escalates anyway — bounded at GUARDIAN_ATTEMPTS.)
        dplan="$RESUME_DIR/$(slug_of "$gdesc").design.md"
        dfile="/tmp/autopilot-guardian-design-plan-c$cycle-$done_guard.md"
        datt="$(( $(attempts_of "$gdesc") + 1 ))"
        if [ "$datt" -ge 2 ] && [ -s "$dplan" ]; then
            phase design reused
            log "guardian [$class] · design REUSED (attempt $datt — impl refines the prior plan against the rejection)"
            design_text="$(cat "$dplan")"; cp -f "$dplan" "$dfile"; : > "$dlog"
        else
            phase design opus
            design="$(agent "$GUARDIAN_MODEL" "You are a compiler guardian DESIGNER specialising in $class. Do NOT write code. Produce a concise root-cause + fix PLAN: (a) the root cause, (b) the exact crates/files/functions to change, (c) the approach — matching the ../sky READ-ONLY reference, root-cause only, NEVER a hack/fixture-edit/gate-weakening, (d) the regression test to add. FOCUS: $(guardian_focus "$class"). Item: $gdesc .$(resume_hint "$gdesc") If there is NO sound fix (needs a human decision, genuinely multi-session, or would require a hack), print exactly 'DESIGN: ESCALATE <why>' and nothing else. Otherwise print 'DESIGN: <the plan>'." | tee "$dlog")"
            if printf '%s' "$design" | rg -q 'DESIGN: ESCALATE'; then
                log "guardian [$class] · design → ESCALATE"
                guardian_failed "$class" "$gdesc" "$gbr" "design escalate: $(reason_of "$design")"
                git worktree remove --force "$gwt" 2>/dev/null; git branch -D "$gbr" 2>/dev/null; continue
            fi
            design_text="$(printf '%s' "$design" | jq -rj 'select(.type=="assistant") | .message.content[]? | select(.type=="text") | .text' 2>/dev/null)"
            [ -z "$design_text" ] && design_text="$design"   # STREAM=0 (already plain text) or jq absent
            mkdir -p "$RESUME_DIR"; printf '%s\n' "$design_text" > "$dplan"   # persist so a retry REUSES it
            printf '%s\n' "$design_text" > "$dfile"
        fi

        # ── v4 stage 2: Sonnet IMPL — follows the plan FILE ($dfile), never argv
        # (PROGDEV_STREAM=1 $design is many-KB stream-json → ARG_MAX E2BIG). On a
        # retry resume_hint points impl at the prior diff + why it was rejected.
        phase impl sonnet
        ( cd "$gwt"; CARGO_TARGET_DIR="$GUARDIAN_TARGET" \
          agent "$AUTHOR_MODEL" "You are the IMPLEMENTER. READ the DESIGN plan at $dfile FIRST, then follow it EXACTLY — do NOT redesign or deviate from it. Obey the operating contract (6 principles + 2 rules + the seal). Item: $gdesc .$(resume_hint "$gdesc") Boundary: the Rust-port crates + runtime; ../sky is READ-ONLY. Implement the fix + the regression test the design names. SELF-CHECK (do NOT run the full workspace test suite — the integration gate runs that once): (1) 'cargo clippy --workspace --all-targets -- -D warnings' is clean; (2) 'cargo build -p skyc', then rebuild the failing example and confirm its ORIGINAL diagnostic is GONE. Iterate until BOTH pass (cap ~3 tries). Then 'git add -A && git commit'. Final line: 'IMPL: DONE' or 'IMPL: STUCK <why>'." ) >"$glog" 2>&1
        ahead="$(git rev-list --count "$BASE..$gbr" 2>/dev/null || echo 0)"
        if [ "$ahead" -eq 0 ]; then
            log "guardian [$class] · impl made no commit"
            guardian_failed "$class" "$gdesc" "$gbr" "impl no-commit. design: $(printf '%s' "$design_text" | tr '\n' ' ' | cut -c1-200) · notes: $(tail -10 "$glog" 2>/dev/null | tr '\n' ' ' | cut -c1-300)"
            git worktree remove --force "$gwt" 2>/dev/null; git branch -D "$gbr" 2>/dev/null; continue
        fi

        # ── v4 stage 3: Opus REVIEW — BEFORE the expensive gate (failures are review-dominated) ──
        phase review opus
        review="$(agent "$GUARDIAN_MODEL" "You are an ADVERSARIAL reviewer of a $class change on branch $gbr (diff: git diff $BASE..$gbr). REFUTE it — $(reviewer_angle "$class"). Read the diff + the added tests. If you find ANY unsoundness / behaviour change vs the ../sky reference / disguised hack, print 'REVIEW: REJECT <why>'. Only if you cannot break it after genuine effort, print 'REVIEW: ACCEPT'. Default to REJECT when uncertain." | tee "$rlog")"
        if ! printf '%s' "$review" | rg -q 'REVIEW: ACCEPT'; then
            log "guardian [$class] · review → REJECT"
            guardian_failed "$class" "$gdesc" "$gbr" "review REJECT: $(reason_of "$review")"
            git worktree remove --force "$gwt" 2>/dev/null; git branch -D "$gbr" 2>/dev/null; continue
        fi

        # ── v4 stage 4: INTEGRATE — the ONE cargo test --workspace + clippy + fuzz ──
        phase gate ·
        git switch "$BASE" >/dev/null 2>&1
        gpre="$(git rev-parse HEAD)"   # pre-merge sha; a failed gate reverts HERE, never a stale HEAD
        if git merge --no-ff -m "autopilot: $class fix — $gdesc" "$gbr" >/dev/null 2>&1 \
           && ( touch runtime/tests/*.rs crates/skyc/tests/*.rs 2>/dev/null; CARGO_TARGET_DIR="$GATE_TARGET" timeout 3000 cargo test --workspace >/tmp/autopilot-gate.log 2>&1 && CARGO_TARGET_DIR="$GATE_TARGET" timeout 1200 cargo clippy --workspace --all-targets -- -D warnings >>/tmp/autopilot-gate.log 2>&1 ) \
           && ( "$FUZZ" --iters "$FUZZ_ITERS" --quiet >>/tmp/autopilot-gate.log 2>&1 ); then
            log "guardian [$class] · LANDED (gate-green + fuzz-clean)"; mark LANDED "$class" "$gdesc"
        else
            log "guardian [$class] · gate RED — reverting to $gpre"
            git merge --abort 2>/dev/null; git reset --hard "$gpre" >/dev/null 2>&1
            guardian_failed "$class" "$gdesc" "$gbr" "gate failed after merge. tail: $(tail -12 /tmp/autopilot-gate.log 2>/dev/null | tr '\n' ' ' | cut -c1-400)"
        fi
        git worktree remove --force "$gwt" 2>/dev/null; git branch -D "$gbr" 2>/dev/null
    done
    git worktree prune
    # persistent per-cycle cost ledger — cumulative agent $ from this run's logs,
    # recorded each cycle so a later run's /tmp overwrite can't erase the trend.
    { printf '%s\t%s\tc%s\t' "$(date +%FT%H:%M)" "${START_SHA:0:7}" "$cycle"
      for f in /tmp/autopilot-guardian-*.log /tmp/autopilot-triage-*.log /tmp/autopilot-audit-*.log; do
          [ -f "$f" ] && rg '"type":"result"' "$f" 2>/dev/null | tail -1
      done | jq -rs '([.[] | .total_cost_usd // 0] | add // 0) | "$" + ((.*100|floor)/100|tostring)'
    } >> "$LEDGER" 2>/dev/null || true
done

# ── landed digest for the human meta-audit ───────────────────────────────────
{
    echo "# Autopilot run digest — $(date -Is)"
    echo ""; echo "Base $START_SHA → $(git rev-parse --short HEAD).  Review before trusting the guardian-tier commits."
    echo ""; echo "## Landed this run"; git log --oneline "$START_SHA"..HEAD | sed 's/^/- /'
    echo ""; echo "## Queue tail"; tail -15 "$QUEUE"
} > "$DIGEST"
log "done. digest → $DIGEST (audit the guardian-tier commits). queue → $QUEUE"
