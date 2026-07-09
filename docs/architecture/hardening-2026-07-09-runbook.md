# Hardening run 2026-07-09 — runbook

Source: `principles-audit-2026-07-09.md` (12/12 partitions, 14 verified
high/critical). Specs: `hardening-2026-07-09-batch-a-spec.md` (parallel),
`hardening-2026-07-09-batch-b-spec.md` (serial).

## Pre-dispatch Fable efficiency check (done)

A Fable review of `autopilot.sh`/`orchestrate.sh`/`run.sh`/`watch.sh` (token +
wall-clock, box: 8 cores / 11 GiB avail RAM / 30 GB disk free) found the
pipeline sound as-is, with two moves worth making for THIS scoped 5-item run:

1. **Don't let autopilot drift past the mechanical batch.** Once Batch A's 5
   items are done, `autopilot.sh`'s normal loop continues into an Opus audit +
   full 35-example remeasure + 30-iter fuzz sweep + Opus triage — and triage
   REFILLS the queue with unrelated backlog blockers, turning a scoped batch
   into an unattended continuation. **Fix (no code change): `PROGDEV_MAX_CYCLES=1`.**
   The runaway-backstop check (`autopilot.sh:304`) fires at the TOP of cycle 2,
   before any audit/remeasure/triage work runs — cycle 1 does exactly one
   mechanical batch (all 5 items, since `PROGDEV_LANES=5` fits them in one
   `orchestrate.sh` call) then the loop exits clean.
2. **Widen lanes to 5.** Authoring is API-bound, worktrees are cheap, and the
   box has headroom — `PROGDEV_LANES=5` runs all 5 items in one wave instead of
   3 waves at the default LANES=2.

Also confirmed safe (no code change needed): pointing a mechanical lane at an
absolute spec-file path works with existing tooling (`Read`/`cat` already
allowed per `orchestrate.sh:47-51`; the pattern mirrors the guardian tier's
proven `resume_hint` mechanism). Item descriptions must explicitly override
`prompt.md`'s security/soundness-gate self-exclusion instinct (several Batch A
titles read "security"/"DoS" even though the FIX itself is a bounded,
reference-free wire — pre-vetted by the audit, not a lane-time judgment call).

Deferred (not applied — genuinely optional per Fable, and inapplicable to this
run): the guardian design-reuse gate at `autopilot.sh:387-388` only reuses a
pre-seeded plan on ATTEMPT ≥2, wasting a Fable design call on attempt 1 even
when a good plan exists at the resume-dir path. Irrelevant here — Batch A goes
through the MECHANICAL path (never touches the guardian design-reuse code),
and Batch B is hand-implemented, not through the guardian pipeline either.

## Dispatch — Batch A

```bash
cd /home/arthur/Documentos/comp/sky-rust

# Seed the queue: 5 PENDING mechanical items, each pointing at its spec section.
SPEC=/home/arthur/Documentos/comp/sky-rust/docs/architecture/hardening-2026-07-09-batch-a-spec.md
Q=docs/architecture/progressive-development-queue.tsv
override='PRE-APPROVED for this mechanical lane — an already-completed security/soundness audit vetted this exact fix as a bounded, reference-free wire with NO design decision remaining; the standard security/FFI/soundness-gate self-exclusion rule does NOT apply to this item.'
skip='Skip prompt.md'"'"'s backlog.md/log/escalations read-state step — this item is PINNED, not picked from the backlog.'

printf 'PENDING\tmechanical\tAUD-15 (diagnostics code-list drift). %s READ section "## A1" in %s FIRST, follow it exactly. %s\n' "$override" "$SPEC" "$skip" >> "$Q"
printf 'PENDING\tmechanical\tAUD-14 (canon qualifier last-wins). %s READ section "## A2" in %s FIRST, follow it exactly. %s\n' "$override" "$SPEC" "$skip" >> "$Q"
printf 'PENDING\tmechanical\tAUD-12 (numeric-defaulting Append gate). %s READ section "## A3" in %s FIRST, follow it exactly. %s\n' "$override" "$SPEC" "$skip" >> "$Q"
printf 'PENDING\tmechanical\tAUD-07 (hardcoded sqlite::memory: SKY_DB_URL). %s READ section "## A4" in %s FIRST, follow it exactly. %s\n' "$override" "$SPEC" "$skip" >> "$Q"
printf 'PENDING\tmechanical\tAUD-11 (parser MAX_DEPTH bypass, DoS). %s READ section "## A5" in %s FIRST, follow it exactly. %s\n' "$override" "$SPEC" "$skip" >> "$Q"

# One mechanical cycle, all 5 lanes, then clean exit (see finding 1+2 above).
PROGDEV_MAX_CYCLES=1 PROGDEV_LANES=5 scripts/progressive-development/autopilot.sh --no-watch
```

`AUTHOR_MODEL` left at its default (`PROGDEV_AUTHOR_MODEL` unset →
`claude-sonnet-4-6`) — there is no "Sonnet 5" model; the latest Sonnet is
4.6 and it is already the mechanical-lane default.

## Batch B — implemented by hand, not through this pipeline

B1 (lower.rs pair) → B2 (constrain.rs+auth.rs cluster) → B3 (db.rs) → B4
(emit_expr.rs) → B5 (FFI interim mitigation), each gated green before the
next starts. See `hardening-2026-07-09-batch-b-spec.md`.

## Post-run

- `docs/architecture/progressive-development-digest.md` — Batch A's landed
  commits (autopilot writes this on exit).
- Update `docs/architecture/backlog.md` AUD-01..15 lines: mark each landed
  item done (strike or move to a "closed" note), keep AUD-09's mediums/lows
  and AUD-08 as still-open (out of scope for this run).
- Re-run `principles-audit`-style spot check is NOT required — the
  regression tests named in each spec section are the proof.
