# T2 — SEAL-breach closure

Findings: **CO-TYPES-001** (exhaustiveness blind to Prelude builtin ADTs → E0004),
**CO-BACKEND-001** (local shadows a bare-emitted top-level fn → E0618 / silent
wrong-call + non-injective `mangle_reserved`), **CO-BACKEND-002** (dead
`assert_mod_idents_unique` gate → E0428 + silent file overwrite).

All three are exit-0-then-cargo-fail (THE SEAL) or worse (a silent miscompile with
no cargo signal at all). All three were missed by `src/ipe-cli/tests/negative_suite.rs`.

---

## Theme root cause

Two structural defects generate the whole theme:

1. **Three hand-maintained copies of the "Prelude builtin ADT" set, drifted.**
   The set of constructors that resolve without a `type` declaration
   (`Maybe`/`Result`/`Bool`, plus `SqlValue`/`SqlField`/`ChunkEvent`/`StreamId`/
   `Error`/`ErrorKind`/`ErrorDetails`/`Order`) is declared THREE times, each by a
   different stage, each independently:
   - **canon** — `install_builtin_ctors` (`canon/src/env.rs:255-361`): the FULL
     set (30+ ctors) so patterns RESOLVE and the program type-checks.
   - **lower** — `BuiltinCtors` struct built in `lower/src/lib.rs:142-190`
     (fields defined `lower/src/lower.rs:6381`): the FULL set again, feeding the
     synthesised `EnumDef`s and the per-arm coverage backstop.
   - **exhaust** — `Sigs::build` (`types/src/exhaust.rs:80-106`): seeds ONLY
     `Maybe`/`Result`. Every other builtin union is absent from
     `ctor_to_union`/`union_ctors`.

   Because the exhaust copy is a strict subset, `pattern_uses_unknown_ctor`
   (`exhaust.rs:238-256`) sees any `ErrorKind`/`SqlValue`/`ChunkEvent` ctor as
   "unknown", `check_case` (`exhaust.rs:466-471`) early-returns `Ok(())`, and no
   usefulness analysis runs. canon already accepted the program; lowering's
   `Match::new` backstop (`ir/src/ir.rs:2415-2455`) is TOP-constructor-only by its
   own documented contract, so a `Maybe ErrorKind` scrutinee passes on its full
   `{Just,Nothing}` top set while the nested `ErrorKind` arm set is never checked.
   The emitted Rust `match` is non-exhaustive on `IpeErrorKind` → rustc E0004.
   **This is CO-TYPES-001.** The generative cause is not "exhaust forgot
   `ErrorKind`" — it is "three tables that must agree, maintained by hand, with no
   forcing function that they agree."

2. **Emitted-name collisions are checked pairwise within one namespace, never
   across namespaces or against the injectivity of the fold that produces them.**
   Three distinct name spaces are folded into flat Rust identifiers:
   - top-level fns via `naming::module_value` → bare `main_update` (no `crate::`
     qualifier at any call site — `emit_expr.rs:941-949` `callee_name` returns the
     bare name for `Callee::Func`, contrast `Callee::Ffi` which DOES `crate::ffi::`);
   - locals via `EmitCtx::emit_ident` → verbatim `let main_update = …`
     (`emit_expr.rs:5953-5956`), only keyword-mangled by `naming::mangle_reserved`;
   - module `mod` idents via `rust_file::mod_ident` → `ipe_mod_std_ui`
     (`rust_file.rs:39-44`), folded through `naming::module_prefix` +
     `to_snake_case`.

   The existing gates (`lib.rs:544`, `lib.rs:590`) compare
   `func_names.values()`/`enum_names.values()` ONLY against each other. They do
   NOT see: a local shadowing a top-level fn (CO-BACKEND-001 — a `let main_update`
   shadows `fn main_update` for the rest of the block; a later call to the
   top-level fn emits `main_update(args)` binding to the local → E0618 for a
   value-typed local, or a SILENT wrong-call for a fn-typed local); nor two module
   homes folding to the same `mod_ident` (CO-BACKEND-002 — `module Std.Ui` and
   `module Std_Ui` both fold via `module_prefix` to `"Std_Ui"` →
   `ipe_mod_std_ui`). The gate built for the latter,
   `assert_mod_idents_unique` (`rust_file.rs:82`), has **no production caller**
   (only a `#[cfg(test)]` call at `rust_file.rs:261/278`), and the collection that
   would catch it (`project.rs:774-786`) folds into a `BTreeSet<String>` that
   SILENTLY DEDUPES the collision instead of failing on it. The comment at
   `project.rs:770-772` asserts uniqueness is "already guaranteed" — false.
   `mangle_reserved`'s trailing-underscore rule (`naming.rs:223`) is also
   non-injective (`match` → `match_` collides with a user local literally spelled
   `match_`).

