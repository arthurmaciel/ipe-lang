# PubSub.publish `class = Tea` investigation

Scope: is `class = Tea` on `PubSubPublish` / `PubSubPublishNoEcho`
(`src/compiler/kernels/src/lib.rs:2129-2132`) a latent dispatch bug for the
Task-shaped, non-TEA-context (raw server handler / Cli job) call path CLAUDE.md
advertises?

## §0 Honesty ledger

- **CONFIRMED (read in source):** the type scheme, the emit arm, the runtime
  signatures, the emit-dispatch mechanism (`is_tea()` is a hardcoded match, not
  `decl().class`), the current unreachability of the `PubSub` qualifier, and the
  `tea` module-flag coupling.
- **SUSPECTED (not built):** the *concrete* downstream symptom once the
  qualifier is wired (spurious `pub mod tea` + `IpeCmd<M>`/`IpeSub<M>` aliases in
  a headless app). Reasoned from the flag's documented effect, not reproduced by
  an emit. Flagged as such below.

## Verdict

**`class = Tea` is NOT a codegen/dispatch bug for the server/cli Task path.** It
is a **misleading classification with one real, low-severity side effect** (a
spurious module-flag pull-in), plus a **latent inconsistency** against the
effect-boundary taxonomy. Severity: **low** (cosmetic + one harmless-but-untidy
module pull-in), currently **UNREACHABLE**, so not shippable-broken today.

The reason it is sound despite the "Tea" label: **`class` and emit dispatch are
decoupled.** Two independent facts both happen to be spelled "Tea" here, and
only the harmless one is wrong.

### What `class = Tea` actually does

`decl().class` is consumed in exactly one behavioural place:
`KernelFn::wasm_client_available` (`kernels/src/lib.rs:4911-4975`) — the wasm
target allowlist. For `KernelClass::Tea` it allows only
`CmdNone|CmdBatch|CmdPerform|SubNone`; `PubSubPublish` falls through to `false`
(no wasm denotation). That is correct — server-only effect, no browser
denotation. So the `class` field's *only* semantic consumer treats it fine.

**Emit dispatch does NOT read `decl().class`.** The top-level kernel dispatcher
(`emit_expr.rs:6041-6055`) routes via the hardcoded predicates `is_db()` →
`is_tea()` → `is_server()` → `is_ui()`, each an independent `matches!` list
(`kernels/src/lib.rs:3986`, `4009`, `4114`). `is_tea()` hardcodes
`PubSubPublish`/`PubSubPublishNoEcho` into its match — it is NOT derived from
`class`. So "class=Tea" and "routes through `emit_tea_call`" are two separate
assertions; the emit path is chosen by `is_tea()`, not by the class field.

### Why the `emit_tea_call` path is correct for a Task value

Routing through `emit_tea_call` does **not** imply emitting a `Cmd`. The
`PubSubPublish` arm (`emit_expr.rs:2151-2160`) emits:

```
pubsub_publish::<_, IpeError>(topic, payload)
```

which is a **plain Task expression**, not a `Cmd` constructor. Confirmed against
the runtime (`src/runtime/rust/src/live/pubsub.rs:150`):

```
pub fn pubsub_publish<T, E>(topic: String, payload: T) -> IpeTask<E, i64>
```

Contrast `cmd_publish` (`pubsub.rs:183`) → `IpeCmd<M>`. Same `class = Tea`, same
`emit_tea_call` function, but the two arms emit **different runtime types**:
`PubSub.*` → `IpeTask`, `Cmd.*` → `IpeCmd`. `emit_tea_call` is just "the file the
arm lives in," not a Cmd-only contract.

The type scheme agrees (`constrain.rs:4405`):

```
K::PubSubPublish => fun(string(), fun(var(0), task(int())))   -- String -> a -> Task Error Int
```

No `msg` type var, result is `Task Error Int` — **the type-checker treats it as
pure Task tier, never as `Cmd msg`.** A raw `Ipe.Http.Server` handler (returns
`Task Error Response`) or a Cli job (`Task.run`) can bind/sequence it exactly
like `Http.post` or any `Db.*` Task. The emitted `IpeTask<IpeError, i64>` needs
no update loop, no `msg` anchor, no dispatch context.

