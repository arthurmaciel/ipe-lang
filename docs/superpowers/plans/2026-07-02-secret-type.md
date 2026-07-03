# Plan — Opaque `Secret` stdlib type (task #44)

## Goal

Add a first-class, opaque `Secret` type to the Sky-Rust stdlib whose runtime
representation makes **accidental disclosure structurally impossible**: a
`Secret` can never be `Display`-formatted, never `Debug`-formatted to reveal its
payload, never `toString`/interpolated to its plaintext, and never compared with
`==` (only via a constant-time kernel). The only way to read the wrapped bytes is
one explicit, grep-able boundary — `Secret.reveal`. This delivers the runtime
substrate the §8 non-regression ("secrets are typed — `Auth.signToken` /
`verifyToken` take `String`, not `any`; `fmt.Sprintf("%v", secret)` forbidden")
depends on, and the sealed value type the future WASM hydration island needs so a
signing key can be threaded through a handler without ever landing in a rendered
string.

### Scope boundary (what this plan does NOT do)

`Std.Auth` is **not yet ported** to the Rust backend (the registry has no
`Auth.signToken` / `verifyToken` kernels; `runtime/src/sky_runtime/auth.rs` exists
but is unreachable from Sky today). This plan ships the `Secret` *type and its
guarantees* as `Sky.Core.Secret`, standalone and fully tested. Re-typing
`Auth.signToken : Secret -> …` and wiring the WASM island are downstream consumers
that land when Auth ports; they are explicitly out of scope here and are named
only so the type's surface is designed to receive them. This is grounding, not a
capability comparison: where `../sky` types the auth secret as a plain `String`,
Sky-Rust goes one step further and gives it a sealed newtype — a Rust
type-system correctness gain (parallel to the `Bytes = Vec<u8>` divergence already
recorded in `docs/architecture/divergence-policy.md`), not a defect in the
reference.

## Architecture

`Secret` is modelled exactly like the existing opaque built-in primitives `Bytes`
(`Vec<u8>`) and `Db` (`DbPool` handle): a distinct `IrType` leaf, a runtime type
re-exported through `pub use sky_runtime::*`, kernel functions registered in the
closed `sky_kernels` registry, typed in `sky_types::constrain`, and dispatched in
`sky_lower`. The difference from `Bytes` is that `Secret`'s runtime rep is a
**sealed newtype** (`struct Secret(String)`), not a transparent alias — because the
transparent alias would inherit `String`'s `Display`/`Debug`/`PartialEq` and leak.

End-to-end path a `Secret` value travels (each is a wiring point below):

```
Sky source  Secret.fromString "sk_live_…"
  parse/canon   → qualifier "Secret", name "fromString"   (env.rs QUALIFIERS)
  lower         → Callee::Kernel(KernelFn::SecretFromString)  (lower.rs lower_callee)
  type          → String -> Secret                       (constrain.rs kernel_ty)
  ir type       → IrType::Secret                          (sky_ir ir.rs)
  emit          → secret_from_string(arg)  : Secret       (naming.rs + emit_types.rs)
  runtime       → Secret(String)  [redacting Debug, no Display, no PartialEq]
```

Because `emit_expr.rs` dispatches every non-HOF kernel generically
(`Callee::Kernel(k) => Ok(kernel_name(*k))`, `emit_expr.rs:107`), the four `Secret`
kernels need **no** `emit_expr` change — they emit as plain `fn(args)` calls.

## Tech Stack

- Rust (workspace, `rust-toolchain.toml`), GHC-free — this is the Rust port.
- Runtime crate `sky_runtime` (`runtime/`); `subtle = "2"` is already a
  non-optional dependency (`runtime/Cargo.toml:38`) and provides
  `ConstantTimeEq` — reused for `Secret`, no new dep for the core type.
- Compiler crates: `sky_kernels` (leaf registry), `sky_types` (HM constrain/solve),
  `sky_lower` (IR lowering), `sky_ir` (IR), `sky_backend_rust` (emit), `sky_canon`
  (name resolution), `skyc` (driver + embedded stdlib).
- Golden E2E harness: `crates/skyc/tests/support/mod.rs` +
  `tests/golden/<name>/{Main.sky,oracle.meta,expected_go.txt}`, gated `SKY_E2E=1`,
  shared cargo target at `~/.cache/sky-rust-target`.

## Global Constraints

1. **PRINCIPLES order (strict, from `PRINCIPLES.md`):** security > correctness >
   soundness > efficiency > completeness > readability. When two pulls conflict,
   the earlier wins. For `Secret`, security is the whole point: a redaction that
   costs an allocation beats a fast path that risks a leak.
2. **PARSE, DON'T VALIDATE.** The moment a plaintext crosses `Secret.fromString`
   it becomes a `Secret`; downstream code receives the sealed type, not a
   `String` it must remember not to log. There is exactly one un-parse
   (`Secret.reveal`), and it is a named, greppable boundary — not a scattered
   convention.
3. **MAKE INVALID STATES UNREPRESENTABLE.** "A secret that can be printed" is not
   representable: the newtype omits `Display`, its `Debug` is redacting, its
   `SkyStringify` is redacting, and it derives neither `PartialEq` nor `Hash` — so
   "a secret compared in variable time" and "a secret used as a map key" are also
   unrepresentable. The type checker rejects `secret == secret` at SKY-T0014
   (Task 9) rather than letting it reach cargo.
4. **Fail closed, no wildcards.** Every new match arm is explicit; unregistered
   kernels must not fall through to the `Ty::Var(u32::MAX)` sentinel
   (`constrain.rs:4249`) — that is the exit-0-then-cargo-fail class task #45
   tracks. New IR variant → every exhaustive `match` gets a real arm (the
   compiler enforces this), never a `_ =>`.
5. **No new reachable panic.** Runtime code is under the panic-class clippy denies
   (`runtime/src/lib.rs`): no `unwrap`/`expect`/`panic`/indexing in non-test code.
6. **Divergence discipline.** `Secret` is a sanctioned Rust-correctness divergence
   (sealed newtype vs a plain-string secret); record the rationale in the module
   doc comment and `docs/architecture/divergence-policy.md`, matching the `Bytes`
   precedent.

## Parallel-safety / file-overlap callouts

This work **overlaps** two in-flight efforts. Land in this order and rebase, or
coordinate the shared files:

- **Registry migration (Phase B, HEAD commit `691e275`) — `constrain.rs`,
  `sky_kernels/src/lib.rs`, `sky_lower/src/lower.rs` `lower_callee`.** Phase B is
  threading `VarHome::Kernel(KernelId)` and retiring the legacy `(Symbol, Symbol)`
  table. Both this plan and Phase B edit the **same three files' kernel tables**.
  Mitigation: (a) append `Secret` variants at the **end** of the `StdlibKernel`
  enum / `decl` / `ALL` (Task 5) so discriminants don't shift under Phase B's
  reordering; (b) add the `("Secret", …)` `lower_callee` arms and the
  `(Some("Secret"), …)` `kernel_ty` arms as **new, self-contained arm blocks**, not
  edits to existing arms — a clean 3-way merge. If Phase B has already retired the
  string-tuple `lower_callee` path, register the callee through whatever
  `VarHome::Kernel` seam replaced it (the KernelId is `StdlibKernel::Secret*`);
  the enum/decl/ALL/QUALIFIERS/constrain work is unchanged either way.
- **Task 9 shares `ty_is_equatable` (`sky_types/src/lib.rs:309`) with task #45.**
  #45 will add the full opaque-Con denylist (Task/Db/Decoder/Cmd/Sub/Request/
  Response/Route/Cookie) flagged by the 2026-07-01 principles audit. This plan
  adds **only `"Secret"`** to that gate, via a named helper
  `is_opaque_non_equatable_con` so #45 extends one list rather than re-deriving
  the check. Note the shared edit in the #45 task and the #44 task so whoever
  lands second just adds names to the helper.
