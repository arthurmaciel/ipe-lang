# M5b-db follow-ups — implementation plan

> Guardian planner (#34). Doc-only plan. Read-only grounding pass against HEAD
> `691e275` (registry-migration Phase B). Every line anchor below was
> re-verified against HEAD; where the originating spec/task title had drifted,
> this plan corrects it inline and says so.

## Goal

Close the four M5b-db follow-ups to a make-invalid-states-unrepresentable bar:

1. **SqlValue 7→9 variants** — `SqlDecimal` / `SqlMoney` reachable end-to-end.
   **Drift correction:** this is ALREADY wired at HEAD across canon
   (`sky_canon/src/env.rs`), types (`sky_types/src/constrain.rs`), lowering
   (`sky_lower/src/lower.rs`), backend projection
   (`sky_backend_rust/src/project.rs`), and a golden fixture
   (`tests/golden/m5b_db_sql_decimal_money`). The only residual defect is a
   **stale doc comment** in `synthetic_sqlvalue_enum` that still lists 7. Task 1
   fixes the comment and adds a structural regression test that pins the variant
   count/shape at 9, so a future edit cannot silently drop a variant.
2. **Exhaustive `emit_db_call` (no wildcard)** — replace the guarded
   `_ if k.is_db() => Err(CompilerBug)` + trailing `_ => Ok(None)` pair (Rust
   cannot check exhaustiveness through a match guard) with a wildcard-free
   `match` over a backend-local `DbKernel` enum, so adding a Db kernel is a
   *compile error* in the emitter, not a codegen-time diagnostic reached only
   when a program happens to call it.
3. **Self-oracle** — a golden fixture that round-trips the five currently
   un-exercised `SqlValue` variants (`SqlFloat`, `SqlBool`, `SqlBytes`,
   `SqlTime`, `SqlNull`) through a real SQLite so the projection arms are proven
   by execution, not just by `cargo build`.
4. **db-without-live build** — prove (unit) that a Db-only program's manifest
   promotes `db` but NOT `server`/`live`, and prove (integration) the runtime
   crate compiles standalone under `--no-default-features --features db`. Both
   are structurally supported today but untested; the runtime `[features]`
   comment (`runtime/Cargo.toml:78-83`) *claims* the standalone build works —
   this plan adds the guard that keeps the claim honest.

## Architecture

The M5b-db data path, ground-truth at HEAD:

```
Sky source  SqlInt 5 / SqlDecimal "3.14" / SqlNull (SqlInt 0)
  │
  ├─ sky_canon/src/env.rs        ctor name→(type, index, arity) table
  │                              (SqlValue idx 0..8, SqlField idx 0..1)  [env.rs:108-122]
  ├─ sky_types/src/constrain.rs  per-ctor HM schemes  (payload → SqlValue) [constrain.rs:395-479]
  ├─ sky_lower/src/lower.rs      builtins interning     [lower.rs:4671-4683]
  │                              ctor_arity seeding      [lower.rs:832-835]
  │                              synthetic_sqlvalue_enum [lower.rs:958-1013]  ← 9 variants, stale DOC
  │                              synthetic_sqlfield_enum [lower.rs:1015-…]
  │                              emit_db_call dispatch is in the BACKEND, not here
  └─ sky_backend_rust/
        emit_expr.rs             emit_db_call()          [emit_expr.rs:466-823]  ← wildcard pair
        project.rs               emit_db_projection_impls [project.rs:785-837]  ← into_sql_param 9 arms
        project.rs               db_cargo_toml / server_cargo_toml [project.rs:381-517]
runtime/src/sky_runtime/db.rs
        SqlParam { Text,Int,Float,Bool,Bytes,Null }      [db.rs:1657-1671]  (6-variant target)
        bind_sql_param (TOTAL, per-variant)              [db.rs:1696-1705]
runtime/Cargo.toml               db = [sqlx,tokio,serde_json,json,sha2]  [Cargo.toml:84]  (no live/server)
```

Two facts that shape the design:

* **`SqlParam` (runtime, 6 variants) is intentionally narrower than `SqlValue`
  (Sky, 9 variants).** The 9→6 collapse happens in the *generated*
  `into_sql_param` (`project.rs:813-826`): `SqlTime→Int`, `SqlDecimal→Text`,
  `SqlMoney→Text`, `SqlNull(_)→Null`. Do not widen `SqlParam`; the collapse is
  the parity contract with Go (`db.rs:1635-1643`).
* **`IrType::Bytes → "Vec<u8>"`** (`emit_types.rs:118`). So the generated
  `SqlBytes(Vec<u8>)` variant and `Self::SqlBytes(v) => SqlParam::Bytes(v)`
  are type-consistent. The runtime module comment `s.into_bytes()`
  (`db.rs:1639`) is a stale hypothetical — the real emission passes the
  `Vec<u8>` straight through. No code change; Task 3 exercises it.

`DbKernel` (Task 2) lives in the **backend** crate (`sky_backend_rust`), NOT in
`sky_kernels`, specifically to stay off the registry migration's write path
(see Global Constraints → parallel-safety).

## Tech Stack

* Rust 2021, workspace toolchain pinned by `rust-toolchain.toml`.
* Test runners: `cargo test -p <crate>` for unit/integration; the E2E golden
  suite gates on `SKY_E2E=1` (`crates/skyc/tests/golden_m5b_db.rs:100-107`).
* SQLite via `sqlx` 0.8 (`runtime-tokio-rustls`, `sqlite`) in emitted projects.
* Golden oracle harness: `crates/skyc/tests/support/` (`build_and_run_emitted`,
  `assert_go_parity`). Db goldens are `oracle_divergence = true` (self-checked
  constant output; no Go run) — see `golden_m5b_db.rs:10-34`.
* Enumeration for completeness tests: `StdlibKernel::ALL`
  (`sky_kernels/src/lib.rs:1134`) — the canonical wired-variant slice, already
  tripwire-tested by `canon_equals_registry` in `sky_canon`.

## Global Constraints

**PRINCIPLES order (strict, from `PRINCIPLES.md`):**
`security > correctness > soundness > efficiency > completeness > readability`.
When two pulls conflict, the earlier wins. Concretely here: the exhaustive
`emit_db_call` (soundness) outranks the small readability cost of a second enum;
the `SqlParam` 9→6 collapse (correctness/parity) outranks a "cleaner" symmetric
`SqlParam` (completeness).

**Rule 1 — PARSE, DON'T VALIDATE.** Convert unstructured input into a type that
makes the good state the only representable one, once, at the boundary. Task 2
turns the `is_db()` boolean predicate + string-keyed dispatch into a
`DbKernel` sum whose *existence* proves the kernel is a Db kernel and whose
match is exhaustive — downstream code never re-checks "is this really a Db
kernel".

**Rule 2 — MAKE INVALID STATES UNREPRESENTABLE.** No wildcard/guard arm may
absorb an unhandled Db kernel. A newly added Db kernel MUST fail to compile
until it is wired, rather than reach a runtime/codegen diagnostic. `SqlParam`
stays closed (6 variants); `SqlValue`/`SqlField` are backend-injected closed
enums.

**Fail-closed, never panic/wildcard.** All new error paths return
`Diagnostic::CompilerBug { where_, detail }` (the existing pattern,
`emit_expr.rs:492-496`). No `unwrap`/`expect`/`panic!`/`todo!` in library code.
Diagnostics name the offending kernel and the fix.

**Parity note (PUBLIC-artifact rule).** Where `../sky` (the Go reference
backend) is named it is a capability/parity reference only. The Db path
diverges deliberately: Go emits `database/sql` + `mattn/go-sqlite3` (cgo); ipê
emits `sqlx` + `sqlx-sqlite`. Row shape is identical
(`[]map[string]any` ⇔ `Vec<HashMap<String,String>>`); the concrete runtime
differs. This is recorded as `oracle_divergence = true` on every Db golden and
is not a defect in either backend.

**Parallel-safety / file overlap.**

* **Registry migration (in flight — `sky_kernels` leaf crate, parse-once
  seam; commits `45e7e9d`/`691e275`).** It owns `sky_kernels/src/lib.rs`
  (the `StdlibKernel` enum, `is_db()`, `ALL`) and the callee-resolution seam in
  `constrain.rs`/`lower.rs`. **Task 2 must NOT edit `sky_kernels/src/lib.rs`.**
  `DbKernel` + its classifier live entirely in `sky_backend_rust`
  (`emit_expr.rs`, or a new `emit_db.rs`). Task 2 only *reads* `StdlibKernel`
  and `StdlibKernel::ALL`. If the migration renames a `Db*` variant, Task 2's
  classifier is a mechanical rename in one backend file.
* **#49 TCO (pending — `sky_ir` +2 variants, `lower.rs`, `emit_expr.rs`).**
  It adds loop/`continue` emission for tail-recursive functions. Its
  `emit_expr.rs` edits live in the *expression-tail* region, disjoint from
  `emit_db_call` (`emit_expr.rs:466-823`) and the new `DbKernel` block. To
  eliminate even textual conflict risk, Task 2 SHOULD land `DbKernel` in a new
  file `crates/sky_backend_rust/src/emit_db.rs` and leave only the
  `emit_db_call` body swap in `emit_expr.rs`. Sequence Task 2 before or after
  #49 with a rebase; they do not share logic.
* Task 1 edits `lower.rs` doc + adds a test in the existing `lower.rs` test mod
  (`lower.rs:4597`) — overlaps #49's `lower.rs` edits only at file scope; land
  on separate commits.
* Tasks 3 & 4 add new files only (golden fixtures, a runtime integration test,
  a project.rs unit test) — no overlap with any in-flight work.

**Definition of done per task:** failing test written first, observed red,
minimal impl, observed green, `cargo fmt` + `cargo clippy -p <crate>`
clean, one commit. Commit trailer: `Claude-Session:` line per repo rule; NO
co-author line.

---

## Task 1 — Pin `SqlValue` at 9 variants; correct the stale doc

**Rationale.** The 9 variants are already wired (drift-corrected above). This
task removes the misleading 7-variant doc and adds a structural test so the
count/shape cannot silently regress (e.g. a future refactor dropping
`SqlDecimal`). Cheapest, highest-leverage guard.

**Files.**
* `crates/sky_lower/src/lower.rs` — `synthetic_sqlvalue_enum` doc
  (`lower.rs:942-957`), body (`lower.rs:958-1013`); test mod (`lower.rs:4597`).

**Interfaces.**
* Consumes: `Lowerer::synthetic_sqlvalue_enum(&self) -> EnumDef`
  (`lower.rs:958`). `EnumDef { name: Symbol, type_params: Vec<…>, variants:
  Vec<Variant> }`; `Variant { name: Symbol, fields: Vec<IrType> }`.
* Consumes: `self.builtins.{sql_string,sql_int,sql_float,sql_bool,sql_bytes,
  sql_time,sql_decimal,sql_money,sql_null}` — all present
  (`lower.rs:4673-4681`).
* Produces: no signature change. A test proving 9 variants in canonical order
  with the expected field `IrType`s.

**Steps.**

1. **Write failing test.** In `lower.rs`'s `#[cfg(test)] mod tests`
   (`lower.rs:4597`), add:

   ```rust
   #[test]
   fn synthetic_sqlvalue_has_nine_variants_in_canonical_order() {
       let lowerer = test_lowerer(); // existing helper that builds a Lowerer w/ interned builtins
       let e = lowerer.synthetic_sqlvalue_enum();
       let b = lowerer.builtins;
       let expected: [(Symbol, &[IrType]); 9] = [
           (b.sql_string,  &[IrType::Str]),
           (b.sql_int,     &[IrType::Int]),
           (b.sql_float,   &[IrType::Float]),
           (b.sql_bool,    &[IrType::Bool]),
           (b.sql_bytes,   &[IrType::Bytes]),
           (b.sql_time,    &[IrType::Int]),
           (b.sql_decimal, &[IrType::Str]),
           (b.sql_money,   &[IrType::Str]),
           (b.sql_null,    &[IrType::Enum { name: b.sqlvalue, args: vec![] }]),
       ];
       assert_eq!(e.variants.len(), 9, "SqlValue must have exactly 9 variants");
       for (i, (name, fields)) in expected.iter().enumerate() {
           assert_eq!(e.variants[i].name, *name, "variant {i} name");
           assert_eq!(e.variants[i].fields.as_slice(), *fields, "variant {i} fields");
       }
   }
   ```

   If no `test_lowerer()` helper exists, inspect the existing tests near
   `lower.rs:4597` for the current construction idiom (the builtins are interned
   there at `lower.rs:4671-4683`) and reuse it verbatim; do not invent a new
   fixture.

2. **Run it — expect a red only if drift regressed the body.** Ground truth:
   the body at HEAD already emits 9 in this order, so this test should *pass*
   immediately, which is acceptable for a pin test. To confirm the test has
   teeth, temporarily delete the `sql_money` variant from the body, run:

   ```
   cargo test -p sky_lower synthetic_sqlvalue_has_nine
   ```

   Expected: `assertion failed: variants.len() == 9` (or a name mismatch at
   index 7). Restore the variant.

3. **Minimal impl — fix the doc.** Replace the `synthetic_sqlvalue_enum` doc
   block (`lower.rs:944-952`) so the `type SqlValue` listing includes all 9:

   ```text
   /// type SqlValue
   ///     = SqlString String
   ///     | SqlInt Int
   ///     | SqlFloat Float
   ///     | SqlBool Bool
   ///     | SqlBytes Bytes
   ///     | SqlTime Int          -- Unix-millisecond timestamp → SqlParam::Int
   ///     | SqlDecimal String    -- lossless decimal TEXT       → SqlParam::Text
   ///     | SqlMoney String      -- "ISO_CODE AMOUNT" TEXT       → SqlParam::Text
   ///     | SqlNull SqlValue     -- type-witness, discarded       → SqlParam::Null
   ```

4. **Run green.**
   ```
   cargo test -p sky_lower synthetic_sqlvalue_has_nine
   ```
   Expected: `test result: ok. 1 passed`.

5. **Lint + commit.**
   ```
   cargo fmt -p sky_lower && cargo clippy -p sky_lower --all-targets -- -D warnings
   git add crates/sky_lower/src/lower.rs && git commit
   ```
   Message: `M5b-db: pin SqlValue at 9 variants + correct stale synth doc`.

---

## Task 2 — Exhaustive `emit_db_call` via a backend-local `DbKernel`

**Rationale.** `emit_db_call` currently ends with
`_ if k.is_db() => Err(CompilerBug)` (`emit_expr.rs:813-819`) and
`_ => Ok(None)` (`emit_expr.rs:820-821`). The guard makes the miss *fail-closed
at codegen* — good — but Rust's exhaustiveness checker cannot see through a
guard, so a new Db kernel is only caught when a user program calls it, and a Db
kernel that was never added to `is_db()` falls through `_ => Ok(None)` to the
standard path **silently** (wrong arg wiring, exit-0-then-cargo-fail). This task
makes the miss a *compile error in the emitter itself*.

**Design (one pass — no prior spec).** Introduce a backend-local sum whose
variants are exactly the Db kernels, plus a total classifier:

```rust
// crates/sky_backend_rust/src/emit_db.rs  (new file)
#[derive(Clone, Copy)]
pub(crate) enum DbKernel {
    Connect, Open, Close, ExecRaw, Exec, Query, QueryDecode,
    GetString, GetInt, GetBool, GetField, InsertRow, GetById,
    UpdateById, DeleteById, FindOneByField, FindManyByField,
    FindByConditions, UnsafeFindWhere, InsertFields, UpdateFields,
    InsertFieldsReturning, WithTransaction, Migrate,
    DecString, DecInt, DecFloat, DecBool, DecNullable, DecMap,
    DecAndThen, DecSucceed, DecFail, DecMap2, DecMap3, DecMap4,
    DecRequired, DecOptional,
}

/// Total classifier. The SOLE StdlibKernel→DbKernel map; its completeness is
/// guarded by `db_kernel_covers_is_db` (below). Non-Db kernels → None.
pub(crate) fn db_kernel(k: sky_ir::KernelFn) -> Option<DbKernel> {
    use sky_ir::KernelFn as K;
    Some(match k {
        K::DbConnect => DbKernel::Connect,
        K::DbOpen => DbKernel::Open,
        // … one arm per Db variant …
        K::DbDecOptional => DbKernel::DecOptional,
        _ => return None,
    })
}
```

Then `emit_db_call`'s body becomes:

```rust
let Callee::Kernel(k) = callee else { return Ok(None); };
let Some(dbk) = db_kernel(*k) else { return Ok(None); };
match dbk {
    DbKernel::ExecRaw => { /* existing body */ }
    // … EVERY DbKernel variant, NO wildcard, NO guard …
    DbKernel::Connect | DbKernel::Open | DbKernel::Close
    | DbKernel::DecString | … | DbKernel::DecOptional => Ok(None), // standard-path group
}
```

The inner `match dbk` has **no wildcard** — adding a `DbKernel` variant is a
`non-exhaustive patterns` compile error here. The single residual `_ => None`
lives in `db_kernel` (classification only) and is completeness-tested against
`StdlibKernel::ALL` + `is_db()`, closing the "forgot to classify" gap that the
old `_ if k.is_db()` guard could only catch at codegen time.

**Why not put `DbKernel` in `sky_kernels`?** That crate is the registry
migration's active write surface (Global Constraints). Keeping `DbKernel`
backend-local costs one mechanical rename if a variant is renamed upstream, and
removes all merge contention.

**Files.**
* NEW `crates/sky_backend_rust/src/emit_db.rs` — `DbKernel`, `db_kernel`,
  completeness tests.
* `crates/sky_backend_rust/src/lib.rs` — `mod emit_db;` (find the existing
  `mod emit_expr;` line and add beside it).
* `crates/sky_backend_rust/src/emit_expr.rs` — swap `emit_db_call` body
  (`emit_expr.rs:466-823`) to dispatch on `db_kernel`; delete the
  `_ if k.is_db() => Err(...)` and `_ => Ok(None)` arms.

**Interfaces.**
* Consumes: `sky_ir::KernelFn` (= `sky_kernels::StdlibKernel`),
  `StdlibKernel::is_db(self) -> bool` (`sky_kernels/src/lib.rs:1585`),
  `StdlibKernel::ALL: &'static [Self]` (`sky_kernels/src/lib.rs:1134`).
* Consumes: existing per-arm emit bodies in `emit_db_call`
  (`emit_expr.rs:533-783`) — moved verbatim, only the match scrutinee changes
  from `KernelFn` to `DbKernel`.
* Produces: `pub(crate) enum DbKernel`, `pub(crate) fn db_kernel(KernelFn) ->
  Option<DbKernel>`. `emit_db_call` signature unchanged
  (`emit_expr.rs:476-483`).

**Steps.**

1. **Write failing completeness test first** (drives `db_kernel` into
   existence). In `emit_db.rs`'s test mod:

   ```rust
   #[cfg(test)]
   mod tests {
       use super::db_kernel;
       use sky_ir::KernelFn;

       /// Every is_db() kernel MUST classify to Some(DbKernel); every non-db
       /// kernel MUST classify to None. This is the sole classification and its
       /// completeness closes the "new Db* variant not wired" gap that the old
       /// `_ if k.is_db()` codegen guard could only catch late.
       #[test]
       fn db_kernel_covers_is_db_exactly() {
           for &k in KernelFn::ALL {
               assert_eq!(
                   db_kernel(k).is_some(),
                   k.is_db(),
                   "classification disagrees with is_db() for {k:?}"
               );
           }
       }
   }
   ```

2. **Run — expect red (does not compile: `db_kernel` missing).**
   ```
   cargo test -p sky_backend_rust db_kernel_covers_is_db_exactly
   ```
   Expected: `error[E0433]: failed to resolve: use of undeclared … db_kernel`.

3. **Minimal impl.** Create `emit_db.rs` with `DbKernel` + `db_kernel` (all 38
   arms, one per the `is_db()` list `sky_kernels/src/lib.rs:1588-1625`); add
   `mod emit_db;` to `lib.rs`. Run the test:
   ```
   cargo test -p sky_backend_rust db_kernel_covers_is_db_exactly
   ```
   Expected: `test result: ok. 1 passed`. If it fails with a `{k:?}` name, that
   variant is missing an arm — add it (this is the guard working).

4. **Swap `emit_db_call` body.** In `emit_expr.rs`, change the scrutinee: after
   the `Callee::Kernel(k)` guard (`emit_expr.rs:485-487`), add
   `let Some(dbk) = crate::emit_db::db_kernel(*k) else { return Ok(None); };`
   and change `match k {` to `match dbk {`, rewriting each arm pattern from
   `KernelFn::DbExecRaw` to `DbKernel::ExecRaw`, etc. Delete the
   `_ if k.is_db() => Err(...)` and `_ => Ok(None)` arms (`emit_expr.rs:807-821`)
   — the match is now total over `DbKernel`. Keep the standard-path group arm
   (`DbKernel::Connect | … | DbKernel::DecOptional => Ok(None)`) explicit.
   Update the `arg!` macro's `{:?}` on `k` to still print `*k` (the original
   `KernelFn`) for diagnostics.

