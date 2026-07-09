# Hardening Batch B — spec/plan (serial, implemented by me)

5 tasks — same-file clusters or genuinely architectural/security-judgment fixes
that don't belong on an unattended mechanical lane. Implemented sequentially,
each ending in a green `cargo build`/`cargo test` before the next starts (the
pawl: never leave the tree red between tasks). Order below is priority order
(critical seal first).

---

## B1 — `crates/sky_lower/src/lower.rs`: AUD-01 + AUD-05

Both live in the same ~10k-line file; doing them as one pass avoids two
separate read-throughs of the same lowering machinery.

### B1a — AUD-01 (🔴 critical seal): per-occurrence `any` → concrete, not a shared generic

**Root cause:** `constrain.rs:1576-1583` gives every `any` occurrence in an
annotation a FRESH flex UV (comment: "Fresh flex UV per occurrence —
intentionally NOT inserted into vars"). `lower.rs:4331-4343` (+`3540-3553`)
lowers each param-position `any` to `IrType::Generic(v)` keyed by the SINGLE
interned `"any"` Symbol — so `f : any -> any -> Int` emits `fn
main_f<T1>(a:T1,b:T1)`, and a well-typed call `f "x" 3` fails cargo E0308.

**Fix (picked — resolve each param `any` from its SOLVED region type, mirroring
the existing return-position fix at `lower.rs:3483-3502` — reuse the
established pattern in this same file rather than inventing per-occurrence
alpha-renaming):**
1. Read `lower.rs:3483-3502` first (the return-position `any` substitution) to
   copy its exact mechanism: it consults the zonked env type for that binding
   position and substitutes the concrete resolved type when the IR type is
   `Generic(any_sym)`.
2. Apply the SAME substitution at each PARAM position where `ir_type_from_canon`
   currently emits `IrType::Generic(v)` for a bare `any` (`lower.rs:4331-4336`)
   — instead of unconditionally emitting `Generic(*v)`, check whether the
   param's SOLVED region type (per-occurrence, via the same lookup the
   return-position fix uses) resolves to something concrete; if so emit that
   concrete `IrType` instead.
3. Fail-closed when a param `any` cannot be resolved to a concrete type (emit a
   `Diagnostic::Lower` — do not silently fall back to `Generic`).
4. Update the stale doc comment at `crates/sky_ir/src/ir.rs:493-508`
   ("wildcard any [is] NOT representable here: rejected at lowering (M2c)") to
   match the new actual behavior.
5. Correctness note: this does NOT touch the union-ctor `any` payload path
   (already correctly pinned to `Dict String String` via `any_carrier_field_ir`
   / `pin_any_in_ty`) — leave that alone; this fix is annotation/param position
   only.

**Regression test:** `f : any -> any -> Int; f _ _ = 0` called as `f "x" 3`
(two DIFFERENT concrete types at the two `any` occurrences) — assert `cargo
build` succeeds on the emitted project (a `crates/skyc/tests/` E2E fixture, or
extend the existing Bug-28 regression `any_in_param_position_lowers_without_ice`
at `sky_lower/src/lib.rs:834-880` — READ that test first, it documents the
PRE-fix Generic-lowering behavior this fix changes; update its assertion to
check for the NEW per-occurrence-concrete shape instead of the shared generic).

### B1b — AUD-05 (🟠 seal): re-key `SolvedTypes::bounds` by `(home, name)`

**Root cause:** `SolvedTypes::env`/`regions` are keyed by `(home, name)`
because bare-name lookup is unsound cross-module — but `bounds` is keyed by
bare `Symbol` (`sky_types/src/lib.rs:82`: `pub bounds: BTreeMap<Symbol,
BTreeMap<Symbol, TyBounds>>`), populated with a plain `bounds.insert(*def_name,
var_bounds)` overwrite while iterating `(home, def_name)` pairs
(`lib.rs:306-323`) — home discarded. Two modules each declaring a same-named
generic fn with different obligations → the later-iterated wins; the OTHER
module's fn is emitted with the WRONG bound set (missing a needed trait bound
→ cargo E0369, or checked against the wrong obligation → false-accept). The
SAME bare key feeds `check_scheme_applications` (`lib.rs:374-395`,
`bounds.get(&app.name)`) via `SchemeApp` (`constrain.rs:915-921`, `name:
Symbol` — no home field), so the use-site soundness gate ALSO checks the wrong
def.

