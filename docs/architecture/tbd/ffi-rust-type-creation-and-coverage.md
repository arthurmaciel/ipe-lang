# FFI: creating Rust types from Ipê + crate-coverage roadmap

Status: design (tbd). Scopes what is soundly buildable for the "define a Rust
type from Ipê" blocker and defers what is not.

The existing FFI subsystem (`ffi-subsystem-design.md`, `ffi-sandbox-and-generator-impl-ready.md`,
ADR-0033) is **consume-only**: the inspector introspects a crate's existing
symbols and the generator emits `_bindings.rs` wrappers that *call into* them.
Nothing in the pipeline lets Ipê **define** a Rust-side type or hand a Rust
API an Ipê function as a callback. That gap blocks every crate whose API demands
a user-supplied type — a Bevy `Component`, an Iced `update` fn, an Axum handler,
a `dyn Fn(A) -> B + Send + Sync`. This document specifies a sound mechanism for
it and a priority-ordered crate roadmap.

Fenced code blocks are **illustrative** unless the prose says a fixture ran.

---

## 0. What "create a Rust type from Ipê" must mean, precisely

There are two distinct blockers. Keep them separate:

* **(a) define a Rust type** — the crate's API needs a *nominal Rust type the
  crate does not itself provide*: a `struct` implementing the crate's trait, an
  `enum`, or a closure value of an exact `Fn` signature. Today the generator
  can only name types that already exist in the inspected crate.
* **(b) lifetime-borrowed opaque returns** — a foreign fn returns `&'a T`
  borrowing an input. The whole subsystem is **owned-only** by construction
  (`own_ref_idx` strips `&` from params so nothing borrows across the wrapper
  boundary; `translate_rust_ret` turns every `&T`/`&str` return into an owned
  `.to_owned()`/`.to_string()`). A borrowed return has no owner to attach a
  lifetime to on the Ipê side.

The recommendation resolves (a) with a new **provide** surface, and resolves
(b) by **elevating owned-only from an implementation accident to an invariant**:
borrowed opaque returns stay refused (over-drop), permanently and on purpose.
Ipê values are immutable and have no lifetime algebra; a `Rust.` type Ipê can
construct is therefore always `'static`-owned. This is a *correctness* win
(§PRINCIPLES), not a completeness regression — the alternative (surfacing Rust
lifetimes into Ipê) would breach Soundness.

---

## 1. The trust gate this must live behind (non-negotiable)

`canonicalise_foreign_call` (`src/compiler/canon/src/resolve.rs:3657`) is the
subsystem's keystone: a `Ffi.binding "<wrapper>" args` node — the only bridge
from Ipê into Rust — is minted **only** inside a module whose origin is
`ModuleOrigin::FfiInterface` (`resolve.rs:408`). User source can never mint a
`ForeignCall`; an arbitrary wrapper identifier outside a driver-vouched
interface module is unrepresentable.

**Every type-creation surface MUST inherit this gate.** A user cannot be given
a free-form "write Rust here" construct in ordinary `.ipe` source — that would
route untrusted text into emitted Rust outside the two typed decode boundaries
(`PkgInfo` decode, `Call` decode) and destroy the SEAL. Whatever the surface
looks like, it must compile down to entries the *driver* generates into the
`FfiInterface` module + `_bindings.rs`, keyed off `wrapper_ref_name`, subject
to the same over-drop / reject-at-decode discipline.

This single constraint eliminates the most tempting-but-unsound designs (see
§4, approach C).

---

## 2. Recommended surface: a declarative `provide` block in the crate manifest

Ipê defines a Rust type **declaratively**, in the crate's own `ipe.toml`
`[rust]` section, not with new expression syntax in user `.ipe` files. The
declaration is author-authored, shown to the user under informed-consent at
`ipe add` (it is *native* code, so it declares capabilities like any other
`Rust.` surface — `package-coordination-and-capabilities-design.md`), and the
driver compiles it into new `FnShape` variants.

Two provide-forms cover the crate roadmap:

### 2.1 `provide.struct` / `provide.enum` — an Ipê record/union becomes a Rust type

