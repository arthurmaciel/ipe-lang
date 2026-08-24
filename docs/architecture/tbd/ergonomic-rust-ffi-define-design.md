# Ergonomic in-language Rust-FFI type definitions

Status: design (tbd). Design-only spec; no implementation is proposed to land
with this document.

The Rust-FFI *type-definition* surface — the author-declared struct / enum /
closure that a native crate needs but that Ipê must mint on the Rust side —
currently lives in `ipe.toml` as `[[rust.define.struct]]`, `[[rust.define.enum]]`,
and `[[rust.define.closure]]` tables. It is the last piece of FFI vocabulary that
`package.ipe` cannot express, and the reason `examples/ffi/iced-counter` is kept
on the legacy `ipe.toml` reader. This spec designs a first-class *in-language*
replacement: the author declares the foreign types in Ipê itself, not in a toml
table, and never in a `Rust.define*` toml-shaped vocabulary bolted onto the
package pipeline.

The design is chosen so that the FFI attack surface is no wider than the toml
one it replaces: every name, field, variant, carrier spelling, and derive still
re-parses through the driver's closed decode gates before any Rust is emitted,
and the minted binding still lives only in the unforgeable `FfiInterface` module.

> All `foreign` / `Ffi.*` code blocks below are **illustrative proposed syntax**
> for a surface this spec designs but does not yet implement — the `foreign`
> keyword and the `Ffi.*` vocabulary do not exist in the compiler today and are
> not runnable. The `ipe.toml` and `package.ipe` blocks, by contrast, are drawn
> verbatim from the current source and are real.


## A. Current state

### What the three tables express

The tables are read line-by-line in `src/ipe-cli/src/ffi.rs`
(`rust_define_closures_from_manifest`, `rust_define_structs_from_manifest`,
`rust_define_enums_from_manifest`) into three flat carriers:

- **`ManifestDefineStruct`** — a nominal Rust `struct`: a target `crate`, the
  Rust `struct_name`, an ordered list of `(field, carrier-spelling)` pairs, a
  `#[derive]` list, and a constructor-wrapper name `ctor` (defaulting to
  `<snake(name)>_new`). It DEFINES a record of owned carrier fields plus one
  constructor binding.

  ```toml
  [[rust.define.struct]]
  crate   = "iced"
  name    = "Counter"
  fields  = { value = "i64" }
  derives = ["Default", "Clone"]
  ```

- **`ManifestDefineEnum`** — a nominal Rust `enum`: a `crate`, the `enum_name`,
  an ordered list of `(variant, [payload-carrier, …])` pairs (an empty payload
  list is a unit variant), a `#[derive]` list, and a constructor-prefix `ctor`
  (each variant mints `<ctor>_<snake(variant)>`).

  ```toml
  [[rust.define.enum]]
  crate    = "iced"
  name     = "Message"
  variants = { Increment = [], Decrement = [] }
  derives  = ["Clone", "Debug"]
  ```

- **`ManifestDefineClosure`** — a *named adapter for a closure-typed argument*.
  It carries a `crate`, the wrapper `name`, and a `signature` string of the shape
  `Fn(P0, P1, …) -> R + Send + Sync + 'static`. Crucially it declares a **type
  signature**, not a function value: parameters and return are carrier spellings,
  the bound set is closed to `{Send, Sync, 'static}`.

  ```toml
  [[rust.define.closure]]
  crate     = "iced"
  name      = "counter_update"
  signature = "Fn(Int) -> Int + Send + Sync + 'static"
  ```

### Who reads it and how it feeds bindgen / inspector

At `ipe install` / `ipe add`, `ffi.rs` reads the three tables from the manifest
path (`PROJECT_MANIFEST`), then `merge_provides` splices the parsed entries into
the JSON inspection document the bwrap-jailed inspector produced for the crate,
*before* the driver decodes it. The driver
(`src/compiler/ffi/src/{pkginfo,carrier,driver}.rs`) re-decodes every spliced
entry through its closed gates:

- `Carrier::parse` — the only accepted field / param / return spellings are the
  six scalar carriers (`Int/Float/Bool/Char/String/Bytes`) and an uppercase-led
  opaque handle name; a Rust primitive Ipê cannot carry (`u32`, `str`, `&T`) is
  refused, not misread.
- `StructDef::parse` / `EnumDef::parse` — validate the fields / variants and the
  derive set against a closed `#[derive]` allowlist (float-total-Eq hazards
  excluded).
- `ClosureSig::parse` — consume-and-assert-empty parse of the `Fn(...) -> R +
  bounds` string; any unconsumed tail is a hard refusal, so no manifest text ever
  reaches the emitted adapter as a raw fragment.

A malformed entry **over-drops** (the whole entry is refused at decode) rather
than emit-and-cargo-fail. The minted binding lands in the crate's `FfiInterface`
module, which user `.ipe` source cannot forge a `ForeignCall` into.

### Why it is unergonomic

- **Stringly and toml-shaped.** `fields = { value = "i64" }` and `signature =
  "Fn(Int) -> Int + Send + Sync + 'static"` are strings inside a toml table.
  The author writes Rust-ish type syntax quoted inside toml, in a file that is
  otherwise being retired in favour of `package.ipe`.
- **Not part of the manifest model.** The tables are not on `ProjectManifest`;
  they are re-scanned ad hoc by three bespoke line-readers. `ipe migrate config`
  therefore **drops them** — it renders only the typed `ProjectManifest`, so a
  migrated `iced-counter` loses its defines.
- **Divorced from the types that use them.** The author writes a `Counter` and a
  `Message` in toml even though the Ipê program already has a model type and a
  message type. Two sources of truth for one shape.
- **A parallel dialect.** The carrier spellings (`Int` vs `i64`) and the derive
  allowlist are FFI concepts the author must learn in a toml idiom that looks
  neither like Ipê nor like Rust.


## B. Options for an in-language surface

Three candidate surfaces were considered. All three keep the driver's decode
gates unchanged — they differ only in *what the author writes* and *how the CLI
lifts it into the same `StructDef` / `EnumDef` / `ClosureSig` the driver already
decodes.

### Option 1 — A `Package.rustDefine` sub-pipeline in `package.ipe`

Extend the existing `package.ipe` builder vocabulary with a define builder, in
the same style as `Package.wrapper ( Rust.wrapper … |> Rust.expose … )`:

```elm
package =
    Package.named "iced-counter"
        |> Package.rustDependencies [ Package.rustDep "iced" "=0.12.1" ]
        |> Package.rustDefine
            [ Rust.struct "Counter"
                |> Rust.field "value" Rust.int
                |> Rust.derives [ Rust.default_, Rust.clone ]
            , Rust.enum "Message"
                |> Rust.variant "Increment" []
                |> Rust.variant "Decrement" []
                |> Rust.derives [ Rust.clone, Rust.debug ]
            , Rust.closure "counter_update"
                |> Rust.fn_ [ Rust.int ] Rust.int
            ]
```

Tradeoffs. **+** Reuses the package pipeline reader (`package_manifest.rs`), its
`expect_blessed_call` no-user-function guarantee, and its exact `|>` spine; it
is the *least new machinery*. **+** Carriers become blessed nullary constructors
(`Rust.int`, `Rust.string`), so an invalid carrier is a read-time rejection, not
a decoded-then-refused string. **−** The maintainer directive is explicit: the
replacement must NOT be a `Rust.define*` toml-shaped vocabulary *bolted onto*
`package.ipe`; this is exactly that, one layer of indirection removed. It still
declares the foreign type *in the manifest*, not *as an Ipê type*, so the
"two sources of truth" defect survives. **Rejected on the maintainer
constraint.**

### Option 2 — Foreign type declarations in a dedicated `.ipe` FFI module

