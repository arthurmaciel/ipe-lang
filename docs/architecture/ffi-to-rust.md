# Ipê → Rust FFI — one boundary, one design

This is the canonical specification of the entire Ipê→Rust foreign-function
boundary: type import and creation, trust and packaging, the call surfaces,
the failure story, and the security model that contains it all. The
architecture-decision record for the subsystem is `docs/adr/0033`; the
capability/sandbox posture is `docs/adr/0038`, `0040`, `0041`, `0044`,
`0046`, `0049`, `0051`, and `0053`. Where those ADRs record *why*, this
document specifies *what the boundary is* and what remains to build.

Every fenced block in this document is an illustrative design sketch (Ipê
source of a proposed surface, or an ASCII dependency diagram) — none is a
runnable shell command.

Priority order for every decision below: security > correctness > soundness >
efficiency > completeness > readability. Two rules derive most of the design:

1. **Parse, don't validate.** Attacker-influenced data (rustdoc JSON, manifest
   tables, asserted symbol paths) crosses into Ipê only through validating
   `TryFrom<wire> → Result<Domain, Diagnostic>` constructors. After decode, an
   ill-formed foreign binding is unrepresentable.
2. **Over-drop, never under-bind.** The only sound error direction is silently
   omitting a bindable symbol (a completeness gap, reported in the coverage
   ledger) plus loudly rejecting an unrenderable one (`IPE-F` diagnostics).
   Emitting Rust that `cargo` then rejects breaks THE SEAL
   (`ipe build ⇒ cargo build`) and is forbidden on every compiler-derived
   path. §5 states the one author-asserted refinement of this rule.

## 1. The boundary in one view — three orthogonal axes

Every foreign binding is classified on three independent axes. Naming them
separately dissolves most apparent design conflicts, because "two-tier" means
a different thing on each axis:

| Axis | Values | Question it answers |
|---|---|---|
| **Representation** (per type) | *transparent* / *opaque* | Does the Ipê program see the type's structure, or hold a sealed handle? |
| **Binding path** (per symbol) | *inspected* / *asserted* | Did the signature come from crate introspection, or from an author assertion (`Rust.Ffi.call`)? |
| **Provenance** (per crate) | *third-party crate* / *declarative define* / *author wrapper crate* | Who wrote the Rust, and what guarantee kind applies? |

The invariant that spans all three: **everything that crosses the boundary is
typed.** Opaque handles are nominal newtypes, never `any`; there is no
`Ty::Any` arm anywhere in the type mapper. Ipê-side aliasing, sharing, and
storage of foreign values follow ordinary Ipê value semantics because the
boundary is **owned-only**: no borrow ever crosses it (§2.4).

A second invariant spans them too: this is *source-level* binding, not C-ABI
FFI. Generated wrappers and the foreign crate compile in one `cargo` build,
so `repr`/layout/calling-convention concerns never arise — rustc checks every
seam. The risk surface is therefore not memory layout but (a) arbitrary code
execution at *build* time (build scripts, proc-macros — §7), (b) panics and
effects at *run* time (§6), and (c) untrusted names flowing into generated
source (killed at decode by the validated-identifier newtypes, §2.1).

### The kernel seam

An FFI binding is a kernel whose signature came from introspection instead of
the stdlib. Both tiers share the open, `KernelId`-indexed registry: an FFI
call lowers to `Call { callee: Kernel(Ffi(id)), args }` exactly like a stdlib
call, dispatch keys on the id (no match arms grow), and the classification
predicates return none-of-these for FFI ids so an FFI kernel can never be
mis-routed into a stdlib fast path. The `Rust.` module prefix is the
namespace trust boundary (`docs/adr/0053`): every native crossing is
greppable and visible at the import site.

### The interface gate

A `Ffi.binding "<wrapper>" args` node — the only bridge from Ipê into Rust —
is minted only inside a module whose origin is `ModuleOrigin::FfiInterface`
(`src/compiler/canon/src/resolve.rs`), which only the driver generates. User
`.ipe` source can never mint a `ForeignCall` from free text. Every surface in
this document, including the escape hatch, compiles down to driver-generated
interface entries; none of them relaxes this gate.