```toml
# ipe.toml  (illustrative)
[rust]
dependencies = { iced = "=0.12.1" }

[[rust.provide.struct]]
# The Rust type name to define, and the trait it satisfies for the crate.
name    = "Counter"
derives = ["Default", "Clone"]        # closed allowlist, see §3.2
# field name -> Rust type, each an owned, Ipê-coercible carrier
fields  = { value = "i64" }
```

The driver emits, into `_bindings.rs`, a concrete definition plus a
constructor wrapper — the constructor is exactly the existing `EnumCtor` /
`FieldSet` inbound path (`owned_value_coercion`, `bindings.rs:1408`) generalised
to a struct literal:

```rust
// generated (illustrative)
#[derive(Default, Clone)]
pub struct Counter { pub value: i64 }

// IPE-FFI-WRAPPER BEGIN counter_new
pub fn iced_counter_new(arg0: i64) -> Counter { Counter { value: arg0 } }
// IPE-FFI-WRAPPER END
```

On the Ipê side this is one opaque nominal `type Counter = Counter` in the
`Rust.Iced` interface plus a `counter_new : Int -> Counter` forwarder — the
exact shape opaque foreign types already take (`interface.rs` opaque path). The
Ipê program never sees the struct's internals; it holds an opaque handle. This
solves (a) with **zero new trust surface**: the constructor body is built from
decode-validated newtypes only, identical to today's `EnumCtor`.

> **Wired.** The interface now ADMITS the `provide.struct` / `provide.enum`
> constructors as Ipê forwarders (the opaque nominal + `counter_new` /
> per-variant `message_new_*` bindings), not just emits the `_bindings.rs`
> definition. A provide-defined type resolves at the crate-absolute path
> `crate::ffi::<slug>::<Name>` — it lives in the emitted app crate, not an
> external `::crate::Path`. The nominal-name-vs-inspected-opaque clash fails
> closed, a name shadowing an Ipê builtin over-drops the whole entry, and a
> nullary constructor (a fieldless struct / unit variant) binds a zero-arg
> forwarder. The `provide.closure`-to-`run` handoff — surfacing a boxed Ipê
> closure as an Ipê-held value to pass onward — stays deferred (a separate,
> harder step).

### 2.2 `provide.closure` — an Ipê function becomes a `dyn Fn` of an exact signature

```toml
[[rust.provide.closure]]
# The wrapper takes an Ipê function value and returns a boxed Rust closure of
# an EXACT, author-declared signature. No inference from crate metadata.
name     = "update_fn"
signature = "dyn Fn(Counter, Message) -> Counter + Send + Sync + 'static"
# The Ipê arrow the caller must supply, mapped component-wise to the signature.
ipe_arg  = "(Counter -> Message -> Counter)"
```

This is the crux for Bevy systems, Iced `update`, Axum handlers. The generator
emits an adapter that closes over the Ipê function value and re-enters the Ipê
evaluator per call. See §3.3 for the soundness argument (the hard part).

### Why declarative-in-manifest, not new `.ipe` syntax

* It stays **greppable at the `Rust.` boundary (D1)**: everything Rust-side is
  under `[rust.provide.*]` in one file, mirroring `[rust.dependencies]`.
* It is **author-declared native code** → it flows through the existing
  capability-consent model unchanged; the user sees it at `ipe add`.
* It keeps user `.ipe` source **free of any Rust-text injection point** — the
  SEAL and the "user can't mint a ForeignCall" gate are untouched.
* Invalid states are unrepresentable: every `name`/`fields`/`signature` value
  decodes through a validating newtype (§3.1) before any emission, exactly like
  `CrateName`/`RustIdent`/`FeatureName` do today.

---

## 3. Soundness design

### 3.1 A third-and-fourth decode boundary — same discipline

The subsystem has exactly two typed decode boundaries today (`PkgInfo`, `Call`).
`provide` adds decoding of the manifest `[rust.provide.*]` tables. To preserve
"parse, don't validate", these decode through the SAME newtype gates:

* `name` → `RustIdent` (`naming.rs`), already `^[A-Za-z_][A-Za-z0-9_]*$`.
* struct field names → `RustIdent`; field types → a **closed** carrier set
  (`i64|f64|bool|char|String|Bytes|<opaque-already-in-this-crate>`), the same
  set `owned_value_coercion` can already lift. Anything else → refuse the whole
  `provide` entry at decode (over-drop the type, never emit-and-cargo-fail).
