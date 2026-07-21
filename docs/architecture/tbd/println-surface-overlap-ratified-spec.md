# Ipê `println` surface overlap — Silent-Effect Elimination: Ratified Spec

Status: RATIFIED (read-only security-soundness-guardian ruling). Scope: remove the silent-effect `Prelude.println`, replace with an effect-honest `Io.println : String -> Task ()`, add a dev-only pass-through `Debug.println : a -> a`, and close the silent-auto-force hole that made the current `println` print as a side effect of `let _ = …`. All file:line anchors verified against the branch HEAD at filing time (`src/compiler/lower/src/lower.rs`); re-confirm line numbers before editing.

> Filed by request as a guardian ruling for later review. Implementation is NOT started — this document is the design of record for the `Debug`/`Prelude`/`Io` `println` overlap when the work is picked up. Code blocks below are illustrative excerpts of existing source / proposed shapes, not runnable commands.

## 0. Problem statement

Today `Prelude.println` is a pure-typed (`String -> ()` / `a -> a`-ish) value registered directly in the canon environment. Because it is *not* `Task`-typed, one might assume it cannot run as an effect. That assumption is **false**. The lowerer's F1 auto-force arm (`lower_let_inner`, `PAnything`) keys **only** on the HM `Task` type via `is_task_typed`; it ignores the kernel effect column and the capability axis entirely. The result: any `let _ = <task>` sequences and *runs* the effect. And separately, `println`'s current registration means it *already prints* today. The effect column and the Capability are, respectively, a **codegen-dispatch** axis and a **sandbox-report** axis — **neither is a use-site effect gate.** The only mechanical, no-silent-effect fix is a **fail-closed diagnostic at the auto-force arm itself**, not at the class/capability layer.

---

## 1. Ruling A — Remove `Prelude.println`

### 1.1 Exact site to delete

- **File:** `src/compiler/canon/src/env.rs`
- **Site:** the `println` registration at **`env.rs:384`** (the Prelude-scope binding of `println`). Delete this entry outright. `println` must no longer be resolvable as an unqualified Prelude value.

### 1.2 Downstream impact

- Every unqualified `println x` in source becomes an **unresolved-name** error (IPE-N000x) after deletion — this is intended. Users migrate to `Io.println` (Ruling B) or `Debug.println` (Ruling C).
- No kernel table entry is orphaned by the deletion alone: the *value* binding is what is removed. The runtime `log_println` fn (see §2.4) survives and is **reused** by the new `IoPrintln` kernel.
- Any golden or example that relies on bare `println` must be re-blessed to the new qualified spelling (see §6).

### 1.3 Verification

- After deletion, `rg -n "\bprintln\b" src/compiler/canon/src/env.rs` shows **no Prelude-scope binding** — only the new `Io`/`Debug` qualifier registrations from Rulings B/C.
- Compile a fixture `let _ = println "x"` → must now be a resolve error, **not** a silent print.

---

## 2. Ruling B — `Io.println : String -> Task ()` (effect-honest)

### 2.1 New kernel spec

| Field | Value |
|---|---|
| Kernel name | `IoPrintln` |
| Module (qualifier) | `Io` |
| Member func | `println` |
| Arity | `1` |
| Type scheme | `String -> Task ()` |
| Effect column | (codegen-dispatch axis) — effect-bearing, dispatches through the Task runtime; **NOT** a use-site gate |
| Capability | `None` (report axis only; see note) |
| Runtime fn | **reuse** existing runtime `log_println` (String → Task ()) |

Note on Capability = `None`: capability is the *sandbox-report* axis; a `None` capability does **not** stop the effect from running and is not a use-site permission. It is orthogonal to the auto-force fix. Do not attempt to make `println` "safe" by tuning the capability column — that axis has no bearing on whether `let _ =` runs it.

### 2.2 Reuse runtime `log_println` — yes

The runtime already provides `log_println` (the fn the removed `Prelude.println` bound to). `IoPrintln` reuses it verbatim. No new runtime fn is written for Ruling B. This keeps the runtime surface unchanged; only the *front-end typing* changes (pure → `Task ()`).

### 2.3 ALL anti-drift sites to touch (8-site kernel contract)

Per the project's kernel-registration anti-drift contract, `IoPrintln` must be wired at **every** site or the tables desync:

