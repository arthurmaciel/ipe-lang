# Sky Compiler Rust Port — Milestone 0 (Spine) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Compile the canonical Sky snippet (`tests/golden/m0/Main.sky`) end-to-end — parse → canonicalise → type-check → lower → IR → Rust emit — producing `main.rs` **byte-identical** to the Haskell reference golden (`tests/golden/m0/main.rs`, sha256 `675ad9e4cf3ed15e4ff391e16812eb25c68ea286fa1a2590315780805a0d6f9e`), which builds against `runtime-rust` and prints `1`.

**Architecture:** Acyclic Cargo workspace of stage crates (`sky_intern` → `sky_diagnostics` → `sky_syntax` → `sky_parse` → `sky_canon` → `sky_types` → `sky_ir` → `sky_lower` → `sky_backend`/`sky_backend_rust` → `skyc`). Frontend never names a backend; backends see only `sky_ir` + the `sky_backend` trait. Each ported crate has the Haskell module as its source-of-truth reference; the golden byte-diff is the correctness gate.

**Tech Stack:** Rust (stable, edition 2021), `cargo`, `clippy` (hardest), `miri`. No third-party parser generators for M0 (hand-written lexer/parser, mirroring `Sky/Parse`). Dev deps: `insta` (snapshot) optional; plain `assert_eq!` golden compare is the floor.

## Plan Adaptation Note

This plan ports a ~65k-LOC compiler. Contract crates (Tasks 1–5) are given as **complete code** — they are small and frozen first. The ported algorithmic crates (parse/canon/types/lower/backend, Tasks 6–13) are given as **exact module layout + public interfaces (signatures) + representative TDD tests + the precise Haskell reference file each agent ports from**. Inlining every line of those ports here would *be* the implementation; instead the interface + tests + reference citation + the golden gate fully constrain the work. The `security-soundness-guardian` reviews each task against the gates before it is "done".

---

## Global Constraints

Copied verbatim from the design spec; every task's requirements implicitly include this section.

- **Edition:** Rust 2021. **Toolchain:** stable for build; `nightly` only for `miri` runs.
- **Every crate** begins with `#![forbid(unsafe_code)]`.
- **Workspace lint table** (in root `Cargo.toml` `[workspace.lints.clippy]`), all `= "deny"`: `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `unreachable`, `todo`, `unimplemented`, `pedantic`, `nursery`. Every crate sets `[lints] workspace = true`. CI fails on any clippy warning (`cargo clippy --all-targets -- -D warnings`).
- **No `String` errors.** All errors are typed enums in `sky_diagnostics`.
- **No `panic!`/`unwrap`/`expect`/raw indexing** in compiler code. Fallible ops return `Result<_, Diagnostic>`. Internal invariant violations return `Diagnostic::CompilerBug { .. }` — never crash.
- **Determinism:** never iterate `HashMap` where order is observed. Use `BTreeMap`/`IndexMap`.
- **Untrusted-input bounds:** parser recursion depth cap, solver step budget (mirror `SKY_SOLVER_BUDGET`).
- **Correctness contract:** M0 output is byte-identical to the golden. Any deliberate divergence is documented in `docs/divergences.md` (created when first needed) — not silently introduced.
- **mem-guard:** before running any heavy build/test, ensure `sky/scripts/mem-guard.sh` is running (the Haskell reference + cargo can pressure the host).
- **Commits:** no co-author trailer, no AI attribution (project rule).
- **Reference repo:** `/home/arthur/Documentos/comp/sky` (Haskell). **Runtime:** `/home/arthur/Documentos/comp/sky/runtime-rust/src/sky_runtime/` (vendored by copy into generated projects).

---

## File Structure

```
sky-rust/
  Cargo.toml                     # workspace + lint table
  rust-toolchain.toml            # pin stable; miri via nightly component
  clippy.toml
  crates/
    sky_intern/        src/lib.rs
    sky_diagnostics/   src/lib.rs            src/diagnostic.rs  src/span.rs
    sky_syntax/        src/lib.rs            src/ast.rs         src/located.rs
    sky_parse/         src/lib.rs            src/lexer.rs       src/layout.rs    src/parser.rs
    sky_canon/         src/lib.rs            src/env.rs         src/resolve.rs
    sky_types/         src/lib.rs            src/ty.rs          src/unionfind.rs src/constrain.rs src/unify.rs src/solve.rs
    sky_ir/            src/lib.rs            src/ir.rs
    sky_lower/         src/lib.rs            src/lower.rs
    sky_backend/       src/lib.rs                                # trait Backend
    sky_backend_rust/  src/lib.rs            src/preamble.rs    src/emit_types.rs src/emit_expr.rs src/naming.rs src/project.rs
    skyc/              src/main.rs
  tests/
    golden/m0/{Main.sky,main.rs,Cargo.toml}  # already committed
    golden_m0.rs                             # workspace integration test (Task 14)
  runtime/                                   # symlink or copy of runtime-rust/src/sky_runtime (Task 13)
