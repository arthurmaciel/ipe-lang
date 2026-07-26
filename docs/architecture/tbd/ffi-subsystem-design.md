# Ipê automatic shim-free Rust-crate FFI subsystem — architecture

> **Status:** design-locked, implementation PARKED behind compiler completion.
> The one slice that may start now is the inspector-hardening slice
> (final section — verdict: **YES, but only after its prerequisite B0.0
> de-workspaces the inspector**; until then it shares the workspace
> `target/`/lock and is NOT disjoint).
>
> **This document is the architecture the port-spec defers to milestones.**
> The sandbox threat-model, the RCE gate, the milestone sequence M-A..M-G, the
> `bwrap`-primary / `unshare`-with-post-spawn-isolation-proof fallback, and the
> async→`Task Error a` boundary shape are **settled input** and live in
> [`ffi-port-spec.md`](./ffi-port-spec.md). They are **referenced, not
> re-litigated, here.**

## Governing invariant

> **`ipe build` ⇒ `cargo build`.** The only sound error direction is *over-drop
> at introspection* (a bindable symbol silently omitted — a completeness bug,
> never a soundness bug) plus *reject-at-decode with `IPE-F4400`* (an
> unrenderable foreign-call AST refused before emission). **Under-bind is
> forbidden** — a binding emitted that `cargo` then rejects breaks
> "if it compiles, it works" at the FFI seam. Between over-drop and under-bind,
> always over-drop.

Two design rules derive everything below:

1. **Parse, don't validate.** The trusted surface is exactly two typed decode
   boundaries — `PkgInfo` decode and `Call` decode. Every byte of
   attacker-influenced rustdoc JSON crosses into Ipê's type world through those
   `TryFrom<wire> → Result<T, Diagnostic>` constructors. After decode, an
   ill-formed foreign binding is *unrepresentable*, so every downstream emitter
   and lowering step is a **total function** with no defensive re-check and no
   re-coercion to `any`. Opaque foreign types enter as nominal `Ty::Con` and
   unify only with themselves.
2. **Make invalid states unrepresentable.** Kernel origin is a sum type; the
   accessor-flag soup collapses into a closed `FnKind`; call kinds / by-kinds /
   closure kinds are closed enums with no catch-all; an unrenderable
   foreign-call AST is rejected at decode as `IPE-F4400`, never emit-and-cargo-fail.

Principle order throughout: **security > correctness > soundness > efficiency >
completeness > readability.**

Path conventions in citations: `sky/` = Haskell generator
(`/home/arthur/Documentos/comp/sky/src/Ipê/Build/`); `insp` = vendored inspector
(`/home/arthur/Documentos/comp/sky-rust/tools/ipe-ffi-inspector/src/main.rs`);
target seams = `/home/arthur/Documentos/comp/sky-rust/crates/`.

---

## §0 — Resolved blocking contradictions

These four were the load-bearing contradictions in the asker's question list.
The panel converged on the following resolutions; they are LOCKED.

### R0.1 — the "single typed parse point" is two decode sites over one domain type

There are two physical decode sites at two times:

- **add-time:** `PkgInfo` decode from inspector stdout (`sky/…/Rust/Ffi.hs:135`).
- **warm-build-time:** `<crate>.kernel.json` decode from the cache
  (`ffi-port-spec.md` §warm-cache) — the inspector never re-runs.

**Resolution.** Both sites decode through *the same* `ipe_ffi::call`/`pkginfo`
domain constructors and the same `validate_call`. The `Call` AST written into
`kernel.json` is a re-serialization of an already-validated domain `Call`; the
warm build re-runs the identical `TryFrom` validators on read (so a
hand-corrupted cache is re-rejected). Drift is impossible **by code structure**:
there is one `Call` domain type with one fallible constructor, and both entry
points construct through it. `IPE-F4400` fires at whichever site sees the
defect first; the build-time `kernel.json` decode is the authority a warm build
depends on. Ports `validateCall`/`parseCall` (`sky/…/Rust/FfiCall.hs:756-820`).

### R0.2 — M4 must be an OPEN, `KernelId`-indexed registry; the exhaustive `match KernelFn` sites get a total FFI default

Today kernels are a closed 404-variant enum (`src/compiler/ir/src/ir.rs:822`→`:1863`,
`Callee` at `:804`) matched exhaustively by `kernel_is_db`/`_is_tea`/`_is_server`
(`src/compiler/lower/src/lower.rs:324,386,456`), `kernel_native_ir_type`
(`:2128`), plus arity handling now scattered across lower (`ctor_arity`/
`max_def_arity` `:687-755`, `ir_type_from_ty` `:1746`, `native_ir_type`
`:2128`). An FFI binding is data-driven and cannot be an enum variant.

**Resolution (hard M4 acceptance criterion).** M4 ships an *open* registry
indexed by an opaque `KernelId`, not a widened closed enum. A call lowers to
`Callee::Kernel(KernelId)`; dispatch keys on the id (so 76 000 FFI kernels add
*zero* match arms). For the classification predicates, an FFI `KernelId`
resolves to a **total default of none-of-these** — never `is_tea`/`is_db`/
`is_server` — preserving the over-drop keystone (an FFI kernel is never
mis-classified into a stdlib fast-path). `native_ir_type` for an FFI kernel
comes from its `kernel.json`/`.ipei` signature, not from a match arm. **Do not
spawn M-D against the current closed `KernelFn` enum** (spec warning) — FFI
kernels would have to be re-keyed later.

### R0.4 — getter fallibility is a single stored bit; risk #4 becomes unrepresentable

