#!/usr/bin/env bash
# autopilot.sh — the autonomous development loop over the backlog (backlog.jsonl).
#
#   pick pending item → design → impl → adversarial review → gate → audit → repeat
#
# The WORK SOURCE is backlog.jsonl (the SSOT), NOT a sweep. There is NO remeasure/
# triage refill and NO tier distinction: EVERY pending item runs the SAME
# design→impl→review→gate pipeline. It runs until every actionable backlog item is
# either soundly fixed or ESCALATED to a human — then STOPS. It never invents work.
#
#   pending backlog item? → route by CLASS (a heuristic that only tailors the
#                           design/review FOCUS + preserves the security HARD-RULE):
#                           type-system / runtime / security. Dispatch a
#                           class-specialised Opus agent (worktree) + a
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
# Config (env): PROGDEV_MAX_CYCLES (100 backstop) · PROGDEV_LANES (2 items/cycle,
#   sequential) · PROGDEV_AUTHOR_MODEL (opus 4.8) · PROGDEV_REVIEW_MODEL /
#   touch autopilot.stop to halt after the current phase.
set -uo pipefail
cd "$(dirname "$0")/../.."
REPO="$(pwd)"
HERE="scripts/progressive-development"

MAX_CYCLES="${PROGDEV_MAX_CYCLES:-100}"   # runaway backstop only; real stop = 2-dry-pass convergence
# PROGDEV_LANES = concurrent authoring lanes per cycle. Each lane authors
# (design→impl→review) in its OWN worktree + OWN cargo target, in parallel; the
# GATE (git switch/merge/nextest) is SERIAL — one lane integrates at a time, so
# the shared checkout stays single-writer. 2 is the safe max on a ~15 GB box.
LANES="${PROGDEV_LANES:-2}"
FUZZ_ITERS="${PROGDEV_FUZZ_ITERS:-30}"       # no-panic fuzzer iters (measure sweep + gate)
FUZZ="scripts/fuzz-well-typed.sh"
STREAM="${PROGDEV_STREAM:-1}"                 # DEFAULT ON (watch.sh renders it); PROGDEV_STREAM=0 to disable. Safe: logic is grep/queue-based
WATCH="${PROGDEV_WATCH:-1}"                   # 1 = auto-launch watch.sh alongside (one terminal); 0 / --no-watch disables
CONTEXT="$REPO/$HERE/context.md"             # operating contract: 6 principles + 2 rules + the seal
REVIEW_MODEL="${PROGDEV_REVIEW_MODEL:-claude-opus-4-8}"
AUTHOR_MODEL="${PROGDEV_AUTHOR_MODEL:-claude-opus-4-8}"     # all stages default to Opus 4.8 (2026-07-14); override via env
DESIGN_MODEL="${PROGDEV_DESIGN_MODEL:-claude-opus-4-8}"     # Fable NO LONGER AUTHORIZED — Opus 4.8 (override via PROGDEV_DESIGN_MODEL)
# SANCTIONED CARGO-TARGET ROOT (2026-07-15): every cargo target the loop writes —
# gate, per-lane, shared-oracle — lives under $IPE_CACHE and NOWHERE else, so an
# emergency prune has ONE known root. Targets outside it (/tmp, $HOME root) filled
# the disk to 100% and crashed the loop; reclaim_disk sweeps strays as a safety net,
# but the write-boundary (DEVELOPMENT.md) forbids creating them at all.
IPE_CACHE="${PROGDEV_CACHE_ROOT:-$HOME/.cache/ipe}"
mkdir -p "$IPE_CACHE"
GATE_TARGET="${MASTER_GATE_TARGET:-$IPE_CACHE/gate-target}"
QUEUE="docs/architecture/progressive-development-queue.tsv"   # ATTEMPT LEDGER only (mark/attempts_of) — NOT the work source
BACKLOG="$HERE/backlog.jsonl"                                 # THE work source: pending items (SSOT)
BACKLOG_SH="$HERE/backlog.sh"                                 # claim/unclaim/close/defer (flock-serialized JSONL edits)
CLAIM_TTL="${PROGDEV_CLAIM_TTL:-14400}"                       # 4h lease: a claim older than this is stale → auto-reclaimed
CUR_CLAIMS=()                                                 # ids this cycle's lanes hold (released by EXIT trap; reset per cycle)
# Every pending backlog item runs the SAME pipeline (design→impl→review→gate) —
# there is NO tier distinction. This heuristic only tailors the
# design/review FOCUS + preserves the security HARD-RULE scrutiny.
classify() { case "$(printf '%s' "$1" | tr 'A-Z' 'a-z')" in
  *auth*|*secret*|*crypto*|*password*|*sql*|*unsafe*|*ffi*|*token*|*jwt*) printf 'security' ;;
  *panic*|*e0382*|*e0507*|*e0277*|*e0308*|*cargo-fail*|*seal*|*runtime*|*oracle*) printf 'runtime' ;;
  *) printf 'typesystem' ;;
esac; }
RESUME_DIR="docs/architecture/progressive-development-resume"  # gitignored resume artifacts
MAX_ATTEMPTS="${PROGDEV_MAX_ATTEMPTS:-2}"  # v4: 2 resuming attempts, then phase-4 (review-dominated failures → 3rd try low-yield)
DIGEST="docs/architecture/progressive-development-digest.md"
STOP="autopilot.stop"
LOCK=".autopilot.lock"
LANE_TARGET_BASE="$IPE_CACHE/lane"
DISK_FLOOR="${PROGDEV_DISK_FLOOR_GB:-20}"    # reclaim when free drops below this
DISK_CRIT="${PROGDEV_DISK_CRIT_GB:-10}"      # graceful-stop when still below this after reclaim
LANE_TARGET_MAX_GB="${PROGDEV_LANE_TARGET_MAX_GB:-60}"   # per-lane target hard size cap (regardless of total disk; 0 = off)
GATE_TARGET_MAX_GB="${PROGDEV_GATE_TARGET_MAX_GB:-30}"   # gate-target hard size cap — the full gate's incremental cruft grows UNBOUNDED across runs (hit 41G → 100%-disk crash 2026-07-15); purge past cap, rebuilds warm (0 = off)
# The loop's ENTIRE cargo-target footprint — everything else under ~/.cache is
# reclaimable. Bounds disk to these two + the shared dev target.
#
# TARGET-DIR RULE (2026-07-14, empirically proven — /tmp/skyc-share-exp):
#   Two build PHASES with opposite sharing rules, and conflating them is the
#   dominant wasted compute in the loop:
#   1. COMPILER-crate build (sky_lower/sky_backend/skyc — SOURCE DIFFERS per lane):
#      MUST use an ISOLATED CARGO_TARGET_DIR (LANE_TARGET_BASE / GATE_TARGET). Two
#      lanes building different source of the SAME crate to one target = the
#      Task-13 stale-link clobber (the lock serializes the builds but the last
#      writer's artifact wins → the other lane reads the wrong backend).
#   2. EMITTED-E2E-PROJECT build (vendored `sky_runtime` + tokio/sqlx/axum + a
#      UNIQUELY-named emitted app crate): SHOULD use the SHARED $IPE_CACHE/oracle-target so
#      the ~8GB runtime compiles ONCE and is reused across every example, every
#      lane, author AND reviewer. PROVEN race-free: cargo's build-dir file lock
#      serialises concurrent access (both logs showed `Blocking waiting for file
#      lock`, both exit 0, tokio NOT recompiled), because the only shared crate
#      (sky_runtime) is byte-identical source across non-runtime lanes and the app
#      crates have unique names. EXCEPTION: a change that edits runtime/ vendors a
#      DIFFERENT runtime → its emitted build must isolate too (case-1 clobber).
#   MECHANISM: the E2E harness (tools/oracle) inherits the ambient CARGO_TARGET_DIR,
#   so today's `CARGO_TARGET_DIR=$LANE_TARGET_BASE` on the impl step ALSO redirects
#   the emitted build into the isolated target → a fresh runtime compile per lane
#   (the self-inflicted slow path). The real fix is in the oracle (pin the emitted
#   build to the shared target unless runtime/ changed) — filed as its own task;
#   until it lands, sccache (rustc-wrapper, global) reclaims most of the dep cost.
KEEP_TARGETS="$(basename "$GATE_TARGET") oracle-target"   # under $IPE_CACHE: keep gate + shared-oracle; lane-* kept via prefix in reclaim_disk
LEDGER="docs/architecture/progdev-cost-ledger.tsv"   # persistent per-cycle agent-cost ledger (survives /tmp overwrite across runs)
SHIMDIR="/tmp/autopilot-shims"                        # rg-enforcement: grep/egrep/fgrep shims prepended to the agent PATH