1. **`KernelFn` enum** — add the `IoPrintln` variant (kernel-registry definition site).
2. **constrain / typing** — register the scheme `String -> Task ()` (the scheme table; `FIRST_SCHEMED` set if it is a genuine new scheme).
3. **canon `QUALIFIERS` / `STDLIB_MODULE_QUALIFIERS`** — register `Io.println` so the member resolves under the `Io` qualifier (both the qualifier-alias path and the stdlib-module-qualifier path — the memory-recorded anti-drift asymmetry means **both** must be checked).
4. **lower callee dispatch** — map `("Io", "println") => Ok(Callee::Kernel(KernelFn::IoPrintln))` in the qualified-member match (same region as `("Task","andThen")` at `lower.rs:14924`).
5. **lower arity table** — arity `1`.
6. **pretty-printer** — kernel name rendering.
7. **naming / runtime binding** — bind `IoPrintln` to runtime `log_println`.
8. **golden** — add/adjust the golden proving `Io.println` emits and runs (see §6).

### 2.4 CRITICAL — full trace: why non-Pure / Capability does NOT stop the silent auto-force

This is the load-bearing part of the ruling. The claim "a non-`Task`/non-effect classification prevents `let _ =` from running the effect" is **false**, and here is the exact mechanical reason.

**The auto-force decision arm:**

- **File:** `src/compiler/lower/src/lower.rs`
- **Function:** `lower_let_inner` (starts `lower.rs:16535`)
- **Arm:** `canon::Pattern_::PAnything` at **`lower.rs:16562`**
- **Predicate it keys on:** `self.is_task_typed(b.body.span)` at **`lower.rs:16563`**

**What `is_task_typed` inspects (`lower.rs:16103`–16109), illustrative excerpt:**

```rust
fn is_task_typed(&self, span: Span) -> bool {
    matches!(
        self.region_ty(span),
        Some(Ty::Con { name, .. })
            if self.interner.resolve(*name).is_some_and(|n| n == "Task")
    )
}
```

It matches **purely on the solved HM type constructor being named `Task`.** It reads *nothing* about the kernel effect column and *nothing* about the capability. Those two axes are:

- **Effect column** = *codegen-dispatch* axis (chooses how the runtime emits/awaits an effect). It is a backend emission detail, not a front-end use-site gate.
- **Capability** = *sandbox-report* axis (what the sandbox reports/permits at run boundary). It is a reporting axis, not a use-site gate.

Neither is consulted at the `PAnything` arm. Therefore, the moment a discarded binding's *type* is `Task …`, the arm fires and **sequences the effect**:

**The two emission branches (`lower.rs:16564`–16574):**

- **Async context** (`self.fn_is_async.get()` true) → `Expr::TaskSeq { effect, rest }` → lowers to `task_and_then(effect, |_| rest)` — the whole chain stays a `Task` value and the effect is awaited in-chain.
- **Sync context** → `Expr::TaskSeqSync { effect, rest }` → lowers to `{ let _ = task_run(effect); rest }` — this **blocks on and RUNS** the task, discards the result, and continues.

So `let _ = <task>` **RUNS + discards**. Consequently, once `println` is `Task`-typed (Ruling B), `let _ = Io.println "x"` will *silently run* — the effect fires with the value dropped. Even under the *old* pure typing, the effect-column/capability being `None`/Pure did **not** stop this arm, because the arm never looks at them; it is the *type* that trips it. The old `Prelude.println` printed on `let _ =` precisely because of this arm, independent of its capability.

**Why the fix must live AT the arm (make-invalid-states-unrepresentable):** the only place where "a discarded effect is about to run silently" is *representable* is this `PAnything` + `is_task_typed` branch. Fixing anywhere else (class, capability, effect column) leaves the arm free to fire. The parse-don't-validate / invalid-states-unrepresentable move is: **at the point the compiler is about to synthesize a run-and-discard of a `Task`-typed discarded binding, refuse.**

### 2.5 The precise fail-closed diagnostic fix