## 2. Type mapping — importing foreign types

### 2.1 The pipeline that feeds it

`ipe rust add <crate>` (driver: `src/ipe-cli/src/ffi.rs`) runs: trust prompt →
jailed fetch → jailed inspect (`tools/ipe-ffi-inspector`, post-macro-expansion
rustdoc JSON) → typed decode (`src/compiler/ffi/src/pkginfo.rs`,
`call.rs`) → gates (`instance.rs`, `unify.rs`) → three emitters
(`interface.rs`, `emit.rs`, `bindings.rs`) → the project-local cache
`.ipe/cache/ffi/rust/<crate>.{ipei, kernel.json, _bindings.rs, coverage.md}`.
A warm `ipe build` reads the cache and never re-runs the inspector; the
cached call AST is re-validated on read, so a hand-corrupted cache is
re-rejected. The cache is **pinned to the resolved dependency version** (and
the inspector toolchain channel): a dependency bump after inspection is a
cache miss that forces re-inspection, never a build validated against a
stale signature — this pin is load-bearing for the asserted-call checker in
§5.2, whose compile-time check reads the cached inspection.

One naming SSOT (`naming.rs`) keys all three artifacts off `wrapper_ref_name`,
so the `.ipei` binding name, the `kernel.json` entry, the `_bindings.rs`
sentinel region, and the dead-code-elimination reachability key are byte-equal
by construction. Identifiers from crate metadata are validated newtypes
(`RustIdent`, `ModulePath`) at decode — a crate that names a symbol
`"; std::process::Command::new(...)"` cannot construct one, so the injection
class dies at the trusted surface.

Scalars cross through the one saturating coercion source (`num_coerce.rs`):
total, documented clamps (never wraps, never a sign-flip), `usize`/`isize`
routed through `try_from` so 32-bit targets are correct by construction. This
is a recorded sanctioned divergence, not silent coercion.

### 2.2 Functions (the shipped baseline)

Free functions, methods, accessors, constructors, and builder setters bind
per the closed `FnShape` sum with a single decoded `Fallibility` bit read by
every emitter. `Result<T, E>` maps to `Result Error a` / `Task Error a` with
the foreign `E` folded into a typed `Error` at the boundary — never a type
parameter, never a `String` error. Generic functions bind only at concrete,
demand-driven instantiations whose type arguments pass the closed-set gate
and whose bounds are within the modellable trait set (`MODELLABLE_5 =
{Hash, Eq, Ord, Clone, Default}`, fenced two-way between inspector and
generator). A generic slot instantiated at a bare type variable is rejected
at the call site — the user monomorphises at the boundary; there is no erased
fallback.

### 2.3 Structs and enums — transparent or opaque, decided once

Today a foreign type surfaces only as an opaque nominal (`type Version` — no
constructors) plus bound functions. The target model classifies every
imported type exactly once, at decode:

- **Transparent** — chosen when *every* field (struct) or variant payload
  (enum) maps into the closed carrier set, the type is fully owned
  (no borrowed fields, no lifetime parameters), and its variant set is a
  stable contract:
  - a Rust `struct` becomes an Ipê **record**; the generator emits total
    conversion glue in both directions (record → struct literal, struct →
    record) as ordinary sentinel-bracketed wrappers;
  - a Rust `enum` becomes an Ipê **closed union**, preserving exhaustiveness:
    Rust's closed variant set becomes an exhaustive Ipê `case`, so
    make-invalid-states-unrepresentable crosses the boundary intact. C-style
    enums map to unit unions; data-carrying variants map their payloads
    through the same carrier gate.
- **Opaque** — chosen whenever transparency is not sound: a non-mappable or
  private field, a borrowed/lifetime-parameterised payload, a trait-object
  payload, a `#[non_exhaustive]` enum (its variant set is *not* a contract,
  and Ipê's closed-union `case` deliberately refuses catch-all arms, so
  surfacing it as a union would make every consumer un-compilable on a
  minor-version bump), or a type the author simply wants sealed. An opaque
  type is a typed nominal handle (`Ty::Con { module: "Rust.<Crate>", name }`,
  interned so references unify nominally across interface files) plus its
  bound functions. The program never reaches into it.

