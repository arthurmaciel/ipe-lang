# Example 38 — `++` on `List` blocked by `BinopClass::Append` design

**Status**: diagnosed, not fixed — architectural change required.

## Symptom

`examples/38-composite-ui-multibackend` fails at `skyc build` with
a type error (`SKY-T0001`) pointing at `src/State.sky`.  The
reported location (`nowMs // millisPerDay`) is **mislocated** — it
is a secondary downstream failure caused by the primary root cause
below.

## Root Cause

`State.sky` uses `++` to append two lists:

```sky
-- State.sky (simplified)
type alias CompletedTask = (Int, Bool)
completedTasks : List CompletedTask
completedTasks =
    oldTasks ++ [ (nowMs, True) ]   -- ++ on List (Int, Bool)
```

The Sky Rust compiler currently constrains `++` exclusively to
`String -> String -> String` via `BinopClass::Append` in
`crates/sky_types/src/constrain.rs`.  This matches the Elm origin:
`++` is a `Semigroup`-class operator in Elm, but the Rust port has
not yet implemented a generalised `Appendable` constraint — only the
`String` monomorphisation is present.

### Affected code

- `crates/sky_types/src/constrain.rs` — `BinopClass::Append` branch
  returns `fun(string(), fun(string(), string()))` unconditionally.
- `crates/sky_lower/src/lower.rs` — `BinopClass::Append` lowers to
  `string_append_`, a String-only runtime function.

### Why the error is mislocated

`pipeline_err` in `crates/skyc/src/lib.rs` wraps **all** infer
errors with the entry file's source span (`src/Main.sky`), even
when the actual constraint violation occurs in an imported module
(`src/State.sky`).  The reported span inside `State.sky` is then
displaced to the nearest region in the entry file that uses the
type, which happens to be the integer division `nowMs // millisPerDay`.

Two separate issues are therefore filed:

1. **Primary** — `++` on `List a` is not supported.
2. **Secondary** — error location is wrong when the violation is in
   an imported module.

## Correct Fix (not yet implemented)

### Fix 1: Appendable super-type

The `BinopClass::Append` constraint must be generalised to support
`List a`:

```
Appendable a ⊇ { String, List a }
++  :  a -> a -> a   where  Appendable a
```

Steps:
1. Add a `TyClass::Appendable` variant to the type-class enum (if
   one exists) or model it as a bounded type variable during HM
   inference.
2. In `constrain.rs`: change the `BinopClass::Append` arm to unify
   both operands with a fresh `Appendable` variable; the solver
   must satisfy it against `String` or `List a`.
3. In `lower.rs`: dispatch `BinopClass::Append` based on the solved
   type — `string_append_` for `String`, `list_append_` for `List a`.
4. Add `list_append_` (or reuse `List.append`) in the runtime.
5. Add regression test: `"a" ++ "b"` still works; `[1,2] ++ [3,4]`
   now works.

### Fix 2: Error location propagation

`pipeline_err` must carry the **source module path** from the
lowering error rather than pinning every error to the entry file.
The `SkyError` / `Diagnostic` types need to preserve the originating
file path so the rendered diagnostic points at the correct `.sky`
source location.

## Tracking

- Primary blocker: `BinopClass::Append` / `Appendable` generalisation
  — tracked as part of the `sky_types` constraint design work.
- Secondary blocker: error location propagation in `pipeline_err`
  (`crates/skyc/src/lib.rs`) — tracked separately.

## Impact

Example 38 (`38-composite-ui-multibackend`) will remain blocked
until Fix 1 lands.  No other currently-passing example is known to
use `++` on `List`, but any real-world Sky app that appends lists
with `++` will hit this gap.  `List.append` is the current
workaround (it is already a registered kernel and lowers correctly).
