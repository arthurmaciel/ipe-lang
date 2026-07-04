# Seal hole #89 — JsonDecP pipeline: skyc exit-0 → cargo-fail

> **Status:** design (Design Lane, read-only on crates; no build run, no crate
> edits). **Seal-touching** (`skyc` exit-0 MUST imply `cargo` exit-0) →
> **adversarial review before commit** (see §9), same protocol as
> `seal-noncopy-move-design.md` / `seal-gates-msg-lambda-view-design.md`.
>
> **Verdict up front (one fix or per-class?):** **two substantive fixes plus a
> one-line kernel-name fix** — NOT one per rustc error code:
>
> | Fix | Closes | Where |
> |---|---|---|
> | **A. `succeed`-argument emission** (curry lambdas, factory-wrap plain values) | E0593 **and** E0308 (one root cause, two surface codes) | `sky_backend_rust/src/emit_expr.rs:2722-2733` |
> | **B. `DbDecSucceed` name mapping** (discovered during this investigation) | E0425 (`db_decode_succeed` does not exist in the runtime) | `sky_backend_rust/src/naming.rs:647` |
> | **C. Decoder-typed `let` thunking** (rebuild per use — `Decoder` is move-only and `!Clone`) | E0382 | `sky_lower/src/lower.rs` (IR rewrite; zero backend changes) |
>
> The runtime (`runtime/src/sky_runtime/json.rs`) is **correct and untouched**
> — it is byte-equivalent to the upstream reference runtime that the Haskell
> Rust backend compiles against (verified by diff, §6.1). Every fix is
> compiler-side.
>
> **Scope caveat:** the shapes actually used by `examples/06-json` are mostly
> blocked *earlier*, at skyc, by three *non-seal* gaps (§8) — `|>`-pipe partial
> application (SKY-L0102), `succeed RecordCtor` (SKY-N0001), top-level decoder
> bindings (SKY-L0102). Fixing #89 alone does not turn 06-json green; §8 maps
> the full gate.

---

## 0. The hole in one paragraph

`JsonDec.succeed f |> JsonDecP.required "name" JsonDec.string |> …` builds a
record decoder by threading a **curried constructor** through pipeline steps.
The runtime models that accumulator as
`Decoder<E, Box<dyn FnOnce(T) -> F + Send>>` (a fresh single-shot chain per
`run`, produced by a `curry{N}` **factory**). The emit layer only produces that
shape when `succeed`'s argument is a **named top-level function**
(`decode_succeed(curry3(main_make_label))`). For every other argument shape —
a **lambda**, a **plain value** — it falls through to generic emission, which
produces a raw `Box::new(move |a, b| …)` / bare value where the runtime needs
the `Box<dyn Fn() -> A + Send>` factory. skyc exits 0; cargo rejects the
emitted Rust. Independently, a **`let`-bound decoder used twice** moves at the
first `decode_from_json_string(d, …)` call (by-value parameter, `Decoder` is
`!Clone`) and E0382s at the second.

---

## 1. Ground truth from the crates (file:line — all verified in-repo)

### 1.1 The runtime contract (`runtime/src/sky_runtime/json.rs`)

- **`Decoder` is move-only, `!Clone`, `!Copy`** — `json.rs:21-24`:
  ```rust
  pub struct Decoder<E, T> {
      pub run: Box<dyn Fn(&JsonVal) -> SkyResult<E, T> + Send>,
      pub fields: Vec<String>,
  }
  ```
  `Box<dyn Fn>` has no `Clone`; there is no `impl Clone for Decoder` (verified
  by `rg` over both this runtime and the upstream one).
- **`decode_succeed` takes a FACTORY**, not a value — `json.rs:740-744`:
  ```rust
  pub fn decode_succeed<E: From<String> + 'static, A: 'static + Send>(
      factory: Box<dyn Fn() -> A + Send>,
  ) -> Decoder<E, A>
  ```
  The doc comment (`json.rs:723-739`) states the contract explicitly: *"The
  generated Rust code always calls this as `decode_succeed(curryN(ctor))`"* —
  the factory produces a **fresh `FnOnce` chain per `run` invocation** (one per
  decoded value / DB row).
- **`curry1`..`curry10`** — `json.rs:799-1210`. Each returns
  `Box<dyn Fn() -> Box<dyn FnOnce(A1) -> … -> R + Send> + Send>` and requires
  `F: Fn(A1,…) -> R + Clone + Send + 'static` (rationale comment
  `json.rs:780-798`: fn pointers and non-capturing closures are `Copy ⊆ Clone`).
  **There is no `curry11`+** — arity > 10 has no runtime helper.