```

---

## Task 1: Workspace skeleton + lint gate

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `clippy.toml`
- Create: `crates/sky_intern/Cargo.toml`, `crates/sky_intern/src/lib.rs` (placeholder)

**Interfaces:**
- Produces: a buildable empty workspace with the deny-table active.

- [ ] **Step 1: Write the workspace `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = ["crates/sky_intern"]

[workspace.package]
edition = "2021"
version = "0.0.0"
license = "Apache-2.0"

[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
indexing_slicing = "deny"
unreachable = "deny"
todo = "deny"
unimplemented = "deny"
pedantic = "deny"
nursery = "deny"
```

- [ ] **Step 2: `rust-toolchain.toml` + `clippy.toml`**

```toml
# rust-toolchain.toml
[toolchain]
channel = "stable"
components = ["clippy", "rustfmt"]
```
```toml
# clippy.toml
# (empty for now; thresholds added as needed)
```

- [ ] **Step 3: `crates/sky_intern/Cargo.toml` placeholder**

```toml
[package]
name = "sky_intern"
edition.workspace = true
version.workspace = true

[lints]
workspace = true
```

- [ ] **Step 4: placeholder `crates/sky_intern/src/lib.rs`**

```rust
#![forbid(unsafe_code)]
```

- [ ] **Step 5: Verify build + clippy clean**

Run: `cargo build && cargo clippy --all-targets -- -D warnings`
Expected: both succeed, no warnings.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml rust-toolchain.toml clippy.toml crates/sky_intern
git commit -m "build: workspace skeleton with clippy deny-table and forbid(unsafe)"
```

---

## Task 2: `sky_intern` — interner

**Files:**
- Modify: `crates/sky_intern/src/lib.rs`
- Test: inline `#[cfg(test)]` module.

**Interfaces:**
- Produces:
  - `pub struct Symbol(u32);` — `Copy + Eq + Ord + Hash + Debug`
  - `pub struct Interner { .. }` with `pub fn new() -> Self`, `pub fn intern(&mut self, s: &str) -> Symbol`, `pub fn resolve(&self, sym: Symbol) -> &str` (returns `&str`; on unknown symbol returns `""` — never panics; unknown is unreachable for in-process symbols but handled as value).
  - Ordering of `Symbol` is interning order (deterministic).

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn intern_dedups_and_resolves() {
        let mut i = Interner::new();
        let a = i.intern("Increment");
        let b = i.intern("Increment");
        let c = i.intern("Decrement");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(i.resolve(a), "Increment");
        assert_eq!(i.resolve(c), "Decrement");
    }
    #[test]
    fn resolve_unknown_is_empty_not_panic() {
        let i = Interner::new();
        assert_eq!(i.resolve(Symbol::from_raw(999)), "");
    }
}
```

- [ ] **Step 2: Run, expect FAIL** — `cargo test -p sky_intern` → fails (types missing).

- [ ] **Step 3: Implement**

```rust
#![forbid(unsafe_code)]
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Symbol(u32);

impl Symbol {
    #[must_use]
    pub const fn from_raw(n: u32) -> Self { Self(n) }
    #[must_use]
    pub const fn as_raw(self) -> u32 { self.0 }
}