# log() writes to a NARRATOR file (watch.sh tails it). It prints to the terminal
# ONLY when NO watch is attached — an auto-launched watch owns the tty with a
# scroll-region pinned header, and a second writer (raw log lines) truncates it.
NARRATOR="docs/architecture/progdev-narrator.log"
log()  {
    local line; line="$(date +%H:%M) | autopilot | $*"
    printf '%s\n' "$line" >> "$NARRATOR" 2>/dev/null
    { [ "${WATCH:-1}" != 1 ] || [ ! -t 1 ]; } && printf '%s\n' "$line"
    return 0
}
die()  {   # aborts always show on the terminal, watched or not
    local line; line="$(date +%H:%M) | autopilot | ABORT: $*"
    printf '%s\n' "$line" >> "$NARRATOR" 2>/dev/null; printf '%s\n' "$line"; exit 1
}

# PER-LANE status header — one file per concurrent lane so lane subshells never
# race on a shared file. watch.sh renders one row per LIVE lane (by file mtime).
# A lane context sets STATUS locally to its own file (default = lane 0, covers
# serial phases). start_epoch drives watch's elapsed-time column.
status_file() { printf 'docs/architecture/progdev-status-lane%s.txt' "$1"; }
STATUS="$(status_file 0)"
# CUR_FIRST_EPOCH: set once per item (first set_task call); never reset on
# phase transitions — watch.sh uses it to show TOTAL elapsed across retries.
CUR_FIRST_EPOCH=""
set_task() {
    CUR_TYPE="$1"; CUR_TASK="$(printf '%.110s' "$2")"; CUR_START="$(date +%H:%M)"
    CUR_START_EPOCH="$(date +%s)"; CUR_ATT="${3:-·}"
    # Persist first-seen epoch across attempts: set only when blank (new item)
    # or when the caller clears it (lane_author resets it at item start via
    # unset CUR_FIRST_EPOCH before set_task, so every new item gets a fresh epoch).
    [ -z "${CUR_FIRST_EPOCH:-}" ] && CUR_FIRST_EPOCH="$CUR_START_EPOCH"
}
phase() {  # <phase-name> <model>
    { printf 'task    %s\n' "${CUR_TASK:-·}"
      printf 'type    %s   attempt %s   started %s   start_epoch %s   first_seen_epoch %s\n' \
             "${CUR_TYPE:-·}" "${CUR_ATT:-·}" "${CUR_START:-·}" "${CUR_START_EPOCH:-0}" "${CUR_FIRST_EPOCH:-0}"
      printf 'phase   %-11s model %-8s now %s\n' "$1" "${2:-·}" "$(date +%H:%M)"; } > "$STATUS" 2>/dev/null
    log "· $1 (${2:-·})"
}

usage() {
    cat <<'EOF'
autopilot.sh — self-refilling autonomous development loop
  fix → measure(remeasure + no-panic fuzzer) → triage → mechanical-burn →
  burn → audit → repeat. Runs until only human-decision work remains,
  then STOPS and reports. Never manufactures busy-work.

Usage: scripts/progressive-development/autopilot.sh [--no-watch] [-h|--help]

Flags:
  --no-watch      don't auto-launch the live monitor (watch.sh) in this terminal
  -h, --help      show this help and exit

Env vars (defaults):
  PROGDEV_MAX_CYCLES      (100)    runaway backstop only (real stop = converge: 2 passes, no new findings)
  PROGDEV_LANES           (2)      items dispatched per cycle (SEQUENTIAL — single-writer design, not concurrent)
  PROGDEV_FUZZ_ITERS      (30)     no-panic fuzzer iters (measure sweep + gate)
  PROGDEV_STREAM          (1)      agents emit stream-json for the live view; 0 = plain-text logs
  PROGDEV_WATCH           (1)      auto-launch watch.sh alongside (one terminal); 0 = don't (== --no-watch)
  PROGDEV_AUTHOR_MODEL    (claude-opus-4-8)     implementer (impl-stage) model
  PROGDEV_REVIEW_MODEL  (claude-opus-4-8)     review / audit model
  PROGDEV_LANES_DISK_MIN  (30)     free-GB floor below which a cycle drops to 1 lane
  PROGDEV_LANE_TARGET_MAX_GB (60)  per-lane cargo target size cap; over → purged (0 = off)
  MASTER_GATE_TARGET      ($IPE_CACHE/gate-target)         isolated gate target dir

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
    [ -x scripts/guards/mem-guard.sh ] || die "scripts/guards/mem-guard.sh missing/not executable — cannot start the memory kill-switch"
    log "mem-guard.sh not running — dispatching it (memory kill-switch)"
    nohup ./scripts/guards/mem-guard.sh >/tmp/mem-guard.out 2>&1 & disown
    for _ in 1 2 3 4 5; do
        pgrep -f mem-guard.sh >/dev/null 2>&1 && { log "mem-guard.sh up (pid $(pgrep -f mem-guard.sh | head -1))"; return 0; }
        sleep 1
    done
    die "mem-guard.sh failed to start within 5s (see /tmp/mem-guard.out)"
}

