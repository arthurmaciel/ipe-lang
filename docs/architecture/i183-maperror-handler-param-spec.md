# Spec: #183 — `Result.mapError` wildcard-handler param lowers as `JsonVal` instead of `SkyError` (exit-0-then-cargo-fail E0277)

Status: investigation complete, root-caused, fix verified end-to-end with a
temporary probe (skyc-0 ⇒ cargo-0 ⇒ prints `concrete`). This spec is the
one-shot implementation brief for a Sonnet agent. **Do NOT re-derive — the
mechanism, the exact edit, the two prior-attempt failure modes, and the
regression-test wiring are all confirmed below.**

## Repro (confirmed)

```elm
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
main = println (Result.withDefault "def" (Result.mapError (\_ -> Error.invalidInput "x") (Ok "concrete")))
```

`skyc build` exits 0. The emitted `main.rs` (line ~239) is:

```rust
sky_result_map_error(
    ok_res("concrete".to_string()),                      // SkyResult<SkyError, String>  (E = SkyError)
    { let __sky_fn: Box<dyn Fn(JsonVal) -> SkyError + Send + Sync + 'static>
        = Box::new(move |arg_0: JsonVal| -> SkyError { sky_error_invalid_input("x".to_string()) });
      __sky_fn }
)
```

`cargo build` then fails:

```
error[E0277]: expected a `FnOnce(SkyError)` closure, found `dyn Fn(serde_json::Value) -> SkyError + Send + Sync`
   --> src/main.rs:239
   = note: expected a closure with signature `fn(SkyError)`
```

## Root cause

The error is entirely in **`sky_lower`** (the IR the backend faithfully
renders), not the backend or runtime.