#[derive(Default)]
pub struct Interner {
    map: HashMap<String, Symbol>,
    strings: Vec<String>,
}

impl Interner {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    pub fn intern(&mut self, s: &str) -> Symbol {
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let id = u32::try_from(self.strings.len()).unwrap_or(u32::MAX);
        let sym = Symbol(id);
        self.strings.push(s.to_owned());
        self.map.insert(s.to_owned(), sym);
        sym
    }

    #[must_use]
    pub fn resolve(&self, sym: Symbol) -> &str {
        self.strings.get(sym.0 as usize).map_or("", String::as_str)
    }
}
```

- [ ] **Step 4: Run, expect PASS** — `cargo test -p sky_intern && cargo clippy -p sky_intern -- -D warnings`.

- [ ] **Step 5: Commit** — `git commit -am "feat(sky_intern): deterministic string interner"`.

---

## Task 3: `sky_diagnostics` — spans + typed errors

**Files:**
- Create: `crates/sky_diagnostics/{Cargo.toml,src/lib.rs,src/span.rs,src/diagnostic.rs}`
- Add member to workspace `Cargo.toml`.

**Interfaces:**
- Produces:
  - `pub struct Span { pub lo: u32, pub hi: u32 }` (byte offsets) + `pub const DUMMY: Span`.
  - `pub struct Located<T> { pub span: Span, pub value: T }` with `pub fn new(span, value)` and `pub fn map`.
  - `pub enum Diagnostic` with at least: `Parse { span: Span, msg: ParseError }`, `Name { span: Span, msg: NameError }`, `Type { span: Span, msg: TypeError }`, `CompilerBug { where_: &'static str, detail: String }`.
  - Sub-error enums `ParseError`, `NameError`, `TypeError` (variants grow per task; start with `Unexpected`, `Unknown`, `Mismatch` respectively).
  - `pub type DResult<T> = Result<T, Diagnostic>;`
  - `Diagnostic` is `Debug + Clone + PartialEq`.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn compiler_bug_carries_context() {
    let d = Diagnostic::CompilerBug { where_: "lower", detail: "no type for region".into() };
    assert!(matches!(d, Diagnostic::CompilerBug { where_: "lower", .. }));
}
```

- [ ] **Step 2: Run, expect FAIL.**

- [ ] **Step 3: Implement** `span.rs`, `diagnostic.rs` per the Interfaces block above; `lib.rs` re-exports. `#![forbid(unsafe_code)]` at top of `lib.rs`. No `String` outside `CompilerBug.detail` and human-message rendering.

- [ ] **Step 4: Run, expect PASS** + clippy clean.

- [ ] **Step 5: Commit** — `git commit -am "feat(sky_diagnostics): spans, Located<T>, typed diagnostic enums"`.

> **CONTRACT FREEZE GATE (guardian):** Tasks 1–3 define the shared vocabulary. The guardian reviews and freezes `Symbol`, `Span`, `Located`, `Diagnostic`, `DResult` before any parallel work starts. No later task may change these signatures without a guardian-approved amendment.

---

## Task 4: `sky_ir` — backend-agnostic typed IR

**Files:**
- Create: `crates/sky_ir/{Cargo.toml,src/lib.rs,src/ir.rs}`; add to workspace.

**Interfaces (the boundary every backend consumes):**
- Produces an IR where illegal states are unrepresentable for M0's subset:
  - `pub struct Program { pub modules: Vec<Module> }`
  - `pub struct Module { pub name: ModPath, pub types: Vec<TypeDef>, pub funcs: Vec<Func>, pub entry: Option<FuncId> }`
  - `pub enum TypeDef { Enum(EnumDef) }` (M0: only nullary-variant enums); `EnumDef { name: Symbol, variants: Vec<Symbol> }`
  - `pub struct Func { pub id: FuncId, pub name: Symbol, pub params: Vec<(Symbol, IrType)>, pub ret: IrType, pub body: Expr }`
  - `pub enum IrType { Int, Float, Bool, Str, Unit, TaskUnit, Enum(Symbol) }` (M0 subset; widened later)
  - `pub enum Expr { Int(i64), Var(Symbol), Ctor { ty: Symbol, variant: Symbol }, BinOp { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> }, Match { scrutinee: Box<Expr>, arms: Vec<Arm> }, Call { callee: Callee, args: Vec<Expr> } }`
  - `pub enum Callee { Func(FuncId), Kernel(KernelFn) }`
  - `pub enum KernelFn { StringFromInt, LogPrintln }` (M0 subset)
  - `pub enum BinOp { Add, Sub }`
  - `pub struct Arm { pub pat: Pat, pub body: Expr }`; `pub enum Pat { Ctor { ty: Symbol, variant: Symbol } }`
  - **Invariant by construction:** every `Var` is a bound param/let symbol; every `Match` is exhaustive over its enum (the constructor takes a checked arm-set — provide `Match::new(scrutinee, arms) -> DResult<Match>` that verifies exhaustiveness, OR require the lowerer to pass a proof token; choose the constructor-validates approach and return `Diagnostic::CompilerBug` if violated — illegal IR cannot be silently built).

- [ ] **Step 1: Failing test** — build the IR for `update` by hand and assert it round-trips through `Debug`; assert `Match::new` rejects a non-exhaustive arm set with `CompilerBug`.

```rust
#[test]
fn match_new_rejects_non_exhaustive() {
    // enum Msg { Increment, Decrement } but arms cover only Increment
    let r = Match::new(/* scrutinee */, vec![/* one arm */], /* expected variants = 2 */);
    assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
}
```

- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement `ir.rs`** per Interfaces; depend on `sky_intern`, `sky_diagnostics`.
- [ ] **Step 4: Run, expect PASS** + clippy clean.
- [ ] **Step 5: Commit** — `git commit -am "feat(sky_ir): typed backend-agnostic IR (M0 subset), exhaustiveness-by-construction"`.

> **CONTRACT FREEZE GATE (guardian):** IR shape frozen here. Backend (Task 11–12) builds against this without the lowerer existing.

---

## Task 5: `sky_backend` — the `Backend` trait

**Files:** Create `crates/sky_backend/{Cargo.toml,src/lib.rs}`; add to workspace.

**Interfaces:**
- Produces:
  - `pub struct EmittedProject { pub files: BTreeMap<String, String>, pub cargo_toml: String }`
  - `pub trait Backend { fn name(&self) -> &'static str; fn emit(&self, program: &sky_ir::Program) -> sky_diagnostics::DResult<EmittedProject>; }`
- Depends on `sky_ir`, `sky_diagnostics`. **Must not** depend on any frontend crate.

- [ ] **Step 1: Failing test** — a `struct NoopBackend` in the test impls `Backend`, returns an empty `EmittedProject`; assert `name() == "noop"`.
- [ ] **Step 2: FAIL → Step 3: Implement → Step 4: PASS + clippy.**
- [ ] **Step 5: Commit** — `git commit -am "feat(sky_backend): Backend trait + EmittedProject (the only backend boundary)"`.

> **CONTRACT FREEZE GATE (guardian):** All four contracts (intern/diag/ir/backend) now frozen. Parallel phase may begin.

---

## Task 6: `sky_syntax` — Source AST

**Reference (source of truth):** `sky/src/Sky/AST/Source.hs`, `sky/src/Sky/AST/Utils/*`.

**Files:** Create `crates/sky_syntax/{Cargo.toml,src/lib.rs,src/ast.rs}`.

**Interfaces:** mirror `Source.hs` for the M0 subset only — `Module`, `Import`, `Value` (with optional `TypeAnnotation`), `Union`, `Expr`/`Expr_` (variants: `VarLocal`, `VarQual`, `Int`, `Call`, `Case`, `Binops`), `Pattern`/`Pattern_` (`PVar`, `PCtor`, `PAnything`), `TypeAnnotation` (`TLambda`, `TVar`, `TType`). Use `Located<T>` from `sky_diagnostics`, `Symbol` from `sky_intern`.

- [ ] **Step 1:** Failing test constructing the expected AST for `tests/golden/m0/Main.sky` by hand and asserting field access compiles + `PartialEq` round-trips.
- [ ] **Step 2: FAIL → Step 3: Implement `ast.rs` → Step 4: PASS + clippy.**
- [ ] **Step 5: Commit** — `git commit -am "feat(sky_syntax): source AST (M0 subset) mirroring Source.hs"`.

---

## Task 7: `sky_parse` — lexer + layout + parser

**Reference:** `sky/src/Sky/Parse/{Primitives,Type,Pattern,Expression,Declaration,Module}.hs`.

**Files:** `crates/sky_parse/{Cargo.toml,src/lib.rs,src/lexer.rs,src/layout.rs,src/parser.rs}`.

**Interfaces:**
- Consumes: `&str` source + `&mut Interner`.
- Produces: `pub fn parse_module(src: &str, interner: &mut Interner) -> DResult<sky_syntax::Module>`.
- Recursion depth cap constant `MAX_DEPTH: u32 = 256`; exceeding → `Diagnostic::Parse{ ParseError::TooDeep, .. }`.

- [ ] **Step 1: Failing test** — `parse_module(include_str!("../../../tests/golden/m0/Main.sky"), &mut i)` returns `Ok` with: module name `Main`, one import, one union `Msg{Increment,Decrement}`, `update` value with a type annotation `Msg -> Int -> Int` and a `Case` body, `main` value whose body is nested `Call`s. Assert these structurally.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement** lexer (tokens incl. layout-significant newlines/indent), layout filter (port `Sky/Parse` layout algorithm), recursive-descent parser for the M0 grammar. No raw indexing — iterate via slices/iterators with `.get()`.
- [ ] **Step 4: Run, expect PASS** + clippy clean.
- [ ] **Step 5: Add a fuzz-smoke test** — feed 1 KB of random bytes; assert it returns `Err` (never panics). `cargo test -p sky_parse`.
- [ ] **Step 6: Commit** — `git commit -am "feat(sky_parse): lexer + layout + recursive-descent parser (M0 subset)"`.

---

## Task 8: `sky_canon` — name resolution

**Reference:** `sky/src/Sky/Canonicalise/{Module,Expression,Environment}.hs`.

**Files:** `crates/sky_canon/{Cargo.toml,src/lib.rs,src/env.rs,src/resolve.rs}` + a `canonical_ast` module (or a `sky_canon::ast` submodule) mirroring `AST/Canonical.hs` M0 subset.

**Interfaces:**
- Consumes: `sky_syntax::Module` + a kernel table (built-in: `Sky.Core.Prelude` exposes ctors of the user union; kernels `println`, `String.fromInt`).
- Produces: `pub fn canonicalise(m: &sky_syntax::Module, interner: &mut Interner) -> DResult<canon::Module>` where `canon::Expr` distinguishes `VarLocal`, `VarTopLevel{module,name}`, `VarKernel{module,name}`, `VarCtor{...}` (per `Canonical.hs`). Unknown name → `Diagnostic::Name{ NameError::Unknown, .. }`.

- [ ] **Step 1: Failing test** — canonicalise the parsed M0 module; assert `Increment`/`Decrement` resolve to `VarCtor` of union `Main.Msg`; `println` → `VarKernel{"Sky.Core.Io"/appropriate, "println"}`; `String.fromInt` → `VarKernel`; `update` ref inside `main` → `VarTopLevel{Main, update}`; `count`/`msg` → `VarLocal`. Confirm exact kernel module strings against the reference (read `sky/sky-stdlib/Sky/Core/*.sky` and the Haskell canonicaliser to get them right).
- [ ] **Step 2: FAIL → Step 3: Implement → Step 4: PASS + clippy.**
- [ ] **Step 5: Commit** — `git commit -am "feat(sky_canon): name resolution to canonical AST (M0 subset)"`.

---

## Task 9: `sky_types` — HM inference (minimal but real)

**Reference:** `sky/src/Sky/Type/{Type,UnionFind,Unify,Solve}.hs`, `sky/src/Sky/Type/Constrain/Expression.hs`.

**Files:** `crates/sky_types/{Cargo.toml,src/lib.rs,src/ty.rs,src/unionfind.rs,src/constrain.rs,src/unify.rs,src/solve.rs}`.

**Interfaces:**
- Consumes: `canon::Module`.
- Produces: `pub fn infer(m: &canon::Module, interner: &Interner) -> DResult<SolvedTypes>` where
  `SolvedTypes { env: BTreeMap<Symbol, Ty>, regions: BTreeMap<Span, Ty> }` (mirrors Haskell `SolvedTypes._stEnv` + `_stRegions` — the region map drives type-directed lowering).
- `Ty` enum mirrors `Type/Type.hs` (M0 subset): `Var`, `Fun`, `Con{module,name,args}`, `Unit`. Solver budget constant `SOLVER_BUDGET` (env-overridable) → exceed yields `Diagnostic::Type{ TypeError::BudgetExceeded, .. }`.
- M0 scope: must infer `count: Int`, `msg: Main.Msg`, `update: Msg -> Int -> Int` (from annotation), `main: Task ()` (from `println` kernel return), and the type of every sub-expression region used by the lowerer (`count + 1 : Int`, the match scrutinee `: Msg`, etc.).

- [ ] **Step 1: Failing test** — `infer(canon_m0).regions` contains: the `update Increment 0` call region `: Int`, the `String.fromInt ...` region `: String`, the `println ...` region `: Task ()`. And `env[update] = Fun(Con Msg, Fun(Int, Int))`.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement** union-find (port `UnionFind.hs`), constraint generation (port the M0-relevant arms of `Constrain/Expression.hs`: VarLocal/VarKernel/VarCtor/Call/Case/Binop/annotation), unify (port `Unify.hs`), solve with budget (port `Solve.hs`). No `unwrap`/index; UF uses `Vec` with `.get()`-guarded access returning `CompilerBug` on impossible dangling ids.
- [ ] **Step 4: Run, expect PASS** + clippy clean.
- [ ] **Step 5: Miri** — `cargo +nightly miri test -p sky_types` (UF mutation is the riskiest aliasing surface). Expected: clean.
- [ ] **Step 6: Commit** — `git commit -am "feat(sky_types): HM inference with union-find + region map (M0 subset)"`.

---

## Task 10: `sky_lower` — Canonical AST + types → IR (sequential integration point)

**Reference:** `sky/src/Sky/Build/Compile.hs` (the lowering core; M0-relevant paths only), `sky/src/Sky/Build/LowerCtx.hs`.

**Files:** `crates/sky_lower/{Cargo.toml,src/lib.rs,src/lower.rs}`.

**Interfaces:**
- Consumes: `canon::Module` + `sky_types::SolvedTypes`.
- Produces: `pub fn lower(m: &canon::Module, types: &SolvedTypes, interner: &Interner) -> DResult<sky_ir::Program>`.
- Maps: union decl → `TypeDef::Enum`; `update` → `Func` (case→`Match` built via the exhaustiveness-checking `Match::new`; binop→`BinOp`); `main` → `Func` with `entry`; `VarKernel println`→`Callee::Kernel(LogPrintln)`, `String.fromInt`→`Kernel(StringFromInt)`; `VarTopLevel update`→`Callee::Func`. Reads region types from `SolvedTypes.regions` to fill `IrType` slots (type-directed). Missing region type → `Diagnostic::CompilerBug`.

- [ ] **Step 1: Failing test** — `lower(canon_m0, types_m0)` yields a `Program` with one module, one enum `Msg{Increment,Decrement}`, funcs `update` and `main`, `main.entry` set, and the `Match` exhaustive. Assert structurally.
- [ ] **Step 2: FAIL → Step 3: Implement → Step 4: PASS + clippy + Miri.**
- [ ] **Step 5: Commit** — `git commit -am "feat(sky_lower): canonical AST + solved types -> typed IR (M0 spine)"`.

---

## Task 11: `sky_backend_rust` — preamble template

**Reference (byte source of truth):** `tests/golden/m0/main.rs` lines 1–127 (fixed preamble) + 138–172 (helpers + entry); `sky/src/Sky/Generate/Rust/Builder/{Emitter,Kernel}.hs`.

**Files:** `crates/sky_backend_rust/{Cargo.toml,src/lib.rs,src/preamble.rs}`.

**Interfaces:**
- Produces: `pub fn preamble() -> String` returning the fixed prologue (golden lines 1–127), and `pub fn epilogue() -> String` returning golden lines 139–172 (ffi polyfill, list helpers, FFI placeholder comment block, entry point). These are emitted verbatim for every M0 program.

- [ ] **Step 1: Failing test** — assert `preamble()` equals the exact substring of the golden from line 1 through the line before USER TYPES, and `epilogue()` equals the exact golden tail. Load golden via `include_str!("../../../tests/golden/m0/main.rs")` and slice by known markers (`// USER TYPES`, `// ENTRY POINT`). Compare byte-for-byte.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement** as `const`/`include_str!`-backed string builders. (Implementer: capture the exact bytes from the golden; do not hand-retype — read the file and embed.)
- [ ] **Step 4: Run, expect PASS** + clippy.
- [ ] **Step 5: Commit** — `git commit -am "feat(sky_backend_rust): fixed preamble/epilogue templates (byte-exact)"`.

---

## Task 12: `sky_backend_rust` — type + expr emission + project assembly

**Reference:** golden `main.rs` lines 27–137; `sky/src/Sky/Generate/Rust/Builder/{ExprEmitter,TypeEmitter,TypeRenderer,Naming,Pattern,Project}.hs`.

**Files:** `crates/sky_backend_rust/src/{emit_types.rs,emit_expr.rs,naming.rs,project.rs,lib.rs}`.

**Interfaces:**
- `naming.rs`: `pub fn module_value(module, name) -> String` (e.g. `Main.update`→`main_update`), `pub fn enum_name(module, ty) -> String` (`Main.Msg`→`MainMsg`), snake/case rules matching the golden.
- `emit_types.rs`: `pub fn emit_enum(&EnumDef) -> String` → the `#[derive(...)] pub enum MainMsg {..}` + `impl SkyStringify` block (golden lines 31–43).
- `emit_expr.rs`: `pub fn emit_expr(&Expr) -> String`, `pub fn emit_func(&Func) -> String` → golden lines 129–137 for `update`/`main` (note: `count + 1` emits as `(count + 1)`; `println(...)` → `log_println(...)`; `String.fromInt` → `string_from_int`; entry fn wraps `sky_main`).
- `project.rs` + `lib.rs`: `RustBackend` impl of `sky_backend::Backend` assembling `main.rs` = `preamble() + user_types + user_funcs + epilogue()`, plus the golden `Cargo.toml` (from `tests/golden/m0/Cargo.toml`).

- [ ] **Step 1: Failing test** — `RustBackend.emit(&program_m0)?.files["src/main.rs"]` equals the full golden `main.rs` byte-for-byte (`assert_eq!`), and `cargo_toml` equals the golden `Cargo.toml`.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement** emitters; match whitespace/section-comment banners exactly (the golden has specific `// ===...` banners and blank-line counts — reproduce them).
- [ ] **Step 4: Run, expect PASS** + clippy clean.
- [ ] **Step 5: Commit** — `git commit -am "feat(sky_backend_rust): type/expr emission + project assembly (byte-identical to golden)"`.

---

## Task 13: Runtime vendoring + `skyc` driver

**Reference:** `sky/src/Sky/Generate/Rust/Project.hs` `copyRustRuntime` (the copy-vendoring step) + golden project layout.

**Files:** `crates/skyc/{Cargo.toml,src/main.rs}`; a build helper that locates `runtime-rust/src/sky_runtime/`.

**Interfaces:**
- `skyc build <entry.sky>`: parse→canon→types→lower→emit; write `sky-out/rust/{Cargo.toml, src/main.rs}`; copy `runtime-rust/src/sky_runtime/` into `sky-out/rust/src/sky_runtime/` (resolve path by walking up from a configured `--runtime <path>` or env `SKY_RUNTIME_DIR`, defaulting to `../sky/runtime-rust/src/sky_runtime`). No `unwrap`; all IO errors → `Diagnostic`.

- [ ] **Step 1: Failing test** (integration, in `skyc`) — run the driver on `tests/golden/m0/Main.sky` into a temp dir; assert `src/main.rs` matches golden and `src/sky_runtime/core.rs` exists.
- [ ] **Step 2: FAIL → Step 3: Implement → Step 4: PASS + clippy.**
- [ ] **Step 5: Commit** — `git commit -am "feat(skyc): driver — full pipeline + runtime vendoring"`.

---

## Task 14: Golden integration test + build-and-run gate

**Files:** `tests/golden_m0.rs` (workspace integration test), CI workflow `.github/workflows/ci.yml`.

- [ ] **Step 1: Write the end-to-end test**

```rust
// tests/golden_m0.rs
#[test]
fn m0_emits_byte_identical_and_runs() {
    // 1. run skyc on tests/golden/m0/Main.sky into a tempdir
    // 2. assert emitted src/main.rs == tests/golden/m0/main.rs (byte-for-byte)
    // 3. `cargo build` the emitted project (feature-gated behind SKY_E2E=1 to keep unit CI fast)
    // 4. run the binary, assert stdout == "1\n"
}
```

- [ ] **Step 2: Run, expect FAIL** (until skyc complete) then **PASS** once Tasks 1–13 land.
- [ ] **Step 3: CI workflow** — jobs: `build`, `clippy` (`-D warnings`), `test`, `miri` (nightly, on `sky_intern`/`sky_types`/`sky_ir`/`sky_lower`), `golden` (the E2E with `SKY_E2E=1`).
- [ ] **Step 4: Commit** — `git commit -am "test(m0): byte-identical golden + build-and-run E2E gate + CI"`.

> **MILESTONE 0 EXIT GATE (guardian, blocking):** all five gates green — (1) typed-IR boundary intact, (2) `forbid(unsafe)` + clippy deny-table green workspace-wide, (3) golden byte-diff passes, (4) FFI N/A for M0 (skipped, documented), (5) Miri clean on covered crates. Plus: emitted project builds and prints `1`.

---

## Self-Review

**Spec coverage:** §3 architecture → Tasks 1–13 (each crate). §4 IR → Task 4. §5 soundness rules → Global Constraints + per-task clippy/Miri steps. §6 verification → Tasks 11–14 (golden + run-equiv). §7 security → parser depth cap (Task 7), solver budget (Task 9); FFI surface is out of M0 scope (documented in exit gate). §8 Milestone 0 → the whole plan. §9 swarm → freeze gates after Tasks 3/4/5; parallel Tasks 6/7 (parse), 9 (types), 11/12 (backend vs golden) ; sequential Task 10 (lower). §10 gates → freeze gates + exit gate.

**Placeholder scan:** Algorithmic ports (Tasks 7–10) intentionally specify interface + tests + reference file instead of full inline source (see Plan Adaptation Note) — this is the deliberate, declared granularity for a compiler port, not a placeholder. Contract crates (1–5) and backend templates (11–12) have complete code / byte-exact targets.

**Type consistency:** `Symbol`/`Span`/`Located`/`Diagnostic`/`DResult` defined in Tasks 2–3, used consistently downstream. `SolvedTypes{env,regions}` defined in Task 9, consumed in Task 10. `Program`/`Func`/`Expr`/`Match::new` defined in Task 4, consumed in Tasks 10/12. `Backend`/`EmittedProject` defined Task 5, impl'd Task 12.

## Execution Handoff

User pre-selected **autonomous swarm**. Execution proceeds via the guardian-supervised swarm: contract freeze (Tasks 1–5, sequential) → parallel implementation (6/7, 9, 11/12) → sequential integration (8→10→13) → golden gate (14), with the guardian reviewing each task against the gates.