5. **Prove no wildcard remains + still exhaustive.** Add a targeted test
   asserting each Db kernel routes (Some for projection kernels, Ok(None) for
   standard-path). Since `emit_db_call` needs an `EmitCtx`, keep this light:
   assert only the *classification* half here (already covered by step 1) and
   rely on the golden suite (Task 3 + existing `golden_m5b_db.rs`) for the emit
   bodies. Then force the exhaustiveness teeth:
   ```
   # temporarily comment one DbKernel arm in emit_db_call, then:
   cargo build -p sky_backend_rust
   ```
   Expected: `error[E0004]: non-exhaustive patterns: `DbKernel::Migrate` not
   covered` (or whichever arm). Restore the arm. This is the make-invalid-
   states-unrepresentable proof.

6. **Full crate test + lint.**
   ```
   cargo test -p sky_backend_rust
   cargo clippy -p sky_backend_rust --all-targets -- -D warnings
   cargo fmt -p sky_backend_rust
   ```
   Expected: all pass. Note: the `#[allow(clippy::match_same_arms)]` /
   `#[allow(clippy::too_many_lines)]` on `emit_db_call` (`emit_expr.rs:467-475`)
   stay; the standard-path group arm keeps `match_same_arms` relevant.

7. **Commit.** `git add crates/sky_backend_rust/src/{emit_db.rs,lib.rs,emit_expr.rs}`
   → `M5b-db: exhaustive emit_db_call via backend-local DbKernel (no wildcard)`.