- **Pipeline steps are tupled 3/4-arg calls** whose accumulator parameter is
  the curried FnOnce chain — `decode_pipeline_required`, `json.rs:1320-1345`:
  ```rust
  pub fn decode_pipeline_required<E: …, T: 'static, F: 'static>(
      name: String,
      decoder: Decoder<E, T>,
      next_decoder: Decoder<E, Box<dyn FnOnce(T) -> F + Send>>,
  ) -> Decoder<E, F>
  ```
  (`optional`: `json.rs:1347-1385` — extra `default: T` with `T: Clone`;
  `required_at`: `json.rs:1390-1432`; `custom`: `json.rs:1433-1460`.)
  The uncurried convention is documented at `json.rs:1310-1319` and pinned by
  the upstream contract test
  `../sky/runtime-rust/tests/calling_convention.rs:1-18`.
- **`decode_from_json_string` consumes the decoder BY VALUE** —
  `json.rs:770-774`:
  ```rust
  pub fn decode_from_json_string<E: From<String> + 'static, T>(
      decoder: Decoder<E, T>,
      json: String,
  ) -> SkyResult<E, T>
  ```
- **`decode_list` takes a factory** `impl Fn() -> Decoder<E, T> + Send`
  (`json.rs:486-488`) — relevant to the thunk design (§5.C).

### 1.2 The emit layer (`crates/sky_backend_rust/src`)

- **The `succeed` special case matches ONLY `Expr::FuncValue`** —
  `emit_expr.rs:2722-2733`:
  ```rust
  if matches!(callee, Callee::Kernel(sky_ir::KernelFn::JsonDecSucceed))
      && let Some(Expr::FuncValue { callee: fn_callee, ty: IrType::Fun(params, _) }) = args.first()
      && !params.is_empty()
  {
      … return Ok(Some(format!("decode_succeed(curry{n}({fn_name}))")));
  }
  ```
  A lambda argument (`Expr::Lambda`) and a plain-value argument fall through to
  the standard path. (`emit_json_decoder_call` spans `emit_expr.rs:2699-2743`;
  the `JsonDecList` factory-wrap is `emit_expr.rs:2734-2741`.)
- **Generic lambda emission always produces a boxed tupled closure** —
  `emit_lambda`, `emit_expr.rs:3986-4010`:
  ```rust
  Ok(format!("Box::new(move |{}| -> {ret} {{ {body} }})", parts.join(", ")))
  ```
- **`IrType::Fun` renders as a tupled `Fn` trait object** —
  `emit_types.rs:186-205`: `Box<dyn Fn(T0, …) -> R + Send + 'static>` — never
  the curried `FnOnce` chain the pipeline accumulator needs. (Matters for any
  *annotated* Decoder-of-function position; the fixes below avoid emitting such
  annotations.)
- **Kernel names** — `naming.rs:494` (`JsonDecSucceed → "decode_succeed"`),
  `naming.rs:500-504` (`JsonDecP* → decode_pipeline_*`), and **`naming.rs:647`:
  `KernelFn::DbDecSucceed => "db_decode_succeed"` — a function that does NOT
  exist in the runtime** (`rg 'pub fn db_decode' runtime/src/sky_runtime/db.rs`
  → `db_decode_string/int/float/bool/money/nullable/required/optional` only;
  same for the upstream runtime). Any `Std.Db.Decode.succeed` use is a
  guaranteed exit-0-then-cargo-fail (E0425). `DbDecSucceed` sits on the
  standard emit path (`emit_expr.rs:795-811`), so it also misses the curry
  wrap.
- **`Expr::Let` carries no type** — `sky_ir/src/ir.rs:897-904`; it emits as
  `({ let {name} = {value}; {body} })` (`emit_expr.rs:2828-2836`). There is no
  clone/thunk machinery in `EmitCtx` (established in
  `seal-noncopy-move-design.md` §1.2).

### 1.3 The lowerer and type checker are already correct for these shapes

- HM signatures for the pipeline are Elm-exact —
  `sky_types/src/constrain.rs:3439-3456` (`required : String -> Decoder a ->
  Decoder (a -> b) -> Decoder b`, etc.).
- Kernel routing — `sky_lower/src/lower.rs:4846-4851` (`JsonDecP.*` →
  `KernelFn::JsonDecP*`).
