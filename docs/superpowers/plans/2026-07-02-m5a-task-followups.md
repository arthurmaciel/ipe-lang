# Plan — M5a Task fail-closed follow-ups (arity ICE + Task-in-ADT-ctor gate)

**Status:** ready to execute (doc-only design-then-plan; no code written yet)
**Tracks:** task #32 (M5a follow-ups, non-blocking, fail-closed)
**Author:** guardian planner (#32)

---

## Goal

Close two fail-closed gaps in the `Task` type-handling path so that no
well-typed-looking *source* program can reach a `CompilerBug` (an ICE, rendered
"please report") or emit `cargo`-rejecting Rust:

1. **Task arity ICE.** A user annotation whose `Task` is applied to a number of
   type arguments other than 1 or 2 — `x : Task Error a b` (3) or `x : Task`
   (0) — currently returns `Diagnostic::CompilerBug` from
   `constrain.rs::normalize_annotation_ty` (the `n` arm). It is *reachable from
   source*: canonicalisation does not validate the arity of non-alias type
   constructors (verified below), so the stale code comment claiming
   "canonicalisation rules out arity-0 or arity-3+" is wrong. Replace the ICE
   with a spanned, fail-closed **type** diagnostic (`SKY-T0015`).

2. **Task-in-ADT-constructor mis-lower.** A well-formed `Task Error a` (arity 2)
   embedded in a constructor payload — `type Job = Job (Task Error Int)` —
   passes every existing `lower_enum` gate and lowers to a `Variant` carrying an
   `IrType::Task` field. The backend emits the enum with
   `#[derive(Clone, Debug, PartialEq)]` + a `SkyStringify` impl
   (`emit_types.rs:220`), none of which the runtime `SkyTask<A>` (a
   `Pin<Box<dyn Future + Send>>`, `core.rs`) satisfies — so `cargo` rejects the
   emitted crate. Add a fail-closed **lowering** gate (`SKY-L0119`) that rejects
   a `Task`/`Cmd`/`Sub` value in a constructor payload before broken Rust is
   emitted, mirroring the existing `SKY-L0114` function-in-payload gate.

Each fix ships with a golden-fixture regression that currently ICEs (case 1) or
silently emits `cargo`-failing Rust (case 2), plus the diagnostic-taxonomy
wiring (code const, title, explain page, render arm) the CI gates require.

## Architecture

The compiler pipeline: `sky_parse` → `sky_canon` → `sky_types` (constrain +
solve) → `sky_lower` (canon+solved → `sky_ir`) → `sky_backend_rust` (IR → Rust
crate) → `cargo`. The diagnostic currency is the single `Diagnostic` enum in
`sky_diagnostics`; every user-facing failure is one of `Parse | Name | Type |
Lower`, and `CompilerBug` is reserved for *violated internal invariants* — never
for a shape a source program can construct.

The two fixes sit at two distinct pipeline boundaries and therefore use two
distinct diagnostic kinds:

- **Fix 1** is a *type-well-formedness* fact about an annotation, caught at the
  constrain boundary (`normalize_annotation_ty`, applied to every result of
  `from_canon`). It becomes a `Diagnostic::Type` (`TypeError::TaskArity`,
  `SKY-T0015`).
- **Fix 2** is a *backend-capability* fact about a constructor payload, caught at
  the lowering boundary (`lower_enum`, per constructor field). It becomes a
  `Diagnostic::Lower` (`LowerError::Unsupported(Feature::CtorPayloadTask)`,
  `SKY-L0119`), the natural sibling of `Feature::CtorPayloadFunction`
  (`SKY-L0114`).

A third, secondary reachable ICE — a mis-arity `Task` in a *constructor field
type* (`type J a = J (Task Error a Bool)`), which never passes through
`normalize_annotation_ty` and instead hits the `CompilerBug` at
`lower.rs:1462` inside `ir_type_from_canon` — is closed as a step of Task 2,
reusing `SKY-T0015` via a spanned pre-check in `lower_enum` (canon carries no
inner type spans, so `ctor.span` is the emission anchor). The `CompilerBug` arms
at `lower.rs:1462` and `lower.rs:1781` remain as genuine unreachable-defence
(the solver only ever mints unary `Task`; canon field types are pre-checked).

## Tech Stack

- Rust (workspace, `rust-toolchain.toml` pins the edition/toolchain).
- Crates touched: `sky_diagnostics` (code/diagnostic/render + explain page),
  `sky_types` (constrain), `sky_lower` (lower). No `sky_ir`, no
  `sky_backend_rust` source changes (Fix 2 rejects *before* emission; the
  backend derive behaviour is the thing being protected, not changed).
- Tests: `cargo test -p sky_diagnostics` (taxonomy + explain-page gates),
  `cargo test -p skyc --test golden_m5a_task_gates` (existing gate harness,
  extended), plus one new golden test binary
  `crates/skyc/tests/golden_m5a_ctor_task_gate.rs`.
- Golden fixtures live under `tests/golden/<name>/Main.sky` and are driven by the
  `assert_gate`/`skyc::build` harness already used by
  `golden_m5a_task_gates.rs`.

## Global Constraints

**PRINCIPLES order (ties resolve to the earlier):**
`security > correctness > soundness > efficiency > completeness > readability`.
Here the dominant principles are **correctness** (a source program must never
ICE) and **soundness** (a source program must never silently emit
`cargo`-failing Rust). Both fixes are strict tightenings — they reject programs
that previously ICE'd or mis-lowered; no previously-accepted program changes
behaviour.

**Two fundamental design rules:**

- **Parse, don't validate.** Turn the wrong-arity / non-derivable-payload
  invariant into a precise typed failure at the *first* boundary that can see it
  (constrain for annotations; lower for constructor payloads), so no downstream
  stage re-checks or trips over it. The two surviving `CompilerBug` arms in
  `lower.rs` are the parsed-away invariant's unreachable-defence, not a second
  validation.
- **Make invalid states unrepresentable.** After Fix 2, an `EnumDef` `Variant`
  can never carry a `Task`/`Cmd`/`Sub`-bearing field — the state that produced
  non-building Rust is rejected at construction of the IR, so it cannot flow to
  the backend. Fix 1 does the same for a `Ty::Con{Task}` of arity ∉ {1,2}
  reaching the solver.

**Fail-closed, not wildcards.** Every new match arm is explicit. The
`TypeError` severity match (`diagnostic.rs:705`), secondary-span match
(`diagnostic.rs:933`), `code_of` map (`diagnostic.rs:797`), and render label
match (`render.rs:417`) are all non-wildcard exhaustive over `TypeError` — the
new `TaskArity` variant MUST be added to each, and the compiler enforces it.
Likewise `Feature` → `code_of_feature` (`diagnostic.rs:820`) and the
`feature_label` match (`render.rs:548`) are exhaustive over `Feature`.

**PUBLIC-artifact rule.** Where `../sky` (the Go reference compiler) is named,
it is a parity/capability reference only. Fact for the explain pages: the Go
reference *also* fails `type Job = Job (Task ...)` today (its codegen emits Go
that `go build` rejects — same class as the hand-verified `SKY-L0114` note in
`golden_m3a_function_payload_gate.rs`). State the difference (ipê gives a clean
diagnostic where the reference emits non-building output) without disparagement.

**Parallel-safety / file-overlap (read before starting):**

- **Registry migration (in-flight, task-adjacent; a locked worktree
  `agent-a9d7784a177721ba3` exists at HEAD `691e275`).** It edits
  `constrain.rs` and `lower.rs` too, but in *disjoint regions*: the kernel-scheme
  table (`constrain.rs` ~line 2330, `(Some("Task"), Some("succeed"))` …) and the
  callee-resolution match (`lower.rs` ~line 3793, `("Task","succeed") =>
  Callee::Kernel(...)`). This plan touches `constrain.rs::normalize_annotation_ty`
  (~1264–1332) and `lower.rs::lower_enum` (~1061–1094) + `ir_type_from_canon`
  Task arm (~1450–1468) — **no overlap** with the kernel/callee tables. The one
  real coordination point is **code-number allocation**: if the registry work
  also adds new `SKY-T####`/`SKY-L####` codes, `SKY-T0015`/`SKY-L0119` and the
  `ALL` list + count assertions (`code.rs:420`, `code.rs:438`) will collide on
  rebase. Allocate the next free numbers at merge time and re-run
  `cargo test -p sky_diagnostics`.
- **#49 TCO (pending).** It adds 2 variants to `sky_ir::ir.rs` and edits
  `lower.rs` (expression lowering) + `emit_expr.rs`. This plan adds **no**
  `sky_ir` variant and edits `lower.rs` only in `lower_enum` / `ir_type_from_canon`
  (type lowering, not expression lowering) — region-disjoint from TCO. No
  `emit_expr.rs` touch. Low collision risk; rebase `lower.rs` region-wise.

**Line anchors are HEAD (`691e275`) — re-verify before editing; a rebase past
either in-flight branch may shift them. Every anchor below was read against HEAD
for this plan.**

---

## Task 1 — `SKY-T0015`: fail-closed `Task` arity diagnostic (annotation path)

Replace the reachable-from-source `CompilerBug` in
`constrain.rs::normalize_annotation_ty` with a spanned `TypeError::TaskArity`,
and wire the new code through the diagnostics taxonomy.

**Files**
- `crates/sky_diagnostics/src/diagnostic.rs` (variant + code_of + severity +
  secondary-span + `use` import)
- `crates/sky_diagnostics/src/code.rs` (const + title + explain_page + `ALL` +
  two count assertions)
- `crates/sky_diagnostics/src/render.rs` (label arm)
- `crates/sky_diagnostics/explain/SKY-T0015.md` (new)
- `crates/sky_types/src/constrain.rs` (the `n` arm, ~line 1326)
- `tests/golden/m5a_gate_task_arity3/Main.sky` (new)
- `tests/golden/m5a_gate_task_arity0/Main.sky` (new)
- `crates/skyc/tests/golden_m5a_task_gates.rs` (extend)

**Interfaces**

_Consumes:_
- `TypeError` (`diagnostic.rs:401`) — enum of type failures; currently ends at
  `SuperTypeUnsatisfied` (`SKY-T0014`).
- `Builder::normalize_annotation_ty(&self, ty: Ty, span: Span) -> DResult<Ty>`
  (`constrain.rs:1264`); the `n` arm:
  ```rust
  n => Err(Diagnostic::CompilerBug {
      where_: STAGE,
      detail: format!("Task annotation with {n} type argument(s); expected 1 or 2"),
  }),
  ```
- `assert_gate(fixture: &str, out_suffix: &str, expected: sky_diagnostics::Code)`
  (`golden_m5a_task_gates.rs:22`) — builds `tests/golden/<fixture>/Main.sky`,
  asserts the pipeline diagnostic `.code()` equals `expected`, never a panic.

_Produces:_
- `TypeError::TaskArity { found: usize }` (new variant).
- `pub const SKY_T0015: Code = Code("SKY-T0015");`
- `code_of` mapping `TypeError::TaskArity { .. } => SKY_T0015`.
- Constrain `n` arm now returns
  `Diagnostic::Type { span, msg: TypeError::TaskArity { found: n } }`.

**Steps**

1. **Write the failing regression fixtures + tests (red).**
   Create `tests/golden/m5a_gate_task_arity3/Main.sky`:
   ```sky
   module Main exposing (main)

   -- `Task Error a b` applies `Task` to THREE type arguments. `Task` takes
   -- exactly its error channel (`Error`) and one success type; three args is a
   -- fail-closed type error (SKY-T0015), NOT a compiler ICE.
   doThing : Task Error Int Bool
   doThing =
       Task.succeed 42

   main =
       println "test"
   ```
   Create `tests/golden/m5a_gate_task_arity0/Main.sky`:
   ```sky
   module Main exposing (main)

   -- Bare `Task` with no type arguments — also SKY-T0015 (arity 0).
   doThing : Task
   doThing =
       Task.succeed 42

   main =
       println "test"
   ```
   Append to `crates/skyc/tests/golden_m5a_task_gates.rs`:
   ```rust
   /// `Task Error a b` (3 args) must be rejected with SKY-T0015 (Task arity),
   /// NEVER a CompilerBug. Regression for the M5a arity ICE.
   #[test]
   fn task_arity_three_is_sky_t0015() {
       assert_gate(
           "m5a_gate_task_arity3",
           "m5a_gate_task_arity3_emit",
           sky_diagnostics::SKY_T0015,
       );
   }

   /// Bare `Task` (0 args) must also be SKY-T0015, not a CompilerBug.
   #[test]
   fn task_arity_zero_is_sky_t0015() {
       assert_gate(
           "m5a_gate_task_arity0",
           "m5a_gate_task_arity0_emit",
           sky_diagnostics::SKY_T0015,
       );
   }
   ```

2. **Run — confirm it fails for the right reason.**
   ```
   cargo test -p skyc --test golden_m5a_task_gates task_arity 2>&1 | tail -20
   ```
   Expected: both tests FAIL because `SKY_T0015` does not yet exist (compile
   error `no associated item named SKY_T0015`). This is the red state — it also
   proves the code const is missing. (Once the const exists but the constrain
   arm is unchanged, the failure becomes `expected SKY-T0015, got CompilerBug`,
   which is the runtime red state.)

3. **Add the `SKY-T0015` code const, title, and `ALL` membership (minimal).**
   In `crates/sky_diagnostics/src/code.rs`:
   - After `pub const SKY_T0014` (line 149) add:
     ```rust
     /// `Task` applied to a wrong number of type arguments (needs exactly 1 or 2)
     pub const SKY_T0015: Code = Code("SKY-T0015");
     ```
   - In `title` (the `SKY_T0014 => ...` arm at line 273) add after it:
     ```rust
     SKY_T0015 => "the `Task` type takes exactly an error channel and a success type",
     ```
   - Add `SKY_T0015` to the `ALL` slice (`code.rs:410`, in the `SKY_T00..`
     run) and bump BOTH count assertions from `75` → `76`
     (`code.rs:420` `taxonomy_has_seventy_five_codes`, and `code.rs:438`
     `assert_eq!(seen.len(), 75)`). Rename the test fn to
     `taxonomy_has_seventy_six_codes` for honesty (Task 2 bumps to 77).

4. **Create the explain page.**
   `crates/sky_diagnostics/explain/SKY-T0015.md` — line 1 MUST be exactly
   `# SKY-T0015: the \`Task\` type takes exactly an error channel and a success type`
   (must equal `format!("# {}: {}", code, title(code))` — the
   `every_code_has_a_conforming_explain_page` gate at `code.rs:447` asserts it)
   and the body MUST contain **≥ 3** ```` ```sky ```` fences. Draft:
   ```md
   # SKY-T0015: the `Task` type takes exactly an error channel and a success type

   `Task` is written `Task Error a` — its error channel is always `Error`, and
   its success type is the one value it produces. Applying it to any other
   number of type arguments is a type error.

   ## Three arguments

   ```sky
   doThing : Task Error Int Bool    -- three args: not a Task shape
   doThing = Task.succeed 42

   main = println "unreachable"
   ```

   ## No arguments

   ```sky
   doThing : Task               -- bare Task carries no success type
   doThing = Task.succeed 42

   main = println "unreachable"
   ```

   ## The correct shape

   ```sky
   doThing : Task Error Int     -- OK: error channel + success type
   doThing = Task.succeed 42

   main = println "test"
   ```

   Note: the Go reference compiler accepts some of these shapes and only fails
   later in code generation; ipê rejects them here, at the type boundary, with a
   precise message.
   ```

5. **Add the `TaskArity` variant + wire the four exhaustive matches.**
   In `crates/sky_diagnostics/src/diagnostic.rs`:
   - Add to `TypeError` (after `SuperTypeUnsatisfied`, line 449):
     ```rust
     /// A `Task` type applied to a number of type arguments other than 1 or 2.
     /// `Task Error a` (2) and the internal unary `Task a` (1) are the only
     /// well-formed shapes; `found` is the offending count. [SKY-T0015]
     TaskArity { found: usize },
     ```
   - Add `SKY_T0015` to the `use crate::code::{…}` import block
     (`diagnostic.rs:21-22`, the `SKY_T00..` run).
   - `code_of` (after line 805 `SuperTypeUnsatisfied => SKY_T0014`):
     ```rust
     TypeError::TaskArity { .. } => SKY_T0015,
     ```
   - Severity match (`diagnostic.rs:705-714`): add `| TypeError::TaskArity { .. }`
     to the `=> Severity::Error` group.
   - Secondary-span/help match (`diagnostic.rs:933-939`): add
     `| TypeError::TaskArity { .. }` to the `=> Vec::new()` group.

6. **Add the render label arm.**
   In `crates/sky_diagnostics/src/render.rs`, in the `type_label` match
   (before the `Mismatch | BudgetExceeded | StepBudgetExceeded => None` arm at
   line 438):
   ```rust
   TypeError::TaskArity { found } => Some(format!(
       "`Task` takes 1 or 2 type arguments, found {found}"
   )),
   ```

7. **Run diagnostics gates (green for the taxonomy layer).**
   ```
   cargo test -p sky_diagnostics 2>&1 | tail -20
   ```
   Expected: `test result: ok.` — `taxonomy_has_seventy_six_codes`,
   `codes_are_distinct_and_well_formed`, and
   `every_code_has_a_conforming_explain_page` all pass (the last proves the
   explain page's line 1 + fence count).

8. **Replace the constrain ICE with the fail-closed type error (green).**
   In `crates/sky_types/src/constrain.rs`, the `n` arm of
   `normalize_annotation_ty` (line 1326):
   ```rust
   n => Err(Diagnostic::Type {
       span,
       msg: TypeError::TaskArity { found: n },
   }),
   ```
   Also fix the stale doc comment at `constrain.rs:1262-1263` ("canonicalisation
   rules out arity-0 or arity-3+ applications") — it is false; replace with:
   "Returns `SKY-T0015` when a `Task` annotation carries a number of type
   arguments other than 1 or 2 (canonicalisation does not validate non-alias
   type-constructor arity, so this IS reachable from source)."
   Confirm `TypeError` is in scope in `constrain.rs` (it already constructs
   `TypeError::TypeMismatch` at line 1311 — no new import needed).

9. **Run the gate tests (green).**
   ```
   cargo test -p skyc --test golden_m5a_task_gates 2>&1 | tail -20
   ```
   Expected: `task_arity_three_is_sky_t0015` and `task_arity_zero_is_sky_t0015`
   pass, plus the pre-existing `task_bad_error_channel_is_sky_t0001` still
   passes (no regression to the arity-2 error-channel path).

10. **Full-workspace check + commit.**
    ```
    cargo test -p sky_diagnostics -p sky_types -p skyc 2>&1 | tail -30
    ```
    Expected: all green. Then:
    ```
    git add -A && git commit -m "constrain: fail-closed SKY-T0015 for wrong-arity Task annotations (was ICE)"
    ```

---

## Task 2 — `SKY-L0119`: fail-closed gate for a `Task`/`Cmd`/`Sub` in a constructor payload

Add a `lower_enum` gate that rejects a constructor field whose IR type embeds an
async-opaque runtime handle (`SkyTask`/`SkyCmd`/`SkySub`) — which cannot satisfy
the enum's derived `Clone`/`Debug`/`PartialEq` + `SkyStringify` — before the
backend emits non-building Rust. Then close the secondary mis-arity-`Task`-in-a-
constructor-field ICE by pre-checking each ctor field's canon type and reusing
`SKY-T0015` with `ctor.span`.

**Files**
- `crates/sky_diagnostics/src/diagnostic.rs` (`Feature` variant + `code_of_feature`)
- `crates/sky_diagnostics/src/code.rs` (const + title + explain_page + `ALL` +
  count 76 → 77)
- `crates/sky_diagnostics/src/render.rs` (`feature_label` arm)
- `crates/sky_diagnostics/explain/SKY-L0119.md` (new)
- `crates/sky_lower/src/lower.rs` (`lower_enum`, ~1061–1094; a new
  `ir_embeds_async_opaque` predicate near `ir_contains_fun`, ~220; a new
  `task_arity_in_canon` pre-check helper near `collect_type_vars`, ~188)
- `tests/golden/m5a_ctor_task_payload/Main.sky` (new)
- `tests/golden/m5a_ctor_task_arity3/Main.sky` (new)
- `crates/skyc/tests/golden_m5a_ctor_task_gate.rs` (new test binary)

**Interfaces**

_Consumes:_
- `Feature` (`diagnostic.rs:455`) — currently ends at `RoutedLiveApp`
  (`SKY-L0118`).
- `Lowerer::lower_enum(&self, u: &canon::Union) -> DResult<EnumDef>`
  (`lower.rs:1061`); per-field loop with Gate 1 `Polymorphism`
  (`lower.rs:1072`) and Gate 2 `CtorPayloadFunction` via `ir_contains_fun`
  (`lower.rs:1079`).
- `ir_contains_fun(ty: &IrType) -> bool` (`lower.rs:220`) — precedent walker;
  note `IrType::Task(inner) | IrType::Cmd(inner) | IrType::Sub(inner) =>
  ir_contains_fun(inner)` (line 225), which is exactly the family to reject at
  the *head*, not recurse past.
- `unsupported(span: Span, feature: Feature) -> Diagnostic` (`lower.rs:661`).
- `collect_type_vars(t: &canon::Type, out: &mut BTreeSet<Symbol>)`
  (`lower.rs:188`) — precedent canon-type walker for the arity pre-check shape.
- `TypeError::TaskArity` + `SKY_T0015` from Task 1.

_Produces:_
- `Feature::CtorPayloadTask` (new variant).
- `pub const SKY_L0119: Code = Code("SKY-L0119");`
- `code_of_feature` mapping `Feature::CtorPayloadTask => SKY_L0119`.
- `fn ir_embeds_async_opaque(ty: &IrType) -> bool` — true iff `ty` is, or nests,
  an `IrType::Task | IrType::Cmd | IrType::Sub`.
- `fn task_arity_in_canon(&self, t: &canon::Type) -> Option<usize>` — returns the
  arg count of a `Task`-headed `canon::Type::Con` when it is ∉ {1,2} (for the
  ctor-field arity pre-check), else `None`.
- Two new gates in `lower_enum`.

**Steps**

1. **Write the failing regression fixtures + test binary (red).**
   `tests/golden/m5a_ctor_task_payload/Main.sky`:
   ```sky
   module Main exposing (main)

   -- A well-formed `Task Error Int` (arity 2) embedded in a constructor payload.
   -- The generated Rust enum derives Clone/Debug/PartialEq + SkyStringify; the
   -- runtime SkyTask (a boxed future) satisfies none of them, so accepting this
   -- would emit cargo-failing Rust. The lowerer must reject it (SKY-L0119).
   type Job = Job (Task Error Int)

   run : Job -> Int
   run j =
       case j of
           Job _ -> 0

   main =
       println (String.fromInt (run (Job (Task.succeed 1))))
   ```
   `tests/golden/m5a_ctor_task_arity3/Main.sky`:
   ```sky
   module Main exposing (main)

   -- A mis-arity `Task` in a CONSTRUCTOR FIELD type. This never passes through
   -- the annotation normaliser, so it must be caught at lowering and surfaced as
   -- SKY-T0015 (Task arity), NEVER a CompilerBug.
   type Boxed a = Boxed (Task Error a Bool)

   main =
       println "test"
   ```
   New binary `crates/skyc/tests/golden_m5a_ctor_task_gate.rs` (model on
   `golden_m5a_task_gates.rs`'s `assert_gate`; copy the `repo_root` + harness
   verbatim):
   ```rust
   //! M5a fail-closed follow-up gates:
   //!  * a `Task`/`Cmd`/`Sub` in a constructor payload -> SKY-L0119 (never
   //!    cargo-failing Rust);
   //!  * a mis-arity `Task` in a constructor FIELD type -> SKY-T0015 (never an
   //!    ICE) — the constructor-field sibling of the annotation-path arity gate.

   use std::path::{Path, PathBuf};
   use skyc::CliError;

   fn repo_root() -> PathBuf {
       let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
       std::fs::canonicalize(&joined).unwrap_or(joined)
   }

   fn assert_gate(fixture: &str, out_suffix: &str, expected: sky_diagnostics::Code) {
       let root = repo_root();
       let entry = root.join("tests").join("golden").join(fixture).join("Main.sky");
       let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(out_suffix);
       let _ = std::fs::remove_dir_all(&out);
       let Ok(runtime) = skyc::resolve_runtime() else { return };
       let built = skyc::build(&entry, &out, &runtime);
       let got = match &built {
           Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
           _ => None,
       };
       assert_eq!(got, Some(expected),
           "fixture {fixture}: expected {expected:?}, got {built:?}");
   }

   #[test]
   fn task_in_ctor_payload_is_sky_l0119() {
       assert_gate("m5a_ctor_task_payload", "m5a_ctor_task_payload_emit",
           sky_diagnostics::SKY_L0119);
   }

   #[test]
   fn mis_arity_task_in_ctor_field_is_sky_t0015() {
       assert_gate("m5a_ctor_task_arity3", "m5a_ctor_task_arity3_emit",
           sky_diagnostics::SKY_T0015);
   }
   ```

2. **Run — confirm red.**
   ```
   cargo test -p skyc --test golden_m5a_ctor_task_gate 2>&1 | tail -20
   ```
   Expected: compile error `no associated item named SKY_L0119` (const missing)
   — red. (After the const lands but before the gate: `task_in_ctor_payload_...`
   would report `got Ok(...)` or a cargo failure, and
   `mis_arity_..._sky_t0015` would report `got CompilerBug`.)

3. **Add `SKY-L0119` code const, title, `ALL` membership, count bump.**
   In `code.rs`: after `SKY_L0118` (line 190):
   ```rust
   /// a `Task` / `Cmd` / `Sub` stored in a constructor payload not supported yet
   pub const SKY_L0119: Code = Code("SKY-L0119");
   ```
   `title` arm (after the `SKY_L0118 => ...` at line 291):
   ```rust
   SKY_L0119 => "storing a `Task`, `Cmd`, or `Sub` in a constructor payload is not supported yet",
   ```
   Add `SKY_L0119` to `ALL` (`code.rs:411`, the `SKY_L01..` run, after
   `SKY_L0118`) and bump both count assertions `76` → `77`
   (rename `taxonomy_has_seventy_six_codes` → `..._seventy_seven_codes`,
   `code.rs:420`; `assert_eq!(seen.len(), 77)`, `code.rs:438`).

4. **Create the explain page** `crates/sky_diagnostics/explain/SKY-L0119.md` —
   line 1 exactly
   `# SKY-L0119: storing a \`Task\`, \`Cmd\`, or \`Sub\` in a constructor payload is not supported yet`
   and ≥ 3 ```` ```sky ```` fences:
   ```md
   # SKY-L0119: storing a `Task`, `Cmd`, or `Sub` in a constructor payload is not supported yet

   A data constructor was given a `Task`, `Cmd`, or `Sub` value as one of its
   payload fields. The generated Rust enum derives `Clone`, `Debug`, `PartialEq`
   and a string-rendering impl; the runtime `Task` is a boxed asynchronous
   computation (a future) that satisfies none of those — so accepting it would
   emit Rust that does not compile.

   This is the async-runtime sibling of SKY-L0114 (a *function* value in a
   constructor payload): both are values with no derivable identity.

   ## Task in a payload

   ```sky
   type Job = Job (Task Error Int)     -- not yet: Task is not derivable

   main = println "unreachable"
   ```

   ## Laundered through a type variable

   ```sky
   type Job a = Job a

   -- `Job (Task.succeed 1)` instantiates the payload to `Task Error Int`
   main = println "unreachable"       -- still not yet
   ```

   ## Carry the result, not the Task

   Run the `Task` and store its result — a plain value derives cleanly:

   ```sky
   type Job = Job Int

   main = println (String.fromInt 1)  -- OK
   ```

   Note: the Go reference compiler also fails a `Task`-in-payload constructor —
   its code generation emits Go that `go build` rejects; ipê gives this clean
   diagnostic instead.

   `[feature: ctor-payload-task]`

   This is a current limitation of the ipê Rust compiler, not a problem with
   your code. If it matters to your project, please tell us at
   https://codeberg.org/sky-lang/sky-rust/issues.
   ```

5. **Add the `Feature` variant + `code_of_feature` + `feature_label`.**
   - `diagnostic.rs`, after `RoutedLiveApp` (line 544):
     ```rust
     /// A `Task`/`Cmd`/`Sub` value stored in a CONSTRUCTOR PAYLOAD — declared
     /// (`type Job = Job (Task Error Int)`) or laundered there through a type
     /// variable. The generated Rust enum derives `Clone`/`Debug`/`PartialEq` +
     /// `SkyStringify`; the runtime `SkyTask`/`SkyCmd`/`SkySub` (boxed
     /// futures / handles) satisfy none, so accepting it would emit cargo-failing
     /// Rust. Async-runtime sibling of `CtorPayloadFunction`. [SKY-L0119]
     CtorPayloadTask,
     ```
   - Add `SKY_L0119` to the `use crate::code::{…}` import block (`diagnostic.rs:16`,
     the `SKY_L01..` run).
   - `code_of_feature` (after `RoutedLiveApp => SKY_L0118`):
     ```rust
     Feature::CtorPayloadTask => SKY_L0119,
     ```
   - `render.rs` `feature_label` (after the `RoutedLiveApp` arm at line 602):
     ```rust
     Feature::CtorPayloadTask => {
         "storing a `Task`, `Cmd`, or `Sub` value in a constructor payload is \
          not supported yet [feature: ctor-payload-task]"
     }
     ```

6. **Run diagnostics gates (green for the taxonomy layer).**
   ```
   cargo test -p sky_diagnostics 2>&1 | tail -20
   ```
   Expected: `ok.` (count now 77; explain page conforms).

7. **Add the two `lower_enum` gates (green).**
   In `crates/sky_lower/src/lower.rs`:
   - Add the IR predicate near `ir_contains_fun` (~line 220). It must reject at
     the *head* of the async-opaque family (unlike `ir_contains_fun`, which
     recurses through them):
     ```rust
     /// Does this IR type embed an async-opaque runtime handle — `SkyTask` /
     /// `SkyCmd` / `SkySub` — anywhere? These are boxed futures/handles with no
     /// derivable `Clone`/`Debug`/`PartialEq`/`SkyStringify`, so a constructor
     /// payload carrying one cannot satisfy the enum's derives ([SKY-L0119]).
     fn ir_embeds_async_opaque(ty: &IrType) -> bool {
         match ty {
             // The three async-opaque heads: reject here (do NOT recurse past —
             // the head itself is the non-derivable value).
             IrType::Task(_) | IrType::Cmd(_) | IrType::Sub(_) => true,
             // Scalars / opaque handles / plain types: no async value inside.
             IrType::Int
             | IrType::Float
             | IrType::Bool
             | IrType::Str
             | IrType::Char
             | IrType::Unit
             | IrType::Bytes
             | IrType::Json
             | IrType::Decoder(_)
             | IrType::Db
             | IrType::ServerRequest
             | IrType::ServerResponse
             | IrType::ServerRoute
             | IrType::ServerCookie
             | IrType::Generic(_)
             | IrType::UiPlain(_)
             | IrType::LiveReq
             | IrType::LiveRoute
             | IrType::Fun(_, _) => false,
             // Composites: recurse into carried types.
             IrType::Enum { args, .. } => args.iter().any(ir_embeds_async_opaque),
             IrType::Maybe(e) | IrType::List(e) => ir_embeds_async_opaque(e),
             IrType::Result(err, ok) => {
                 ir_embeds_async_opaque(err) || ir_embeds_async_opaque(ok)
             }
             IrType::Dict(k, v) => ir_embeds_async_opaque(k) || ir_embeds_async_opaque(v),
             IrType::Set(a) => ir_embeds_async_opaque(a),
             IrType::Tuple(elems) => elems.iter().any(ir_embeds_async_opaque),
             IrType::Record(fields) => fields.values().any(ir_embeds_async_opaque),
             IrType::Ui { msg, .. } => ir_embeds_async_opaque(msg),
         }
     }
     ```
     Every `IrType` arm is enumerated (no wildcard), mirroring `ir_contains_fun`
     (`lower.rs:220-258`) so a future `IrType` variant forces an explicit
     decision. **Re-verify the arm set against `IrType` at HEAD before pasting**
     — the list above is copied from `ir_contains_fun`'s arms at HEAD (`691e275`);
     if the enum has drifted, `cargo build` will name the missing/extra arm.
   - Add the canon-level arity pre-check near `collect_type_vars` (~line 188):
     ```rust
     /// The arg count of a `Task`-headed constructor-field type when it is not a
     /// valid `Task` shape (∉ {1, 2}); `None` otherwise. Lets `lower_enum` reject
     /// a mis-arity `Task` in a ctor field with a spanned SKY-T0015 before the
     /// `ir_type_from_canon` CompilerBug arm is reached (canon carries no inner
     /// type spans, so the constructor span is the anchor).
     fn task_arity_in_canon(&self, t: &canon::Type) -> Option<usize> {
         if let canon::Type::Con { name, args, .. } = t
             && self.resolve(*name).ok() == Some("Task")
             && args.len() != 1
             && args.len() != 2
         {
             return Some(args.len());
         }
         None
     }
     ```
     (Confirm `Lowerer::resolve(&self, Symbol) -> DResult<&str>` exists — it is
     used at `lower.rs:1402` `match self.resolve(*name)?`. If `resolve` returns
     `DResult`, use `.ok()` as shown.)
   - In `lower_enum`'s per-field loop, insert BEFORE the existing Gate 1
     (`lower.rs:1069`, the `collect_type_vars` block):
     ```rust
     // Gate 0a: a mis-arity `Task` in this field type would hit the
     // `ir_type_from_canon` CompilerBug arm; reject it as a spanned type error
     // (SKY-T0015) at the constructor instead.
     if let Some(found) = self.task_arity_in_canon(arg) {
         return Err(Diagnostic::Type {
             span: ctor.span,
             msg: sky_diagnostics::TypeError::TaskArity { found },
         });
     }
     ```
     And insert AFTER the `let ir = self.ir_type_from_canon(...)?;` line
     (`lower.rs:1074`), alongside Gate 2:
     ```rust
     // Gate 3: a `Task`/`Cmd`/`Sub`-bearing payload cannot satisfy the enum's
     // derives — reject before emitting non-building Rust (SKY-L0119).
     if ir_embeds_async_opaque(&ir) {
         return Err(unsupported(ctor.span, Feature::CtorPayloadTask));
     }
     ```
     Confirm `Diagnostic`, `TypeError`, `Feature` are imported in `lower.rs`
     (`Feature`/`unsupported` already used; add `TypeError` to the
     `sky_diagnostics::{…}` use if absent — grep the `use` header first).

8. **Run the new gate tests (green).**
   ```
   cargo test -p skyc --test golden_m5a_ctor_task_gate 2>&1 | tail -20
   ```
   Expected: `task_in_ctor_payload_is_sky_l0119` and
   `mis_arity_task_in_ctor_field_is_sky_t0015` both pass.

9. **Guard against regressions in the existing lower gates.**
   ```
   cargo test -p sky_lower 2>&1 | tail -20
   cargo test -p skyc --test golden_m3a_function_payload_gate 2>&1 | tail -10
   ```
   Expected: green — the new `ir_embeds_async_opaque` walker does not disturb the
   function-payload (`SKY-L0114`) path, and no previously-accepted enum is now
   rejected (a `Task`-free enum returns `false` from the predicate).

10. **Full-workspace check + commit.**
    ```
    cargo test -p sky_diagnostics -p sky_lower -p skyc 2>&1 | tail -30
    ```
    Expected: all green. Then:
    ```
    git add -A && git commit -m "lower: fail-closed SKY-L0119 for Task/Cmd/Sub in a constructor payload; SKY-T0015 for mis-arity Task in a ctor field"
    ```

---

## Task 3 — verification sweep + `explain`/CI hygiene

Confirm both fixes hold end-to-end and the taxonomy is internally consistent.

**Files:** none (verification-only).

**Steps**

1. **Whole-diagnostics + touched-crate suite.**
   ```
   cargo test -p sky_diagnostics -p sky_types -p sky_lower -p skyc 2>&1 | tail -40
   ```
   Expected: `test result: ok.` everywhere; the code-count assertion reads `77`.

2. **Clippy on the three touched crates (no new lint).**
   ```
   cargo clippy -p sky_diagnostics -p sky_types -p sky_lower --all-targets 2>&1 | tail -20
   ```
   Expected: no warnings from the new code (explicit walkers, no wildcard arms).

3. **Grep-proof no reachable `CompilerBug` remains on the two source shapes.**
   Confirm the only remaining `Task`-arity `CompilerBug` arms are the two
   genuine unreachable-defence sites and are documented as such:
   ```
   rg -n "Task.*type argument|Task applied to.*argument" crates/sky_lower/src/lower.rs crates/sky_types/src/constrain.rs
   ```
   Expected: `lower.rs:1462` and `lower.rs:1781` remain `CompilerBug` (solver
   only mints unary `Task`; ctor fields are pre-checked in `lower_enum`);
   `constrain.rs` no longer has a `CompilerBug` arity arm.

4. **Confirm the `explain` CLI renders both pages** (the `include_str!` + gate
   already prove existence; this is a human-readable spot check):
   ```
   cargo run -p skyc -- explain SKY-T0015 2>&1 | head -5
   cargo run -p skyc -- explain SKY-L0119 2>&1 | head -5
   ```
   Expected: each prints its `# SKY-…:` title line and body. (Skip if the
   `explain` subcommand is not yet wired in `skyc` — the taxonomy gate is the
   binding check.)