The decision is a per-type fact recorded in the interface artifacts; the two
representations are not mixed for one type. Between transparent-and-wrong and
opaque, choose opaque; between opaque-and-unsound (e.g. a lifetime-carrying
handle), over-drop. Partial admission stays sound the way the enum-ctor
emitter already works: a partially-bindable surface binds its sound subset
and over-drops the rest into `coverage.md`, never wholesale.

### 2.4 The owned-only invariant (permanent)

Ipê values are immutable, with no lifetime algebra. Therefore:

- parameters cross by owned value (the generator strips `&`, re-borrowing at
  the call site); borrowed *returns* are refused (over-drop) permanently —
  there is no owner to attach a lifetime to on the Ipê side;
- every foreign value Ipê holds is `'static`-owned; a lifetime can never
  escape into an Ipê value by construction;
- opaque handles are Clone-gated (call sites clone per use) so Ipê-side
  aliasing is sound; `!Clone` types stay dropped until the banked `Arc`/
  affine-handle extensions are demonstrated necessary.

This elevates an implementation property to an invariant: refusing borrowed
returns is a correctness win, not a completeness regression.

## 3. Type mapping — creating Rust types from Ipê

Consuming a crate is not enough for APIs that demand a *user-supplied* type —
a UI framework's message enum, a server framework's handler closure, an ECS
component. The **define surface** covers this, declaratively, in the package
manifest — never as Rust text in `.ipe` source:

- `[[rust.define.struct]]` / `[[rust.define.enum]]` — an author-declared
  shape (fields/variants over the closed carrier set, derives from the
  modellable allowlist plus `Debug`) that the driver *generates* into
  `_bindings.rs` as a real nominal Rust type plus constructor forwarders.
  Safe by construction: every emitted line is a total function of
  decode-validated newtypes. Opaque fields/payloads resolve through the crate
  opaque-map with transitive over-drop (a define type holding a dropped
  define type falls with it, never emit-and-cargo-fail).
- `[[rust.define.closure]]` — an Ipê function value becomes a boxed Rust
  closure of an exact author-declared signature (bounds a closed subset of
  `{Send, Sync, 'static}`). Each call re-enters the Ipê evaluator
  panic-isolated; a per-call panic folds to the signature's `Err`/`None`
  where an error channel exists. A *sync total* return (`-> B` with no
  `Result`/`Option`) has no such channel: the shipped behavior is a
  deliberate, loud `std::process::abort()` on a caught panic in the supplied
  Ipê function — never a laundered `Default`, never UB — and this is the one
  sanctioned abrupt exit at the boundary (§6). Async-total is
  unrepresentable in `ClosureRet` (a poll panic only surfaces through a join
  error, which needs an error channel). Async-returning closures emit the
  concrete `IpeTask`-shaped boxed future — the received box type *is* the
  `Send + 'static` proof, discharged by rustc — awaited under a spawned task
  with an abort-on-drop guard so cancellation propagates and a poll panic
  folds through the join-error arm.

Under the representation axis a define struct/enum whose members are all
identity carriers is *definitionally* transparent, and it surfaces as a
record/union using the same conversion glue as transparent import (§2.3) —
one emission path serves both directions of type traffic; imported and
created types are the same machinery viewed from opposite ends. The
transparent surface is fail-closed to the seams the glue covers: a define
type another define surface holds as an opaque handle (a closure signature's
parameter/return, another define's field/payload — seams whose generated
Rust names the defined type directly), one whose nominal an inspected
foreign type also claims, or one with a member outside the identity set
(`Bytes`, an opaque handle) keeps the opaque-nominal-plus-forwarders
surface, with the reason recorded. Constructor forwarders remain on a
transparent define — smart constructors beside the record/union surface —
converting their foreign result through the same seam glue. Closure handles
are sealed values by nature and always stay opaque.

### Crate-coverage roadmap

Priority is by soundness tractability, natural fits first; each row names the
surfaces it exercises:

| Crate class | Fit | Needs |
|---|---|---|
| Iced (Elm-style UI) | TEA maps directly | define struct (model) + enum (message) + closure (update/view); the closure→`run` handoff |
| Axum / Hyper | server handlers | async define closures (shipped) + struct extractors |
| Ratatui | immediate-mode TUI | sync define closure (draw) + struct (state) |
| Bevy | ECS, trait-heavy | Tier-2 wrapper crates with `#[define_in_ipe]` trait impls (§4) |
| Slint / Dioxus / Gtk | markup/native UI | define closures (callbacks) + structs (props); macro cases → Tier 2 |

The known residual for the closure family is the handoff: surfacing the boxed
adapter as an Ipê-held value passed onward to a crate `run`-style entrypoint.

## 4. Trust & packaging — the provenance tiers

Provenance determines *what kind of guarantee* the boundary gives. Stated
honestly, weakest binding-source to strongest:

- **Third-party crate** (crates.io / gated git): not source-scanned — its
  internal panics and effects are outside the authored-code guarantee. Its
  *build* is jailed (§7), its bindings are generated (so SEAL-safe), and its
  runtime effects are what the capability/consent model governs.
- **Tier 2 — author-supplied wrapper crate** (`[rust.wrapper]` with `path`,
  `expose`, `capabilities`): the author writes ordinary Rust; Ipê binds it by
  running the *same* inspect → sandbox → generate pipeline as any crate,
  plus two provenance gates that third-party crates do not get:
  1. **source panic-scan** — authored Rust inside an Ipê package is held to
     the no-authored-abrupt-failure bar; a hit is an attributed user error;
  2. **capability inference + reconcile** (`capability_scan`, enforced in
     `install_wrapper`): a static scanner proposes the capability set, the
     manifest declares it, and the install gate fail-closes on any mismatch
     or on any construct the scanner cannot see past (`extern` blocks,
     `include!`, non-std deps, unlexable source). Axes whose runtime jail arm
     cannot yet prove containment on the host platform are refused at
     install, not admitted unenforced; each axis re-opens as its jail arm
     lands (`docs/adr/0046`, `0049`, `0051`).
  The `#[define_in_ipe]` marker (inert attribute from the `ipe_bindgen`
  crate) rides this tier: it only widens which wrapper symbols are
  *candidates* for auto-exposure — every marked item still passes the same
  carrier gate, panic-scan, and capability gate. A non-compiling wrapper is
  caught by the sandboxed build before `ipe` exit 0, so THE SEAL holds.
- **Declarative define** (§3): impossible-by-construction — Ipê generated
  every line from validated data.

So the guarantee ladder is: *generated* (unrepresentable failure) >
*authored-and-verified* (detected, attributed failure) >
*third-party* (contained failure). Package admission (`docs/adr/0044`) reads
the same single capability vocabulary: pure-Ipê capabilities are proven by
inference; native capabilities are declared, reconciled, consent-surfaced,
and — where a jail arm exists — contained. The README and user docs must
state the ladder in exactly this tiered form, never flatter.

## 5. Surface — bound imports and the asserted escape

### 5.1 The default surface: inspected imports

`import Rust.<Crate>` exposes the inspected bindings as ordinary typed
values. This is the shared-package path: capability-disclosed, over-drop
audited via `coverage.md`, zero ceremony at the call site.

### 5.2 `Rust.Ffi.call` — low-ceremony, typed, asserted

For "I know this crate; just call this one function", the full define/expose
ceremony is over-weight. The escape hatch keeps the discipline both language
boundaries share — **typed by default, one explicit escape, and the escape
skips ceremony, never soundness** (the JS-side analogue is the raw-HTML
escape riding the same rule):

```
frobnicate : Int -> Int
frobnicate = Rust.Ffi.call "some_crate::frobnicate"
```

Semantics, each load-bearing:

- **The crate must already be admitted.** The target crate is a declared
  dependency (`ipe rust add`), so it has passed the trust prompt and the
  build jail, and its `PkgInfo` inspection is in the cache. `Rust.Ffi.call`
  against an undeclared crate is a compile error, so the escape can never
  bypass the admission pipeline.
