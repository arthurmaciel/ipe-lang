# Seal gates #94 (Msg-admissibility) + #95 (lambda-view Model gate-bypass)

> **Status:** design (Doc/Design Lane). READ-ONLY study of the crates; no code
> written. Both gates are **seal-touching** (`skyc` exit-0 MUST imply `cargo`
> exit-0) → **Opus adversarial review before commit** (see §6).
>
> **Update (2026-07-04, #108 round 4):** the §2.3 shared extractor
> `fn_param_ty` LANDED in `emit_model_gate.rs` as part of the #108 seal fixes
> (review C4 required it — a lambda-`view` ROUTED app silently emitted the
> non-routed `live_app`). `model_ty_of_view` now routes through it, closing
> **#95** (Model-gate side — regressions in `model_admissibility.rs`
> `live_lambda_view_*`) and the #108 routed-detection side (regressions in
> `golden_m7_live_lambda_view_routed.rs`). **#94 (Msg gate, SKY-L0121) remains
> unimplemented** — §2's `msg_ty_of_update` / `check_admissible_msg` /
> `InadmissibleAppMsg` are still to build on top of the landed extractor.
>
> **Reference:** #91 (COMPLETED) — the app-entry Model-admissibility gate. This
> design ports #91's shape to the Msg type parameter (#94) and closes the
> lambda-binding bypass that lets an inadmissible Model slip past the #91 gate
> (#95).

---

## 0. The hole class in one paragraph

A `Std.Live` / `Std.Tui` / `Std.Webview` app threads a **Model** and a **Msg**
type through the TEA quartet (`init` / `update` / `view` / `subscriptions`). The
runtime entry points impose Rust trait bounds on both type parameters. If a
well-typed Sky program puts a non-derivable / non-serde value (an `Html`, a
`Cmd`, a `Task`, a function, …) into its Model **or** its Msg, `skyc` succeeds
and the emitted Rust then fails `cargo build` on the missing trait bound. That
is the "exit-0-then-cargo-fail" seal violation. #91 closed the Model side; #94
closes the Msg side; #95 closes a recovery-site bypass that reopens the Model
side whenever `view` is a lambda.

---

## 1. How #91's Model gate works today (file:line)

### 1.1 Runtime bounds (the source of truth for what is "admissible")

`runtime/src/sky_runtime/live/mod.rs` — `pub fn live_app<E, Model, Msg, …>`:

```rust
Model: serde::Serialize + serde::de::DeserializeOwned + Clone + PartialEq + Send + Sync + 'static,
Msg:   Clone + Send + Sync + std::fmt::Debug + 'static,
```

`runtime/src/sky_runtime/tui/app.rs` — `pub fn tui_app<…>` and
`runtime/src/sky_runtime/webview.rs` — `pub fn webview_app<…>`:

```rust
Model: Clone + Send + 'static,
Msg:   Clone + Send + 'static,
```

So the **Model** admissibility predicate is app-shape-dependent:
`Live → serde`, `Tui/Webview → Clone`. (This is exactly what #91 encodes.)

### 1.2 The IR predicates (#87 / #93)

`crates/sky_ir/src/ir.rs`:

- `ir_type_is_derivable(ty, enum_derivable)` — ir.rs:699. `true` iff the rendered
  Rust type derives `Clone + Debug + PartialEq`. Leaves `Task/Cmd/Sub/Decoder/
  Db/Fun/Server*/Live*` → `false`; the two Clone-only `Ui` carriers
  (`html::Attribute`/`Event`) and all UI value types are `true`.
- `ir_type_is_serde(ty, enum_serde)` — ir.rs:796. `true` iff the rendered type
  derives `serde::Serialize + DeserializeOwned`. A **strict subset** of the
  derivable leaf set: it additionally demotes every `Ui`/`UiPlain` carrier to
  `false` (Html/Element/Color derive only Clone/Debug/PartialEq, never serde).
  Structural invariant documented at ir.rs:763 — `is_serde(t) ⇒ is_derivable(t)`.

Both are total (exhaustive match, no wildcard arm — walker-arm rule), and both
take an `enum_*` closure so the whole-program enum fixpoint (`EmitCtx`) answers
the enum-to-enum question.

### 1.3 The gate module

`crates/sky_backend_rust/src/emit_model_gate.rs`:

- `model_ty_of_view(view_e: &Expr) -> Option<&IrType>` — emit_model_gate.rs:38.
  **This is the recovery site.** It matches **only**
  `Expr::FuncValue { ty: IrType::Fun(params, _), .. }` and returns
  `params.first()` (the Model, since `view : Model -> Html Msg`). **Any other
  shape returns `None`, and the caller then SKIPS the gate (fail-open).** ← this
  `None`-skip is the exact seam #95 exploits.
- `check_admissible_model(ctx, model_ty, app) -> DResult<()>` —
  emit_model_gate.rs:62. Applies `ir_type_is_serde` for `Live`,
  `ir_type_is_derivable` for `Tui/Webview`. On failure, `blame` (emit_model_gate.rs:101)
  walks the record to name the offending field and `leaf_of` (emit_model_gate.rs:119)
  classifies the first non-admissible leaf → `Diagnostic::Lower { span: DUMMY,
  msg: LowerError::InadmissibleAppModel { app, field, leaf } }`.

### 1.4 The invocation sites (one per app shape)

- `crates/sky_backend_rust/src/emit_live.rs:229` (inside `emit_live_app_inner`,
  after `view_e = lookup_field(ctx, fields, "view")`).
- `crates/sky_backend_rust/src/emit_tui.rs:143`.
- `crates/sky_backend_rust/src/emit_webview.rs:126`.

Each does:

```rust
if let Some(model_ty) = crate::emit_model_gate::model_ty_of_view(view_e) {
    crate::emit_model_gate::check_admissible_model(ctx, model_ty, AppShape::<Shape>)?;
}
```

All three emitters already `lookup_field(ctx, fields, "update")` into a local
`update_e` (emit_live.rs:220, emit_tui.rs:133, emit_webview.rs:118) — so the Msg
recovery site (§2) needs no new plumbing.

### 1.5 The diagnostic

`crates/sky_diagnostics/src/diagnostic.rs`:
`LowerError::InadmissibleAppModel { app: AppShape, field: Box<str>, leaf: ModelLeaf }`
(diagnostic.rs:629), code `SKY_L0120` (code.rs:199, mapped at diagnostic.rs:904),
message builder `inadmissible_model_message(app, field, leaf)` (diagnostic.rs:1045),
help/note wiring at diagnostic.rs:1085, render label at render.rs:453, explain
doc `crates/sky_diagnostics/explain/SKY-L0120.md`. `AppShape` (Live/Tui/Webview)
at diagnostic.rs:576; `ModelLeaf` (Function/Command/Subscription/Task/Decoder/
Handle/ViewValue) at diagnostic.rs:594.

### 1.6 The regression tests

`crates/skyc/tests/model_admissibility.rs` — compile-only (runs the `skyc`
pipeline, never `cargo`; not gated on `SKY_E2E`). `assert_accepted` for good
Models, `assert_rejected_with(…, "SKY-L0120")` for `LIVE_CMD_MODEL`,
`LIVE_HTML_MODEL`, `TUI_CMD_MODEL`.

---

## 2. #94 — Msg-admissibility gate

### 2.1 Critical correctness finding — Msg needs `derivable`, NOT `serde`

Read the runtime bounds again (§1.1): **for all three app shapes the Msg bound
is `Clone + Send [+ Sync + Debug]` — serde is never required of Msg.** Only the
Live *Model* is persisted to the session store; the Msg is transient (dispatched
over channels, never gob/serde-round-tripped in a way the type bound demands).

Therefore the Msg predicate is **`ir_type_is_derivable` for Live, Tui, AND
Webview** — the *same* predicate for every shape. Do **not** reuse
`check_admissible_model`'s `Live → is_serde` branch for Msg; that would
false-reject an admissible Live Msg that carries an `Html`/`Element`/`Color`
(all of which are `Clone` and thus a legal Msg payload, e.g. a
`GotView (Html Msg)` message).

| Slot  | Live      | Tui       | Webview   |
|-------|-----------|-----------|-----------|
| Model | `serde`   | derivable | derivable |
| Msg   | derivable | derivable | derivable |

This asymmetry (Html admissible in a Live *Msg*, inadmissible in a Live *Model*)
is correct and must be preserved by a fixture (§2.5, `LIVE_HTML_MSG` → green).

What a Msg **cannot** carry (→ non-derivable → gate fires, all shapes): a
function `Fun`, a `Cmd`, a `Sub`, a `Task`, a `Decoder`, a `Db`/server/live
handle — directly, inside a transparent carrier (`Maybe`/`List`/`Set`/`Result`/
`Dict`/`Tuple`/`Record`), or inside a variant payload of a user enum reachable
from Msg.

### 2.2 Recovery site — `update`'s first parameter

`update : Msg -> Model -> (Model, Cmd Msg)`. The Msg is `params[0]` of `update`'s
function type — present in **all three** app shapes (Live/Tui/Webview all bound
`FUpdate: Fn(Msg, Model) -> …`). `update_e` is already looked up in every
emitter (§1.4). Recover Msg as `fn_param_ty(update_e, 0)`.

Rationale for `update` over `view`/`subscriptions`: `view : Model -> Html Msg`
carries Msg only *inside* its return `Html<Msg>` (awkward to peel);
`subscriptions : Model -> Sub Msg` likewise. `update`'s first *parameter* is the
Msg directly — one hop, same shape recovery used for the Model. (For Tui,
`onKey : String -> String -> Msg` also exposes Msg as its return, but it is
optional; `update` is mandatory and uniform.)

### 2.3 Recovery predicate reuse — one shared, Lambda-aware extractor

Introduce a single parameter extractor that both gates use (this also fixes #95
— see §3):

```rust
/// The `idx`-th parameter type of a function-valued cfg field, whether that
/// field is a named function reference (`Expr::FuncValue`) or a lambda
/// (`Expr::Lambda`). `None` for any other shape (caller skips — documented
/// residual, see §3.3).
pub fn fn_param_ty(e: &Expr, idx: usize) -> Option<&IrType> {
    match e {
        Expr::FuncValue { ty: IrType::Fun(params, _), .. } => params.get(idx),
        Expr::Lambda { params, .. }                        => params.get(idx).map(|(_, ty)| ty),
        _ => None,
    }
}
```

`model_ty_of_view(view_e)` becomes `fn_param_ty(view_e, 0)`;
`msg_ty_of_update(update_e)` is `fn_param_ty(update_e, 0)`.

### 2.4 The Msg check + diagnostic

Add a Msg-specific check that always uses `ir_type_is_derivable`, reusing the
`blame`/`leaf_of` machinery. The cleanest refactor keeps the internal helpers
(`admissible`, `blame`, `leaf_of`) parameterized by an admissibility mode rather
than raw `AppShape`, so the Msg path forces `derivable` regardless of shape:

```rust
enum AdmMode { ModelFor(AppShape), Msg }   // Msg ⇒ always derivable

fn admissible(ctx, ty, mode) -> bool {
    match mode {
        AdmMode::ModelFor(AppShape::Live) => ir_type_is_serde(ty, …),
        AdmMode::ModelFor(_) | AdmMode::Msg => ir_type_is_derivable(ty, …),
    }
}
```

`check_admissible_msg(ctx, msg_ty, app) -> DResult<()>` mirrors
`check_admissible_model` but with `AdmMode::Msg` and emits the Msg variant.

**Diagnostic (recommended: a sibling variant + a new code).** Add
`LowerError::InadmissibleAppMsg { app: AppShape, field: Box<str>, leaf: ModelLeaf }`
mapped to a new `SKY_L0121`, with its own explain doc `explain/SKY-L0121.md`.
Reuse `AppShape` and `ModelLeaf` unchanged. Factor the message builder to take
the slot noun + requirement so wording stays DRY:

- Model/Live: "a Sky.Live **Model** must be serialisable (it is persisted to the
  session store), but …".
- Msg/any: "a Sky.Live/Sky.Tui/Sky.Webview **Msg** must be clonable, but its
  variant/field … is a command (`Cmd`) / a function / … — keep only plain data
  in messages".

`field` names the offending record field when Msg is a record; for the common
case (Msg is an enum with a bad variant payload) `field` is empty and the leaf
phrase carries the signal (as it already does for a non-record Model).

Rationale for a new code over overloading `SKY-L0120`: one code = one situation
keeps `explain` precise and the code.rs title honest ("app **Msg** is not
admissible …"). An acceptable lighter-weight alternative is a single unified
variant `InadmissibleAppState { app, slot: AppSlot { Model, Msg }, field, leaf }`
under one code — but that renames the existing #91 variant and its explain doc;
the sibling-variant path is lower-churn and mirrors #91 exactly. **Recommend the
sibling variant.**

### 2.5 Invocation — one line per emitter, next to the existing Model gate

In `emit_live_app_inner` / the tui / webview inner emitters, immediately after
the existing Model gate block:

```rust
if let Some(msg_ty) = crate::emit_model_gate::msg_ty_of_update(update_e) {
    crate::emit_model_gate::check_admissible_msg(ctx, msg_ty, AppShape::<Shape>)?;
}
```

### 2.6 Fixtures that must red→green (`crates/skyc/tests/msg_admissibility.rs`)

Model on the existing `model_admissibility.rs` harness (compile-only, no cargo).

**Must be REJECTED with `SKY-L0121`:**

1. `LIVE_CMD_MSG` — `type Msg = Tick | Defer (Cmd Msg)` (a `Cmd`-carrying
   variant). Live.
2. `LIVE_FUNC_MSG` — `type Msg = Tick | WithK (Int -> Int)` (a function payload).
   Live.
3. `LIVE_TASK_MSG` — variant carrying `Task Error String`. Live.
4. `TUI_CMD_MSG` — `Cmd`-carrying Msg variant under `Tui.app`.
5. `WEBVIEW_FUNC_MSG` — function-carrying Msg variant under `Webview.app`.
6. `LIVE_LAMBDA_UPDATE_CMD_MSG` — the #94×#95 crossover: `update = \msg model ->
   …` bound as a **lambda** with a `Cmd`-carrying Msg. Proves the Msg gate fires
   even when `update` is a lambda (recovery via `Expr::Lambda.params[0]`). This
   is the shape the happy-path fixtures dodge — flagged for Opus review (§6).

**Must be ACCEPTED (skyc-0):**

7. `LIVE_HTML_MSG` — `type Msg = Tick | GotView (Html Msg)`. **Green** — Html is
   `Clone`, admissible as a Msg payload for Live (the Model/Msg asymmetry, §2.1).
8. `LIVE_PLAIN_MSG` / reuse `LIVE_GOOD` — plain enum Msg, green.

---

## 3. #95 — lambda-view Model gate-bypass

### 3.1 Root cause

`model_ty_of_view` (emit_model_gate.rs:38) matches **only**
`Expr::FuncValue { ty: Fun(params, _) }`. When the cfg is written point-free with
a named `view` function (`view = view`), the field lowers to an `Expr::FuncValue`
whose `ty` is the solved `Fun([Model], Html<Msg>)`, and the gate reads
`params[0]`. But when the user writes `view = \m -> …`, the field expression is
an **`Expr::Lambda`**, not a `FuncValue`. `model_ty_of_view` has no `Lambda` arm,
returns `None`, and the caller's `if let Some(..)` **skips the gate (fail-open)**.
A well-typed program with an inadmissible Model behind a lambda `view` therefore
sails past #91 and cargo-fails — a live seal hole.

The same fail-open would bite the #94 Msg gate if it recovered from a lambda
`update` and only matched `FuncValue` — which is exactly why §2.3 makes the
extractor Lambda-aware from the start.

### 3.2 Why a lambda binding carries the type anyway (the fix is cheap)

`Expr::Lambda { params: Vec<(Symbol, IrType)>, ret, body }` (ir.rs:982) carries
**concrete** parameter `IrType`s: `sky_lower::lower_lambda` (lower.rs:2224) reads
the lambda's solved region type (`self.types.regions.get(&span)`, lower.rs:2231)
and, per parameter, records `ir_type_from_ty(arg, …)` (lower.rs:2259). For an app
cfg lambda, the `Live.app` / `Tui.app` / `Webview.app` record constraint (see
upstream `("Live","app")` in `sky/src/Sky/Type/Constrain/Expression.hs` — §4)
pins `view : Model -> Html Msg`, so the lambda's `params[0].1` is the concrete
Model type, fully solved. Nested `\m -> \… ` lambdas are flattened into one
multi-param `Lambda` (lower.rs:2277), so `params[0]` is always the first
user-visible parameter.

**Fix:** the Lambda-aware `fn_param_ty` in §2.3 is the whole fix. `model_ty_of_view`
becomes `fn_param_ty(view_e, 0)` and now returns `Some(&Model)` for both the
`FuncValue` and `Lambda` bindings. No diagnostic change; no new plumbing.

### 3.3 Residual (documented, tracked) — neither FuncValue nor Lambda

If `view`/`update` is some *other* expression (a `Var` referencing a let-bound
local, a partial application, a point-free composition), `fn_param_ty` still
returns `None` and the gate still skips. This is a narrower residual than today's
(lambdas are the common inadmissible-behind-view shape; the others are rare).
Two belt-and-braces options for a **follow-up** hardening, not required to close
#95:

- **(c) gate at a site that always has the type** — recover Model/Msg from the
  solver's region type for the `Live.app` *call* itself (the cfg record's field
  types are pinned by the `("Live","app")` constraint), independent of the field
  expression's syntactic shape. Bigger change; deferred.
- Reject a cfg whose `view`/`update` field is neither a `FuncValue` nor a
  `Lambda` with a clean "app cfg fields must be a function or lambda"
  diagnostic (fail-closed instead of fail-open). Cheapest guaranteed-total
  option; evaluate under Opus review.

### 3.4 Fixtures that must red→green

Add to `model_admissibility.rs` (Model side) and `msg_admissibility.rs`:

1. `LIVE_LAMBDA_VIEW_CMD_MODEL` — Model has a `Cmd`-typed field, `view = \m ->
   Ui.layout [] …` bound as a **lambda**. Pre-fix: `skyc`-0 then cargo-fail.
   Post-fix: **rejected `SKY-L0120`**. (The core #95 regression.)
2. `TUI_LAMBDA_VIEW_CMD_MODEL` — same, `Tui.app`, lambda `view`.
3. `LIVE_LAMBDA_VIEW_GOOD` — plain-data Model, lambda `view` → still **accepted**
   (proves the Lambda arm does not false-reject).
4. `LIVE_LAMBDA_UPDATE_CMD_MSG` (shared with §2.6 #6) — lambda `update`,
   inadmissible Msg → **rejected `SKY-L0121`**.

---

## 4. Port vs. sanctioned divergence

Upstream `../sky` (Go backend) types the TEA quartet's field shapes to propagate
Model/Msg through HM — `("Live","app")` in
`sky/src/Sky/Type/Constrain/Expression.hs` builds an **open** record type
constraint (`init/update/view/subscriptions/routes/notFound` + `appExt` row var)
so Model/Msg flow into user code. It does **not** perform any admissibility
(serde/Clone) check — Go's runtime reflects/gob-encodes the Model dynamically and
tolerates functions/Html in Model/Msg at compile time (they just fail to
round-trip at runtime, or are simply carried as `any`).

| Piece | Port or divergence |
|---|---|
| Recovering Model/Msg from the constrained cfg field types (view param / update param) | **Port** — this is upstream's TEA field-type propagation (`("Live","app")` record constraint), reused as the recovery basis. |
| The admissibility **gate** itself (#91 Model, #94 Msg) — rejecting non-serde/non-Clone Model/Msg at `skyc` | **Sanctioned divergence.** Rust's static trait bounds (`serde`/`Clone` on `live_app`/`tui_app`/`webview_app`) make the Go-dynamic path a `cargo`-fail. We gate at `skyc` to preserve the seal (skyc-0 ⇒ cargo-0). Record in `docs/divergences-from-sky.md`. |
| #95 lambda-view fix (Lambda-aware recovery) | **Divergence-support** — a robustness fix on the divergent gate's recovery site; no upstream analog (Go has no such gate). |
| Msg predicate = `derivable` for **all** shapes (not serde) | **Divergence detail**, dictated by the Rust runtime bounds (§1.1), not by upstream. |

---

## 5. Lane A task breakdown (bite-sized)

Each step is independently compilable; keep the one build lane saturated.

- **A1** (sky_ir / none): confirm no predicate change needed — `ir_type_is_derivable`
  already covers the Msg leaf set. (Read-only confirm; likely zero code.)
- **A2** (emit_model_gate.rs): add `fn_param_ty(e, idx)` with `FuncValue` +
  `Lambda` arms; re-express `model_ty_of_view` as `fn_param_ty(view_e, 0)`. Add
  `msg_ty_of_update(update_e)` = `fn_param_ty(update_e, 0)`. **Closes #95.**
- **A3** (emit_model_gate.rs): introduce `AdmMode { ModelFor(AppShape), Msg }`;
  thread it through `admissible`/`blame`/`leaf_of`; add `check_admissible_msg`
  (always `derivable`).
- **A4** (sky_diagnostics): add `LowerError::InadmissibleAppMsg`, `SKY_L0121`
  (code.rs const + title + `code_of` arm + `explain` mapping + `all_codes`
  list), generalize `inadmissible_model_message` to take slot noun/requirement,
  wire `lower_help` + `lower_label`, add `explain/SKY-L0121.md`.
- **A5** (emit_live.rs / emit_tui.rs / emit_webview.rs): add the one-line Msg
  gate invocation after each existing Model gate block. **Closes #94.**
- **A6** (tests): `crates/skyc/tests/msg_admissibility.rs` (§2.6 fixtures 1–8) +
  extend `model_admissibility.rs` with the §3.4 lambda-view fixtures. All
  compile-only, no `SKY_E2E`.
- **A7** (docs): update `docs/divergences-from-sky.md` (gate is sanctioned
  divergence) + note #94/#95 closed in the seal status; keep the walker-arm
  exhaustiveness note (new `LowerError` variant → explicit arms in code.rs,
  render.rs, diagnostic.rs).
- **A8** (adversarial): the §6 review pass **before** commit.

---

## 6. Seal-touching → Opus adversarial review before commit

Both gates decide whether `skyc` accepts or rejects a program at the exact
skyc-0 ⇒ cargo-0 boundary. A too-loose gate reopens the seal hole; a too-tight
gate false-rejects a valid app. The happy-path fixtures deliberately dodge the
adversarial shapes — Opus must build and confirm the following **before**
commit (build the Rust, run `cargo` on the emitted project for the accept cases,
confirm the reject cases produce the diagnostic and never reach `cargo`):

1. **Inadmissible Model behind a lambda `view`** (`LIVE_LAMBDA_VIEW_CMD_MODEL`,
   `TUI_LAMBDA_VIEW_CMD_MODEL`) — the #95 core. Must reject `SKY-L0120`, not
   cargo-fail.
2. **Inadmissible Msg behind a lambda `update`** (`LIVE_LAMBDA_UPDATE_CMD_MSG`) —
   the #94×#95 crossover. Must reject `SKY-L0121`.
3. **Admissible Html in a Live Msg** (`LIVE_HTML_MSG`) — must **accept** and
   `cargo`-build, proving the Msg predicate is `derivable` (not serde) and the
   Model/Msg asymmetry is preserved. A regression here would be a silent
   false-reject.
4. **Nested-carrier Msg** — `type Msg = Batch (List (Cmd Msg))` and
   `Keyed (Dict String (Int -> Int))` — must reject (leaf recursion through
   `List`/`Dict` into the non-derivable leaf), proving `leaf_of`/`admissible`
   recurse under `AdmMode::Msg`.
5. **Residual fail-open** (§3.3) — a `view`/`update` bound to a let-bound `Var`
   or a partial application with an inadmissible Model/Msg. Confirm the current
   behaviour (skip → cargo-fail) is acknowledged and decide whether A-follow-up
   (option (c) or the fail-closed reject) ships now or is filed. Do **not** leave
   it as a silent unknown.
6. **Enum fixpoint** — a Msg enum whose bad payload is reached only through a
   *second* user enum (`Msg = Wrap Inner`, `Inner = Bad (Cmd Msg)`) — must
   reject, proving the `enum_derivable` fixpoint feeds `check_admissible_msg`.

---

## 7. Summary of exact sites

| Concern | Site |
|---|---|
| Model recovery (today, FuncValue-only) | `crates/sky_backend_rust/src/emit_model_gate.rs:38` |
| Model check + blame + leaf | `emit_model_gate.rs:62` / `:101` / `:119` |
| Model gate invocation | `emit_live.rs:229`, `emit_tui.rs:143`, `emit_webview.rs:126` |
| `update_e` already available | `emit_live.rs:220`, `emit_tui.rs:133`, `emit_webview.rs:118` |
| Msg recovery (#94, new) | `update_e` `params[0]` via Lambda-aware `fn_param_ty` |
| Msg predicate | `ir_type_is_derivable` for **all** shapes (ir.rs:699) |
| Lambda param types (why #95 is cheap) | `sky_lower/src/lower.rs:2224` (`lower_lambda`), `Expr::Lambda` ir.rs:982 |
| Runtime bounds (source of truth) | `live/mod.rs` `live_app`, `tui/app.rs` `tui_app`, `webview.rs` `webview_app` |
| Diagnostic (reuse + new) | `diagnostic.rs:629` (`InadmissibleAppModel`) → add `InadmissibleAppMsg`; `code.rs:199` (`SKY_L0120`) → add `SKY_L0121` |
| Regression harness | `crates/skyc/tests/model_admissibility.rs` (compile-only) |