The Haskell computes fallibility twice — `emitRustKernelJson`
(`sky/…/Rust/Ffi.hs:1495`) and `emitSkyiRustFn` (`Ffi.hs:1560`) each independently
select `if infallibleFfiFn fn then fieldSkyType else wrapperSkyType True`.
Agreement is human-maintained.

**Resolution.** Fallibility is decoded *once* into a `Fallibility { Infallible |
TaskError }` field on the `FnShape` sum type; **both** emitters read the same
bit. A diff-golden test byte-checks the two artifacts' fallibility per fn.
Risk #4 is thus closed at the *type* level, not merely tested against. Ports
`infallibleFfiFn`/`fieldSkyType`/`wrapperSkyType` (`Ffi.hs:1573`,
`FfiGen.hs:1090-1097,501`).

### R0.5 — build order is leaf-first: M-C before M-B

`FfiCall` imports `NumCoerce` (`FfiCall.hs:67`; cycle note `NumCoerce.hs:4-11`).
The C.0 numbering is nominal; the DAG mandates **M-A → M-C → M-B → M-D → M-E →
M-F → M-G**. Executors must not stub `numSaturate` inside M-B.

---

## Crate topology

One workspace crate `ipe_ffi`, leaf-first module DAG mirroring the Haskell
cycle-break (`Ffi → FfiInstance → FfiCall → NumCoerce`). The sandbox lives in a
*separate* `ipe_sandbox` crate so `ipe_ffi`'s decode/emit core is unit-testable
with **no process capability** — the RCE surface is confined to one crate.

```
ipe_ffi
├── diag.rs        Diagnostic + IPE-F#### codes (no String errors on public surface)
├── num_coerce.rs  LEAF. saturating scalar coercion, no deps           (M-C)
├── naming.rs      SSOT: wrapper_ref_name, module/kernel names, sentinels
├── pkginfo.rs     wire DTOs + validating TryFrom → domain PkgInfo/FnInfo (M-A)
├── typeref.rs     TypeRef enum + hand-written Visitor + Ty mapper       (M-A/M-B)
├── call.rs        Call AST + validate_call → IPE-F4400                  (M-B)  KEYSTONE
├── emit/{ipei,kernel,bindings}.rs   three emitters + BEGIN/END sentinels (M-D)
├── instance.rs    generic monomorphisation, MODELLABLE_5, closure-Clone (M-E)
├── async_bridge.rs async→Task Error a wrapper body (design now, impl later)(M-G)
└── driver.rs      ipe add/install/remove, dynamic Cargo.toml, cache      (M-F, uses ipe_sandbox)
```

**Error discipline (project-wide).** Every fallible `ipe_ffi` public function
returns `Result<T, Diagnostic>` carrying an `IPE-F####` code. There is **no
`Result<_, String>` / `Task String`** anywhere on a public surface — the
Haskell `Either String Call` / `fail String` (`FfiCall.hs:756-820`) becomes
`Result<Call, Diagnostic>` with a closed `CallDefect` reason enum. (The
inspector's *internal* `errors: Vec<String>` fail-closed channel, `insp:451`, is
exempt — it is a `tools/` crate signal the generator consumes as opaque, not an
Ipê public surface.)

---

## Locked architecture decisions

Each decision: **the decision**, a one-line rationale, and the exact Haskell
source range it ports.

### D1 — `PkgInfo` / `FnInfo` decode: two-layer wire→domain, closed enums, newtype idents

**Decision.**
- A permissive `wire` layer of `#[derive(Deserialize)]` DTOs byte-mirrors the
  inspector output — every field `Option`/`#[serde(default)]` exactly as the
  Haskell `.:? … .!= default`. Inert data, never handed downstream.
- A domain layer with **private fields, constructed only via
  `TryFrom<wire::_> → Result<_, Vec<Diagnostic>>`**. This is the validating
  decoder; it is the sole constructor, so no unvalidated `FnInfo` can exist.
- The accessor-flag cluster (`is_field`/`is_field_set`/`is_enum_ctor`/`_tag`/
  `_extract`) collapses into a closed **`FnShape`** sum; two-flags-set →
  `IPE-F4402` reject-that-fn (over-drop one binding, keep the package). This
  carries the single `Fallibility` bit (R0.4).
- `CallKind` (`method`/`function`), `ByKind` (`ref`/`refmut`/`value`),
  `ClosureKind` (`Fn`/`FnMut`/`FnOnce`), `Effect` (`pure`/`fallible`/`effectful`)
  decode as closed enums **with no catch-all** — an unknown string is a hard
  serde/`IPE-F4401` error, never a defaulted variant.
- **Identifiers are validated newtypes at the decode boundary.** `RustIdent` /
  `ModulePath` / `FieldName` have private constructors validating
  `^[A-Za-z_][A-Za-z0-9_]*$`. A crate that names a symbol
  `"; std::process::Command::new(...)"` can never construct a `RustIdent`, so
  the injection class dies at the trusted surface (emit-side `rust_str_lit` /
  `absolutize_crate` demote to belt-and-suspenders).
- `deny_unknown_fields` is **NOT** used on the wire structs — the wire
  deliberately omits absent optional keys for byte-identical back-compat
  (`_call_iterAdapters` default `[]`, `FfiCall.hs:98-105,787`).

**Rationale.** Make an invalid foreign binding unrepresentable at the one trusted
surface, so every downstream step is total and injection-free.

**Ports.** `FnInfo`/`PkgInfo` FromJSON `sky/…/FfiGen.hs:145-249` (raw `_fnGeneric`
passthrough `:145-153,222`); ident/literal safety `sky/…/Rust/Ffi.hs:50,412-413`.