---

## Task 3 — Self-oracle golden for the 5 un-exercised `SqlValue` variants

**Rationale.** Grepping the fixtures, only `SqlString`/`SqlInt`/`SqlDecimal`/
`SqlMoney` are exercised end-to-end; `SqlFloat`, `SqlBool`, `SqlBytes`,
`SqlTime`, `SqlNull` are proven only by `cargo build`, never by execution
against a real driver. A self-oracle fixture (constant expected output, ipê-
checked, `oracle_divergence = true` like the sibling goldens) closes that.

**SqlBytes nuance (resolved).** `Db.query` returns
`Vec<HashMap<String,String>>`, so a BLOB column read back through `getString`
is not a clean round-trip. The fixture therefore **writes** a `SqlBytes` param
and asserts the *row is inserted* (via a `COUNT(*)` / a companion non-blob
column), rather than reading the blob back as text. This proves the
`SqlBytes(Vec<u8>) → SqlParam::Bytes(v)` bind path executes without asserting a
lossy text decode. Construct the bytes with `Bytes.fromString "hi"` (verify the
kernel name/signature in `sky-stdlib`/constrain before writing; if
`Bytes.fromString` is not yet wired in the Rust backend, use `Bytes.fromHex`
whichever is proven by an existing golden — do NOT introduce an unwired kernel
just for this fixture).