The author declares the foreign types as Ipê type declarations in a normal `.ipe`
module, annotated as foreign through a blessed `Ffi.*` marker vocabulary the
compiler reads. The types are written in Ipê's own type syntax; a `port`-like
annotation binds each to its target crate.

```elm
module Ffi.Iced exposing (Counter, Message, counterUpdate)

import Ffi


{-| Bound to the real `iced` crate's model struct. -}
foreign Counter =
    Ffi.crate "iced"
        |> Ffi.struct { value : Int }
        |> Ffi.derives [ Ffi.default_, Ffi.clone ]


{-| Iced's `Message` enum — two unit variants. -}
foreign Message =
    Ffi.crate "iced"
        |> Ffi.enum
            [ Ffi.variant "Increment" []
            , Ffi.variant "Decrement" []
            ]
        |> Ffi.derives [ Ffi.clone, Ffi.debug ]


{-| The update adapter: `Message -> Counter -> Counter`, carrier-checked. -}
foreign counterUpdate : Ffi.Fn
counterUpdate =
    Ffi.crate "iced"
        |> Ffi.closure (Ffi.fn [ Ffi.int ] Ffi.int)
```

Tradeoffs. **+** This is genuinely "declaring the foreign types in Ipê itself":
the struct's fields are an Ipê record type `{ value : Int }`, the enum is a list
of Ipê-named variants, and both live in a `.ipe` module the rest of the program
can `import`. **+** It is a natural home for the `import` edge — the Ipê type
that *uses* `Counter` refers to the same declared name. **−** It needs a new
`foreign` declaration form (or a blessed top-level `Ffi.*` value convention the
manifest step scans) and a compiler/CLI pass that lifts these declarations into
the inspection document, which is more surface than Option 1. **−** It risks
implying the foreign type is a *first-class Ipê type* usable in arbitrary
positions, which it is not (it is a carrier-restricted define). The `Ffi.*`
vocabulary must make the restriction visible.

### Option 3 — Infer the defines from the Ipê types at the call boundary

Emit no explicit define surface at all. When Ipê code calls a bound crate
function whose signature mentions a type the crate does not export, infer the
struct / enum / closure from the *Ipê* type at that call site and mint the
define automatically.

Tradeoffs. **+** Zero author ceremony in the common case. **−** Fatal against
*make-invalid-states-unrepresentable at the FFI boundary* and against *Security*:
inference makes the emitted Rust type depend on flow-sensitive call-site
reconstruction, so the author cannot see, review, or consent to the exact native
type being minted at `ipe install`. It also cannot recover a derive set or the
crate association, and it re-introduces the "compiler guesses the FFI shape"
hazard the closed-carrier gates exist to prevent. **Rejected on
Security / reviewability.** (It could later *assist* Option 2 by suggesting a
`foreign` declaration, but must never silently mint one.)


## Recommendation — Option 2, the `foreign` `.ipe` declaration surface

Recommend **Option 2**: the author declares each foreign type as a `foreign`
declaration in a normal `.ipe` module, using a blessed `Ffi.*` builder
vocabulary, and the CLI lifts those declarations into the same inspection
document `merge_provides` produces today.

Justification against PRINCIPLES:

- **Security (first).** The lifted declarations flow through the *unchanged*
  driver gates: `Carrier::parse`, `StructDef::parse`, `EnumDef::parse`,
  `ClosureSig::parse`, the closed derive allowlist, and the `FfiInterface`
  minting path. The new surface is a *front-end re-spelling*, not a new emission
  path — it cannot widen what reaches emitted Rust. The author still sees and
  consents to every minted native type at `ipe install` (§D).
- **Make-invalid-states-unrepresentable at the FFI boundary.** Carriers become
  blessed nullary constructors (`Ffi.int`, `Ffi.string`, `Ffi.opaque "Element"`)
  instead of quoted strings, so an unrepresentable carrier is a *read-time
  rejection with a span*, one step earlier than the current decode-then-refuse.
  The struct body is an Ipê record type, so a duplicate or ill-formed field is a
  parse error, not a silently-dropped toml line.
