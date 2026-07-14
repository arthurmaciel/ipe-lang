#!/usr/bin/env bash
# autopilot.sh — the autonomous development loop over the backlog (backlog.jsonl).
#
#   pick pending item → design → impl → adversarial review → gate → audit → repeat
#
# The WORK SOURCE is backlog.jsonl (the SSOT), NOT a sweep. There is NO remeasure/
# triage refill and NO mechanical-vs-guardian tier: EVERY pending item runs the SAME
# design→impl→review→gate pipeline. It runs until every actionable backlog item is
# either soundly fixed or ESCALATED to a human — then STOPS. It never invents work.
#
#   pending backlog item? → route by CLASS (a heuristic that only tailors the
#                           design/review FOCUS + preserves the security HARD-RULE):
#                           type-system / runtime / security. Dispatch a
#                           class-specialised Opus guardian (worktree) + a
#                           class-specific adversarial review + the FUZZER gate.
#   after each landed item → audit the landed digest (Opus, adversarial).
#   nothing actionable?   → TERMINAL: stop, emit the landed digest for human audit.
#
# SOUNDNESS NOTE: cargo test alone is NOT a sufficient oracle for type-system work.
# Every item is verified at soundness-grade: an independent adversarial review (a
# second Opus told to REFUTE the fix) AND the no-panic fuzzer (scripts/
# fuzz-well-typed.sh — proven to catch a real panic) at the integrate gate. Never
# trust the gate alone. The human keeps a LIGHT meta-audit via the digest.
#
# VERIFICATION MODEL (2026-07-14, distilled from the manual burndown):
#   * The trust boundary is a SCRIPT'S captured exit code, never an agent's
#     narration. An agent can report "999/999 passed" while a buried `if false &&`
#     disabled the whole fix — that survived precisely because a *report* was
#     trusted. So agents supply CODE + adversarial PROBES (the creative part an
#     LLM is for); the SCRIPT runs the build/test/SEAL and branches on `$?` (the
#     part that must not be delegated). The gate below (`&&`-chained nextest +
#     --features full + doctest + clippy + fuzz, revert-on-fail) is that oracle.
#   * THE SEAL is the gate oracle: skyc-exit-0 MUST imply the emitted crate
#     cargo-builds. A "fix" that only an agent's report closes but not for real
#     fails the gate (nextest + fuzz), is reverted, and re-queued PENDING — the
#     loop self-corrects, deterministically, off the ledger not a re-sweep.
#   * Adversarial review = build the COMMITTED branch (cache-hits on unchanged
#     code are fine — recompiling identical deterministic source yields the same
#     binary, zero signal), then RUN the SEAL + tests + the reviewer's OWN probes.
#     Integrity comes from running-from-committed + probing, NOT from recompiling.
#
# Config (env): PROGDEV_MAX_CYCLES (100 backstop) · PROGDEV_MAX_GUARDIAN (2/cycle) ·
#   PROGDEV_LANES (2) · PROGDEV_AUTHOR_MODEL (opus 4.8) · PROGDEV_GUARDIAN_MODEL /
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
AUTHOR_MODEL="${PROGDEV_AUTHOR_MODEL:-claude-opus-4-8}"     # all stages default to Opus 4.8 (2026-07-14); override via env
DESIGN_MODEL="${PROGDEV_DESIGN_MODEL:-claude-opus-4-8}"     # Fable NO LONGER AUTHORIZED — Opus 4.8 (override via PROGDEV_DESIGN_MODEL)
GATE_TARGET="${MASTER_GATE_TARGET:-$HOME/.cache/master-gate-target}"
QUEUE="docs/architecture/progressive-development-queue.tsv"   # ATTEMPT LEDGER only (mark/attempts_of) — NOT the work source
BACKLOG="$HERE/backlog.jsonl"                                 # THE work source: pending items (SSOT)
# Every pending backlog item runs the SAME pipeline (design→impl→review→gate) —
# there is NO mechanical-vs-guardian tier. This heuristic only tailors the
# design/review FOCUS + preserves the security HARD-RULE scrutiny.
classify() { case "$(printf '%s' "$1" | tr 'A-Z' 'a-z')" in
  *auth*|*secret*|*crypto*|*password*|*sql*|*unsafe*|*ffi*|*token*|*jwt*) printf 'guardian-security' ;;
  *panic*|*e0382*|*e0507*|*e0277*|*e0308*|*cargo-fail*|*seal*|*runtime*|*oracle*) printf 'guardian-runtime' ;;
  *) printf 'guardian-typesystem' ;;