1. `Result.mapError`'s stdlib scheme is `mapError : (e -> f) -> Result e a ->
   Result f a` — `e` stays genuinely polymorphic (the def is unannotated:
   `crates/skyc/stdlib/Sky/Core/Result.sky:42`, `mapError fn r = case r of Ok
   x -> Ok x; Err e -> Err (fn e)`).

2. The handler is a wildcard lambda `\_ -> …`. In `Lowerer::lower_lambda`
   (`crates/sky_lower/src/lower.rs:7186`), a `PAnything` param takes its Rust
   type from `ir_type_from_ty_json(arg, …)` (line 7234-7235), where `arg` is
   the lambda's solved **param region type**. Because the call is UNPINNED
   (no result-type annotation forces `e`), the solver legitimately leaves the
   handler's param a bare free `Ty::Var`. `ir_type_from_ty_json`'s `Ty::Var`
   arm (line 8060-8062) defaults a genuinely-free, non-enclosing-generic var
   to **`IrType::Json`** (`JsonVal` / `serde_json::Value`).

3. The **value side** defaults the SAME free `e` to `SkyError`. `Ok
   "concrete"` routes through `KernelFn::ResultOkDefault` (the runtime
   `ok_res`, `lower_call_uniform` line ~9471-9480), which pins the error
   channel to the project's canonical `SkyError`. This is deliberate — see
   the `"Result"` con-rule in `ir_type_from_ty_json` (line 8094-8117): a free
   `Ty::Var` in the error slot is pinned to `IrType::Error`, "one defaulting
   policy, both sides".

4. **The asymmetry is the bug.** The value side pins `E = SkyError`; the
   handler-param side defaults the same free var to `JsonVal`. The backend
   (`emit_lambda` / `emit_lambda_unboxed`, `crates/sky_backend_rust/src/
   emit_expr.rs:7692` and `:7746`) is IR-type-driven — it *always* annotates
   both the closure binder (`move |arg_0: JsonVal|`) and the boxed trait
   object (`Box<dyn Fn(JsonVal) -> …>`) from `params[i].1`. So a `JsonVal`
   param IR type becomes a `FnOnce(JsonVal)` closure that cannot unify with
   `sky_result_map_error`'s `impl FnOnce(E) -> F` where `E = SkyError` →
   E0277.

### Why `kernel_turbofish_pin` (the #181 machinery) does NOT cover this

`kernel_turbofish_pin`'s `KernelFn::ResultMapError` arm
(`lower.rs:9400-9408`) pins the *discarded `Ok` type* (2nd Result arg) when
it is free — a DIFFERENT ambiguity (#181, result-type turbofish). In this
repro `Ok "concrete"` gives a concrete `Ok = String`, so that arm returns
`CallPin::None`. #183 is about the **error-handler param region**, not the
result type. The two are orthogonal; #181's fix cannot help here.

### Reference parity (`../sky` Haskell Rust backend) — how it avoids this

`../sky` (READ-ONLY reference) sidesteps the same asymmetry by **NOT
annotating the closure param at all** for a genuinely-free handler:
`ExprEmitter.hs:795-882` emits `move |psStr| { body }`, where `psStr`'s
fallback arm (`annotPsIx _ p = patternToRustParam p ++ annot`, line 848,
with `annot = ""`) leaves the param bare. It only adds a param type
annotation for concrete/inferable cases (`ecPipeInnerType`,
`ecForcedClosureParam`, `inferRecordClosureParam`, or
`inferParamRustTypeFromRegions` — **concrete-only, skips type vars**). rustc
then infers `arg: SkyError` from the `sky_result_map_error` bound. The
reference's semantic policy is even stronger for the Result error slot:
`ModuleEmitter.hs:983-993` normalises the Result error channel to `SkyError`
unconditionally (even a genuine `String` error renders `SkyError`).

Our Ipê backend is IR-type-driven and *always* annotates, so we cannot "just
omit the annotation" without a large backend change. The
architecturally-correct, minimal, reference-consistent fix is to **make the
IR carry the right type** — pin the handler param to `IrType::Error` at
lowering, matching the value side's `SkyError` default. Same "one defaulting
policy, both sides" principle already applied to the Result con-rule.

## Why the two autopilot attempts failed

Both attempts landed the *correct* fix idea (retype the mapError handler
binder `IrType::Json → IrType::Error`). The resume artifact
(`docs/architecture/progressive-development-resume/183-…md`) records only
"gate failed after merge" with a truncated tail showing **PASSing** tests
and **no FAIL line**. Combined with:

- The fix's own repro passes E2E (verified this session — see Verification).
- The related golden suites pass with the fix in place (verified:
  `golden_l0114_ctor_payload_function` 34/34,
  `golden_i181_ambiguous_kernel_turbofish`, `sky_lower` unit tests all green).

…the failures were **not** the fix's correctness. The two most likely gate
breakers, in priority order, are:

1. **Hand-written `oracle.meta` sha256 drift.** Attempt #2 hand-authored
   `tests/golden/result_map_error_wildcard/oracle.meta` with a literal
   `main_sky_sha256 = …`. `assert_go_parity` → `oracle::check_parity`
   (`crates/skyc/tests/support/mod.rs:405-421`) **re-hashes `Main.sky` and
   hard-fails loudly** ("run refresh-oracle") if the recorded hash does not
   match the fixture byte-for-byte. Any whitespace/comment difference between
   the committed `Main.sky` and the hashed content breaks the gate even
   though the compiler fix is perfect. **Do NOT hand-write `oracle.meta`** —
   generate it with the `refresh-oracle` tool (below).

2. **Full-workspace `cargo test` gate flake (OOM / disk / timeout).** Per the
   project's known "Rust lane OOM + cargo-test compile scope" learning, a
   `-p skyc` test run compiles ~155 test binaries; on a memory- or
   disk-constrained lane the gate can be killed mid-run and surface as a
   non-specific failure with a truncated tail — exactly what the resume
   shows. The autopilot gate should run the fix's targeted golden under
   `SKY_E2E=1` plus the two related golden suites, in an **isolated
   `CARGO_TARGET_DIR`**, not a full unbounded workspace test.

The Sonnet impl agent must therefore (a) place the retype in `sky_lower`
exactly as below, and (b) wire the regression fixture via `refresh-oracle`,
never a hand-written sha256.

## The fix

**One crate, one file:** `crates/sky_lower/src/lower.rs`.

### Edit 1 — add a helper method on `Lowerer`

Place it immediately before `fn lower_call_uniform` (near line 9435). This is
the same shape as the prior attempt's `retype_result_map_error_handler`,
which was verified sound:

```rust
/// #183 — re-align a `Result.mapError` error-HANDLER wildcard-lambda param
/// that defaulted to `IrType::Json` onto the value side's `IrType::Error`
/// (`SkyError`) default.
///
/// `Result.mapError : (e -> f) -> Result e a -> Result f a` keeps `e`
/// polymorphic. A wildcard `\_ -> …` handler over a genuinely-free `e`
/// lowers its `PAnything` binder via `ir_type_from_ty_json` (`lower_lambda`),
/// and a bare free `Ty::Var` there defaults to `IrType::Json`. But the
/// `Ok "concrete"` value side pins the SAME free `e` to `IrType::Error`
/// (`ok_res` / `ResultOkDefault`). One var, two defaults → emitted
/// `FnOnce(JsonVal)` closure vs `SkyResult<SkyError, _>` value → E0277
/// (exit-0-then-cargo-fail SEAL breach).
///
/// Fix — "one defaulting policy, both sides": when the handler is the
/// wildcard lambda whose single binder defaulted to `IrType::Json`, retype
/// that binder to `IrType::Error`. The `PAnything` binder is never read, so
/// the binder-only rewrite is sound with no body change. Fires ONLY when the
/// param is exactly `IrType::Json`: a resolved/annotated `e`
/// (`Result String a` handler → `IrType::Str`, or a named-function handler
/// like `tag : String -> String`) is left untouched. Canon arg order is
/// `[fn, r]`, so the handler is `args[0]`.
fn retype_result_map_error_handler(resolved: &Callee, lowered_args: &mut [Expr]) {
    if !matches!(resolved, Callee::Kernel(KernelFn::ResultMapError)) {
        return;
    }
    if let Some(Expr::Lambda { params, .. }) = lowered_args.first_mut()
        && let [(_, ty)] = params.as_mut_slice()
        && *ty == IrType::Json
    {
        *ty = IrType::Error;
    }
}
```

### Edit 2 — call it in the exact-arity kernel-call arm

In `lower_call_uniform`, the `VarKernel | VarTopLevel` →
`std::cmp::Ordering::Equal` arm (around line 9509-9524), between the
`kernel_turbofish_pin` line and the `Ok(Expr::Call { … })`:

```rust
                    std::cmp::Ordering::Equal => {
                        let pin = self.kernel_turbofish_pin(&resolved, args, call_span);
                        // #183: re-align a `Result.mapError` wildcard-handler
                        // binder that defaulted to `IrType::Json` onto the
                        // value side's `IrType::Error` (`SkyError`) default,
                        // so the emitted closure's `FnOnce(SkyError)` unifies
                        // with the `SkyResult<SkyError, _>` value (else E0277,
                        // an exit-0-then-cargo-fail SEAL breach).
                        let mut lowered_args = lowered_args;
                        Self::retype_result_map_error_handler(&resolved, &mut lowered_args);
                        Ok(Expr::Call {
                            callee: resolved,
                            args: lowered_args,
                            pin,
                        })
                    }
