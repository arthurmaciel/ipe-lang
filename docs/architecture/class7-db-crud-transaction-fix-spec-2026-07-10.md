# db_crud / db_transaction E2E fix spec — two unrelated regressions, one test file

> Root-cause + implementation spec for the BACKLOG "Hardening follow-ups" row
> *"Pre-existing E2E failures found while landing #61"*
> (`crates/skyc/tests/golden_m5b_db.rs`'s `db_crud` / `db_transaction`).
> All file:line pointers below verified against master `2bc2a67`
> (2026-07-10). Design-only pass — no fix implemented here; a Track-1 lane
> lands §3 and §4 as two independent commits.
>
> Repro (isolated target dir to avoid racing concurrent lanes):
>
> ```bash
> cd /home/arthur/Documentos/comp/sky-rust
> CARGO_TARGET_DIR=~/.cache/sky-rust-target-track2-dbcrud SKY_E2E=1 \
>   cargo nextest run -p skyc --test golden_m5b_db -E 'test(db_crud) or test(db_transaction)'
> # → 2 tests run: 0 passed, 2 failed  (db_crud in 0.05s at skyc type-check;
> #    db_transaction after the full emitted-project cargo build)
> ```

## Corrections to the backlog's framing (read this first)

1. **These are NOT mysterious pre-existing failures — both are recent
   regressions with identified culprit commits.** "Reproduced identically on
   unmodified master `00c4d32`" was correct but only proved #61 didn't cause
   them. The actual causes: `db_crud` was broken by **`bdbc572`**
   (2026-07-06, "Cluster U + Money" progressive-development checkpoint) and
   `db_transaction` by **`5db4cd3`** (2026-07-09, "bring the real Error
   ErrorKind ErrorInfo ADT", backlog #85/#160). Both were green at their
   creating commit `06cc599` (2026-07-01, "add the SQL database kernels").

2. **The two failures are unrelated bugs** that only share a test file.
   §3 (db_crud — Dict-vs-List kernel-surface drift) and §4 (db_transaction —
   `Task.fail` error-channel scheme vs the SkyError enum) are fully
   independent; land in either order.

3. **The blast radius of §4 is wider than the backlog row says.** The same
   root cause currently breaks **`golden_m5a_task::error_channel` and
   `golden_m5a_task::task_map_error_lambda`** under `SKY_E2E=1` (verified
   red at `2bc2a67`: `2 tests run: 0 passed, 2 failed`). §4's fix closes all
   three tests. Any E2E sweep accounting must count these two as part of
   this row, not as new discoveries.

---

## §1 — Failure A: `db_crud` fails skyc type-check (SKY-T0001 Dict vs List)

### Symptom

```
skyc: error[SKY-T0001]: type mismatch
  --> tests/golden/m5b_db_crud/Main.sky:18:29
   |
18 |                             Db.insertRow txconn
   |                             ^^^^^^^^^^^^ expected List (String, String), found Dict String String
```

The fixture never reaches emission.

### Root cause — one-sided kernel-surface change in `bdbc572`

Timeline of the `DbInsertRow` / `DbUpdateById` row-parameter type:

| Commit | Date | Scheme (constrain.rs) | Lowering/emitter | Fixture |
|---|---|---|---|---|
| `06cc599` | 07-01 | `List (String, String)` | Vec→HashMap convert | list-of-tuples literal — **green** |
| `1949452` (registry Phase D) | — | `list(tuple2(string(), string()))` relocated verbatim | unchanged | unchanged — still green |
| `bdbc572` | 07-06 | **flipped to `dict(string(), string())`** | **unchanged** | **unchanged — red since** |

`bdbc572`'s own message states the motive: *"ex27 (SKY-T0001 Db.insertRow
dict vs list): Fix DbInsertRow and DbUpdateById stdlib_scheme from
list(tuple2(string(), string())) to dict(string(), string())"*. That
direction is **correct** — the upstream reference
(`../sky/sky-stdlib/Std/Db.sky:144,157`) declares

```elm
insertRow : Db -> String -> Dict String String -> Task Error Int
updateById : Db -> String -> String -> Dict String String -> Task Error Int
```

and `examples/27-*` (written against upstream) exercises the Dict surface.
`06cc599`'s original `List (String, String)` surface was an unrecorded,
unsanctioned divergence from upstream. But `bdbc572` changed only the type
scheme and left the two consumers of the old surface stale:

* the golden fixture `tests/golden/m5b_db_crud/Main.sky:20,26` still passes
  raw list-of-tuples literals (its header comment, lines 3–5, still
  describes the "Vec<(String,String)>→HashMap conversion");
* the emitter arms `crates/sky_backend_rust/src/emit_expr.rs:1565`
  (DbInsertRow) and `:1579` (DbUpdateById) still emit
  `({row_s}).into_iter().collect::<HashMap<String, String>>()` with comments
  claiming "the Sky type is `List (String, String)`";
* the lowering arity-table comments `crates/sky_lower/src/lower.rs:7530`
  and `:7566` still document the List surface.

There is **no inference bug**: the solver is right to reject a
`List (String, String)` literal against a `dict(string(), string())`
parameter. The fixture is what's stale.

### Current declarations (verified at `2bc2a67`)

* Scheme: `crates/sky_types/src/constrain.rs:3883` (DbInsertRow),
  `:3894` (DbUpdateById) — both `dict(string(), string())`. **Keep.**
* Runtime: `runtime/src/sky_runtime/db.rs` `db_insert_row` /
  `db_update_by_id` take `HashMap<String, String>`; Sky `Dict String String`
  lowers to exactly `HashMap<String, String>`
  (`runtime/src/sky_runtime/dict.rs:15` — `pub type SkyDict<T> =
  HashMap<String, T>`). The surface and the runtime now agree with zero
  conversion needed.

### Design validation (performed on a scratch copy, no repo change)

A copy of the fixture with the two literals wrapped in `Dict.fromList`
type-checks, emits, `cargo check`s **and runs green with the current
compiler, unmodified**, producing exactly the oracle output
(`tests/golden/m5b_db_crud/expected_go.txt`: `apple/5` / `apple/10` /
`deleted`). The emitter's now-redundant
`.into_iter().collect::<HashMap<String, String>>()` still compiles when fed
a `HashMap` (HashMap → pair-iterator → HashMap round-trip), so the fixture
change alone flips the test green. The conversion removal below is hygiene,
not a gate.

### Fix

1. **Fixture** — `tests/golden/m5b_db_crud/Main.sky`:
   * line 20: `[ ( "id", "1" ), ( "name", "apple" ), ( "qty", "5" ) ]` →
     `(Dict.fromList [ ( "id", "1" ), ( "name", "apple" ), ( "qty", "5" ) ])`
   * line 26: `[ ( "qty", "10" ) ]` → `(Dict.fromList [ ( "qty", "10" ) ])`
   * lines 3–5 header comment: drop the "Vec<(String,String)>→HashMap
     conversion" description; the fixture now exercises the upstream-parity
     `Dict String String` surface end-to-end.
   * `expected_go.txt` unchanged (validated above).
2. **Emitter** — `crates/sky_backend_rust/src/emit_expr.rs:1565-1576`
   (DbInsertRow) and `:1579-…` (DbUpdateById): drop the
   `.into_iter().collect::<HashMap<String, String>>()` wrap — pass
   `{row_s}` straight through (the arg is already `HashMap<String,
   String>`) — and rewrite both arm comments to say the Sky surface is
   `Dict String String` (upstream parity, `bdbc572`). Per the
   prefer-concrete-codegen principle, do not keep a defensive re-collect
   over an already-correct type.
3. **Lowering comments** — `crates/sky_lower/src/lower.rs:7530,7566`:
   `List (String, String)` → `Dict String String` in the two arity-table
   comments (comment-only; the arity entries themselves are correct).
4. **No scheme change, no runtime change, no new divergence-ledger entry**
   (the Dict surface IS upstream's; the divergence being deleted was never
   recorded).

### Regression coverage

`golden_m5b_db.rs::db_crud` (`crates/skyc/tests/golden_m5b_db.rs:159`)
itself is the regression test — it runs the full
type-check → emit → cargo build → run → oracle-compare pipeline under
`SKY_E2E=1` and the check-only tier without it. No new test needed. If the
emitter cleanup (step 2) lands separately from the fixture (step 1), each
half keeps the test green on its own — validated above for step 1;
step 2 emits the identical runtime value.

---

## §2 — Failure B: `db_transaction` fails the emitted-project cargo build (E0308 String vs SkyError)

### Symptom

skyc succeeds; the emitted project fails:

```
error[E0308]: mismatched types
  --> src/main.rs:316
   |  task_fail("rollback-test".to_string())
   |  --------- ^^^^^^^^^^^^^^^ expected `SkyError`, found `String`
```

from the fixture's `Task.fail "rollback-test"`
(`tests/golden/m5b_db_transaction/Main.sky:49`).

### Root cause — `5db4cd3` turned `SkyError` into a real enum; the `Task.fail` scheme still admits any `e`

The emitted prelude wrapper has pinned the error channel since day one
(`tests/golden/m0/main.rs:123`, included verbatim into every emitted
project via `runtime_bindings()` at
`crates/sky_backend_rust/src/project.rs:331`):

```rust
pub fn task_fail<A: Send + 'static>(e: SkyError) -> SkyTask<A> { … }
```

Until `5db4cd3` this was harmless because the same prelude declared
`type SkyError = String;` — so `task_fail("…".to_string())` compiled and the
three String-error fixtures were genuinely green. `5db4cd3` replaced the
alias with the 11-kind `pub enum SkyError`
(`runtime/src/sky_runtime/error.rs:75`) and re-exported it into the prelude
(`tests/golden/m0/main.rs:45`), but added no compensating conversion at the
one call-site shape that still hands the wrapper a raw `String`.

The type checker cannot catch this today because the kernel scheme is
over-polymorphic: `crates/sky_types/src/constrain.rs:3674`

```rust
K::TaskFail => fun(var(1), task(var(0))),
```

`var(1)` accepts anything — while `task(_)` is the **one-argument** Task
constructor whose error slot is implicitly the fixed `Error` type, and every
sibling combinator already pins the channel:
`K::TaskMapError` (`:3677`) and `K::TaskOnError` (`:3678`) both use
`error_ty()`. So `Task.fail "x"` HM-checks as if the error channel were
polymorphic, the emitter (which has no TaskFail-specific arm — the wrapper
name comes from `crates/sky_backend_rust/src/naming.rs:574` via the generic
kernel-call path) passes the String expression through verbatim, and rustc
is the first thing to object. "Compilation successful → cargo build fails"
is exactly the failure class the project's own principles forbid.

The over-polymorphic scheme also contradicts skyc's own bundled stdlib,
which already declares the pinned form
(`crates/skyc/stdlib/Sky/Core/Task.sky:33`):

```elm
fail : Error -> Task Error a
```

### Why not an emitter-side `.into()` instead?

`SkyError` has `From<String>` / `From<&str>`
(`runtime/src/sky_runtime/error.rs:233,239` — both map to
`SkyError::unexpected`), so emitting `task_fail(({e}).into())` would compile
for both String and SkyError arguments (reflexive `From<T> for T` covers the
latter). **Rejected** — it converts the value but not the type judgement:
the Sky-side HM type of the error channel would still be `String`, while
every recovery-side wrapper (`task_on_error` handler param, `task_map_error`
fn) is pinned to `SkyError`. `Task.fail "x" |> Task.onError (\e ->
println (String.toUpper e))` would HM-check (`e : String`) and then fail the
emitted cargo build again — same bug class, one hop later. The scheme pin
fixes the judgement at the source; the checker rejects the program before
codegen with a proper SKY-T0001.

### Fix

1. **Scheme** — `crates/sky_types/src/constrain.rs:3674`:

   ```rust
   K::TaskFail => fun(error_ty(), task(var(0))),
   ```

   One line. Aligns `fail` with `mapError`/`onError`'s already-pinned
   channel and with `crates/skyc/stdlib/Sky/Core/Task.sky:33`.

2. **Fixtures** (all three call sites in the tree; verified via
   `rg 'Task\.fail "' crates/skyc/stdlib examples tests` — stdlib and
   examples have zero String-arg call sites, all examples already pass
   `Error.unexpected …` / `Error.io …` values):
   * `tests/golden/m5b_db_transaction/Main.sky:49` →
     `Task.fail (Error.unexpected "rollback-test")`
   * `tests/golden/m5a_error_channel/Main.sky:12` →
     `Task.fail (Error.unexpected "an error")`
   * `tests/golden/m5a_task_map_error_lambda/Main.sky:10` →
     `Task.fail (Error.unexpected "an error")`

   The `Error.unexpected …` shape is already green end-to-end today:
   `tests/golden/m86_error_module/Main.sky` emits
   `task_fail(sky_error_unexpected("boom".to_string()))` and passes a full
   `cargo check` (validated on a scratch emission at `2bc2a67`). Expected
   outputs unchanged: none of the three fixtures print the error value
   itself (the m5a pair prints a constant `recovered`; db_transaction's
   handler discards `err`). If the m5a fixtures' doc comments in
   `crates/skyc/tests/golden_m5a_task.rs:17,26,95,117` quote the old source,
   refresh the quotes.

3. **Stale prose** — `crates/skyc/stdlib/Sky/Core/Task.sky:6` still says
   *"always `SkyError = String` at the Rust level"*; update to describe the
   `5db4cd3` enum (`SkyError::Error(kind, info)`).

4. **Divergence ledger** — add a `docs/divergences-from-sky.md` entry:
   upstream declares `fail : e -> Task e a`
   (`../sky/sky-stdlib/Sky/Core/Task.sky:51`); Ipê pins
   `fail : Error -> Task Error a`. Rationale: the Rust runtime's task error
   channel is monomorphic (`SkyTask<A> = SkyTask<SkyError, A>` in every
   emitted wrapper), upstream's own house rules forbid `Task String a` in
   public surfaces, and the polymorphic reading was unimplementable — it
   produced ill-typed Rust, never a working program. `mapError`/`onError`
   already diverge identically (pinned `error_ty()`); this entry covers the
   family.

5. **No runtime change.** `runtime/src/sky_runtime/task.rs`'s generic
   `task_fail<E, A>` stays as-is; the prelude wrapper stays as-is.

### Regression coverage

Closes three currently-red E2E tests, which are the regression tests:
`golden_m5b_db.rs::db_transaction` (`crates/skyc/tests/golden_m5b_db.rs:175`),
`golden_m5a_task.rs::error_channel` (`:98`) and `::task_map_error_lambda`
(`:123`). Additionally add one **negative** check-only test (no `SKY_E2E`
needed): a program containing `Task.fail "oops"` must now fail skyc with
SKY-T0001 `expected Error, found String` — this pins the scheme so a future
"restore Elm-parity polymorphism" change can't silently reopen the
ill-typed-emission hole without confronting the recorded divergence.

---

## §3 — Landing order & verification

The two fixes are independent; either order. Suggested: §1 first (smaller,
fixture-dominated), §2 second (touches the scheme).

Per-fix verification:

```bash
CARGO_TARGET_DIR=… SKY_E2E=1 cargo nextest run -p skyc --test golden_m5b_db \
  -E 'test(db_crud) or test(db_transaction)'
CARGO_TARGET_DIR=… SKY_E2E=1 cargo nextest run -p skyc --test golden_m5a_task \
  -E 'test(error_channel) or test(task_map_error_lambda)'
cargo test -p sky_types -p sky_lower -p skyc          # non-E2E tiers
```

plus the standard sweep gate before merge. Neither fix touches `SqlValue` /
`SqlFragment` (#61), `DbExec`/`DbQuery` param typing (`6868f2c`), or any
Class-7 SQL-security item in
`class7-sql-db-fix-spec-2026-07-09.md` — no ordering constraint against
those lanes.