esac; }
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
#
# TARGET-DIR RULE (2026-07-14, empirically proven — /tmp/skyc-share-exp):
#   Two build PHASES with opposite sharing rules, and conflating them is the
#   dominant wasted compute in the loop:
#   1. COMPILER-crate build (sky_lower/sky_backend/skyc — SOURCE DIFFERS per lane):
#      MUST use an ISOLATED CARGO_TARGET_DIR (GUARDIAN_TARGET / GATE_TARGET). Two
#      lanes building different source of the SAME crate to one target = the
#      Task-13 stale-link clobber (the lock serializes the builds but the last
#      writer's artifact wins → the other lane reads the wrong backend).
#   2. EMITTED-E2E-PROJECT build (vendored `sky_runtime` + tokio/sqlx/axum + a
#      UNIQUELY-named emitted app crate): SHOULD use the SHARED sky-rust-target so
#      the ~8GB runtime compiles ONCE and is reused across every example, every
#      lane, author AND reviewer. PROVEN race-free: cargo's build-dir file lock
#      serialises concurrent access (both logs showed `Blocking waiting for file
#      lock`, both exit 0, tokio NOT recompiled), because the only shared crate
#      (sky_runtime) is byte-identical source across non-runtime lanes and the app
#      crates have unique names. EXCEPTION: a change that edits runtime/ vendors a
#      DIFFERENT runtime → its emitted build must isolate too (case-1 clobber).
#   MECHANISM: the E2E harness (tools/oracle) inherits the ambient CARGO_TARGET_DIR,
#   so today's `CARGO_TARGET_DIR=$GUARDIAN_TARGET` on the impl step ALSO redirects
#   the emitted build into the isolated target → a fresh runtime compile per lane
#   (the self-inflicted slow path). The real fix is in the oracle (pin the emitted
#   build to the shared target unless runtime/ changed) — filed as its own task;
#   until it lands, sccache (rustc-wrapper, global) reclaims most of the dep cost.
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
  PROGDEV_AUTHOR_MODEL    (claude-opus-4-8)     implementer (impl-stage) model
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

# Agent dispatch. EVERY autopilot agent (design/impl/guardian/audit/review) is
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
        --allowedTools 'Bash(cargo *)' 'Bash(git *)' 'Bash(skyc *)' 'Bash(rg *)' 'Bash(skydex *)' 'Bash(ipe-index *)' \
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
        [ "$keep" -eq 1 ] && continue
        # 2026-07-13: this call site is now UNCONDITIONAL (fired proactively
        # after every landed mechanical batch / guardian item, not just when
        # disk_free_gb dips below DISK_FLOOR) — see the two call sites'
        # comments. That makes the pre-existing no-liveness-check gap a real
        # hazard it wasn't before: a concurrent orchestrate.sh lane can have
        # its OWN CARGO_TARGET_DIR (not in $KEEP_TARGETS) actively open at
        # the exact moment a DIFFERENT lane's completion triggers this
        # reclaim. Skip anything a live process still references — mirrors
        # disk-guard.sh's path_is_live() gate; a live writer always wins
        # over a disk-space goal.
        if pgrep -f -- "$d" >/dev/null 2>&1; then
            log "  skip $(basename "$d") (live process still references it)"
            continue
        fi
        log "  rm $(basename "$d") ($(du -sh "$d" 2>/dev/null | cut -f1))"; rm -rf "$d"
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
# skydex wrapper on the agent PATH: code-relation index over the ../sky READ-ONLY
# reference (Sky↔Haskell↔Go↔Rust routes; parity/rdeps/deps/covers/locate). Runs
# from ../sky where .skydex/index.db lives; read-only ref → never stale.
if [ -x "$REPO/../sky/tools/skydex/target/release/skydex" ]; then
    printf '#!/usr/bin/env bash\ncd "%s/../sky" && exec ./tools/skydex/target/release/skydex "$@"\n' "$REPO" > "$SHIMDIR/skydex"
    chmod +x "$SHIMDIR/skydex"
