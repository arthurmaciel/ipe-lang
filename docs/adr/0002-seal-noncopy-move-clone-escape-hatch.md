Status: Accepted
Date: 2026-07-11

# 0002. Non-Copy move seal: last-use move + clone-everything-earlier

## Context

The Rust backend enforces a *seal*: `skyc` exit-0 MUST imply `cargo` exit-0 — a
well-typed Sky program can never emit Rust that fails to compile. Two holes in
that seal (#104, #99) both had the shape "`skyc` prints success but the emitted
Rust fails `cargo` with E0382 use-of-moved-value on a non-`Copy` payload":

- **#104 (by-value arg then reuse).** The runtime value kernels take their
  `String` / `Vec` / struct arguments **by value** (e.g.
  `pub fn starts_with(prefix: String, s: String) -> bool`). So emitting
  `String.startsWith "#" s` as `starts_with("#".to_string(), s)` **moves** `s`;
  any later read of `s` is E0382. Changing the runtime signatures to borrow is
  out of scope (large parity churn), so the fix must **clone** at the call site,
  not borrow.
- **#99 (refutable as-pattern double-move).** A match arm `Just ((a,b) as w)`
  over an owned scrutinee lowers to `SkyMaybe::Just(w @ (a, b))`, which binds the
  whole tuple into `w` **and** its fields into `a`/`b` — a partial move, E0382
  whenever the arm body uses both `w` and the parts.

Both are implemented (`emit_binding_stmts` machinery in
`crates/sky_backend_rust/src/emit_expr.rs`); #96 (the irrefutable
`let (a,b) as whole = v` sibling) landed first and this design leans on its
clone-split. The single shared enabling invariant is that *every non-`Copy`
value is `Clone`* (guaranteed upstream by the #87/#93 derive-seal), so a clone
is always available as the escape hatch. This ADR records the *why*; the code +
goldens are the source of truth for the *how*.

## Decision

**#104 and #99 are two separate fixes** — different machinery, different files,
independently testable. They share only the clone-escape-hatch invariant.

### #104 — the "exactly-one-bare-occurrence" invariant

A narrow "just clone `String.*` kernel args" fix is **not total** — it leaves
siblings open (`let s = …; userFn s; otherFn s`; `Ctor s` then read `s`; `[s]`
then read `s`; `f (g s) (h s)`). The seal demands totality, so #104's real fix
is a **general move-safety analysis over local variable reads**, which subsumes
the kernel-arg repro. For each local binding `x` of non-`Copy` type, within its
scope:

1. Enumerate every **read** site of `x` (`Expr::Var` reads only — not the
   `Pat::Var` binder sites).
2. Classify each read's position as **borrow** (operand of a comparison op /
   `++` / interpolation) or **owned-consume** (everything else).
3. **Borrow reads never consume** → emit bare (`x`); they impose no clone
   obligation and do not count against liveness.
4. Among **owned-consume** reads, in evaluation order, the **last** one is
   emitted **bare** (a move); **every earlier** owned-consume read is emitted
   `x.clone()`.

**Invariant:** at most one occurrence of `x` is emitted in a value-moving form,
and it is the last owned-consume read in evaluation order — so nothing that runs
after it can touch `x`. Every other read either borrows or clones. Therefore `x`
is consumed at most once, on a path where no later use exists ⇒ no E0382,
provably total.

Minimality: a binding read once → bare move, zero clones (byte-identical to
before). A binding read N times in owned positions → N−1 clones, one move.

### #99 — bind the whole by move, reconstruct the parts from a clone

For an arm-pattern alias `name @ inner` over an owned scrutinee, mirror #96
adapted to the refutable (arm) position: bind the whole by move and reconstruct
the parts from a clone (clone only when both the whole and the parts are live).

## Consequences

### Divergences from the reference (`../sky`), recorded in the ledger

* **#104 — the reference over-clones; we diverge (strictly better).** The
  reference (`ExprEmitter.hs` `varLocalRead`) clones a local read iff it is in
  `ecCloneVars`, the set of locals used ≥ 2 times — so a var used twice clones
  **both** reads, including the last. It is simple and total but over-clones.
  skyc uses **true last-use** analysis: clone every owned read except the last,
  which moves (N−1 vs N clones), and additionally excludes borrow positions the
  reference's count does not. Rust move semantics let us move the last use; the
  coarser reference set does not exploit it. Strictly-better divergence, reason
  recorded per the sanctioned-divergence policy.
* **#99 — the reference has a latent bug; skyc must be correct.** The reference
  (`ExprEmitter.hs:4206` `patternToMatchString`) **drops the alias name
  entirely** — it renders `((a,b) as w)` as just `(a, b)` and never binds `w`.
  A body that uses `w` fails Rust with E0425 "cannot find value `w`" (an
  *unbound*-name bug). The reference sidesteps E0382 by discarding the whole
  binding, which is wrong whenever the whole is used. skyc **correctly binds the
  whole** via the clone-split. Do **not** port the reference's drop-the-alias
  behaviour. (The reference's `let-else` + irrefutability discipline in
  `patternToRustArg` *is* worth porting — it is exactly the refutable
  reconstruction branch.)

### Invariant that must keep holding

Every non-`Copy` value stays `Clone` (the #87/#93 derive-seal). If that ever
regresses, the clone escape hatch these fixes rely on disappears and the seal
reopens. Any change to the runtime value-kernel signatures (by-value → by-ref)
would also change the borrow/owned classification and must revisit the
"exactly-one-bare-occurrence" analysis.