The theme fix establishes two structural properties: (A) **one builtin-ctor table,
consumed by canon + exhaust + lower**, so the three can never disagree; (B) **every
emitted Rust name is drawn from a provably-disjoint, injective namespace, and every
name-collision gate is a real fail-closed check with a live caller**, so a
representable name clash is rejected at `ipe` time (THE SEAL), never shipped to cargo.

---

## CO-TYPES-001 — exhaustiveness blind to Prelude builtin ADTs

### Root cause
The exhaust pass's constructor-signature table (`Sigs`) is a hand-written subset
(Maybe/Result only) of the full builtin-ctor set that canon and lower already
carry. Any `case` over a builtin ADT other than Maybe/Result is silently excluded
from analysis and slips to cargo as E0004.

### Design — one shared builtin-ctor table (property A)

Introduce a single source of truth for the Prelude builtin unions, in the lowest
crate all three consumers already depend on (`ipe_canon`; `types` depends on
`canon`, `lower` depends on both):

New module `ipe_canon::builtins`:

```rust
/// One Prelude built-in union: its type name and its constructors, each with
/// declaration index + payload arity. The SINGLE source of truth consumed by
/// canon (name resolution), types::exhaust (usefulness), and lower (synthetic
/// EnumDefs + coverage). Adding a variant here updates all three at once — no
/// hand-kept second copy can drift.
pub struct BuiltinUnion {
    pub type_name: &'static str,
    /// (ctor name, index, arity), declaration order.
    pub ctors: &'static [(&'static str, usize, usize)],
}

/// Every Prelude built-in union that resolves without a `type` declaration.
/// Bool/Maybe/Result/Order/SqlValue/SqlField/ChunkEvent/StreamId/Error/
/// ErrorKind/ErrorDetails. Ordering + indices are load-bearing (they pin the
/// emitted enum variant order — see the DO-NOT-REORDER notes at env.rs:291).
pub const BUILTIN_UNIONS: &[BuiltinUnion] = &[ /* … */ ];

/// Intern every builtin ctor/type name once, returning lookup tables keyed by
/// interned Symbol. Called by each consumer at setup.
pub fn intern_builtins(interner: &mut Interner) -> DResult<InternedBuiltins>;
```

`InternedBuiltins` carries the `Symbol`-keyed `ctor_to_union` / `union_ctors` /
`ctor_arity` maps (exactly the shape `exhaust::Sigs` needs) plus the
`Symbol`-keyed struct-of-fields that `lower::BuiltinCtors` needs, both derived
from the ONE `BUILTIN_UNIONS` const.

Then:
- `canon::env::install_builtin_ctors` iterates `BUILTIN_UNIONS` to build its
  `CtorHome` entries (replacing the hand-written `for (name, type_name, index,
  arity)` array at `env.rs:284-347`). Same `home: Vec::new()` convention.
- `exhaust::Sigs::build` seeds `ctor_to_union`/`union_ctors`/`ctor_arity` from
  `intern_builtins` INSTEAD of the hand-written Maybe/Result-only block
  (`exhaust.rs:80-106`). Now every builtin union is analysable → the nested
  `ErrorKind` arm set IS checked → the missing 9 variants surface as
  `TypeError::NonExhaustiveCase` (IPE-T0010) at `ipe` time.
- `lower`'s `BuiltinCtors` is populated from `intern_builtins` rather than the
  hand-written `interner.intern(...)` block (`lib.rs:142-190`).

This is parse-don't-validate at the table level: the builtin set is parsed ONCE
into typed `Symbol` tables and every stage consumes the same value; the drifting
subset becomes unrepresentable.

