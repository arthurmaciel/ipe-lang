# Sweep-red root-cause: #217 (36-composite-server IPE-T0001) + #218 (18-job-queue SEAL breach E0507)

Read-only diagnosis, 2026-07-16. No code/example/fixture edited; all bisection on
`/tmp` copies. Reference behaviour verified against `../sky` (Haskell compiler +
Rust backend + Go runtime) per DEVELOPMENT.md §0a.

---

## #217 — 36-composite-server: IPE-T0001 — VERDICT: not an HM-checker bug; three stdlib contracts drifted from the reference

### (a) Repro

```
( cd examples/36-composite-server && "$SKYC_BIN" build sky.toml --out sky-out/rust )
```

```
skyc: error[IPE-T0001]: type mismatch
  --> src/Auth.sky:47:42
   |
47 |                 |> Jwt.withClaim "email" (JsonEnc.string payload.email)
   |                                          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected String, found Value
```

### (b) Minimal trigger (1 expr)

```elm
Jwt.claims |> Jwt.withClaim "email" (JsonEnc.string "a@b.c")
-- IPE-T0001: expected String, found Value
```

### (c) Root cause — kernel type-scheme contract drift, not inference

The HM checker (unify/solve) is doing its job correctly; the **scheme it
enforces is wrong**. Bisection of a `/tmp` copy (patch each blocker, re-run)
shows the example trips **three distinct stdlib symbols whose contracts diverge
from the reference `../sky/sky-stdlib`**, all one generative class:
*kernel-backed stdlib surface authored in the scheme table instead of ported
from the reference module's Sky-source signature.*

| # | Symbol | Ours (drifted) | Reference (`../sky/sky-stdlib`) | Sites in example |
|---|---|---|---|---|
| 1 | `Jwt.withClaim` | `String -> String -> Claims -> Claims` — `crates/sky_types/src/constrain.rs:4878` (scheme), `crates/sky_kernels/src/lib.rs:470,1853` (decl), `runtime/src/sky_runtime/jwt.rs:416` (`sky_jwt_with_claim(key: String, value: String, …)`) | `withClaim : String -> JsonEnc.Value -> Claims -> Claims` — `Sky/Core/Jwt.sky:79` (pure Sky, NO kernel upstream; every typed helper `issuer`/`expiresAt`/… is built ON it via `JsonEnc.string`/`JsonEnc.int`) | `src/Auth.sky:47` |
| 2 | `Sky.Http.Server.Response` | opaque nominal — `constrain.rs:267,590` (`server_response`, "the opaque server response type") | `type alias Response = { status : Int, body : String, headers : Dict String String, contentType : String }` — `Sky/Http/Server.sky:66` (record alias; the example's comment at `Routes/Todos.sky:226-229` documents that upstream *deliberately* builds it as a record literal) | `src/Server.sky:128`, `src/Routes/Todos.sky:232`, `src/Routes/Health.sky:154` |
| 3 | `Std.Db.Migration` + `Db.migrate` | `Migration` alias not registered at all; `Db.migrate : Db -> List (String, String) -> Task (List String)` — `constrain.rs:4376-4379` (`K::DbMigrate`) | `type alias Migration = { name : String, sql : String }` + `defaultMigration` — `Std/Db.sky:237,246`; `migrate` takes `List Migration` | `src/Migrations.sky` (4 bindings + `Main.sky` call) |

Generative reason: the D-00/#152 Jwt builder (and the Server/Db type surfaces)
were **authored, not ported** — the value-arg type was guessed as `String`, and
record aliases were registered as opaque nominals. Precedent that this class
recurs: divergence-ledger entry **B-JwtDecode** (`docs/divergences-from-sky.md:1215`)
already converged the *same family's* `Jwt.decode` after the identical
authored-not-ported drift. Ledger entry B9 (`:163`) claims the builder API
"shipped ✅" but never records the `String`-vs-`Value` narrowing — an
**unsanctioned** divergence under the sanctioned-divergence policy
(PRINCIPLES.md "Match the reference": a hack is never a divergence).

Confirm-or-refute the brief's hypothesis: **REFUTED as a checker gap, CONFIRMED
as a compiler defect** — the verbatim-ported program is well-typed under the
reference stdlib; our scheme table rejects it. (Correctness + Completeness.)

Note the drift also LOSES expressiveness: an `Int`/`Bool`/nested-object custom
claim is inexpressible through our `String`-typed `withClaim`; the runtime
stores the value as a JSON *string*, so even the String case produces different
token bytes than upstream for non-string claims — a silent correctness hazard,
not just a rejection.

### Residual red (honest report, distinct blocker)

After patching all 5 drift sites in the `/tmp` copy, the example STILL fails —
`IPE-L0126` (non-Clone capture, fail-closed) at `src/Main.sky:73`
(`\db -> Db.migrate db Migrations.all |> Task.andThen …`). That is a separate
lowering-completeness gap in the same shared-capture machinery as #218 (fails
closed here instead of mis-emitting — no SEAL breach). File it as its own
backlog item; 36-composite-server needs BOTH fixed to go green.