### D2 — `TypeRef → Ty` mapper (two distinct directions) + `NumCoerce` leaf

**Decision — two mappers, never conflated.**

| Direction | Function | Purpose | Totality |
|---|---|---|---|
| `TypeRef` AST → Rust source | `render_type_ref` | emit `_bindings.rs` | total, **no `"F?"` fallback** — the only unrepresentable case (nested/non-direct closure) is rejected by `validate_call` check 6 (D3); a `TRClosure` reaching the non-direct path is a `CompilerBug`-class diagnostic, never a leaked `"F?"` string |
| `FnInfo` → Ipê `Ty` | `sky_type_of` | seed `.ipei` HM env | total |
| Ipê `Ty` → Rust type (concrete) | `ty_to_rust` | wrapper param/ret | total on closed set, emit-only `→ String` fallback tolerated ONLY on the non-`call` shared path (unknown means over-drop already happened upstream) |
| Ipê `Ty` → Rust type (generic slot) | `ty_to_rust_closed` | generic bindability gate | **fallible, no fallback** — record/tuple/fn/bare-TVar/opaque → `Err` → `IPE-F4400` |

`TypeRef` deserializes via a **hand-written single-key `Visitor`**, not
`#[serde(untagged)]` — untagged swallows *which* variant failed (destroying
diagnostic quality) and can backtrack / mis-route an adversarial map. It rejects
the multi-discriminator case (`param`+`prim` both present).

**Mapping table (`sky_type_of`, foreign → Ipê).** Integer widths (`i8..i128`,
`usize`, `isize`) → `Int` (carrier `i64`); `f32/f64` → `Float`; `bool`/`char`/
`()` direct; `String`/`&str`/`&Path`/`&OsStr` → `String`; `Option<T>` →
`Maybe`; `Result<T,E>` → `Result Error a` (**`E` erased to Ipê `Error`, never a
type param, never `Task String`**); `Vec<T>` → `List`; `Dict String a` →
`HashMap`; serde-bound generic (`TRSerdeValue`/`TRSerdeValueRef`) → **`String`
(JSON text)**; anything else → **nominal `Ty::Con { module: "Rust.<Crate>",
name }`**. There is **no `Ty::Any` arm anywhere** — that absence is the eval-hole
foreclosure.

**Opaque `Ty::Con` keying.** `module` is the computed `Rust.<Crate>` module
(`Ffi.hs:262`), `name` the type ident, both interned `Symbol`s
(`src/compiler/types/src/ty.rs:29-33`) so two references to the same opaque type
from different `.ipei` files unify. The mapper trusts the inspector's per-param
`rustType` override verbatim on the emit side (`_fnRustParamTypes`,
`FfiGen.hs:94-98`); it does not re-derive it.

**`NumCoerce` placement.** `num_coerce.rs` is the DAG leaf (no deps). One
saturating helper pair: `num_saturate` (param, Ipê width → foreign, clamps —
`u64` from `i64` is `.max(0) as u64`; `usize`/`isize` route through `try_from`
so they are 32-bit-correct by construction, which a bare `as` and all-64-bit CI
would never catch) and `num_widen_scalar` (return, foreign → Ipê carrier —
`u64::MAX` avoids the `as i64` sign-flip via `.min(i64::MAX as u64)`). **Every**
scalar cast in `emit` and `instance` delegates here; a grep-fence test asserts
no bare `as i64`/`as u64` outside `num_coerce`.

**Rationale.** One typed foreign→Ipê seed with no `any`; one closed-set gate
that turns every unbindable generic arg into a loud reject; one saturating
scalar source.

**Sanctioned divergence (recorded).** Values above `i64::MAX` **saturate**
(not wrap, not error), logged `oracle_divergence = true`, reason "total
documented clamp per NumCoerce, not a `-1 → 3.4e38` sign-flip." Satisfies
"no silent numeric coercion" because the clamp is total and documented.

**Ports.** `skyTypeToRust` `sky/…/Rust/Ffi.hs:331-347`; `skyTypeToRustClosed`
`sky/…/Rust/FfiInstance.hs:263-284`; `TypeRef`/`renderTypeRef` + serde reduction
`sky/…/Rust/FfiCall.hs:200-224,681-691`; `NumCoerce` invariant `NumCoerce.hs:8-11`,
`numSaturate:58`, `numWidenScalar:93`; return widening `Ffi.hs:577-645`;
param narrowing call site `FfiCall.hs:510`.

### D3 — `Call` decode gate: `validate_call` inside `TryFrom` → `IPE-F4400`