- `|>` desugars in canon to a **nested** call — `sky_canon/src/resolve.rs:
  2331-2342`: `x |> f a b` ⇒ `Call(Call(f,[a,b]),[x])`. The inner 2-of-3-arg
  kernel call is *partial*, so lowering routes it through `eta_expand_partial`
  (`lower.rs:3461-3521`), which converts the residual param/return types via
  `ir_type_from_ty` (`lower.rs:3510`, `3514`) — and a `Ty::Var` there is
  SKY-L0102 (`lower.rs:2717-2728`). This is the `|>`-form blocker (§8.1), a
  skyc-visible failure, **not** part of the seal break.

---

## 2. Reproduction matrix (prebuilt `skyc`, commit-era e586668, emit-only — no cargo run)

All fixtures emitted with
`SKY_RUNTIME_DIR=runtime/src/sky_runtime skyc build <fixture> --out /tmp/…`.
"seal break" = skyc exit-0 while the emitted Rust cannot compile against the
§1.1 signatures.

| # | Shape (Sky source) | skyc | Emitted shape (verbatim from `/tmp/**/src/main.rs`) | Class |
|---|---|---|---|---|
| R1 | `JsonDec.succeed (\name age -> …)` nested in `required`/`optional` chain | **0** | `decode_succeed(Box::new(move \|name: String, age: i64\| -> String { … }))` | **SEAL — E0593/E0308** |
| R2 | 1-arg lambda `JsonDec.succeed (\name -> name ++ "!")` | **0** | `decode_succeed(Box::new(move \|name: String\| -> String { … }))` | **SEAL — same** |
| R3 | plain value `JsonDec.succeed 42` | **0** | `decode_succeed(42)` | **SEAL — E0308** |
| R4 | `let d = <monomorphic lambda pipeline> in` … `decodeString d j1` … `decodeString d j2` | **0** | `let d = decode_pipeline_required(…); … decode_from_json_string(d, j1); … decode_from_json_string(d, j2)` | **SEAL — E0382** |
| R5 | idiomatic `\|>` pipe form (as written in `examples/06-json`) | **exit 2, SKY-L0102** at the partial `JsonDecP.required "name" JsonDec.string` | — (no emit) | not seal; §8.1 |
| R6 | `JsonDec.succeed Profile` (record-alias ctor as value) | **exit ≠0, SKY-N0001** "not found in scope" | — | not seal; §8.2 |
| R7 | top-level `userDecoder : JsonDec.Decoder String` binding | **exit ≠0, SKY-L0102** | — | not seal; §8.3 |
| R8 | lambda whose body is a record literal (`succeed (\u f -> { username = u, … })`), or identity lambda inside a `let`-bound decoder | **exit ≠0, SKY-L0102** on the lambda param | — | not seal; §8.4 (newly discovered) |
| R9 | **control**: named builder `makeProfile : String -> Int -> Profile` + `required`+`optional` | **0** | `decode_succeed(curry2(main_make_profile))` — the golden shape | expected green |

The existing golden `tests/golden/m4h_json_dec_pipeline/Main.sky` passes
because it deliberately uses only R9's shape (named `makeLabel` + fully inlined
duplicated decoder expressions — its own header comment documents dodging R7).

### 2.1 Fixture sources (self-contained; R1/R3/R4 become the red fixtures of §7)

R1 (`m4h_json_dec_pipeline_lambda`):

```elm
module Main exposing (main)

main =
    let
        r1 =
            JsonDec.decodeString
                (JsonDecP.required "age" JsonDec.int
                    (JsonDecP.required "name" JsonDec.string
                        (JsonDec.succeed
                            (\name age -> name ++ "|" ++ String.fromInt age)))
                )
                "{\"name\":\"Alice\",\"age\":30}"
    in
    case r1 of
        Ok label1 -> println label1
        Err _ -> println "err1"
```

R3 (`m4h_json_dec_succeed_value`): `JsonDec.decodeString (JsonDec.succeed 42) "{}"` → print `42`.

R4 (`m4h_json_dec_pipeline_reuse`): bind the R1 decoder as `let d = …` (nested
form, monomorphic lambda) and run `JsonDec.decodeString d` on two different
JSON strings; print both results.

R9 (`m4h_json_dec_pipeline_record`, green control worth adding — the literal
"required+optional record decoder" from the task):