**SqlNull nuance.** `SqlNull (SqlInt 0)` binds as SQL NULL (witness discarded).
Assert the read-back column is absent/`Nothing` via a `Decode.nullable`
decoder (the pattern already works — `m5b_db_nullable` fixture).

**Files.**
* NEW `tests/golden/m5b_db_all_sqlvalues/Main.sky`
* NEW `tests/golden/m5b_db_all_sqlvalues/expected_go.txt`
* NEW `tests/golden/m5b_db_all_sqlvalues/oracle.meta`
* NEW `tests/golden/m5b_db_all_sqlvalues/sanctioned.divergence`
* `crates/skyc/tests/golden_m5b_db.rs` — add `fn db_all_sqlvalues()`.

**Interfaces.**
* Consumes: `assert_runs_and_matches_oracle(name: &str)`
  (`golden_m5b_db.rs:100-107`), `support::build_and_run_emitted`,
  `support::assert_go_parity`.
* Consumes: kernels `Db.open`, `Db.execRaw`, `Db.exec`, `Db.query`,
  `Db.queryDecode`, `Db.getString`/`getInt`, `Decode.{string,int,float,bool,
  nullable,map*}` — all proven by existing fixtures.
* Produces: one deterministic stdout line asserted as the oracle.

**Steps.**