**Fix (picked — re-key `bounds` as `(Vec<Symbol>, Symbol)`, thread `home`
through `SchemeApp`, update BOTH consumer call sites):**
1. Change `SolvedTypes::bounds` field type from `BTreeMap<Symbol,
   BTreeMap<Symbol, TyBounds>>` to `BTreeMap<(Vec<Symbol>, Symbol),
   BTreeMap<Symbol, TyBounds>>` (`lib.rs:82`) — mirror whatever key shape
   `env`/`regions` already use for their `(home, name)` pairing (check their
   exact field types first: `rg -n "pub env:|pub regions:" sky_types/src/lib.rs`
   — match the SAME home representation, don't invent a new one).
2. Update the population site (`lib.rs:306-323`, iterating `(home, def_name)`)
   to `bounds.insert((home.clone(), *def_name), var_bounds)`.
3. Add a `home: Vec<Symbol>` field to `SchemeApp` (`constrain.rs:915-921`); at
   its construction site (`constrain.rs:2001` `self.scheme_apps.push(SchemeApp
   { name, vars, span })`) — check what `home` value is available in that
   scope (the constraining def's home module path) and thread it through.
4. Update `check_scheme_applications` (`lib.rs:374-381`) to look up
   `bounds.get(&(app.home.clone(), app.name))`.
5. Update `lower.rs:3548`'s `self.types.bounds.get(&name)` to
   `self.types.bounds.get(&(def.home_path(), name))` — first find how `home`
   is obtained for `def` at this point in `lower.rs` (grep for how the
   enclosing function already gets `def`'s home — it's used elsewhere in this
   same lowering pass for the `env`/`regions` lookups, which already do
   `(home, name)` correctly; copy that exact accessor).

**Regression test:** two modules `A.sky` (`scale : a -> a -> a; scale x y = x +
y`, obligation `Add`/Number) and `B.sky` (`scale : a -> a -> a; scale x _ = x`,
no numeric obligation) — a third module calling both `A.scale 1 2` and
`B.scale "x" "y"` — assert BOTH build clean with their OWN correct bounds
(the pre-fix bug would either miss `A.scale`'s Number bound on the emitted
generic, or wrongly reject `B.scale "x" "y"` against `A`'s obligation).

**Verify (both B1a+B1b together):** `cargo build -p sky_lower -p sky_types -p
skyc`, `cargo test -p sky_lower -p sky_types`, then the full workspace gate
(`cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D
warnings`) before moving to B2.

---

## B2 — `crates/sky_types/src/constrain.rs` + `runtime/src/sky_runtime/auth.rs`: AUD-02 + AUD-06 + AUD-13

Three findings across two files that all touch the Auth/JWT surface or the
same `constrain.rs` — doing them as one pass keeps the Auth scheme change and
its runtime wrapper change consistent.

### B2a — AUD-02 (🟠 security): JWT leeway + aud-validation, mirror `jwt.rs`'s existing fix

**Root cause:** `runtime/src/sky_runtime/auth.rs:184`
(`auth_verify_token`) creates `jsonwebtoken::Validation::new(HS256)` and only
sets `validate_exp`/`validate_nbf` — leaving the crate's default `leeway=60`
(accepts tokens up to 60s past `exp`) and default `validate_aud=true` with
`aud=None` (rejects any token merely CARRYING an `aud` claim, breaking
sign-then-verify of aud-bearing claims). The SIBLING file `jwt.rs:176-195`
already fixed both, with tests.

**Fix (picked — copy `jwt.rs`'s exact pattern, do not re-derive):**
1. Read `runtime/src/sky_runtime/jwt.rs:170-200` (the whole `Validation`
   construction block + its comments) as the template.
2. In `auth.rs`'s `auth_verify_token` (~line 184), add: `validation.leeway =
   0;`, `validation.reject_tokens_expiring_in_less_than = 1;` (guard the
   `exp==0` u64-underflow case the SAME way `jwt.rs` does — check `jwt.rs` for
   an `exp_is_zero`-style guard and mirror it if present), and
   `validation.validate_aud = false;`.
3. Decide `required_spec_claims` explicitly (don't rely on the crate default)
   — again matching whatever `jwt.rs` already settled on.

**Regression test:** mirror `jwt.rs`'s `test_hs256_expired_token_rejected`
boundary test for the `Auth` surface (a token with `exp` exactly `now`, and
`exp = now - 1s` under the OLD 60s-leeway default would wrongly accept — assert
REJECT post-fix). Second test: sign claims containing an `"aud"` key, then
verify — must be `Ok` (the current code rejects this; assert it now succeeds).

### B2b — AUD-06 (🟠 seal): pin `Auth.signToken`/`verifyToken` claims scheme to `dict(string,string)`

**Root cause:** `constrain.rs:4859` types `signToken`'s claims param (and
`verifyToken`'s return, `:4862`) as flexible `var(0)` — unifies with anything,
so skyc accepts a record, an Int, or any shape as claims. The generated
project's wrapper is pinned to `claims: HashMap<String, String>`
(`crates/sky_backend_rust/src/project.rs:256-263` `AUTH_WRAPPERS`, runtime sig
at `runtime/src/sky_runtime/auth.rs:120-124`) with NO coercion inserted at
lowering (`lower.rs:7873`, `8907-8908` route straight to the kernel) — any
non-`Dict String String` claims value passed by a well-typed Sky program fails
`cargo build`.

**Fix (picked — pin BOTH schemes concretely, per the project's own
concrete-over-generic rule; `var(0)` here is NOT genuine polymorphism, it's an
unpinned wildcard that should have one concrete lowering):**
1. In `constrain.rs`, change the `AuthSignToken`/`AuthVerifyToken` kernel
   scheme entries (~`:4859`/`:4862`) from `var(0)` to `dict(string(), string())`
   (use whatever the scheme-table DSL's helper for a `Dict String String` type
   is — check a neighboring kernel scheme that already types a `Dict String
   String` param for the exact helper call shape, e.g. `rg -n "dict(string"
   crates/sky_types/src/constrain.rs`).
2. Record the divergence from Go's polymorphic claims type `a` in
   `docs/divergences-from-sky.md` (append under whatever section already
   covers Auth/JWT divergences, or a new one — this is a genuine, intentional,
   documented behavior narrowing, not a bug).

**Regression test:** a `crates/skyc/tests/` E2E fixture compiling
`Auth.signToken secret { sub = "x" } 3600` end-to-end through `cargo build` —
before the fix this either silently builds wrong Rust or fails cargo
non-obviously; after the fix it should fail CLEANLY at `skyc` with a clear
type error (record literal vs `Dict String String`), OR — if the actual
call-site shape in real Sky code passes a `Dict.fromList [...]` literal — that
form should build clean end-to-end. Write the fixture using whichever shape
matches how `Std.Auth` documents `signToken`'s claims argument in CLAUDE.md
(check the `Std.Auth` stdlib doc/signature first — the fix must match the
DOCUMENTED claims shape, not an assumption).

### B2c — AUD-13 (🟠 parse-don't-validate): resolve wildcard-`any`-ness ONCE at the boundary

**Root cause:** `Ty::Var(u32)` carries TWO conflated id spaces: annotation
vars carry `Symbol::as_raw()` (`ty.rs:327`, `canon::Type::Var(s) => Ty::Var(s.
as_raw())`), while kernel schemes use bare ordinals (`var(0)`, `var(1)`,
`RowTail::Open(3)` — same file, e.g. `constrain.rs:3083`). `instantiate_in`
decides wildcard-ness by resolving `Symbol::from_raw(*id)` and STRING-COMPARING
to `"any"` — applied uniformly to BOTH spaces. A user program interning
`"any"` at a low raw id that happens to match a scheme ordinal misfires the
wildcard gate.

**Fix (picked — smaller footprint than splitting `Ty::Var` into two variants
everywhere: resolve wildcard-ness ONCE at the canon→Ty boundary into a
dedicated marker, so scheme ordinals structurally can never collide with it):**
1. Add a new `Ty` variant `AnyWildcard` (alongside the existing `Var(u32)`) in
   `crates/sky_types/src/ty.rs` — or, if a full new enum variant is too
   invasive for this pass (many exhaustive matches over `Ty` would need a new
   arm — check `rg -c "Ty::" crates/sky_types/src/*.rs crates/sky_lower/src/*.rs`
   for the blast radius before committing to this), the SMALLER alternative is:
   at the ONE conversion site `ty.rs:327` (`canon::Type::Var(s) => Ty::Var(s.
   as_raw())`), special-case `s` resolving to `"any"` and route it through a
   RESERVED raw-id sentinel that is provably outside the range kernel-scheme
   ordinals ever use (ordinals are small, e.g. 0..~10 per scheme — reserve
   `u32::MAX` or similarly, matching the existing `u32::MAX` fallback sentinel
   pattern already used elsewhere per the audit's own note on the
   HtmlStyleNode incident, `sky_kernels/src/lib.rs:2911-2917`).
2. Update `instantiate_in`'s wildcard check (`constrain.rs:1576-1584`) to
   check the sentinel/variant directly instead of resolving+string-comparing.
3. Decide between the two approaches (new `Ty` variant vs. reserved sentinel)
   by actually measuring the blast radius (step 1's `rg -c` count) BEFORE
   writing code — if `Ty::` match sites number in the dozens+, take the
   sentinel approach; if it's small and contained, the new variant is more
   principled (make-invalid-states-unrepresentable) and worth the larger diff.

**Regression test:** a fixture where a user-declared local intern order could
plausibly collide with a low kernel ordinal (this is inherently about intern
ORDER, which is hard to force deterministically from Sky source — the most
direct test is a `#[test]` in `sky_types` that directly constructs the
`instantiate_in` scenario: intern `"any"` at a controlled low raw id via the
test's own `Interner`, then instantiate a scheme using `var(0)` at that same
raw id, and assert the scheme's ordinal `var(0)` is NOT treated as the
wildcard).

**Verify (B2a+b+c together):** `cargo test -p sky_types -p sky_runtime` (check
runtime crate's actual package name in `runtime/Cargo.toml`), `cargo build -p
skyc`, full workspace gate before B3.

---

## B3 — `runtime/src/sky_runtime/db.rs`: AUD-03 (🟠 correctness, cross-tenant)

**Root cause:** `exec_routed`/`fetch_all_routed`/`fetch_optional_routed`/
`fetch_one_routed` (`db.rs:103-150`) match on `current_txn_conn()` and, when
`Some`, execute on the task-local `TXN_CONN` connection — the `pool` PARAMETER
is completely unused. `db_with_transaction`'s nesting gate (`db.rs:1567-1597`,
specifically `:1574`) checks ONLY `current_txn_conn().is_some()`, never pool
identity. `Db = DbPool` (`db.rs:9`) carries no identity; `connect_cached`
(`db.rs:617`) keys pools by URL, so two `Db.open` calls to different URLs
yield genuinely distinct pools. A well-typed program mixing two `Db` handles
inside one `withTransaction` silently executes the wrong handle's ops on the
FIRST handle's transaction connection — wrong-database read/write, cross-tenant
mixing in per-tenant-DB architectures.

**Fix (picked — tag each pool with an identity, thread it through the
task-local, check on every consult):**
1. Add a monotonic pool-id counter (an `AtomicU64` or similar) and a
   `pool_id: u64` field to whatever wraps `Db`/`DbPool` at the point pools are
   created (`build_pool`/`connect_cached`, `db.rs:595-663`) — if `Db` is a bare
   type alias (`pub type Db = DbPool;`) rather than a struct, this likely needs
   `Db` to become a small newtype `struct DbHandle { pool: DbPool, id: u64 }`
   (check every call site of `Db`/`DbPool` across the crate first — `rg -c
   "\bDb\b|\bDbPool\b" runtime/src/sky_runtime/*.rs` — to gauge the blast
   radius of this type change before committing to it).
2. Extend the `TXN_CONN` task-local (`db.rs:84-87`, currently `Option<TxnConn>`)
   to also carry the owning pool's id (`Option<(u64, TxnConn)>` or a small
   struct).
3. In each `*_routed` helper (`db.rs:103-150`), change the `Some(conn) => ...`
   arm to first check the stored id against `pool`'s id; only use the
   task-local connection on a MATCH, else fall through to `query.execute(pool)`
   as if no transaction were active (mirroring the existing `None` arm).
4. In `db_with_transaction`'s nesting gate (`:1574`), apply the same id check
   before deciding to short-circuit onto the outer transaction — a DIFFERENT
   pool's nested `withTransaction` must open its OWN transaction, not flatten
   onto the outer one.

**Regression test:** two SQLite file pools `dbA`/`dbB` (distinct temp files);
`Db.withTransaction dbA (\_ -> Db.exec dbB "INSERT ...")` (or the Rust-level
equivalent, calling the runtime fns directly in a `#[tokio::test]`) — assert
the INSERT lands in `dbB`'s file, not `dbA`'s (query `dbB` directly after, and
confirm `dbA` does NOT contain the row). Also a nested-transaction test: `dbA`
outer transaction, `dbB` nested `withTransaction` inside it — assert `dbB`'s
transaction commits/rolls back independently of `dbA`'s outcome.

**Verify:** `cargo test -p sky_runtime` (crate name TBD, check
`runtime/Cargo.toml`), `cargo build -p skyc`, full workspace gate before B4.

---

## B4 — `crates/sky_backend_rust/src/emit_expr.rs`: AUD-04 (🟠 seal, deepest fix in this batch)

**Root cause:** `clone_captured_vars` (TaskSeq clone-capture, `emit_expr.rs:4849`,
mechanism `:238-283`) and the let-inlining pass (`replace_word_all`,
`emit_expr.rs:4659-4664`, mechanism `:39-97`) both do TEXTUAL rewrites over
ALREADY-EMITTED Rust source text with no string-literal-state tracking —
`add_clone_to_bare_ident` only checks the byte IMMEDIATELY preceding an
identifier occurrence, so a captured-variable word appearing mid string
literal, or matching a record FIELD name, gets corrupted (`"the count is"` →
`"the count.clone() is"`; `RecCount { count: n }` → `RecCount { count.clone():
n }` — the latter is invalid Rust, a seal breach). `TaskSeqSync`
(`:4860-4865`) has NO clone-capture handling at all (use-after-move E0382 on
any shared non-Copy binding). A multi-use `let` of a directly Task-typed value
(not wrapped in `Expr::List`) also falls through to the plain shared-`let`
path (`:4645-4667`, `expr_value_is_non_clone` only covers `Expr::List`).

**Fix (picked — kill the textual passes; move both analyses to the IR level,
where `Expr::CloneVar` ALREADY EXISTS and is ALREADY EMITTED
(`crates/sky_ir/src/ir.rs:994`, `sky_backend_rust/src/emit_expr.rs:4609` — this
is "wire an existing IR node earlier in the pipeline", not "invent new IR"):**
1. **Clone-capture (TaskSeq + TaskSeqSync unified):** before emission, compute
   the free-variable SET of `rest` on the typed IR `Expr` (not the emitted
   string) — write a small recursive free-vars collector over `Expr` (check
   whether one already exists elsewhere in `sky_ir`/`sky_lower` first — `rg -n
   "fn free_vars|fn collect_free" crates/sky_ir/src/ crates/sky_lower/src/`
   before writing a new one). For each binding in that set that is ALSO used
   inside `effect` and is non-`Copy`, rewrite the `Expr::Var(sym)` occurrence
   inside `effect`'s IR to `Expr::CloneVar(sym)` BEFORE calling
   `emit_expr_inner` on it (a pass over the IR tree, not the string). Apply
   this ONE analysis to BOTH `Expr::TaskSeq` and `Expr::TaskSeqSync` — the
   audit notes they need the identical fix; do it once, call it from both
   emission arms.
2. **Let-inlining:** replace `replace_word_all(&body_s, &name_s, ...)` (text
   substitution) with an IR-level Symbol-keyed substitution: walk the `body`
   IR `Expr` tree and replace every `Expr::Var(name_sym)` node with (a clone
   of) the `value` `Expr` — exact AST substitution, no word-boundary
   heuristics, cannot touch a string literal because it operates on the typed
   tree where a string literal is an opaque leaf node, never text to
   pattern-match against.
3. **Multi-use Task-typed `let` (the narrower `expr_value_is_non_clone` gap):**
   decide by the binding's IR TYPE, not its expression constructor — extend
   the check from `matches!(expr, Expr::List { elem, .. } if
   ir_type_contains_task(elem))` to also cover the case where
   `ir_type_contains_task(type_of(value))` directly (Tuple/Record/Fun-return
   included) — if the binding's type contains a Task anywhere and the body
   uses the name more than once, apply the SAME per-use inline (step 2) or a
   thunk (`let name = || <value>; name()` at each use site) rather than the
   plain shared `let`.
4. This is the most invasive task in the batch — budget real time for it, and
   do NOT rush a partial fix; if the IR-level free-vars collector or the
   Symbol-substitution walk turns out to need touching many `Expr` variant
   arms (exhaustive match), that's expected and correct (make-invalid-states-
   unrepresentable — every `Expr` shape must be walked, no `_ =>` catchall).

**Regression test:** the three witnesses from the audit, each as a
`crates/skyc/tests/` E2E fixture asserting BOTH (a) `cargo build` succeeds and
(b) the RUNTIME OUTPUT is correct (not just compiles):
- `let count = 3; _ = println "the count is" in ...count...` → assert the
  printed output is exactly `"the count is"` (unmutated) and `count` is usable
  afterward.
- A record literal whose field name collides with a captured var name inside
  an effect (`RecCount { count = n }` shape) → assert clean compile + correct
  field value.
- `let tasks = [Task.succeed "a"] in Log.info "run tasks" ...tasks...tasks` →
  assert clean compile (no cargo fail from a corrupted string literal) + the
  logged string is exactly `"run tasks"`.
- `let _ = Io.writeStdout msg in msg` (TaskSeqSync use-after-move witness) →
  assert clean compile + `msg` is usable after the effect.

**Verify:** `cargo test -p sky_backend_rust -p skyc`, full workspace gate
before B5.

---

## B5 — `tools/sky-ffi-inspect-rs/src/main.rs`: AUD-10 (🟠 security, SCOPED interim mitigation only)

**Explicitly OUT of scope for this task:** full sandboxing (landlock/seccomp,
rootless container). That remains the tracked, larger, multi-session item per
memory `ffi-subsystem`/`security-hardening-before-push` and backlog Tier-2
FFI ("RCE-sandbox is blocking gate; blocks on M4 registry") — FFI is
explicitly not on the critical path right now. This task ships ONLY the
audit's own cheap interim mitigation: a build-script denylist + an explicit
opt-in flag, so the RCE surface requires deliberate operator consent rather
than firing silently on any `inspect-crate <name>` invocation.

**Root cause:** `inspect_crate` (`main.rs:1161`, fetch at `:1286-1298`) takes
an arbitrary crates.io name or `--git` URL and runs `cargo fetch` + `cargo
+nightly rustdoc`. rustdoc runs AFTER macro expansion, so the target crate's
(and every transitive dep's) `build.rs`/proc-macros execute with full user
privileges — arbitrary code execution from the exact untrusted input this tool
consumes.

**Fix (picked — the audit's own named interim mitigation, nothing more):**
1. After `cargo fetch` (`:1286-1298`) but BEFORE the `cargo +nightly rustdoc`
   call, run `cargo metadata --offline` (using the already-fetched deps) and
   walk the returned package graph (transitive) for any package with a `build
   = "..."` entry in its manifest (a build script) OR that is itself a
   `proc-macro = true` crate.
2. If any are found AND the caller did not pass a new `--allow-build-scripts`
   CLI flag (add it to the `clap`/arg-parsing struct near wherever
   `inspect_crate`'s other flags are defined), REFUSE with a loud, specific
   error naming EVERY offending package + whether it's a build-script or
   proc-macro, and the exact flag to opt in.
3. When `--allow-build-scripts` IS passed, proceed as today (still print a
   loud provenance warning naming the packages before running rustdoc — this
   is a consent gate, not a technical sandbox; the warning matters).

**Regression test:** an `inspect_crate` test (or a smaller unit test around
the new denylist-check function in isolation, if the full `cargo fetch`
round-trip is too slow/network-dependent for the test suite) — feed it a
`cargo metadata` JSON fixture (captured once from a real crate with a known
`build.rs`, e.g. `openssl-sys` or similar, stored as a test fixture file) and
assert: (a) without the flag, the check returns/errors naming the offending
package; (b) with the flag, it proceeds; (c) a metadata fixture with NO
build-script/proc-macro packages proceeds either way (no false-positive
blocking of safe crates).

**Verify:** `cargo test -p sky-ffi-inspect-rs` (or whatever this tool's crate
name is — check `tools/sky-ffi-inspect-rs/Cargo.toml`), `cargo build`. This is
the last Batch B task — run the FULL workspace gate afterward
(`cargo test --workspace && cargo clippy --workspace --all-targets -- -D
warnings`) as the final confirmation before declaring the batch done.

---

## Cross-batch note

If Batch A (parallel) is still running or has landed by the time B1-B5 start,
`git pull`/`git log` first to confirm no file collision — B1/B2 touch
`sky_types`/`sky_lower`, Batch A's A3 (AUD-12) ALSO touches
`crates/sky_types/src/lib.rs` (a different function, the numeric-defaulting
arm vs the `bounds` field) — low collision risk but re-`git status` before
starting B1 to confirm the tree is clean and A3 already landed+merged.