```elm
type alias Profile = { username : String, followers : Int }

makeProfile : String -> Int -> Profile
makeProfile username followers = { username = username, followers = followers }

describe : Profile -> String
describe p = p.username ++ " " ++ String.fromInt p.followers

main =
    let
        r1 =
            JsonDec.decodeString
                (JsonDecP.optional "followers" JsonDec.int 0
                    (JsonDecP.required "username" JsonDec.string
                        (JsonDec.succeed makeProfile)))
                "{\"username\":\"skydev\"}"
    in
    case r1 of
        Ok p -> println (describe p)
        Err _ -> println "err1"
```
Emits `decode_succeed(curry2(main_make_profile))` +
`decode_pipeline_optional("followers", …, 0, …)` — skyc-0 today; expected
cargo-0 (identical contract to the passing `m4h_json_dec_pipeline` golden).

---

## 3. Root cause per error class

### 3.1 E0593 (closure arity) + E0308 (FnOnce vs Fn) — ONE root cause

`emit_json_decoder_call`'s succeed arm (`emit_expr.rs:2722-2733`) under-matches:
only `Expr::FuncValue`. The two fall-through shapes:

- **Lambda (R1/R2).** Generic `emit_lambda` (`emit_expr.rs:4006-4009`) emits
  `Box::new(move |name: String, age: i64| -> String { … })`. Target parameter:
  `Box<dyn Fn() -> A + Send>` (`json.rs:741`). The boxed closure implements
  `Fn(String, i64)`, not `Fn()` — the unsize coercion cannot apply. rustc
  surfaces this as an arity/type mismatch on the closure (E0593 when the
  closure meets the `Fn()` bound directly, E0308 on the failed
  `Box<{closure}> → Box<dyn Fn() -> A>` coercion). Even with the factory shape
  band-aided, the produced `Decoder<E, A>`'s `A` would be a tupled closure —
  while `decode_pipeline_required`'s `next_decoder` demands
  `A = Box<dyn FnOnce(T) -> F + Send>` (`json.rs:1323`) — the E0308
  "expected `Box<dyn FnOnce(…)>`, found closure" leg. Both codes trace to the
  same miss: **the argument was never routed through `curry{N}`**, which is
  precisely the shape that satisfies both bounds at once
  (`Fn() -> Box<dyn FnOnce…>` factory, `json.rs:780-798`).
- **Plain value (R3).** `decode_succeed(42)` — an `i64` where
  `Box<dyn Fn() -> A + Send>` is expected: plain E0308. (`succeed value` is
  legal, common Elm — `oneOf`/`andThen` fallbacks.)

*Attribution honesty:* this design lane ran no cargo (task constraint). The
class→code mapping above is static, against the exact signatures of §1.1;
Lane A's red run (§7) must confirm the concrete rustc codes before the fix
lands — if a code differs, the *shape* diagnosis stands and only the label
moves.

### 3.2 E0425 — `DbDecSucceed` maps to a nonexistent runtime symbol

`naming.rs:647` emits `db_decode_succeed(…)`; no such function exists in
either runtime (§1.2). The upstream Haskell backend maps **both** decode
families to the shared `decode_succeed`
(`../sky/src/Sky/Generate/Rust/Builder/Kernel.hs:608-609`) — the runtime
`Decoder<E, T>` is explicitly "the unified decoder type shared by JsonDec,
DbDec, and Config" (`json.rs:8`). Same family as 3.1: `Std.Db.Decode`
pipelines (`db_decode_required`, `db.rs:488-494`, takes the same
`Box<dyn FnOnce(A) -> B + Send>` accumulator) need the same curry wrapping.

### 3.3 E0382 — moved decoder on reuse

`Decoder` is `!Clone` (§1.1) and `decode_from_json_string` consumes it
(`json.rs:771`). R4's emitted
`let d = …; decode_from_json_string(d, j1); decode_from_json_string(d, j2)`
moves `d` at the first call → E0382 at the second. **The #104 general
clone-pass cannot close this**: its invariant is "every non-`Copy` value is
`Clone`" (`seal-noncopy-move-design.md` header + §2), which is false for
`Decoder` — an emitted `d.clone()` would just shift the failure to E0599
(no method `clone`). `IrType::Decoder` (and `IrType::Task`) must be **excluded
from #104's clone rule** and handled by rebuilding (§5.C).

---

## 4. Why the runtime is right and must not change

The `Box<dyn FnOnce>` chain + factory design is deliberate and load-bearing:

1. **Multi-row/multi-value reuse.** `decode_succeed` calls the factory once per
   `run` (`json.rs:744`); each pipeline `run` consumes one fresh chain
   (`json.rs:780-798` correctness argument). An `Fn` chain would require every
   constructor argument to be `Clone` at every application step.
2. **Byte-parity with the upstream vendored runtime** — local
   `runtime/src/sky_runtime/json.rs` and
   `../sky/runtime-rust/src/sky_runtime/json.rs` are identical in every cited
   region (diff-verified; only comment-block offsets differ). Editing the local
   copy (e.g. `Arc<dyn Fn>` + `derive(Clone)`) would open a permanent
   `sync-with-upstream` seam for a problem the reference compiler already
   solves at the **emit** layer. Rejected (recorded as the considered-and-
   rejected alternative for §5.C).
3. **The tupled-calling-convention contract test**
   (`../sky/runtime-rust/tests/calling_convention.rs`) pins this exact surface.

---

## 5. The fixes

### 5.A `succeed`-argument emission (closes E0593 + E0308) — port of the reference shape

Reference: `../sky/src/Sky/Generate/Rust/Builder/ExprEmitter.hs:2064-2093`
(`succeedArgArity`/`succeedArity` — multi-arg lambda OR named fn/ctor arity)
and `:2160-2177` (the two emissions):

```haskell
Ann.At _ (Can.Lambda params body) ->
    … calleeName ++ "(curry" ++ show n ++ "(|" ++ psStr ++ "| { " ++ body ++ " }))"
_ ->
    calleeName ++ "(curry" ++ show n ++ "(" ++ exprToRustString ctx arg ++ "))"
```

Port into `emit_json_decoder_call` (`emit_expr.rs:2722-2733`), which becomes a
three-way match on `args.first()` and covers **both** succeed kernels
(`JsonDecSucceed | DbDecSucceed`; add `ConfigSucceed` when Config lands):

1. **`Expr::FuncValue` (named fn), `params.len() = n ≥ 1`** — unchanged:
   `decode_succeed(curry{n}({fn_name}))`.
2. **`Expr::Lambda { params, ret, body }`, `params.len() = n ≥ 1`** — NEW:
   ```text
   decode_succeed(curry{n}(move |p1: T1, …, pn: Tn| -> R { <body> }))
   ```
   Implementation: factor the current `emit_lambda` (`emit_expr.rs:3986-4010`)
   into `emit_lambda_unboxed` (everything but the `Box::new(…)` wrap) +
   a boxing wrapper, so param rendering / return-type rendering / body
   emission are shared, not duplicated.
   - `curry{n}` needs `F: Fn + Clone + Send + 'static` (`json.rs:799`). A
     `move` closure is `Clone` iff all captures are `Clone` — guaranteed for
     ordinary Sky values by the #87/#93 derive-seal; **residual**: a lambda
     capturing a function-typed value / `Decoder` / `SkyTask` (all `!Clone`)
     still fails cargo. The reference has the identical residual (guardian C5
     note, `ExprEmitter.hs:2368-2371`). Document, don't block.
   - Captures moved into the closure that are ALSO used later are #104b's
     clone-prelude hole (`seal-noncopy-move-design.md` §2.5) — orthogonal,
     already filed.
   - **Arity cap — diverge from the reference (fail-closed):** the reference
     formats `curryN` unconditionally; `n > 10` would emit a nonexistent
     `curry11` (its own exit-0-then-cargo-fail, E0425). skyc must instead
     reject at emit with a real diagnostic (new `LowerError` variant, e.g.
     "pipeline constructor arity {n} exceeds the supported 10" — SKY-L01xx),
     per the sanctioned-divergence policy (strictly more total than Go/Haskell
     reference). Elm's ecosystem cap (map8/9-era) makes >10-field pipeline
     ctors rare; a curry11.. extension is a separate additive runtime task if
     ever hit.
3. **Anything else (plain value / var / call result)** — NEW factory-wrap:
   ```text
   decode_succeed({ let __sky_succeed = <arg>; Box::new(move || __sky_succeed.clone()) })
   ```
   The `.clone()` inside the `Fn` body is mandatory (returning the capture by
   move out of an `Fn` closure is E0507; the factory is called once per `run`).
   `A: Clone` holds for all ordinary Sky values (derive-seal); the same
   `!Clone` residual as above applies (a `succeed <decoder-value>` is
   type-legal but pathological; fail is acceptable and matches the reference,
   which — note — does not handle the plain-value case at all and emits the
   broken `decode_succeed(42)` too. Another strictly-better divergence.)