1. **Write the fixture (the failing artifact).** `Main.sky`:
   `Db.open "sqlite" "sqlite::memory:"` → `Db.withTransaction` →
   `Db.execRaw "CREATE TABLE t (f REAL, b INTEGER, ts INTEGER, blob BLOB, n INTEGER)"`
   → `Db.exec "INSERT INTO t (f,b,ts,blob,n) VALUES (?,?,?,?,?)"
   [ SqlFloat 2.5, SqlBool True, SqlTime 1700000000000, SqlBytes (Bytes.fromString "hi"), SqlNull (SqlInt 0) ]`
   → `Db.queryDecode "SELECT f,b,ts,n FROM t" [] decoder` where `decoder` reads
   `f:Float`, `b:Bool`, `ts:Int`, `n : Maybe Int` (via `Decode.nullable`) →
   format `"2.5|true|1700000000000|nothing"` → `println`.
   Model the structure on `m5b_db_sql_decimal_money/Main.sky` verbatim
   (`Task.andThen` nesting, `let _ =` auto-forced effects).

2. **Determine the true oracle by running once.** Because this is
   `oracle_divergence = true`, the expected output IS ipê's own verified output.
   Capture it:
   ```
   SKY_E2E=1 cargo test -p skyc --test golden_m5b_db db_all_sqlvalues -- --nocapture
   ```
   First run: the test will fail on a missing/empty `expected_go.txt`. Read the
   actual stdout from `--nocapture`, hand-verify each field is correct
   (Float formatting, Bool `true`, the millis Int, `nothing` for the NULL), and
   only then write that exact string to `expected_go.txt`. Fill `oracle.meta`
   and `sanctioned.divergence` copying the shape from
   `m5b_db_sql_decimal_money/` (adjust the note to name the 5 variants + the
   SqlBytes-write-only rationale).

   **Guard against Float-format drift:** confirm the printed float matches Sky's
   `String.fromFloat`/`toString` contract (cross-check task #52 — float
   sci-notation threshold). If `2.5` prints as `2.5`, good; pick a value with no
   sci-notation ambiguity (avoid very large/small floats).