**Answer to INVESTIGATE-2:** reachable from a server handler, the class=Tea emit
path produces **correct code** (a Task expression), not wrong code and not an
ICE. It composes with `Task.andThen`/`Task.run` like any Task-tier kernel. This
is structurally the same shape as genuinely Task-tier kernels: `Http.post`,
`Db.*` are `class = Pure`/`Db` and emit bare Task expressions; `PubSub.publish`
emits a bare Task expression too — the only difference is which `emit_*` file
holds its arm, which is invisible at the call site.

### The one real defect: spurious `tea` module-flag pull-in

`lower.rs:5644` sets the per-program flag `self.tea |= k.is_tea()`. Because
`is_tea()` includes `PubSubPublish`, **any program calling `PubSub.publish` sets
`uses_tea = true`** (`lower.rs:5642-5644`, `7282`). That flag makes the backend
append `pub mod tea; pub use tea::*;` and synthesise `IpeCmd<M>` / `IpeSub<M>`
type aliases (`lower.rs:7279-7282`).

For a **headless `Ipe.Http.Server` or `Ipe.Cli`** app that uses `PubSub.publish`
but has no TEA loop and no `msg` type, this pulls the TEA module + `IpeCmd<M>` /
`IpeSub<M>` aliases into an app that structurally has no `Cmd`/`Sub`/`msg`.

- **SUSPECTED, not built:** whether the unused `IpeCmd<M>`/`IpeSub<M>` aliases
  are harmless dead code or trip an "unused"/unconstrained-generic path. Since
  they are `type` aliases (not values needing an inferred `M`), the most likely
  outcome is harmless dead code — an Efficiency (P4) / Readability (P6) blemish,
  not a SEAL breach. This should be confirmed with an actual emit once the
  qualifier is wired (see below), because line 10826-10830 warns that a floating
  `IpeCmd<_>` with "no update loop to anchor `msg`" is an uninferrable-type
  cargo-fail class — the aliases themselves don't instantiate `M`, so they are
  almost certainly clear of that, but it is the one thing to verify.

### Reachability gate (INVESTIGATE-4)

`PubSub.publish` is **UNREACHABLE from any user program today.** The `"PubSub"`
qualifier is deliberately absent from `env.qual_vars`
(`canon/src/env.rs:897-916` registers `Cmd`/`Sub` but not `PubSub`;
`canon/src/lib.rs:2217-2252` `known_unbacked_disjoint_from_qual_vars` is a
tripwire test asserting exactly this). Canonicalisation never mints a
`VarKernel` for `PubSub.*`, so nothing above (correct-Task or the spurious
`tea`-flag) is currently observable. The defect is **latent — it becomes live
the instant the qualifier is wired.** So the fix belongs *with* the
qualifier-wiring change, not after.

### Effect-boundary inconsistency (INVESTIGATE-3)

Real but purely taxonomic. CLAUDE.md lists `PubSub.publish` under Task/effects
and `Cmd.publish` under TEA; the kernel table collapses both to `class = Tea`.
Given `class`'s only behavioural consumer is the wasm allowlist (where Tea is
fine for both), the label is **inconsistent with the documented tier but not
behaviourally wrong** — except through the `is_tea()`→`tea`-flag coupling above,
which is the concrete cost of the mislabel.

## Recommendation

Two independent, cheap fixes; do them together with the qualifier-wiring:

1. **Reclass `PubSubPublish` / `PubSubPublishNoEcho` to `class = Pure`.** Its
   only-consumer semantics (wasm allowlist) are unaffected — `Pure` with
   qualifier `"PubSub"` is not in the wasm floor allowlist (`kernels/src/lib.rs:
   4934-4957` lists specific Pure qualifiers; `"PubSub"` isn't among them), so it
   stays wasm-denied, matching Tea's outcome. This aligns the class field with
   the CLAUDE.md effect tier (Task-tier effect, like `Http`/`File` which are also
   `class = Pure`). **This does NOT by itself fix the module pull-in** — see #2.

2. **Decouple the runtime-module pull-in from `is_tea()`.** The right structural
   fix (fundamental rule 3, fix-the-structure): remove `PubSubPublish`/
   `PubSubPublishNoEcho` from `is_tea()`, and instead have them declare
   `required_runtime_module() => Some(RuntimeModule::Live)` (they already need
   `live::pubsub` — their symbols live there). The `record()` fn already routes
   `required_runtime_module` → `self.live` independently of `is_tea()`
   (`lower.rs:5659-5662`). This pulls in the `live` module (where
   `pubsub_publish` is defined) **without** falsely setting `uses_tea` and
   without synthesising `IpeCmd`/`IpeSub` aliases in a headless app.

   Caveat: removing them from `is_tea()` means the emit dispatcher no longer
   routes them to `emit_tea_call`. Their emit arm must move to whichever
   predicate the dispatcher will match — cleanest is a dedicated non-TEA arm or
   folding into the standard N-arg / a small `is_pubsub()` predicate dispatched
   before `emit_tea_call`. **The emit arm content stays identical**
   (`pubsub_publish::<_, IpeError>(...)`); only its host predicate changes.

   This is the "separate non-TEA emit path" the brief asks about: yes, once the
   kernel is genuinely Task-tier it should not share the `is_tea()` gate, because
   that gate is load-bearing for the `uses_tea` module flag. The emit *code* need
   not differ (it already emits a Task, not a Cmd) — only the *routing predicate*
   should, so the `tea` flag stops being a false positive.

Minimal-change alternative (if #2 is deemed heavier than warranted): keep
`is_tea()` membership but special-case the `tea`-flag OR at `lower.rs:5644` to
exclude the two `PubSub` variants (they set `live` via
`required_runtime_module` regardless). This is a symptom-patch (rule 3 disfavours
it) — the `is_tea()` list would still misreport them to any future consumer —
but it closes the one observable cost. Prefer #2.

## Adjacent findings

- **`is_tea()` / `is_server()` / `is_ui()` are hardcoded matches, not
  `decl().class`.** This is the root structural smell: a kernel's `class` and its
  emit-dispatch predicate are two hand-maintained tables that can (and here do)
  disagree. `PubSubPublish` is `class = Tea` AND in `is_tea()`, but a Task-tier
  kernel is in neither's natural home. Any audit of "is this kernel classed
  right?" must check both. A stronger structural fix (beyond this finding's
  scope) is to derive the `is_*` predicates from `class` so they cannot drift —
  but note the current split is load-bearing for `HttpStreamChunks`
  (`class = Pure` per its own note yet listed in `is_tea()` for emit routing),
  so a naive unification would regress that; it needs its own design pass.

- **No other Task-shaped kernel is mis-homed the same way (spot-check).** The
  other `is_tea()` members are genuinely TEA-typed: `Cmd*`/`Sub*`/`TimeEvery`
  scheme to `Cmd`/`Sub` types; `SubSubscribeTopic`/`SubSubscribeWebSocket`/
  `HttpStreamChunks` scheme to `Sub msg`. Only `PubSubPublish`/
  `PubSubPublishNoEcho` scheme to a bare `Task` while sitting in `is_tea()` —
  they are the sole Task-in-TEA-clothing pair. `HttpStreamChunks` is
  `class = Pure` but in `is_tea()` (documented, for `sub_subscribe_stream`
  routing) — that one is Sub-typed, so its TEA membership is legitimate.

- **Qualifier-wiring dependency (the parallel Sonnet lane).** The lane mapping
  `PubSubPublish → RuntimeModule::Live` is complementary and correct — the symbol
  genuinely lives in `live::pubsub`, so it must force the `live` module. That
  lane's change is exactly the mechanism recommendation #2 leans on. **When the
  `"PubSub"` qualifier is finally added to `env.qual_vars`, both the reclass
  (#1) and the `is_tea()`-decoupling (#2) MUST land in the same change**, and the
  `known_unbacked_disjoint_from_qual_vars` tripwire (`canon/src/lib.rs:2228`)
  must be updated (it currently asserts unreachability; wiring the qualifier
  flips its premise). A real E2E emit of a headless `Server.listen` app that
  calls `PubSub.publish` should be added as the SEAL regression for this path
  (confirming both "Task composes in a handler" and "no spurious
  `IpeCmd`/`IpeSub` breakage").