Where the fix does NOT go: the lowerer. `curry{N}` is a runtime-surface
concern; the backend already owns this special case (established pattern at
`emit_expr.rs:2699-2743`); the lowerer's IR (`Fun` types) cannot express the
curried-FnOnce-chain type anyway (§1.2, `emit_types.rs:186-205`).

### 5.B `DbDecSucceed` mapping (closes E0425)

`naming.rs:647`: `"db_decode_succeed"` → `"decode_succeed"` (matching
`Kernel.hs:608-609`). Include `DbDecSucceed` in 5.A's kernel match so DB
pipeline ctors curry identically. One line + shared arm; covered by a
DbDec-family fixture (§7, gated like the other Db goldens).

### 5.C Decoder-typed `let` thunking (closes E0382)

Port the *pattern* of the reference's #96 RE-THUNK for move-only `let`-bound
`SkyTask`s (`ExprEmitter.hs:2343-2382`): bind a **rebuilder closure** instead
of the value; every read site becomes a call. Applied to `Decoder` in the
sky-rust port as a **pure lowerer IR rewrite** — zero new backend machinery:

> For a `Let { name, value, body }` whose `value` lowers with type
> `IrType::Decoder(_)`:
> - rewrite `value` to `Expr::Lambda { params: [], ret: Decoder(_), body: value }`
> - rewrite every `Expr::Var(name)` read inside `body` to
>   `Expr::Apply { func: Var(name), args: [] }`

Emission then falls out of existing arms: the binding emits
`let d = Box::new(move || -> Decoder<…> { … })` (via `emit_lambda`,
`emit_expr.rs:4006`; type `IrType::Fun([], Decoder)` renders fine per
`emit_types.rs:186-205`), and each use emits `(d)()` — a fresh `Decoder` per
use. `Box<dyn Fn>` is called by reference, so N uses need no clones.

Decisions & justification:

- **Unconditional (no use-count gate).** Thunk every Decoder-typed `let`, even
  single-use. Decoders are pure values (building one runs no effects — every
  constructor in `json.rs` just allocates closures), so rebuild-per-use is
  semantics-neutral and construction-cost-only; Go re-runs decoder expressions
  per reference anyway (same argument the reference makes for Task thunks,
  `ExprEmitter.hs:2350-2352`). Skipping the use-count avoids depending on
  #104's not-yet-landed liveness pass and keeps the rewrite total by
  construction. Cost: a golden-byte diff on single-use bindings — acceptable;
  refresh goldens in the same commit.
- **Composition with `decode_list`.** Today `decode_list(move || { d })`
  (`emit_expr.rs:2734-2741`) moves a raw `let`-bound decoder out of an `Fn`
  closure — E0507, sibling break. Post-rewrite the read site is `d()`, so it
  emits `decode_list(move || { (d)() })`: the thunk is captured, called per
  element — correct. (The capture consumes `d` for any *later* outer use —
  that residual is #104b's clone-prelude; note the thunk at least makes a
  future clone-prelude *possible*, since the boxed thunk could be Arc'd or the
  prelude can re-emit — record in #104b's notes.)
- **No type annotation on the emitted `let`** — `Expr::Let` emission is
  annotation-free already (`emit_expr.rs:2835`), nothing to suppress.
- **Rejected alternative:** `Arc<dyn Fn>` + `derive(Clone)` on `Decoder`
  (runtime edit) — see §4. Also rejected: inlining the RHS at each use
  (duplicated emission, code-size blowup, and breaks if the RHS captures
  once-movable locals).
- **Reach today is small but real:** many `let`-decoder shapes die earlier at
  SKY-L0102 (R8/§8.4), but the monomorphic-lambda shape R4 is live now, and
  every §8 unblock (pipes, top-level values, ctors) widens the reuse surface —
  the thunk must land **before or with** those unblocks or they re-open the
  seal.
- **#104 interaction (must land as part of C):** `IrType::Decoder` /
  `IrType::Task` are excluded from the #104 clone rule (they are `!Clone`;
  §3.3). Add the exclusion note to that design's §1.6 non-`Copy` predicate
  when Lane A implements either side first.

---

## 6. Reference (`../sky`) — port vs diverge summary

