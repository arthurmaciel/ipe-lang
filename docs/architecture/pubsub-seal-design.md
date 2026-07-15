# `Std.PubSub` seal design — unblock `examples/36-composite-server` (backlog #215)

> **Status:** DESIGN ONLY. This document is an implementation plan for a Sonnet
> agent. It touches no compiler/runtime/stdlib/example code. Every file/function
> named below is verified against the current tree on `lane/std-pubsub-seal`.

## 1. The precise blocker

`examples/36-composite-server` (upstream reference, READ-ONLY at
`../sky/examples/36-composite-server`) uses exactly ONE `Std.PubSub` surface:

```elm
-- src/Routes/Todos.sky
import Std.PubSub as PubSub
publishTodoCreated todo =
    PubSub.publish "todos.created" (encodeTodo todo)   -- payload : JsonEnc.Value
        |> Task.onError (\e -> ... Task.succeed 0)
        |> Task.andThen (\_ -> Task.succeed (Server.json ...))
```

`Std.PubSub.publish : String -> any -> Task Error Int` (and its sibling
`publishNoEcho`). No other PubSub member is used, and `Sub.subscribeTopic`
(already wired) is not used here.

### Minimal repro (reproduced live this session, skyc exit 1)

`/tmp/pubrepro/src/Main.sky`:

```elm
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Sky.Core.Task as Task
import Sky.Core.Json.Encode as JsonEnc
import Std.PubSub as PubSub
import Std.Log exposing (println)

main =
    PubSub.publish "topic" (JsonEnc.string "hi")
        |> Task.andThen (\n -> Task.succeed (println (String.fromInt n)))
        |> Task.run
```

Built with `SKY_RUNTIME_DIR=<repo>/runtime/src/sky_runtime skyc build /tmp/pubrepro`:

```
skyc: error[SKY-L0108]: kernel function not available yet
 --> src/Main.sky:9:5
  |
9 |     PubSub.publish "topic" (JsonEnc.string "hi")
  |     ^^^^^^^^^^^^^^ this kernel function is not available yet [feature: kernels]
```

Exit code **1**.

### Why (the deeper surface Lane A missed)

The N0028 resolver gate is NOT what fires — `PubSub.publish` resolves fine. The
mechanism:

1. `Std.PubSub` is a **compiled-source stdlib module**
   (`crates/skyc/src/stdlib.rs::STD_PUBSUB`, dotted `"Std.PubSub"`). Its two
   bindings are point-free kernel aliases:
   `publish = Ffi.kernel "PubSub_publish"`,
   `publishNoEcho = Ffi.kernel "PubSub_publishNoEcho"`.
2. `crates/sky_canon/src/resolve.rs::detect_kernel_alias` splits
   `"PubSub_publish"` at the first `_` → `("PubSub", "publish")` and looks it up
   in `env.stdlib_index`. That index is built from `StdlibKernel::ALL`, which
   **does** contain `PubSubPublish`/`PubSubPublishNoEcho` (kernels decl'd as
   `d("PubSub","publish",2,Tea,"pubsub_publish")` — verified
   `crates/sky_kernels/src/lib.rs:1994` and in `ALL` at `:3061`). So the alias
   resolves to `VarHome::Kernel(PubSubPublish)` — the `"PubSub"`-qualifier-absent
   assumption in the old comments is **only true for a direct `PubSub.foo`
   qualified reference; it is false for the kernel-alias path**, which bypasses
   `qual_vars` entirely.
3. The user's `PubSub.publish` binds to that imported alias, producing a
   `VarKernel { id: Some(PubSubPublish) }` node.
4. Type-check: `constrain_var_kernel` → `stdlib_scheme(PubSubPublish)` returns
   **`None`** (the kernel sits in the `KNOWN_UNBACKED` bucket, explicitly
   `return None` at `crates/sky_types/src/constrain.rs:5563-5565`). The
   `kernel_scheme_or_unsupported(None, None, span)` helper then raises
   `Diagnostic::Lower { Feature::Kernels }` = **SKY-L0108**.