3. **Add the test.** In `golden_m5b_db.rs`, append:
   ```rust
   /// Round-trips the five SqlValue variants not exercised by other goldens:
   /// SqlFloat, SqlBool, SqlTime, SqlBytes (write-only — BLOB not read back as
   /// text), SqlNull (via Decode.nullable). Proves the into_sql_param arms for
   /// idx 2,3,5 + SqlBytes + SqlNull execute against a real sqlx driver.
   #[test]
   fn db_all_sqlvalues() {
       assert_runs_and_matches_oracle("m5b_db_all_sqlvalues");
   }
   ```
   Update the module-doc golden catalogue (`golden_m5b_db.rs:36-49`) with the
   new entry.

4. **Run green.**
   ```
   SKY_E2E=1 cargo test -p skyc --test golden_m5b_db db_all_sqlvalues
   ```
   Expected: `test result: ok. 1 passed`. Also run the whole db golden set to
   ensure no fixture-dir collision:
   ```
   SKY_E2E=1 cargo test -p skyc --test golden_m5b_db
   ```

5. **Commit.** `git add tests/golden/m5b_db_all_sqlvalues/ crates/skyc/tests/golden_m5b_db.rs`
   → `M5b-db: self-oracle golden for SqlFloat/Bool/Time/Bytes/Null`.

