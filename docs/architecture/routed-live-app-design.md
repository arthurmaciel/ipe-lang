# Routed Live.app — Port Design (#108 `RoutedLiveApp`)

> Doc/Design Lane deliverable. READ-ONLY on all crates; this is the port
> spec Lane A implements. Faithful port of `../sky`'s proven approach —
> `../sky` (the original Haskell compiler **and its own already-shipped
> Rust backend + `runtime-rust`**) is the reference spec throughout.
>
> Status: **IMPLEMENTED** — T1–T7 complete (open-record types, unify, Live.app
> scheme, routed emit + set_page, type-directed payload conversion, SKY-I0001
> closed). Sweep gate (T8 E2E) pending CI run with `SKY_E2E=1`.
> **Round 4 (seal review):** three adversarial holes fixed — see §11
> (parametric `IrType::LiveRoute(page)` rendering `Route<Page>`; lambda-view
> routed detection via the shared `fn_param_ty`; per-route page witness
> replacing the shared-var `Live.route` scheme that false-blocked `:param`
> routes).

---

## 0. TL;DR

The corpus writes the **canonical, reference-shaped** entry point:

```elm
main =
    Live.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [ route "/" HomePage, route "/apps/:slug" AppDetailPage ]
        , notFound = HomePage
        }
```

Our port rejects it with **SKY-T0001** because `Live.app`'s cfg is typed as
a **closed 4-field record** `{ init, update, view, subscriptions }`
(`crates/sky_types/src/constrain.rs:2911`), and our record unifier requires
**identical field sets** (`crates/sky_types/src/unify.rs:259`). There is no
row variable to absorb `routes` / `notFound` (let alone `head` /
`consoleAuth` / `guard` / `status`).

The reference does **not** have a separate `appRouted` at the type level. It
has **one** `Live.app` whose cfg is an **open row-polymorphic record** with
six fields typed (`init, update, view, subscriptions, routes, notFound`) plus
an `appExt` row variable, and it **branches at emit time** to `live_app` vs
`live_app_routed` based on whether the Model record carries a `page` field.

**Our port has already landed 3 of the 5 pieces**, unnoticed by the task
brief:

| Piece | Reference | Our port | State |
|---|---|---|---|
| Runtime `live_app_routed` + `route_resolver` | `runtime-rust/.../live/mod.rs:1113` | `runtime/src/sky_runtime/live/mod.rs:1115` | ✅ ported, byte-parity |
| Runtime `route.rs` matcher (`match_routes`/`match_params`) | `runtime-rust/.../live/route.rs` | `runtime/src/sky_runtime/live/route.rs` | ✅ ported, byte-parity |
| `Live.route : String -> page -> LiveRoute` typing (#106) | `Expression.hs:2826` | `constrain.rs:2937` | ✅ done |
| `emit_live` `Live.route` → `Route::new(pattern, closure)` | `ExprEmitter.hs:1809` | `emit_live.rs:106` | ✅ done (String-only) |
| `Live.app` cfg **open record** (routes/notFound + row var) | `Expression.hs:2674` | `constrain.rs:2911` **closed 4-field** | ❌ **missing** |
| Open-record **unify** rule | `Unify.hs:468` | `unify.rs:259` **exact-set only** | ❌ **missing** |
| `Live.app` **emit branch** (page field → `live_app_routed`) | `ExprEmitter.hs:1670` | `lower.rs:3305` gated `unsupported` | ❌ **missing** |
| Partial-ctor **non-String** payload | punts (assumes String) | `emit_live.rs:135` latent E0308 | ⚠️ **resolve** |

So #108 reduces to **three type-system/lowering changes** — everything
downstream of the cfg record already exists.

---

## 1. Reference spec — `../sky` (with file:line)

### 1.1 Type scheme: one open `Live.app`

`/home/arthur/Documentos/comp/sky/src/Sky/Type/Constrain/Expression.hs:2674-2695`

```haskell
("Live", "app") ->
    Just $ T.Forall ["model", "msg", "page", "e", "req", "appExt"]
        (T.TLambda
            (T.TRecord
                (Map.fromList
                    [ ("init",          FieldType 0 (req -> (model, Cmd msg)))
                    , ("update",        FieldType 1 (msg -> model -> (model, Cmd msg)))
                    , ("view",          FieldType 2 (model -> Html msg))
                    , ("subscriptions", FieldType 3 (model -> Sub msg))
                    , ("routes",        FieldType 4 (List Route))
                    , ("notFound",      FieldType 5 (TVar "page"))
                    ])
                (Just "appExt"))                       -- ← ROW VARIABLE
            (Task e ()))
```

The design comment above it (`Expression.hs:2652-2663`) states the intent
verbatim: the record is **OPEN** so the runtime can accept optional fields
(`guard`, `auth`, `consoleAuth`, `head`, `status`, …) without every app
enumerating empty optionals; closing it would reject apps that supply
extras. This is a **deliberate kernel exception** — ordinary user-written
records stay closed/exact.

**Required vs optional (definitive):**

- **Required** (named in the map, indices 0–5): `init`, `update`, `view`,
  `subscriptions`, `routes`, `notFound`. Every routed and non-routed example
  supplies all six — e.g. `examples/09-live-counter/src/Main.sky:89-90`
  supplies `routes`/`notFound` even for a two-page counter.
- **Optional** (absorbed by `appExt`, never named in the kernel sig):
  `head` (v0.15.58, `sky-stdlib/Std/Live/Head.sky`), `consoleAuth`
  (v0.16.0), `guard`, `auth`, `status`. Added purely by being present in the
  user record and flowing into the row var — **no constraint-surface edit
  was needed for any of them** (that is the whole point of the row var).

Canonical record AST (`../sky/src/Sky/AST/Canonical.hs:159`):
```haskell
TRecord !(Map String FieldType) !(Maybe String)   -- Nothing=closed, Just v=open
```
Solver flat form (`../sky/src/Sky/Type/Type.hs:75`):
```haskell
Record1 !(Map String Variable) !Variable          -- extension is a UF Variable
EmptyRecord1                                       -- closed tail
```

### 1.2 Open-record unification

`/home/arthur/Documentos/comp/sky/src/Sky/Type/Unify.hs:468-512` — `unifyRecords`:

1. Split fields into `shared = fields1 ∩ fields2`, `only1 = fields1 \ fields2`,
   `only2 = fields2 \ fields1`. Unify all `shared` pairwise.
2. `closed1 = isClosedRecordExt ext1` (ext resolves to `EmptyRecord1`),
   `closed2` likewise.
3. **Illegality guard:** `only1` non-empty while `closed2` → mismatch;
   symmetrically for `only2` and `closed1`. (A closed side cannot absorb the
   other's extras.)
4. If `only1` and `only2` both empty → unify `ext1` with `ext2`, merge.
5. Else (both open, differing extras) → mint a **fresh uniquely-named** row
   var `newExt`, merge as `Record1 (fields1 ∪ fields2) newExt`. The fresh var
   absorbs any still-unspecified optionals.

`isClosedRecordExt` (`Unify.hs:505`): a record is closed iff its extension
variable resolves to `EmptyRecord1`.

**Optional-field mechanism, precisely:** a field is optional purely because
it is *absent from the kernel sig and present in the user record*: it lands
in `only2`, the kernel's `appExt` is a flex var (open, not `EmptyRecord1`),
the illegality guard does not fire, and step 5 folds it into the merged
record under a fresh tail. Omitting an optional makes `only2` empty → step 4.

### 1.3 Emit branch — one `Live.app`, two runtime entry points

The reference **already ships a Rust backend**; it is the literal port
target. `/home/arthur/Documentos/comp/sky/src/Sky/Generate/Rust/Builder/ExprEmitter.hs:1670-1745`:

```
Can.Call (VarKernel "Live" "app") [Record fields] ->
  -- Recover Model from view's solver type (view : Model -> Html Msg),
  -- peel alias, look up a `page` field on the Model record.
  case (mModelTy, mPageFieldTy) of
    (Just modelTy, Just (FieldType _ pageTy)) ->
        -- ROUTED: emit live_app_routed with routes vec, notFound,
        -- and a generated set_page closure:
        "move |__page: {Page}, __model: {Model}| {Model} { page: __page, ..__model }"
        => "live_app_routed::<SkyError,_,_,_,_,_,_,_,_>(init, update, view,
             subscriptions, routes, notFound, set_page, storeKind, storePath)"
    _ ->
        -- SINGLE-PAGE: routes/notFound dropped, four TEA callbacks
        => "live_app::<SkyError,_,_,_,_,_,_>(init, update, view,
             subscriptions, storeKind, storePath)"
```

Decision key: **does the Model record have a `page` field?** (recovered from
the solved type of `view`). If yes → routed. `routes`/`notFound` are always
present in the cfg (they are required fields) but are simply not emitted in
single-page mode. There is **no `appRouted` kernel** anywhere in the
reference — grep confirms only `live_app` / `live_app_routed` at the *Rust
emitter + runtime* layer, driven by one `Live.app` surface.

The `init` adaptation (`ExprEmitter.hs:1687-1704`): a req-reading init passes
through; a non-req init is wrapped `move |_r: LiveReq| __sky_init(())` so the
same init also feeds `Tui.app`. (Already relevant to our non-routed path.)

### 1.4 `Live.route` typing + emit + the partial-ctor closure

Type (`Expression.hs:2826`): `route : String -> page -> Route`, fully
polymorphic in `page`. Passing a nullary `HomePage` binds `page := Page`;
passing a partial ctor `AppDetailPage` binds `page := (String -> Page)`.
`Route` is a nominal type with no page parameter, so `List Route` stays
monomorphic regardless of ctor arity.

Emit (`ExprEmitter.hs:1809-1826`) — **ctor arity read from the solver**
(`ecRegionTypes`), NOT hard-coded:

```haskell
ctorArity = length (extractParamTypes (regionType hregion))
closure
  | ctorArity == 0 = "{ let __c = {ctor}; move |_p: Vec<String>| __c.clone() }"
  | otherwise      = "move |__p: Vec<String>| {ctor}("
                       <> intercalate ", "
                            [ "__p.get(i).cloned().unwrap_or_default()" | i <- 0..arity-1 ]
                       <> ")"
in "route::Route::new(&(" <> pattern <> "), " <> closure <> ")"
```

`unwrap_or_default()` yields `String::default()` = `""` for a missing
capture — bounds-safe, never OOB. **The reference assumes every applied
payload is `String`** ("Sky route params are always String"). It has the
same latent non-String hole we do; §6 improves on it.

### 1.5 Runtime `live_app_routed` + resolver

`/home/arthur/Documentos/comp/sky/runtime-rust/src/sky_runtime/live/mod.rs:1113-1160`:

```rust
pub fn live_app_routed<E, Model, Msg, Page, FInit, FUpdate, FView, FSubs, FSetPage>(
    init, update, view, subscriptions,
    routes: Vec<route::Route<Page>>, not_found: Page, set_page: FSetPage,
    store_kind: String, store_path: String,
) -> SkyTask<E, ()>
where ... FSetPage: Fn(Page, Model) -> Model + Send + Sync + 'static
{
    let resolver: RouteResolver<Model> =
        Arc::new(move |m, path| (set_page)(route::match_routes(&routes, &not_found, path), m));
    let param_resolver: ParamResolver =
        Arc::new(move |path| route::match_params(&routes_for_params, path));
    // state.route_resolver invoked on every GET (mod.rs:1227/1243/1273)
}
```

`Page` / `FSetPage` are **erased into the boxed resolver** so `LiveState`
keeps its original 6 type params. This is **already ported verbatim** to
`runtime/src/sky_runtime/live/mod.rs:1115` (confirmed identical signature +
`RouteResolver<Model>` at `mod.rs:362`, `route_resolver` field at `:378`).

---

## 2. Our current state (with file:line)

| Concern | File:line | Current shape |
|---|---|---|
| `Ty::Record` | `crates/sky_types/src/ty.rs:42` | `Record(BTreeMap<Symbol, Self>)` — **closed** |
| `FlatType::Record` | `crates/sky_types/src/ty.rs:250` | `Record(BTreeMap<Symbol, VarId>)` — **closed** |
| `Live.app` scheme | `crates/sky_types/src/constrain.rs:2911-2925` | closed `{ init, update, view, subscriptions }` |
| `routes`/`notFound` symbols | `constrain.rs:200-207,297-298` | interned, `#[allow(dead_code)]`, unused |
| `Live.route` scheme | `constrain.rs:2937` | ✅ `String -> page -> LiveRoute` (#106) |
| Record unify | `crates/sky_types/src/unify.rs:259-275` | exact field-set only; else `mismatch` → SKY-T0001 |
| `Live.app` lower (non-routed) | `crates/sky_lower/src/lower.rs:3264` | 4-field literal → `lower_app_entry_cfg` |
| `Live.appRouted` lower | `lower.rs:3305` | `Err(unsupported(RoutedLiveApp))` → SKY-L0118 |
| `Live.app`/`appRouted` kernel map | `lower.rs:5181-5182` | both registered |
| `emit_live` `Live.route` closure | `emit_live.rs:106-160` | ✅ arity from `variant_fields`; String-only payload |
| `emit_live` E0308 hole | `emit_live.rs:135` | `params.get(i).cloned().unwrap_or_default()` (String) |
| Runtime `live_app_routed` | `runtime/src/sky_runtime/live/mod.rs:1115` | ✅ ported |
| Runtime `route.rs` | `runtime/src/sky_runtime/live/route.rs` | ✅ ported |
| `Feature::RoutedLiveApp` diag | `crates/sky_diagnostics/src/render.rs:617` / `explain/SKY-L0118.md` | "not yet wired" |

**Divergence to retire:** our port invented a **separate `Live.appRouted`
kernel** (`KernelFn::LiveAppRouted`, `Feature::RoutedLiveApp`). The reference
has **one** `Live.app`. The corpus calls `Live.app { …, routes, notFound }`
(hence SKY-T0001 from the *closed cfg scheme*, never SKY-L0118). The routed
gate at `lower.rs:3305` is effectively dead for corpus code. The port should
**converge on the reference's single-kernel + emit-branch model** and treat
`LiveAppRouted`/`RoutedLiveApp`/SKY-L0118 as vestigial (keep as a defensive
alias or delete — see §8).

---

## 3. Open-record scheme we emit in `constrain.rs`

Extend `Live.app`'s cfg to the reference's six required fields **plus a row
variable**. Using `var(n)` = the constraint helper's fresh type vars
(model=`var(0)`, msg=`var(1)`; add page=`var(2)`) and `open_record` (new
helper, §4):

```rust
K::LiveApp => {
    let init_ret = tuple2(var(0), cmd(var(1)));                 // (model, Cmd msg)
    let cfg_rec = open_record(                                   // ← OPEN
        {
            let mut m = BTreeMap::new();
            m.insert(f_init,          fun(live_req(), init_ret.clone()));
            m.insert(f_update,        fun(var(1), fun(var(0), init_ret)));
            m.insert(f_view,          fun(var(0), html_t(var(1))));
            m.insert(f_subscriptions, fun(var(0), sub(var(1))));
            m.insert(f_routes,        list(live_route()));       // List LiveRoute
            m.insert(f_not_found,     var(2));                   // page
            m
        },
        /* row var */ var(3),                                    // ≙ appExt
    );
    fun(cfg_rec, task_unit())
}
```

Notes:
- `f_routes` / `f_not_found` are the already-interned symbols at
  `constrain.rs:297-298` — drop their `#[allow(dead_code)]`.
- `routes : List LiveRoute` reuses `live_route()` (nominal, no type param),
  matching #106's typing — a `List LiveRoute` value unifies structurally.
- `notFound : page` is a fresh var — it does **not** need to equal the
  Model's `page` field for the *type* to check (the reference `notFound` is a
  free `page` too); the emit branch (§5) is what threads the actual Page type
  through `set_page`.
- The row var `var(3)` makes the record open: `head`, `consoleAuth`,
  `guard`, `status`, `auth` all flow into it with **no further
  constraint-surface edits** — exactly the reference property.
- `Tui.app` / `Webview.app` schemes SHOULD get the same open treatment for
  their optional fields (`onKey`, `guard`, `canvasWidth/Height`, `window`),
  but that is out of scope for #108 — flag as a fast-follow so the mechanism
  isn't Live-only.

---

## 4. The open-record mechanism (types + unify)

This is the one genuinely new type-system capability. Two viable scopes:

### Option A (recommended) — faithful port of the reference row var

Mirror `TRecord (Map …) (Maybe var)` / `Record1 map var` / `EmptyRecord1`.

**`ty.rs` changes:**

```rust
// Ty (surface/solver-facing)
Record(BTreeMap<Symbol, Self>, RowTail)          // was: Record(BTreeMap<Symbol, Self>)

// FlatType (union-find structure)
Record(BTreeMap<Symbol, VarId>, VarId)           // extension is a UF var
EmptyRecord,                                      // NEW: the closed tail sentinel

enum RowTail { Closed, Open(TyVar) }              // surface-level tail
```

Every existing `Ty::Record(map)` / `FlatType::Record(map)` construction site
becomes `…(map, RowTail::Closed)` / `…(map, empty_record_var)`. Closed
records (all user records, all other kernel schemes) keep exact-set behaviour
because their tail resolves to `EmptyRecord`.

**`unify.rs` — replace the exact-set arm at `unify.rs:259` with a port of
`unifyRecords`:**

```rust
(FlatType::Record(fs1, ext1), FlatType::Record(fs2, ext2)) => {
    // 1. unify shared fields
    for name in fs1.keys().filter(|k| fs2.contains_key(k)) {
        unify(uf, budget, interner, span, fs1[name], fs2[name])?;
    }
    let only1: Map = fs1 \ fs2;
    let only2: Map = fs2 \ fs1;
    let closed1 = is_empty_record(uf, ext1);
    let closed2 = is_empty_record(uf, ext2);
    // 2. closed side cannot absorb the other's extras
    if (closed2 && !only1.is_empty()) || (closed1 && !only2.is_empty()) {
        return Err(mismatch(...));   // SKY-T0001, precise "unexpected field" msg
    }
    if only1.is_empty() && only2.is_empty() {
        // 3. same field set: unify tails, merge
        unify(uf, budget, interner, span, ext1, ext2)?;
        uf.union(ra, rb, Content::Structure(FlatType::Record(fs1.clone(), ext1)))?;
    } else {
        // 4. both open, differing extras: fresh tail absorbs the union
        let new_ext = uf.fresh_flex();
        // Re-point ext1 and ext2 at records that carry the *other* side's
        // extras + the fresh tail, so future unifications stay consistent
        // (see Unify.hs:496 — the reference re-binds each side).
        uf.union(ra, rb, Content::Structure(
            FlatType::Record(fs1 ∪ fs2, new_ext)))?;
    }
    Ok(())
}
```

`is_empty_record(uf, v)` = follow `v`; `true` iff its content is
`Structure(EmptyRecord)`. Occurs-check: the existing UF occurs-check on
`uf.union` covers the tail var like any other structure var — no new
machinery, but the recursive-record case (`{ a : {a: … } }`) must be
exercised by a test (reference relies on the same UF guard).

**Budget:** `unifyRecords` adds one `unify` per shared field + one for tails;
already counted by the existing per-call budget decrement. No unbounded loop
(field maps are finite; no fixpoint iteration).

### Option B (cheaper interim) — kernel-only "superset accept"

If Option A's blast radius (every `Ty::Record` construction site) is judged
too large for one PR, a **scoped** fallback: add a boolean `open: bool` to
`Ty::Record`/`FlatType::Record` (default `false`), set `true` **only** for
the Live/Tui/Webview kernel cfg schemes, and in unify allow: *if exactly one
side is open, the open side's field set must be a **subset** of the closed
side's* (kernel required-fields ⊆ user-supplied fields), unify shared, ignore
user extras. No fresh-tail merge, no open-vs-open, no `EmptyRecord`.

- **Pro:** ~1 field on the enum, one unify arm, no touch to user-record
  semantics; closes the corpus SKY-T0001 immediately.
- **Con:** not the general row var; open-vs-open (two kernel cfgs unified,
  or a let-bound cfg reused) is unsupported; diverges from the reference
  mechanism → must be recorded in `docs/divergences-from-sky.md` and would be
  re-done when task #56 lands the general row var.

**Recommendation:** **Option A.** It is the reference's proven design, it
also discharges task #56 (row-poly subset/superset), and it keeps a single
code path. Option B is the documented escape hatch only if Lane A hits a
time box.

---

## 5. `Feature::RoutedLiveApp` lowering steps

Converge on the reference's **single `Live.app`, branch at lower/emit**.

1. **Delete the routed gate.** `lower.rs:3264` already handles
   `KernelFn::LiveApp` with a 1-arg record literal via
   `lower_app_entry_cfg` → `lower_app_cfg_record`. Extend
   `lower_app_cfg_record` to accept the six fields (init, update, view,
   subscriptions, routes, notFound) — routes/notFound lower like any record
   field (`routes` is a `List` literal of `Live.route …` calls → §1.4 emit;
   `notFound` is a page expr). No new lower node.
2. **Do NOT branch in lower** — keep lowering shape-preserving. The
   live_app-vs-routed decision is a *codegen* concern (needs the solved Model
   type), so it belongs in `emit_live`, matching `ExprEmitter.hs:1670`.
3. **Retire the `LiveAppRouted` path.** At `lower.rs:3305`, replace
   `Err(unsupported(RoutedLiveApp))` with either (a) route `appRouted` to the
   same `lower_app_entry_cfg` (defensive alias), or (b) remove the
   `("Live","appRouted")` kernel mapping (`lower.rs:5182`), the
   `KernelFn::LiveAppRouted` variant, `Feature::RoutedLiveApp`, and
   SKY-L0118. Prefer (b) for a clean single surface; keep SKY-L0118's
   `explain/` file as a tombstone if any fixture references it.
4. **Emit branch** in `emit_live.rs` (`emit_live_app_inner`): recover the
   Model type from the solved type of the `view` field
   (`view : Model -> Html Msg`), peel any alias, look up a `page` field.
   - **Has `page` field** → emit `live_app_routed::<SkyError,_,…,_>(init,
     update, view, subscriptions, routes_vec, not_found, set_page,
     store_kind, store_path)` where
     `set_page = move |__page: {Page}, __model: {Model}| {Model} { page:
     __page, ..__model }` (Page/Model rust type strings from the solved
     types; parity with `ExprEmitter.hs:1721-1733`).
   - **No `page` field** → current 4-callback `live_app::<…>` emit; drop
     routes/notFound. (This is exactly today's working path.)
   The `routes` field emits as `vec![ Route::new(…), … ]` — each element is
   already handled by `emit_live.rs:106`.
5. **Solved-type access in emit.** The branch needs the Model/Page Rust types
   from the solver. Confirm `EmitCtx` exposes solved/region types (the
   reference uses `ecSolvedTypes` / `ecRegionTypes`). Our emit already
   resolves generics; if the Model record type is not currently threaded to
   `emit_live`, add it to `EmitCtx` (mirror `ecSolvedTypes`). **Lane A: verify
   this plumbing exists before estimating** — it is the one unknown.

---

## 6. Partial-ctor route-payload resolution (the E0308 hole)

Reference and our port both emit
`ctor(params.get(i).cloned().unwrap_or_default())` — a `String` — into every
ctor slot (`emit_live.rs:135`; `ExprEmitter.hs:1823`). For `AppDetailPage
String` that is correct. For `NumPage : Int -> Page` the emitted Rust is
`NumPage(String)` → **E0308**. The reference silently assumes String; we do
**better** (sanctioned divergence — strictly safer, closes a real hole).

**Design: param-type-directed conversion at emit, with a clean diagnostic
for unsupported payloads.** In the partial-ctor branch, `variant_fields(home,
ty, variant)` already yields the payload **field types** (we currently only
take `.len()`). For field *i* of type `T_i`, emit:

| `T_i` | emitted expression |
|---|---|
| `String` | `params.get(i).cloned().unwrap_or_default()` (unchanged) |
| `Int`    | `params.get(i).and_then(\|s\| s.parse::<i64>().ok()).unwrap_or_default()` |
| `Float`  | `params.get(i).and_then(\|s\| s.parse::<f64>().ok()).unwrap_or_default()` |
| `Bool`   | `params.get(i).map(\|s\| s == "true").unwrap_or_default()` |
| other    | **compile-time diagnostic** — reject, don't emit |

The `other` arm is the parse-don't-validate boundary: a `:param` segment is
inherently a URL string; feeding it to a payload the runtime cannot derive
from a string is a **program error**, surfaced as a new lower/emit diagnostic
(propose **SKY-L0119** "a route `:param` can only be applied to a
`String`/`Int`/`Float`/`Bool` page-constructor payload; `NumPage` field 0 has
type `Foo`") rather than an opaque downstream `rustc` E0308. This is caught
where the type is known (emit has the variant field types), so the user sees
a Sky diagnostic, never a Rust one.

- **Graceful degradation** matches the reference for the String case and for
  missing captures (`unwrap_or_default`); the numeric/bool parse failures
  degrade to `0`/`0.0`/`false` (same "never panic" spirit as
  `unwrap_or_default`). Whether malformed numeric segments should instead
  route to `notFound` is a **future** refinement — record it, don't build it.
- Record in `docs/divergences-from-sky.md`: *"Routed page-constructor payload
  typing — Rust backend converts `:param` strings to `Int`/`Float`/`Bool`
  ctor fields and rejects other payload types with SKY-L0119; the Go/Haskell
  reference assumes all payloads are `String` and relies on reflect-coercion.
  Reason: static typing catches the mismatch at compile time (parse, don't
  validate)."*

---

## 7. SKY-I0001 standalone `List LiveRoute`

The task brief lists "a standalone `List LiveRoute` binding ICEs as
SKY-I0001". Per #106 (`constrain.rs:2926-2937`), `Live.route` is already
typed `String -> page -> LiveRoute` where `LiveRoute` is nominal with **no
type parameter**, so `let rs = [ route "/" Home ] : List LiveRoute` is a
homogeneous, monomorphic list — it should already type-check and lower. The
comment at `constrain.rs:2932-2934` explicitly notes "a `List LiveRoute`
stays [monomorphic] … `emit_live_call::LiveRoute` already dispatches
nullary-ctor / …".

**Lane A action:** treat SKY-I0001 as **verify-and-regress**, not
build-from-scratch. Add a fixture: a top-level `routeTable : List LiveRoute`
bound outside the cfg literal, referenced as `routes = routeTable`. If it
compiles → add it as a regression test and close the item. If it still ICEs,
the cause is almost certainly the **emit** side: `emit_live.rs:106`'s
`Live.route` arm assumes `builder_e` is a direct `Expr::Ctor` (arity from
`variant_fields`); a route inside a *let-bound* list still hits the same arm,
but a route whose page builder is an *aliased top-level binding* falls to the
generic `(builder)(params)` closure — which requires the binding to be
`Fn(Vec<String>) -> Page`, which a bare ctor is not. That mismatch (not a row
issue) would surface as an ICE/E0308. Fix: in the generic arm, if the builder
resolves to a nullary page value, wrap as `move |_p| builder.clone()`
(mirror the Ctor arm). **Root-cause the actual failing fixture before
coding** — the brief's SKY-I0001 may predate #106 and already be closed.

---

## 8. Task breakdown for Lane A

Ordered; each item is independently reviewable. **T1–T3 are the type-system
core (seal-touching); T4–T8 are mechanical/oracle-verifiable.**

- **T1 — Open-record types.** `ty.rs`: add `RowTail`/extension to
  `Ty::Record` + `FlatType::Record`, add `FlatType::EmptyRecord`. Update all
  construction sites to `RowTail::Closed` / empty-record var. Build green,
  no behaviour change (all records still closed). *(seal-touching)*
- **T2 — Open-record unify.** `unify.rs:259`: replace the exact-set arm with
  the `unifyRecords` port (§4 Option A). Unit tests: closed=closed exact;
  open⊇closed accept; closed⊋open reject with SKY-T0001; open+open fresh-tail
  merge; recursive-record occurs-check. *(seal-touching — Opus review)*
- **T3 — `Live.app` scheme.** `constrain.rs:2911`: six fields + row var
  (§3); un-`dead_code` `f_routes`/`f_not_found`. Corpus routed cfgs now
  type-check (SKY-T0001 gone). *(seal-touching)*
- **T4 — Retire routed kernel gate.** `lower.rs:3305`/`5182`: remove
  `LiveAppRouted`/`Feature::RoutedLiveApp`/SKY-L0118 (or alias to
  `Live.app`); extend `lower_app_cfg_record` to pass routes/notFound through
  (§5.1–5.3). *(oracle-verifiable)*
- **T5 — Emit branch.** `emit_live.rs`: recover Model type from `view`'s
  solved type, branch on a `page` field → `live_app_routed` (+ generated
  `set_page`) vs `live_app` (§5.4). Confirm/add Model-type plumbing on
  `EmitCtx` (§5.5). *(oracle-verifiable; the `set_page` closure shape is
  parity-checkable against `ExprEmitter.hs:1721`)*
- **T6 — Payload type conversion.** `emit_live.rs:135`: use `variant_fields`
  *types* to emit String/Int/Float/Bool conversions; add SKY-L0119 for other
  payloads (§6). *(oracle-verifiable + one diagnostic)*
- **T7 — SKY-I0001 verify/close.** Add a `List LiveRoute` let-bound fixture;
  root-cause any remaining ICE in the emit generic-builder arm; regress
  (§7). *(oracle-verifiable)*
- **T8 — Docs + divergences + sweep.** Update `docs/divergences-from-sky.md`
  (payload typing), the Live surface docs, and run the example sweep;
  `examples/09-live-counter` (routed) + a routed :param example must build +
  run + match the Go oracle. *(oracle-verifiable)*
- **T9 (fast-follow, not #108) — Open Tui/Webview cfg.** Apply the row var to
  `Tui.app`/`Webview.app` schemes so the mechanism isn't Live-only. File
  separately.

Suggested PR grouping: **PR-1 = T1+T2** (open records, the risky seam, small
diff, heavy tests), **PR-2 = T3+T4** (wire Live.app), **PR-3 = T5+T6**
(routed emit + payload typing), **PR-4 = T7+T8** (verify + sweep).

---

## 9. Seal-touching vs oracle-verifiable

| Item | Class | Why |
|---|---|---|
| T1 open-record types | **Seal-touching** | Changes the core `Ty`/`FlatType` lattice; every record flows through it. |
| T2 open-record unify | **Seal-touching — Opus review required** | New unification rule; soundness of open-vs-closed / open-vs-open + occurs-check on the tail var is exactly the class of change that can silently accept wrong types. Adversarial cases: closed⊋open must reject; fresh-tail must not leak a monomorphic binding across two use sites; recursive record must not loop. |
| T3 Live.app scheme | **Seal-touching (light)** | Correct only if T2 is correct; the scheme itself is mechanical but its acceptance surface is defined by the unify rule. |
| T4 lower gate retire | Oracle-verifiable | Shape-preserving; corpus builds or it doesn't. |
| T5 emit branch | Oracle-verifiable | `set_page`/`live_app_routed` emit is byte-comparable to the reference Rust backend + validated by run-vs-Go-oracle. |
| T6 payload conversion | Oracle-verifiable | Emitted Rust either compiles + runs to the oracle value or fails; SKY-L0119 is a fixed diagnostic fixture. |
| T7 SKY-I0001 | Oracle-verifiable | A fixture compiles + runs or ICEs. |
| T8 docs/sweep | Oracle-verifiable | The sweep is the gate. |

**Opus (security-soundness-guardian) must review T2** (and sign off T1's enum
change + T3's acceptance surface as a set). T4–T8 are Sonnet-implementable
under the standard mechcheck, gated by the example sweep + Go oracle.

---

## 10. Divergences to record

1. **Single `Live.app` + emit-branch** (adopt reference) — retire our
   invented `Live.appRouted` kernel / `Feature::RoutedLiveApp` / SKY-L0118.
   *Convergence toward the reference, not a divergence from it.*
2. **Routed payload typing** (§6) — Rust backend statically converts/rejects
   non-String `:param` payloads (SKY-L0119) where the reference assumes
   String + reflect-coercion. *Sanctioned divergence: static safety, parse-
   don't-validate. Log in `docs/divergences-from-sky.md`.*
3. **Open records = general row var** (Option A) also lands task #56's
   row-poly subset/superset — one mechanism, not a Live-only special case.

---

## 11. Round-4 seal fixes (2026-07-04)

The round-3 adversarial review (`design-coherence-review.md` §1.7/§C4) found
three holes in the landed T1–T7 implementation plus a clippy gate. All four
fixed in round 4; the reviewer's repro fixtures are pinned as goldens.

### 11.1 Hole 1 — bare `Route` rendering (exit-0-then-cargo-fail, E0107)

`IrType::LiveRoute` was nullary and rendered as a bare
`sky_runtime::live::route::Route` — but the runtime `Route<Page>`
(`live/route.rs`) has **no default type parameter**. Reachable via (a) an
empty `routes = []` literal's `Vec::<Route>::new()` turbofish and (b) ANY
let-bound route table's top-level fn signature — the `m7_live_let_bound_routes`
golden itself was skyc-0-then-E0107.

**Fix:** `IrType::LiveRoute(Box<IrType>)` — the page type is threaded from the
solver (`Ty::Con "LiveRoute" [page]`, already 1-arg since Part A) through both
lowerer conversion paths (`ir_type_from_ty` / `ir_type_from_canon`) and
rendered as `Route<Page>` in `emit_types.rs`. All structural walkers
(`collect_record_shapes` / `type_reaches_enum` / `contains_generic` /
`collect_generics` / `match_template` / `ir_contains_fun` / `leaf_of`) descend
into the page argument. Goldens `m7_live_routed_empty_routes_ok` (new) and
`m7_live_let_bound_routes` (extended) now skyc-0 **and cargo-build** under
`SKY_E2E=1` with per-fixture isolated `CARGO_TARGET_DIR`s.

### 11.2 Hole 2 — lambda-view routed app silently emitted non-routed

`routed_page_field` → `model_ty_of_view` matched only `Expr::FuncValue`; a
lambda `view` returned `None` → the emitter silently chose `live_app`,
DISCARDING `routes`/`notFound` (no diagnostic — a silent wrong-accept), and
the L0120 Model gate was skipped (the #95 bypass). The type tier's
`RoutedLiveCheck` meanwhile classified the app as routed — tier disagreement
(review §C4).

**Fix:** the #95-designed `fn_param_ty` (matches `Expr::FuncValue` AND
`Expr::Lambda`, whose params carry solved `IrType`s per `lower_lambda`) landed
in `emit_model_gate.rs`; `model_ty_of_view` routes through it, so the Model
gate and `routed_page_field` inherit Lambda-awareness together. Regressions:
`golden_m7_live_lambda_view_routed.rs` (routed lambda-view → `live_app_routed`
emitted, cargo-0) and `model_admissibility.rs::live_lambda_view_*` (gate fires
/ no false-reject). The #94 Msg gate (SKY-L0121) remains its own follow-up.

### 11.3 Hole 3 — param routes could not type-check (false block)

The Part-A scheme `Live.route : String -> page -> LiveRoute page` shared ONE
variable between the builder argument and the page, forcing
`Page ≟ String -> Page` for `Live.route "/u/:id" UserPage` — SKY-T0001 on the
CANONICAL corpus shape, leaving emit's `route_param_get` path dead.

**Fix — per-route witnessing:** the scheme is now
`String -> builder -> LiveRoute page` (distinct vars); each `Live.route`
reference pushes a `RouteWitnessCheck { builder_var, page_var, span }`
(constrain.rs, same pattern as `RoutedLiveCheck`) discharged post-solve by
`resolve_route_witness_checks` (sky_types/lib.rs), which peels the builder's
settled leading arrows and unifies the result with the page var. Nullary
builders witness the page directly; param ctors witness with their result
type; wrong-ADT ctors still SKY-T0001. Runs BEFORE
`resolve_routed_live_checks` so route ctors pin the page var first.
R1/R2/T4d/T4f/MIX rejections all preserved.

Two supporting changes:

* **Lower peephole** (`lower_route_builder`): a BARE payload ctor
  (`UserPage`) as the builder of a `Live.route` call lowers to a zero-arg
  `Expr::Ctor` carrier instead of tripping `Feature::CtorAsFunction` — in this
  one position the ctor never becomes a first-class function (emit folds it
  into the route builder closure). Mirrors the app-cfg intercept precedent.
* **Emit hardening**: function-shaped builders (named fn / inline lambda) now
  emit per-param `route_param_get` conversions (raw `List String -> Page`
  builders pass the vec through); any OTHER builder shape fails CLOSED with
  the interim `CompilerBug` (upgrades to SKY-L0123 with `route_param_get`'s
  payload arm — ledger §B-route-param). Pre-round-4 that arm emitted an
  untyped `(builder)(params)` that cargo-failed for every realistic shape.

Golden: `m7_live_param_routes` — solo + mixed skyc-0 ∧ cargo-0, wrong-ADT
T0001, and under `SKY_E2E=1` the running binary delivers `GET /u/42` →
`user:42` (param captured through `match_routes` into the ctor).

### 11.4 Gate 4 — clippy

`K::LiveApp` backticked in the `RoutedLiveCheck` field docs
(constrain.rs; `doc_markdown` under `-D warnings`).

### 11.5 Also fixed in passing

`Live.appRouted`'s emit arm still ICE'd (`CompilerBug` "should have been
rejected with SKY-L0118") even though T4 aliased the kernel through
`lower_app_entry_cfg`; it now takes the same `emit_live_app_inner` branch as
`Live.app`. (The alias is still unreachable for corpus code — the constrain
registry excludes it — so this is consistency hardening, not a behaviour
change.)