**Decision.** `Call`/`Receiver`/`ByKind`/`ClosureKind` are closed enums.
`Call`'s only constructor is `TryFrom<(wire::Call, n_params)> →
Result<Call, Diagnostic>`, running the seven structural checks *inside* decode.
The error is a `Diagnostic { code: IPE-F4400, span, reason: CallDefect }` where
`CallDefect` is a **closed enum** of the seven defect classes
(`ParamRefOutOfRange`, `ReceiverKindMismatch`, `ArgIndexNegative`,
`ArgIndexDuplicated`, `ArgIndexGap`, `ArgTypeArityMismatch`,
`ClosureNestedOrNonDirect`, `IterAdapterTargetNotVec`) — never a bare `String`.

The seven checks (ported verbatim): (1) every `TRParam i` has `0 ≤ i <
n_params`; (2) method ⇔ receiver present; (3) arg indices non-negative and
gap-free from 0; (4) arg indices unique; (5) `arg_types.len() == call_arity`;
(6) no closure nested in ctor/ret/typeArgs/turbofish (only as a direct arg);
(7) every `iter_adapters` index targets a `Vec<_>` slot.

**Rationale.** Convert every ill-formed foreign call into a first-class
`IPE-F4400` *before* emission, so `render_call` over an `Ok(Call)` is total and
cannot emit-and-cargo-fail (closes the `Vec<closure>` E0412, use-after-move,
`.into_iter()`-on-non-Vec classes at the parse boundary).

**Ports.** `validateCall` seven checks `sky/…/Rust/FfiCall.hs:256-333`;
`parseCall`/kind decode `FfiCall.hs:697-820`.

### D4 — three emitters, one naming SSOT

**Decision.** `naming.rs` owns the single source of truth: `wrapper_ref_name`
(`lower_first(name)` + `"_from_" + lower_first(recv)` for accessors, returning a
validated `RustIdent`), `rust_module_name` (`uuid → Rust.Uuid`),
`rust_kernel_name` (`Rust_<CapBase>`, version-suffix aware), and the sentinel
constants. **No emitter constructs a name independently.** The three emitters
iterate the same `&[FnInfo]` and key every artifact off `wrapper_ref_name`, so
the `.ipei` binding name, the `kernel.json` `"name"`, the `_bindings.rs`
`// IPE-FFI-WRAPPER BEGIN <ref>` sentinel, and the S4 DCE `FfiRef` reachability
key are **byte-equal by construction** — three-way name skew (an under-bind that
link-fails) is structurally impossible. The Rust target always disambiguates, so
the shared emitter's `disambMethods :: Bool` is dropped (Rust-only, always-on).

- **`emit/ipei.rs`** — `module Rust.<Crate> exposing (..)`, one HM signature per
  bindable fn from `sky_type_of`, fallibility from the single `Fallibility` bit
  (D-R0.4). **Also emits one nominal opaque-type declaration** (`type Version` —
  no constructors) per foreign type referenced by any exposed signature, so the
  `.ipei` is a *complete* type-env seed (a `Ty::Con` no module declares would be
  a dangling reference at seed time — a consumer-side under-bind).
- **`emit/kernel.json`** — one entry per fn: `wrapper_ref_name`, `sky_signature`,
  the round-tripped `Call` AST, `origin: Ffi { crate, version }`, the raw
  `generic` block, plus `transitive_deps` / `features` for the driver.
- **`emit/bindings.rs`** — per fn, a wrapper bracketed by BEGIN/END sentinels
  (everything outside a pair is preamble, kept unconditionally), body =
  `render_call` wrapped in `catch_unwind → Err` (D6) + scalar coercions (D2).
  Module top carries the `#[cfg(panic="abort")] compile_error!` fence (D6).

**Rationale.** One naming function makes the tri-artifact agreement +
sentinel + DCE key mutually consistent by construction rather than by test.

**Ports.** `wrapperRefName` `sky/…/Rust/Ffi.hs:222-240`; sentinels
`Ffi.hs:247-258`; DCE `FfiRef` `Ffi.hs:224-229`; `emitKernelJson`/`emitSkyi`
`sky/…/FfiGen.hs:441,1834`; `emitRustKernelJson`/`emitSkyiRustFn`
`Ffi.hs:1495,1560`; `absolutizeCrate` `Ffi.hs:372-381`.

### D5 — kernel-registry integration + `.ipei` HM seeding (BLOCKS on M4, consumer side only)

**Decision.** The **shared thing is the `KernelId`, not a shared `KernelEntry`
struct.** The registry design (kernel-registry-design.md Q1/Q4) proved a single
`KernelEntry` struct **cannot** live in the leaf crate without a dependency
cycle: stdlib is realized instead as N exhaustive `match KernelId → Scheme`
projections in the type-owning crates, so the literal `KernelEntry { id:
KernelId, sky_signature: Scheme, origin: Origin, emit, fallibility }` struct
**survives only for the FFI tier**, living in `sky_types` and seeded from `.ipei`
as an `FfiRegistry`. Both tiers share `KernelId` space and take the **same**
canon / name-resolution / lowering path; the only difference is `origin`
(`Origin { Stdlib | Ffi { crate: CrateRef, version } }`), the signature source
(`.ipei` decode vs the stdlib exhaustive-projection arms), and `emit`. FFI
kernels thus inherit every stdlib soundness property (no `any`, no downstream
re-coercion) for free.

**HM seeding.** The type-env builder loads every `.ipei` in the cache and
registers `(Rust.<Crate>.<name> → KernelId, Scheme)` with `origin = Ffi`. This
is the *consumer-side* single parse point: after this load a foreign value is a
fully-typed Ipê value; opaque types are `Ty::Con` unifying nominally.
Post-registry-migration the stdlib resolution seam **moves from `env.rs`
`QUALIFIERS` (`src/compiler/canon/src/env.rs:182`) to `sky_kernels::resolve`**
(kernel-registry-design.md Q2), so the FFI `Rust.*` → `Ffi(fid)` path targets
`sky_kernels::resolve(qual, name)` returning `KernelId::Ffi(fid)` when a loaded
`.ipei` declares the `Rust.*` qualifier, else an unknown-qualifier error (the
`VarHome::Kernel` → `canon::Expr_::VarKernel` production is at
`src/compiler/canon/src/resolve.rs:1131`; `:986` is a lambda-capture comment, not
the seam). `kernel.json` is consulted at lowering (not typing) to resolve the
`KernelId`.