Go/Elm parity: unaffected — Elm/Go both check exhaustiveness over these unions;
this restores parity the Rust port dropped. No divergence record.

### Impl plan
1. `ipe_canon`: add `builtins.rs` with `BuiltinUnion`, `BUILTIN_UNIONS` (port the
   exact ctor/index/arity rows from `env.rs:284-347`), `InternedBuiltins`,
   `intern_builtins`. Unit test: every row's index is dense per union starting at
   0; every ctor name interns.
2. `canon::env`: rewrite `install_builtin_ctors` to iterate `BUILTIN_UNIONS`.
   Regression: existing canon tests over `Just`/`Error kind info`/`SqlString`
   patterns still resolve (no behaviour change, same `CtorHome`s).
3. `types::exhaust`: replace the Maybe/Result seed in `Sigs::build` with
   `intern_builtins`. Independently testable via the negative suite below.
4. `lower`: build `BuiltinCtors` from `intern_builtins`; delete the hand-written
   field block. Existing lowering goldens must stay byte-identical (the indices
   are preserved).
5. **Negative test** (`negative_suite.rs`) — the audit's exact repro. The fix
   makes it a REJECTION at `IPE-T0010`:
   ```rust
   /// A `case` over a Prelude builtin ADT (`ErrorKind` under `Maybe`) that omits
   /// variants must be caught as non-exhaustive at `ipe` time (IPE-T0010), not
   /// slip to cargo as E0004. Guards CO-TYPES-001 — exhaust must analyse EVERY
   /// builtin union, not just Maybe/Result.
   #[test]
   fn exhaust_builtin_adt_nested_nonexhaustive() {
       let src = format!(
           "{HEAD}import Ipe.Log exposing (println)\n\
            describe : Maybe ErrorKind -> String\n\
            describe m =\n    case m of\n        \
            Just Io      -> \"io\"\n        \
            Just Network -> \"net\"\n        \
            Nothing      -> \"none\"\n\n\
            main = println (describe Nothing)\n");
       assert_rejected("exhaust_builtin_adt", &src, "IPE-T0010");
   }
   ```
   Add a second fixture for the TOP-level `case kind of Io -> …; Network -> …`
   shape (the (b) variant the verdict flags: today a `Diagnostic::CompilerBug`
   "top constructors cover 2 of 11"). After the fix it MUST also be IPE-T0010,
   not a CompilerBug:
   ```rust
   #[test]
   fn exhaust_builtin_adt_toplevel_nonexhaustive() {
       let src = format!(
           "{HEAD}import Ipe.Log exposing (println)\n\
            classify : ErrorKind -> String\n\
            classify k =\n    case k of\n        \
            Io      -> \"io\"\n        Network -> \"net\"\n\n\
            main = println (classify Io)\n");
       assert_rejected("exhaust_builtin_toplevel", &src, "IPE-T0010");
   }
   ```
6. Positive regression (goldens / examples): an EXHAUSTIVE `case` over `SqlValue`
   / `ChunkEvent` still compiles and runs — the documented `forEachChunk`
   `ChunkEvent` handler shape must stay green. Add a tiny example or golden if one
   does not already exercise all-variant coverage.

### Risk / blast radius
- The exhaust pass now fires on programs it previously skipped. Any EXISTING
  example / golden with a genuinely non-exhaustive builtin-ADT `case` will newly
  (correctly) fail `ipe` — that is the bug surfacing, fix the example per §0, do
  not weaken the gate. Full examples sweep is the gate.
- Index/arity rows are load-bearing for emitted enum variant order (the
  DO-NOT-REORDER notes). Porting them verbatim into `BUILTIN_UNIONS` is
  mandatory; a reorder is a silent runtime miscompile. Golden byte-diff catches it.
- Re-gate: `negative_suite.rs`, exhaust unit tests, lowering goldens, full
  examples sweep.

---

## CO-BACKEND-001 — local shadows a bare-emitted top-level fn

### Root cause
Top-level fn calls emit an UNQUALIFIED Rust name (`main_update(args)`), and locals
emit the same verbatim namespace, so a local whose Ipê spelling equals a
top-level fn's folded name shadows the fn in the emitted Rust. The only gate
(`lib.rs:590`) checks top-level names against each other, never against locals.
`mangle_reserved` is additionally non-injective.