- **No stringly toml.** The `signature = "Fn(Int) -> Int + …"` string is replaced
  by `Ffi.fn [ Ffi.int ] Ffi.int` — a carrier list and a carrier return, each a
  blessed constructor. Nothing crosses as a raw fragment.
- **No function-in-record (L0107).** See §C: a `foreign` closure declaration is a
  *type signature that names an adapter binding*, never a stored function value.
  It is exactly the shape the current `ManifestDefineClosure` already is — a
  `ClosureSig`, decoded to mint a named `FfiInterface` binding — so it introduces
  no function-valued field anywhere.
- **No `dyn Any`.** Carriers are the six concrete scalars plus a nominal opaque
  handle; the surface names concrete types, never an erased `Any`.

Option 2 beats the toml surface because the foreign *type* is written once, in
Ipê, in the module the program imports — collapsing the two-sources-of-truth
defect — while the security envelope is byte-for-byte the pre-existing one.
It beats Option 1 because it satisfies the maintainer's explicit "not a
`Rust.define*` bolted onto `package.ipe`" constraint: the declaration lives with
the code, not in the manifest pipeline. It beats Option 3 because every minted
native type stays author-declared, visible, and consent-gated.


## C. The mapping — shown on iced-counter

Each legacy table maps to one `foreign` declaration. The `Ffi.*` builder is a
closed, blessed vocabulary read exactly like the package pipeline's
`expect_blessed_call` (no user function may appear), and each declaration lifts
into the identical driver carrier it does today.

### struct → `Ffi.struct`

Legacy:

```toml
[[rust.define.struct]]
crate = "iced"; name = "Counter"; fields = { value = "i64" }
derives = ["Default", "Clone"]
```