- **The string is a parsed path, not text.** The argument is decoded at
  compile time into validated `RustIdent` segments; anything else is a
  diagnostic. The construct lowers to a driver-generated interface entry and
  a compiler-owned shim template in `_bindings.rs` — user text never reaches
  emitted Rust, and the `FfiInterface` gate (§1) is untouched.
- **The author asserts the Ipê signature; the checker order is fail-closed.**
  1. If the target symbol is present in the cached inspection with a
     mappable signature, `ipe` checks the assertion against it at compile
     time — a mismatch is an ordinary `ipe` diagnostic and THE SEAL holds in
     full.
  2. If the inspector over-dropped the symbol (the very case the escape
     exists for), the emitted shim carries the asserted types, and *rustc*
     is the checker of record: a wrong assertion fails the emitted build
     inside the shim's sentinel region and is mapped back to the assertion
     site as an attributed author-assertion error. This is the one sanctioned
     refinement of THE SEAL, and it is the same class as a non-compiling
     Tier-2 wrapper: **compiler-derived code never fails cargo;
     author-asserted code failing cargo is a surfaced, attributed user error
     — never UB, never a silent success.**
- **The asserted signature can only name types the boundary already admits.**
  Carriers from the closed set, and opaque nominals the crate's interface
  already declares. An assertion cannot conjure a new transparent mapping,
  construct an opaque handle out of thin air, or forge a nominal from
  another crate — the representation axis (§2.3) bounds what is expressible,
  by construction.
- **The coercion layer is not rustc-verified — so it is excluded from the
  assertion's freedom.** The saturating scalar coercions (`num_coerce.rs`)
  sit *between* the Ipê type and the Rust type; a shim that silently
  saturated would let a semantically-wrong assertion (an `Int` asserted for
  a `u64` identifier) pass both checkers. Therefore an asserted shim
  performs **no lossy coercion**: an asserted scalar must match the target's
  carrier exactly (identity or lossless widening only); any signature that
  would need a clamp is rejected at the assertion site with a diagnostic
  naming the required exact carrier. With coercion off the table, the
  remaining degrees of freedom really are shapes rustc verifies.
- **Genuinely-unsafe targets need an explicit marker.** An `unsafe fn`, raw
  pointers, or lifetime-carrying signatures require the `Rust.Ffi.unsafe`
  spelling — modeled on Rust's own `unsafe`: explicit, localized, and *not*
  a type-system off-switch (the shim stays typed; the marker acknowledges
  obligations rustc cannot check). Unmarked, such targets are refused.
- **The escape is a disclosed capability.** Using it flips an `ffi-raw`
  capability on the package: surfaced in capability listings, gated by
  package admission, and it makes the program native-bearing, so the runtime
  consent model (`docs/adr/0041`) applies. A package using the escape is
  honest about it by construction.
- **Every shim is born inside the panic boundary (§6)** — the rustc check
  guards shape, not panics or effects.

The two surfaces relate as a ladder, not alternatives: prototype with
`Rust.Ffi.call`, graduate to inspected imports (or a define/Tier-2 surface)
when the binding is shared. A diagnostic should suggest the graduation when
an asserted call targets a symbol the inspector *can* bind.

## 6. Safety — one fail-closed failure story

The principle at stake: well-typed Ipê never observes an abrupt failure. At
the FFI boundary this decomposes into one rule with two mechanisms:

**Rule: on every fallible surface, every foreign failure mode surfaces as a
typed `Err`.** Two disclosed exceptions bound the claim honestly: (a) a sync
*total*-return define closure has no error channel, so a panic in the
supplied Ipê function is converted to a deliberate loud
`std::process::abort()` — a visible crash attributed at the boundary, chosen
over laundering the panic into a fabricated value (the same no-error-channel
rule contains an opaque-typed field getter, the one accessor whose `.clone()`
runs the crate's own `Clone` impl: its caught panic funnel-logs and aborts
rather than unwind into app-level recovery over half-broken foreign state);
(b) foreign code that
*itself* calls `process::abort`/`process::exit` (or installs its own panic
machinery) terminates without unwinding — no `catch_unwind` can intercept a
non-unwinding exit, and no design that stays in safe Rust can. Everything
else is fail-closed:

- **Sync calls: `catch_unwind` in every wrapper body.** Every generated
  sync wrapper — inspected bindings, asserted shims, Tier-2 bindings, and
  the define-closure evaluator re-entry — executes the foreign call inside
  `std::panic::catch_unwind`, converting a panic in the crate (or anything
  it calls) into the wrapper's typed `Err` instead of unwinding the process.
  `AssertUnwindSafe` is justified structurally: everything crossing the
  boundary is owned (§2.4), the wrapper holds no state that outlives the
  call, and a caught panic yields only the typed error — no half-mutated
  Ipê value is observable. The one honest caveat is foreign-internal shared
  state (e.g. a poisoned lock inside an opaque handle after a caught panic);
  that is the crate's own contract with itself, and subsequent calls surface
  its errors through the same typed channel.
- **Async calls: containment by task boundary.** Async wrappers spawn the
  foreign future onto the process-global runtime; a panic at poll time
  surfaces as a join error folded through the error funnel; an abort-on-drop
  guard propagates cancellation so no foreign side effect runs after the
  owning task is dropped. Both arms — production-time (`catch_unwind` around
  building the future) and poll-time (join error) — land in the same typed
  `Err`.
- **The error funnel redacts.** Every foreign error and panic payload routes
  through the runtime's foreign-error funnel: the raw `Debug` (which for SDK
  errors can echo URLs, bearer tokens, API keys) is logged server-side under
  a fresh correlation id; the Ipê value is a generic typed `Error` naming
  the id. No secret rides an error value.
- **The `panic = "abort"` fence.** `catch_unwind` is sound only under
  `panic = "unwind"`. Every emitted `_bindings.rs` top carries
  `#[cfg(panic = "abort")] compile_error!(...)`, which sees the *effective*
  strategy (workspace profile, `RUSTFLAGS`, std rebuilds) — a manifest
  text-scan cannot. The emitted manifest sets no `panic` key. Consequence:
  native FFI is a native-target feature; a target whose panic strategy is
  abort-only cannot include FFI wrappers, and the fence turns that from a
  latent runtime abort into a loud build refusal.

The async residual is breadth, not mechanism: widening the honest-drop set
(trait-generic parameters, fallible typed identifiers, and the remaining
admission classes in the coverage ledgers) until the full FFI storefront
example builds shim-free with used-set-only dead-code elimination. Over-drop
remains the only sanctioned degradation; a drop that blocks a used set is a
root-cause item, never a shim.

## 7. Security — the sandbox and capability model (blocking gate)

The FFI boundary is a security boundary: `ipe rust add` compiles attacker
code (build scripts and proc-macros run at build time, transitively), and a
bound crate executes arbitrary native code at run time. Nothing in this
document ships reachable-from-source unless it rides this model.

**Build-time containment (shipped; posture is frozen).**
`src/compiler/sandbox` jails every inspector and crate-compile invocation:
bubblewrap primary (network unshared, filesystem read-only except one scoped
tempdir, env scrubbed to an allowlist, fresh PID/IPC/UTS namespaces,
rlimit/wall/output caps); an `unshare` fallback that must *prove* every
claimed namespace post-spawn before any untrusted code runs, else hard-fail;
refusal when neither can prove isolation, with a single explicit red-warning
override env. Fetch is a separate network-enabled jail phase; compile and
introspect run offline with frozen, locked resolution, which also closes any
offline-then-networked fallback. Git sources parse into a typed value
(https-only scheme, host charset + allowlist, pin mutual-exclusion) before
reaching any argv; there is no shell anywhere — direct argv `Command` only.
The trust prompt (crate, resolved version, transitive-compile count,
build-script consent) precedes any fetch; only an explicit flag skips it.
The critical-path rule: untrusted crate code runs *only* inside an explicit
add/install, never on a warm build of a project someone merely checked out.