- **Where:** inside the `PAnything` arm at `lower.rs:16562`, in the `if self.is_task_typed(b.body.span)` branch (currently `lower.rs:16563`–16574).
- **What:** replace the *silent* `TaskSeq` / `TaskSeqSync` synthesis with a **fail-closed compile diagnostic**: `let _ = <Task-typed expr>` is a **compile error** (new IPE-code, e.g. `IPE-L0130` "discarded Task-typed binding: a `Task` result may not be silently discarded; bind it and sequence it explicitly (`Task.andThen` / `do`), or if the effect is intended, name it").
- **Why it is make-invalid-states-unrepresentable:** the pre-fix code *represents* "run this effect and throw the result away" as an ordinary emission path. Post-fix, that state cannot be constructed — the lowerer emits an error instead of code, so no build can silently run-and-discard a `Task`. The type carries the proof: a `Task` is an unrun description of an effect; discarding it via `_` is now a type-level-detectable defect, refused at the exact synthesis point.

**Interaction with the removed silent path:** note the arm has a *dual* purpose (see Ruling 4). The non-Task `else` branch (`lower.rs:16575`–16581, plain `Destructure(Wildcard, …)`) is **unchanged** — dropping a *pure* value via `_` remains legal. Only the `Task`-typed sub-branch flips from "silently run" to "compile error."

---

## 3. Ruling C — `Debug.println : a -> a` (dev-only pass-through)

### 3.1 New kernel spec

| Field | Value |
|---|---|
| Kernel name | `DebugPrintln` |
| Module (qualifier) | `Debug` |
| Member func | `println` |
| Arity | `1` |
| Type scheme | `a -> a` (identity-shaped pass-through; prints a stringification of the argument as a side effect, returns the argument unchanged) |
| Effect column | dev-only trace effect (codegen-dispatch); **NOT** a use-site gate |
| Capability | `None` (report axis) |
| Runtime fn | prints via the same `log_println` sink, then returns the borrowed-then-returned argument |

`Debug.println` is a *new* kernel (distinct from `IoPrintln`). It is `a -> a` so it threads through expressions for debugging (`foo (Debug.println x)`), which is why it does **not** go through the `Task` auto-force arm — its result type is `a`, not `Task`, so `is_task_typed` is false and the `PAnything` arm's Task sub-branch never fires for it.

### 3.2 The two blocking soundness obligations

Both must be discharged or the emitted Rust fails to compile:

1. **`IpeStringify` obligation → prevents E0277.** Because the argument is generic `a` and the kernel must stringify it to print, the constrained variable must carry an `IpeStringify` bound. This is registered in **`constrain_var_kernel`** for `DebugPrintln` (the same place other kernel-level obligations are attached). Without it, the emitted `format!`/stringify call on a bare generic `a` is `E0277: the trait bound a: IpeStringify is not satisfied`. Fail-closed: if the caller's `a` is not `IpeStringify`, it is a **compile error at the obligation**, not a runtime surprise.

2. **Borrow-then-return-arg → prevents E0382.** The runtime fn must **borrow** the argument to stringify it and then **return the same argument by value** (identity pass-through). If it stringifies by *moving* the argument into the print sink and then tries to return it, that is `E0382: use of moved value`. The runtime signature must be shaped `fn debug_println<A: IpeStringify>(a: A) -> A { <print &a>; a }` — print through a *reference*, return the owned value untouched.

### 3.3 Dev-only / production-strip gate status — ASPIRATIONAL, file separately

- There is **no build-profile stripping today.** The claim "`Debug.println` is stripped in production builds" is **aspirational** and must **not** be asserted as implemented.
- Ship `Debug.println` as an always-present kernel now (it prints in all builds). File the production-strip gate (dev-profile-only emission; no-op or removed under a release profile) as a **separate follow-up**, not part of this ruling. Do not gate the merge of Rulings A/B/C on the strip gate.

---

## 4. Highest-risk item — the F1 auto-force arm's dual purpose

### 4.1 The conflict

The `PAnything` arm at `lower.rs:16562` serves **two** distinct correctness goals that pull in opposite directions:

- **Don't-silently-DROP** (the F1 rationale, `lower.rs:16548`–16561): a `Task`-typed value discarded via `_` should not be *dropped unrun* — historically the arm *ran* it so the effect wouldn't vanish.
- **Don't-silently-RUN** (this ruling): a `Task`-typed value discarded via `_` should not be *run silently* — the effect firing invisibly is exactly the `println` bug.