### Design — qualify emitted top-level calls (property B, the structural fix)

The audit's framing offers two fixes; the SEAL-closing, class-killing one is
**qualify every emitted top-level fn reference with `crate::`**, exactly as
`Callee::Ffi` already does. A `crate::main_update(args)` call ALWAYS resolves to
the top-level `fn`, never to a local `let` binder, because `crate::` is an absolute
path that a local binding cannot shadow. This closes BOTH the value-typed case
(no more E0618) and the fn-typed case (no more silent wrong-call) at the root — a
local can never intercept a `crate::`-qualified call, for ANY name, so the entire
collision class disappears rather than being detected per-name.

Change `callee_name` (`emit_expr.rs:941-949`):
```rust
Callee::Func(id) => Ok(format!("crate::{}", ctx.func_name(*id)?)),
```
The `ipe_main` entry-point special case is unaffected (it is called as `ipe_main()`
from the fixed epilogue, which is itself at crate root). Verify every OTHER
top-level-fn reference site routes through `callee_name` / `func_name`; a
partial-take-a-reference site (passing a bare fn as a value, eta-expansion) must
also emit `crate::` — audit `func_name` call sites and qualify each, or centralise
so `func_name` itself returns the qualified path.

**`mangle_reserved` non-injectivity — the secondary, narrower fix.** Qualifying
top-level calls does not fix `match` → `match_` colliding with a user local
literally spelled `match_` (both are locals, both verbatim). Make the mangle
injective by reserving the mangled forms: a user identifier that already ends in
`_` and would collide with a keyword-mangle target is itself mangled to a
provably-disjoint form. The minimal injective rule: mangle `s` to `s_` ONLY when
`s` is reserved; ALSO mangle any user identifier equal to `<reserved>_` (e.g.
`match_`) by the same `+_` rule (→ `match__`), so the reserved-mangle image and
the user-identifier image never intersect. Document the rule at `naming.rs:223`
and unit-test injectivity over the reserved set × the `<kw>_` shadow set.

Parity: qualifying emitted names is an internal codegen detail; no observable
behaviour change, no divergence record.

### Impl plan
1. `emit_expr.rs`: qualify `Callee::Func` in `callee_name` with `crate::`. Audit
   all `func_name` call sites (partial application, eta) and ensure each emits the
   qualified path.
2. `naming.rs`: make `mangle_reserved` injective (reserve `<kw>_` shadow set).
   Unit test: `mangle_reserved` is injective over `{reserved} ∪ {kw_}`.