**Run-time posture (shipped, consent-scoped).** Pure Ipê runs free — an
unreachable capability is structurally absent. A native-bearing program (the
compile-time classification derives from the same capability inference the
manifest gate uses, so it cannot be under-approximated by heuristic) runs
only by deliberate consent, with an OS jail where a per-axis arm exists;
Tier-2 wrappers additionally pass differential confinement at admission. The
capability vocabulary is one shared type across manifest, kernel registry,
inference, and jail — two lists would silently weaken the consistency check.

**What each surface contributes to the model:**

| Surface | Capability story |
|---|---|
| Inspected crate binding | native-bearing; effects contained-or-consented per the runtime posture |
| Define struct/enum | none (total constructors over validated data) |
| Define closure | carries the *caller's* capabilities — re-entering Ipê adds no authority |
| Tier-2 wrapper | declared + inferred + reconciled, fail-closed per axis (§4) |
| `Rust.Ffi.call` | `ffi-raw`, disclosed and admission-gated (§5.2) |

**The gate, stated as a gate:** a new FFI surface may merge only when (a) its
untrusted-code paths run inside the build jail, (b) its capability
contribution is attributed and disclosed as above, (c) its failure modes land
in the §6 typed-`Err` story, and (d) a security-soundness review has passed
on the design and the diff. This is a blocking condition, not guidance.

## 8. Implementation plan — dependency-ordered

Named work packages; arrows are hard dependencies. Everything listed as
*ready* depends only on subsystems that exist today (registry, inspector,
generator, sandbox, capability scan).

```
panic-boundary  ──────────────┬──►  asserted-call (Rust.Ffi.call)
   (shipped)                  │        needs: ffi-raw capability plumbing,
                              │        assertion-vs-PkgInfo check,
async-breadth   (ready)       │        attributed shim-error mapping
                              │
transparent-import (shipped)─►│──►  define-transparency unification (shipped)
                              │        (shares record/union conversion glue)
tier2-axis-reopen (per-axis, ─┘
   gated on run-jail arms)          ──►  crate-coverage roadmap (consumes all)
```

1. **panic-boundary** (shipped) — the plain sync wrapper bodies already
   executed the foreign call under `catch_unwind`; the real gaps this package
   closed were downstream of the catch: caught payloads route through the
   redacting funnel (never a raw `Debug` riding an error value), async
   join-error folds route through the same funnel, and the opaque-field
   getter — the one accessor whose `.clone()` runs foreign code — executes
   under the boundary with the disclosed funnel-log-then-abort response
   (§6). With it shipped, the §6 "every foreign failure routes through the
   funnel" rule is a property of the emitters, and every later surface is
   born inside it. Gates: the asserted-call surface; honesty of the "no
   abrupt failure" claim.
2. **async-breadth** — widen the async admission set until the storefront
   acceptance example is shim-free with used-set DCE. Independent of
   everything else; pure inspector/generator work.
3. **transparent-import** (shipped) — struct→record / enum→closed-union
   import with both-direction conversion glue, the `#[non_exhaustive]`→opaque
   rule, and the per-type decision recorded in the artifacts. Zero new
   trusted surface — it emits from decoded data.
4. **define-transparency unification** (shipped) — all-identity-carrier
   define types surface as records/unions through the same glue, fail-closed
   to the covered seams per §3; the conversion glue resolves a define type at
   its crate-local `crate::ffi::<slug>::<Name>` definition. Residual breadth:
   glue for a transparent define in a closure signature or another define's
   member (those defines stay opaque today, reason recorded).
5. **asserted-call** — `Rust.Ffi.call` + `Rust.Ffi.unsafe` + `ffi-raw`
   capability + the two-checker discipline of §5.2; requires panic-boundary
   (shims born wrapped) and the capability plumbing. A language-boundary
   change: mandatory security-soundness review before merge.
6. **tier2-axis-reopen** — per capability axis, re-admit wrapper crates on
   that axis as its run-jail arm proves fail-closed containment on the
   target platform. Ongoing, orthogonal, each axis its own reviewed slice.
7. **crate-coverage roadmap** — the §3 table, in order; consumes everything
   above and feeds discovered admission gaps back as breadth items.

