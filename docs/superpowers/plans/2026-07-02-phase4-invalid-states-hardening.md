# Implementation Plan — Phase 4: make-invalid-states-unrepresentable hardening (task #31)

Source audit (already merged): `docs/architecture/principles-audit-2026-07-02.md`
§3.1 (make-invalid-states-unrepresentable). This plan does **not** redesign the
findings; it turns the two Phase-4-scoped findings — **F8** (kernel category
dispatch) and **F13** (`classify_binop` wildcard) — into a mechanical, TDD,
task-by-task sequence. Every anchor below was re-verified against HEAD
(`691e275`, "Thread the resolved kernel id through the parse-once seam
(registry migration Phase B)"); the audit was written before the registry
migration moved several targets, and this plan **corrects the drifted anchors**
(see "Drift corrections" below).

Parity note: where Sky's Haskell compiler (`../sky`) is referenced it is a
capability reference only. ipê's operator identity is a closed `BinOpKind` sum
minted once at desugar; Sky carries the operator as a re-inspected string
(`Sky.Type.Constrain.Expression.binopTypes` matches operator names). That
difference — a typed leaf vs a stringly key — is the whole point of F13 and is
stated as a design divergence, nothing more.

---

## Drift corrections against HEAD (audit anchors were stale)

| Audit says | HEAD reality (re-verified) | Consequence for this plan |
|---|---|---|
| **F6** integer `//` lowers to `BinOp::Div`, panics on `x // 0`; helpers dead. | **Closed.** `BinOp::IntDiv` is a distinct `sky_ir` variant (`ir.rs:1850`); `lower.rs:4107` maps `"idiv" => BinOp::IntDiv`; `emit_expr.rs:2248` emits `sky_runtime::math::sky_int_div(l, r)` (call, not infix). `constrain.rs:531-532` already splits `idiv`/`fdiv`. | **F6 is OUT of scope** — not a Phase-4 item, already fixed. |
| **F8** dispatch keys off `emit_expr.rs` `is_tea()`/`is_server()`/`is_ui()` `matches!`-lists on `KernelFn` in `sky_ir`. | Migrated. `KernelFn` is now `sky_kernels::StdlibKernel`; the `is_*` predicates live in **`crates/sky_kernels/src/lib.rs:1585-1788`**. An exhaustive source of truth **already exists**: `StdlibKernel::decl()` (`lib.rs:550`, exhaustive `match self`, no wildcard) returns `StdlibDecl { class: KernelClass, .. }` (`lib.rs:24-48`). | F8 shrinks to: collapse the **seven hand-maintained `is_*` `matches!` lists** onto the already-exhaustive `decl().class`. New file target: `sky_kernels/src/lib.rs`. |
| **F13** `classify_binop` matches raw bytes, `_ => BinopClass::Poly` @523. | Live, anchor moved: `constrain.rs:526-539` (`classify_binop`, `_ => Poly` @537); caller `constrain_binop` @883 reads `func: Symbol` and does `classify_binop(resolve(func))` @890. Operator identity is minted in `sky_canon::resolve::resolve_op_func` (`resolve.rs:1368-1390`) and carried as `func: Symbol` on `canon::Expr_::Binop` (`sky_canon/src/ast.rs:171-177`). | F13 anchors updated throughout. |

I verified the current F8 state is **in sync, not diverged**: after removing
multi-line-`decl()`-arm regex artifacts, `decl().class` and every `is_*` list
have identical membership at HEAD (Db 37, Tea 12, Server 25, Ui 73, Live 4,
Tui 2, Webview 1). So F8 today is a **latent hazard** (two independent
hand-lists that *can* silently diverge), not a live mis-emit. The hardening
makes the divergence unrepresentable.

---

## Goal

Foreclose two classes of *representable-but-invalid* compiler state that the
type system currently permits — both instances of the same anti-pattern the
audit names (a classification carried as a re-inspected string / duplicated
hand-list with a fail-open wildcard):

1. **F13** — a binary operator whose type discipline is chosen by a `match` on
   raw operator-name bytes with a `_ => Poly` fallback. A future operator kernel
   not enumerated silently receives an obligation-free `a -> a -> a` scheme (no
   `Number` / `Order` / `Equality` bound) — the F1 fail-open shape in miniature,
   at the type stage.
2. **F8** — a kernel's emission subsystem (Pure / Db / Tea / Server / Ui / Live /
   Tui / Webview) decided by seven independently hand-maintained `matches!`
   lists that default `false`. A newly-added or re-classified `StdlibKernel`
   variant omitted from a list returns `false` in every predicate, skips all
   category emitters, and can fall through to the plain-kernel emit path —
   a wrong runtime call with **no error**.

