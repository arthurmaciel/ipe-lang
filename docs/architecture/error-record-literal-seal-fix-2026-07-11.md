# SEAL fix: `PanicInfo`/`TypeInfo`/`ErrorInfo` raw record-literal construction (2026-07-11)

**Status: implemented** (this session). Backlog row: "PanicInfo/TypeInfo/ErrorInfo
raw record-literal construction … exit-0-then-cargo-fail", filed by the
independent review of #85 (2026-07-10).

## Root cause

`sky_types::constrain` registered the payload types of `FfiPanic` /
`TypeMismatch` and the info argument of `Error` as **anonymous structural
records** (`Ty::Record`, closed row):

```text
FfiPanic     : { message : String, stack : List String } -> ErrorDetails
TypeMismatch : { expected : String, actual : String }    -> ErrorDetails
Error        : ErrorKind -> { message : String, details : Maybe ErrorDetails } -> Error
```

The Rust runtime, however, gives those payloads **nominal** types
(`sky_runtime::error::{SkyPanicInfo, SkyTypeInfo, SkyErrorInfo}` — concrete
structs inside `SkyErrorDetails` / `SkyError`). The backend lowers every
structural record shape to a project-local synthesized struct
(`RecMessageStack` / `RecActualExpected` / …), so ONE Sky type had TWO
incompatible Rust lowerings. Any program that made the structural half flow
into the nominal half type-checked (structural unification succeeds) but the
emitted Rust could not compile.

## RED repro (captured before the fix)

`FfiPanic { message = "boom", stack = [] }` in a plain CLI `main`:

* `skyc build Main.sky` → **exit 0** ("build ok").
* `cargo build` on the emitted project →

```text
error[E0308]: mismatched types
   --> src/main.rs:258:57
    |
258 | log_println(main_describe(SkyErrorDetails::FfiPanic(RecMessageStack { message: "boom".to_string(), stack: Vec::<String>::new() })))
    |                           ------------------------- ^^^^^^^^^^^^^^^ expected `SkyPanicInfo`, found `RecMessageStack`
```

An exit-0-then-cargo-fail on well-typed source — a violation of PRINCIPLES.md's
seal ("skyc exit 0 ⟹ the emitted project builds"). Until now it was only
recorded as a "sanctioned divergence" in `docs/divergences-from-sky.md`
§B-ErrorADT, which the #85 review correctly rejected as a fix.

## The two candidate remediations

1. **Nominal-type identity** (preferred by the #85 reviewer): give the three
   types a nominal (`Ty::Con`) identity so a bare record literal fails to
   unify at `skyc` time with a normal type-mismatch diagnostic.
2. **Backend coercion**: `emit_ctor` constructs the nominal runtime struct
   from the record-typed argument's fields.

### Why the coercion approach is insufficient here

The reviewer's critique — "fixes the value but not the type judgement" —
**bites**. The structural payload type escapes the constructor position:

```elm
helper p = p.message ++ "!"          -- unannotated helper

case details of
    FfiPanic p -> helper p           -- p is Rust `SkyPanicInfo`,
                                     -- helper's param lowers to `RecMessageStack`
```

This is ANOTHER exit-0-then-cargo-fail of the same class, reachable **today**
without any record literal — the pattern-bound payload (nominal on the Rust
side) flows into any position typed by the structural shape (synthesized
struct on the Rust side). `emit_ctor` coercion closes only the construction
direction; closing every escape direction in the backend would require
type-directed record-literal/parameter emission (the IR does not annotate
record literals with their solved types), i.e. a much larger change through
`sky_lower` — which a concurrent lane is editing.

### Decision: nominal-type identity

Chosen because it is the root-cause fix (ONE type on both sides of the
judgement, in both directions), it makes the invalid state unrepresentable
instead of patching its lowering, and — decisively — the codebase already has
**both** recipes it needs, so it is a session-sized change, not a type-system
redesign:

* **Field access on an opaque nominal Con** is exactly how the server
  `Request` type already works: `resolve_deferred`'s `FieldAccess` pass
  resolves fields of `Con("Request")` against a fixed table
  (`RequestFields`). `PanicInfo`/`TypeInfo`/`ErrorInfo` reuse that mechanism
  with their own fixed field tables, so the #85 golden-suite accesses
  (`panicInfo.message`, `typeInfo.expected`, `info.details`) keep working.
* **A builtin nominal leaf through canon/lower/IR/backend** is exactly the
  #85 `ErrorDetails` registration recipe (`EXTRA_BUILTIN_TYPE_NAMES`,
  `ir_type_from_canon`/`ir_type_from_ty` arms, `IrType` leaf, `render_type`
  arm to the `sky_runtime::error::*` path).