3. **Negative / regression tests.** CO-BACKEND-001's core is a SEAL break, so the
   right test is a POSITIVE compile-AND-run test (the program is well-formed; the
   fix makes it emit correct Rust) plus a SEAL assertion. The negative suite only
   asserts rejection, so add these where they belong:
   - In `negative_suite.rs`, a `mangle_reserved`-collision program that, PRE-fix,
     shipped E0415/E0124 — after the injective fix it must COMPILE (add a
     positive-compile helper `assert_accepted(name, source)` if absent, asserting
     `Outcome::Accepted("compiled successfully (exit 0)")` AND that the emitted
     crate `cargo build`s — reuse the sweep's cargo step). Program:
     ```elm
     module Main exposing (main)
     import Ipe.Prelude exposing (..)
     import Ipe.Log exposing (println)

     update : Int -> Int -> Int
     update a b = a + b

     shadowed : Int -> String
     shadowed n =
         let
             main_update = n            -- Int local, spells top-level fold of Main.update
         in
         String.fromInt (update main_update 5)   -- crate::main_update(...) resolves to the fn

     main = println (shadowed 3)
     ```
     Assert exit-0 AND the emitted crate `cargo build`s (the SEAL) AND runs to the
     expected `8`.
   - A fn-typed-local variant (a lambda local named like a top-level fn, then a
     call to the top-level fn) asserting the RUN output proves the correct fn is
     invoked (guards the silent-wrong-call, which has no cargo signal — only a
     behavioural test catches it).
   - A `mangle_reserved` injectivity fixture: a program with a local `match_` AND
     a construct that mangles `match` → `match_`, asserting compile+run.
4. If a positive-compile-and-run harness does not already exist in
   `negative_suite.rs`, the cleaner home is a new `tests/seal_regression.rs` in
   `ipe-cli` gated on `IPE_E2E` (mirrors the sweep's build+run). State this in the
   backlog entry.

### Risk / blast radius
- `crate::`-qualifying every top-level call touches a hot emission path; every
  existing golden's call sites change from `main_update(...)` to
  `crate::main_update(...)`. Goldens must be regenerated and byte-reviewed (a
  large but mechanical diff). This is the main blast radius — flag it.
- The `ipe_main` epilogue call and any hand-written runtime-glue call site must be
  confirmed still correct (they are already crate-root).
- Re-gate: full examples sweep (build+run), all goldens, `negative_suite.rs` +
  the new SEAL-regression tests.

---

## CO-BACKEND-002 — dead `assert_mod_idents_unique` gate

### Root cause
Two distinct module homes can fold to the same `mod_ident` (`module Std.Ui` and
`module Std_Ui` both → `ipe_mod_std_ui`, because `module_prefix` joins segments
with `_` and both produce `"Std_Ui"`). The gate written to catch this
(`assert_mod_idents_unique`, `rust_file.rs:82`) is never called on the production
path; the collection that could catch it (`project.rs:774-786`) silently dedupes
into a `BTreeSet`. Two identical `mod ipe_mod_std_ui;` decls → E0428; the second
`src/ipe_mods/ipe_mod_std_ui.rs` source silently overwrites the first module's
items.

### Design — revive the gate as a real fail-closed check (property B)

Two layers, both required:

1. **Call the gate.** In `project::emit_program`, immediately after `module_homes`
   is collected and BEFORE the split branch writes any `mod` decl / source file
   (`project.rs:~786`), call `rust_file::assert_mod_idents_unique(&module_homes,
   ctx.interner)?`. It already returns `NameError::DuplicateValue` (→ IPE-N0010) on
   a collision. Replace the silently-deduping `mod_idents: BTreeSet` collection
   with this fail-closed pass; if the disjointness check against the record-struct
   namespace still needs a set, build it AFTER uniqueness is proven. Delete the
   false "already guaranteed" comment at `project.rs:770-772` and the
   `#[cfg(test)]`-only status of the gate.

2. **Make the collision structurally rarer / impossible by fixing the fold's
   non-injectivity at the source (deeper fix).** `mod_ident` folds
   `module_prefix(home)` (segments joined by `_`) then `to_snake_case`. The
   join-by-`_` is what conflates `["Std","Ui"]` with `["Std_Ui"]`. Since Ipê
   module segments cannot themselves contain `.` (the parser splits on `.`), the
   ONLY ambiguity is a literal `_` inside a segment vs. the segment separator. The
   robust fix: fold with a separator that cannot appear in a segment OR escape `_`
   within a segment before joining (e.g. `_` → `__` inside a segment, then join
   with single `_`), restoring injectivity of `home → mod_ident`. With an
   injective fold, the runtime gate (layer 1) becomes a belt-and-braces
   assertion that can only fire on a genuine internal bug — but it MUST still be
   wired (a dead gate with a lying comment is itself a §0 principles violation).

   Note the same non-injectivity underlies `naming::module_value` / `enum_name`
   (both use `module_prefix`); their pairwise gates (`lib.rs:544/590`) currently
   CATCH the fn/enum case as IPE-N0010 but only because they compare values. Fixing
   `module_prefix`'s injectivity (or the escaping) at the shared helper closes the
   whole conflation class — prefer this over patching `mod_ident` alone.

The audit's suggested one-liner ("just call the gate") is necessary but a
band-aid: it turns a silent overwrite into an IPE-N0010 rejection, which is
correct fail-closed behaviour, but leaves the fold non-injective so legal
distinct modules are REJECTED that need not be. The injective-fold fix is the
root cause. Ship both: the gate (fail-closed, mandatory) AND the injective fold
(so the rejection only fires on true duplicates).

Parity: `module_prefix` is an internal codegen fold; changing its escaping is not
observable to a well-formed single-`.`-segment program. If the escaping changes
any existing multi-module golden's `mod_ident`, that is a mechanical golden
regen, not a divergence.

### Impl plan
1. `rust_file.rs` / `naming.rs`: make the `home → mod_ident` fold injective
   (escape `_` within a segment before the `_`-join, or route through an injective
   `module_prefix`). Unit test: `mod_ident(["Std","Ui"]) != mod_ident(["Std_Ui"])`,
   and both round-trip-distinct for the enum/value folds too.
2. `project.rs`: call `assert_mod_idents_unique(&module_homes, ctx.interner)?`
   before the split branch; remove the false comment; replace the deduping
   `BTreeSet` collection with a post-uniqueness set for the record-struct
   disjointness check.
3. `rust_file.rs`: remove the `#[cfg(test)]`-only status implication — the gate is
   now a production caller; keep its existing unit test.
4. **Negative test** (`negative_suite.rs`, via `compile_project` — this is a
   cross-module gate the single-file path cannot observe). With the injective-fold
   fix, `Std.Ui` and `Std_Ui` fold to DISTINCT idents and compile; to test the
   GATE itself we need a genuine collision. Two sub-cases:
   - **Post-injective-fold, the gate fires only on a true duplicate.** Construct
     two modules that genuinely must share a `mod_ident` only via an internal
     path — hard to reach from source once the fold is injective. So the
     primary negative test asserts the PRE-fold-collision pair now COMPILES:
     ```rust
     /// `module Std.Ui` and `module Std_Ui` fold to DISTINCT mod idents after the
     /// injective-fold fix — both compile. Guards CO-BACKEND-002's root cause: the
     /// `_`-join must not conflate a dotted path with an underscore-in-segment name.
     #[test]
     fn modident_dot_vs_underscore_distinct() {
         let files = &[
             ("Main.ipe", "module Main exposing (main)\n\
                 import Ipe.Prelude exposing (..)\n\
                 import Ipe.Log exposing (println)\n\
                 import Std.Ui exposing (foo)\n\
                 import Std_Ui exposing (bar)\n\
                 main = println (foo ++ bar)\n"),
             ("Std/Ui.ipe", "module Std.Ui exposing (foo)\nfoo = \"a\"\n"),
             ("Std_Ui.ipe", "module Std_Ui exposing (bar)\nbar = \"b\"\n"),
         ];
         // assert_accepted_project: exit 0 AND emitted crate cargo-builds (SEAL).
         assert_accepted_project("modident_distinct", files);
     }
     ```
   - **The gate is live** — assert `assert_mod_idents_unique` rejects a
     hand-built `&[RustFileId]` with two homes folding to one ident, as a unit
     test in `rust_file.rs` (it exists; keep + extend it to prove IPE-N0010 is the
     wire code). This proves the fail-closed path even though source can no longer
     reach it post-injective-fold.
5. Add `assert_accepted_project` to `negative_suite.rs` (or the new
   `seal_regression.rs`), mirroring `compile_project` but asserting acceptance +
   emitted-crate `cargo build` under `IPE_E2E`.

### Risk / blast radius
- Changing `module_prefix`/`mod_ident` escaping alters every multi-module golden's
  `mod_ident` and possibly `enum_name`/`module_value`. Mechanical golden regen +
  byte review. Confirm single-module goldens (the common case, no `_`-in-segment)
  are UNCHANGED — the escaping must be a no-op when no segment contains `_`.
- Wiring the gate can newly reject a program that previously silently overwrote —
  correct, but surface it in the sweep.
- Re-gate: multi-module goldens, `clean_vs_incremental_parity.rs`,
  `parity_multimodule_adversarial_edits`, `negative_suite.rs`, full sweep.

---

## Cross-cutting: the negative suite missed all three

The audit proved `negative_suite.rs` has no coverage for (a) exhaustiveness over
builtin ADTs, (b) local-vs-top-level shadowing, (c) mod-ident collision. Two of
the three are SEAL breaks whose test is "compiles at `ipe` AND the emitted crate
`cargo build`s (and runs)", which the current rejection-only harness cannot
express. The shared prerequisite for CO-BACKEND-001 and CO-BACKEND-002 tests is a
**positive-compile-and-cargo-build assertion** (`assert_accepted` /
`assert_accepted_project`) gated on `IPE_E2E`, reusing the examples-sweep's
build-and-run step. Land that harness helper first; the three per-finding tests
above then attach to it. This closes the meta-gap: THE SEAL's contrapositive
(rejection) is covered by `negative_suite.rs`; THE SEAL itself (accept ⇒
cargo-green) now gets regression coverage too.

---

## Proposed backlog entries

```json
{"id": "TBD", "priority": "high", "phase": "principles-audit-fix", "task": "T2/A: one shared Prelude builtin-ctor table (ipe_canon::builtins) consumed by canon+exhaust+lower; fixes CO-TYPES-001 (exhaust blind to builtin ADTs -> E0004 SEAL break). Add BuiltinUnion/BUILTIN_UNIONS/intern_builtins; rewrite install_builtin_ctors, Sigs::build, lower::BuiltinCtors to consume it. Negative tests: exhaust_builtin_adt_nested_nonexhaustive + exhaust_builtin_adt_toplevel_nonexhaustive (both IPE-T0010).", "notes": "Root cause = three hand-maintained copies; exhaust had a Maybe/Result-only subset. Port index/arity rows VERBATIM (load-bearing enum order). Re-gate: exhaust unit tests, lowering goldens, full examples sweep.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t2-seal-closure.md", "blocked_by": [], "status": "pending"}
{"id": "TBD", "priority": "high", "phase": "principles-audit-fix", "task": "T2/B1: qualify emitted top-level fn calls with crate:: in callee_name (emit_expr.rs:941-949) + audit all func_name reference sites; make mangle_reserved injective (naming.rs). Fixes CO-BACKEND-001 (local shadows bare top-level fn -> E0618 SEAL / silent wrong-call). Positive SEAL-regression tests: value-local shadow compiles+runs; fn-local shadow invokes the top-level fn (behavioural, no cargo signal); mangle_reserved match_ collision compiles.", "notes": "crate:: qualification kills the whole shadow class (a local cannot shadow an absolute path). Blast radius: every golden's call sites gain crate:: -> mechanical golden regen + byte review. Depends on the positive-compile-and-run harness helper.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t2-seal-closure.md", "blocked_by": [], "status": "pending"}
{"id": "TBD", "priority": "medium", "phase": "principles-audit-fix", "task": "T2/B2: revive assert_mod_idents_unique as a live fail-closed gate (call it in project::emit_program before the split branch; remove the false 'already guaranteed' comment + deduping BTreeSet); make the home->mod_ident fold injective (escape _ within a segment before the _-join, ideally at shared module_prefix). Fixes CO-BACKEND-002 (Std.Ui vs Std_Ui -> E0428 + silent file overwrite). Negative/regression: modident_dot_vs_underscore_distinct (now compiles) + rust_file unit test proving the gate returns IPE-N0010.", "notes": "Gate call = fail-closed band-aid; injective fold = root cause (so legal distinct modules are not rejected). Escaping MUST be a no-op when no segment contains _ (single-module goldens unchanged). Re-gate: multi-module goldens, clean_vs_incremental_parity, parity_multimodule_adversarial_edits, full sweep.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t2-seal-closure.md", "blocked_by": [], "status": "pending"}
{"id": "TBD", "priority": "high", "phase": "principles-audit-fix", "task": "T2/tests: add a positive compile-AND-cargo-build(-and-run) assertion harness to ipe-cli tests (assert_accepted / assert_accepted_project, IPE_E2E-gated, reusing the examples-sweep build+run step). Closes the meta-gap: negative_suite.rs covers THE SEAL's contrapositive (rejection) but not THE SEAL itself (accept => cargo-green). Prerequisite for the CO-BACKEND-001/002 regression tests.", "notes": "Two of the three T2 findings are SEAL breaks whose regression test is 'accepts at ipe AND emitted crate cargo-builds/runs' — the rejection-only harness cannot express this. Land this helper first (new tests/seal_regression.rs or extend negative_suite.rs).", "spec": "docs/audit/2026-07-17-principles-audit/specs/t2-seal-closure.md", "blocked_by": [], "status": "pending"}
```
