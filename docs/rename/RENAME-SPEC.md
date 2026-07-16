# Sky → Ipê rename — execution spec (#212)

> **Status: PLAN / awaiting approval.** This is the durable approval artifact.
> No file is renamed until (1) the `approved` column in `rename-rules.tsv` is
> filled, (2) the examples sweep is confirmed green, and (3) master is free of
> concurrent agents. Nothing here has been executed.

## Why this document exists

The rename is ~38k token occurrences across ~1604 files (`Sky` 14.3k / `sky`
19.3k / `SKY` 4.3k). A naive global `sed` corrupts the build in three ways, so
the rename is gated, classed, and human-approved before any mutation. The
`approved` column in `rename-rules.tsv` is the gate.

## The casing triple (within approved classes only)

| Source | Target | Applies to |
|---|---|---|
| `Sky`  | `Ipe` | Rust identifiers, crate names, code (ASCII, no accent — Rust idents cannot carry `ê`) |
| `sky`  | `ipe` | lowercase code, paths, commands, env-var lowercasing |
| `SKY`  | `IPE` | diagnostic prefixes, env-var prefixes, screaming-case consts |
| `Sky`  | `Ipê` | **doc prose only** (`*.md` sentences naming the language — accented) |

Prose gets the accent (`Ipê`); code never does (`Ipe`). Keep them distinct.

## The three ways a naive rename breaks the build (the traps)

1. **Upstream refs.** `../sky` (358 refs) is the READ-ONLY Haskell/Go reference
   repo — it stays `Sky`/`sky` forever. `docs/divergences-from-sky.md` and every
   prose mention of the *ancestor language* stay `Sky`. A blind sed renames
   these and silently rewrites history + breaks reference paths.
2. **Stdlib namespace churn.** `Sky.Core.*` / `Sky.Ffi` / `Std.*` are DEFERRED
   to the separate namespace-redesign phase (which flattens `Sky.Core` + `Std`
   into one auto-imported namespace). Mechanically renaming them to `Ipe.Core.*`
   now is wasted work — the flatten throws it away. Do NOT touch the stdlib
   import namespace in this rename.
3. **Golden byte-compares.** Every `tests/golden/**/main.rs` is byte-compared to
   emitted output. Renaming crate names / the `.sky` extension / emitted paths
   changes emitted bytes, so ALL goldens must be regenerated via the sanctioned
   `refresh-oracle` path (never hand-edited) as part of the batch that changes
   emitted output.

## Precondition (do not start execution until ALL hold)

- [ ] Examples sweep is a real **35/35** (example 36 re-added verbatim + full
      sweep). Renaming before green conflates rename-breakage with real compiler
      gaps — you lose the ability to bisect.
- [ ] `rename-rules.tsv` `approved` column filled (per-row `yes`/`no`/`defer`).
- [ ] Master has no concurrent autopilot/agent (rename mutates ~all files; a
      same-ref collision destroys work). Run it as the *only* writer.
- [ ] Clean tree (the 1.92 clippy drift sweep landed; no unrelated dirty state).
- [ ] A tag/branch checkpoint `pre-ipe-rename` cut first (one-command rollback).

## Execution order (each batch ends on a green §6 gate)

Batches 1–4 interlock (the tree must stay buildable), so they land as **one
coordinated pass**, re-gated after, not as independent merges:

1. **Rust surface** — crate dir+pkg renames (`sky_*`→`ipe_*`, `skyc`→`ipe`,
   `sky-runtime-rust`→`ipe-runtime-rust`), `use` paths, internal `Sky*` idents,
   `Cargo.toml` deps, workspace members.
2. **Contracts** — `SKY-`→`IPE-` diagnostic codes; `SKY_`→`IPE_` env vars **with
   `scripts/lib/env.sh` + every script + memory updated in lockstep** (these are
   build-harness contracts — a mismatch silently skips E2E).
3. **Artifacts** — `.sky`→`.ipe` extension (examples, goldens' `Main.sky`, stdlib
   source, compiler file-discovery, `sky.toml` `entry`), `sky.toml`→`ipe.toml`,
   `sky-out/`→`ipe-out/`, `.skycache`→`.ipecache`. Then **regenerate goldens via
   `refresh-oracle`**.
4. **Gate** — full four-lane §6 gate green; fresh `skyc`→`ipe` builds + the full
   examples sweep green under the new names.
5. **Docs prose** (`*.md` `Sky`→`Ipê`) — lands **after** code is green, as its own
   commit; MUST-STAY prose (ancestor-language mentions, `../sky`, README line)
   excluded.
6. **DEFERRED, not in this rename** — stdlib import namespace (`Sky.Core.*` etc.)
   → the namespace-redesign phase. `skydex`→`ipe-index` split → its own task.

## Method (how a class is applied safely)

- Build the file set per class, then **subtract the MUST-STAY exclusion set
  first** (`../sky` paths, `docs/divergences-from-sky.md`, the README line,
  ancestor-language prose, the whole stdlib import namespace).
- Apply the casing triple only inside the surviving set, class by class — never
  a single global pass.
- `rustfmt --edition 2024 <touched file>` (never `cargo fmt`).
- Re-run the §6 gate after each landable batch; a red gate reverts that batch
  (`git reset --hard` on the batch), never advances.

## Suggested executors (post-approval)

Cheap Haiku agents, one per class, each with: its file set, the exclusion set,
the casing triple, the no-`.sky`-namespace rule, and the "run the gate, revert
on red" contract. Crate renames touch shared `Cargo.toml`/workspace files, so
that batch is orchestrator-serial, not parallel. Docs prose parallelizes freely.

## Rollback

`git tag pre-ipe-rename <HEAD>` before batch 1. Any batch's red gate →
`git reset --hard` that batch. Catastrophic → `git reset --hard pre-ipe-rename`.