These are the same syntactic site (`let _ = <task>`) with contradictory "safe" answers: one says "run it so it's not dropped," the other says "don't run it silently."

### 4.2 Resolution: `let _ = <task>` ⇒ **compile error**

The reconciliation is to satisfy **neither** silent behavior and instead **refuse the construct**. `let _ = <Task-typed expr>` becomes a compile error (§2.5). This simultaneously honors *don't-drop* (the effect is not silently discarded — the program won't build) and *don't-run* (the effect is not silently run — no code is emitted). The user is forced to make intent explicit: bind-and-sequence (`Task.andThen` / do-notation) if the effect is wanted, or restructure if it is not. This is the only resolution that does not pick one silent behavior over the other.

### 4.3 REQUIRED pre-check before implementing

**The real do-block / `Task.andThen` desugaring MUST NOT flow through the `PAnything` arm.** If do-notation or monadic sequencing lowered a discarded statement through `lower_let_inner`'s `PAnything` branch, then turning that branch into a compile error would break *legitimate* sequencing — a catastrophic false-block.

**Verification performed at filing time (branch HEAD):**

- `Task.andThen` lowers via the **qualified-member callee dispatch**: `("Task", "andThen") => Ok(Callee::Kernel(KernelFn::TaskAndThen))` at **`lower.rs:14924`** (siblings `Task.andThenResult` at 14928, `Maybe.andThen` 14706, `Result.andThen` 14716, `JsonDec.andThen` 14849). This is a **kernel-application** path — a `Callee::Kernel` on an applied expression — entirely separate from `lower_let_inner`.
- The do-block / monadic-bind sequencing desugars to `Task.andThen (\_ -> rest) effect` (pipe/eta shapes noted at `lower.rs:774`, `855`, `3380`, `7405`), i.e. it becomes an **`andThen` application**, not a `let _ = …` binding. It therefore reaches the kernel dispatch at `lower.rs:14924`, **not** the `PAnything` arm at `lower.rs:16562`.

**Conclusion:** the two paths are disjoint. Making the `PAnything` Task sub-branch a compile error affects only *literal* `let _ = <task>` source bindings, **not** desugared monadic sequencing. This pre-check is a **hard gate**: before landing §2.5, re-confirm with a fixture that a `do`-block / `Task.andThen` chain that discards an intermediate `Task` result still compiles and runs (it must not hit IPE-L0130).

---

## 5. Ordered implementation checklist

1. **Pre-check (BLOCKING):** author a fixture exercising a `do`-block / `Task.andThen`-desugared sequence that discards an intermediate `Task`. Build it on current HEAD; confirm it lowers via `TaskAndThen` (`lower.rs:14924`) and **not** via `lower_let_inner` `PAnything` (`lower.rs:16562`). If it flows through `PAnything`, STOP — the resolution in §4.2 is unsafe and must be redesigned.
2. **Ruling A:** delete the `println` Prelude binding at `env.rs:384`. Rebuild; confirm bare `println` is now an unresolved-name error.
3. **Ruling B kernel `IoPrintln`:** wire all 8 anti-drift sites (§2.3): `KernelFn` variant, scheme `String -> Task ()` (+`FIRST_SCHEMED` if new), `QUALIFIERS`/`STDLIB_MODULE_QUALIFIERS` for `Io.println`, lower callee dispatch (near `lower.rs:14924`), arity `1`, pretty, naming→runtime `log_println`, golden.
4. **Fail-closed diagnostic (§2.5, the core fix):** in the `PAnything` arm (`lower.rs:16562`), replace the `if self.is_task_typed(...)` → `TaskSeq`/`TaskSeqSync` synthesis (`lower.rs:16563`–16574) with a fail-closed `Err(...)` carrying the new IPE-L0130 (discarded-Task) diagnostic. Leave the non-Task `else` (`lower.rs:16575`–16581) untouched.
5. **Re-run pre-check fixture:** confirm the `do`/`andThen` fixture still compiles+runs (no IPE-L0130). Confirm `let _ = Io.println "x"` now yields IPE-L0130 with a message steering to explicit sequencing.
6. **Ruling C kernel `DebugPrintln`:** add the `a -> a` kernel; attach the `IpeStringify` obligation in `constrain_var_kernel` (§3.2.1); write the runtime `fn debug_println<A: IpeStringify>(a: A) -> A { print(&a); a }` borrow-then-return (§3.2.2). Wire the same 8 anti-drift sites for `Debug.println`.
7. **Confirm `DebugPrintln` bypasses the arm:** a fixture `let _ = Debug.println x` must lower via the plain non-Task `else` branch (its type is `a`, not `Task`) and must **not** trip IPE-L0130.
8. **Follow-up (separate, do not block):** file the dev-only / production-strip gate for `Debug.println` (§3.3) — no build-profile stripping exists today.
9. **Workspace gate:** full test suite green, `clippy -D warnings` clean, E2E green.