fi
# ipe-index on the agent PATH: OUR project's Rust def index (def/refs/kind) — agents
# query "where is X defined" instead of sifting rg text hits. Rebuild each run so
# it reflects landed fixes (sqlite, ~1s). Complements skydex (reference) / rg (fallback).
if [ -x "$REPO/scripts/ipe-index" ]; then
    ln -sf "$REPO/scripts/ipe-index" "$SHIMDIR/ipe-index"
    "$REPO/scripts/ipe-index" build >/dev/null 2>&1 &
fi
mkdir -p "$(dirname "$LEDGER")"; [ -f "$LEDGER" ] || printf 'stamp\trun\tcycle\tcum_cost\n' > "$LEDGER"
ensure_disk || die "disk critically low even after reclaim ($(disk_free_gb)G < ${DISK_CRIT}G) — free space and retry"
[ -f "$STOP" ] && die "kill-switch $STOP present"
if ! ( set -o noclobber; echo "$$" > "$LOCK" ) 2>/dev/null; then
    # Lock exists. Is its owner still alive? A prior run SIGKILLed (or the host
    # rebooted) never fires its EXIT trap, leaving a STALE lock that would block
    # every future run forever. Reclaim iff the recorded PID is dead.
    lpid="$(cat "$LOCK" 2>/dev/null)"
    if [ -n "$lpid" ] && kill -0 "$lpid" 2>/dev/null; then
        die "another autopilot (pid $lpid) holds $LOCK"
    fi
    log "stale $LOCK (owner pid ${lpid:-?} is dead) — reclaiming"
    rm -f "$LOCK"
    ( set -o noclobber; echo "$$" > "$LOCK" ) 2>/dev/null || die "raced on $LOCK while reclaiming — another autopilot just started; retry"
fi

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