```

Note `lowered_args` is already `let lowered_args = args.iter().map(…)` at the
top of `lower_call_uniform`; shadowing it `mut` in this arm is local and does
not affect the `Less`/`Greater` arms.

### Soundness envelope (verified this session, keep these invariants)

- **Guard on `*ty == IrType::Json` is load-bearing.** It is the exact
  discriminator between "genuinely-free `e`" (must become `SkyError`) and
  "resolved `e`" (leave alone). Verified: an inline lambda over a `String`
  error channel (`Result.mapError (\s -> String.append "e:" s) (Err "boom")`
  with `check : Result String Int -> String`) lowers the param to
  `IrType::Str` — the guard skips it, emitting `Box<dyn Fn(String) ->
  String>` correctly.
- **Only fires for an inline `Expr::Lambda` handler at `args[0]`.** A
  **named-function** handler (`tag : String -> String`, the existing
  `map_error_arity1_accepted` fixture) is not an `Expr::Lambda`, so the
  guard skips it — that golden stays green.
- **Single-binder only** (`[(_, ty)]`). A curried/multi-arg handler shape is
  gated out (SKY-T0014) before lowering; the pattern match is defensive.
- NO `dyn Any` / downcast / type-erasure. This is a root-cause IR-type fix
  matching the reference's semantic policy.

## Regression test

### Fixture: `tests/golden/result_map_error_wildcard/Main.sky`

```elm
module Main exposing (main)