* closure `signature` → a parsed `ClosureSig { params: Vec<Carrier>, ret:
  Carrier, bounds: BoundSet }` where `BoundSet ⊆ {Send, Sync, 'static}` is a
  closed enum, never free text. A signature that does not parse into this shape
  is refused. **No raw string from the manifest ever reaches emitted Rust** —
  the emitter renders from the parsed `ClosureSig`, exactly as `render_dep_line`
  renders from `CrateVersion`/`FeatureName` and never from raw input.

New `FnShape` variants (the anti-drift registration site,
`pkginfo.rs:174`):

```rust
FnShape::StructCtor { fields: Vec<(RustIdent, Carrier)>, derives: DeriveSet },
FnShape::ClosureAdapter { sig: ClosureSig, ipe_arity: usize },
```

Both decode in `decode_shape` (`pkginfo.rs:806`) and get a `Fallibility` arm
(`pkginfo.rs:849`) — `StructCtor` is `Infallible` (a total literal), and
`ClosureAdapter` is `Infallible` at *construction* (building the box cannot
fail; per-call failure is handled in-band, §3.3).

### 3.2 `derives` is a closed allowlist re-verified against the runtime

The `MODELLABLE_5` fence (`{Hash, Eq, Ord, Clone, Default}`) already exists as a
two-way cross-crate assertion. `provide.struct.derives` reuses it verbatim: a
derive outside the set is refused at decode. This keeps the emitted `#[derive]`
list sound (a struct with an `f64` field can derive `Clone`/`Default` but never
`Eq`/`Hash`/`Ord` — the IEEE-754 cell the fence already guards).

### 3.3 The closure adapter — the hard soundness core

A `provide.closure` wrapper must turn an Ipê function value into
`Box<dyn Fn(A) -> B + Send + Sync + 'static>`. Four obligations, each met by a
gate that already exists or a narrow new one:

1. **Captured Ipê value must be `Send + Sync + 'static`.** The Ipê function
   value is a heap closure over an immutable environment. The runtime's value
   representation (`ipe_runtime`) is already `Send + Sync` for the pure fragment
   (it must be, for `Task`/tokio to move work across threads — see the async
   bridge's C1/C1b/C1c Send gates). The adapter captures the value by move into
   the box; `'static` holds because Ipê values own their environment (no
   borrows). **Gate:** refuse a `provide.closure` whose `ipe_arg` references any
   non-`Send` carrier (there are none in the closed carrier set — every carrier
   is an owned value or an opaque handle that is itself `Send` by the async
   bridge's `PROVABLY_SEND_OPAQUE_NAMES` verdict). Over-drop otherwise.

2. **Per-call re-entry into the evaluator is panic-isolated.** Each `Fn` call
   coerces the Rust args inbound (the existing param coercion, run in reverse of
   `arg_call`), invokes the Ipê function through the runtime apply entrypoint
   inside `std::panic::catch_unwind`, and coerces the result outbound. A panic
   (or an Ipê `Task` error, if the signature's `ret` is fallible) becomes the
   signature's failure value — for a `-> B` closure the crate contract must
   admit a total `B`; for a `-> Result<B, E>` closure the error folds through
   `ipe_error_from_foreign`. **A closure whose Rust signature return is neither
   a total carrier nor a `Result`/`Option` is refused** (there is no sound value
   to yield on failure). This is the single new soundness rule.

3. **Re-entrancy / runtime context.** The Ipê evaluator apply path must be
   callable from an arbitrary Rust thread (a Bevy system thread, a tokio worker).
   The process-global runtime `OnceLock` the async bridge already introduces
   (`ffi-sandbox-and-generator-impl-ready.md` §4, H1) supplies the context.
   **Async-returning closures are now supported** (see §3.5): the declared
   return is a `Future`, the Ipê value carries a concrete `IpeTask`-shaped boxed
   future, and the adapter awaits it under a spawned task guarded by
   `AbortOnDrop`. A `tokio` runtime context must exist at poll time (the crate's
   own executor — Axum/Hyper — supplies it), exactly as the async bridge's
   spawned wrappers require.

4. **`Clone` for multi-call `Fn`/`FnMut`.** A `dyn Fn` may be called many
   times; the captured Ipê value is `Clone` (runtime values are refcounted /
   structurally clonable). `FnOnce` needs no clone. This mirrors the existing
   `closureNeedsClone` capture gate the async bridge specifies.

### 3.4 SEAL and sandbox interaction

* **SEAL** (`ipe build ⇒ cargo build`): the new shapes are total functions over
  a decode-validated `provide` spec, emitted into sentinel regions like every
  other wrapper. A `provide` entry that cannot render soundly emits **nothing**
  (over-drop) and appears in `coverage.md`. No path emits Rust that cargo then
  rejects.
* **Sandbox** stays fail-closed and untouched: `provide` types are compiled in
  the same jailed `cargo check`/`rustdoc` inspection as the rest of the crate.
  Defining a struct/closure does not widen the syscall surface — the capability
  set is still the union of inferred-Ipê + author-declared-native, enforced at
  the OS boundary. A `provide.struct` holding, say, a `File` handle would
  require the `filesystem` capability declared by the author and shown at add,
  exactly like any other native surface.
* **Capabilities**: crossing into a `provide` type is a `Callee::Ffi`, so it
  already contributes `Capability::NativeFfi` (`lower/src/capabilities.rs:6`).
  No new capability kind is needed for the struct/enum case; the closure case
  inherits whatever the crate's trait impl transitively needs.

---

### 3.5 The async-returning closure adapter (Axum/Hyper handlers)

An async `provide.closure` declares a `Future`-returning signature — the shape
an Axum/Hyper route handler needs (`Fn(Request) -> impl Future<Output =
Result<Response, E>>`). Three author spellings decode to the same shape:
`impl Future<Output = R>`, `Pin<Box<dyn Future<Output = R> + …>>`, and
`BoxFuture<'static, R>`. The soundness core rests on one fact:

* **The received box type IS the `Send + 'static`-across-await proof.** The
  runtime type `IpeTask<E, A> = Pin<Box<dyn Future<Output = IpeResult<E, A>> +
  Send + 'static>>`. So an async Ipê fn value on the app side is already typed
  `Box<dyn Fn(A…) -> Pin<Box<dyn Future<Output = CrateRet> + Send + 'static>> +
  Send + Sync + 'static>`. The inner future's `Send + 'static` is part of the
  received box type — rustc discharges it at the wrapper boundary. The adapter
  never re-derives a Send proof; if the captured Ipê environment or an argument
  were not `Send + 'static`, the received box type would not type-check. A
  lifetime-borrowed capture therefore cannot escape into the `'static + Send`
  future by construction.

The adapter receives and returns the SAME concrete boxed-future type (so the
opaque paths on both sides can never disagree — an E0308 otherwise), Arc-wraps
the captured value for multi-call `Clone`, and per call:

1. produces the future by calling the Ipê fn under `catch_unwind` — a
   *production* panic folds to an immediate-error future (`Err`/`None`);
2. returns a boxed future that `tokio::task::spawn`s the produced future and
   awaits its `JoinHandle`, so a *poll* panic surfaces as a `JoinError` folded
   to `Err`/`None`. Only `{JoinHandle, AbortHandle}` cross the await point — the
   `Arc` is consumed producing the future *before* the await, so the returned
   future stays `Send` even though `IpeTask` is `!Sync`.
3. arms an `AbortOnDrop` guard over the spawn (async-bridge §1.1 Δ1): a dropped
   outer task (a cancelled request) aborts the inner one, so no Ipê side effect
   runs after cancel.

**Async returns are always fallible.** There is no `AsyncTotal` shape — a
poll-panic has no synchronous frame to `catch_unwind` and only surfaces as a
`JoinError`, which needs an error channel to fold into. A total async return
would leave a poll-panic to either abort the whole executor (a remote DoS from
inside a request handler) or launder the panic into a `Default` — both refused.
`ClosureRet` makes async-total **unrepresentable**, so the emitter can never be
asked to produce it; the decode boundary over-drops the whole entry.

## 4. Approaches weighed

**A. Declarative `provide` in the manifest (RECOMMENDED).** Trust gate intact,
SEAL intact, reuses `EnumCtor`/`FieldSet` inbound coercion + the async bridge's
Send/Clone gates. Cost: a closed, declarative surface — it cannot express an
arbitrary hand-written Rust `impl` body. That is the point: everything it emits
is a total function of validated data. Justified against PRINCIPLES —
Security (no new RCE/injection surface), Correctness (owned-only invariant),
Soundness (closed carriers + closed bounds) all hold; Completeness is
deliberately bounded.

**B. Inspector-inferred type synthesis (a `#[ipe::provide]` proc-macro the
author writes in a companion Rust crate).** The author writes real Rust; the
inspector reads it. Strictly more expressive. Rejected for the FIRST increment:
it moves author-written Rust *into the trusted emission set* and needs the
inspector to validate arbitrary `impl` bodies — a much larger trusted surface
that the two-decode-boundary model does not yet cover. Revisit post-roadmap for
crates that genuinely need a hand-written trait impl (some Bevy cases).

**C. A `Rust.type`/`Rust.impl` expression in user `.ipe` source.** Rejected
outright: it routes user text into emitted Rust *outside* the `FfiInterface`
gate and the two decode boundaries, breaking §1 and the SEAL. Maximum
Completeness, zero Soundness — a direct PRINCIPLES §0 violation.

Recommendation: **A now**, keep **B** as the sanctioned escape hatch for the
handful of crates A cannot reach, never **C**.

---

## 5. Phased implementation plan (bite-sized)

Each phase is independently landable, TDD, gate-green (`cargo test -p ipe_ffi`),
and preserves the SEAL + over-drop keystone.

* **P0 — `Carrier` + `ClosureSig` decode (leaf).** New closed `Carrier` enum
  and `ClosureSig`/`BoundSet`/`DeriveSet` parsers with validating `TryFrom`.
  Anti-drift: register in `pkginfo.rs`; unit-test refusal of every ill-formed
  shape. No emission yet. *(A minimal slice of this landed — see §7.)*
* **P1 — `FnShape::StructCtor` emit.** *Landed.* Generalised `enum_ctor_lines` /
  `owned_value_coercion` to a struct literal; emits the `#[derive]`ed definition
  + constructor wrapper into `_bindings.rs`; opaque nominal + forwarder into the
  interface. The interface admission (the opaque nominal `type Counter` + the
  `counter_new` forwarder, its arity/signature read from the parsed `StructDef`)
  and the crate-absolute `crate::ffi::<slug>::<Name>` path resolution landed with
  the forwarder-plumbing work. SEAL fixtures `tests/provide_struct_seal.rs` +
  `tests/provide_forwarder_seal.rs` (the latter assembles the app's `src/ffi.rs`
  module tree and `cargo build`s+runs the forwarders under `IPE_E2E`).
* **P2 — `FnShape::ClosureAdapter`, sync single-arg.** The §3.3 adapter for
  `dyn Fn(A) -> B + Send + Sync + 'static`, A/B total carriers, no `Task` in
  body. Fixture: a crate takes a callback, an Ipê fn supplies it, the callback
  fires and its result round-trips.
* **P3 — multi-arg closures + `Result`/`Option` returns** (fallible callback
  bodies fold through `ipe_error_from_foreign`). *OPAQUE returns landed:* the
  closure-adapter emitter threads the crate opaque-map, so a `Result`/`Option`
  Ok/Some carrier that is an opaque handle resolves — a provide-DEFINED type to
  its bare in-module name (it lives in the same `pub mod <slug>` region), an
  INSPECTED crate-opaque to its absolute `::crate::path`. The received and
  returned `Box<dyn Fn …>` types render from the SAME resolved carriers, so their
  opaque paths can never disagree. A per-call panic still folds to `Err`/`None`.
  The one hard residual: a LIFETIME/generic-parameterised inspected opaque
  (`iced::Element<'a, Message>`) OVER-DROPS the whole adapter — the bare-handle
  carrier cannot carry `Element`'s generic args, so emitting the stripped path
  would be an E0107; refusing keeps the SEAL. So `view : Model -> Element
  Message` specifically stays refused (opaque returns work for non-parameterised
  owned opaques). SEAL fixture `tests/provide_opaque_return_seal.rs` (a
  `Result<Counter>`-returning closure cargo-builds+runs under `IPE_E2E`; the
  parameterised case over-drops).
* **P4 — `provide.enum`** (Ipê union → Rust enum; reuse `EnumCtor` per variant).
  *Landed:* `EnumDef` in `carrier.rs` (unit + tuple-payload variants over the
  closed carrier set, IEEE-754 fence generalised to a sum), `FnShape::EnumDefCtor`
  wired through `decode_shape`/`shape_fallibility`/`emit_fn_region` + the
  interface admission gate, `[[rust.provide.enum]]` manifest reader in the CLI's
  `ffi.rs`. Emits the `#[derive]`ed `enum` + one constructor per variant. SEAL
  fixture `tests/provide_enum_seal.rs` (cargo build+run under `IPE_E2E`). The
  derive allowlist gained `Debug` (total for every carrier, no IEEE-754 hazard)
  because Iced's `Sandbox::Message: Debug` bound requires it. Ipê-side forwarder
  plumbing (one forwarder PER variant returning the single shared enum nominal; a
  unit variant binds a zero-arg forwarder) landed alongside `provide.struct` — see
  the §2.1 note and `tests/provide_forwarder_seal.rs`. *OPAQUE fields/payloads
  landed:* the DEFINITION emitter (`struct_ctor_lines`/`enum_def_ctor_lines`)
  threads the SAME crate opaque-map the closure adapter uses, so a `provide.struct`
  field or a `provide.enum` variant payload of an owned non-parameterised opaque
  (including a provide-defined nominal held by another provide type) resolves — the
  definition and every constructor parameter render from the same resolved carrier,
  so their opaque paths cannot disagree. A bare/parameterised handle over-drops the
  whole definition (empty wrapper region ⇒ the survivor gate drops the ref-name ⇒
  the interface skips the forwarder), never emit-and-cargo-fail. Over-drop is
  TRANSITIVE across provide types: `provide_defined` resolvability is a survivor
  FIXED POINT — a provide type resolves a field/payload of another provide type
  only when that referenced type ITSELF survives, so a type holding an over-dropped
  provide type falls with it (else it would name a `pub struct`/`pub enum` never
  emitted, an E0425). The decode gate no
  longer refuses an opaque field/payload — resolvability is now decided at emit
  time by the crate opaque-map, the single oracle. SEAL fixture
  `tests/provide_opaque_field_seal.rs` (a `Model` holding an opaque `Counter` field
  and a `Message` carrying an opaque `Counter` payload cargo-build+run under
  `IPE_E2E`; the parameterised `Element<'a, Message>` field/payload over-drops, and
  a provide type referencing an over-dropped provide type over-drops transitively).
* **P5 — async-returning closures.** *Landed.* A `Future`-returning
  `provide.closure` (`Fn(A…) -> impl Future<Output = Result<B, E>>`), the
  Axum/Hyper handler shape. `ClosureRet` gained `AsyncResult`/`AsyncOption` arms
  (async-total is unrepresentable — no error channel for a poll-panic); the
  signature parser recognises `impl Future` / `Pin<Box<dyn Future>>` /
  `BoxFuture<'static, R>`. The emitter received/returns the same concrete
  `IpeTask`-shaped boxed future (the type IS the `Send + 'static`-across-await
  proof — §3.5), produces the future under `catch_unwind`, and awaits it under a
  spawned task guarded by `AbortOnDrop` so a poll-panic folds through the
  `JoinError` arm to `Err`/`None`. SEAL fixture
  `tests/provide_async_closure_seal.rs` (an async `Result` handler cargo-builds
  on a real `tokio` and runs under `IPE_E2E`, round-tripping the awaited value
  and folding a poll-panic to `Err`; an async-total return over-drops). The
  closure-adapter-to-`run` handoff (surfacing the boxed async handler as an
  Ipê-held value) stays deferred with its sync sibling (Cluster 1).
* **P6 — escape hatch B** (`#[ipe::provide]` companion-crate proc-macro) only if
  the roadmap surfaces a crate A cannot reach.

Anti-drift checklist for every phase touching a shape: `FnShape` variant +
`decode_shape` arm + `fallibility_of` arm + `emit_fn_region` arm + interface
admission gate + `coverage.md` skip reason. A new shape that skips any of these
either over-drops silently (a completeness bug caught by the coverage report) or
fails the byte-diff test.

---

## 6. Crate-coverage roadmap

Priority order is by *soundness tractability under approach A*, natural fits
first. "Create-types features exercised" names which provide-forms each needs.

| # | Crate | Why / fit | Minimal binding surface | Create-types features |
|---|-------|-----------|-------------------------|-----------------------|
| 1 | **Iced** | Elm architecture maps directly onto Ipê TEA — `Model`/`Message`/`update`/`view`. Highest value, cleanest fit. | `Application`/`Sandbox` trait, `Element`, `Command`; the runtime driver. | `provide.struct` (Model), `provide.closure` (update/view), `provide.enum` (Message) — **P1–P4**. |
| 2 | **Axum / Hyper** | async servers; Ipê already has a server story. Handlers are `Fn(Request) -> impl Future`. | `Router`, route registration, handler adapter, extractors as opaque handles. | `provide.closure` returning a `Future` — **P5 (async), landed** (§3.5). Struct handlers via **P1**. |
| 3 | **Ratatui** | TUI; immediate-mode `render(frame)` closure, no async, small trait surface. | `Terminal`, `Frame`, widget constructors (opaque), a `draw` closure. | `provide.closure` sync (draw) + `provide.struct` (app state) — **P1–P3**. |
| 4 | **Bevy** | ECS + systems + closures — the hardest. Systems are `Fn(Query, ...)`; `Component`/`Resource` are user structs, often needing real trait impls. | `App`, `Component`/`Resource` structs, system-fn adapter, `Query` as opaque. | `provide.struct` **with a trait impl** — many need **escape hatch B**; systems need multi-arg `provide.closure` (**P3**). Partial coverage under A; full needs B. |
| 5 | **Slint / Dioxus / Gtk** | declarative/native UI; macro- or markup-driven. Slint compiles `.slint`; Dioxus uses `rsx!`. | Component handle (opaque), event-callback adapters, property setters. | `provide.closure` (callbacks) + `provide.struct` (props). Markup stays crate-side; Ipê binds the imperative seam. **P1–P3**, some macro cases → B. |

**Gap clusters filed to the backlog** (see §8):

* Cluster 0 — provide-type FORWARDER plumbing (`provide.struct` / `provide.enum`
  constructors surfaced as Ipê forwarders + opaque nominals): **LANDED.** An Ipê
  program can now construct provide-defined Rust types.
* Cluster 1 — closure adapter (`provide.closure`, sync) surfaced as an Ipê-held
  boxed-closure value + handed to a crate `run` entrypoint: unblocks Iced update,
  Ratatui draw, Slint/Dioxus callbacks. Largest remaining single unblock. The
  opaque-RETURN closure is now PARTLY landed — a `Result`/`Option` opaque return
  resolves through the opaque-map (P3 above), so an owned non-parameterised
  opaque return builds; only the lifetime/generic-parameterised
  `view : … -> Element<Message>` still over-drops (the bare-handle carrier cannot
  carry `Element`'s generic args). Opaque struct fields / enum payloads are now
  LANDED too: the DEFINITION emitter threads the same crate opaque-map, so a field
  or variant payload of an owned non-parameterised opaque (including a
  provide-defined nominal held by another provide type) resolves and builds; a
  bare/parameterised handle over-drops the whole definition (empty wrapper region ⇒
  the interface skips the forwarder), never emit-and-cargo-fail. The
  lifetime-parameterised `Model` holding a bare `Element` stays refused for the
  same reason the parameterised return does.
* Cluster 2 — struct-with-trait-impl (Bevy `Component`, Iced `Application`):
  needs escape hatch B or a declarative `impl` sub-form.
* Cluster 3 — async-returning closures (Axum handlers): **LANDED.** The
  adapter emits the async return as the concrete `IpeTask`-shaped boxed future
  and awaits it under a spawned task with the `AbortOnDrop` cancel guard (§3.5).
  The Ipê-side handoff (surfacing the boxed handler as an Ipê value passed to a
  crate `run`) is shared with Cluster 1 and still deferred.

---

## 7. First increment status

P0's leaf — the closed `Carrier` enum with a validating `TryFrom` and its
refusal tests — landed this run as `src/compiler/ffi/src/carrier.rs`, wired into
`lib.rs`. It is a pure, emission-free decode leaf: it introduces no new emitted
Rust, touches no sandbox path, and weakens no gate. It is the parse-boundary the
later phases render from (never raw manifest text). `cargo test -p ipe_ffi`
green. Everything from P1 onward is deferred to follow-up PRs per this plan.