# Agent dispatch. EVERY autopilot agent (design/impl/audit/review) is
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
    log "disk reclaim (free $(disk_free_gb)G) — keeping only [$KEEP_TARGETS] under $IPE_CACHE"
    # ALL loop cargo targets live under $IPE_CACHE (the single sanctioned root). Keep
    # the gate + shared-oracle targets and the warm per-lane targets; reap the rest.
    for d in "$IPE_CACHE"/*; do
        [ -d "$d" ] || continue
        local keep=0 k bn; bn="$(basename "$d")"
        for k in $KEEP_TARGETS; do [ "$bn" = "$k" ] && keep=1; done
        # per-lane cargo targets (lane-1, lane-2, …) stay WARM across cycles;
        # NORMAL reclaim keeps them — they are purged only at DISK_CRIT (ensure_disk).
        case "$bn" in lane-*) keep=1 ;; esac
        [ "$keep" -eq 1 ] && continue
        # No-liveness-check would be a hazard: this reclaim is UNCONDITIONAL (fired
        # after every landed item), so a concurrent lane's OWN target could be open
        # when a DIFFERENT lane's completion triggers it. Skip anything a live
        # process references — a live writer always wins over a disk-space goal.
        if pgrep -f -- "$d" >/dev/null 2>&1; then
            log "  skip $(basename "$d") (live process still references it)"
            continue
        fi
        log "  rm $(basename "$d") ($(du -sh "$d" 2>/dev/null | cut -f1))"; rm -rf "$d"
    done
    # SAFETY NET — reap cargo targets OUTSIDE the sanctioned root: legacy
    # ~/.cache/*target* + any /tmp/*target* a manual agent dropped (CACHEDIR.TAG-
    # marked, pgrep-guarded). DEVELOPMENT.md's write-boundary forbids creating these,
    # but sweeping them means a stray one can never fill the disk again (2026-07-15).
    local stray
    for stray in "$HOME"/.cache/*target* $(find /tmp -maxdepth 2 -name CACHEDIR.TAG 2>/dev/null | sed 's|/CACHEDIR.TAG$||'); do
        [ -d "$stray" ] || continue
        case "$stray" in "$IPE_CACHE"/*) continue ;; esac   # sanctioned root handled above
        pgrep -f -- "$stray" >/dev/null 2>&1 && continue
        log "  rm STRAY $stray ($(du -sh "$stray" 2>/dev/null | cut -f1))"; rm -rf "$stray"
    done
    git worktree prune 2>/dev/null
    rm -rf /tmp/sky-fuzz.* /tmp/sky-fuzz-neg.* /tmp/orch-*.log 2>/dev/null
    log "  free now $(disk_free_gb)G"
}
# Hard size cap per lane target — deletes a lane-<i> that has bloated
# past LANE_TARGET_MAX_GB regardless of total free disk (incremental cruft grows
# unbounded across cycles otherwise). Runs at cycle top, when no lane holds a
# target; the pgrep check is belt-and-suspenders. It rebuilds warm next cycle.
prune_oversize_lane_targets() {
    [ "${LANE_TARGET_MAX_GB:-0}" -gt 0 ] 2>/dev/null || return 0
    local d sz
    for d in "$IPE_CACHE"/lane-*; do
        [ -d "$d" ] || continue
        pgrep -f -- "$d" >/dev/null 2>&1 && continue    # never delete a live build's target
        sz="$(du -sBG "$d" 2>/dev/null | cut -f1 | tr -dc '0-9')"
        [ -n "$sz" ] || continue
        if [ "$sz" -ge "$LANE_TARGET_MAX_GB" ]; then
            log "lane target $(basename "$d") = ${sz}G ≥ ${LANE_TARGET_MAX_GB}G cap — purging (rebuilds warm next cycle)"
            rm -rf "$d"
        fi
    done
}
# Hard size cap on the GATE target — the full-gate build (whole-workspace nextest +
# --features full + doctest + clippy + E2E emitted projects) accumulates incremental
# artefacts UNBOUNDED across cycles (bloated to 41G → filled a 217G disk to 100% and
# crashed the loop, 2026-07-15). reclaim_disk KEEPS the gate target (it is load-bearing
# during a gate), so nothing else bounds it. Purge past the cap at cycle top, when no
# gate is running (the pgrep guard is belt-and-suspenders); it rebuilds warm next full
# gate. Runs BEFORE the disk floor is consulted so a slow bloat never reaches ENOSPC.
prune_oversize_gate_target() {
    [ "${GATE_TARGET_MAX_GB:-0}" -gt 0 ] 2>/dev/null || return 0
    [ -d "$GATE_TARGET" ] || return 0
    pgrep -f -- "$GATE_TARGET" >/dev/null 2>&1 && return 0    # never delete a live gate's target
    local sz; sz="$(du -sBG "$GATE_TARGET" 2>/dev/null | cut -f1 | tr -dc '0-9')"
    [ -n "$sz" ] || return 0
    if [ "$sz" -ge "$GATE_TARGET_MAX_GB" ]; then
        log "gate target = ${sz}G ≥ ${GATE_TARGET_MAX_GB}G cap — purging (rebuilds warm at next full gate)"
        rm -rf "$GATE_TARGET"
    fi
}
# reclaim if below floor; return 1 (caller stops) if still critical afterwards.
ensure_disk() {
    [ "$(disk_free_gb)" -ge "$DISK_FLOOR" ] && return 0
    log "low disk ($(disk_free_gb)G < ${DISK_FLOOR}G)"; reclaim_disk
    # CRITICAL tier: if a NORMAL reclaim (which keeps the warm per-lane targets)
    # left us still below DISK_CRIT, the per-lane targets are the last
    # thing to sacrifice — purge them (skipping any a live build still holds).
    # This is the "each lane its own target, deleted ONLY on disk ENOSPC" rule.
    if [ "$(disk_free_gb)" -lt "$DISK_CRIT" ]; then
        log "still critical ($(disk_free_gb)G < ${DISK_CRIT}G) — purging warm per-lane targets"
        for d in "$IPE_CACHE"/lane-*; do
            [ -d "$d" ] || continue
            pgrep -f -- "$d" >/dev/null 2>&1 && { log "  keep $(basename "$d") (live)"; continue; }
            log "  rm $(basename "$d")"; rm -rf "$d"
        done
    fi
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
# cargo shim: headless claude -p ignores PreToolUse hooks, so enforce the
# `cargo fmt`-write ban here too — block it, pass everything else to the real
# cargo. Escape hatch: SKY_ALLOW_FMT=1 (the one sanctioned workspace pass, #214).
cat > "$SHIMDIR/cargo" <<'CARGOSHIM'
#!/usr/bin/env bash
case " $* " in
  *" fmt "*|*" fmt")
    case " $* " in
      *" --check"*) ;;
      *) [ -n "$SKY_ALLOW_FMT" ] || { echo "cargo fmt is disabled for autopilot agents — it reformats the WHOLE workspace. Use rustfmt <exact file>, or cargo fmt --check, or SKY_ALLOW_FMT=1 for the one sanctioned pass." >&2; exit 2; } ;;
    esac ;;
esac
exec "$HOME/.cargo/bin/cargo" "$@"
CARGOSHIM
chmod +x "$SHIMDIR/cargo"
# pgrep shim: pgrep -f self-matches (its own cmdline / the invoking shell) →
# false positives every caller must reason around; steer to the bracket trick.
cat > "$SHIMDIR/pgrep" <<'PGREPSHIM'
#!/usr/bin/env bash
echo "pgrep is disabled for autopilot agents — it self-matches (its own cmdline / the invoking shell), producing false positives. Use the bracket trick: ps -eo pid,args | grep '[c]argo' (first char in [] so the grep process cannot match itself), or ps -eo args | grep -c '[c]argo' for a live count. Never poll in a loop — trust a foreground timeout command's exit code." >&2
exit 2
PGREPSHIM
chmod +x "$SHIMDIR/pgrep"
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
trap 'rm -f "$LOCK"; command -v advance_master >/dev/null 2>&1 && advance_master; [ "${#CUR_CLAIMS[@]}" -gt 0 ] && for _c in "${CUR_CLAIMS[@]}"; do "$BACKLOG_SH" unclaim "$_c" >/dev/null 2>&1; done; [ -n "$watch_pid" ] && { kill "$watch_pid" 2>/dev/null; pkill -P "$watch_pid" 2>/dev/null; }; log "exit"' EXIT
# SIGTERM/SIGINT → request the SAME emergency-graceful stop as `touch autopilot.stop`:
# the Phase-A poll (and the cycle-top / Phase-B checks) pick up the flag, salvage
# in-flight lane work, and exit via the EXIT trap. Turns `kill <pid>` / Ctrl-C into a
# graceful wind-down instead of an abrupt SIGKILL that orphans children + skips salvage.
trap 'touch "$STOP" 2>/dev/null; log "signal received — emergency-graceful stop requested (finishing safe point)"' TERM INT

MASTER="$(git rev-parse --abbrev-ref HEAD)"   # the human-facing branch autopilot advances (e.g. master)
START_SHA="$(git rev-parse HEAD)"
# autopilot integrates on its OWN branch in a dedicated worktree — the MAIN checkout
# is NEVER switched/merged/reset, so a human can edit $MASTER during a run. Master
# advances only by a fast-forward at cycle end (advance_master), under $MERGE_LOCK.
INTEGRATION="progressive-development/integration"
GATE_WT="$REPO/.progressive-development-wt/integration"
MERGE_LOCK="$REPO/.git/pd-master-merge.lock"
setup_integration_worktree() {
    if git -C "$GATE_WT" rev-parse --git-dir >/dev/null 2>&1; then
        git -C "$GATE_WT" reset --hard "$MASTER" >/dev/null 2>&1 && git -C "$GATE_WT" clean -fdq >/dev/null 2>&1
    else
        git worktree remove --force "$GATE_WT" 2>/dev/null || true; rm -rf "$GATE_WT"
        git worktree add --quiet -B "$INTEGRATION" "$GATE_WT" "$MASTER" \
          || die "could not create integration worktree at $GATE_WT"
    fi
}
setup_integration_worktree
BASE="$INTEGRATION"   # lanes cut from + merge into integration (all existing lane code unchanged)

# ── gate cadence: cheap per-lane gate lands on integration; the AUTHORITATIVE full
# gate (full_gate/certify_batch) runs every $FULL_GATE_EVERY CYCLES *or* the moment
# no pending work remains, whichever comes first. Master advances ONLY to a
# full-gate-certified sha ($LAST_FULL_GREEN). $BATCH = items cheap-landed but not yet
# certified (kept claimed; closed on full-green, re-queued on full-red).
FULL_GATE_EVERY="${PROGDEV_FULL_GATE_EVERY:-10}"   # cycles between full gates
LAST_FULL_GREEN="$(git -C "$GATE_WT" rev-parse HEAD)"   # == $MASTER at start (setup reset integration to it)
CYCLES_SINCE_FULL=0
BATCH=()
mkdir -p "$(dirname "$QUEUE")"; touch "$QUEUE"
log "start: master=$MASTER integration=$INTEGRATION start=$START_SHA max_cycles=$MAX_CYCLES lanes=$LANES"

# ── master-ref safety: autopilot advances $MASTER ONLY by fast-forward, at cycle
# boundaries, under $MERGE_LOCK — the one place it touches the main checkout.
resync_integration() {   # cycle START: pull any human commits on $MASTER into integration
    ( flock 9
      if ! git -C "$GATE_WT" merge --no-ff -m "autopilot: resync $MASTER into integration" "$MASTER" >/dev/null 2>&1; then
          git -C "$GATE_WT" merge --abort 2>/dev/null; exit 1
      fi
    ) 9>"$MERGE_LOCK"
    local rc=$?
    [ "$rc" -eq 0 ] || log "resync: $MASTER conflicts with integration (human overlap) — skipping cycle"
    return "$rc"
}
advance_master() {   # fast-forward $MASTER to the LAST FULL-GATE-CERTIFIED sha (never uncertified integration HEAD)
    [ -n "$LAST_FULL_GREEN" ] || return 0
    ( flock 9
      local head; head="$(git -C "$REPO" rev-parse --abbrev-ref HEAD 2>/dev/null)"
      if [ "$head" = "$MASTER" ]; then
          git -C "$REPO" merge --ff-only "$LAST_FULL_GREEN" >/dev/null 2>&1    # ff keeps the human's uncommitted non-overlapping edits
      else
          git -C "$REPO" push --quiet . "$LAST_FULL_GREEN:$MASTER" >/dev/null 2>&1   # $MASTER not checked out → advance the ref only, ff-only
      fi
    ) 9>"$MERGE_LOCK" \
      && log "advance: $MASTER ← integration (ff)" \
      || log "advance: ff DEFERRED ($MASTER moved mid-cycle or working-tree overlap) — next cycle's resync absorbs it"
}
# Opt-in mutual lock: a pre-commit hook that makes a HUMAN commit wait out
# autopilot's brief cycle-boundary ff (flocks the same $MERGE_LOCK). Correctness
# does NOT depend on it — advance_master already DEFERS if master moves — so it is
# pure window-narrowing. Skips a pre-existing non-ours hook (never clobbers).
install_merge_hook() {
    [ "${PROGDEV_INSTALL_MERGE_HOOK:-1}" = 0 ] && return 0
    command -v flock >/dev/null 2>&1 || return 0
    local hookdir hook; hookdir="$(git -C "$REPO" rev-parse --git-path hooks 2>/dev/null)" || return 0
    hook="$hookdir/pre-commit"
    if [ -e "$hook" ] && ! grep -q 'pd-master-merge.lock' "$hook" 2>/dev/null; then
        log "merge-hook: an existing pre-commit hook is present — NOT overwriting (add the flock line by hand, or PROGDEV_INSTALL_MERGE_HOOK=0)"; return 0
    fi
    mkdir -p "$hookdir"
    cat > "$hook" <<'HOOK'
#!/usr/bin/env bash
# progressive-development merge lock — serialize this commit with autopilot's brief
# cycle-boundary master fast-forward. Waits up to 30s for the lock, then proceeds.
L="$(git rev-parse --git-path pd-master-merge.lock 2>/dev/null)"
[ -e "$L" ] && command -v flock >/dev/null 2>&1 && flock -w 30 "$L" true 2>/dev/null
exit 0
HOOK
    chmod +x "$hook"
    log "merge-hook: installed pre-commit lock (opt out: PROGDEV_INSTALL_MERGE_HOOK=0)"
}
install_merge_hook

# Ledger helpers ($QUEUE is now ONLY the attempt ledger, NOT the work source —
# the work source is $BACKLOG). Every item gets up to MAX_ATTEMPTS *thoughtful*
# tries — each RESUMES from the prior attempt's saved artifact rather than restarting
# cold — then is ESCALATED to a human. ESCALATED = final (suppressed from pending()).
# This is what lets the loop converge while still giving hard items a real effort.
pending() { # THE work source: every actionable backlog.jsonl item → "<class>\t#<id> <task>"
    # actionable = status pending + blockers resolved + not ESCALATED + < MAX_ATTEMPTS tries.
    # ONE uniform stream (no tier distinction); class only tailors focus.
    command -v jq >/dev/null 2>&1 || return 0
    # Actionable work source comes from the backlog.sh interface (the schema
    # owner): pending, non-deferred, blockers-all-done — emitted as the exact
    # `#<id> <task>` string this loop uses as the $QUEUE ledger key. autopilot
    # keeps only its OWN concerns below (classify + ESCALATED/attempt suppression).
    "$BACKLOG_SH" ready 2>/dev/null \
      | while IFS= read -r d; do
          [ -z "$d" ] && continue
          # skip if escalated (dead) in the ledger, or already tried MAX_ATTEMPTS times
          x="$d" awk -F'\t' 'BEGIN{x=ENVIRON["x"]} $3==x && $1=="ESCALATED"{f=1} END{exit f}' "$QUEUE" 2>/dev/null || continue
          [ "$(attempts_of "$d")" -lt "${MAX_ATTEMPTS:-2}" ] && printf '%s\t%s\n' "$(classify "$d")" "$d"
        done
}
mark() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> "$QUEUE"; }   # status kind desc

# ── backlog claim lease (cross-process race guard) ───────────────────────────
# The backlog is shared by THIS autopilot and any hand-dispatched agent. Claiming
# flips an item's status pending→claimed so the other dispatcher's pending() skips
# it. bk() is best-effort (a claim failure must never crash the loop). id_of()
# pulls the leading "#NNN" that pending() prepends.
bk() { PROGDEV_RUN_ID="autopilot-$$" "$BACKLOG_SH" "$@" >/dev/null 2>&1 || true; }
id_of() { local t="${1%% *}"; printf '%s' "${t#\#}"; }   # "#170 #170 foo" → "170"
# Reclaim leases whose owner died without releasing (SIGKILL skips the EXIT trap).
# A claimed item is invisible to pending(); without this a crashed claimer would
# lock it forever. Mirrors the .autopilot.lock self-heal: liveness by TTL.
reclaim_stale_claims() {
    command -v jq >/dev/null 2>&1 || return 0
    local now; now="$(date +%s)"
    local id at ce
    while IFS=$'\t' read -r id at; do
        [ -n "$id" ] && [ -n "$at" ] || continue
        ce="$(date -d "$at" +%s 2>/dev/null)" || continue
        if [ "$(( now - ce ))" -ge "$CLAIM_TTL" ]; then
            bk unclaim "$id" && log "reclaimed stale claim on #$id (lease > ${CLAIM_TTL}s, owner gone)"
        fi
    done < <("$BACKLOG_SH" claims 2>/dev/null)   # claimed leases via the interface (schema owner)
}
attempts_of() { d="$1" awk -F'\t' 'BEGIN{d=ENVIRON["d"]} $3==d && ($1=="ATTEMPTED"||$1=="BLOCKED"){n++} END{print n+0}' "$QUEUE"; }
slug_of()     { printf '%s' "$1" | tr -cs 'A-Za-z0-9' '-' | sed 's/^-//;s/-*$//' | cut -c1-64; }
# Save what an attempt tried (diff + why it didn't land) so the NEXT
# attempt resumes instead of restarting cold. Gitignored → survives git reset.
save_resume() { # <class> <desc> <branch> <reason>
    mkdir -p "$RESUME_DIR"
    local f="$RESUME_DIR/$(slug_of "$2").md"
    { echo "# Resume: $2"
      echo "class=$1  attempts_used=$(( $(attempts_of "$2") + 1 ))/$MAX_ATTEMPTS  saved=$(date -Is)"
      echo; echo "## Why the last attempt did not land"; printf '%s\n' "$4" | head -40
      echo; echo "## Prior attempt diff (a STARTING point — the reviewer may have refuted it; do NOT blindly re-apply)"
      echo '```diff'; git diff "$BASE".."$3" 2>/dev/null | head -500; echo '```'
    } > "$f"
    log "  resume saved → $f"
}
resume_hint() { # <desc> → a prompt fragment if a prior attempt exists (ABSOLUTE path:
    # the lane runs in a worktree, but the artifact is gitignored in the main checkout)
    local f="$RESUME_DIR/$(slug_of "$1").md"
    [ -f "$f" ] && printf ' A PRIOR ATTEMPT exists at %s — READ it (its diff + why it failed) and CONTINUE from there; do NOT restart cold and do NOT repeat a refuted approach.' "$REPO/$f"
}
# One attempt failed: save it for resume, then re-queue (PENDING) if
# attempts remain, else ESCALATE to a human (artifact preserved).
reason_of() { printf '%s' "$1" | "$HERE/render-stream.sh" 2>/dev/null | tr '\n' ' ' | cut -c1-600; }  # agent stream-json → readable one-liner (resume reasons/logs)
item_failed() { # <class> <desc> <branch> <reason>
    save_resume "$1" "$2" "$3" "$4"
    local n=$(( $(attempts_of "$2") + 1 ))
    mark ATTEMPTED "$1" "$2"
    # release the backlog lease so the item is claimable again next pass (the
    # $QUEUE ledger still suppresses it if this was the ESCALATED attempt).
    local _gid; _gid="$(id_of "$2")"; bk unclaim "$_gid"   # release the backlog lease (EXIT-trap over-unclaim is harmless)
    if [ "$n" -ge "$MAX_ATTEMPTS" ]; then
        mark ESCALATED "$1" "$2"; log "lane [$1] exhausted $MAX_ATTEMPTS attempts — ESCALATED to human (resume at $RESUME_DIR/$(slug_of "$2").md)"
    else
        mark PENDING "$1" "$2"; log "lane [$1] attempt $n/$MAX_ATTEMPTS failed — re-queued to RESUME next pass"
    fi
}   # status kind desc

# ── per-class routing (type-system / runtime / security) ────────────
# Each class gets a focused brief + a class-specific adversarial angle.
# A fuzzer-found panic → runtime; an inferencer/soundness bug →
# typesystem; anything on an auth/secrets/crypto/unsafe/FFI surface →
# security (highest stakes).
class_focus() { case "$1" in
  typesystem) printf '%s' "the HM type inferencer / solver / codegen SOUNDNESS (crates/sky_types, sky_lower, sky_backend_rust). Preserve the parametric-generic + wildcard-any gates. HARD BAN: NEVER introduce \`dyn Any\`, \`Box<dyn Any>\`, \`.downcast\`, or any runtime type-erasure/reflection to paper over a type — the port's guarantee is fully-typed codegen; a fix that reaches for \`Any\` is not a fix, it is a soundness hole. A fix that makes the no-panic fuzzer fail or that accepts an ill-typed program is a regression, not a fix." ;;
  runtime)    printf '%s' "the EMITTED-CODE runtime behaviour (runtime/src/sky_runtime, the emit in crates/sky_backend_rust). A well-typed program must never panic — add/repair the runtime guard or codegen so the failing case exits cleanly with a typed Error, matching the ../sky reference. Do NOT silence a panic by weakening a check." ;;
  security)   printf '%s' "a SECURITY-sensitive surface (auth/secrets/crypto/SQL/unsafe/FFI). EXTRA RULES: secrets stay typed and are NEVER logged or fmt-stringified; constant-time compares for anything secret; NO new \`unsafe\` — full stop (a 'justified' unsafe is still a hole; if the change seems to need unsafe, STOP and escalate, do not write it); parse-don't-validate; SQL as typed fragments, never string interpolation. This is the highest-stakes class — prefer a minimal, conservative, heavily-tested change and STOP+escalate rather than guess." ;;
  *)                   printf '%s' "the compiler internals; root-cause only." ;;
esac; }
reviewer_angle() { case "$1" in
  typesystem) printf '%s' "construct a program the change now WRONGLY accepts (unsoundness) or wrongly rejects (regression); probe the parametric-generic gate + numeric/collection defaulting. AUTO-REJECT if the diff introduces ANY \`dyn Any\` / \`.downcast\` / runtime type-erasure." ;;
  runtime)    printf '%s' "find an input that still panics, or a case where the new guard changes observable behaviour vs the ../sky reference." ;;
  security)   printf '%s' "ATTACK it, assume malice: can a secret leak via logs/errors/timing? is any compare non-constant-time? does it weaken an existing gate or open an injection? AUTO-REJECT if the diff adds ANY new \`unsafe\`." ;;
  *)                   printf '%s' "try to find any unsoundness, behaviour change, or disguised hack." ;;
esac; }

# ── lane_author <i> — CONCURRENT authoring for one lane (own worktree + own cargo
# target). Runs design→impl→review; writes a one-line verdict to ${lane_res[i]}:
# OK / ESCALATE / NOCOMMIT / REJECT (+reason). Touches ONLY per-lane artifacts
# (logs, its own status file, its own resume plan) — NO $QUEUE/bk/shared-status
# writes; those are serial (Phase B). Runs as a background subshell, so its local
# STATUS override + cwd changes never leak to siblings.
lane_author() {
    local i="$1"
    local class="${lane_class[$i]}" gdesc="${lane_desc[$i]}" gbr="${lane_br[$i]}"
    local gwt="${lane_wt[$i]}" tgt="${lane_tgt[$i]}" glog="${lane_glog[$i]}" res="${lane_res[$i]}"
    local dlog="/tmp/autopilot-lane-design-c$cycle-$i.log"
    local rlog="/tmp/autopilot-lane-review-c$cycle-$i.log"
    local dfile="/tmp/autopilot-lane-design-plan-c$cycle-$i.md"
    local dplan="$RESUME_DIR/$(slug_of "$gdesc").design.md"
    local datt design design_text review ahead
    STATUS="$(status_file "$i")"                       # this lane's header (local to the subshell)
    datt="$(( $(attempts_of "$gdesc") + 1 ))"
    CUR_FIRST_EPOCH=""   # reset so set_task stamps a fresh first-seen epoch for this item
    set_task "$class" "$gdesc" "$datt/$MAX_ATTEMPTS"

    # DESIGN — reuse the saved plan on a retry, else fresh (Opus)
    if [ "$datt" -ge 2 ] && [ -s "$dplan" ]; then
        phase design reused
        design_text="$(cat "$dplan")"; cp -f "$dplan" "$dfile"; : > "$dlog"
    else
        phase design opus
        # SANDBOX: run the design agent inside the lane worktree (same as IMPL)
        # so that any file the agent writes despite the "do NOT write code"
        # instruction lands in the throwaway lane branch, not the main checkout.
        design="$( ( cd "$gwt"; agent "$DESIGN_MODEL" "You are a compiler fix DESIGNER specialising in $class. Do NOT write code. Produce a concise root-cause + fix PLAN: (a) the root cause, (b) the exact crates/files/functions to change, (c) the approach — matching the ../sky READ-ONLY reference, root-cause only, NEVER a hack/fixture-edit/gate-weakening, (d) the regression test to add. FOCUS: $(class_focus "$class"). Item: $gdesc .$(resume_hint "$gdesc") If there is NO sound fix (needs a human decision, genuinely multi-session, or would require a hack), print exactly 'DESIGN: ESCALATE <why>' and nothing else. Otherwise print 'DESIGN: <the plan>'." ) | tee "$dlog")"
        if printf '%s' "$design" | rg -q 'DESIGN: ESCALATE'; then
            printf 'ESCALATE design escalate: %s\n' "$(reason_of "$design")" > "$res"; return 0
        fi
        design_text="$(printf '%s' "$design" | jq -rj 'select(.type=="assistant") | .message.content[]? | select(.type=="text") | .text' 2>/dev/null)"
        [ -z "$design_text" ] && design_text="$design"
        mkdir -p "$RESUME_DIR"; printf '%s\n' "$design_text" > "$dplan"; printf '%s\n' "$design_text" > "$dfile"
    fi

    # IMPL — in the lane worktree, own cargo target
    phase impl opus
    ( cd "$gwt"; CARGO_TARGET_DIR="$tgt" \
      agent "$AUTHOR_MODEL" "You are the IMPLEMENTER. READ the DESIGN plan at $dfile FIRST, then follow it EXACTLY — do NOT redesign or deviate from it. Obey the operating contract (6 principles + 2 rules + the seal). Item: $gdesc .$(resume_hint "$gdesc") Boundary: the Rust-port crates + runtime; ../sky is READ-ONLY. Implement the fix + the regression test the design names. SELF-CHECK (do NOT run the full workspace test suite — the integration gate runs that once): (1) 'cargo clippy --workspace --all-targets -- -D warnings' is clean; (2) 'cargo build -p skyc', then rebuild the failing example and confirm its ORIGINAL diagnostic is GONE; (3) if your fix LEGITIMATELY changes the emitted Rust for an EXISTING golden (a tests/golden/** byte-identical assertion now fails BECAUSE your codegen change is correct — not a regression), REGENERATE that golden's oracle with 'cargo run -p refresh-oracle -- <golden_name>' and include the refreshed files in the commit — NEVER hand-edit oracle.meta / expected_go.txt, and refresh ONLY the goldens your change explains (never blanket-refresh; a stale un-refreshed golden red-gates, a wrongly-refreshed one hides a regression the reviewer will REJECT). Iterate until all pass (cap ~3 tries). Then 'git add -A && git commit'. Final line: 'IMPL: DONE' or 'IMPL: STUCK <why>'." ) >"$glog" 2>&1
    ahead="$(git rev-list --count "$BASE..$gbr" 2>/dev/null || echo 0)"
    if [ "$ahead" -eq 0 ]; then
        printf 'NOCOMMIT impl no-commit. notes: %s\n' "$(tail -10 "$glog" 2>/dev/null | tr '\n' ' ' | cut -c1-300)" > "$res"; return 0
    fi

    # REVIEW — adversarial, in the lane worktree, own cargo target
    phase review opus
    review="$( ( cd "$gwt"; CARGO_TARGET_DIR="$tgt" \
      agent "$REVIEW_MODEL" "You are an ADVERSARIAL reviewer of a $class change on branch $gbr (diff: git diff $BASE..$gbr). REFUTE it — $(reviewer_angle "$class"). Read the diff + the added tests, THEN VERIFY WITH YOUR OWN HANDS: build skyc from THIS branch and cargo-build your OWN probe programs — confirm skyc-exit-0 ⇒ cargo-exit-0 (THE SEAL) yourself. The impl's self-report is NOT evidence (a fix can report green while a buried guard disabled it, or while it emits skyc-0-then-cargo-fail on a shape the impl never tested). Cache-hits on unchanged code are fine — recompiling identical source is zero-signal; integrity comes from RUNNING the committed source + your probes. If the diff REFRESHES any golden (tests/golden/**, oracle.meta / expected_go.txt), that refresh is AUTHORISED only when it contains ONLY the emit deltas THIS codegen change explains — inspect the refreshed bytes yourself and REJECT if a golden refresh hides an unrelated change, silences a real regression, or looks blanket-regenerated. If you find ANY unsoundness / behaviour change vs the ../sky reference / disguised hack / skyc-0-then-cargo-fail / illegitimate golden refresh, print 'REVIEW: REJECT <why + the exact repro>'. Only if you cannot break it after genuine effort, print 'REVIEW: ACCEPT'. Default to REJECT when uncertain." ) | tee "$rlog")"
    if ! printf '%s' "$review" | rg -q 'REVIEW: ACCEPT'; then
        printf 'REJECT review REJECT: %s\n' "$(reason_of "$review")" > "$res"; return 0
    fi
    printf 'OK\n' > "$res"
}

# ── lane_gate <i> — SERIAL integrate for a lane whose author verdict was OK. Runs
# on the shared checkout (single-writer): merge → nextest+doc+clippy+fuzz → close
# or revert. A merge CONFLICT (this lane was cut from the pre-cycle base and an
# earlier lane advanced HEAD into the same files) → abort + requeue, no attempt burned.
# lane_gate <i> — per-lane CHEAP gate (fast): merge the lane into integration, then
# build skyc + run ONLY the touched crates' tests + clippy. A cheap-green item lands
# on integration but stays CLAIMED and is added to $BATCH — the authoritative full
# workspace gate (+ E2E + fuzz) runs every $FULL_GATE_EVERY cycles via certify_batch(),
# which is the ONLY thing that closes items and advances master. So master never sees
# uncertified code, but per-item iteration is minutes, not ~30.
lane_gate() {
    local i="$1"
    local class="${lane_class[$i]}" gdesc="${lane_desc[$i]}" gbr="${lane_br[$i]}" gid="${lane_id[$i]}"
    local GRF gpre touched pflags
    GRF="-C link-arg=-fuse-ld=mold -Zthreads=8"
    STATUS="$(status_file "$i")"
    phase gate ·
    git -C "$GATE_WT" switch "$INTEGRATION" >/dev/null 2>&1
    gpre="$(git -C "$GATE_WT" rev-parse HEAD)"
    if ! git -C "$GATE_WT" merge --no-ff -m "autopilot: $class fix — $gdesc" "$gbr" >/dev/null 2>&1; then
        git -C "$GATE_WT" merge --abort 2>/dev/null
        log "lane [$class] · merge race with a landed lane — requeued (no penalty): $gdesc"
        bk unclaim "$gid"; return 0
    fi
    # scope the cheap gate to the crates this change touched (+ always check skyc).
    touched="$(git -C "$GATE_WT" diff --name-only "$gpre..HEAD" 2>/dev/null)"
    pflags="$(printf '%s\n' "$touched" | sed -nE 's#^crates/([^/]+)/.*#-p \1#p; s#^runtime/.*#-p sky-runtime-rust#p; s#^tools/([^/]+)/.*#-p \1#p' | sort -u | tr '\n' ' ')"
    [ -z "$pflags" ] && pflags="-p skyc"
    if ( cd "$GATE_WT"; touch crates/skyc/tests/*.rs 2>/dev/null; \
         RUSTFLAGS="$GRF" CARGO_TARGET_DIR="$GATE_TARGET" timeout 1200 cargo +nightly check -p skyc >/tmp/autopilot-gate.log 2>&1 \
         && RUSTFLAGS="$GRF" CARGO_TARGET_DIR="$GATE_TARGET" timeout 1500 cargo +nightly nextest run $pflags >>/tmp/autopilot-gate.log 2>&1 \
         && RUSTFLAGS="$GRF" CARGO_TARGET_DIR="$GATE_TARGET" timeout 900 cargo +nightly clippy $pflags --no-deps -- -D warnings >>/tmp/autopilot-gate.log 2>&1 ); then
        # NB: no --all-targets — the authoritative full gate (full_gate) lints
        # lib+bins only (clippy --workspace, no --all-targets), so a stricter cheap
        # gate would false-RED skyc-touching lanes on pre-existing test-file lint
        # debt the full gate never checks (this false-escalated #194/#197). Adopting
        # --all-targets in BOTH gates after the test-lint sweep is #203 (DEVELOPMENT §3b).
        log "lane [$class] · landed on integration (cheap-green; pending full-gate cert)"; mark LANDED "$class" "$gdesc"
        BATCH+=("$gid|$class|$gdesc")   # stays claimed; certify_batch closes on full-green / re-queues on full-red
    else
        log "lane [$class] · cheap-gate RED — reverting integration to $gpre"
        git -C "$GATE_WT" merge --abort 2>/dev/null; git -C "$GATE_WT" reset --hard "$gpre" >/dev/null 2>&1
        item_failed "$class" "$gdesc" "$gbr" "cheap-gate failed. errors: $(rg -N -e 'error\[' -e '^error:' -e 'FAILED' -e 'panicked' -e 'could not compile' -e 'Killed' /tmp/autopilot-gate.log 2>/dev/null | tail -6 | tr '\n' ' ' | cut -c1-520)"
    fi
}

# full_gate — the AUTHORITATIVE workspace gate (nextest + --features full + doctest
# + clippy + fuzz) on integration HEAD. Returns 0 green / 1 red. Run only by
# certify_batch, every $FULL_GATE_EVERY cycles (or when pending hits 0).
full_gate() {
    local GRF SKY_SHARE
    GRF="-C link-arg=-fuse-ld=mold -Zthreads=8"
    if git -C "$GATE_WT" diff --name-only "$LAST_FULL_GREEN..HEAD" 2>/dev/null | rg -q '^runtime/'; then SKY_SHARE=""; else SKY_SHARE="$IPE_CACHE/oracle-target"; fi
    ( cd "$GATE_WT"; touch runtime/tests/*.rs crates/skyc/tests/*.rs 2>/dev/null; \
      RUSTFLAGS="$GRF" CARGO_TARGET_DIR="$GATE_TARGET" SKY_ORACLE_SHARED_TARGET="$SKY_SHARE" timeout 3000 cargo +nightly nextest run --workspace >/tmp/autopilot-gate.log 2>&1 \
      && RUSTFLAGS="$GRF" CARGO_TARGET_DIR="$GATE_TARGET" SKY_ORACLE_SHARED_TARGET="$SKY_SHARE" timeout 1800 cargo +nightly nextest run -p sky-runtime-rust --features full >>/tmp/autopilot-gate.log 2>&1 \
      && RUSTFLAGS="$GRF" CARGO_TARGET_DIR="$GATE_TARGET" timeout 600 cargo +nightly test --workspace --doc >>/tmp/autopilot-gate.log 2>&1 \
      && RUSTFLAGS="$GRF" CARGO_TARGET_DIR="$GATE_TARGET" timeout 1200 cargo +nightly clippy --workspace --no-deps --jobs 4 -- -D warnings >>/tmp/autopilot-gate.log 2>&1 ) \
      && ( cd "$GATE_WT"; "$FUZZ" --iters "$FUZZ_ITERS" --quiet >>/tmp/autopilot-gate.log 2>&1 )
}

# certify_batch — full-gate the accumulated $BATCH. GREEN: close every batch item,
# mark this the new LAST_FULL_GREEN, advance master to it. RED: reset integration to
# the last certified sha + re-queue every batch item (the full-gate error is logged;
# next attempts + the per-error grep pinpoint the culprit). Coarse-but-safe: master
# only ever advances to a fully-gated sha.
certify_batch() {
    [ "${#BATCH[@]}" -gt 0 ] || return 0
    phase certify ·
    log "certify: full gate over ${#BATCH[@]} cheap-landed item(s)…"
    local e id
    if full_gate; then
        log "certify: full gate GREEN — ${#BATCH[@]} item(s) certified"
        for e in "${BATCH[@]}"; do id="${e%%|*}"; bk close "$id" --done-at "$(date +%F)"; done
        LAST_FULL_GREEN="$(git -C "$GATE_WT" rev-parse HEAD)"; BATCH=()
        advance_master
    else
        log "certify: full gate RED — reverting batch to $LAST_FULL_GREEN + re-queuing ${#BATCH[@]}. errors: $(rg -N -e 'error\[' -e '^error:' -e 'FAILED' -e 'panicked' -e 'could not compile' -e 'Killed' /tmp/autopilot-gate.log 2>/dev/null | tail -6 | tr '\n' ' ' | cut -c1-500)"
        git -C "$GATE_WT" reset --hard "$LAST_FULL_GREEN" >/dev/null 2>&1
        for e in "${BATCH[@]}"; do id="${e%%|*}"; bk unclaim "$id"; done
        BATCH=()
    fi
}

# EMERGENCY-GRACEFUL stop helper: after the in-flight lanes' process groups are
# killed, preserve every lane's work so a mid-cycle stop loses NOTHING — committed
# work → a durable salvage/ ref; uncommitted worktree edits → a patch under
# $RESUME_DIR — then release each claim. Reads the cycle-local lane_* arrays (bash
# globals). The EXIT trap still runs advance_master afterward.
salvage_lanes() {
    local k br gid pf
    for ((k=0; k<n; k++)); do
        br="${lane_br[$k]}"; gid="${lane_id[$k]}"
        if git rev-parse --verify -q "$br" >/dev/null 2>&1 && [ -n "$(git log --oneline "$BASE..$br" 2>/dev/null)" ]; then
            git branch -f "salvage/stopped-i$gid-c$cycle" "$br" 2>/dev/null \
              && log "  salvaged committed work: $br → salvage/stopped-i$gid-c$cycle"
        fi
        if [ -d "${lane_wt[$k]}" ]; then
            pf="$RESUME_DIR/stopped-i$gid-c$cycle.patch"; mkdir -p "$RESUME_DIR" 2>/dev/null
            ( cd "${lane_wt[$k]}" && git add -A 2>/dev/null && git diff --cached ) > "$pf" 2>/dev/null
            [ -s "$pf" ] && log "  salvaged uncommitted edits → $pf"
        fi
        bk unclaim "$gid" 2>/dev/null
    done
    log "emergency-graceful stop: $n lane(s) salvaged + claims released; exiting via cleanup"
}

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

    reclaim_stale_claims   # return leases whose owner died (before convergence reads pending)
    prune_oversize_lane_targets   # hard per-lane-target size cap (independent of total disk)
    prune_oversize_gate_target    # hard gate-target size cap — bounds the full-gate cruft that filled the disk 2026-07-15
    # cycle START: pull any human commits on $MASTER into integration (under the lock).
    # A genuine overlap conflict needs a human — stop + escalate rather than spin.
    resync_integration || { log "STOP: integration ⇄ $MASTER conflict — resolve in $GATE_WT, then restart"; break; }
    # convergence: did the PREVIOUS cycle make progress? (integration advanced, or work remains)
    cur_head="$(git -C "$GATE_WT" rev-parse HEAD)"
    act="$(pending | wc -l | tr -d ' ')"
    # full-gate cadence: certify the cheap-landed batch every $FULL_GATE_EVERY cycles,
    # OR the moment pending work is exhausted (act==0) — whichever comes first. certify
    # green → close batch + advance master; red → reset integration + re-queue batch.
    if [ "${#BATCH[@]}" -gt 0 ] && { [ "$CYCLES_SINCE_FULL" -ge "$FULL_GATE_EVERY" ] || [ "$act" -eq 0 ]; }; then
        certify_batch; CYCLES_SINCE_FULL=0
        cur_head="$(git -C "$GATE_WT" rev-parse HEAD)"; act="$(pending | wc -l | tr -d ' ')"
    fi
    if [ -n "$prev_head" ] && [ "$cur_head" = "$prev_head" ] && [ "$act" -eq 0 ]; then
        dry=$((dry+1)); log "no-progress pass $dry/2 (nothing landed, no tractable findings)"
        [ "$dry" -ge 2 ] && { log "converged — 2 dry passes; work done or human-blocked. stopping."; break; }
    else
        dry=0
    fi
    prev_head="$cur_head"
    log "cycle $cycle (dry=$dry, actionable=$act)"

    # 1 ── (mechanical tier ABOLISHED) — every backlog item runs the SAME
    #       design→impl→review→gate pipeline below; no mechanical fast-path.

    # 2 ── audit what landed (adversarial soundness review of new commits) ────
    if [ "$(git -C "$GATE_WT" rev-parse HEAD)" != "$START_SHA" ] && [ "$(git -C "$GATE_WT" rev-parse HEAD)" != "$last_audit" ]; then
        last_audit="$(git -C "$GATE_WT" rev-parse HEAD)"
        log "digest audit: adversarial review of landed commits (Opus)"
        landed="$(git -C "$GATE_WT" log --oneline "$START_SHA"..HEAD)"
        agent "$REVIEW_MODEL" "You are AUDITING autonomous commits for soundness. Review the diffs of these commits on the current branch and answer: did ANY of them land a HACK rather than a root-cause fix — e.g. editing a reference-identical example fixture to satisfy our type-checker, weakening/removing a gate or soundness check, adding a \`_ =>\` catch-all to dodge exhaustiveness, or a \`#[allow]\`/\`unwrap\` that hides a contract violation? Commits:
$landed
For each, \`git show <sha>\`. If you find a violation, print AUDIT: VIOLATION <sha> <why> and STOP (do not fix). If all are genuine root-cause work, print AUDIT: CLEAN. Be adversarial; err toward flagging." | tee /tmp/autopilot-audit-c$cycle.log | show_agent audit
        if rg -q "AUDIT: VIOLATION" /tmp/autopilot-audit-c$cycle.log; then
            log "AUDIT FLAGGED A VIOLATION — stopping for human review (see /tmp/autopilot-audit-c$cycle.log)"; break
        fi
    fi

    # 2b ── (remeasure sweep + fuzz + triage ABOLISHED) — the work source is
    #        backlog.jsonl, so there is no sweep to discover/refill items.
    # 3 ── dispatch: up to LANES items — author CONCURRENTLY, integrate SERIALLY ──
    mapfile -t guard < <(pending)
    if [ "${#guard[@]}" -eq 0 ]; then log "nothing actionable this pass"; continue; fi
    CUR_CLAIMS=()          # reset the per-cycle lease list (EXIT trap releases whatever is in flight)
    # lane count this cycle: LANES, dropped to 1 when disk is tight — two concurrent
    # cargo targets need headroom (guards against the ENOSPC crisis).
    want="$LANES"   # concurrent authoring lanes this cycle
    if [ "$(disk_free_gb)" -lt "${PROGDEV_LANES_DISK_MIN:-30}" ]; then
        want=1; log "disk $(disk_free_gb)G < ${PROGDEV_LANES_DISK_MIN:-30}G — 1 lane this cycle"
    fi
    rm -f docs/architecture/progdev-status-lane*.txt 2>/dev/null   # clear last cycle's lane rows for watch

    # ── setup (SERIAL): claim + worktree + per-lane arrays ──
    lane_class=(); lane_desc=(); lane_br=(); lane_wt=(); lane_tgt=(); lane_glog=(); lane_res=(); lane_id=()
    n=0
    for g in "${guard[@]}"; do
        [ "$n" -ge "$want" ] && break
        [ -f "$STOP" ] && break
        class="${g%%$'\t'*}"; gdesc="${g#*$'\t'}"; [ -z "$class" ] && class="typesystem"
        gid="$(id_of "$gdesc")"; bk claim "$gid"; CUR_CLAIMS+=("$gid")
        gbr="progressive-development/lane-c$cycle-$n"
        gwt="$REPO/.progressive-development-wt/lane-$n"
        # collision-proof: a SIGKILLed prior run leaves the worktree registration AND
        # branch behind; force-clear both (branch is scratch, resume artifact preserved).
        git worktree remove --force "$gwt" 2>/dev/null || true; rm -rf "$gwt"; git branch -D "$gbr" 2>/dev/null || true
        if ! git worktree add --quiet -b "$gbr" "$gwt" "$BASE"; then
            mark BLOCKED "$class" "$gdesc"; bk unclaim "$gid"; continue
        fi
        lane_class+=("$class"); lane_desc+=("$gdesc"); lane_br+=("$gbr"); lane_wt+=("$gwt")
        lane_tgt+=("$LANE_TARGET_BASE-$((n+1))-target"); lane_glog+=("/tmp/autopilot-lane-c$cycle-$n.log")
        lane_res+=("/tmp/autopilot-lane-c$cycle-$n.result"); lane_id+=("$gid")
        : > "/tmp/autopilot-lane-c$cycle-$n.result"
        n=$((n+1))
    done
    [ "$n" -eq 0 ] && continue
    log "${#guard[@]} actionable; authoring $n lane(s) concurrently (design→impl→review)…"

    # ── Phase A: author CONCURRENTLY, then barrier — but POLL the stop flag so an
    #    emergency stop responds in seconds instead of waiting out slow authoring.
    #    `set -m` puts each lane in its OWN process group so a stop can SIGTERM the
    #    whole subtree (subshell + timeout + claude + cargo) cleanly — no orphans,
    #    and Lane-3 / other agents (different groups) are untouched.
    set -m
    apids=()
    for ((k=0; k<n; k++)); do lane_author "$k" & apids+=("$!"); done
    set +m
    STOP_MIDCYCLE=0
    while :; do
        alive=0
        for p in "${apids[@]}"; do kill -0 "$p" 2>/dev/null && alive=1; done
        [ "$alive" -eq 0 ] && break
        if [ -f "$STOP" ]; then
            log "emergency-graceful stop: winding down $n in-flight lane(s) (SIGTERM process groups)…"
            for p in "${apids[@]}"; do kill -TERM -- -"$p" 2>/dev/null; done
            sleep 3
            for p in "${apids[@]}"; do kill -KILL -- -"$p" 2>/dev/null; done
            STOP_MIDCYCLE=1; break
        fi
        sleep 2
    done
    for p in "${apids[@]}"; do wait "$p" 2>/dev/null; done
    # mid-cycle stop → salvage every lane's work (committed branch + uncommitted edits),
    # release claims, then break the loop into the EXIT trap. Nothing is lost.
    if [ "$STOP_MIDCYCLE" = 1 ]; then salvage_lanes; break; fi

    # ── Phase B: integrate SERIALLY (single-writer git) ──
    for ((k=0; k<n; k++)); do
        [ -f "$STOP" ] && break
        v="$(head -1 "${lane_res[$k]}" 2>/dev/null)"; verdict="${v%% *}"; vreason="${v#* }"
        case "$verdict" in
            OK) lane_gate "$k" ;;
            ESCALATE|NOCOMMIT|REJECT)
                log "lane [${lane_class[$k]}] · $verdict · ${lane_desc[$k]}"
                item_failed "${lane_class[$k]}" "${lane_desc[$k]}" "${lane_br[$k]}" "$vreason" ;;
            *)  log "lane [${lane_class[$k]}] · no verdict (author died) — requeued"; bk unclaim "${lane_id[$k]}" ;;
        esac
        git worktree remove --force "${lane_wt[$k]}" 2>/dev/null; git branch -D "${lane_br[$k]}" 2>/dev/null
    done
    CYCLES_SINCE_FULL=$((CYCLES_SINCE_FULL+1))   # full gate fires every $FULL_GATE_EVERY of these (or when pending hits 0)
    reclaim_disk
    git worktree prune

    # persistent per-cycle cost ledger — cumulative agent $ from this run's logs,
    # recorded each cycle so a later run's /tmp overwrite can't erase the trend.
    { printf '%s\t%s\tc%s\t' "$(date +%FT%H:%M)" "${START_SHA:0:7}" "$cycle"
      for f in /tmp/autopilot-lane-*.log /tmp/autopilot-triage-*.log /tmp/autopilot-audit-*.log; do
          [ -f "$f" ] && rg '"type":"result"' "$f" 2>/dev/null | tail -1
      done | jq -rs '([.[] | .total_cost_usd // 0] | add // 0) | "$" + ((.*100|floor)/100|tostring)'
    } >> "$LEDGER" 2>/dev/null || true
done

# ── landed digest for the human meta-audit ───────────────────────────────────
{
    echo "# Autopilot run digest — $(date -Is)"
    echo ""; echo "Base $START_SHA → $(git rev-parse --short HEAD).  Review before trusting the lane commits."
    echo ""; echo "## Landed this run"; git log --oneline "$START_SHA"..HEAD | sed 's/^/- /'
    echo ""; echo "## Queue tail"; tail -15 "$QUEUE"
} > "$DIGEST"
log "done. digest → $DIGEST (audit the lane commits). queue → $QUEUE"