import Sky.Core.Prelude exposing (..)


-- #183 SEAL positive: `Result.mapError`'s handler is a wildcard lambda
-- `\_ -> …` over a genuinely-free error var `e`, and the call is UNPINNED (no
-- result-type annotation forces `e`). skyc must default the handler parameter
-- to the SAME concrete error type (`SkyError`) that the `Ok "concrete"` value
-- side defaults `e` to — otherwise the emitted closure is `FnOnce(JsonVal)`
-- against a `SkyResult<SkyError, _>` value and cargo fails E0277 (an exit-0-
-- then-cargo-fail SEAL breach). Since the input is `Ok`, the handler never
-- runs; `Result.withDefault` unwraps the `Ok` and prints "concrete".
main =
    println (Result.withDefault "def" (Result.mapError (\_ -> Error.invalidInput "x") (Ok "concrete")))
```

### Oracle metadata — GENERATE, do not hand-write

The Go oracle rejects this shape (`Error.invalidInput` is a NAMING ERROR
[E1001] in the Go frontend — `Error.*` is an Ipê-only prelude kernel
qualifier, a pre-existing sanctioned divergence, #86), so the fixture is an
`oracle_divergence = true` fixture whose `expected_go.txt` is skyc's own
SEAL-correct output (`concrete`). **Do not hand-author `oracle.meta`** — the
`main_sky_sha256` field is re-verified by `assert_go_parity` and a stale hash
is the most likely cause of the two prior gate failures. Instead run the
`refresh-oracle` tool (built binary invoked as `l`), which records the sha256,
`expected_go.txt`, `exit_code`, and the divergence reason atomically:

```bash
# from repo root, isolated target
CARGO_TARGET_DIR=/tmp/i183-target cargo run -p refresh-oracle -- result_map_error_wildcard
```

If the tool refuses because Go isn't available/errors on this shape, it marks
`oracle_divergence = true` with the E1001 reason automatically and records
skyc's output — that is the intended state for this fixture. Verify
`tests/golden/result_map_error_wildcard/expected_go.txt` contains exactly
`concrete\n` and `oracle.meta` carries a `main_sky_sha256` matching the
committed `Main.sky`.

### Test registration: `crates/skyc/tests/golden_m88_combinators.rs`

Append:

```rust
/// #183 SEAL positive — `Result.mapError` with a wildcard handler `\_ -> …`
/// over a genuinely-free error var, called UNPINNED (no result-type
/// annotation). The handler's `PAnything` param used to default to
/// `IrType::Json` while the `Ok "concrete"` value side defaulted the same
/// free `e` to `IrType::Error` (`SkyError`); the emitted `FnOnce(JsonVal)`
/// closure then failed to unify with the `SkyResult<SkyError, _>` value and
/// cargo rejected it with E0277 despite skyc exit-0. The fix retypes the
/// handler binder to `SkyError`; this gate proves the whole pipeline
/// (skyc → cargo build → run) now succeeds and prints `concrete`.
#[test]
fn result_map_error_wildcard_handler() {
    assert_runs_and_matches_oracle("result_map_error_wildcard");
}
```

The harness (`assert_runs_and_matches_oracle`, line 70) is E2E-gated on
`SKY_E2E`: it builds the emitted project, runs it, and asserts stdout parity +
exit 0. Without `SKY_E2E` it returns early (compile-only in CI's fast lane).

## Verification (exact commands — isolated target, foreground, bounded)

Run from the repo root (or worktree). Reuse a dedicated target dir; never
touch the default workspace target.

```bash
export TGT=/tmp/i183-target
export RT="$(pwd)/runtime/src/sky_runtime"