**Blocking boundary (LOCKED, per R3).** The *generator* (M-A..M-E: decode →
gates → three artifacts) is a pure function from inspector JSON to files and has
**zero** dependency on the registry — acceptance rung 1 (the `semver`
byte-diff of emitted artifacts) is reachable *before M4 exists*. Only the
*consumer wiring* (`.ipei` seeding + `KernelId` lowering resolution) blocks on
M4. So M-A..M-E proceed in parallel with M4; only the build-and-run rung waits.

**M-D acceptance gate — `.ipei → Ty` ≡ stdlib hand-`match` structural identity
(net-new, tied to task #42).** Before the **first `Ffi` entry lands**, verify
that `.ipei → Ty` decoding produces a **structurally-identical `Ty`** to the
stdlib exhaustive-projection scheme for the **same logical signature** — e.g. an
FFI `fun(int, string)` must type-check *identically* to stdlib `String.fromInt`.
This is load-bearing because the stdlib and FFI schemes are built by **two
independent `Ty`-constructors** (the hand-`match` projection vs the `.ipei`
decoder) that agree only **by test, not by construction**. This is the registry
design's **OPEN DECISION 1** (kernel-registry-design.md §OPEN DECISIONS, `:249-252`),
imported here as a **gating check**: if the two paths show any structural
divergence for the same logical signature, promote to a shared descriptor;
otherwise keep the hand-`match`. Revisit before the first `Ffi` entry — this is
an M-D acceptance criterion, not a deferred nicety.

**Rationale.** One registry means FFI call sites type-check, resolve, and lower
through the exact stdlib code; splitting the blocking boundary keeps the
generator off M4's critical path.

**Ports.** canon seeding analogue `src/compiler/canon/src/env.rs:186-268`;
module-name computation `sky/…/Rust/Ffi.hs:262-292`; kernel-name
`Ffi.hs:270-278`.

### D6 — async → `Task Error a` + `catch_unwind` + the panic-profile compile fence

**Decision (design-complete now, implementation after the sync ladder).**
- Every foreign call in every wrapper (sync and async) is wrapped so a foreign
  panic becomes an Ipê `Err(Error::…)`: sync uses `std::panic::catch_unwind`;
  **async uses `futures::FutureExt::catch_unwind` on the pinned future** (you
  cannot `.await` inside the closure `std::panic::catch_unwind` takes).
- **Panic-profile gate is a `compile_error!` fence emitted IN the wrapper
  crate**, once per `<crate>_bindings.rs` top:
  `#[cfg(panic = "abort")] compile_error!("…requires panic=unwind");`.
  `cfg(panic)` (stable since 1.60) sees the *effective* config — catching a
  workspace-root profile, `RUSTFLAGS=-Cpanic=abort`, `-Zbuild-std` — which a
  `Cargo.toml` text-scan cannot. `cargoProfilePanicIsUnwind` is ported but
  **demoted to an advisory pre-check**. This is the sound gate; the manifest
  (D7) sets no `panic=` key.
- A foreign `async fn -> Result<T, E>` maps to `Task Error T` (**never
  `Task String`**): the runtime bridge integrates with **the single reactor the
  Task executor owns** — the wrapper does `spawn(async move { … })` and bridges
  the handle into the Task completion channel; **it never calls `block_on`**
  (calling `block_on` inside a running reactor panics — a forbidden runtime
  panic). The foreign `E` folds into a typed `Error` value at the boundary;
  Ipê's type is always `Task Error a`.
- `Send` discipline: async combinator closure args need `Box<dyn Fn + Send +
  'static>`; the **inspector** supplies the `Send` verdict
  (`recv_provably_async_send`, `PROVABLY_SEND_OPAQUE_NAMES`); an unprovable
  concrete keeps dropping (over-drop). The generator never re-derives Send-ness.

**Honest scope.** M-G proves the *bridge* on a small async crate. Large async
SDKs (firestore/firebase/stripe) stay hand-shimmed even in upstream Sky and are
**not** marketed shim-free.

**Rationale.** Preserve "well-typed Ipê never panics" across the boundary while
keeping the effect-boundary typed; the fence is the only way to make
`catch_unwind` soundness *enforced* rather than assumed.

**Ports.** `catch_unwind` contract `sky/…/Rust/Ffi.hs:53-65`;
`cargoProfilePanicIsUnwind` `Ffi.hs:66-82`; `_call_isAsync` +
`synthesiseGenericWrapper` `sky/…/Rust/FfiCall.hs:123-130`,
`FfiInstance.hs:550,720`; Send census `insp:4672,4923`.

### D7 — dynamic `Cargo.toml` + S4 wrapper DCE

**Decision.** The emitted project's `Cargo.toml` is generated: base manifest +
one `[dependencies]` line per FFI crate the program actually uses, each with the
**exact pinned version** from `PkgInfo.transitive_deps` (resolve package name
via the `(ident, canonical_name, version)` triple — **never guess `_`→`-`,
never emit `"*"`**) and the **effective feature set** rustdoc succeeded with
(under-including features makes feature-gated types vanish — the firestore
#73/#100 class). Transitive deps a wrapper's `::<ident>::…` reference needs are
injected. **No `panic=` key** (cargo default unwind — D6).

**S4 DCE.** Whole-program reachability (the same pass that drives generic
instance collection, D5) yields the reached `wrapper_ref_name` set; the driver
**text-slices** `_bindings.rs` on the BEGIN/END sentinels, keeping only reached
wrapper regions + preamble — **without parsing Rust**. Conservative-keep (keep
if reached OR referenced by a kept wrapper's preamble) so it can never
under-keep (an under-bind); over-keep is dead code cargo strips. This is what
makes a 76 000-symbol crate (13-skyshop scale) compile only the ~dozen wrappers
a program calls.

**Rationale.** Reproducible, feature-correct manifests + sentinel DCE are the
two scale keystones; conservative-keep preserves the governing invariant.

**Ports.** `transitiveDeps`/`features` `sky/…/FfiGen.hs:172-187,247-248`,
`Ffi.hs:1526-1534`; sentinel protocol `Ffi.hs:247-258`; DCE `FfiRef`
`Ffi.hs:224-229`.

### D8 — driver: `ipe add`/`install`/`remove` + cache + argv-exec

**Decision.** The driver replaces `sh -c` (`Ffi.hs:131,182`) with direct argv
`std::process::Command` (kills shell-injection structurally). Every inspector
invocation and crate compile runs inside `ipe_sandbox` per the settled threat
model ([`ffi-port-spec.md`](./ffi-port-spec.md) §A — **not re-litigated here**):
`bwrap` primary, `unshare`-with-post-spawn-isolation-proof fallback, refusal
otherwise. Fetch (network on) is a distinct phase from inspect/compile
(`--frozen --locked --offline`). `--git` gated: https-only, host charset +
`IPE_FFI_GIT_HOSTS` allowlist, rev/branch/tag mutual-exclusion, `safe_crate_name`
before any command. Trust gate prints crate/version/git-url/transitive-count and
requires confirm (`--yes` for CI). `forward_inspector_diagnostics` is preserved
so `[sky-ffi]` feature-downgrade lines stay visible.

- **`ipe add`** — trust-gate → fetch → sandboxed inspect → decode (M-A/B) →
  gate (M-E) → emit (M-D) → write four cache artifacts.
- **`ipe install`** — from-scratch regenerate each recorded dep; idempotent on
  warm cache.
- **`ipe remove`** — delete the four cache files + the manifest line + re-seed
  the type-env.

**Cache.** `.ipe/cache/ffi/rust/<crate>.{ipei, kernel.json, _bindings.rs,
coverage.md}` (`coverage.md` reports what was over-dropped — the keystone made
visible). **Never regenerated on a warm cache** — only an explicit `add`/
`install` regenerates (mirrors the ipe watch-cache rule). Cache key includes the
inspector's **nightly toolchain channel** so a pin bump re-inspects.

**Rationale.** Argv-exec + phase separation + per-`add` trust make `ipe add` the
one place foreign code is fetched and introspected, all sandbox-wrapped.

**Ports.** `runRustInspector*` `sky/…/Rust/Ffi.hs:87-188`; `generateRustBindings`
`Ffi.hs:195-219`; `resolveRustInspector`/`findRustInspector` `Ffi.hs:295-326`;
cache-file naming/`slugify` `Ffi.hs:199-204`; single-invocation manifest mode
`Ffi.hs:168-188`.

---

## M4 kernel-registry dependency (explicit)

The consumer half of this subsystem **BLOCKS on the M4 kernel registry**, which
does not yet exist. M4 has two hard acceptance criteria this design depends on:

1. **Open, `KernelId`-indexed registry** — not a widened closed `KernelFn` enum
   (R0.2). FFI kernels register as `KernelEntry` with `origin: Ffi { crate }`;
   dispatch keys on the id so FFI adds zero match arms; the classification
   predicates (`is_db`/`is_tea`/`is_server`) return a total none-of-these default
   for FFI ids.
2. **One `KernelEntry` shape for stdlib + FFI**, so `.ipei`-seeded signatures and
   hand-seeded stdlib signatures flow through one canon/resolve/lower path.

The *generator* (M-A..M-E) does **not** block on M4 and can byte-diff its
artifacts in parallel; only `.ipei` seeding + `KernelId` lowering resolution wait
(R3 blocking-boundary split).

---

## OPEN DECISIONS

Two genuine forks remain unresolved by the panel and are deferred to
implementation-time with the trade-offs recorded.

### OPEN-1 — FFI artifact cache locality: project-local vs global content-addressed

- **Project-local `.ipe/cache/ffi/rust/` (default, R1+R2).** Per-project trust
  consent on every `ipe add`; artifacts travel/wipe with the project; matches
  the settled `.ipe/cache/` directive. A cross-project shared cache is a
  trust-laundering / poisoning surface (project A's `ipe add` of a
  sandboxed-but-hostile crate writes artifacts project B consumes).
- **Global content-addressed introspection cache (R3).** `~/.cache/ipe/…` keyed
  by `crate+version+features+toolchain+inspector-rev`; introspect-once (the
  expensive sandboxed rustdoc step), reuse across projects. Efficiency win, but
  ranks below security.
- **Recommended resolution (R2 middle).** Ship project-local as the source of
  truth for what compiles; add a global cache **only for the raw introspection
  `PkgInfo`** (not the emitted artifacts), gated by **re-consent on first use in
  each new project**. Captures the introspect-once win without laundering trust.
  Decide at M-F.

### OPEN-2 — per-region solved-`Ty` availability at FFI callees (M-E prerequisite)

Demand-driven generic instance collection (D5) assumes lowering exposes a
per-call-site region→concrete-`Ty` map at FFI callees (analogue of the Haskell
`SolvedTypes.regions → IrType`, `sky/…/Rust/FfiInstance.hs:106-107`). **The
capability appears present:** the lowering pass already imports `SolvedTypes,
Ty` (`src/compiler/lower/src/lower.rs:26`) and already handles a missing inferred
region type per region (`lower.rs:40-41,102+`) — so per-region solved `Ty` is
threaded into lower today. The remaining confirmation is narrow: verify that
map reaches the **FFI-callee region specifically** (not just stdlib/user call
sites). This de-risks M-E scheduling — likely no new lowering capability is
required, only a targeted check. Confirm against the FFI-callee region before
scheduling M-E.

> **Note — polymorphic-passthrough generics are NOT open; they are LOCKED as
> reject.** A generic FFI slot instantiated at a bare tyvar (a call site inside
> a still-polymorphic Ipê function) fails `ty_to_rust_closed` → `IPE-F4400` at
> the call site. No erased `func(any) any` fallback — that is the eval-hole the
> design forbids. The user must monomorphise at the boundary. This is a stated
> expressiveness limitation and the only sound answer.

---

## Generic FFI + closure + MODELLABLE_5 (M-E detail, LOCKED)

Not a fork — recorded here for completeness. Instance collection is
demand-driven from reachable call sites (bounded by program size, not the
crate's 76 000 symbols), deduped by `(callee, types)`, gated by `check_instance`
**before** any Rust is emitted. Per instance, per type-param, in order:

1. **Closed-set check** (`ty_to_rust_closed`): non-nameable Rust type →
   `mk_closed_set_error` `IPE-F4400`.
2. **Trait-bound check** (only on args that passed 1): bound ∉ `MODELLABLE_5` →
   `mk_unmodellable_bound_error` (names the *bound*); bound ∈ set but concrete
   lacks it → `mk_trait_bound_error` (e.g. `f64` at a `Hash`/`Eq`/`Ord` slot).

A reject at a *reached* call site is a loud `IPE-F4400`; a bound outside
`MODELLABLE_5` on an *unused* symbol is over-drop (silent, sound).

**MODELLABLE_5 two-way drift fence.** `{Hash, Eq, Ord, Clone, Default}` exists on
both the inspector (`insp:411`, asserted exactly the modellable subset with
`MARKER_TRAITS.len() > MODELLABLE_5.len()`, `insp:12962-12971`) and the generator
(`modellableTrait`, `FfiInstance.hs:292-293`). A cross-crate test asserts the two
sets are byte-identical; either side changing without the other fails CI, never a
user's cargo build.

**Closure-capture Clone gate.** `Fn`/`FnMut` slots re-clone every capture per call
(owned-clone bridge `FfiCall.hs:627-651`) → every capture must be *positively*
`Clone` via a closed **allowlist** (`ty_to_rust_closed` → `rust_type_is_clone`,
never a denylist); first non-Clone → `IPE-F4400`. `FnOnce` is moved once, never
gated (`FfiCall.hs:581-583`). The `traits_of_rust_type` table must be
**re-verified cell-by-cell against sky-rust's actual runtime derives** (notably
`SkyMaybe` derives *no* Default/Hash/Eq, and `f64`/`f32` are Clone+Default only —
the IEEE-754 security-critical cell) — this is a named M-E acceptance item, not a
port-on-faith. Ports `checkInstances`/`gateClosureArg*`/`traitsOfRustType`
`FfiInstance.hs:137-141,184-239,292-419`.

---

## Milestone ordering

Sequence and gates are owned by [`ffi-port-spec.md`](./ffi-port-spec.md) §C
(M-A..M-G). This design only *refines* the order per R0.5 (leaf-first) and the
R3 blocking-boundary split:

```
NOW  Inspector-hardening slice (below): B0.0 de-workspaces it FIRST (edits shared
     root Cargo.toml — run when primary build lane idle); only then is it
     parallel-worktree, workspace-disjoint
     ── generator M-A..M-E proceed in parallel with M4 ──
M-A  wire→domain decode: RustIdent/ModulePath newtypes, FnShape+Effect+Fallibility
     closed sums, hand-written TypeRef Visitor
M-C  NumCoerce leaf (saturating, sanctioned divergence, try_from for usize/isize)
M-B  Call decode → CallDefect/IPE-F4400 gate → render_call total
        ▶ FIXTURE 107 (semver) artifact byte-diff green  (no M4 needed)
M-D  three emitters, one wrapper_ref_name SSOT, one Fallibility bit,
     compile_error! fence + catch_unwind from day one
M-E  demand-driven generic instances + per-instance IPE-F4400 + MODELLABLE_5
     two-way fence + closure-Clone gate     [prereq: OPEN-2]
     ── consumer wiring (.ipei seed + KernelId lowering) requires M4 ──
M-F  driver (argv-exec, ipe_sandbox-wrapped, dynamic pinned+featured Cargo.toml,
     project-local cache), S4 sentinel DCE
        ▶ the 10 shim-free sync crates byte-diff green — shim-free claim proven
M-G  async → Task Error a (FutureExt::catch_unwind, single Task-owned reactor,
     typed Error) — designed now, proven on a small async crate only
```

---

## Inspector-hardening slice (parallel-startable)

### Verdict: **YES, but only after B0.0 de-workspaces the inspector; until then it shares the workspace target/lock and is NOT disjoint.**

`tools/ipe-ffi-inspector` has **no dependency on the Ipê workspace**
(`sky_*`/`ipe_*`) and **no dependency on the M4 registry** (which does not
exist). But it is **currently a member of the root `[workspace]`
(`Cargo.toml:18`)**, so today it shares the workspace `target/` and the single
root `Cargo.lock`. That means the "safe to build in isolation / does not touch
the shared `~15 GB` workspace `target/` / blocks nothing" claim is **FALSE
as-is** — a `cargo build` of the inspector rebuilds against, and locks with, the
shared workspace, so it *does* serialize behind build-gated compiler work. The
prerequisite **B0.0 (below)** de-workspaces the inspector — its own `target/`,
own `Cargo.lock`, own dir-scoped `rust-toolchain.toml`. **Only after B0.0** does
the slice become genuinely disjoint, parallel-worktree-able, and safe to build
in isolation. Once de-workspaced it blocks nothing and is blocked by nothing. It
is pure risk-reduction (de-risks the #3 soundness risk — adversarial-JSON DoS),
orthogonal to the #1 RCE risk (which the separate `ipe_sandbox` §A slice, also
startable now on its own lane, addresses at the driver layer). Unanimous across
the panel.

### Exact disjoint task list

- **B0.0 — de-workspace the inspector (BLOCKING prerequisite for all of B0).**
  `tools/ipe-ffi-inspector` is currently a member of the root `[workspace]`
  (`Cargo.toml:18`), so it shares the workspace `target/` and the single root
  `Cargo.lock`. Two consequences make this a hard prerequisite: (a) the
  "safe-to-build-in-isolation / touches-nothing-shared / blocks-nothing" verdict
  is literally false while it is a workspace member; (b) B0.1's "vendor + commit
  `Cargo.lock`" is **impossible** while it is a member — a Cargo workspace has
  exactly one root lockfile, so the inspector cannot own its own. B0.0:
  **remove `tools/ipe-ffi-inspector` from the root workspace's `members`**, give
  the inspector its **own `target/`**, its **own `Cargo.lock`**, and its **own
  dir-scoped `rust-toolchain.toml`** (the nightly pin B0.1 needs). Only *after*
  B0.0 is the slice genuinely disjoint and parallel-worktree-able. **B0.0 edits
  the shared root `Cargo.toml`**, so it must run **when the primary build lane is
  idle** (it is not itself worktree-isolatable — it mutates the shared manifest).
- **B0.1 — reproducibility pin.** Add `tools/ipe-ffi-inspector/rust-toolchain.toml`
  with the **nightly pin** (rustdoc JSON is nightly-only; the exact channel is
  the drift-fence anchor and the byte-diff determinism anchor). Vendor + commit
  `Cargo.lock`. Pin `serde` / `serde_json` / `tempfile` to exact versions. Add a
  nightly CI job rebuilding from the pin. *(Restores a regression — the vendoring
  dropped the toolchain file + lockfile.)*
- **B0.2 — fail-closed the internal parse (a REVERSAL, not an addition; ~130
  call-site fixes).** The inspector's `Cargo.toml` **already carries a
  `[lints.clippy]` block that deliberately sets `unwrap_used` / `expect_used` /
  `panic = "allow"`, with a justifying comment.** B0.2 therefore **reverses that
  prior decision** — flip those three to **deny** — and is *not* an additive
  lint tightening on a clean slate. The reversal exposes ~130 call sites that
  must then be driven to zero: **42 `unwrap` / 57 `expect` / 31 `panic`** on
  every path touching decoded rustdoc JSON. Budget the slice for that call-site
  work plus the deliberate-decision reversal (record why the original `allow`
  no longer holds). On any internal parse failure, **push to `errors:
  Vec<String>` (`insp:451`) and exit non-zero** — never abort. Distinguish a
  *fail-closed over-drop* `unwrap` (keep the drop, remove the panic →
  error-`PkgInfo`) from a genuine invariant assertion; the **over-drop keystone
  comments survive verbatim** (`insp:812,1667,1965,2950,4578,4634,4670`). No
  B0.2 change may alter *which symbols are dropped* on a well-formed crate (that
  would perturb the downstream byte-diff).
- **B0.3 — adversarial-JSON fuzz.** A fuzz/property target feeding malformed +
  adversarial rustdoc JSON (truncated, wrong-typed, huge arrays, cyclic ids,
  non-UTF-8) asserting: **no panic, bounded memory, error-`PkgInfo` out.** This
  is the acceptance test for B0.2.
- **`--git` gate + `safe_crate_name` (defense-in-depth, inspector entry).** The
  inspector may enforce `safe_crate_name` (`insp:3756`, `[A-Za-z0-9_-]+`) and the
  git-URL scheme/host charset at its own entry now — a testable gate independent
  of the absent M-F driver. The full https-only + host allowlist +
  rev/branch/tag mutual-exclusion belongs to the ported driver (M-F).
- **Rename `ipe-ffi-inspector` → `ipe-ffi-inspect`.** **DEFER** to the
  post-completion namespace sweep — renaming the crate, the
  `IPE_FFI_INSPECTOR_RS` probe (`Ffi.hs:307`), the `bin/` walk-up (`Ffi.hs:319`),
  and the `[sky-ffi]` diagnostic prefix (`Ffi.hs:149`) mid-port would churn the
  byte-diff anchors. Cosmetic; not load-bearing for hardening.

### Hard constraints on the slice

- **Freeze the `PkgInfo` wire contract.** B0 is internal robustness +
  reproducibility only. Any change to the wire shape desyncs the in-flight M-A
  decode design and is prohibited. No B0.2 change may perturb a well-formed
  crate's `PkgInfo`.
- **The inspector's `errors: Vec<String>` stays `String`** — it is the tools
  crate's internal fail-closed channel, not an Ipê public surface; the
  no-`Result String` rule does not reach it.
- **Verify the inspector-side MODELLABLE_5 fence survives untouched** so the M-E
  two-way fence closes with no inspector edit.
- **Do NOT start the sandbox here.** `ipe_sandbox` (§A) is a separate slice at
  the driver layer. B0 hardens the inspector's *parse*; it does **not** make
  `ipe add` safe against an untrusted crate — that remains the sandbox's job and
  the real ship-gate. Ship the slice with a doc note to that effect.