---

## Spec ambiguities resolved (to make the plan mechanical)

1. **No dedicated spec existed** — this is a one-pass design-then-plan, so the
   design decisions below are made here, not inherited.
2. **Diagnostic kind per fix.** Fix 1 → `Diagnostic::Type` (`SKY-T0015`): arity
   is a type-well-formedness fact caught at constrain. Fix 2 → `Diagnostic::Lower`
   (`SKY-L0119`): non-derivable payload is a backend-capability fact caught at
   lower. This matches the existing split (`SKY-T00xx` type errors vs
   `SKY-L01xx` feature gaps) and keeps `CompilerBug` for true invariants only.
3. **Scope of Fix 2's predicate.** The ITEM names "Task-in-ADT-ctor," but
   `Cmd`/`Sub` (`SkyCmd`/`SkySub`) share the *identical* non-derivable-opaque
   property and the same `ir_contains_fun` grouping (`lower.rs:225`). Per
   "make invalid states unrepresentable," the gate covers the whole
   `{Task, Cmd, Sub}` family under one feature (`CtorPayloadTask`, message names
   all three) rather than leaving two adjacent holes open. Db/Decoder/Server-*
   opaque handles are *out of scope* here (they are not the ITEM's target and
   their derivability is a separate audit); note them as a follow-up if a future
   sweep shows they also fail an enum derive.
4. **Second reachable ICE folded in.** A mis-arity `Task` in a *constructor
   field* (`type J a = J (Task Error a Bool)`) does not pass through
   `normalize_annotation_ty` and would hit the `lower.rs:1462` `CompilerBug`.
   Rather than duplicate a Type-error emission deep in `ir_type_from_canon`
   (which has no span), it is caught in `lower_enum` with `ctor.span`, reusing
   `SKY-T0015`. The `lower.rs:1462`/`1781` `CompilerBug` arms stay as
   unreachable-defence — consistent with "parse, don't validate."
5. **Stale comment corrected.** `constrain.rs:1262-1263`'s claim that
   canonicalisation rules out arity-0/3+ `Task` is **false** — verified against
   `resolve.rs:1458-1522`, which validates arity only for *aliases*
   (`NameError::AliasArity`), never for non-alias type constructors like `Task`.
   The plan corrects the comment in Task 1 step 8.
6. **Code numbers.** `SKY-T0015` and `SKY-L0119` are the next free numbers at
   HEAD (`691e275`). If the registry migration merges first with new codes,
   re-allocate at rebase and update the `ALL` slice + the (now `77`) count
   assertions; the diagnostics suite is the tripwire.