### (d) Structural fix + invariant

1. **Converge the three contracts to the reference** (anti-drift sites per
   DEVELOPMENT.md §0b, all in lockstep):
   - `K::JwtWithClaim` scheme → `fun(string(), fun(json_value, fun(claims_ty(), claims_ty())))`;
     runtime `sky_jwt_with_claim(key: String, value: JsonValue, claims: JsonValue)`
     inserts the value **directly** (runtime `Claims` and `JsonEnc.Value` are both
     `serde_json::Value` — mechanical); kernel doc `lib.rs:470`.
   - `Response` → expand structurally as `Ty::Record` with the 4 reference
     fields. **In-tree precedent exists**: `HttpResponse`/`HttpRequest` are
     already expanded as closed `Ty::Record` aliases at `constrain.rs:2361-2375`
     and `constrain.rs:3713-3720` — reuse that exact mechanism. Audit the
     `Server.text/json/html/withStatus/…` kernel schemes to return/accept the
     record type.
   - Register `Migration = { name : String, sql : String }` the same way;
     `K::DbMigrate` → `db -> List Migration -> Task (List String)`; add
     `defaultMigration`; adapt `db_migrate_apply` runtime arg shape.
2. **Close the class** (fix-the-structure): a stdlib-contract parity gate — a
   test that walks every kernel-backed module mirroring a reference
   `sky-stdlib` module and asserts our scheme renders identical to the
   reference `.sky` signature (skydex indexes the reference; `ipe-index parity`
   already reconciles kernel routes). B-JwtDecode + this bug are two instances
   of the same class; without the gate a third is guaranteed.

**Invariant established:** for every surface shared with the reference, *the
reference Sky-source signature IS the contract* — a scheme that drifts fails a
gate at compiler-test time, not a user program at type-check time. Record
aliases stay structural records (make-invalid-states-unrepresentable: an opaque
nominal for a documented record alias makes valid programs unrepresentable).
No SEAL impact: type-level widening + runtime adapted in lockstep; scheme and
kernel signature move together (the anti-drift sites are type-checker-enforced).

### (e) Affected files

- `crates/sky_types/src/constrain.rs` — `:4878` (JwtWithClaim), `:267,590`
  (Response opaque), `:4376` (DbMigrate); Response/Migration record expansion
  next to the HttpResponse precedent (`:2361`, `:3713`).
- `crates/sky_kernels/src/lib.rs:470,1853` (doc/decl); `crates/sky_canon/src/env.rs:838-860`
  (Jwt name routing; + `Migration`/`defaultMigration` registration).
- `runtime/src/sky_runtime/jwt.rs:416`; `runtime` `db` migrate kernel
  (`db_migrate_apply`).
- `docs/divergences-from-sky.md` — correct B9; record the convergence.

### (f) Regression tests

- Golden porting the reference's canonical builder use:
  `Jwt.withClaim "email" (JsonEnc.string e) |> Jwt.withClaim "n" (JsonEnc.int 1)`
  → encode → decode round-trip, byte-compared vs Go oracle (extends
  `golden_m5b_uuid_jwt.rs`).
- Golden building `Response` and `Migration` as record literals (the exact
  upstream idiom), `IPE_E2E=1`.
- The stdlib-contract parity gate above (class-closer).
- Sweep row 36-composite-server goes green only after the residual IPE-L0126 is
  also fixed — keep the row red until then (no shortcut).

**CONFIDENCE: HIGH** on the root cause of every IPE-T0001 (all three symbols
verified against reference source; minimal 1-expr trigger; bisect exhausted the
class). Residual IPE-L0126 is reported, not root-caused (separate item).

---

## #218 — 18-job-queue: SEAL breach (skyc exit-0, cargo E0507) — VERDICT: `wrap_shared_lambda_if_needed` breaks the clone relay at intermediate closure boundaries

### (a) Repro

```
( cd examples/18-job-queue && "$SKYC_BIN" build sky.toml --out sky-out/rust )   # exit 0
CARGO_TARGET_DIR=~/.cache/ipe/diag-217-218-target cargo build \
  --manifest-path examples/18-job-queue/sky-out/rust/Cargo.toml
```

```
error[E0507]: cannot move out of `insertRow`, a captured variable in an `Fn` closure
   --> src/main.rs:424:682
error[E0507]: cannot move out of `selectRecent`, a captured variable in an `Fn` closure
   --> src/main.rs:427:790
```