So PubSub is currently in a **fail-closed-but-incomplete** state, exactly as
documented in `crates/skyc/src/stdlib.rs:382-391` ("LOWERING-BLOCKED (#196): the
kernels are in the registry … but have NO lower/emit arm … fails closed with
SKY-L0108 … Unblocked once the TEA lower + emit arms are added"). The seal holds
(no exit-0-then-cargo-fail); the completeness gap is what blocks example 36.

### What already exists (do NOT re-add)

Verified present and correct on this branch:

- **Kernel enum + decl + ALL** — `PubSubPublish`/`PubSubPublishNoEcho` decl'd
  (`sky_kernels/src/lib.rs:643-645`, `:1994-1997`), in `ALL` (`:3061-3062`), in
  `is_tea()` (`:3658-3659`).
- **Arity table** — both in the arity-2 group (`sky_lower/src/lower.rs:11279-11281`).
- **`sky_ir::pretty`** — `PubSubPublish => "PubSub.publish"` (`pretty.rs:711-712`).
- **Naming** — `PubSubPublish => "pubsub_publish"`,
  `PubSubPublishNoEcho => "pubsub_publish_no_echo"`
  (`sky_backend_rust/src/naming.rs:786-787`).
- **Runtime** — `pubsub_publish<T, E>(topic: String, payload: T) -> SkyTask<E, i64>`
  and `pubsub_publish_no_echo<T, E>` fully implemented + unit-tested
  (`runtime/src/sky_runtime/live/pubsub.rs:150-179`). `T: Clone + Send + 'static`,
  `E: From<String> + Send + 'static`.

The gap is exactly three things: **a type scheme, an emit arm, and the bucket
re-classification + its anti-drift tests.** No new kernel, no new runtime code,
no `lower_callee` string arm, no `pretty`/`naming`/`arity` change.

## 2. The fix plan (concrete, unambiguous steps)

### Step A — Type scheme (`crates/sky_types/src/constrain.rs`)

The wired sibling `Cmd.publish` is the exact template:

```rust
// crates/sky_types/src/constrain.rs:4131
K::CmdPublish => fun(string(), fun(var(1), cmd(var(0)))),        // String -> a -> Cmd msg
K::CmdPublishNoEcho => fun(string(), fun(var(1), cmd(var(0)))),
```

PubSub differs only in the result: `Task Error Int` instead of `Cmd msg`, and it
has NO `msg` variable — only the payload. Add, in the `Cmd`/`Sub` scheme
neighbourhood (right after the `SubSubscribeTopic` arm, ~`:4141`), using the
existing `string()`, `fun()`, `task()`, `int()`, `var()` helpers:

```rust
// ── PubSub.publish / publishNoEcho (backlog #215) ──
// `PubSub.publish : String -> a -> Task Error Int`
// var(0) = payload (polymorphic, monomorphized by rustc — like the runtime T).
// Result is `Task Error Int` (subscriber count), NOT `Cmd msg` (no msg var).
K::PubSubPublish => fun(string(), fun(var(0), task(int()))),
K::PubSubPublishNoEcho => fun(string(), fun(var(0), task(int()))),
```

Notes for the implementer:
- `task(a)` builds `Task Error a` (the error slot is fixed to `Error`; confirm by
  reading the `task` helper near the top of the scheme table — it is used by e.g.
  `AuthRegister`/`File.*` schemes and always pins `Error`). If `task` takes the
  error explicitly in this file, mirror whatever `Cmd.perform`
  (`:4123`, `task(var(0))`) does.
- `var(0)` is a genuine named type variable → rustc monomorphizes it per call
  site. This is NOT the wildcard-`any` carrier path; the payload flows to the
  runtime's generic `T` and is never erased (PRINCIPLES §"concrete over
  generic": `any` here is a real type var, exactly like `Cmd.publish`'s payload
  `var(1)` and `Sub.subscribeTopic`'s `var(0)`).

**Remove** `K::PubSubPublish | K::PubSubPublishNoEcho` from the schemeless
`return None` arm at `:5563-5565`. That arm currently reads:

```rust
K::PubSubPublish
| K::PubSubPublishNoEcho
| K::LiveAppRouted => return None,
```

Leave `K::LiveAppRouted => return None,` (it stays REACHABLE_BUT_UNLOWERED). The
match must stay wildcard-free (F1 invariant) — since the two PubSub variants now
have real arms above, deleting them from this arm keeps exhaustiveness. Also trim
the two `PubSub.publish / publishNoEcho — KNOWN_UNBACKED` bullet comments in that
block (`:5545-5546`, `:5557-5560`) so the surviving prose is accurate.

### Step B — Bucket re-classification + anti-drift tests (same file)

Move both variants from `KNOWN_UNBACKED` to `FIRST_SCHEMED`:

1. `KNOWN_UNBACKED` (`:7612-7615`) currently `&[K::PubSubPublish,
   K::PubSubPublishNoEcho]`. After the move it becomes **empty** (`&[]`). Confirm
   the `known_unbacked_never_schemed` test (`:7621`) still compiles over an empty
   slice — it iterates `for &k in KNOWN_UNBACKED`, so an empty slice is a vacuous
   pass. The `REACHABLE_BUT_UNLOWERED` loop below it (over `LiveAppRouted`) is
   untouched. Update the `KNOWN_UNBACKED` doc-comment (`:7603-7611`) — it is now
   an empty bucket; state that explicitly (structural reason, no archaeology).
2. `FIRST_SCHEMED` (`:6895`) — add `K::PubSubPublish, K::PubSubPublishNoEcho` to
   the slice (any position; the disjointness gates key on set membership). Update
   its doc-comment (`:6889-6894`) which currently says PubSub is the sole
   remaining `Ty::Var` fallback exclusion — that line becomes false; replace it
   with a note that PubSub is now schemed (`Task Error Int`, backlog #215).

These two edits keep the four gate tests green **by construction**:
- `known_unbacked_never_schemed` — vacuous (empty bucket).
- `stdlib_scheme_total_over_reachable` — PubSub now HAS a scheme, so it is no
  longer an excluded reachable kernel; membership check passes.
- `first_schemed_have_schemes` (`:7707`-region) — both now return `Some(ty)`.
- `stdlib_scheme_some_iff_relocated_or_first_schemed` (`:7722`-region) — both are
  now in `FIRST_SCHEMED` AND return `Some`; both sides of the iff agree.

Read each gate test body and confirm before running — they are self-checking, so
a missed spot fails loudly at `cargo test -p sky_types`.

### Step C — Emit arm (`crates/sky_backend_rust/src/emit_expr.rs`)

Current arm (`:2104-2110`) hard-errors:

```rust
KernelFn::PubSubPublish | KernelFn::PubSubPublishNoEcho => Err(Diagnostic::CompilerBug { ... }),
```

The wired sibling `Cmd.publish` (`:2100`) is `Ok(None)` → the standard N-arg emit
path emits `pubsub_publish(topic, payload)` via the naming table. PubSub needs the
same routing, **plus** an explicit error-generic turbofish.

**The turbofish subtlety (load-bearing — get this exactly right).** The runtime
signature is `pubsub_publish<T, E>(topic, payload) -> SkyTask<E, i64>`:
- `T` (payload) is inferred from the second argument — fine.
- `E` (error) appears ONLY in the return `SkyTask<E, i64>`. Like the `Std.Csv`
  parse kernels (`emit_expr.rs:5928-5943`) and the network kernels
  (`http_get::<SkyError>`), `E` can be left unconstrained at some call sites
  (e.g. when the result feeds a context that does not pin the error type),
  yielding an `E0283` cargo failure — a seal violation.

Because turbofish is prefix-positional and `T` comes first, a bare
`pubsub_publish::<SkyError>(…)` would bind `T = SkyError` (WRONG). The correct
emission anchors `E` while inferring `T`:

```rust
pubsub_publish::<_, SkyError>(topic, payload)
```

Two implementation options — **choose Option 1** (minimal touch, no runtime
change):

- **Option 1 (recommended).** In `emit_tea_call`, replace the CompilerBug arm
  with a dedicated arm that emits the call directly with the `::<_, SkyError>`
  turbofish (do NOT return `Ok(None)`, because the standard path's turbofish slot
  only special-cases `CsvParse`/`CsvParseWithDelimiter` and cannot express the
  `<_, SkyError>` two-slot form):

  ```rust
  // ── PubSub.publish / publishNoEcho (backlog #215) ──
  // Runtime `pubsub_publish::<T, E>` — T (payload) infers from arg 1; E
  // (error) appears only in the SkyTask<E, i64> result, so anchor it to
  // SkyError with a `<_, SkyError>` turbofish (mirror of the CsvParse
  // `::<SkyError>` anchor; two slots because T precedes E).
  KernelFn::PubSubPublish | KernelFn::PubSubPublishNoEcho => {
      let topic_e = arg!(0, "topic")?;
      let payload_e = arg!(1, "payload")?;
      let topic_s = emit_expr_at(ctx, topic_e, indent, child, generics)?;
      let payload_s = emit_expr_at(ctx, payload_e, indent, child, generics)?;
      let name = crate::naming::kernel_name(k); // "pubsub_publish" / "pubsub_publish_no_echo"
      Ok(Some(format!("{name}::<_, SkyError>({topic_s}, {payload_s})")))
  }
  ```

  Confirm `crate::naming::kernel_name` is the in-crate accessor for the naming
  table (grep `fn kernel_name` in `naming.rs`; if the fn has a different name or
  visibility, use whatever `emit_server_call`/`callee_name` use to fetch the
  runtime symbol). If a fully-qualified `sky_runtime::…::pubsub_publish` path is
  required (check whether the emitted prelude `use`-imports `pubsub_publish` —
  see how `cmd_publish` is brought into scope in `project.rs`
  `tea_bindings()`/prelude), prepend the module path accordingly. **The payload
  and topic argument order matches the runtime (topic first, payload second) —
  no `parts.reverse()` needed.**

- **Option 2 (only if Option 1's `E`-inference proves flaky).** Reorder the
  runtime generics to `pubsub_publish<E, T>` in `live/pubsub.rs` (both fns) and
  emit `pubsub_publish::<SkyError>(…)` (prefix-anchors `E`, infers `T`). This
  touches the runtime + its 6 in-module unit tests
  (`pubsub_publish::<u8, String>` → `::<String, u8>` etc.). Heavier; avoid unless
  Option 1 fails the E2E build. Record the reason if taken.

Whichever option, the arm must stay inside the `match k { … }` so the trailing
`_ => Err(CompilerBug "is_tea() but no emit arm")` guard (`:2115`) keeps every
future TEA kernel fail-closed. Also update the `emit_tea_call` doc-comment
(`:2004-2009`) which lists `PubSubPublish`/`PubSubPublishNoEcho` under
"M6-reserved … guard fires" — they are now wired; move them to the wired list.

### Step D — `REGISTRY_ONLY_ALLOWLIST` stays (`crates/sky_lower/src/lower.rs`)

**No change to routing.** PubSub is reachable ONLY via the kernel-alias
`id = Some(PubSubPublish)` fast path (`lower_callee_resolve:12258-12260`); it has
no `lower_callee` legacy string arm and needs none. So it must **remain** in
`REGISTRY_ONLY_ALLOWLIST` (`:15048-15049`) — that list tells
`decl_equiv_legacy_match` to skip variants with no legacy arm. Removing it would
red that test. **Only update the doc block** (`:15028-15049`): the "EMITTABILITY
VERDICT … → Err(Diagnostic::CompilerBug) [NOT emittable]" lines (`:15037-15038`)
are now false — both are emittable via the dedicated arm from Step C. Rewrite that
verdict to state they emit `pubsub_publish::<_, SkyError>(…)` and remain
alias-only-reachable (hence still in the allowlist). Leave the `const` slice
value unchanged.

### Step E — `stdlib.rs` doc (`crates/skyc/src/stdlib.rs`)

Update the `STD_PUBSUB` doc-comment (`:382-391`): change "LOWERING-BLOCKED (#196)
… have NO lower/emit arm, so a member use fails closed with SKY-L0108" to a
RESOLVES-note mirroring the `STD_UI_EVENTS` entry (`:400-408`): PubSub now
resolves + emits (skyc-0 AND cargo-0) via the `Task Error Int` scheme + the
`pubsub_publish::<_, SkyError>` emit arm; cite `docs/divergences-from-sky.md`
§B-FfiKernelAliasSealed. Keep the "Not in `STDLIB_MODULE_QUALIFIERS`" line — the
disjointness invariant (`compiled_vs_kernel_qualifier_disjoint`) still holds
because `"PubSub"` is still absent from `QUALIFIERS`; the module is reachable
purely through the alias path, which is the intended design.

## 3. SEAL discipline — how each site fails closed

- **Scheme (Step A).** Before: `stdlib_scheme` returns `None` → SKY-L0108 at
  type-check (fail-closed, no cargo-fail). After: returns
  `Some(String -> a -> Task Error Int)`. The alias's *body* is typed via this HM
  scheme, so an arity/shape mismatch in the stdlib `.sky` annotation would be
  SKY-T0001 at skyc-time (the layered fail-closed already documented in
  §B-FfiKernelAliasSealed). No flexible `u32::MAX` var is introduced — the payload
  is a real `var(0)`, monomorphized concretely, so there is no
  "type-checks-but-cargo-fails" wildcard hole.
- **Emit (Step C).** Before: `Err(CompilerBug)` if reached. After: emits a
  concrete `pubsub_publish::<_, SkyError>(…)` call to a runtime fn that provably
  exists (verified). The `E` anchor removes the only inference hole (`E0283`),
  guaranteeing skyc-0 ⇒ cargo-0. The `_ => Err(CompilerBug)` guard still catches
  any future unwired TEA kernel.
- **Buckets (Step B).** The four disjointness/totality gates in `constrain.rs`
  mechanically prove `stdlib_scheme` is `Some` for EXACTLY `RELOCATED ∪
  FIRST_SCHEMED` and `None` elsewhere. Adding PubSub to `FIRST_SCHEMED` and
  emptying `KNOWN_UNBACKED` keeps that partition exact — any missed edit reds a
  gate test at `cargo test -p sky_types`, not at a downstream cargo build.

Result: skyc-0 ⇒ cargo-0 for every PubSub call site. Nothing about this surface
requires larger work — the runtime is complete, the scheme is a real HM type, the
payload is a genuine monomorphized type var. There is **no** un-sealable remainder.

## 4. Verification plan

Run under `CARGO_TARGET_DIR=/home/arthur/.cache/ipe/lane-b-target`, foreground,
`timeout`-wrapped. Order cheapest-first.

1. **Unit/gate tests (fast, no E2E):**
   ```
   timeout 900 cargo +nightly test -p sky_types --  \
       known_unbacked_never_schemed \
       stdlib_scheme_total_over_reachable \
       first_schemed
   timeout 900 cargo +nightly test -p sky_lower -- decl_equiv_legacy_match
   ```
   All green ⇒ the bucket partition + arity/legacy invariants hold.

2. **PubSub SEAL probe** — add to
   `crates/skyc/tests/golden_stdlib_module_seal.rs`, mirroring the `Std.Csv`
   entry (`:240-259`). Two tests + a `const … MAIN`:

   ```rust
   // ── #215: Std.PubSub ──
   // PubSub.publish : String -> any -> Task Error Int. No Live.app runs in the
   // probe, so publish resolves to Err(Unavailable) — the test asserts the
   // program BUILDS + RUNS (exit 0) via a fixed marker, not a subscriber count
   // (which is 0 / errors without a broker). Task.onError swallows the error.
   const PUBSUB_MAIN: &str = "module Main exposing (main)\n\
       import Sky.Core.Prelude exposing (..)\n\
       import Sky.Core.Task as Task\n\
       import Sky.Core.Json.Encode as JsonEnc\n\
       import Std.PubSub as PubSub\n\
       import Std.Log exposing (println)\n\n\
       main =\n\
       \x20   PubSub.publish \"t\" (JsonEnc.string \"hi\")\n\
       \x20       |> Task.onError (\\_ -> Task.succeed 0)\n\
       \x20       |> Task.andThen (\\_ -> Task.succeed (println \"PUBSUB_OK\"))\n\
       \x20       |> Task.run\n";

   #[test]
   fn pubsub_resolves_and_emits() { let _ = compile_module_probe("pubsub", PUBSUB_MAIN); }

   #[test]
   fn pubsub_builds_and_runs() { seal_module("pubsub", PUBSUB_MAIN, "PUBSUB_OK"); }
   ```

   Confirm the `Task.run` / `Task.andThen (\_ -> Task.succeed (println …))` shape
   type-checks and prints via the probe (the harness's `println` returns `()`);
   if the exact expression shape needs adjusting to satisfy the type-checker,
   match the `PURE_MAIN` entry's `let _ = … in println "MARKER"` idiom
   (`:165-176`) instead. The `_resolves_and_emits` half runs without `SKY_E2E`;
   `_builds_and_runs` is gated on `SKY_E2E=1`.

   ```
   timeout 300 cargo +nightly test -p skyc --test golden_stdlib_module_seal -- pubsub_resolves_and_emits
   SKY_E2E=1 SKY_ORACLE_SHARED_TARGET=/home/arthur/.cache/ipe/lane-b-target \
       timeout 1200 cargo +nightly test -p skyc --test golden_stdlib_module_seal -- pubsub_builds_and_runs
   ```

3. **The example-36 sweep (the proof).** Once the above pass, build the real
   example against this skyc + runtime:
   ```
   cp -r ../sky/examples/36-composite-server /tmp/ex36   # READ-ONLY source; work on a copy
   SKY_RUNTIME_DIR=<repo>/runtime/src/sky_runtime \
       timeout 300 <skyc> build /tmp/ex36 --out /tmp/ex36/sky-out --runtime <repo>/runtime/src/sky_runtime
   # then cargo build the emitted crate (SKY_E2E path) and, ideally, run its
   # CompositeServerTest boot probe.
   ```
   Success criterion: skyc exit 0 AND the emitted crate `cargo build`s. That is
   the #215 close condition. (Do NOT commit anything under `/tmp` or the example
   copy.)

4. **Runtime unchecked** — Option 1 touches no runtime code, so
   `cargo test -p sky-runtime-rust --features full` needs re-running only if
   Option 2 was taken.

## 5. Divergence ledger update (`docs/divergences-from-sky.md`)

In §B-FfiKernelAliasSealed (`:1483-1505`), move `Std.PubSub` OUT of the
"Lowering-blocked (kernel in the registry but no lower/emit arm, SKY-L0108 …)"
bullet (`:1495-1498`). Either delete the PubSub sub-bullet (leaving that bullet
for any genuinely-still-blocked module — currently PubSub is its only member, so
the whole "Lowering-blocked" bullet can be removed if empty) or add a short
"resolved" note: `Std.PubSub` now resolves + emits (skyc-0 AND cargo-0) via a
`String -> a -> Task Error Int` scheme and a `pubsub_publish::<_, SkyError>` emit
arm (backlog #215); the payload is a monomorphized type var (concrete-over-
generic), never erased. No NEW divergence is introduced — this closes a
completeness gap under the existing §B-FfiKernelAliasSealed sanctioned entry.

Also fix the now-stale factual claims wherever they survive in code comments
(they assert PubSub has "no Rust runtime fn" / is "unreachable" — both false):
`sky_types/src/constrain.rs` KNOWN_UNBACKED prose, `sky_lower/src/lower.rs`
REGISTRY_ONLY_ALLOWLIST "LOUD FINDING" block, `sky_backend_rust/src/emit_expr.rs`
`emit_tea_call` doc. These are covered by Steps A/B/C/D above; this bullet is the
reminder that the *prose* must match the new behaviour (DEVELOPMENT.md
"state the contract, not its history": no archaeology, just the current rule).

## 6. Summary for the implementer

Six files, all additive/reclassifying — no new kernel, no new runtime code:

| File | Change |
|---|---|
| `crates/sky_types/src/constrain.rs` | Add `PubSubPublish`/`PubSubPublishNoEcho` scheme (`String -> a -> Task Error Int`); remove from schemeless `return None` arm; move `KNOWN_UNBACKED`→`FIRST_SCHEMED` (KNOWN_UNBACKED becomes empty); refresh 3 doc-comments. |
| `crates/sky_backend_rust/src/emit_expr.rs` | Replace the `CompilerBug` arm in `emit_tea_call` with a dedicated arm emitting `pubsub_publish::<_, SkyError>(topic, payload)` (Option 1); refresh the fn doc-comment. |
| `crates/sky_lower/src/lower.rs` | NO routing change (stays alias-only via `id=Some`); refresh the `REGISTRY_ONLY_ALLOWLIST` "EMITTABILITY VERDICT" doc to say emittable. |
| `crates/skyc/src/stdlib.rs` | Refresh `STD_PUBSUB` doc: LOWERING-BLOCKED → RESOLVES. |
| `crates/skyc/tests/golden_stdlib_module_seal.rs` | Add `pubsub_resolves_and_emits` + `pubsub_builds_and_runs` (mirror `Std.Csv`). |
| `docs/divergences-from-sky.md` | Move `Std.PubSub` out of the Lowering-blocked bullet; note resolved. |

Verify in the order of §4. The #215 close proof is the example-36 build at exit 0.
```

Do not redesign — every named site is verified against the current tree; the only
open judgement call is Option 1 vs Option 2 in Step C, and Option 1 is the
default unless its `E`-inference fails the E2E build.