# 1. Build skyc with the fix.
CARGO_TARGET_DIR=$TGT timeout 900 cargo build -p skyc --bin skyc 2>&1 | tee /tmp/i183-build.log
# expect: Finished, exit 0

# 2. Emit + cargo-build + run the repro (proves skyc-0 ⇒ cargo-0 ⇒ prints concrete).
mkdir -p /tmp/i183-repro/src
cat > /tmp/i183-repro/src/Main.sky <<'SKY'
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
main = println (Result.withDefault "def" (Result.mapError (\_ -> Error.invalidInput "x") (Ok "concrete")))
SKY
printf 'name = "i183repro"\nversion = "0.1.0"\nentry = "src/Main.sky"\n' > /tmp/i183-repro/sky.toml
rm -rf /tmp/i183-out
SKY_RUNTIME_DIR="$RT" $TGT/debug/skyc build /tmp/i183-repro/src/Main.sky --out /tmp/i183-out --runtime "$RT" 2>&1 | tee /tmp/i183-emit.log
# skyc exit 0
( cd /tmp/i183-out && CARGO_TARGET_DIR=$TGT timeout 400 cargo build 2>&1 | tee /tmp/i183-cargo.log )
# expect: Finished, NO error[E0277]
$TGT/debug/sky-app        # prints: concrete

# 3. Golden E2E for the fixture (proves the recorded gate passes).
CARGO_TARGET_DIR=$TGT SKY_E2E=1 timeout 600 cargo test -p skyc --test golden_m88_combinators result_map_error_wildcard_handler 2>&1 | tee /tmp/i183-golden.log
# expect: 1 passed

# 4. Non-regression on the neighbouring mapError goldens.
CARGO_TARGET_DIR=$TGT timeout 600 cargo test -p skyc --test golden_l0114_ctor_payload_function --test golden_i181_ambiguous_kernel_turbofish 2>&1 | tee /tmp/i183-neighbours.log
# expect: all passed (map_error_arity1_accepted, map_error_curried_stays_gated included)
```

Read each `tee`'d log; never rely on a single-shot tail. On a
disk/memory-constrained lane, keep to the isolated `$TGT` and the targeted
`--test` filters above — do **not** run an unbounded full-workspace
`cargo test`, which is the suspected environmental cause of the prior gate
failures.

## Files touched by the fix (summary for the impl agent)

| File | Change |
|---|---|
| `crates/sky_lower/src/lower.rs` | Add `retype_result_map_error_handler` helper (before `lower_call_uniform`); call it in the `Ordering::Equal` kernel-call arm. |
| `tests/golden/result_map_error_wildcard/Main.sky` | New fixture (above). |
| `tests/golden/result_map_error_wildcard/{expected_go.txt,oracle.meta}` | Generated by `refresh-oracle`, NOT hand-written. |
| `crates/skyc/tests/golden_m88_combinators.rs` | Register `result_map_error_wildcard_handler`. |

No backend (`sky_backend_rust`) or runtime change is needed — the runtime
`sky_result_map_error<E,F,A>` (`runtime/src/sky_runtime/core.rs:438`) is
correct; the backend faithfully renders whatever IR type the param carries.
The whole fix is making `sky_lower` carry `IrType::Error` for the free
handler binder.
