# FFI surface UX — infer-by-default, assisted `foreign module` escape hatch

Status: draft design (not yet implemented)

> Every fenced block below is **illustrative of proposed, not-yet-implemented
> syntax** — none of it runs against the current toolchain. It shows the target
> author experience, not a working command.

## Problem

Today a native package declares its foreign type/closure surface in `ipe.toml`
via `[[rust.define.struct]]` / `[[rust.define.enum]]` / `[[rust.define.closure]]`:

```toml
[[rust.define.closure]]
crate = "iced"
name = "counter_update"
signature = "Fn(Int) -> Int + Send + Sync + 'static"

[[rust.define.struct]]
crate = "iced"
name = "Counter"
fields = { value = "i64" }
derives = ["Default", "Clone"]
```

Three defects, measured against `PRINCIPLES.md`:

1. **Stringly-typed.** `signature` and `fields` are free-form strings the author
   hand-writes and the compiler re-parses. That inverts *parse, don't validate*:
   the shape is text at the surface, not a typed value.
2. **Rust leaks into an Ipê-author surface.** `i64`, `Send + Sync + 'static`,
   `derives` — the author must think in Rust, hurting Readability (principle 6)
   and the kind-teacher goal.
3. **Split brain.** The foreign *type surface* lives in config (`ipe.toml`),
   divorced from the `.ipe` code that uses it. One feature, two files, two mental
   models.

## Goals

- The author reads and writes **Ipê types** at the foreign surface, never Rust.
- The common case needs **zero boilerplate**.
- When automatic inference cannot express a shape, the author has a **typed,
  fail-closed** way to supply it — in Ipê, not in stringly-typed TOML.
- One mental model for the foreign surface whether it is inferred or authored.
- Every guarantee already in place holds: the SEAL (ipe-accepts ⇒ cargo-builds),
  capability consent, and fail-closed admission.

## Non-goals

- Changing the crate/version pinning mechanism (stays in `ipe.toml`, see SSOT).
- Changing the Tier-2 native jail or the capability model themselves.
- Broadening what the inspector can infer (tracked separately; this design only
  defines the boundary and the escape hatch when inference stops).

## Overview

Two layers over one model — **the foreign surface of a crate is an Ipê module,
`Rust.<Crate>`**:

- **Default (infer).** The author writes only an import. The inspector derives
  the typed `Rust.<Crate>` surface from the crate; `exposing (…)` is the
  capability boundary. No `[rust.define]`, no config type-DSL.
- **Escape hatch (author).** When inference is incomplete or wrong for a crate,
  the author supplies a hand-written `foreign module Rust.<Crate>` file. The app
  import is *identical* either way — only the source of the surface changes.

The two layers share one name (`Rust.<Crate>`) and one syntax (Ipê module + Ipê
types), so there is exactly one way to think about a foreign surface.

## The default path — infer

App code never declares foreign shapes; it imports the inferred module and lists
exactly the names it consents to use:

```
import Rust.Iced exposing (Counter, Message, counterUpdate)
```

- The inspector generates the typed `Rust.Iced` surface (records, sum types,
  function signatures) in Ipê types — `Int`, not `i64`.
- The `exposing (…)` list is the consent surface: only listed names are reachable,
  so the attack surface is the author's explicit choice, not the whole crate.
- `ipe add <crate>` records the version pin (below) and shows the inferred
  surface + its capability set for informed consent before anything is fetched
  or built.

## The escape hatch — `foreign module`

When the inspector cannot express a shape (an opaque return, a const-generic, a
type the mapping does not cover — the const-generic and `Result`-arity inspector
gaps are the current examples), the author writes the surface by hand, as the
very module the app imports:

`src/Rust/Iced.ipe`
```
foreign module Rust.Iced from crate "iced"

type Counter = { value : Int } deriving (Default, Clone)
type Message  = Increment | Decrement deriving (Clone, Debug)

counterUpdate : Int -> Int
```

- It is Ipê syntax, parsed and type-checked by the real front end — invalid
  shapes are compile errors at the SEAL, not a string re-parse.
- App code is unchanged: `import Rust.Iced exposing (…)` resolves to this file
  instead of the inferred surface.
- The `foreign module … from crate "<name>"` header is self-identifying: a module
  whose header names a crate is a foreign surface; resolution needs no directory
  convention or new file extension.

## Assisted scaffolding — the answer to "write a stub?"

**Yes: when inference is incomplete, `ipe add` (and `ipe install`) scaffold a
complete `foreign module` stub the author finishes.** This is the hinge that
makes the escape hatch cheap.

When `ipe add iced` (or a build) finds that some of `Rust.Iced` cannot be
inferred, the tool writes `src/Rust/Iced.ipe` pre-filled:

```
foreign module Rust.Iced from crate "iced"

-- Inferred automatically. Edit only if a signature is wrong.
type Counter = { value : Int } deriving (Default, Clone)
type Message  = Increment | Decrement deriving (Clone, Debug)

-- TODO(you): `counter_update` returns Iced's opaque `Counter`; the inspector
-- cannot express an opaque return yet. Replace the hole with the Ipê signature
-- you want to expose.
counterUpdate : ??? -> ???
```

- **Inferred names are filled**; only the un-inferable names are typed **holes**
  (`???`), each with a diagnostic that names why inference stopped and links the
  relevant tracked limitation. This is the compiler-as-kind-teacher pattern
  applied to FFI.
- A hole is a **compile error until filled** — the author cannot accidentally
  ship a surface with an unresolved shape. Fail-closed by construction.
- Because the tool writes the boilerplate, "author the whole module" costs the
  author only the holes — the verbosity objection to authoring a full surface
  disappears.

`ipe add` reports what it scaffolded and points at the file, so the assisted move
is visible, never silent.

## Precedence & single source of truth

- **All-or-nothing per crate, but assisted.** If `src/Rust/<Crate>.ipe` exists,
  it is the **complete, authoritative** surface for that crate; inference is off
  for that crate. There is no inferred/authored merge, so no conflict rule and no
  ambiguity is representable (make-invalid-states-unrepresentable). Verbosity is
  not a burden because the scaffold pre-fills the inferable part.
- **Version pin SSOT.** The crate version lives only in `ipe.toml`
  `[rust.dependencies]` (all deps + versions in one place, the cargo/npm mental
  model). The `foreign module … from crate "<name>"` header names the crate
  **without** a version; the compiler asserts the crate is present in
  `[rust.dependencies]` and errors if it is not. One source of truth, no
  hand-sync.

## Consent & security

- **Capability model unchanged.** A package importing `Rust.<Crate>` is
  native-bearing; its `[capabilities] declared` set must include `native-ffi`
  (the existing capability-consistency check, proven to reject an omission). The
  `foreign module` header does not weaken this — the module *is* the crossing.
- **Informed consent at add/install time.** `ipe add` shows the surface
  (inferred or authored) and its capability set before fetch/build; the
  `exposing (…)` list narrows what app code can reach.
- **SEAL.** Both paths are typed by the front end. An inferred surface that
  cannot be expressed as well-typed Ipê is refused (fail-closed), never emitted
  as ill-typed Rust; an authored surface that does not type-check is a compile
  error. Neither path can produce ipe-accepts-then-cargo-fails.
- **No raw text into emitted Rust.** As today, every name/field/variant/derive
  re-parses through the closed decode gate and the derive allowlist; the escape
  hatch changes the *authoring surface*, not the decode gate behind it.

## Type mapping & the inference boundary

- The compiler owns the Rust↔Ipê mapping (`Int↔i64`, records↔structs, sum
  types↔enums, `deriving`↔the derive allowlist) in one place (SSOT); the author
  never writes a Rust type name.
- The inference boundary is explicit: the inspector infers what the mapping
  covers and **stops with a named hole** on anything it does not, rather than
  emitting a wrong or bare shape (the bare-`Result` mis-mapping is exactly the
  failure this rule forbids). "Stop with a hole" is the fail-closed default that
  feeds the assisted scaffold.

## Migration from `[rust.define]`

- `[[rust.define.*]]` is superseded by `foreign module` files. A migration step
  (`ipe fix` or a one-shot) can read existing `[rust.define]` blocks and emit the
  equivalent `src/Rust/<Crate>.ipe`, since the TOML already carries the shape.
- `[rust.dependencies]` is unchanged.
- Deprecation is staged: accept `[rust.define]` with a warning that points at the
  generated `foreign module`, then remove it in a later version.

## Open questions / risks

- **Nested/relocated crate paths.** A crate exposing items under submodules
  (`iced::widget::…`) — does `Rust.Iced` flatten or mirror the path? Proposal:
  mirror as `Rust.Iced.Widget`, so the module tree matches the crate tree.
- **Hole syntax.** `???` is a placeholder; the real typed-hole token must be one
  the parser already recognises (or a new reserved one) so a hole is a clean,
  teachable diagnostic rather than a parse error.
- **Scaffold churn.** Re-running `ipe add` must not clobber an author's completed
  holes; it should only add newly-missing names and leave filled ones intact
  (diff-and-augment, never overwrite).

## Testing

- Inferred surface: a crate that fully infers needs only the import; golden on
  the generated surface + a build (SEAL).
- Escape hatch: a `foreign module Rust.<Crate>` file overrides inference; the app
  import resolves to it; unfilled hole ⇒ compile error; filled ⇒ builds.
- Assisted scaffold: an incompletely-inferable crate ⇒ `ipe add` writes the stub
  with inferred names filled + named holes; re-run augments without clobbering.
- Consent: omitting `native-ffi` still rejects; `exposing` narrows reach.
- Migration: a `[rust.define]` package converts to an equivalent `foreign module`
  and builds identically.