---

## Task 4 — db-without-live build guard (unit + integration)

**Rationale.** `project.rs` composes features independently
(`project.rs:287-331`): `uses_db` promotes `db`; `uses_server||uses_live||
uses_webview` promotes `server`; `uses_live||uses_webview` promotes `live`. A
Db-only program therefore already gets `db` without `server`/`live`. And the
runtime `db` feature is self-contained (`runtime/Cargo.toml:84` —
`db = [sqlx,tokio,serde_json,json,sha2]`, no `live`/`server`; `db.rs` references
no `crate::live`/`crate::server`). But **neither fact is tested**. This task
adds the two guards.

**Files.**
* `crates/sky_backend_rust/src/project.rs` — new unit test in the existing
  `#[cfg(test)] mod tests` (`project.rs:839-917`).
* NEW `runtime/tests/db_standalone_build.rs` OR a scripts/CI line — integration
  guard that the runtime compiles under `--no-default-features --features db`.
  (Prefer a `#[test]` that shells `cargo build`; if the repo forbids nested
  cargo in tests, add a line to CI instead — see step 3 note.)

**Interfaces.**
* Consumes: `db_cargo_toml() -> DResult<String>` (`project.rs:381`),
  `default_line(&str) -> &str` test helper (`project.rs:844-849`).
* Consumes (integration): the runtime crate manifest features
  (`runtime/Cargo.toml:74-99`).
* Produces: `db_toml_promotes_db_not_server_or_live` unit test; a standalone
  build guard.

**Steps.**

