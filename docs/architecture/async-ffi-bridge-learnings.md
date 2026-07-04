# Async-FFI auto-bridge — learnings mined from the reference (`../sky`, `feat/runtime-rust`)

> LEARNING-arm deliverable of the async-FFI double-swarm (2026-07-04). Everything
> below is cited `file:line` against `/home/arthur/Documentos/comp/sky` (the
> reference) or this repo. READ-ONLY mining; no builds run.

## 0. Headline correction to our banked assumption

The banked memory ("the reference's advanced FFI **cannot** bind async libs;
shims are the only path") is **stale as of 2026-06-23**. The reference ran a
full wall campaign (#44–#110, WALL-3a/4/5/6/F/G/H/I/J/K) and its auto-FFI now:

- binds foreign `async fn` as Sky `Task Error a` natively (shipped #44, commits
  `1515a035` + `758cdf89` — `../sky/runtime-rust/docs/superpowers/specs/2026-06-23-rust-ffi-async-fn-design.md:9-16`);
- binds **firestore 0.49 directly, shim-free** (owned CRUD + owned query path,
  fixture `../sky/runtime-rust/tests/sky/104-ffi-owned-query-builder/`; the
  `SKY_DCE=0` full-surface residual fell 124 → 10 —
  `../sky/runtime-rust/docs/superpowers/specs/2026-06-27-rust-ffi-73-firestore-dce-residuals.md:8-37`);
- proved **every stripe mechanism on synthetic fixtures** (93/94/95/96) but the
  real `async-stripe` end-to-end build was **never executed** ("HEAVY multi-crate
  cargo build — pure real-crate verification, no remaining mechanism gap" —
  `../sky/runtime-rust/docs/superpowers/specs/2026-06-26-wall-i-stripe-resource-builders.md:403-406`);
- **never migrated `skyshop-rs` off its shims** — the example still ships all
  three wrapper crates and its README still documents the shim architecture
  (`../sky/examples/rust/skyshop-rs/README.md:21-67`).

So the shims are a **pre-#44 fossil** — written when auto-FFI could not model
async at all ("the empirical skyshop-rs-no-shim test (2026-06-23) showed EVERY
real operation of firestore / async-stripe / firebase is `async fn` and
auto-FFI drops them — the shim crates exist ONLY to paper over this",
`2026-06-23-rust-ffi-async-fn-design.md:3-7`). They remain the best spec of the
END-TO-END transform an auto-bridge must automate; the wall campaign is the
best spec of HOW. Our job is to **port + finish** that campaign, not re-derive
it (and not to re-implement the shim's own `block_on` pattern, which the
reference explicitly superseded — §4).

## 1. The shim pattern table

Three crates, `../sky/examples/rust/skyshop-rs/wrappers/` (shared conventions:
`wrappers/README.md:10-19`). All 10 public fns share ONE skeleton:

```
pub fn f(a: &str, …) -> Result<FlatShape, String>   // TOTAL, sync surface
    1. clone every &str param to owned String        ('static-ification)
    2. pure pre-parse (serde_json) outside the future
    3. block_on(async move { client()?; …builder chain….await.map_err(fmt)?; flatten })
```

where `block_on` = spawn OS thread → current-thread tokio runtime →
`rt.block_on(fut)` → `.join()` maps any panic to `Err(String)` (byte-identical
in all three shims: firestore `lib.rs:54-75`, stripe `lib.rs:88-109`, firebase
`lib.rs:59-80`).

### Per-function catalog

| Shim fn (file:line) | SDK call chain wrapped | Signature transform | Class |
|---|---|---|---|
| `fs_get_doc` (firestore `lib.rs:178-196`) | `FirestoreDb::with_options(…).await` → `db.fluent().select().by_id_in(&col).obj::<HashMap<String,String>>().one(&id).await` (2 awaits, serde-generic `obj::<T>`) | `(&str,&str) -> Result<HashMap<String,String>,String>`; `Option<row>` → `_status=ok`-tagged row / `_status=not_found` row (`lib.rs:137-147`) | chain: **M**; Option→status: **J** |
| `fs_set_doc` (`lib.rs:202-219`) | `db.fluent().update().in_col(&c).document_id(&id).object(&fields).execute::<HashMap<..>>().await` | `fields_json: &str` pre-parsed via serde (`lib.rs:205-206`); returns echoed id | **M** (JSON param = the D3 crutch) |
| `fs_delete_doc` (`lib.rs:225-239`) | `db.fluent().delete().from(&c).document_id(&id).execute().await` | `(&str,&str) -> Result<String,String>` | **M** |
| `fs_query` (`lib.rs:244-258`) | `db.fluent().select().from(c).obj::<HashMap<..>>().query().await` | `Vec<HashMap<String,String>>` rows, each `_status=ok`-tagged | **M** |
| `fs_query_where` (`lib.rs:262-285`) | + `.filter(\|q\| build_filter(&q,…))` — closure arg over the builder | op-string → `FirestoreQueryFilter` dispatch (`lib.rs:153-169`: `"!=" \| "neq"` → `not_equal`, … default `equal`) | closure into builder: **M-hard**; op-string mini-DSL: **J** |
| `fs_query_where_order` (`lib.rs:292-324`) | + `.order_by([(field, direction)])` | `dir=="desc"` → `FirestoreQueryDirection::Descending` enum map (`lib.rs:305-309`) | **M** (string→enum via FromStr-like map) |
| `stripe_create_checkout_session` (stripe `lib.rs:127-186`) | `ClientBuilder::new(secret).url(base).build()` → `CreateCheckoutSession::new().mode(Payment).customer_email(e).line_items(v).success_url(s).cancel_url(c).send(&client).await` (builder chain + trait-generic `send<C: StripeClient>`) | 5×`&str` in; `line_items_json` decoded to `Vec<LineItem>` (`lib.rs:53-63,139-143`) then mapped to `CreateCheckoutSessionLineItems` with nested `price_data`/`product_data` (`lib.rs:149-163`); big `CheckoutSession` struct → 3-key flat map, `session.url.unwrap_or_default()` (`lib.rs:177-184`) | chain+send: **M** (= WALL-I/J/K); nested-struct construction from JSON: **M-hard** (serde); `quantity==0→1`, `Currency…unwrap_or(USD)`: **J** |
| `stripe_create_customer` (`lib.rs:190-203`) | `CreateCustomer::new().email(e).name(n).send(&client).await` | `-> Result<String,String>` (just `customer.id.to_string()`) | **M** (the exact WALL-J `send` shape, `wall-i…md:166-177`) |
| `stripe_retrieve_session` (`lib.rs:212-288`) | `RetrieveCheckoutSession::new(id).expand(["customer_details"]).send(&client).await` | `Option<CheckoutSessionStatus>` → `as_str`/`""`; `customer_details` → 8 flat keys via nested `unwrap_or_default` (`lib.rs:243-268`); email fallback chain (`lib.rs:272-276`) | expand+retrieve: **M**; Option-flattening to `""`: **M** (mechanical rule: `Display`-or-empty); email fallback: **J** |
| `fb_verify_id_token` (firebase `lib.rs:152-188`) | emulator: `App::emulated()` + `id_token_verifier()`; live: `App::live().await?` + `id_token_verifier()?`; both → `verifier.validate(&token).await` → `HashMap<String, serde_json::Value>` claims | claims → flat strings via `claim_str` (`lib.rs:85-91`); `sub`→`uid` remap + required-claim check (`lib.rs:161-165`); optional claims copied when present (`lib.rs:177-184`) | validate chain: **M**; dual-path selection + claim remap/policy: **J** |

**Cross-cutting transforms** (every shim):

| Transform | Where | Verdict |
|---|---|---|
| `&str` → owned `String` clone before `async move` ('static-ification; no Arc used anywhere — clone-into-owned suffices) | e.g. stripe `lib.rs:134-137`, firestore `lib.rs:179-180` | **M** — generator already does this: `ownRefIdx` "(A) ASYNC/effectful: `&Opaque` would ESCAPE (E0521) → strip `&`, re-borrow at call site" `../sky/src/Sky/Build/Rust/Ffi.hs:757-800` |
| dedicated-thread `block_on` async→sync bridge | 3× identical | **M but superseded** — native await-as-Task is strictly better (§4) |
| panic containment via `.join()` → `Err(String)` | 3× | **M** — superseded by `tokio::task::spawn` JoinError arm (`Ffi.hs:1002,1006`) |
| error mapping: backend `Display` embedded verbatim in `Err(String)` (`stripe_err` `lib.rs:113-115`; firestore `lib.rs:190` etc.) | 3× | **M** — and the auto path is SAFER: `sky_error_from_foreign` redacts + correlation-id logs (§4), whereas the shims leak transport `Display` (can carry URLs/tokens) |
| `_status` key in the Ok payload because "the Sky FFI `Err` payload is unusable" (`wrappers/README.md:15-19`) | 3× | **J-as-workaround** — the auto-bridge must instead make the error channel usable (typed `Task Error a` + `Error.toString`); `Option<T>` returns should bind as `Maybe`, not a status key |
| client construction from env (`STRIPE_API_KEY`/`STRIPE_API_BASE` `lib.rs:68-80`; `FIRESTORE_PROJECT_ID` `lib.rs:43-47`) | per crate | **J** — app config; auto-bridge just binds `ClientBuilder::new`/`with_options` and lets Sky code thread the env (`System.getenv`) |
| `EmulatorTokenSource` — a hand `impl Source for …` foreign-trait impl to bypass ADC (`firestore lib.rs:82-93`) + dev-mode gate refusing emulator paths in prod (`lib.rs:108-113`; firebase `lib.rs:109-114`) | firestore, firebase | **J — genuinely not automatable**: user-written trait impls + security policy. The only shim content with no auto-bridge answer; belongs in Sky-side or a stdlib helper |
| numbers/bools stringified at the boundary ("D3 flat shape", `README.md:43-49`) | 3× | **J-as-workaround** for pre-typed-FFI days; auto-bridge's serde→JSON-String surface (#47) or typed records replace it |

**Bottom line:** ~80 % of shim content is mechanical and the reference already
automated it. The judgment residue is (a) data-shaping policy (flat rows,
defaults, fallbacks), (b) env/config/security wiring, (c) one foreign-trait
impl. (a) shrinks to near-zero with a typed/JSON return surface; (b)+(c) are
Sky-program concerns, not bridge concerns.

## 2. The exact async rejection sites (inspector)

The inspector does **not** drop async wholesale — it classifies and gates.
Reference: `../sky/tools/sky-ffi-inspect-rs/src/main.rs` (16 853 lines); our
vendored copy `tools/sky-ffi-inspect-rs/src/main.rs` (18 500 lines) is a
superset with identical machinery at shifted lines (both carry WALL-K's
`TRAIT_ID_CANON_PATH`).

- **Detection**: `header.is_async` flag (struct field doc: vendored
  `main.rs:200-208`) + `is_future_type` (`Pin`/`Future`/`BoxFuture` heads,
  vendored `main.rs:6294-6302`). `classify_effect` maps `is_async → "effectful"`
  (reference `main.rs:5961`; vendored `main.rs:6277-6292`). So async is emitted
  **with the effect marker**, generator binds it as `Task` — the inspector never
  drops for async-ness itself.
- **De-async of macro/RPITIT sugar**: `de_async_clone` unwraps the
  `#[async_trait]` desugar `Pin<Box<dyn Future<Output=T>+Send>>` → `T` +
  `is_async=true` (reference `main.rs:11007`; vendored `main.rs:11552-11633`,
  WALL-4 #64 + RPITIT #81). A `?Send` async-trait shape returns `None` → drop
  `async-future-not-send` (vendored `main.rs:11552-11602`).
- **The three drop gates** (drop reason `async-future-not-send`, fail-closed —
  tokio's multi-thread `spawn` requires `Send + 'static`):
  - **C1 output gate** — Ok-type must be provably Send (primitives + String +
    Send-seq + proven-Send opaque handle); note "Sky-coercible admits
    `Clone + !Send`… the async gate is TIGHTER" (vendored `main.rs:4281-4361`;
    reference drop at `main.rs:4156`).
  - **C1b param gate** — every `async move`-captured param (vendored
    `main.rs:4364-4424`; reference `main.rs:4165-4213`).
  - **C1c receiver gate** — the by-move-captured `self` (vendored
    `main.rs:4426-4451`; reference `main.rs:4221-4239`), backed by the frozen
    owning-crate Send verdict B3 (`send_ok`, vendored `main.rs:820-823`) and
    `recv_provably_async_send` (Send-supertrait / explicit `impl Send` /
    all-fields-Send proof — vendored `main.rs:3057-3060` comment).
- **Non-async but async-adjacent drops that actually blocked the SDKs**:
  - `touches_lifetime` (reference `main.rs:3774`) — drops every borrowing
    fluent builder (`fluent(&self) -> FirestoreExprBuilder<'_,D>`); guardian
    ruled auto-binding the borrow UNSOUND and routed to the owned params API
    instead (WALL-6, §3).
  - `unmodellable-bound` — `classify_param_bound` (reference `main.rs:7109`)
    dropped `send<C: StripeClient>` until WALL-K widened the cross-crate trait
    resolution (`wall-i…md:314-345`).
  - `undeclared type-var Self` — dropped inherent `send` returning
    `<Self as StripeRequest>::Output` until WALL-J's sibling-impl assoc
    resolution (`wall-i…md:193-215`).
  - **Feature invisibility** (#89): `cargo rustdoc --all-features` is rejected
    for external deps, so every feature-gated API (ALL stripe `Create*` builders
    + `send`) was invisible; fix enumerates features via `cargo metadata` and
    injects through the dep table — surface jumped 92 → 20 358 symbols
    (`wall-i…md:179-191`).

## 3. What the reference tried / shipped / left unfinished

Chronology (all under `../sky/runtime-rust/docs/superpowers/specs/`):

1. **Shims first** (pre-2026-06-23, Stages 1-4 stub→real) — because auto-FFI
   "couldn't model async at all" (`2026-06-23-rust-ffi-async-fn-design.md:80-84`).
2. **#44 async→Task** (design 2026-06-23, shipped): bind as `Task`, await
   natively on Sky's ambient tokio runtime — explicitly REJECTING the shim's
   block_on ("tokio block_on inside an already-running runtime PANICS…; native
   `.await` inside a Task has NO such hazard", `…async-fn-design.md:104-111`).
   Two shipped divergences from design: split Send predicates (Clone ≠ Send)
   and a global panic hook + `tokio::spawn`'s catch instead of per-await
   `catch_unwind` (`…async-fn-design.md:9-16`).
3. **The wall ladder to firestore**: WALL-3a serde trait methods, WALL-4
   `#[async_trait]` desugar, WALL-5 ground ctor, WALL-6 owned-query
   circumvention of borrowing builders ("dropping the borrowing fluent builders
   costs ZERO capability — bind the owned API the fluent layer is built on",
   `2026-06-27-rust-ffi-wall6-selfref-builder-design.md:13-33`), #100 feature
   propagation, #105/#106/#109/#110 root-cause fixes → **10 complex crates
   genuinely shim-free** + firestore direct
   (`2026-06-27-ffi-10-complex-crate-coverage-scorecard.md:39-54,116-122`).
4. **The stripe arc — mechanisms done, proof pending**: WALL-G/H (cross-crate
   unique-impl mono + structural Send), WALL-I (customize-chain: provided-method
   projection + `Self::Output`), WALL-J (inherent `<Self as Trait>::Output`
   sibling-impl resolution; Stage 2+3 composed with **zero extra code** on the
   exact stripe `send` shape, fixture 95 — `wall-i…md:298-308`), WALL-K
   (external-trait 3-crate triangle, fixture 96). Remaining: the real
   async-stripe multi-crate cargo build, never run (`wall-i…md:403-406`).
5. **Abandoned/rejected paths** (each with reason):
   - shim-style per-call `block_on` — panics inside a running runtime (§2 above);
   - auto-binding lifetime-carrying builders — foreign covariance unverifiable
     at codegen ⇒ unsound (WALL-6 guardian, fixture 104 header
     `…104-ffi-owned-query-builder/src/Main.sky:5-9`);
   - admitting `Clone`-opaque outputs through the async gate — `Clone ≠ Send`
     E0277 class (`…async-fn-design.md:33-40`);
   - `spawn_local`/current-thread forking for non-Send futures — "a non-Send
     future could still reach `task_parallel`" (`…async-fn-design.md:40`);
   - raw `{e:?}` in Sky-visible errors — B8 security block, fixed by redaction
     (`wall-i…md:254-261`).
   - Known open tail: firestore `SKY_DCE=0` residual 10 (UFCS private-module /
     lifetime-leak / return-borrow classes, `…73-firestore-dce-residuals.md:85-132`);
     rusqlite `Connection` is `!Send` ("likely needs a wrapper") and csv's
     `Reader<R>` is IO-generic (`scorecard:112-114`).

## 4. The reference's OWN async-kernel bridging idioms (runtime-rust)

- **Entry boundary only** uses block_on: `sky_runtime/task.rs:5-18` —
  multi-thread `tokio::runtime::Runtime::new()`, future driven on a spawned OS
  thread, `.join()` → `Err` ("async task panicked"). Webview variant
  `block_on_current_thread` (`task.rs:47-60`) for the macOS main-thread rule.
  This is what the shims copied — appropriate at the PROGRAM entry, wrong
  per-FFI-call.
- **Kernels return futures, never block**: `Http.get` is
  `pub fn http_get<E: From<String>+Send+'static>(url: String) -> SkyTask<E, HttpResponse> { Box::pin(do_request(...)) }`
  (`sky_runtime/http_client.rs:345-355`) — owned params, `Box::pin(async move …)`,
  errors via `format!(…).into()` through `E: From<String>`, body-size caps
  in-stream (`http_client.rs:323-341`). `Task.parallel` uses `tokio::spawn`
  (`task.rs:199`), which is what forces the Send gates.
- **Generated FFI wrappers mirror this**: effectful wrapper type
  `SkyTask<retInner>` (`../sky/src/Sky/Build/Rust/Ffi.hs:856-858`), body
  `Box::pin(async move { match tokio::task::spawn(async move { call.await }).await { Ok(Ok(v)) => ok_res(coerce v), Ok(Err(e)) => Err(sky_error_from_foreign(e)), Err(_) => Err("foreign async call panicked") } })`
  (`Ffi.hs:997-1006`; instance/generic form `FfiInstance.hs:716-754`, serde
  prelude spliced INSIDE the async block `FfiInstance.hs:820-825`).
- **Error redaction (B8, load-bearing security)**: `sky_error_from_foreign`
  logs the raw `Debug` server-side under an 8-hex correlation id and returns
  only `external operation failed (ref <id>)`
  (`runtime-rust/src/sky_runtime/core.rs:54-59` + `log_foreign_error:67+`) —
  the shims' verbatim-`Display` embedding is the LESS safe pattern.

## 5. DCE state (Rust side)

The Rust backend has whole-program DCE **including FFI**, matching Go:

- **Program-level**: dep modules + entry module pruned to the
  transitively-reachable-from-`main` set (`Dce.TopRef`), `SKY_DCE=0` / empty
  set → keep-everything fallback (`../sky/src/Sky/Build/Compile.hs:5178-5233`).
- **FFI wrapper level (S4)**: `sky add` writes ALL wrappers into the cached
  `<slug>_bindings.rs`, each bracketed by `// SKY-FFI-WRAPPER BEGIN <ref>` /
  `END` sentinels (`../sky/src/Sky/Build/Rust/Ffi.hs:243-258`); at build time
  `writeFilteredBindings` drops unreached regions keyed by
  `Dce.FfiRef kernelName wrapperRefName` — one SSOT name fn makes key/item
  divergence "impossible by code structure" (`Ffi.hs:222-240`;
  `../sky/src/Sky/Generate/Rust/Project.hs:499-540`). FULL-EMIT fail-safes on
  missing kernel.json / sentinel-bijection failure.
- **Generic-wrapper synthesis is demand-driven**: only REACHED generic FFI fns
  are synthesised; an unreachable unmodellable fn must NOT block the build
  (`Compile.hs:5250-5266`).
- **Scale evidence**: DCE is what makes 76k-symbol crates viable — stripe's
  full visible surface is 20 358 symbols (3 534 bound) post-#89, and firestore's
  862 wrappers tree-shake to the used subset; "a SKY_DCE=0 residual ≠ a
  default-DCE main-fn blocker" (bytes lesson, `scorecard:78-83`). Feature sets
  are propagated inspector→kernel.json→generated Cargo.toml incl. git deps
  (Part B #100, `…73-firestore-dce-residuals.md:10-33`).

## 6. Verdict

1. **Mechanically automatable (reference already proved it)**: async→Task
   native-await wrapper + tokio-spawn panic containment; `&str`/`&Opaque` →
   owned-clone 'static-ification; Result-flatten + redacted foreign-error
   mapping; owned builder-chain threading (`self`-returning `with_*`);
   string→enum via FromStr bridges; serde-generic returns as JSON-String
   surface; feature enumeration/propagation; sentinel-region FFI DCE. That is
   ~80 % of what the shims hand-wrote.
2. **Genuinely judgment (leave OUT of the bridge)**: flat-row/`_status` data
   shaping (obsolete once errors are typed and `Option`→`Maybe`), env/client
   config policy, emulator/security dev-gates, and user `impl ForeignTrait`
   (firestore's `EmulatorTokenSource`) — these live in Sky code or stdlib, not
   in generated glue.
3. **Solve-first order implied by the reference's scars**: (a) the #44
   async→Task gate with the STRICT tri-gate Send discipline (output+param+
   receiver; `Clone ≠ Send`) and `#[async_trait]`/RPITIT de-async — everything
   else layers on it; (b) B8 error redaction BEFORE any real network SDK binds;
   (c) never block_on-per-call — one ambient runtime; (d) lifetime-carrying
   builders: drop-and-route-to-owned-API, don't bind borrows; (e) feature
   visibility + propagation before judging any SDK "unbindable"; (f) DCE
   sentinels + demand-driven synthesis from day 1 at 76k-symbol scale.
4. **Cheapest win available to us**: the campaign is already vendored — our
   inspector superset carries every wall through WALL-K; the missing half is
   the HASKELL generator port (`Ffi.hs`/`FfiInstance.hs`/`FfiCall.hs` async
   arms cited in §4) plus the un-run real-stripe verification.
5. **Exceeding the reference = finishing two things it left open**: run the
   real async-stripe end-to-end build (mechanisms complete, proof absent) and
   migrate skyshop-rs' three shims to direct binds (firestore's path is already
   green in fixture 104; firebase mirrors firestore's async-trait shape).