- **#49 TCO — no overlap.** #49 adds two `sky_ir` variants and edits
  `lower.rs`/`emit_expr.rs` **statement/expression** emission (loop/continue). This
  plan adds one `sky_ir` **type** variant (`IrType::Secret`) and edits
  **type**-lowering + kernel tables. Different regions of the same crates; adding
  an `IrType` variant forces exhaustive-match arms that #49 does not touch. No
  logical conflict — a mechanical merge at most.

---

## Task 1 — Runtime `Secret` newtype + four kernels

**Files:**
- create `runtime/src/sky_runtime/secret.rs`
- edit `runtime/src/sky_runtime/mod.rs` (register + re-export the module)

**Interfaces**

Consumes: `subtle::ConstantTimeEq` (already a dep, `runtime/Cargo.toml:38`);
`crate::sky_runtime::stringify::SkyStringify` (`stringify.rs:32`, method
`fn sky_show(&self) -> String`).

Produces (public runtime surface, all reachable via `pub use sky_runtime::*`):

```rust
pub struct Secret(String);                       // sealed newtype
pub fn secret_from_string(s: String) -> Secret;  // Secret.fromString : String -> Secret
pub fn secret_reveal(s: Secret) -> String;       // Secret.reveal : Secret -> String
pub fn secret_redacted(s: Secret) -> String;     // Secret.redacted : Secret -> String  ("<redacted>")
pub fn secret_constant_time_eq(a: Secret, b: Secret) -> bool; // Secret.constantTimeEq : Secret -> Secret -> Bool
```

Invariants the type upholds (each has a test below):
- `impl Debug` is **redacting** — prints `Secret("<redacted>")`, never the payload.
- **No `Display`** impl exists.
- **No `PartialEq`/`Eq`/`Hash`** derive — `==` and map-key use are compile errors.
- `impl SkyStringify` returns `"<redacted>"` (so Sky `toString`/interpolation
  redacts; the concrete-type impl wins the zero-autoref dispatch over the
  `ViaDebug` fallback in `stringify.rs`).
- `#[derive(Clone)]` only (Sky eval clones values; cloning a sealed secret is safe).

**Steps**

1. Write the failing test file first. Create `runtime/src/sky_runtime/secret.rs`
   with the module skeleton and a `#[cfg(test)]` block, then the impl. Content:

```rust
//! `Sky.Core.Secret` — a sealed, non-disclosing wrapper for sensitive strings.
//!
//! Divergence from Sky: the reference types an auth secret as a plain `String`.
//! Sky-Rust wraps it in an opaque newtype that CANNOT be Display-formatted,
//! CANNOT be Debug-formatted to its payload, CANNOT be `toString`/interpolated to
//! plaintext, and CANNOT be `==`-compared (only `Secret.constantTimeEq`). The
//! only read boundary is `Secret.reveal` — one greppable un-parse. Rationale:
//! make accidental disclosure (the `fmt.Sprintf("%v", secret)` class) a type
//! error rather than a review convention. See docs/architecture/divergence-policy.md.

use subtle::ConstantTimeEq;

use crate::sky_runtime::stringify::SkyStringify;

/// The redaction marker rendered anywhere a `Secret` would otherwise stringify.
const REDACTED: &str = "<redacted>";

/// A sealed secret string. Constructed only via [`secret_from_string`]; read only
/// via [`secret_reveal`]. Derives `Clone` and nothing else — no `PartialEq`
/// (equality is constant-time only), no `Hash` (never a map key), no `Display`.
#[derive(Clone)]
pub struct Secret(String);

// Redacting Debug: even the `ViaDebug` fallback in `stringify` prints the marker,
// never the payload. This is the belt to `SkyStringify`'s suspenders.
impl core::fmt::Debug for Secret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Secret({REDACTED:?})")
    }
}

// Sky `toString` / `{{interpolation}}` route through SkyStringify; redact.
impl SkyStringify for Secret {
    fn sky_show(&self) -> String {
        REDACTED.to_owned()
    }
}

/// `Secret.fromString : String -> Secret` — seal a plaintext.
pub fn secret_from_string(s: String) -> Secret {
    Secret(s)
}

/// `Secret.reveal : Secret -> String` — the single explicit un-parse boundary.
pub fn secret_reveal(s: Secret) -> String {
    s.0
}

/// `Secret.redacted : Secret -> String` — the safe display companion; always the
/// marker, so a log/UI can show *that* a secret is present without its value.
pub fn secret_redacted(_s: Secret) -> String {
    REDACTED.to_owned()
}

/// `Secret.constantTimeEq : Secret -> Secret -> Bool` — timing-safe compare.
/// Uses `subtle::ConstantTimeEq` over the raw bytes; the branch on length is
/// itself not secret (length is not the payload).
pub fn secret_constant_time_eq(a: Secret, b: Secret) -> bool {
    let (ab, bb) = (a.0.as_bytes(), b.0.as_bytes());
    if ab.len() != bb.len() {
        return false;
    }
    bool::from(ab.ct_eq(bb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_payload() {
        let s = secret_from_string("sk_live_TOPSECRET".to_owned());
        let shown = format!("{s:?}");
        assert!(!shown.contains("TOPSECRET"), "Debug leaked payload: {shown}");
        assert!(shown.contains(REDACTED));
    }

    #[test]
    fn sky_show_redacts_payload() {
        let s = secret_from_string("sk_live_TOPSECRET".to_owned());
        assert_eq!(s.sky_show(), REDACTED);
    }

    #[test]
    fn reveal_round_trips() {
        let s = secret_from_string("hunter2".to_owned());
        assert_eq!(secret_reveal(s), "hunter2");
    }

    #[test]
    fn redacted_never_returns_payload() {
        let s = secret_from_string("hunter2".to_owned());
        assert_eq!(secret_redacted(s), REDACTED);
    }

    #[test]
    fn constant_time_eq_matches_and_mismatches() {
        let a = secret_from_string("abc".to_owned());
        let b = secret_from_string("abc".to_owned());
        let c = secret_from_string("abd".to_owned());
        let d = secret_from_string("abcd".to_owned());
        assert!(secret_constant_time_eq(a, b));
        assert!(!secret_constant_time_eq(
            secret_from_string("abc".to_owned()),
            c
        ));
        assert!(!secret_constant_time_eq(
            secret_from_string("abc".to_owned()),
            d
        ));
    }
}
```

2. Register the module in `runtime/src/sky_runtime/mod.rs`. Add next to the other
   `pub mod … / pub use …::*` lines (e.g. after the `bytes` pair at `mod.rs:66-67`):

```rust
pub mod secret;
pub use secret::*;
```

3. Run the failing test (module not yet wired → these are new tests, so they run
   as soon as the module compiles):

```bash
cargo test -p sky_runtime secret::
```

Expected once the impl above is in place:

```
running 5 tests
test sky_runtime::secret::tests::debug_redacts_payload ... ok
test sky_runtime::secret::tests::sky_show_redacts_payload ... ok
test sky_runtime::secret::tests::reveal_round_trips ... ok
test sky_runtime::secret::tests::redacted_never_returns_payload ... ok
test sky_runtime::secret::tests::constant_time_eq_matches_and_mismatches ... ok
```

4. Confirm the "no Display / no PartialEq" invariants are enforced by the type
   system, not just convention. Add a `compile_fail` doctest to `secret.rs` (doc on
   the `Secret` struct):

