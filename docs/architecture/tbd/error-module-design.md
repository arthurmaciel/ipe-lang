# Ipe.Error — conciliated module design

> **⚠️ CORRECTION (impl-guardian gate, verified, 2026-07-03).** Steps 1–2's
> PRIMARY approach — "Ipe.Error as a *compiled Ipê source module*, helpers
> stay pure Ipê, no kernels" — is **INFEASIBLE at HEAD**: nothing calls
> `ipe::stdlib::source` (embedded `.ipe` are parse-tested only; `build`/
> `build_project` never inject stdlib source), and `golden_list_ops_wiring`
> establishes that **kernel routing is the only exit-0-safe wiring** (a qualified
> stdlib call lacking a `KernelFn`/lower-arm/scheme emits IPE-L0108). So the
> **KERNEL PATH (the doc's "fallback") is the real plan**: ~17 `Error.*` helper
> kernels + the Error ADT emitted **project-side in `main.rs`** (the runtime is
> generic over `E: From<String>` *because* it cannot name the project enum) +
> the atomic `type SkyError = String → SkyCoreErrorError` flip across **69 byte-
> compared goldens** (a scripted golden-regen gated on per-project cargo-build —
> a MULTI-CONTEXT effort). Already DONE + safe in-tree: runtime generic,
> **B8 redaction (approved)**, HM channel typing (`Task String a` already fails
> at ipe), `errorToString` prelude kernel. **PARKED / non-blocking** — Error-as-
> String works today; land IPE-N0001 (#82) + #76 BATCH 0 first. See task for the
> corrected step-by-step.
>
> **Status:** design of record. Reconciles three independent fresh designs
> (produced with no knowledge of upstream Sky) against the upstream upstream Sky
> learnings (weeks of prior investment), judged in strict principle order:
> **(1) security (2) correctness (3) soundness (4) efficiency (5) completeness
> (6) readability**, under the two rules **"PARSE, DON'T VALIDATE"** and
> **"MAKE INVALID STATES UNREPRESENTABLE"**.

## Verified ground truth (this repo, HEAD)

- `sky-out/.ipe-stdlib/Sky/Core/Error.ipe` is byte-identical to upstream:
  `type Error = Error ErrorKind ErrorInfo`; `ErrorKind` = 11 fieldless variants
  (`Io | Network | Ffi | Decode | Timeout | NotFound | PermissionDenied |
  InvalidInput | Conflict | Unavailable | Unexpected`); `ErrorDetails` =
  `FfiPanic PanicInfo | TypeMismatch TypeInfo | HttpStatus Int | JsonDecode
  String | Custom String`; aliases `ErrorInfo { message : String, details :
  Maybe ErrorDetails }`, `PanicInfo { message, stack : List String }`,
  `TypeInfo { expected, actual }`. Smart constructors default `details =
  Nothing` via `mkInfo`. Pure-Ipê `toString`/`kindLabel`/`isRetryable`.
- The Rust backend currently hardcodes the fallback preamble
  `const START: &str = "type SkyError = String;"` (`src/compiler/backend/rust/src/project.rs:193`).
- Lowering collapses the error type to a String: `"String" | "Error" =>
  Ok(IrType::Str)` at `src/compiler/lower/src/lower.rs:1730` and `:2089`.
- HM already interns a distinct nullary `Error` builtin
  (`src/compiler/types/src/constrain.rs:245`) and validates the `Task Error a`
  channel against it (`is_error_ty`, `:1301`/`:1372`). **Only the value level
  is missing.**
- Builtin ADTs are already seeded for ctor + exhaustive-match support via
  `enum_variants` / `ctor_arity` (SqlValue/SqlField at `lower.rs:1108`);
  Maybe/Result are runtime-owned and seeded at `lower.rs:1101-1102`. Both
  precedents exist and are usable.
- Error is **not yet registered in canon** (open task #78); today `Task Error a`
  lowers to `SkyTask<String, a>`.

## Chosen approach

**Adopt the upstream mechanism, made unconditional.** `Ipe.Error` is a
compiled Ipê *source* module — its functions (`unexpected`, `io`, `network`,
`withMessage`, `withDetails`, `kindLabel`, `toString`, `isRetryable`, …) stay
pure Ipê and are exhaustiveness-checked by the compiler. Its *type* is emitted
through the ordinary user-ADT path as a normal Rust enum
(`SkyCoreErrorError`), exactly as upstream Sky's `Emitter.hs` does — **not** as a
bespoke runtime-owned primitive. The pervasive error channel becomes
`type SkyError = SkyCoreErrorError` **unconditionally** (the upstream
String-fallback else-branch is rejected). The runtime stays fully generic over
`E: Send + From<String>` and never names `Error`; the single `impl From<String>`
(→ `Unexpected`) is the confined, crate-private parse boundary for runtime/FFI
string failures. Rust→Ipê construction is the runtime boundary; Ipê→Rust
construction is the smart constructors; observation is the pure-Ipê
`Error.toString`. Because the emitted enum *is* the `E` in
`SkyResult<E,A>`/`SkyTask<E,A>`, the Ipê↔Rust round-trip is identity — no
marshalling, no encode/decode, nothing that can drift out of sync.

### Rationale in principle order

1. **Security.** B8 redaction at the foreign-error seam is adopted verbatim:
   a reqwest/stripe/hyper `Debug` (which can echo bearer tokens, API keys,
   internal URLs) is **never** placed in the Ipê-visible message. It is logged
   server-side under a correlation id (control bytes scrubbed) and Ipê sees only
   `external operation failed (ref <id>)`. `Error.toString` renders **only**
   `kindLabel ++ ": " ++ message` — it never folds `ErrorDetails::FfiPanic.stack`
   or `TypeInfo` into a user-facing string. This preserves the two-level error
   pattern (log detail, show ref id) at the runtime boundary.
2. **Correctness.** Matching the proven upstream emission path (weeks of
   investment) is the surest route to golden byte-parity: `Error.toString`
   semantics are literally the same Ipê source. The unconditional alias removes
   the non-determinism where an incidental import decided whether `Task Error a`
   lowered to `SkyTask<String,_>` or the ADT.
3. **Soundness.** Keeping the renderers/`isRetryable`/`kindLabel` in **pure
   Ipê** means the Ipê compiler exhaustiveness-checks every match over the
   closed `ErrorKind`; adding a 12th kind forces every site to update. The
   `Task Error a` channel is structurally `Error` (`is_error_ty`), so
   `Task String a` / `Result String a` (banned by §8) are unrepresentable at the
   source level, not merely linted.
4. **Efficiency.** The runtime monomorphises `E` to the concrete enum with zero
   dispatch; the error value is only ever materialised on the failure path; the
   happy path never clones it.
5. **Completeness.** Every canon member is backed — all 11 kinds, all 5 detail
   variants, `withMessage`/`withDetails`, `kindLabel`, `isRetryable`,
   `toString`, and the Prelude `errorToString` alias. No member resolves to
   `id=None`.
6. **Readability.** One trait bound (`E: From<String>`) expresses the whole
   runtime contract; smart constructors are the single sanctioned raise API;
   the ADT maps 1:1 to an idiomatic Rust enum with nothing Haskell-ish to strip.

## Final Rust Error representation

Emitted through the ordinary user-type path (like any Ipê ADT), **not** a
runtime-owned primitive. Names/variant order/field names mirror `Error.ipe`
exactly so lowered ctor/pattern code type-checks with no shim.

```rust
// type ErrorKind = Io | Network | … | Unexpected   (11 nullary variants)
#[derive(Clone, Debug, PartialEq)]
pub enum SkyCoreErrorErrorKind {
    Io, Network, Ffi, Decode, Timeout, NotFound,
    PermissionDenied, InvalidInput, Conflict, Unavailable, Unexpected,
}

// record alias PanicInfo = { message : String, stack : List String }
#[derive(Clone, Debug, PartialEq)]
pub struct SkyCorePanicInfo { pub message: String, pub stack: Vec<String> }

// record alias TypeInfo = { expected : String, actual : String }
#[derive(Clone, Debug, PartialEq)]
pub struct SkyCoreTypeInfo { pub expected: String, pub actual: String }

// type ErrorDetails = FfiPanic PanicInfo | TypeMismatch TypeInfo
//                   | HttpStatus Int | JsonDecode String | Custom String
#[derive(Clone, Debug, PartialEq)]
pub enum SkyCoreErrorErrorDetails {
    FfiPanic(SkyCorePanicInfo),
    TypeMismatch(SkyCoreTypeInfo),
    HttpStatus(i64),
    JsonDecode(String),
    Custom(String),
}

// record alias ErrorInfo = { message : String, details : Maybe ErrorDetails }
#[derive(Clone, Debug, PartialEq)]
pub struct SkyCoreErrorErrorInfo {
    pub message: String,
    pub details: SkyMaybe<SkyCoreErrorErrorDetails>,
}

// type Error = Error ErrorKind ErrorInfo   (ONE constructor)
#[derive(Clone, Debug, PartialEq)]
pub enum SkyCoreErrorError {
    Error(SkyCoreErrorErrorKind, SkyCoreErrorErrorInfo),
}

// The pervasive channel — UNCONDITIONAL (no String fallback).
pub type SkyError = SkyCoreErrorError;

// The one, confined parse boundary (runtime/FFI seam only; stdlib forbidden).
impl From<String> for SkyCoreErrorError {
    fn from(message: String) -> Self {
        SkyCoreErrorError::Error(
            SkyCoreErrorErrorKind::Unexpected,
            SkyCoreErrorErrorInfo { message, details: SkyMaybe::Nothing },
        )
    }
}

// Rust-better divergences (guarded by the same "Error present" path):
//   impl std::fmt::Display  — delegates to Error.toString semantics
//                             (kindLabel ++ ": " ++ message ONLY; never details)
//   impl std::error::Error  — so `?` and the Rust error ecosystem work natively
// str_err(&str) delegates to From<String> so the Unexpected shape exists once.
```

**Why this shape.** A single-constructor sum pairing a **closed** kind enum with
a message-plus-optional-details record is the minimal shape that makes every
invalid state unrepresentable while giving control-flow code (`isRetryable`) a
machine-inspectable tag. Fields are `pub` because generated struct/enum literals
need them — but unrepresentability here is **structural** (every field is itself
a total type), not encapsulation-based, so opacity is not required and would
cost the pure-Ipê, compiler-checked renderers. `Clone/Debug/PartialEq` are the
right minimum (Task combinators clone the error on some paths; `PartialEq`
serves `Ipe.Test` assertions like `Err (Error.unexpected "x")`); `serde` is
added only where a boundary needs it.

### Round-trip (closed, total, by identity)

- **Rust → Ipê (construct):** a failing effect returns `SkyTask<SkyError, A>`;
  its `"msg".into()` uses `From<String>` → `Error(Unexpected, {message, Nothing})`.
  High-fidelity origins (`http → network`, `timeout → timeout`, `db → io`) call
  the typed smart constructors instead (non-blocking follow-up). Foreign errors
  route through the B8-redacting seam.
- **Channel:** `Task Error a` carries the concrete `SkyError` end to end — no
  boxing, no coercion.
- **Ipê → Rust (construct):** `Task.fail (Error.unexpected "boom")` runs the
  compiled-from-source `unexpected`, building the identical enum value.
- **Ipê (observe):** `Error.toString e` / `errorToString e` run the pure-Ipê
  renderer over the same Rust value — total String, zero allocation beyond the
  message it already owns.

## Invalid states made unrepresentable

- **PARSE, DON'T VALIDATE.** The moment a raw failure string enters the runtime
  it is *parsed* into a structured `Error` via `From<String>` (→ `Unexpected`).
  Downstream never re-parses a `"<Kind>: <msg>"` string to recover the kind — it
  matches the closed `ErrorKind`. That `From<String>` seam is the *only* legal
  ingress for untyped data, and it is crate-private to the runtime/FFI boundary;
  **stdlib surfaces are forbidden from it** and must pick a real kind (the one
  ADAPT over upstream, which otherwise lets `Unexpected` become a default).
- **Closed `ErrorKind`.** No `Unset`/`Unknown`/`Invalid` variant — a kind-less
  error cannot exist; `kindLabel`/`isRetryable` are total by exhaustiveness.
- **Single constructor `Error ErrorKind ErrorInfo`.** No half-built error; every
  value pairs a kind with an info.
- **`ErrorInfo.message : String` (mandatory, not `Option`).** An empty/absent
  message is unrepresentable.
- **`details : Maybe ErrorDetails`.** Absence is expressed by the type, never a
  sentinel/empty-string; each `ErrorDetails` variant pairs its shape with
  exactly its payload (`HttpStatus i64`, `TypeMismatch { expected, actual }`) —
  you cannot build an `HttpStatus` carrying a `TypeInfo`.
- **Structural channel typing.** `Task Error a` is `Error`, never `String`; the
  §8 non-regression ("no `Result String a` / `Task String a`") is enforced by
  the type system, not a lint.

## Ordered, implementable build tasks

1. **Canon registration (task #78).** Add `Ipe.Error` to
   `src/ipe-cli/src/stdlib.rs`: an `ERROR = include_str!("../stdlib/Ipê/Core/Error.ipe")`
   const + a `MODULES` entry (drop the byte-identical copy under
   `src/stdlib/Ipê/Core/Error.ipe`). As a compiled source module its
   functions resolve as ordinary top-level bindings (`Callee::TopLevel`) — no
   N0004, no per-function QUALIFIERS wiring. Keep `Error`/`ErrorKind`/
   `ErrorDetails` in `RESERVED_BUILTIN_TYPES` (`src/compiler/canon/src/resolve.rs`)
   but exempt the canonical declarer via a `{Maybe→Ipe.Maybe,
   Result→…, Error→Ipe.Error}` owner table (reuse the exemption Maybe.ipe
   already relies on; make it explicit if currently implicit-by-compile-order).
   Repoint the Prelude `errorToString` to resolve to `Ipe.Error.toString`.
2. **Kernels.** **None for the helpers** — they are compiled Ipê bodies (this is
   the soundness win: the Ipê compiler exhaustiveness-checks `kindLabel` /
   `isRetryable` / `toString`). The only wiring is the `errorToString` Prelude
   alias from step 1. *(Fallback only if compiling the Ipê bodies against the
   emitted structs surfaces friction — e.g. record-update codegen: demote the
   ~17 helpers to `KernelClass::Pure` StdlibKernel variants backed by
   `ipe_runtime::error::*`, each with matching `decl(qualifier="Error", …)`
   arity and a `("Error", name)` `lower_callee` arm. Strictly more code;
   avoided by default.)*
3. **HM scheme (`src/compiler/types/src/constrain.rs`).** Remove the String-magic
   co-treatment: keep `builtins.error` interned and `is_error_ty` (Task-channel
   validation) unchanged, but derive `Error : ErrorKind -> ErrorInfo -> Error`,
   the 11 nullary `ErrorKind` ctors, and the 5 `ErrorDetails` ctors **from the
   source ADT** like any user type. `errorToString : Error -> String`. This
   closes the #45 drift class for the Error family (schemes derived, not a
   hand-maintained table).
4. **Lowering (`src/compiler/lower/src/lower.rs`).** Split the shared arm at
   `:1730` and `:2089`: `"String" => Ok(IrType::Str)` stays; add
   `"Error" => Named(SkyCoreErrorError)` plus `"ErrorKind"`/`"ErrorDetails"` and
   the three record aliases `"ErrorInfo"`/`"PanicInfo"`/`"TypeInfo"` → their
   named runtime shapes (**key the alias bridge on the nominal name from the
   annotation, never on structural field-shape**, so an unrelated user record
   `{ message, details }` can't be mis-mapped). Seed the Error family into
   `enum_variants` + `ctor_arity` mirroring the SqlValue block at `:1108`
   (`Error`=2; all `ErrorKind`=0; `FfiPanic`/`TypeMismatch`/`HttpStatus`/
   `JsonDecode`/`Custom`=1) so `case err of Error kind info ->` and
   `case kind of Timeout ->` take the validated exhaustive-match path and
   `Error Io (mkInfo m)` lowers as a saturated construction — no bespoke arms.
5. **Backend emission (`src/compiler/backend/rust`).** Emit the Error ADT + the
   three record structs through the ordinary user-type path with
   `#[derive(Clone, Debug, PartialEq)]` (+`serde` only at boundaries that need
   it). Emit **unconditional** `type SkyError = SkyCoreErrorError;`, `str_err`,
   and `impl From<String> for SkyCoreErrorError` (→ `Unexpected`); have
   `str_err` delegate to `From<String>` so the shape exists once. Add
   `impl Display` + `impl std::error::Error` delegating to `toString` semantics
   but **never** folding `details`. Update the golden anchor
   `project.rs:193 START` from `"type SkyError = String;"` to the new alias, and
   emit a `use ipe_runtime::…` alias block so camelCase-mangled names resolve.
6. **Runtime construct / render (`src/runtime/rust/src`).** Keep the runtime
   **generic** over `E: Send + From<String>` — it never names `Error`. Confine
   the lossy `From<String>` to the runtime/FFI seam. Implement B8 redaction in
   `sky_error_from_foreign`: log the foreign `Debug` server-side under a
   correlation id (control bytes scrubbed), surface only `external operation
   failed (ref <id>)` to Ipê. Switch high-value origins (`http_client → network`,
   `time timeout → timeout`, `db → io`) to typed smart constructors as a
   **non-blocking follow-up** (until then those are `Unexpected` via
   `From<String>` — correct, low-fidelity).
7. **Golden test + regressions.** Regenerate **every** `tests/golden/*/main.rs`
   fixture carrying `type SkyError = String;` in lockstep (the intrusive ripple)
   and re-verify the `runtime_bindings()` slice anchors. Add fixtures exercising
   the full round-trip: `Task.fail (Error.unexpected "boom") |> onError (\e ->
   Error.toString e)`; `case` on `ErrorKind`; `isRetryable`;
   `withMessage`/`withDetails`; `kindLabel`. Add a **security regression**
   asserting a stack-bearing `Error` (`FfiPanic`) renders via `toString` with the
   stack **omitted**. Add a compile-fail regression that `Task String a` is a
   type error. Gate landing behind the go-oracle equivalence harness (task #51) — if
   upstream Go's `toString` format differs, it is a one-line change with the kind
   already in hand. Finally build at least one Live + one Db + one Task example
   end-to-end (`cargo build` of the emitted project), not just `ipe` exit-0.

## Rejected & why

- **A1 — opaque `struct SkyError { kind, message }` (no `ErrorDetails`,
  Ipê-invisible, render in Rust).** Rejected on **completeness (5)** and
  **soundness (3)**: it drops `isRetryable`/`kindLabel`/`ErrorDetails`
  (canon members would resolve to nothing) and forces rendering into Rust,
  forfeiting the Ipê compiler's exhaustiveness check over `ErrorKind`. It also
  gratuitously diverges from the proven upstream shape (**correctness (2)**).
  Opacity buys encapsulation the closed-total-fields design already achieves
  structurally.
- **A2 — runtime-owned opaque builtin + a kernel per function.** Rejected on
  **soundness (3)** and **readability (6)**: making `kindLabel`/`isRetryable`/
  `toString` Rust kernels duplicates the taxonomy in Rust and loses the
  compiler-checked exhaustiveness that pure-Ipê bodies give for free, at the
  cost of ~16 hand-maintained kernel/scheme/arity arms (drift surface). Kept
  only as the step-2 *fallback* if Ipê-body codegen proves infeasible.
- **A3's runtime-owned *type* (the Maybe/Result club).** A3's framing that
  Error.ipe is a compiled source module and the round-trip is by identity is
  **adopted**, but its choice to make the *type* runtime-owned is rejected in
  favour of upstream's emit-as-normal-ADT: the runtime-owned route needs a
  genuinely new `ErrorInfo`/`PanicInfo`/`TypeInfo` → runtime-struct bridge
  (A3's own flagged risk), whereas emitting via the ordinary user-type path
  reuses the existing record/ADT machinery, keeps the runtime blast radius at
  zero (runtime never names `Error`), and matches weeks of upstream investment
  (**correctness (2)** + lower risk).
- **Upstream's *conditional* `type SkyError = String` else-branch.** Rejected
  (this is upstream's own REJECT verdict): a program's error type must not
  depend on an incidental import. Make `type SkyError = SkyCoreErrorError`
  **unconditional** so the String path is unreachable *by construction* — an
  invalid state made unrepresentable, not merely unreached
  (**correctness/soundness/completeness**).
- **Using `From<String>` inside stdlib surfaces.** Rejected as a soft
  PARSE-DON'T-VALIDATE violation: it wraps rather than classifies, silently
  degrading typed errors to `Unexpected`. The bridge stays, but crate-private to
  the runtime/FFI seam; stdlib must pick a real kind.
- **Folding `ErrorDetails`/stack into user-facing `toString`.** Rejected on
  **security (1)**: `FfiPanic.stack` / `TypeInfo` can carry internal paths;
  `toString` renders kind+message only, details stay for debuggers/middleware.