`foreign` form (the struct body is an Ipê record type; the field carrier is
inferred from the field's Ipê type, `Int → Carrier::Int`):

```elm
foreign Counter =
    Ffi.crate "iced"
        |> Ffi.struct { value : Int }
        |> Ffi.derives [ Ffi.default_, Ffi.clone ]
```

Lift: `ManifestDefineStruct { krate: "iced", struct_name: "Counter",
fields: [("value", "i64")], derives: ["Default", "Clone"], ctor: "counter_new" }`
— identical to the toml path. The constructor still defaults to
`<snake(name)>_new`; an explicit `|> Ffi.ctor "make_counter"` overrides it.

### enum → `Ffi.enum`

Legacy:

```toml
[[rust.define.enum]]
crate = "iced"; name = "Message"
variants = { Increment = [], Decrement = [] }; derives = ["Clone", "Debug"]
```

`foreign` form:

```elm
foreign Message =
    Ffi.crate "iced"
        |> Ffi.enum
            [ Ffi.variant "Increment" []
            , Ffi.variant "Decrement" []
            ]
        |> Ffi.derives [ Ffi.clone, Ffi.debug ]
```

A payload variant carries a carrier list, e.g. `Ffi.variant "SetValue"
[ Ffi.int ]`. Lift: the identical `ManifestDefineEnum` with `variants =
[("Increment", []), ("Decrement", [])]`; each variant mints
`message_new_<snake(variant)>` as today.

### closure → `Ffi.closure` (no function-in-record)

Legacy:

```toml
[[rust.define.closure]]
crate = "iced"; name = "counter_update"
signature = "Fn(Int) -> Int + Send + Sync + 'static"
```

`foreign` form:

```elm
foreign counterUpdate : Ffi.Fn
counterUpdate =
    Ffi.crate "iced"
        |> Ffi.closure (Ffi.fn [ Ffi.int ] Ffi.int)
```

**How the closure define is expressed without a record-stored function.** The
declaration carries a *carrier signature*, never a closure value. `Ffi.fn
[ Ffi.int ] Ffi.int` builds a `ClosureSig`-shaped value — a parameter carrier
list and a return carrier — exactly the data the current
`ManifestDefineClosure.signature` string decodes to. The closed bound set
`{Send, Sync, 'static}` is implicit (it is the only set the sync adapter
supports) and is not author-spelled. The name `counterUpdate` binds a *minted
`FfiInterface` adapter*, not a field holding a fn; nothing anywhere stores a
function in a record. This is the same non-function-in-record posture the toml
closure define already has — the signature is data, the emitted adapter is a
named binding in an unforgeable module — so L0107 is untouched. Opaque returns
(the real Iced arrows returning `Element<Message>`) remain the same follow-up gap
they are today, expressed here as `Ffi.opaque "Element"` once that plumbing lands.

### The migrated `package.ipe` for iced-counter

The package manifest keeps *only* the crate dependency; the defines move to the
`.ipe` module:

```elm
-- package.ipe
package =
    Package.named "iced-counter"
        |> Package.version "0.1.0"
        |> Package.rustDependencies [ Package.rustDep "iced" "=0.12.1" ]
```

```elm
-- src/Ffi/Iced.ipe  (the three defines, in Ipê)
module Ffi.Iced exposing (Counter, Message, counterUpdate)

import Ffi

foreign Counter =
    Ffi.crate "iced" |> Ffi.struct { value : Int } |> Ffi.derives [ Ffi.default_, Ffi.clone ]

foreign Message =
    Ffi.crate "iced"
        |> Ffi.enum [ Ffi.variant "Increment" [], Ffi.variant "Decrement" [] ]
        |> Ffi.derives [ Ffi.clone, Ffi.debug ]

foreign counterUpdate : Ffi.Fn
counterUpdate =
    Ffi.crate "iced" |> Ffi.closure (Ffi.fn [ Ffi.int ] Ffi.int)
```


## D. Security

The new surface must not widen the FFI attack surface versus the toml one. The
design keeps every existing gate and adds one earlier one:

1. **Same decode gates, unchanged.** The lift produces `ManifestDefineStruct /
   Enum / Closure` values that are *byte-identical* to what the toml readers
   produce, then hands them to the existing `merge_provides` +
   `install_from_inspection` path. `Carrier::parse`, `StructDef::parse`,
   `EnumDef::parse`, `ClosureSig::parse`, and the closed derive allowlist run
   exactly as today. Over-drop-on-malformed is preserved: a bad declaration
   refuses the entry, never emits-and-cargo-fails.
2. **Parse-once into a typed define model.** The `foreign` declaration is parsed
   through the same blessed-call reader the package pipeline uses
   (`expect_blessed_call`: the callee must be a `Module.builder`, never a user
   binding, lambda, or computed function). Carriers are blessed nullary
   constructors, so an invalid carrier is rejected *with a span* at read time —
   strictly earlier and more precise than the current decode-then-refuse. There
   is no second, looser path: the reader emits the typed model or a diagnostic.
3. **Unforgeable `FfiInterface` minting, unchanged.** The minted struct ctor,
   enum ctors, and closure adapter land only in the crate's `FfiInterface`
   module; user `.ipe` source still cannot forge a `ForeignCall`. The `foreign`
   keyword is the *only* way to declare a define, and it is inert outside the
   FFI-lift pass (it declares data, it cannot call anything).
4. **Same jail, same consent.** Bindings are still generated by the bwrap-jailed
   inspector (network denied, build scripts confined). The `ipe install` consent
   summary still lists every minted native type; because the defines now live in
   a reviewable `.ipe` module rather than a toml table, the author's consent is
   *better* informed, not worse.
5. **No new capability.** Declaring a `foreign` type still requires the existing
   `native-ffi` capability the crate dependency already gates; `foreign` grants
   no new authority on its own.

Net: the surface is a front-end re-spelling that terminates in the identical
typed model and the identical emission path — the attack surface is equal to the
toml surface, with one strictly-earlier rejection point added.


## E. Migration and re-green

### `ipe migrate config` carries the defines forward

Today `ipe migrate config` renders only `ProjectManifest`, which does not carry
the defines, so migration silently drops them. The fix has two parts:

1. **Read the defines during migration.** `run_migrate_config` already re-reads
   the raw `ipe.toml` text to recover the wrapper table
   (`rust_wrapper_from_manifest`). It gains a sibling read of the three
   `rust_define_*_from_manifest` readers, producing the `ManifestDefine*`
   vectors.
2. **Render them as a `.ipe` FFI module, not a manifest stage.** Because the
   defines move *out* of the manifest, migration emits a second file — e.g.
   `src/Ffi/<Crate>.ipe` — containing one `foreign` declaration per define,
   rendered by the inverse of the `Ffi.*` reader (mechanical, like the existing
   `render_*` functions). The `package.ipe` itself renders exactly as it does
   now, minus any define stage. Migration prints both written paths and leaves
   `ipe.toml` in place, as it does today.

The round-trip property test extends to cover the defines: the emitted `.ipe`
FFI module must re-read (through the new `Ffi.*` reader) to the identical
`ManifestDefine*` vectors the toml produced — the same lossless guarantee the
manifest round-trip test pins.

### iced-counter goes green under `package.ipe`

`examples/ffi/iced-counter` is currently an intentional known-red: when
`package.ipe`'s P4 removes the `ipe.toml` reader, its defines have nowhere to
live. Under this design the re-green path is:

1. `ipe migrate config` on `iced-counter` produces `package.ipe` (deps only) +
   `src/Ffi/Iced.ipe` (the three `foreign` declarations).
2. Delete the `ipe.toml`; the `ffi_define_source` sidecar shim is removed once no
   example depends on it.
3. `ipe install --yes --allow-build-scripts` reads the `foreign` declarations
   from the `.ipe` module, lifts them into the inspection document, and mints the
   identical `Counter` / `Message` / `counter_update` bindings the toml minted.
4. The examples-sweep entry, previously expected-red, asserts the same emitted
   bindings and byte output as the pre-migration `ipe.toml` build — the parity
   check that proves migration is faithful.


## F. Phased plan and guardian-review points

The work is design-only until this spec is accepted. The implementation phases,
each gated, are:

1. **Design (this doc).** Accept the `foreign` `.ipe` surface and the `Ffi.*`
   blessed vocabulary. *Guardian review: language-boundary* — the reader must
   route through `expect_blessed_call`, carriers must be blessed constructors,
   and the surface must terminate in the existing typed define model.
2. **Surface + reader.** Add the `foreign` declaration form and the `Ffi.*`
   reader that lifts declarations into `ManifestDefine*` values, reusing the
   package-pipeline blessed-call machinery. No emission change. *Guardian review:
   FFI-security boundary* — confirm the lift is a pure re-spelling that produces
   byte-identical `ManifestDefine*` values and adds no new emission path.
3. **Bindgen wiring.** Feed the lifted defines into `merge_provides` /
   `install_from_inspection` from the `.ipe` module source instead of (or
   alongside) the toml readers. *Guardian review: FFI-security boundary* — the
   `FfiInterface` minting and jail/consent path is unchanged; over-drop-on-
   malformed preserved.
4. **Migrate.** Extend `ipe migrate config` to read the defines and render the
   `.ipe` FFI module; extend the round-trip property test to the defines.
   *Guardian review: language-boundary* — the render/re-read round-trip is
   lossless.
5. **Re-green.** Migrate `iced-counter`, delete its `ipe.toml`, remove the
   `ffi_define_source` sidecar shim, and flip its examples-sweep entry from
   expected-red to a parity assertion. *Guardian review: FFI-security boundary +
   examples-sweep parity* — the migrated build mints the identical bindings and
   output.

Each guardian-review point is blocking. The security-soundness guardian must
sign the two FFI-security-boundary reviews (phases 2, 3, 5); the language reviewer
signs the two language-boundary reviews (phases 1, 4).