| Concern | Reference behaviour | This design |
|---|---|---|
| `succeed <lambda>` | `curryN(\|ps\| { body })` (`ExprEmitter.hs:2169-2175`) | **Port** (as `move` closure via `emit_lambda_unboxed`) |
| `succeed <named fn/ctor>` | `curryN(name)` (`ExprEmitter.hs:2176-2177`; ctor arity via `ecCtorArity`, `:2073-2075`) | Already ported (`emit_expr.rs:2722-2733`); ctor-as-value blocked upstream of emit (§8.2) |
| `succeed <plain value>` | **Unhandled — same latent break** | **Diverge: fix** (factory-wrap, §5.A.3) |
| `curryN`, N > 10 | Emits nonexistent `curry11` (latent E0425) | **Diverge: fail-closed diagnostic** |
| `DbDec.succeed` | `decode_succeed` (`Kernel.hs:608-609`) | **Port** (naming.rs one-liner) |
| Decoder reuse — top-level | Top-level values emit as per-reference zero-arg fns (`ModuleEmitter.hs:315-357` comment: "lowers as a per-reference function") → rebuilt per use | N/A yet (top-level decoder values are L0102, §8.3); when they land, emit as zero-arg fns for the same effect |
| Decoder reuse — `let`-bound | No Decoder-specific gate found; `SkyTask` RE-THUNK exists (`ExprEmitter.hs:2343-2382`); a `let`-bound decoder read ≥2 would hit `ecCloneVars` → `d.clone()` → **latent break** (Decoder `!Clone`) | **Diverge: total fix** (thunk ALL Decoder lets, §5.C) |

Runtime: identical in both repos for every cited symbol; untouched.

---

## 7. Red→green fixtures (`tests/golden/…`, wired like `crates/skyc/tests/golden_m4h_json_dec.rs`)