Semantics after the fix:

* `FfiPanic { … }` / `TypeMismatch { … }` / `Error kind { … }` → clean
  `skyc`-time type mismatch (`expected PanicInfo, found { message : …, stack
  : … }`). The sanctioned construction path is unchanged: `Error.io`/… smart
  constructors + `Error.withDetails` + the non-record variant constructors
  (`HttpStatus`/`JsonDecode`/`Custom`). `FfiPanic`/`TypeMismatch` payloads
  are runtime-origin values — inspectable from Sky, not forgeable from Sky —
  matching the reference design's smart-constructor discipline; `Custom` is
  the user-detail channel.
* Field access (`p.message`, `p.stack`, `t.expected`, `t.actual`,
  `info.message`, `info.details`) keeps working via the field tables.
* Record *update* (`{ p | message = … }`) on the three nominals is rejected
  at `skyc` time (there is nowhere sound for the updated structural value to
  flow). It surfaces as SKY-T0012 ("type PanicInfo has no field `message`")
  via the existing non-record fall-through — loud, span-attributed and
  sound, though the wording is imperfect for a field that IS readable; a
  dedicated "opaque builtin type does not support record update" diagnostic
  is a possible DX follow-up, not filed as a blocker.
* The escape direction is now **green**, not just rejected: an unannotated
  (or `PanicInfo`-annotated — the three names are annotatable builtins now)
  helper over a pattern-bound payload lowers its parameter to
  `SkyPanicInfo`, agreeing with the call site. The nominal fix converts a
  previously cargo-failing shape into a working program.

### Sanctioned-divergence note withdrawn

`docs/divergences-from-sky.md` §B-ErrorADT is updated in this commit: the
"record-literal construction not supported (silent cargo failure)" caveat is
replaced by the loud `skyc`-time rejection described above.

## Fix (implementation map)

| Layer | Change |
|---|---|
| `sky_types/src/constrain.rs` | `panic_info_ty`/`type_info_ty`/`error_info_ty` become `Ty::Con` (`PanicInfo`/`TypeInfo`/`ErrorInfo`, empty home) in the ctor schemes. |
| `sky_types/src/lib.rs` | `RequestFields` generalized: `BuiltinRecordFields` table also carries the three error-record field sets; the `FieldAccess` Con arm consults it. `RecordUpdate` on the Cons falls through to the existing non-record rejection. |
| `sky_canon/src/resolve.rs` | `PanicInfo`/`TypeInfo`/`ErrorInfo` added to `EXTRA_BUILTIN_TYPE_NAMES` (annotations may now name them). |
| `sky_lower/src/lower.rs` | Minimal additive arms: `ir_type_from_canon` + `ir_type_from_ty` map the three names to the new leaves; leaf classification in `ir_contains_fun` / `clone_class`. |
| `sky_ir` | `IrType::{ErrorInfo, PanicInfo, TypeInfo}` leaves + derivable/serde arms + pretty. |
| `sky_backend_rust` | `render_type` arms → `sky_runtime::error::{SkyErrorInfo, SkyPanicInfo, SkyTypeInfo}`; sibling exhaustive matches extended. |
| runtime | none (structs already exist, `pub` fields, `Clone + PartialEq + serde`). |

## Verification

* RED repro re-run: `skyc build` now REJECTS with a type mismatch naming
  `PanicInfo` (no emitted project to fail).
* New negative regression `crates/skyc/tests/error_record_literal_rejected.rs`
  pins the rejection for all three types.
* New positive golden `tests/golden/error_nominal_payload/` +
  `crates/skyc/tests/golden_error_nominal_payload.rs` pins the
  newly-coherent escape direction (annotated `PanicInfo -> String` helper fed
  by a pattern-bound payload) end-to-end.
* #85 goldens (`golden_error_details_roundtrip`, `golden_error_adt_roundtrip`)
  re-run green under `SKY_E2E=1`.
* Full gate: `cargo nextest run --workspace`, `cargo test --doc --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`.