Both fixes replace a wildcard/hand-list with an **exhaustive `match` over a
closed sum**, so that adding a variant without classifying it becomes a
**compile error**, not a silent unsound accept.

Non-blocking: neither fix gates a release; both are pure hardening (behaviour is
byte-identical today, since the hand-lists currently agree with the exhaustive
sources). They are sequenced around two in-flight efforts (#45 registry
migration, #49 TCO) whose file overlap is called out per task.

## Architecture

Enumeration of the representable-but-invalid states in the invalid-states class,
the ADT/typed-boundary change that forecloses each, and the owning task:

| Representable-but-invalid state | Foreclosing change | Owner |
|---|---|---|
| Operator kernel not in `classify_binop`'s enumerated arms ⇒ `Poly` (obligation-free scheme). | Mint a closed `BinOpKind` at desugar; `classify_binop` becomes exhaustive `match BinOpKind` (no `_`). Missing operator ⇒ non-exhaustive-match compile error. | **Task 1 (F13 core)** |
| Two sources of operator identity on the canon node (`func: Symbol` **and** `kind: BinOpKind`) that could disagree. | Drop `func`/`home`; the desugar mints `kind` only; lower maps `kind → ir::BinOp`. Single typed source. | **Task 2 (F13 closure)** |
| `StdlibKernel` variant absent from an `is_*` list ⇒ `false` everywhere ⇒ mis-emit. | Delete the seven `matches!` lists; each `is_*` delegates to the already-exhaustive `decl().class`. Missing variant ⇒ `decl()` non-exhaustive-match compile error (the single choke point). | **Task 3 (F8 core)** |
| Emit dispatch keyed off boolean `if !k.is_tea()` guards + trailing `CompilerBug`, not one exhaustive match over `KernelClass`; a new `KernelClass` needs no emit arm. | `emit_kernel_call` dispatches on `k.category()` via exhaustive `match KernelClass` (no `_`); a new class ⇒ compile error. | **Task 4 (F8 closure)** |
| Resolved-but-unschemed kernel ⇒ `Ty::Var(u32::MAX)` (F1/F3, ≈231 exit-0 holes). | Exhaustive `kernel_ty` over `KernelId`. | **Task #45** — *out of Phase-4 scope; explicitly deferred, see "Boundaries".* |

Data-flow direction (unchanged by this plan): `sky_parse → sky_canon (mint
BinOpKind) → sky_types::constrain (consume kind) / sky_lower (consume kind) →
sky_backend_rust::emit`. `sky_kernels` is a leaf crate (deps: `sky_intern`,
`sky_diagnostics` only — `lib.rs:6-9`); F8 stays inside it plus its emit
consumer.

## Tech Stack

Rust workspace crates: `sky_canon`, `sky_types`, `sky_lower`, `sky_kernels`,
`sky_backend_rust`, `skyc`. Per-crate unit tests via `cargo test -p <crate>`;
lints via `cargo clippy -p <crate> -- -D warnings`. End-to-end build+run for the
F13/F8 behaviour-preservation checks uses the shared harness
`crates/skyc/tests/support/mod.rs` (`build_and_run_emitted`). No new
dependencies. `BinOpKind` is a plain `#[derive(Clone, Copy, PartialEq, Eq,
Debug)]` enum in `sky_canon::ast`; `category()` is a `const fn`.

## Global Constraints

**PRINCIPLES order — apply in this priority when any step forces a trade-off:**
1. **Security** · 2. **Correctness** · 3. **Soundness** · 4. **Efficiency** ·
5. **Completeness** · 6. **Readability** (from `PRINCIPLES.md`).

Neither fix touches an untrusted-input boundary, so the operative principles are
**Correctness / Soundness**: the fixes convert a fail-open type/emit decision
into a fail-closed exhaustive match. Efficiency is neutral (`decl()` /
`category()` are `const fn`; `BinOpKind` is `Copy`).

**Two fundamental rules (this plan is a direct application of both):**
- **PARSE, DON'T VALIDATE** — parse the operator token **once** into a closed
  `BinOpKind` at desugar; never re-inspect the operator as `&str` downstream.
  Parse the kernel identity **once** into `StdlibKernel` (already done by the
  registry migration); never re-inspect it via parallel `matches!` lists.
- **MAKE INVALID STATES UNREPRESENTABLE** — a binary op with no type discipline,
  and a kernel with no emission category, must both be **compile errors** via
  exhaustive `match` over the closed sum, never a `_ =>` fallback or a
  default-`false` predicate.

**Fail-closed diagnostics, not panics/wildcards** — every new gap surfaces as a
`sky_diagnostics::Diagnostic` (`CompilerBug` where a compiler invariant is
violated) or, preferentially, a non-exhaustive-match **compile error**. No
`unwrap`/`panic!`/`_ =>` added anywhere in this plan.

**Parallel-safety matrix (file overlap with in-flight work):**

| Task | Files touched | Overlaps | Sequencing rule |
|---|---|---|---|
| 1 (F13 core) | `sky_canon/src/ast.rs`, `sky_canon/src/resolve.rs`, `sky_types/src/constrain.rs` (@494-539, @883-965) | **#45** also edits `constrain.rs` but in `kernel_ty`/`constrain_var_kernel` (@1422, @1868) — **disjoint regions**, low conflict. **#49 TCO** does *not* touch these files. | Landable **now, in parallel**. Rebase-order tie-break: land before #45's next constrain touch, or coordinate the two `constrain.rs` hunks. |
| 2 (F13 closure) | `sky_canon/src/ast.rs`, `sky_canon/src/resolve.rs`, **`sky_lower/src/lower.rs`** (@2237, @4094) | **#49 TCO** edits `lower.rs` heavily (`lower_def` Typed arm, `analyze_tail_recursion`, `rewrite_tail_calls`). The `binop` fn (@4094) and the `Binop` lowering arm (@2237) are **not** the TCO edit sites, but same file. | **Sequence AFTER #49 TCO lands** (or after Task 1, whichever is later) to avoid a `lower.rs` merge conflict. Optional — Task 1 already closes the audit's F13 (the type-stage wildcard). |
| 3 (F8 core) | **`sky_kernels/src/lib.rs`** (@1585-1788, +`category()` near @1789) | **#45 registry migration** actively edits `sky_kernels/src/lib.rs` (the whole crate is its workspace). **Same file, adjacent `impl StdlibKernel` block.** High conflict risk. | **Coordinate with #45** — land as part of finishing the registry migration, or immediately after a #45 phase settles. Do NOT run in parallel with an open #45 branch touching the same `impl` block. |
| 4 (F8 closure) | **`sky_backend_rust/src/emit_expr.rs`** (@809-1110), `sky_kernels/src/lib.rs` (add `category()` if not already from Task 3) | **#49 TCO** edits `emit_expr.rs` (`emit_func`, `emit_expr_tail`) — different fns than the kernel-category dispatch (`emit_tea_kernel_call` @849, `emit_server` @964, ui @1110), but same file. | **Sequence AFTER #49 TCO** and **after Task 3**. Optional closure — Task 3 already removes the duplicate-hand-list hazard. |

Recommended landing order: **Task 1 → Task 3** (the two audit-scoped fixes;
independent, different crates) **→ Task 2 → Task 4** (closures, after their
overlapping in-flight work settles).

---

## Task 1 — F13 core: closed `BinOpKind`, exhaustive type-stage classification

**Files:**
- `crates/sky_canon/src/ast.rs` (add enum; add field to `Expr_::Binop` @171)
- `crates/sky_canon/src/resolve.rs` (mint `kind` in `combine_binop` @1318; new
  `binop_kind` map beside `resolve_op_func` @1368)
- `crates/sky_types/src/constrain.rs` (`BinopClass` @494, `classify_binop` @526,
  `constrain_binop` @883)

**Interfaces:**

Consumes (existing, verified at HEAD):
- `sky_canon::ast::Expr_::Binop { op: Symbol, home: Symbol, func: Symbol, lhs, rhs }` (`ast.rs:171-177`)
- `resolve::resolve_op_func(op: Symbol, interner: &mut Interner) -> DResult<Symbol>` (`resolve.rs:1368`)
- `constrain::classify_binop(func: &str) -> BinopClass` (`constrain.rs:526`) — `const fn`, `_ => Poly` @537
- `constrain::Builder::constrain_binop(&mut self, local, func: Symbol, lhs, rhs) -> DResult<VarId>` (`constrain.rs:883`), reads `classify_binop(self.interner.resolve(func).unwrap_or(""))` @890

Produces (new / changed):
- `sky_canon::ast::BinOpKind` — `pub enum BinOpKind { Add, Sub, Mul, FloatDiv, IntDiv, Eq, Neq, Lt, Gt, Le, Ge, And, Or, Append }` (14 variants; the closed operator set — `::`/`|>`/`<|` are desugared *before* `combine_binop` and never reach `Binop`, verified `resolve.rs:1329-1352`)
- `sky_canon::ast::binop_kind(op: &str) -> Option<BinOpKind>` — `const fn`, total over the 14 operator spellings, `None` for anything else (fail-closed; a `None` at the mint site is a `Diagnostic`, never a `Poly` default)
- `Expr_::Binop { op, home, func, kind: BinOpKind, lhs, rhs }` — **additive** `kind` field (keep `func` this task so `sky_lower` is untouched; `func` removed in Task 2)
- `constrain::classify_binop(kind: BinOpKind) -> BinopClass` — exhaustive `match kind`, **no `_` arm**
- `BinopClass` loses its `Poly` variant (now unreachable — the 14 kinds map onto `Num`/`IntDiv`/`FloatDiv`/`Order`/`Equality`/`Boolean`/`Append` only)

**Steps:**

1. **Write the failing test** (mint layer). In `crates/sky_canon/src/lib.rs`
   `mod tests` (@100), add:
   ```rust
   #[test]
   fn binop_kind_maps_the_closed_operator_set() {
       use crate::ast::{BinOpKind, binop_kind};
       assert_eq!(binop_kind("//"), Some(BinOpKind::IntDiv));
       assert_eq!(binop_kind("/"),  Some(BinOpKind::FloatDiv));
       assert_eq!(binop_kind("++"), Some(BinOpKind::Append));
       assert_eq!(binop_kind("=="), Some(BinOpKind::Eq));
       assert_eq!(binop_kind("::"), None); // desugared before Binop; not an op-kernel
   }
   ```
2. **Run it — fails to compile (red).**
   `cargo test -p sky_canon binop_kind_maps_the_closed_operator_set`
   Expected: `error[E0432]: unresolved import ` / `error[E0433]: ... BinOpKind` —
   the enum and `binop_kind` do not exist yet.
3. **Minimal impl (canon).**
   - In `ast.rs`, add the `BinOpKind` enum (with a doc comment naming it the
     closed desugar-time operator identity) and the `pub const fn binop_kind(op:
     &str) -> Option<BinOpKind>` mapping the 14 spellings (mirror the arms of
     `resolve_op_func` @1370-1383: `"+"=>Add … "++"=>Append`).
   - Add `kind: BinOpKind` to `Expr_::Binop` @171.
   - In `resolve.rs` `combine_binop` @1353-1362, compute
     `let kind = ast::binop_kind(interner.resolve(op.value).unwrap_or("")) .ok_or_else(|| /* Diagnostic::CompilerBug: operator reached combine_binop without a BinOpKind */)?;`
     and set `kind` in the constructed `Expr_::Binop`. (`::`/`|>`/`<|` return
     early above, so a `None` here is a genuine compiler invariant miss →
     fail-closed `Diagnostic`, not a silent `Poly`.)
4. **Run — passes (green).**
   `cargo test -p sky_canon binop_kind_maps_the_closed_operator_set` → `test result: ok. 1 passed`.
5. **Write the failing test** (type layer). In `crates/sky_types/src/constrain.rs`
   `mod tests` (near @4534), add a test that type-checks `5 // 3 : Int` and
   `5.0 / 2.0 : Float` through the existing constrain harness used by the tests
   at @4534-4617 (follow that module's established `build`/`solve` helper
   pattern), asserting the inferred types are `Int` and `Float` respectively.
   This test compiles only once `constrain_binop` reads `kind` from the canon
   node.
6. **Run — fails (red).** `cargo test -p sky_types` — either a compile error
   (`classify_binop` signature mismatch once step 7 is half-applied) or a
   behavioural assertion; capture the red before step 7.
7. **Minimal impl (constrain).**
   - Change `classify_binop` to `const fn classify_binop(kind: BinOpKind) ->
     BinopClass` with an **exhaustive** `match kind` (14 arms → the 7 non-`Poly`
     `BinopClass` variants; `Add|Sub|Mul` carry their `TyBounds`; `IntDiv`/`FloatDiv`
     as today; `Lt|Gt|Le|Ge => Order`; `Eq|Neq => Equality`; `And|Or => Boolean`;
     `Append => Append`). **Delete the `_ => Poly` arm.**
   - Delete `BinopClass::Poly` (@522) and its arm in `constrain_binop` (@960-964).
   - In `constrain_binop`, replace the `func: Symbol` parameter's use: read the
     `kind` off the `Binop` node at the call site (`constrain.rs:1574`, the
     `Expr_::Binop { .. }` match arm — thread `kind` through instead of `func`).
8. **Run — passes (green).** `cargo test -p sky_types` → all pass, pending count
   unchanged. Then `cargo test -p sky_canon` (regression). Then
   `cargo clippy -p sky_canon -p sky_types -- -D warnings` (expect clean; the
   deleted `Poly` arm removes a now-dead branch).
9. **Behaviour-preservation E2E.** `cargo test -p skyc` — the emitted-code
   harness must be byte-identical (F13 changes only the type stage; lower still
   reads `func`). Expected: no new failures.
10. **Commit.** `git commit -am "F13: closed BinOpKind + exhaustive type-stage
    binop classification (task #31)"`.

---

## Task 2 — F13 closure: single typed operator source (drop `func`/`home`)

**Sequence AFTER Task 1 AND after #49 TCO lands (both touch `lower.rs`).**
Optional — Task 1 already closes the audit's F13. This task removes the residual
*two-sources-of-truth* invalid state (`func: Symbol` and `kind: BinOpKind` on the
same node could disagree).

**Files:**
- `crates/sky_canon/src/ast.rs` (`Expr_::Binop`: remove `func`, `home`)
- `crates/sky_canon/src/resolve.rs` (`combine_binop`: stop minting `func`/`home`; keep `op` for diagnostics; delete `resolve_op_func` if now unused)
- `crates/sky_lower/src/lower.rs` (`Binop` arm @2237; `binop` fn @4094)

**Interfaces:**

Consumes:
- `lower::Ctx::binop(&self, func: Symbol, span: Span) -> DResult<ir::BinOp>` (`lower.rs:4094`), maps `"add"=>Add … "idiv"=>IntDiv` (@4096-4109)
- `Expr::BinOp { op: ir::BinOp, lhs, rhs }` produced at `lower.rs:2237`

Produces:
- `lower::Ctx::binop_of_kind(&self, kind: canon::BinOpKind) -> ir::BinOp` — total `match` (no `DResult` needed; every `BinOpKind` maps to exactly one `ir::BinOp`, so the fallible string path is deleted)
- `Expr_::Binop { op: Symbol, kind: BinOpKind, lhs, rhs }` — `func`/`home` gone

**Steps:**
1. **Failing test.** In `sky_lower` tests, assert `binop_of_kind(BinOpKind::IntDiv)
   == ir::BinOp::IntDiv` and `..::FloatDiv == ir::BinOp::Div` (the fdiv→Div
   mapping, preserving the comment at `lower.rs:4100-4101`). References
   `binop_of_kind` which does not exist → **red** (E0599).
2. **Run — red.** `cargo test -p sky_lower binop_of_kind`.
3. **Impl.** Add `binop_of_kind` (total match over the 14 kinds → `ir::BinOp`);
   change the `Binop` lowering arm @2237 to `op: self.binop_of_kind(*kind)`;
   remove `func`/`home` from `Expr_::Binop`; drop `resolve_op_func` and the
   `func` mint in `combine_binop`; delete `Ctx::binop(func: Symbol)` @4094.
4. **Run — green.** `cargo test -p sky_lower`, then `cargo test -p sky_canon -p
   sky_types -p skyc` (full downstream regression), then `cargo clippy
   --workspace -- -D warnings`.
5. **Commit.** `git commit -am "F13 closure: single BinOpKind source, drop
   stringly func/home from canon Binop (task #31)"`.

---

## Task 3 — F8 core: single-source kernel category, delegated `is_*` predicates

**Coordinate with #45 — same file (`sky_kernels/src/lib.rs`), adjacent `impl
StdlibKernel` block. Land as part of / immediately after a #45 phase, never in
parallel with an open #45 branch touching this `impl`.**

**Files:**
- `crates/sky_kernels/src/lib.rs` (add `category()` after `decl()` @543-1578; rewrite the seven `is_*` fns @1585-1788; add a consistency test in `mod tests` @1814)

**Interfaces:**

Consumes (existing, exhaustive, verified `lib.rs:550-1578`):
- `StdlibKernel::decl(self) -> StdlibDecl` — exhaustive `match self`, no wildcard
- `StdlibDecl.class: KernelClass` (`lib.rs:24-48`: `Pure|Db|Server|Tea|Ui|Live|Tui|Webview|Ffi`)
- `StdlibKernel::ALL: &'static [Self]` (`lib.rs:1134`)
- consumers of the predicates (unchanged): `emit_expr.rs:813/869/964/1110`, `lower.rs` `expr_uses_*_kernel` walkers

Produces:
- `StdlibKernel::category(self) -> KernelClass` — `#[must_use] pub const fn`, body `self.decl().class` (single choke point; already exhaustive by construction)
- Seven `is_*` fns rewritten from `matches!(self, Self::A | Self::B | …)` to `matches!(self.category(), KernelClass::X)` (or `self.category() == KernelClass::X`). The `matches!` **enumeration lists are deleted.**

**Steps:**
1. **Write the failing test.** In `sky_kernels` `mod tests` (@1814), add:
   ```rust
   #[test]
   fn is_predicates_agree_with_category_for_every_variant() {
       use super::{KernelClass, StdlibKernel};
       for &k in StdlibKernel::ALL {
           let c = k.category();               // <-- does not exist yet
           assert_eq!(k.is_db(),      c == KernelClass::Db,      "{k:?}");
           assert_eq!(k.is_tea(),     c == KernelClass::Tea,     "{k:?}");
           assert_eq!(k.is_server(),  c == KernelClass::Server,  "{k:?}");
           assert_eq!(k.is_ui(),      c == KernelClass::Ui,      "{k:?}");
           assert_eq!(k.is_live(),    c == KernelClass::Live,    "{k:?}");
           assert_eq!(k.is_tui(),     c == KernelClass::Tui,     "{k:?}");
           assert_eq!(k.is_webview(), c == KernelClass::Webview, "{k:?}");
       }
   }
   ```
2. **Run — fails to compile (red).**
   `cargo test -p sky_kernels is_predicates_agree_with_category_for_every_variant`
   Expected: `error[E0599]: no method named `category` found for ... StdlibKernel`.
3. **Minimal impl.**
   - Add `#[must_use] pub const fn category(self) -> KernelClass { self.decl().class }`
     immediately after `decl()`'s closing brace (before the `is_*` block @1580).
   - Rewrite each of the seven `is_*` fns to delegate:
     `pub const fn is_db(self) -> bool { matches!(self.category(), KernelClass::Db) }`,
     … through `is_webview`. Delete the seven long `matches!` variant lists
     (@1586-1626, 1633-1647, 1654-1684, 1691-1766, 1772-1775, 1781, 1787).
4. **Run — passes (green).** `cargo test -p sky_kernels` → the new test passes
   *because the delegation makes it tautological, and the pre-existing
   membership (verified identical at HEAD) is preserved.* The existing
   `no_colliding_qualifier_name_pairs` test still passes.
5. **Regression + lint.** `cargo test -p sky_backend_rust -p sky_lower` (the
   predicate consumers must be byte-identical), then
   `cargo clippy -p sky_kernels -- -D warnings` (expect clean; the delegated
   bodies are shorter and the `#![allow(clippy::module_name_repetitions)]` at
   `lib.rs:19` already covers the `KernelClass` naming).
6. **Commit.** `git commit -am "F8: single-source kernel category via decl().class;
   delegate is_* predicates (task #31)"`.

**Why this is the make-invalid-states fix:** after this, adding an
`StdlibKernel` variant forces a `decl()` arm (non-exhaustive-match compile
error) — the *single* choke point — and its `class` flows to every `is_*`
automatically. The "variant classified in `decl()` but forgotten in an `is_*`
list" state is now unrepresentable (the lists no longer exist).

---

## Task 4 — F8 closure: exhaustive `KernelClass` emit dispatch

**Sequence AFTER Task 3 AND after #49 TCO lands (both touch `emit_expr.rs`).**
Optional closure — Task 3 already removes the duplicate-hand-list hazard. This
task removes the residual invalid state: emit dispatch is boolean-guarded
(`if !k.is_tea()`) rather than one exhaustive `match KernelClass`, so a new
`KernelClass` variant needs no emit arm.

**Files:**
- `crates/sky_backend_rust/src/emit_expr.rs` (kernel-call dispatch @809-1110)

**Interfaces:**

Consumes:
- `StdlibKernel::category(self) -> KernelClass` (from Task 3)
- existing category emitters: `emit_tea_kernel_call` (@849), the server emitter (@964), the ui/live/tui/webview emitter (@1110), the db arm (@813), plain-kernel path

Produces:
- one `fn emit_kernel_call(k, …) -> DResult<String>` dispatching `match k.category() { Pure => …, Db => …, Server => …, Tea => …, Ui|Live|Tui|Webview => …, Ffi => Err(Diagnostic::unsupported(...)) }` — **no `_` arm**. Each arm delegates to the existing emitter. The `if !k.is_*()` early-return guards inside each emitter become `debug_assert!`-level invariants (the dispatcher now guarantees the class).

**Steps:**
1. **Failing test.** In `sky_backend_rust` tests, assert that `emit_kernel_call`
   routes one representative kernel of each `KernelClass` to the right emitter
   (e.g. `StringLength → Pure`, `DbExec → Db`, `ServerListen → Server`,
   `CmdPerform → Tea`, `UiLayout → Ui`), and that a `KernelClass::Ffi`-classed
   kernel returns `Err(Diagnostic::…)` (fail-closed, not panic). References
   `emit_kernel_call` → **red** (E0425/E0599).
2. **Run — red.** `cargo test -p sky_backend_rust emit_kernel_call`.
3. **Impl.** Introduce `emit_kernel_call` with the exhaustive
   `match k.category()`; route each arm to the existing emitter; convert the
   trailing `_ => CompilerBug` fall-throughs into the exhaustive arms. Keep every
   emitter's internal per-variant `CompilerBug` for an unwired-but-classed
   variant (that remains a fail-closed gap, distinct from a mis-classed variant).
4. **Run — green.** `cargo test -p sky_backend_rust`, then `cargo test -p skyc`
   (E2E emitted-code byte-identity), then `cargo clippy -p sky_backend_rust --
   -D warnings`.
5. **Commit.** `git commit -am "F8 closure: exhaustive KernelClass emit dispatch
   (task #31)"`.

---

## Boundaries — explicitly out of Phase-4 scope

- **F1 / F3** (`constrain.rs:3984` `kernel_ty` fail-open `_ => Ty::Var(u32::MAX)`;
  three-table asymmetry) — the ≈231 exit-0-then-cargo-fail root. **Owned by
  task #45** (the in-flight registry migration this plan is careful not to
  collide with). Not touched here. Task 3's `category()` and Task 1's
  `BinOpKind` are complementary: they harden the *emit* and *operator* stages;
  #45 hardens the *scheme* stage.
- **F6** integer `//` — **already closed at HEAD** (see Drift corrections).
- **F16** (`onKeyPress` dead scheme), **F2/F14** (stringly kernel guards) — fold
  into #45's shared `KernelFn`/`KernelId` resolution; not re-solved here.
- Task 4's exhaustive-emit and Task 2's `func`/`home` removal are **closures**,
  not required to satisfy the audit's F8/F13 line items (Task 1 + Task 3 do
  that). They are listed so the make-invalid-states surface is *fully*
  foreclosed once #45 and #49 settle.

## Verification gate (whole batch)

Before declaring the batch done: `cargo test --workspace` green (pending count
unchanged), `cargo clippy --workspace -- -D warnings` clean, and `cargo test -p
skyc` (E2E) byte-identical emitted output — since every task in this plan is
behaviour-preserving hardening, any emitted-code diff or new test failure is a
defect in the refactor, not an intended change.