```rust
/// ```compile_fail
/// use sky_runtime::sky_runtime::secret::{secret_from_string, Secret};
/// let s = secret_from_string("x".to_owned());
/// let _ = format!("{s}");            // no Display — must not compile
/// ```
///
/// ```compile_fail
/// use sky_runtime::sky_runtime::secret::secret_from_string;
/// let a = secret_from_string("x".to_owned());
/// let b = secret_from_string("x".to_owned());
/// let _ = a == b;                    // no PartialEq — must not compile
/// ```
```

Run: `cargo test -p sky_runtime --doc secret` → expect the compile_fail doctests
to pass (i.e. they *fail to compile* as required).

5. Clippy gate (the crate denies panic-class lints on non-test code):

```bash
cargo clippy -p sky_runtime --all-targets -- -D warnings
```

Expected: `Finished` with no warnings (the impl has no `unwrap`/`expect`/index).

6. Commit: `feat(runtime): sealed Secret newtype — redacting Debug, no Display/Eq, constant-time compare`.

---

## Task 2 — `IrType::Secret` variant

**Files:** edit `crates/sky_ir/src/ir.rs`; edit `crates/sky_ir/src/pretty.rs` if it
matches exhaustively on `IrType`.

**Interfaces**

Consumes: nothing new. Produces: `IrType::Secret` (nullary opaque leaf).

**Steps**

1. Add the variant next to the other opaque leaves (`IrType::Db` at `ir.rs:504`):

```rust
    /// The `Secret` type — a sealed, non-disclosing string wrapper (`Sky.Core.Secret`).
    ///
    /// Zero type arguments. Renders as `Secret`, the runtime newtype re-exported
    /// via `pub use sky_runtime::*`. Opaque: the backend never synthesises a
    /// struct body for it and never treats it as a function type.
    Secret,
```

2. Build the crate to surface every exhaustive match that now needs an arm:

```bash
cargo build -p sky_ir
```

Expected: `error[E0004]: non-exhaustive patterns: \`Secret\` not covered` at each
`match` over `IrType` inside `sky_ir` (e.g. `pretty.rs`). For each, add an arm
mirroring `IrType::Db` (opaque, renders/pretty-prints as the bare name `Secret`).
Re-run until `cargo build -p sky_ir` is clean.

3. Add a pretty-print unit test if `pretty.rs` has a test module (mirror the `Db`
   case): assert `IrType::Secret` pretty-prints to `"Secret"`. Run
   `cargo test -p sky_ir`.

4. Commit: `feat(ir): IrType::Secret opaque leaf`.

---

## Task 3 — Emit `IrType::Secret` as the runtime `Secret` type

**Files:** edit `crates/sky_backend_rust/src/emit_types.rs`.

**Interfaces**

Consumes: `IrType::Secret`. Produces: the Rust type string `"Secret"` (resolved in
emitted code via the golden preamble's `pub use sky_runtime::*;`,
`tests/golden/m0/main.rs:9` — **no golden edit needed**, exactly like `Db`).

**Steps**

1. Add the render arm next to `IrType::Db => "Db".to_owned()` (`emit_types.rs:130`):

```rust
        // `Secret` is the opaque sealed-string type, re-exported from the runtime
        // via `pub use sky_runtime::*`. Never synthesised as a struct.
        IrType::Secret => "Secret".to_owned(),
```

2. Build to catch any other exhaustive `IrType` match in the backend crate:

```bash
cargo build -p sky_backend_rust
```

Add opaque arms (render as `"Secret"`, no type params) wherever the compiler flags
a missing pattern. Re-run until clean.

3. Unit test (mirror the existing `Db`/`Bytes` render test in `emit_types.rs`
   tests): assert `render_type(ctx, &IrType::Secret, &[])` yields `"Secret"`. Run
   `cargo test -p sky_backend_rust emit_types`.

4. Commit: `feat(backend): render IrType::Secret as runtime Secret`.

---

## Task 4 — Lower `Secret` type annotations + kernel callees + arities

**Files:** edit `crates/sky_lower/src/lower.rs`.

**Interfaces**

Consumes: canon `Type::Con { name: "Secret" }`, solved `Ty::Con { name: "Secret" }`,
canon callee `(qualifier "Secret", name)`. Produces: `IrType::Secret`,
`Callee::Kernel(KernelFn::Secret*)`, kernel arities.

Four anchors (all verified at HEAD):

- `ir_type_from_canon` (`lower.rs:1396`) — the `"Bytes" => Ok(IrType::Bytes)` arm at
  `lower.rs:1414`.
- `ir_type_from_ty` (`lower.rs:1746`) — the `"Bytes" => Ok(IrType::Bytes)` arm at
  `lower.rs:1766`.
- `callee_arity` (`lower.rs:2839`) — arity-1 arm block (Bytes arity-1 at
  `lower.rs:2918`) and the arity-2 arm block.
- `lower_callee` string-tuple dispatch — Bytes arms at `lower.rs:3691-3699`.

**Steps**

1. Write a failing lower-level test first. In the `sky_lower` test suite add a case
   that lowers a Sky binding `k : Secret` / `k = Secret.fromString "x"` and asserts
   the callee lowers to `KernelFn::SecretFromString` and the annotation lowers to
   `IrType::Secret`. (Follow the existing Bytes lowering test shape; if none, add a
   focused `#[test]` in `lower.rs`'s test module using the same fixture builders as
   the Bytes tests.) Run and watch it fail with "Secret not covered" / unknown
   callee.

2. Add the type arm in **both** `ir_type_from_canon` and `ir_type_from_ty`, next to
   the `Bytes` arm in each:

```rust
                // `Secret` is a built-in opaque sealed-string primitive
                // (Sky.Core.Secret). Zero type arguments; maps to the runtime
                // `Secret` newtype. Divergence: the reference types the auth
                // secret as a plain String; Sky-Rust seals it.
                "Secret" => Ok(IrType::Secret),
```

3. Add the callee arms in `lower_callee`, as a self-contained block after the Bytes
   arms (`lower.rs:3699`):