What is honestly *not* startable: nothing in this plan blocks on absent
infrastructure — the historical blocker (the kernel registry) shipped. The
long pole is review bandwidth: items 5 and 6 are security-gated and cannot
be parallelised past the guardian's throughput.

## 9. Risks and open questions

- **Transparent-enum evolution.** A closed union import makes every consumer
  `case` exhaustive; a crate adding a variant in a minor version breaks
  consumers at re-add time. That is the *designed* behavior (the compile
  error is the feature), but the diagnostic must say "the crate's enum grew"
  and the `#[non_exhaustive]`→opaque rule must be documented prominently, or
  users will read it as an Ipê bug.
- **Unwind-safety residue.** `catch_unwind` + a foreign type with interior
  mutability can observe the crate's own broken invariants on subsequent
  calls (poisoned locks). Contained (typed errors), but worth a documented
  stance: Ipê guarantees *its* values are never half-mutated; it cannot
  repair a foreign crate's internal state.
- **Abort-only targets.** The panic fence intentionally refuses FFI wrappers
  wherever the effective panic strategy is abort (including targets that are
  abort-only by nature). The user-facing story for "why can't my FFI program
  build for this target" needs an explain page.
- **Asserted-call generics.** Whether `Rust.Ffi.call` may name a generic
  symbol at a concrete instantiation (turbofish in the path) or is restricted
  to monomorphic paths initially. Leaning restricted-first: the demand-driven
  instantiation gates exist, but wiring them to an asserted path multiplies
  the assertion surface.
- **Seccomp depth.** The build jail's syscall filter remains the documented
  stretch hardening; namespaces + offline + read-only carry the gate today.
- **Private/ssh git sources.** Allowed only behind a flag; scoping an SSH
  agent into the fetch jail without leaking keys is unresolved.
- **Capability-inference precision.** The Tier-2 static scanner is
  deliberately over-refusing; if false positives materially narrow authors,
  the pressure valve is finer axes, never trusting the self-report.
- **Coverage-ledger fidelity.** Over-drop is only honest if `coverage.md`
  names every drop with its reason; any admission change that drops
  silently is a defect class of its own.

## Soundness review

An adversarial security-soundness design review was run against this
specification with the code reality as evidence. Verdict:
**approve, with conditions** — all conditions are spec-tightenings, none
required re-architecting the boundary; every condition is folded into the
sections above.

Per-question rulings:

1. **Containment of arbitrary-Rust execution — sound.** Build-time: the
   jail + refusal-by-default + trust gate + warm-cache rule (untrusted code
   runs only inside an explicit add/install) hold as written. Run-time: the
   native-bearing classification derives from the same capability inference
   the manifest gate uses, so a native-reaching program cannot be classified
   pure; Tier-2 axis refusal admits no wrapper whose runtime effects are
   both uncontained and unconsented. Honesty caveat kept explicit in §7:
   "contained" is universal only where a jail arm has landed — elsewhere the
   guarantee is consent, not containment.
2. **Panic boundary — sound with disclosure.** The
   `catch_unwind`/join-error/abort-on-drop/`compile_error!`-fence story is
   fail-closed on every fallible surface. Two residuals had to be stated
   rather than implied, and now are (§3, §6): the sync total-return define
   closure's deliberate loud abort, and foreign code that directly calls a
   non-unwinding exit.
3. **Opaque-handle forgeability — sound.** The `FfiInterface`-only minting
   gate, the representation-axis bound on what an assertion may name, and
   the `Rust.` namespace trust boundary jointly block every probed forgery
   vector, including cross-crate nominal spoofing of a same-named opaque.
4. **Asserted signature vs the type system — sound after tightening.** The
   review found the original "only rustc-verified shapes remain" claim false
   in the presence of the saturating coercion layer; §5.2 now excludes
   lossy coercion from asserted shims (exact-carrier rule), and §2.1 pins
   the inspection cache to the resolved dependency version so the
   compile-time checker can never validate against a stale signature.
5. **Other probed seams — sound.** The SEAL refinement for attributed
   author-assertion errors, transitive over-drop, escape-hatch capability
   gating, and the error funnel (which also scrubs log-injection control
   characters, stronger than the redaction claim requires) all held.