# Ledger helpers ($QUEUE is now ONLY the attempt ledger, NOT the work source —
# the work source is $BACKLOG). Every item gets up to GUARDIAN_ATTEMPTS *thoughtful*
# tries — each RESUMES from the prior attempt's saved artifact rather than restarting
# cold — then is ESCALATED to a human. ESCALATED = final (suppressed from pending()).
# This is what lets the loop converge while still giving hard items a real effort.
pending() { # THE work source: every actionable backlog.jsonl item → "<class>\t#<id> <task>"
    # actionable = status pending + blockers resolved + not ESCALATED + < GUARDIAN_ATTEMPTS tries.
    # ONE uniform stream (no mechanical/guardian tier); class only tailors focus.
    command -v jq >/dev/null 2>&1 || return 0
    jq -r 'select((.id//"")!="" and .status=="pending" and ((.blocked_by//[])|length==0))
           | "#\(.id) " + ((.task//"") | gsub("[\n\t]";" "))' "$BACKLOG" 2>/dev/null \
      | while IFS= read -r d; do
          [ -z "$d" ] && continue
          # skip if escalated (dead) in the ledger, or already tried GUARDIAN_ATTEMPTS times
          x="$d" awk -F'\t' 'BEGIN{x=ENVIRON["x"]} $3==x && $1=="ESCALATED"{f=1} END{exit f}' "$QUEUE" 2>/dev/null || continue
          [ "$(attempts_of "$d")" -lt "${GUARDIAN_ATTEMPTS:-2}" ] && printf '%s\t%s\n' "$(classify "$d")" "$d"
        done
}
mark() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> "$QUEUE"; }   # status kind desc
attempts_of() { d="$1" awk -F'\t' 'BEGIN{d=ENVIRON["d"]} $3==d && ($1=="ATTEMPTED"||$1=="BLOCKED"){n++} END{print n+0}' "$QUEUE"; }
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
    act="$(pending | wc -l | tr -d ' ')"
    if [ -n "$prev_head" ] && [ "$cur_head" = "$prev_head" ] && [ "$act" -eq 0 ]; then
        dry=$((dry+1)); log "no-progress pass $dry/2 (nothing landed, no tractable findings)"
        [ "$dry" -ge 2 ] && { log "converged — 2 dry passes; work done or human-blocked. stopping."; break; }
    else
        dry=0
    fi
    prev_head="$cur_head"
    log "cycle $cycle (dry=$dry, actionable=$act)"

    # 1 ── (mechanical tier ABOLISHED) — every backlog item runs the SAME
    #       design→impl→review→gate pipeline below; no orchestrate.sh fast-path.

    # 2 ── audit what landed (adversarial soundness review of new commits) ────
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

    # 2b ── (remeasure sweep + fuzz + triage ABOLISHED) — the work source is
    #        backlog.jsonl, so there is no sweep to discover/refill items.
    # 3 ── dispatch: the UNIFIED pipeline over pending backlog items ──────────
    mapfile -t guard < <(pending)   # each entry: "<class>\t#<id> <task>" (class = focus only)
    if [ "${#guard[@]}" -eq 0 ]; then log "nothing actionable this pass"; continue; fi
    log "${#guard[@]} backlog item(s) actionable. Dispatching up to $MAX_GUARDIAN this run (same pipeline for all)."
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
            design="$(agent "$DESIGN_MODEL" "You are a compiler guardian DESIGNER specialising in $class. Do NOT write code. Produce a concise root-cause + fix PLAN: (a) the root cause, (b) the exact crates/files/functions to change, (c) the approach — matching the ../sky READ-ONLY reference, root-cause only, NEVER a hack/fixture-edit/gate-weakening, (d) the regression test to add. FOCUS: $(guardian_focus "$class"). Item: $gdesc .$(resume_hint "$gdesc") If there is NO sound fix (needs a human decision, genuinely multi-session, or would require a hack), print exactly 'DESIGN: ESCALATE <why>' and nothing else. Otherwise print 'DESIGN: <the plan>'." | tee "$dlog")"
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

        # ── v4 stage 2: IMPL (Opus) — follows the plan FILE ($dfile), never argv
        # (PROGDEV_STREAM=1 $design is many-KB stream-json → ARG_MAX E2BIG). On a
        # retry resume_hint points impl at the prior diff + why it was rejected.
        phase impl opus
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
        review="$(agent "$GUARDIAN_MODEL" "You are an ADVERSARIAL reviewer of a $class change on branch $gbr (diff: git diff $BASE..$gbr). REFUTE it — $(reviewer_angle "$class"). Read the diff + the added tests, THEN VERIFY WITH YOUR OWN HANDS: build skyc from THIS branch and cargo-build your OWN probe programs — confirm skyc-exit-0 ⇒ cargo-exit-0 (THE SEAL) yourself. The impl's self-report is NOT evidence (a fix can report green while a buried guard disabled it, or while it emits skyc-0-then-cargo-fail on a shape the impl never tested). Cache-hits on unchanged code are fine — recompiling identical source is zero-signal; integrity comes from RUNNING the committed source + your probes. If you find ANY unsoundness / behaviour change vs the ../sky reference / disguised hack / skyc-0-then-cargo-fail, print 'REVIEW: REJECT <why + the exact repro>'. Only if you cannot break it after genuine effort, print 'REVIEW: ACCEPT'. Default to REJECT when uncertain." | tee "$rlog")"
        if ! printf '%s' "$review" | rg -q 'REVIEW: ACCEPT'; then
            log "guardian [$class] · review → REJECT"
            guardian_failed "$class" "$gdesc" "$gbr" "review REJECT: $(reason_of "$review")"
            git worktree remove --force "$gwt" 2>/dev/null; git branch -D "$gbr" 2>/dev/null; continue
        fi

        # ── v4 stage 4: INTEGRATE — nextest + doctests + clippy + fuzz, on NIGHTLY ──
        # nextest run replaces `cargo test` (faster scheduler + better isolation) but
        # does NOT run doctests → a `--doc` pass follows. The `--features full`
        # runtime lane is LOAD-BEARING (gate blind spot, 2026-07-11): the runtime's
        # `default = []` and workspace feature-unification never enables `live`/
        # `db`/`tui`/…, so the workspace run silently skips every feature-gated
        # test — incl. the whole `sky_runtime::live::*` surface (style_inject
        # CSS-injection sink gates, SSE/session/dispatch) and the spawn_blocking
        # regressions. Mirrors CI's `runtime-full-features` job. Gate runs on nightly
        # with RUSTFLAGS="mold link + -Zthreads=8" (parallel rustc frontend — which IS
        # clippy — so it speeds clippy AND the test builds; mold speeds the link).
        # GRF replaces the config's rustflags (must re-include mold). clippy dropped
        # --all-targets (tests already run via nextest) + --no-deps + --jobs 4.
        # NOTE: gate is nightly; impl self-checks are stable — a nightly-only lint on
        # agent code can red a gate (rare drift), then re-queues. GATE_TARGET rebuilds
        # once on the stable→nightly switch, then stays nightly.
        GRF="-C link-arg=-fuse-ld=mold -Zthreads=8"
        phase gate ·
        git switch "$BASE" >/dev/null 2>&1
        gpre="$(git rev-parse HEAD)"   # pre-merge sha; a failed gate reverts HERE, never a stale HEAD
        if git merge --no-ff -m "autopilot: $class fix — $gdesc" "$gbr" >/dev/null 2>&1 \
           && ( touch runtime/tests/*.rs crates/skyc/tests/*.rs 2>/dev/null; \
                RUSTFLAGS="$GRF" CARGO_TARGET_DIR="$GATE_TARGET" timeout 3000 cargo +nightly nextest run --workspace >/tmp/autopilot-gate.log 2>&1 \
                && RUSTFLAGS="$GRF" CARGO_TARGET_DIR="$GATE_TARGET" timeout 1800 cargo +nightly nextest run -p sky-runtime-rust --features full >>/tmp/autopilot-gate.log 2>&1 \
                && RUSTFLAGS="$GRF" CARGO_TARGET_DIR="$GATE_TARGET" timeout 600 cargo +nightly test --workspace --doc >>/tmp/autopilot-gate.log 2>&1 \
                && RUSTFLAGS="$GRF" CARGO_TARGET_DIR="$GATE_TARGET" timeout 1200 cargo +nightly clippy --workspace --no-deps --jobs 4 -- -D warnings >>/tmp/autopilot-gate.log 2>&1 ) \
           && ( "$FUZZ" --iters "$FUZZ_ITERS" --quiet >>/tmp/autopilot-gate.log 2>&1 ); then
            log "guardian [$class] · LANDED (gate-green + fuzz-clean)"; mark LANDED "$class" "$gdesc"
        else
            log "guardian [$class] · gate RED — reverting to $gpre"
            git merge --abort 2>/dev/null; git reset --hard "$gpre" >/dev/null 2>&1
            guardian_failed "$class" "$gdesc" "$gbr" "gate failed after merge. tail: $(tail -12 /tmp/autopilot-gate.log 2>/dev/null | tr '\n' ' ' | cut -c1-400)"
        fi
        git worktree remove --force "$gwt" 2>/dev/null; git branch -D "$gbr" 2>/dev/null
        reclaim_disk   # proactive per-item hygiene — see the mechanical-batch call site's comment
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