---

## 6. Golden re-bless expectations

- **Bare-`println` goldens:** every golden/example using unqualified `println` must be re-spelled to `Io.println` (effectful, must now be sequenced — a bare discarded `Io.println` will be IPE-L0130) or `Debug.println` (pass-through). Re-bless accordingly.
- **New golden — `IoPrintln` emits+runs:** a golden proving `Io.println "…"` inside a proper `Task` sequence (`|> Task.andThen …` or do-block) emits the `log_println` call and prints at runtime. Assert the emitted Rust routes through the reused `log_println` sink.
- **New golden — IPE-L0130 fail-closed:** a fixture `let _ = Io.println "x"` (or any `let _ = <Task-typed>`) must produce IPE-L0130, **not** a silent run. Assert the diagnostic text and that **no** binary is emitted (ipe non-zero, no cargo build).
- **New golden — `do`/`andThen` NOT blocked:** the pre-check fixture (§5.1) is committed as a positive golden — a `Task.andThen`-desugared discard compiles (ipe 0, cargo 0, runs) and does **not** emit IPE-L0130. This is the regression guard for the §4.3 disjointness claim.
- **New golden — `DebugPrintln` pass-through:** `Debug.println x` threads its argument (`foo (Debug.println x)` returns `foo x`'s result) and compiles under an `IpeStringify` arg; a non-`IpeStringify` arg is a fail-closed E0277-equivalent obligation error at constrain time (assert the obligation, not a raw rustc error leak).
- **Trap (per project memory):** if goldens share a `CARGO_TARGET_DIR` and all emit the same app version, cargo fingerprint reuse can produce a **false pass** that masks a real cargo failure. Build the `IoPrintln`-run and `andThen`-not-blocked fixtures in **isolated per-fixture target dirs**, sequentially, reclaiming disk between them.

---

## Appendix — current-surface facts (established from bytes, this investigation)

- ONE kernel today: `KernelFn::LogPrintln` = `d("Log","println",1,Pure,"log_println")` in `src/compiler/kernels/src/lib.rs:1601`. Effect column `Pure`; scheme already `String -> Task ()` (`src/compiler/types/src/constrain.rs:4181` = `fun(string(), task_unit())`); runtime `log_println` returns `IpeTask<E,()>` (`src/runtime/rust/src/log.rs:187`).
- `Ipe.Log` is kernel-only (no `.ipe` source), exposing `println/info/debug/warn/error` via `env.rs install_prelude_qualifiers` (~line 568). Unqualified `println` builtin var at `env.rs:384` (`("println", log, "println")`) = the `Prelude.println` to remove.
- Module-alias table `STDLIB_MODULE_QUALIFIERS` (`env.rs` ~48) maps `Ipe.Prelude -> Basics`; NO `Ipe.Debug` entry. `Ipe.Io` (`src/stdlib/Ipe/Io.ipe`) has `readLine/writeStdout/writeStderr` (Task-typed) but NO `println`.
- NO `Ipe.Debug` module exists. `docs/architecture/tbd/elm-core-coverage.md` records `Debug.log` as an explicit GAP ("no `Debug.log`; `Ipe.Log` is a `Task`-tier logger, not the value-passthrough dev helper").
- ALL examples use `import Ipe.Log exposing (println)` then `println "…"` / `_ = println (…)`. None use the unqualified bare `println` builtin, and none use `Io.println`. So Ruling A's deletion of the bare builtin has limited example impact, but the `_ = println (…)` (i.e. `_ = Log.println …`) sites across examples DO exercise the auto-force arm and will hit IPE-L0130 once §2.5 lands — they must migrate to explicit sequencing or `Debug.println`.