Source: `examples/18-job-queue/src/Main.sky:148-166` (`saveSnapshot`:
`insertRow db ts = Db.exec …` / `writeAll db = … |> Task.andThen (\_ -> Time.now ())
|> Task.andThen (\ts -> insertRow db ts)`) and the mirror `loadHistory:172-184`.

### (b) Minimal trigger (18 lines, reproduced: skyc=0, cargo=101 E0507 on `insertRow`)

```elm
save : Int -> Task Error Int
save n =
    let
        insertRow x ts = Task.succeed (String.length x + n + ts)
        writeAll x =
            Task.succeed x
                |> Task.andThen (\_ -> Time.unixMillis ())
                |> Task.andThen (\ts -> insertRow x ts)
    in
        Task.succeed "seed" |> Task.andThen writeAll
```

Three required ingredients (each verified by ablation):
1. a `let`-bound function (`insertRow`) read only at lambda-nesting depth ≥ 2 —
   `needs_shared_capture` (`lower.rs:1935`) fires, the binding is promoted to
   `SharedLambda` → emitted `Arc<dyn Fn(…)>` (the #164 machinery, correctly);
2. a pipeline stage `|> Task.andThen (\ts -> insertRow x ts)` whose callback
   also captures a **non-Copy** enclosing-lambda param (`x : String`; in the
   example, `db : Db`). The multiuse pre-clone wrap `{ let x = x.clone(); … }`
   around the partial callee defeats the direct-call collapse, so
   `eta_expand_partial` (`lower.rs:10437`) synthesizes a REAL intermediate
   closure `move |eta_0| task_and_then(eta_0, k)`. (With `x : Int`/CopyLeaf the
   pipe collapses to `let eta_0 = …; task_and_then(eta_0, k)` — no intermediate
   boundary, compiles clean; confirmed.)
3. the enclosing `writeAll` is itself a `Box<dyn Fn>` closure (re-callable).

### (c) Root cause — exact site

`crates/sky_lower/src/lower.rs:3012` (and the `SharedLambda` twin at `:3022`),
in `wrap_shared_lambda_if_needed` (`:3008`), called from
`force_shared_capture_clones`'s Lambda arms (`:2767-2771`), invoked from
`lower_let_pvar` (`:14480`) once `needs_shared_capture` has promoted the binding:

```rust
let needs_wrap = !shadowed && sym_referenced_directly(sym, &body);   // ← PRE-recursion body
let body = if shadowed { body } else { Box::new(force_shared_capture_clones(sym, *body)) };
```

Two conjoined defects, one generative reason:

- `sym_referenced_directly` (`:2649`) is **lambda-opaque** — it returns `false`
  for a body that reaches `sym` only through a deeper nested lambda, on the
  documented theory (":2645-2648") that the inner lambda "gets its own wrap".
- `needs_wrap` is evaluated on the body **before** the recursive rewrite runs.

The theory is a broken induction. The inner lambda's wrap is
`Let { name: sym, value: CloneVar(sym), body: inner_lambda }` — placed in the
**intermediate** closure's body. That pre-clone's read (`sym.clone()`) makes the
intermediate `move` closure capture `sym` **by value**; constructing it inside
the enclosing `Fn` closure (`writeAll`) moves `sym` out of an env that
`Fn::call` only `&self`-borrows → E0507. The wrap that would fix it (a
pre-clone directly outside the intermediate closure, exactly what rustc's
`help:` suggests) is never emitted because the intermediate lambda's
`needs_wrap` decision was taken on the pre-wrap body where the reference was
still hidden behind the inner lambda boundary.

Emitted evidence (minimal trigger and `main.rs:424` alike): `x` (param,
CloneOk, handled by the separate `rewrite_multiuse_clones` per-boundary relay,
`:3901`) gets `{ let x = x.clone(); … }` at BOTH boundaries; `insertRow`
(function-typed, handled by the #164 shared-capture pass) gets its pre-clone
only at the innermost boundary.

Why goldens missed it: the `golden_i193_*` suite covers nested captures where
the intermediate closure references the symbol **directly**; the
pipeline-synthesized `move |eta_0|` wrapper is precisely an intermediate
closure that NEVER references the symbol directly.

### Reference behaviour (`../sky`, per user directive)

`../sky/src/Sky/Generate/Rust/Builder/ExprEmitter.hs` cannot exhibit this class,
by construction:

- `argToRustString` Lambda arm (`:794-813`): for EVERY lambda in argument
  position, `captured = collectVarLocals body − paramNames` is computed with a
  **lambda-transparent** walker, and `clonePreludeFor` (`:764`) emits
  `let v = v.clone();` for every captured var **immediately before the closure
  literal, in the enclosing scope** — then adds every captured var to
  `ecCloneVars` for the body (":804-807": "For move closures, EVERY captured
  non-Copy variable used inside needs to be cloned"). Same pattern at the
  general lambda site (`:1449-1458`, `outerInherited` union) and at the
  partial-application closure (`:2108`, `:2144-2160` — `{ clonePrelude ++
  theClosure }`).
- I.e. the reference relays the clone across **every** closure boundary,
  unconditionally (over-cloning; ADR-0002 records our leaner last-use
  divergence as sanctioned). Our #164 pass adopted the lean discipline but
  dropped the relay at boundaries that don't read the symbol directly.
- Upstream `Jwt` (for #217) has no kernels at all: `withClaim` is pure Sky
  (`skydex locate withClaim` → `sky-stdlib/Sky/Core/Jwt.sky` binding only);
  the ordinary HM checker types it from source — nothing to drift.

### (d) Structural fix + invariant

In `wrap_shared_lambda_if_needed` (`lower.rs:3008`): recurse FIRST, then decide
on the **processed** body —

```rust
let body = Box::new(force_shared_capture_clones(sym, *body));   // children first
let needs_wrap = !shadowed && sym_referenced_directly(sym, &body); // post-recursion
```

(one reorder, both the `Lambda` and `SharedLambda` arms). The induction then
closes itself: an inner lambda's wrap plants a direct `CloneVar(sym)` read in
its parent lambda's body, so the parent's post-recursion check sees it and
wraps too — the pre-clone **relays outward through every boundary** until it
reaches sym's home scope. This is the reference's per-boundary clone-prelude
discipline, but emitted only along the path that actually needs it (lean where
the reference is blanket — consistent with the sanctioned ADR-0002 divergence).

**Invariant established (tie to SEAL/ADR-0002/ADR-0007):** after
`force_shared_capture_clones(sym, e)`, no `Lambda`/`SharedLambda` in `e`
captures the *outer* `sym` — every move-closure boundary crossed between sym's
binding and any read captures a fresh `Arc::clone` minted in its
immediately-enclosing scope. `Clone`-availability is guaranteed by the #87/#93
derive-seal + the `SharedLambda`/`Arc` promotion that gates entry to this pass;
`reject_fn_value_reuse` (`:3857`) stays the fail-closed gate for non-Clone
values. Cost: one extra `Arc::clone` per intermediate boundary (pointer bump) —
Efficiency yields to Soundness/SEAL per the principle order. The `:2645-2648`
doc comment's "its own wrap … not this one's concern" claim must be corrected
in the same change (Readability: the comment currently documents the bug).

NOT the fix: swapping `sym_referenced_directly` for the lambda-transparent
`lambda_body_refs_sym` in the pre-recursion check would also close the hole but
wraps every ancestor lambda of every read unconditionally — the post-recursion
relay is both leaner and self-proving (each wrap exists because a direct read
exists). Weakening `needs_shared_capture`, or special-casing eta closures,
would be symptom patches (the same breach recurs via any other synthesized or
source-level intermediate closure).

### (e) Affected files

- `crates/sky_lower/src/lower.rs` — `wrap_shared_lambda_if_needed`
  (`:3008-3055`; the two `needs_wrap` lines `:3012`,`:3022`), doc comment
  `:2640-2648`/`:2719-2755`.
- No backend/runtime change; no scheme change. Goldens that pin current #164
  output (`golden_i193_*`) re-verify — intermediate-boundary wraps are additive.

### (f) Regression tests

- New golden (i193 family, e.g. `golden_i193_clone_relay_intermediate_eta`):
  the 18-line trigger verbatim — let-bound fn read at depth ≥ 2 through a
  pipeline-synthesized eta closure with a non-Copy enclosing param — byte
  golden + `IPE_E2E=1` (THE SEAL: skyc-0 ⇒ cargo-0).
- A sibling variant with a SOURCE-level intermediate lambda (not
  eta-synthesized) that doesn't directly reference the symbol, pinning the
  class not the instance.
- Sweep row: 18-job-queue back to green (build+run+Go-equivalence).

**CONFIDENCE: HIGH** — mechanism traced end-to-end (promotion → wrap decision →
emitted bytes), minimal trigger reproduces the exact example failure shape,
ablation isolates each ingredient, rustc's own suggested fix is the missing
wrap, and the reference's per-boundary prelude confirms the invariant.

---

## Class kinship (one paragraph)

#217 and #218 are both drift-from-invariant bugs: #217 drifts the *contract
table* from the reference stdlib (checker rejects a valid program — fails
closed, Correctness/Completeness); #218 drifts the *clone-relay invariant*
inside one pass (emitter accepts and mis-emits — fails open, SEAL breach,
Soundness). Per PRINCIPLES order #218 outranks #217 in urgency. The residual
IPE-L0126 in 36-composite-server (`Main.sky:73`) lives in the same
shared-capture machinery as #218 and should be filed + investigated when the
relay fix lands (it may simply be the fail-closed face of the same gap).