```rust
                    // ── Sky.Core.Secret ──────────────────────────────────────
                    ("Secret", "fromString") => Ok(Callee::Kernel(KernelFn::SecretFromString)),
                    ("Secret", "reveal") => Ok(Callee::Kernel(KernelFn::SecretReveal)),
                    ("Secret", "redacted") => Ok(Callee::Kernel(KernelFn::SecretRedacted)),
                    ("Secret", "constantTimeEq") => {
                        Ok(Callee::Kernel(KernelFn::SecretConstantTimeEq))
                    }
```

   (If the registry migration has already replaced this string-tuple path, register
   the same four `StdlibKernel::Secret*` ids through the `VarHome::Kernel` seam
   instead — see the parallel-safety note.)

4. Add the arities in `callee_arity`: `SecretFromString`, `SecretReveal`,
   `SecretRedacted` to the arity-1 arm (alongside the Bytes arity-1 variants at
   `lower.rs:2918`); `SecretConstantTimeEq` to the arity-2 arm. Each must be listed
   explicitly (the arm's own comment: "a new entry can never silently inherit a
   wrong count", `lower.rs:2841`).

5. `cargo build -p sky_lower` — resolve any remaining `IrType`/`KernelFn`
   exhaustiveness errors with explicit arms (never `_ =>`). Then run the Task-4
   test:

```bash
cargo test -p sky_lower -- secret
```

Expected: the new lowering test passes; no other `sky_lower` test regresses.

6. Commit: `feat(lower): lower Secret type + fromString/reveal/redacted/constantTimeEq callees`.

---

## Task 5 — Register the four kernels in `sky_kernels`

**Files:** edit `crates/sky_kernels/src/lib.rs`.

**Interfaces**

Consumes: nothing. Produces four `StdlibKernel` variants + their `decl` +
`ALL` membership. `decl` fields (`StdlibDecl`, `lib.rs:54`): `qualifier`, `name`,
`arity`, `class`, `emit` (the runtime symbol from Task 1).

**Steps**

1. Append the four variants at the **end** of the `StdlibKernel` enum (before the
   closing `}` at `lib.rs:541`, after `UiOnBool,`) so no discriminant shifts under
   the concurrent Phase B reordering:

```rust
    // ── Sky.Core.Secret ───────────────────────────────────────────────────────
    SecretFromString,
    SecretReveal,
    SecretRedacted,
    SecretConstantTimeEq,
```

2. Add the `decl` arms in the `decl()` match (`lib.rs:550`), using the `d(…)`
   shorthand (`lib.rs:552`), mirroring the Bytes rows (`lib.rs:705`). The `emit`
   strings MUST equal the Task-1 runtime fn names:

```rust
            Self::SecretFromString => d("Secret", "fromString", 1, Pure, "secret_from_string"),
            Self::SecretReveal => d("Secret", "reveal", 1, Pure, "secret_reveal"),
            Self::SecretRedacted => d("Secret", "redacted", 1, Pure, "secret_redacted"),
            Self::SecretConstantTimeEq => {
                d("Secret", "constantTimeEq", 2, Pure, "secret_constant_time_eq")
            }
```

3. Append the four to `ALL` (before the closing `];` at `lib.rs:1577`):

```rust
        // Secret
        Self::SecretFromString,
        Self::SecretReveal,
        Self::SecretRedacted,
        Self::SecretConstantTimeEq,
```

4. Add the emit-name mapping in `crates/sky_backend_rust/src/naming.rs`
   `kernel_name` (`naming.rs:221`), next to the Bytes block (`naming.rs:361`):

```rust
        // ── Secret kernels ──────────────────────────────────────────────────
        KernelFn::SecretFromString => "secret_from_string",
        KernelFn::SecretReveal => "secret_reveal",
        KernelFn::SecretRedacted => "secret_redacted",
        KernelFn::SecretConstantTimeEq => "secret_constant_time_eq",
```

   (`kernel_name` and `decl().emit` are two tables that must agree; a mismatch is
   caught by the drift test in step 6 if one exists, otherwise by the E2E golden in
   Task 10 failing to link.)

5. `cargo build -p sky_kernels -p sky_backend_rust` — resolve any remaining
   `KernelFn` exhaustiveness errors (e.g. a `class`-grouping match) with explicit
   `Pure` arms.

6. Run the registry's own tests:

```bash
cargo test -p sky_kernels
```

Expected: all pass, including any `ALL`-length / `decl`-parity self-test.

7. Commit: `feat(kernels): register Secret.{fromString,reveal,redacted,constantTimeEq}`.

---

## Task 6 — Type the `Secret` kernels in `constrain`

**Files:** edit `crates/sky_types/src/constrain.rs`.

**Interfaces**

Consumes: the interner. Produces: an interned `secret` type symbol, a `secret`
`Ty::Con`, and four `kernel_ty` scheme arms. These MUST be explicit so the kernels
never fall to the `_ => Ty::Var(u32::MAX)` sentinel (`constrain.rs:4249`) — the
exit-0-then-cargo-fail class.

Schemes:

```
Secret.fromString    : String -> Secret
Secret.reveal        : Secret -> String
Secret.redacted      : Secret -> String
Secret.constantTimeEq: Secret -> Secret -> Bool
```

**Steps**

1. Add a `secret: Symbol` field to the `Builtins` struct (near `bytes` at
   `constrain.rs:70`) and intern it in `Builtins::new` (near
   `bytes: interner.intern("Bytes")?` at `constrain.rs:223`):

```rust
    /// `Sky.Core.Secret` opaque type-constructor symbol.
    secret: Symbol,
```
```rust
            secret: interner.intern("Secret")?,
```

2. In `kernel_ty` (`constrain.rs:2061`), where the local `bytes` `Ty::Con` is built
   (`constrain.rs:2119`), add the `secret` con and the scheme arms next to the
   Bytes block (`constrain.rs:2308`):

```rust
        // `secret` is a zero-argument opaque constructor: `Secret`.
        let secret = Ty::Con {
            module: Vec::new(),
            name: self.builtins.secret,
            args: Vec::new(),
        };
```
```rust
            // ── Sky.Core.Secret ──────────────────────────────────────────
            // fromString : String -> Secret
            (Some("Secret"), Some("fromString")) => fun(string.clone(), secret.clone()),
            // reveal : Secret -> String  (the single explicit un-parse)
            (Some("Secret"), Some("reveal")) => fun(secret.clone(), string.clone()),
            // redacted : Secret -> String
            (Some("Secret"), Some("redacted")) => fun(secret.clone(), string),
            // constantTimeEq : Secret -> Secret -> Bool
            (Some("Secret"), Some("constantTimeEq")) => {
                fun(secret.clone(), fun(secret.clone(), bool_ty))
            }
```

   (Confirm the in-scope local names for the `String`/`Bool` con builders in this
   function — the Bytes block uses `string` and `bool_ty`; reuse the same locals,
   cloning where the value is consumed more than once, matching the surrounding
   style.)

3. Write/extend a constrain unit test that infers the type of each `Secret.*` call
   and asserts the resolved `Ty` (e.g. `Secret.fromString "x"` ⇒
   `Ty::Con { name: Secret, args: [] }`; `Secret.reveal s` ⇒ `String`). Follow the
   Bytes constrain test shape. Run:

```bash
cargo test -p sky_types -- secret
```

Expected: passes; no fall-through to `Ty::Var(u32::MAX)`.

4. Commit: `feat(types): constrain schemes for Secret kernels`.

---

## Task 7 — Register the `Secret` qualifier in canon

**Files:** edit `crates/sky_canon/src/env.rs`.

**Interfaces**

Consumes: nothing. Produces: a `QUALIFIERS` entry so `Secret.fromString` etc.
resolve as known kernel references (and the `canon_equals_registry` tripwire,
`sky_canon/src/lib.rs:1355`, sees parity with the registry `ALL` from Task 5).

**Steps**

1. Add the entry to the `QUALIFIERS` table (`env.rs:203`), next to the `Bytes`
   entry (`env.rs:369`):

```rust
            // `Sky.Core.Secret` — sealed-secret kernels.
            (
                "Secret",
                &["fromString", "reveal", "redacted", "constantTimeEq"],
            ),
```

2. Run the tripwire that pins canon ↔ registry parity:

```bash
cargo test -p sky_canon canon_equals_registry
```

Expected: passes (every registry `ALL` variant — including the four new Secret
ones — has a matching `QUALIFIERS` entry, and vice-versa). If it fails listing a
`Secret.*` name, the qualifier list and the Task-5 `decl` names disagree — fix the
spelling to match.

3. Commit: `feat(canon): register Secret qualifier`.

---

## Task 8 — Embed the `Sky.Core.Secret` stdlib module

**Files:** create `crates/skyc/stdlib/Sky/Core/Secret.sky`; edit
`crates/skyc/src/stdlib.rs`.

**Interfaces**

Consumes: the kernel dispatch wired in Tasks 4–7. Produces: an importable module
`Sky.Core.Secret exposing (Secret, fromString, reveal, redacted, constantTimeEq)`.
Mirrors `Sky.Core.Bytes` exactly — the built-in type name `Secret` is *exposed*
without a `type` declaration (canon does not require one; `Bytes` does the same),
and each function is an `Ffi.kernel "…"` alias carrying its HM signature.

**Steps**

1. Create `crates/skyc/stdlib/Sky/Core/Secret.sky`:

```elm
module Sky.Core.Secret exposing
    ( Secret
    , fromString
    , reveal
    , redacted
    , constantTimeEq
    )

-- Divergence from Sky: the reference types an auth secret as a plain `String`.
-- Sky-Rust seals it in an opaque `Secret` that cannot be Display-formatted,
-- Debug-formatted to its payload, `toString`-ed to plaintext, or `==`-compared
-- (only `constantTimeEq`).  The sole read boundary is `reveal`.  Rationale:
-- make accidental disclosure a type error, not a review convention.


-- | Seal a plaintext string into a `Secret`.
fromString : String -> Secret
fromString =
    Ffi.kernel "Secret_fromString"


-- | Reveal the wrapped plaintext — the single explicit, auditable un-parse.
reveal : Secret -> String
reveal =
    Ffi.kernel "Secret_reveal"


-- | The safe display companion — always the redaction marker, never the value.
redacted : Secret -> String
redacted =
    Ffi.kernel "Secret_redacted"


-- | Timing-safe equality of two secrets.  Prefer this over `==`, which is a
-- type error on `Secret` by design.
constantTimeEq : Secret -> Secret -> Bool
constantTimeEq =
    Ffi.kernel "Secret_constantTimeEq"
```

   (Confirm the `Ffi.kernel "Name"` string convention against `Bytes.sky` /
   `Crypto.sky` — the reference uses the `Qualifier_function` shape, e.g.
   `"Bytes_empty"`. The string is documentation/registry-facing; the actual
   dispatch is by qualifier+name via Tasks 4–7, so these strings must read
   consistently with the rest of the stdlib.)

2. Register the module in `crates/skyc/src/stdlib.rs`: add the `include_str!`
   const next to `CRYPTO` (`stdlib.rs:47`) and the `StdModule` entry in `MODULES`:

```rust
/// `Sky.Core.Secret` — sealed-secret opaque type + kernels.
const SECRET: &str = include_str!("../stdlib/Sky/Core/Secret.sky");
```
```rust
    StdModule {
        name: "Sky.Core.Secret",
        source: SECRET,
    },
```

3. `sky fmt` the new `.sky` file (idempotent — two passes byte-identical), then
   confirm it parses via the existing stdlib `parses` test:

```bash
cargo test -p skyc stdlib
```

Expected: the stdlib parse/embedding test passes with `Sky.Core.Secret` included.

4. Commit: `feat(stdlib): embed Sky.Core.Secret module`.

---

## Task 9 — Reject `==` on `Secret` at SKY-T0014 (security gate)

**Files:** edit `crates/sky_types/src/lib.rs`.

**Interfaces**

Consumes: `Ty`. Produces: `ty_is_equatable(&Secret) == false`, so an equality
obligation on a `Secret` fails closed with SKY-T0014 (SuperTypeUnsatisfied) at
type-check time instead of reaching cargo.

**Why:** `ty_is_equatable` (`lib.rs:309`) currently returns `true` for any
zero-arg `Ty::Con` (`Ty::Con { args, .. } => args.iter().all(ty_is_equatable)` —
empty args ⇒ vacuously true, `lib.rs:315`). So `secret == secret` would type-check
today, then either fail cargo (no `PartialEq` derive — the exit-0-then-cargo-fail
class) or, worse, if `Secret` ever derived `PartialEq`, silently permit a
variable-time compare. Both violate the security principle. This gate makes
"a secret compared with `==`" unrepresentable at the Sky level.

**Shared-file note:** this is the same function task #45 will extend with the full
opaque-Con denylist. Add a **named helper** so #45 appends names rather than
re-deriving the check; note the shared edit in both tasks.

**Steps**

1. Write the failing type-check test first: a Sky program `bad = s == s` where
   `s : Secret` must produce SKY-T0014. In the `sky_types` test suite (mirror an
   existing SuperTypeUnsatisfied test), assert the diagnostic code. Run and watch
   it currently **pass type-check** (bug) — i.e. the test asserting an *error*
   fails because no error is raised.

2. Add the helper and thread it into `ty_is_equatable` (`lib.rs:309`):

```rust
/// Opaque runtime type-constructors whose emitted Rust rep deliberately does NOT
/// derive `PartialEq`, so `==` / `/=` on them must be a type error (SKY-T0014)
/// rather than reaching cargo. `Secret` is here for a security reason: equality
/// on a secret must be constant-time (`Secret.constantTimeEq`), never `==`.
///
/// Task #45 extends this list with the rest of the audit-flagged opaque set
/// (Task/Db/Decoder/Cmd/Sub/Request/Response/Route/Cookie); keep it a single
/// name list so that work appends here.
fn is_opaque_non_equatable_con(interner: &Interner, name: Symbol) -> bool {
    matches!(interner.resolve(name), Some("Secret"))
}
```

   In `ty_is_equatable`, change the `Con` arm from unconditional-on-args to reject
   the opaque set. Because `ty_is_equatable` currently takes only `&Ty` and has no
   interner, either (a) thread the `interner` into `ty_is_equatable` and its two
   callers `emitted_bound_satisfied` / `concrete_super_ok` (both already hold an
   `interner: &Interner`, `lib.rs:258` / `lib.rs:286`), or (b) resolve the name at
   those two call sites before recursing. Prefer (a) for one source of truth:

```rust
fn ty_is_equatable(interner: &Interner, ty: &Ty) -> bool {
    match ty {
        Ty::Var(_) | Ty::Fun(_, _) => false,
        Ty::Unit => true,
        Ty::Tuple(elems) => elems.iter().all(|t| ty_is_equatable(interner, t)),
        Ty::Record(fields) => fields.values().all(|t| ty_is_equatable(interner, t)),
        Ty::Con { name, args, .. } => {
            !is_opaque_non_equatable_con(interner, *name)
                && args.iter().all(|t| ty_is_equatable(interner, t))
        }
    }
}
```

   Update the two call sites (`lib.rs:274`, `lib.rs:300`) to pass `interner`.

3. Re-run the Task-9 test — it now sees SKY-T0014 and passes. Also run the full
   `sky_types` suite to confirm no equatable type regressed (Int/String/tuple/enum
   equality must still type-check):

```bash
cargo test -p sky_types
```

Expected: all pass, including the new SKY-T0014-on-Secret test.

4. Commit: `feat(types): reject == on Secret (SKY-T0014) — use constantTimeEq`.

---

## Task 10 — End-to-end golden: seal, reveal, redact, compare

**Files:** create `tests/golden/secret_roundtrip/Main.sky`,
`tests/golden/secret_roundtrip/oracle.meta`,
`tests/golden/secret_roundtrip/expected_go.txt`; create
`crates/skyc/tests/golden_secret.rs`.

**Interfaces**

Consumes: the full pipeline (Tasks 1–9). Produces: proof that a `Secret` compiles,
builds, runs, and that (a) `reveal` returns the plaintext, (b) `toString` /
`redacted` redact, (c) `constantTimeEq` works, (d) the emitted program's stdout
never contains the plaintext except where `reveal` was explicitly called.

**Steps**

1. `tests/golden/secret_roundtrip/Main.sky`:

```elm
module Main exposing (main)

import Sky.Core.Prelude exposing (..)
import Sky.Core.Secret as Secret
import Std.Log exposing (println)


main =
    let
        s =
            Secret.fromString "sk_live_TOPSECRET"

        _ =
            println (Secret.redacted s)

        _ =
            println (toString s)

        _ =
            println (toString (Secret.constantTimeEq s (Secret.fromString "sk_live_TOPSECRET")))

        _ =
            println (toString (Secret.constantTimeEq s (Secret.fromString "different")))
    in
    println (Secret.reveal s)
```

   Expected program stdout:

```
<redacted>
<redacted>
True
False
sk_live_TOPSECRET
```

   (Only the final line — the explicit `reveal` — carries the plaintext; the two
   redaction lines prove `toString`/`redacted` never leak. Confirm the `Bool`
   stringification casing `True`/`False` against the runtime `SkyStringify for
   bool` impl; adjust the expected lines to whatever the runtime actually emits.)

2. Write `expected_go.txt` with those five lines and `oracle.meta` mirroring an
   existing golden's meta (copy the shape from
   `tests/golden/m5a_crypto_sha_hash/oracle.meta`; since `Secret` is a Rust-only
   divergence with no Go oracle, mark the oracle as self-authored per the
   divergence convention already used for other `Bytes`/divergent goldens — follow
   whatever `oracle.meta` field the harness reads for "no Go oracle, pin the Rust
   output").

3. Create `crates/skyc/tests/golden_secret.rs` mirroring
   `crates/skyc/tests/golden_m4g_json_enc.rs` (`mod support;`,
   `assert_runs_and_matches_oracle("secret_roundtrip")`, gated on `SKY_E2E=1`).

4. Run the E2E golden (shared target — first run compiles deps, then fast):

```bash
SKY_E2E=1 cargo test -p skyc golden_secret
```

Expected: builds the emitted project, runs it, stdout equals the five expected
lines. A **grep guard** inside the test (assert the captured stdout contains the
plaintext exactly once and only on the `reveal` line) makes the redaction property
a hard assertion, not just an eyeball match.

5. Add a **compile-fail** golden proving `==` on `Secret` is rejected. Create
   `tests/golden/secret_eq_rejected/Main.sky` with `bad = Secret.fromString "a" ==
   Secret.fromString "a"` and a test that asserts `skyc` fails with SKY-T0014
   (mirror any existing negative/`check`-fails golden in the suite). Run it and
   confirm the expected diagnostic.

6. Commit: `test(e2e): Secret round-trip + redaction + == rejection goldens`.

---

## Task 11 — (Additive hardening) zeroize-on-drop

**Files:** edit `runtime/Cargo.toml`; edit `runtime/src/sky_runtime/secret.rs`.

**Rationale (security principle, additive):** Tasks 1–10 close *accidental
disclosure via formatting/comparison*. Zeroize closes a distinct threat —
*memory remanence*: a dropped `Secret`'s bytes lingering in freed heap for a core
dump / cold-boot scavenge. This is defense-in-depth, so it is a separate task and
can be deferred if the extra dependency is unwanted; the core guarantee does not
depend on it.

**Steps**

1. Add the dependency to `runtime/Cargo.toml [dependencies]` (non-optional, tiny,
   no-std-friendly): `zeroize = { version = "1", features = ["derive"] }`.

2. In `secret.rs`, zeroize the inner string on drop. Keep the manual `Debug` /
   `SkyStringify` / no-`Display` / no-`PartialEq` guarantees; add:

```rust
use zeroize::Zeroize;

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
```

   Note `secret_reveal` moves the `String` out (`s.0`) before drop, so a revealed
   secret is the caller's responsibility — document this on `reveal`. `Clone`
   remains safe (each clone owns its buffer and zeroizes independently).

3. Add a test that a revealed value is intact (zeroize must not corrupt the moved
   string) and rerun the suite:

```bash
cargo test -p sky_runtime secret::
cargo clippy -p sky_runtime --all-targets -- -D warnings
```

4. Commit: `feat(runtime): zeroize Secret on drop (defense-in-depth)`.

---

## Final verification (run before declaring done)

```bash
# Whole-workspace type/build + unit tests
cargo build --workspace
cargo test -p sky_runtime -p sky_kernels -p sky_types -p sky_canon -p sky_lower -p sky_ir -p sky_backend_rust
cargo test -p skyc stdlib
# E2E goldens (shared target)
SKY_E2E=1 cargo test -p skyc golden_secret
# Lints
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green everywhere; the canon↔registry tripwire green; the redaction grep
guard green; the SKY-T0014-on-`Secret` negative test green.

## Docs to update in the same change (Template sync rule)

- `docs/architecture/divergence-policy.md` — add the `Secret` sealed-newtype
  divergence next to `Bytes`.
- `docs/stdlib.md` — add the `Sky.Core.Secret` surface (`fromString` / `reveal` /
  `redacted` / `constantTimeEq`) with the "no `==`, use `constantTimeEq`;
  `reveal` is the only un-parse" note.
- When `Std.Auth` ports (out of scope here), re-type `signToken` / `verifyToken`
  to take `Secret` and update `docs/skyauth/overview.md`.
