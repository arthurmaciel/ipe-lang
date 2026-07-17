# CO-TYPES findings

3 findings: 0 critical, 1 high, 0 medium, 2 low.

Audited: `src/compiler/types/src/{unionfind,unify,solve,ty,doc,exhaust,constrain,lib}.rs`,
`src/compiler/lower/src/{lib,lower}.rs`, plus load-bearing contract surfaces they
depend on (`src/compiler/ir/src/ir.rs` `Match::new`/`new_flat`,
`src/compiler/canon/src/env.rs` builtin-ctor registration) read for reachability
verification only.

## CO-TYPES-001 · Exhaustiveness pass is blind to Prelude builtin ADTs — nested non-exhaustive `case` reaches cargo as E0004
- severity: high
- axis: soundness
- principle: THE SEAL — "if `ipe` accepts a program (exit 0), the emitted Rust MUST `cargo build`"; make-invalid-states-unrepresentable (a drifted table)
- location: `src/compiler/types/src/exhaust.rs:80-106` (Sigs::build seeds only Maybe/Result), `src/compiler/types/src/exhaust.rs:466-471` (whole-`case` skip on any unknown ctor), `src/compiler/ir/src/ir.rs:2399-2455` (`Match::new` backstop is top-constructor-only by contract), `src/compiler/lower/src/lower.rs:6900-7001` (lowerer's full builtin variant tables the exhaust pass lacks)
- reachability: any user `case` whose patterns mention a Prelude builtin ADT constructor other than `Just`/`Nothing`/`Ok`/`Err` — `ErrorKind` (`Io`/`Network`/…, 11 variants), `ErrorDetails` (`FfiPanic`/`HttpStatus`/…), `SqlValue` (9 variants), `SqlField`, `ChunkEvent` (`Chunk`/`Done`/`Errored`, the documented `forEachChunk` handler shape). The chain is complete: `canon::env::install_builtin_ctors` registers these ctors (patterns resolve), `constrain.rs:905-983` registers their ctor schemes (the program type-checks), but `exhaust::Sigs::build` seeds only Maybe/Result, so `pattern_uses_unknown_ctor` returns true and `check_case` skips the ENTIRE `case` (exhaust.rs:466-471).
- problem: two manifestations of the one drifted table.
  (a) **SEAL violation** — a nested non-exhaustive `case` over a builtin ADT, e.g. `case m of Just Io -> …; Just Network -> …; Nothing -> …` (a `Maybe ErrorKind`, 9 nested variants missing, no catch-all): exhaust skips it; lowering takes the all-ctor `Match::new` path whose backstop checks only TOP constructors (`Just`+`Nothing` both present → passes, by its documented contract "exhaustiveness over the nested shape is proven UPSTREAM"); the emitted Rust `match` is genuinely non-exhaustive → rustc E0004 after `ipe` exit 0. The lower-side comment at `lower.rs:16415-16418` ("a non-exhaustive nested `case` is already IPE-T0010 and never reaches here") is false for exactly these types. Same escape through `Match::new_flat`'s `all_ctor_headed` backstop (ir.rs:2503-2508) when an alias head routes the arm set there.
  (b) **wrong error semantics** — a TOP-LEVEL non-exhaustive `case` over a builtin ADT (`case kind of Io -> …; Network -> …`, 9 variants missing) is caught by `Match::new`'s variant-set backstop, but as `Diagnostic::CompilerBug` ("non-exhaustive match: top constructors cover 2 of 11 variants") — the user's own error presented as an internal compiler bug instead of the friendly IPE-T0010 with missing-pattern witnesses. Redundancy analysis (IPE-T0011) is likewise silently skipped for these cases.
- fix direction: give `exhaust::Sigs::build` the SAME builtin constructor table canon's `install_builtin_ctors` and the lowerer's `enum_variants` seeding already share — one table, three consumers, so the class of "registered in canon+constrain but invisible to exhaust" cannot recur (fix the structure: today the identical variant lists are hand-maintained in three places, and exhaust's copy holds 2 of 8 types).
- prior: new (runtime-audit-verdict.md covers the runtime only)

## CO-TYPES-002 · Record unification step 4 drops both extension variables and the closed bound
- severity: low
- axis: correctness
- principle: P2 Correctness (parity is the default; this is bug-compatible with `../sky` but diverges from elm's `unifyRecords`, and the divergence from elm is not recorded)
- location: `src/compiler/types/src/unify.rs:376-392` (step 4 of the record arm)
- reachability: an open record meeting a closed record whose extras are all on the CLOSED side — today only via the kernel cfg open records (`Live.app`/`Tui.app`/`Webview.app` schemes; user annotations always instantiate `RowTail::Closed`, `ty.rs:475-481`), so a user cfg literal with unknown extra fields.
- problem: the "both open" comment on step 4 is not what the guard enforces — the branch also runs when one side is CLOSED (closed side has extras, open side has none: `extras1_illegal`/`extras2_illegal` both false). The merge (1) discards `ext1`/`ext2` without unifying them against the leftover rows (elm's `unifyRecords` unifies each ext with a record of the other side's extras, preserving row-sharing and closedness; the Haskell `../sky` port at `Unify.hs:468-512` has the same drop, so this is faithful-but-bug-compatible) and (2) always mints a fresh FLEX tail, so a closed record that survives the merge becomes OPEN — subsequent unifications can absorb further fields the original closed type never had. Practical effect today: a typo'd OPTIONAL cfg field (`heda = …` for `head`) is silently absorbed and ignored with no diagnostic (required-field omissions are still caught by step 2). No exit-0-then-cargo-fail shape found: cfg records are consumed positionally by the emit tier, not through the widened row.
- fix direction: in step 4, mirror elm — unify `ext2` with `Record(only1, fresh)` and `ext1` with `Record(only2, fresh)` (which makes closed-side extras fail naturally and keeps closedness); record the elm-vs-sky choice in `docs/divergences-from-sky.md` either way.
- prior: new

## CO-TYPES-003 · Empty-module wildcard in `Con` unification weakens nominal identity
- severity: low
- axis: correctness
- principle: fundamental rule "make invalid states unrepresentable" (nominal identity is `(home, name)` everywhere else — exhaust `TyId`, `SolvedTypes` keys, `enum_variants` — but unify compares by name alone when either home is empty)
- location: `src/compiler/types/src/unify.rs:286-292` (`modules_compat = m1 == m2 || m1.is_empty() || m2.is_empty()`)
- reachability: requires a `Con` whose home the canonicaliser lost — the code comment documents the `unwrap_or_default() → []` fallback for unknown type names. A user type `A.Color` then unifies with an unresolved bare `Color` from another module (same name, same arity), silently adopting `A`'s home as canonical instead of failing.
- problem: two nominally distinct types can unify whenever one side's home was defaulted to empty, masking a genuine type error; downstream `(home, name)`-keyed tables (exhaust, `enum_variants`) then judge the value against the wrong constructor set. Marked low: both-non-empty conflicting homes DO mismatch, the empty-home fallback is claimed to match the Haskell oracle, and no concrete end-to-end miscompile was constructed — the hazard is contingent on the canonicaliser emitting an empty-home `Con` for a name that also exists elsewhere, which ideally errors earlier.
- fix direction: make the canonicaliser's unknown-name fallback a hard error (or a distinguished `UnresolvedHome` marker unify treats as builtin-only), so an empty home structurally means "builtin", never "unknown user type".
- prior: new

## Prior-audit cross-check (runtime-audit-verdict.md)

The prior audit is runtime-scoped; one item touches this partition's boundary:
the Phase-2 plan requires an "upstream codegen IPE-* diagnostic rejecting
non-both-literal `Ffi.callPure`/`callTask` so the [ffi_polyfills] panics become
provably dead". That gate is not in `src/compiler/types/**` or
`src/compiler/lower/**` (`Callee::Ffi` in lower.rs is the separate foreign-crate
wrapper path); it belongs to the kernels/backend partition — flagged there, not
re-filed here.

## Clean areas (audited, no finding)

- `unionfind.rs` — fully safe, iterative find with path compression, all arena
  access via `get`/`get_mut`, dangling ids → `CompilerBug`, rank saturating.
- `unify.rs` occurs check — iterative, budget-ticked, runs before every
  flex-to-structure bind (including the record extension var), so the arena
  stays acyclic and zonk terminates.
- `solve.rs` — budget-bounded (checked_sub, `IPE_SOLVER_BUDGET` three-mode,
  malformed value falls back to the default rather than unbounded).
- `exhaust.rs` algorithm itself — a correct Maranget usefulness port (witness
  cap, closed Bool/List signatures, `(home, name)`-keyed ADT identity,
  refutable-pattern gate IPE-T0015 on every def/lambda/let binding position via
  the ONE shared `is_irrefutable` predicate).
- `constrain.rs` `zonk` — iterative work-stack, node cap + budget, no panics;
  AUD-13 solver-var tagging closes the `"any"`-raw collision.
- `lib.rs` post-solve passes — joint field-access/record-update fixpoint
  terminates (progress-or-error), super-type obligations fail CLOSED on bare
  vars (`emitted_bound_satisfied` / `concrete_super_ok`), numeric/SQL-param
  defaulting re-verifies the pinned type.
- `lower.rs` TCO — analysis counts arg-position and wrong-arity self-calls as
  disqualifying non-tail; rewrite descends only tail propagators; `Match`
  pattern shapes untouched (`map_bodies` seals the former unchecked rebuild).
- `lower.rs` guarded-arm (C2) gate — a guard-refutable arm set without a
  trailing irrefutable catch-all fails closed (IPE-L0116/L0129-family), never
  emitting a rustc-non-exhaustive match.
- No `unwrap`/`expect`/`panic!`/raw-indexing on any non-test path in the
  partition (all hits are inside `#[cfg(test)]` modules); both crates are
  `#![forbid(unsafe_code)]`.