1. **Write failing unit test.** In `project.rs` `mod tests`:
   ```rust
   /// A Db-only program (no server/live/webview) must promote `db` but NOT
   /// `server` or `live` in the default feature list. Guards the independent-
   /// feature composition in `emit` (project.rs:287-331).
   #[test]
   fn db_toml_promotes_db_not_server_or_live() {
       let out = db_cargo_toml().expect("db_cargo_toml must succeed");
       let def = default_line(&out);
       assert!(def.contains(r#""db""#),     "db must be promoted: {def}");
       assert!(!def.contains(r#""server""#), "server must NOT be promoted: {def}");
       assert!(!def.contains(r#""live""#),   "live must NOT be promoted: {def}");
       // sqlx present; axum absent (axum only via server_cargo_toml).
       assert!(out.contains("sqlx"),  "sqlx dep expected: {out}");
       assert!(!out.contains("axum"), "axum dep must be absent: {out}");
   }
   ```

2. **Run.**
   ```
   cargo test -p sky_backend_rust db_toml_promotes_db_not_server_or_live
   ```
   Expected: green immediately (composition already correct — this is a
   characterization/regression pin). To confirm teeth, temporarily make
   `db_cargo_toml` also append `, "server"` and observe the `!contains`
   assertion fail; revert.

3. **Integration guard — runtime standalone db build.** Add
   `runtime/tests/db_standalone_build.rs`:
   ```rust
   //! Guards runtime/Cargo.toml:78-83's claim that `--features db` builds
   //! standalone (no live/server, no default crypto). Fail-closed: a missing
   //! feature dep (e.g. sha2 for db_migrate_apply) surfaces here, not in a
   //! user's generated project.
   #[test]
   fn runtime_builds_with_only_db_feature() {
       let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
       let out = std::process::Command::new(env!("CARGO"))
           .args(["build", "--quiet", "--manifest-path", manifest,
                  "--no-default-features", "--features", "db"])
           .output()
           .expect("cargo build must spawn");
       assert!(
           out.status.success(),
           "runtime must compile with --no-default-features --features db\n{}",
           String::from_utf8_lossy(&out.stderr)
       );
   }
   ```
   Note: nested `cargo build` in a test is heavy but bounded; gate it behind
   `SKY_E2E` if the default `cargo test` budget is tight — mirror the
   `golden_m5b_db.rs:101` early-return idiom. If repo policy forbids
   cargo-in-test entirely, instead add the invocation to CI
   (`.github/workflows/*.yml`) as a matrix cell and reference it here; verify
   which by checking whether any existing `runtime/tests/*.rs` shells cargo
   before choosing.

4. **Run the guard.**
   ```
   cargo test -p sky_backend_rust db_toml_promotes_db_not_server_or_live
   cargo test -p sky_runtime --test db_standalone_build   # or: SKY_E2E=1 …
   ```
   Expected: both green. The standalone build must emit no `feature ... not
   found` / `unresolved import crate::live` errors.

5. **Lint + commit.**
   ```
   cargo fmt -p sky_backend_rust -p sky_runtime
   cargo clippy -p sky_backend_rust --all-targets -- -D warnings
   git add crates/sky_backend_rust/src/project.rs runtime/tests/db_standalone_build.rs
   git commit
   ```
   Message: `M5b-db: guard db-without-live (manifest unit + runtime standalone build)`.

---

## Sequencing & integration

* **Order:** Task 1 → Task 4 → Task 2 → Task 3. Tasks 1 and 4 are pure
  additive guards (safe any time). Task 2 is the structural change; land it on a
  clean tree and rebase over any registry-migration / #49 movement. Task 3
  depends on nothing but is the slowest (E2E) — run last.
* **After all four:** run the full db golden set once under E2E and the two
  crate test suites:
  ```
  cargo test -p sky_lower -p sky_backend_rust
  SKY_E2E=1 cargo test -p skyc --test golden_m5b_db
  ```
* **Background hygiene (repo rule):** none of these tasks spawn long-running
  servers; no `run_in_background` cleanup needed beyond the standard end-of-
  session sweep.

## Residual gaps explicitly out of scope

* Making `is_db()` itself exhaustive over `StdlibKernel` (the "forgot to add to
  is_db" gap) is task **#45**'s cross-cutting concern; Task 2 closes the *db*
  slice of it via `db_kernel_covers_is_db_exactly` but does not generalize.
* A native `IrType::Decimal` (so `SqlDecimal` carries a real decimal rather than
  a lossless `String`) is deferred — `lower.rs:992-995` documents the minimal
  `IrType::Str` wiring as intentional until then.
* `SqlBytes` BLOB *read-back* as typed bytes (vs the write-only proof in Task 3)
  needs a `Bytes`-typed column decoder in `Db.Decode`; file separately if a user
  needs it.