Each fixture must satisfy the full seal predicate **skyc-0 ∧ cargo-0 ∧ run-0 ∧
stdout == oracle** via the existing `assert_runs_and_matches_oracle` harness
(`golden_m4h_json_dec.rs:57-77`, `SKY_E2E=1`-gated). RED first: add fixture +
test, record the cargo failure (confirming §3's code attribution), then land
the fix and flip green. Timeout-bound every cargo/run invocation per repo
rules.

| Fixture | Source (§2.1) | Exercises | Expected stdout |
|---|---|---|---|
| `m4h_json_dec_pipeline_lambda` | R1 | 5.A.2 (multi-arg lambda curry) | `Alice\|30` |
| `m4h_json_dec_pipeline_lambda1` | R2 | 5.A.2 (1-arg lambda / `curry1`) | `Alice!` |
| `m4h_json_dec_succeed_value` | R3 | 5.A.3 (factory-wrap) | `42` |
| `m4h_json_dec_pipeline_reuse` | R4 | 5.C (thunk; two `decodeString` calls on one binding) | `Alice\|30` + `Bob\|25` |
| `m4h_json_dec_pipeline_record` | R9 | required+optional **record** decoder (the task's literal ask); green control pre-fix | `skydev 0` |
| `m4h_json_dec_list_letbound` | R4-variant with `JsonDec.list d` | 5.C × `decode_list` factory composition | `2` |
| DbDec sibling (gated with Db goldens) | `DbDec.succeed makeRow \|> …` analog | 5.B | per-oracle |

Regression guards (unit-level, no E2E): an emit test asserting
`JsonDec.succeed <2-arg lambda>` produces a string containing
`decode_succeed(curry2(` and NOT `decode_succeed(Box::new(move |`; an emit
test asserting a Decoder-typed `let` produces `move ||` + `()` call sites;
a naming test pinning `kernel_name(DbDecSucceed) == "decode_succeed"`.

Oracle note: run the Go oracle for each fixture per the existing
`support::assert_go_parity` flow; `optional`-with-absent-field semantics
(default only on absent/null, error on present-but-malformed —
`json.rs:1365-1378`) must be covered by at least one fixture input.

---

## 8. NOT this seal hole, but gates `examples/06-json` — file/verify each (no-deferral)

06-json goes green only when #89 **and** these land. All reproduced this
session (§2 R5-R8):

1. **`|>` pipe form → SKY-L0102** (R5). `x |> f a b` canonises to
   `Call(Call(f,[a,b]),[x])` (`resolve.rs:2337-2342`); the inner partial
   kernel call eta-expands (`lower.rs:3461+`) and dies in `ir_type_from_ty`
   (`lower.rs:3510/3514 → 2728`). Even with types pinned, the eta-lambda's
   accumulator param would render as a tupled `Box<dyn Fn…>`
   (`emit_types.rs:186-205`) — a §3.1-class cargo-fail. **Direction:** flatten
   in `combine_binop` — `x |> f(args)` ⇒ `Call(f, args ++ [x])` (semantics-
   identical in a curried language; the lowerer re-splits by arity —
   `lower.rs:3115-3131` — so over-application still routes to `saturate_over`).
   This makes the pipe form take the exact same lowered shape as the
   already-working nested form (R1/R9) — it is how the reference gets
   `decode |> Pipeline.required "x" dec` to the direct 3-arg call
   (`json.rs:1316-1319`). Check first whether an existing task (#94/#95/#99/
   #104/#112 per `parity-gap-snapshot.md:9`) owns it; file otherwise.
2. **`succeed RecordCtor` → SKY-N0001** (R6): record-alias constructor as a
   first-class value. Owned by `record-alias-ctor-design.md`; once it lowers
   (presumably to `Expr::FuncValue`), 5.A.1 curries it for free — add a
   fixture then.
3. **Top-level decoder bindings → SKY-L0102** (R7) — the M4h golden's
   documented limitation (`tests/golden/m4h_json_dec_pipeline/Main.sky:11-15`).
   When fixed, emit them as per-reference zero-arg fns (reference behaviour,
   §6) so no reuse/E0382 surface re-opens.
4. **NEW — lambda-body-shape L0102s** (R8): `succeed (\u f -> { … record … })`
   and `let`-bound `succeed (\id -> id)` leave a lambda-param region
   unresolved (verified: adding a typed consumer moves the error from T0012 to
   L0102 on the param). Type-side region-propagation gap, distinct from all of
   the above. **Spotted = filed**: needs its own task; 06-json's `userDecoder`
   / `todoDecoder` lambdas hit it once pipes are unblocked.
5. 06-json itself currently stops at SKY-L0106 (untyped top-level fns,
   `parity-gap-snapshot.md:23`) before any of this is reachable.

---

## 9. Landing protocol

- **Seal-touching** → adversarial (Opus guardian) review before commit, per
  the protocol in `seal-noncopy-move-design.md`. Review focus: (a) `curry{N}`'s
  `F: Clone` bound vs capture kinds — enumerate what a succeed-lambda can
  capture; (b) the thunk rewrite's interaction with `Expr::Var` reads inside
  nested closures/match arms (the rewrite must catch every read, or a bare
  `d` where `d` is now a thunk emits a type error — *fail-loud, still verify*);
  (c) confirm the rustc code attribution from the red run.
- **Ordering:** A+B first (one PR: emit arm + naming + fixtures
  R1/R2/R3/R9/Db), C second (lowerer rewrite + R4/list fixtures + #104
  exclusion note). A+B are independent of #104/#104b; C must coordinate with
  #104's predicate (§5.C) if that lands first.
- **Lane A task breakdown (bite-sized, each red→green):**
  1. Factor `emit_lambda_unboxed` out of `emit_lambda` (pure refactor, goldens
     byte-identical).
  2. Extend `emit_json_decoder_call` succeed arm: lambda→`curry{n}`, plain
     value→factory-wrap, kernel match widened to `JsonDecSucceed | DbDecSucceed`;
     arity>10 fail-closed diagnostic (+ `skyc explain` entry).
  3. `naming.rs:647` → `"decode_succeed"` + naming unit test.
  4. Fixtures R1/R2/R3/R9 + Db sibling; confirm red (record cargo codes),
     then green; refresh oracles.
  5. Lowerer thunk rewrite for `IrType::Decoder` lets + read-site `Apply`
     rewrite; emit unit tests.
  6. Fixtures R4 + `list_letbound`; golden refresh for single-use byte-diffs.
  7. File §8.1 (pipe flatten — check existing task ownership first) and §8.4
     (lambda-param region gap) as tasks; cross-link #104's Decoder/Task
     exclusion.

---

*Investigation artifacts: fixtures + emitted projects under
`/tmp/jsondecp-fx-*` / `/tmp/jsondecp-out-*` (ephemeral; sources reproduced in
§2.1). skyc binary: prebuilt `~/.cache/sky-rust-target/release/skyc`
(2026-07-01, HEAD e586668-era). No crate was modified; no cargo build was run
(design-lane constraint) — §3's rustc-code attribution is static analysis
against the cited runtime signatures, to be confirmed by Lane A's red run.*
