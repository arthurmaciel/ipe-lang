# Seal holes #104 (by-value arg-then-reuse) + #99 (refutable as-pattern double-move)

> **Status:** design (Doc/Design Lane). READ-ONLY study of the crates; no code
> written, no build run. Both holes are **seal-touching** (`skyc` exit-0 MUST
> imply `cargo` exit-0) → **Opus adversarial review before commit** (see §7).
>
> **Reference completed sibling:** #96 — the irrefutable-`Destructure`
> `let (a,b) as whole = v` double-move, closed by the clone-split in
> `emit_binding_stmts`. #99 is its *refutable* (match-arm) sibling; #104 is the
> straight-line-reuse cousin (a value consumed by a call, then read again).
>
> **Verdict up front (§8):** #104 and #99 are **two separate fixes** — different
> machinery, different files, independently testable. They share only the
> invariant that *every non-`Copy` value is `Clone`* (guaranteed upstream by the
> #87/#93 derive-seal), so a clone is always available as the escape hatch.

---

## 0. The hole class in one paragraph

The Rust runtime's value kernels take their `String` / `Vec` / struct arguments
**by value** (owned) — e.g. `string.rs`:
`pub fn starts_with(prefix: String, s: String) -> bool`. So emitting
`String.startsWith "#" s` as `starts_with("#".to_string(), s)` **moves** `s`. If
the same Sky binding `s` is read again anywhere later, `skyc` still prints
success but the emitted Rust fails `cargo` with **E0382 "use of moved value"**
(#104). The pattern side has the same shape: a match arm `Just ((a,b) as w)`
over an **owned** scrutinee lowers to `SkyMaybe::Just(w @ (a, b))`, which binds
the whole tuple into `w` **and** its fields into `a`/`b` — a partial move,
E0382, whenever the arm body uses both `w` and the parts (#99). Both are
"exit-0-then-cargo-fail" seal violations on a non-`Copy` payload.

---

## 1. Ground truth from the crates (file:line)

### 1.1 Runtime kernels are by-value, not by-ref

`runtime/src/sky_runtime/string.rs`:

```rust
pub fn starts_with(prefix: String, s: String) -> bool { … }
pub fn ends_with(suffix: String, s: String) -> bool { … }
pub fn contains(sub: String, s: String) -> bool { … }
pub fn append(a: String, b: String) -> String { … }
pub fn slice(start: i64, end: i64, s: String) -> String { … }
pub fn split(sep: String, s: String) -> Vec<String> { … }
```

Every one takes `String` **by value**. So a plain-variable argument is a
consuming move — borrow is **not** available at these positions without a
runtime-signature change (out of scope; would be a large parity churn). The
fix for #104 must therefore **clone** the argument when the binding is read
again, not borrow it.

### 1.2 `Expr::Var` is emitted bare — there is *no* move-safety pass today

`crates/sky_backend_rust/src/emit_expr.rs`:

- **`emit_expr.rs:2792`** — `Expr::Var(sym) => ctx.emit_ident(*sym)`. A variable
  read is the bare identifier: an owned **move** at every by-value position.
- **`emit_expr.rs:2535-2548`** — the general `Expr::Call` arg loop: each arg is
  `emit_expr_at`'d and spliced verbatim. A `Var` arg is a bare move.
- The only existing move-safety in the backend is a handful of **blanket
  per-kernel clones**: `DictGet` (`emit_expr.rs:2533` — `{dict}.clone()`), every
  `Db` kernel (`emit_expr.rs:544`+ — `{conn}.clone()`), `Http` req builders
  (`emit_expr.rs:994`, `1014` — `{req}.clone()`), record update
  (`emit_expr.rs:3416` — `({base}).clone()`), and the #96 destructure /
  list-element rebinds. There is **no general use-count / liveness clone pass**.
  `EmitCtx` (`lib.rs:127`) carries no clone-vars / copy-vars / use-count set.

This is why #104 is a genuine, still-open **general** hole: any well-typed
`let s = … in <consume s>; <read s>` over a non-`Copy` `s` that is *not* routed
through one of the blanket-clone kernels emits E0382.

### 1.3 Borrow vs owned positions (needed for minimality, §4)

Some positions already **borrow** and so never consume — cloning at them would
be pure waste:

- **`emit_expr.rs:2805`** — `BinOp::Append` → `format!("{}{}", l, r)`. `format!`
  borrows via `Display`; `s ++ x` does **not** move `s`.
- **`emit_expr.rs:2814`+** — comparison/`==`/`<`/… render as infix `(l == r)`.
  Rust's `PartialEq`/`PartialOrd` take `&self` via autoref, so `s == t` borrows.
- The `if` cond / `&&` / `||` operands are `bool` (Copy) anyway.

Owned-consume positions: call args (kernel + user fn), `Ctor` args, record
fields, list elements, tuple elements, `Cons` head/tail, and the by-value match
scrutinee.

### 1.4 The match-arm path (#99)

- **`emit_expr.rs:2807`** — `emit_match_scrutinee`: for the **default** case
  (not string, not list) the scrutinee is the **raw owned** expression. So
  `match m { … }` matches `m` **by value** and every binder moves out of it.
- **`emit_expr.rs:2834`** — `emit_arm_head`, shared by BOTH the value-context
  `emit_match` (`emit_expr.rs:2772`) and the tail-context match arm inside
  `emit_expr_tail` (`emit_expr.rs:3454-3461`). This is the single choke point
  the #99 fix must land at (mirrors how #96 landed in the shared
  `emit_binding_stmts`).
- **`emit_expr.rs:2857`** — `emit_ctor_arm_pat` renders each ctor sub-pattern via
  `render_pat`.
- **`emit_expr.rs:3131`** — `render_pat` `Pat::Alias(inner, name) → "{name} @
  {inner}"`. Over an owned scrutinee this is the double-move. (The doc-comment at
  `emit_expr.rs:3119-3130` already flags that `name @ inner` is sound **only**
  in a by-ref position and that by-value irrefutable sites are intercepted by
  `emit_binding_stmts` — the match-arm by-value site is precisely the case
  that is **not yet** intercepted.)

### 1.5 The #96 machinery this design leans on

- **`emit_expr.rs:3195`** `pat_contains_alias` — does a binder carry an `as`
  anywhere?
- **`emit_expr.rs:3237`** `emit_binding_stmts` / **`3244`** `push_binding_stmts`
  — the clone-split: `let whole = <v>; let (a,b) = whole.clone();`, recursing
  through tuples with fresh `__sky_bind_N` temps so nested aliases clone from
  their own temp.
- **`emit_expr.rs:2921`** `str_binder_rebinds` / **`2970`** `list_binder_rebinds`
  — the **prelude precedent for match arms**: `emit_arm_head` already prepends a
  per-arm `let … = …;` prelude (`emit_match:2787-2791` wraps the arm body in
  `{ prelude body }`). #99 reuses exactly this prelude slot.

### 1.6 The non-`Copy` predicate

`crates/sky_ir/src/ir.rs:408` `IrType`. **Copy** ⇔ one of `Int`, `Float`,
`Bool`, `Char`, `Unit`. Everything else (`Str`→`String`, `List`→`Vec`,
`Tuple`, `Record`, `Enum`, `Maybe`, `Result`, `Task`, `Fun`/closures, generic
`Generic`/type-var) is **non-`Copy`** but **is `Clone`** (derive-seal #87/#93).
A generic type-var is treated non-`Copy` (conservative: clone is always sound
since the bound carries `Clone`; a monomorphic-`Int` instance merely over-clones
a scalar, which is a bitwise copy).

**Caveat:** `Expr::Let` (`ir.rs:900`) carries **no** type for its bound name, so
the Copy predicate needs either (a) a region/solved-type lookup keyed at the
`Let`-value region — the backend already consults solved types elsewhere — or
(b) a small IR change adding `IrType` to `Let`/`Destructure`. See §4/§6.

---

## 2. #104 — the move-vs-clone-vs-borrow decision rule

### 2.1 Why a narrow "clone the kernel arg" fix is NOT total

A fix that only clones `String.*` kernel args would leave siblings open —
`let s = …; userFn s; otherFn s` (two by-value user calls), `Ctor s` then read
`s`, `[s]` then read `s`, `f (g s) (h s)` (two args from one binding). The seal
demands **totality**: a well-typed Sky program can *never* emit E0382. Only a
**general move-safety analysis over local variable reads** achieves that. #104's
real fix is that pass; it **subsumes** the kernel-arg repro.

### 2.2 The rule (the "exactly-one-bare-occurrence" invariant)

For each local binding `x` of **non-`Copy` type**, within its scope:

1. Enumerate every **read** site of `x` (`Expr::Var` reads only — *not* the
   `Pat::Var` binder sites, which are a different code path).
2. Classify each read's position as **borrow** (§1.3: operand of a comparison
   op / `++` / interpolation) or **owned-consume** (everything else).
3. **Borrow reads never consume** → emit bare (`x`); they impose no clone
   obligation and do not count against liveness.
4. Among **owned-consume** reads, in evaluation order, the **last** one is
   emitted **bare** (a move); **every earlier** owned-consume read is emitted
   `x.clone()`.

**Invariant:** at most one occurrence of `x` is emitted in a value-moving form,
and it is the last owned-consume read in evaluation order — so nothing that runs
after it can touch `x`. Every other read either borrows or clones. Therefore `x`
is consumed **at most once**, on a path where no later use exists ⇒ **no E0382,
provably total.**

### 2.3 Minimality

- A binding read **once** → bare move, **zero clones** (byte-identical to
  today).
- A binding read **N** times in owned positions → **N−1** clones, one move.
  This is the information-theoretic minimum when every position is
  owned-consume, and it is **strictly better than the reference** (which clones
  *all N*, including the last — see §5).
- Borrow-position reads are excluded from the count, so `s == "#"` then `f s`
  clones **zero** times (`s == "#"` borrows, `f s` is the last owned read →
  moves). This is the "borrow when the callee takes a reference" goal from the
  task.

### 2.4 Control-flow subtleties (the adversarial cases)

- **Branches are independent paths.** `if c { f x } else { g x }` — `x` may be
  moved in **each** arm independently (only one runs). "Clone all-but-last in
  evaluation order" stays **total** regardless (cloning is always safe); at
  worst it clones the textually-earlier arm's use that could have moved. A
  branch-aware refinement (treat the *last owned read on each mutually-exclusive
  path* as movable) removes that residual clone; recommended as a later polish,
  **not** required for the seal. The floor rule is already total here.
- **A value moved in one branch, live in another.** Covered by the same
  reasoning: each arm is its own path; the enclosing "after the `if`" read (if
  any) is the true last read and moves; in-arm reads clone. Total.
- **Loops are the one hard constraint.** A read of an **outer** binding **inside
  a `TailLoop` body** (`emit_expr_tail`, `emit_expr.rs:3434`+) executes on every
  iteration; moving it would use-after-move on iteration 2. **Rule: any read
  inside a loop body of a binding defined *outside* that loop is never treated
  as "last" → always `clone()` (unless Copy).** Bindings defined *inside* the
  loop body are fresh per iteration and follow the normal rule. The loop
  **parameters** themselves are reassigned via `continue` (already handled by
  the TCO lowerer) and are out of scope for this pass.

### 2.5 Adjacent hole spotted — closure capture (file a sibling task)

`emit_lambda` (`emit_expr.rs:4006`) emits `Box::new(move |…| { body })` — a
**`move`** closure that captures every free non-`Copy` local **by move with no
clone-capture prelude**. So `let s = …; onClick (\_ -> use s); read s` (or two
sibling closures each capturing `s`) is the **same** E0382 class, but its fix
lives at the **capture site** (wrap `{ let s = s.clone(); move |…| … }`, the
reference's `clonePreludeFor`, ExprEmitter.hs:755-770), **not** at the inner
read — a `move` closure has already consumed `s` before the body runs, so a
body-internal `s.clone()` cannot help. This is **not** covered by the §2.2
read-site pass and is a genuine separate hole. Per the no-deferral principle
(spotted = filed): **file it as its own task** (working name #104b —
"closure-capture-after-use / multi-capture clone-prelude"), adjacent to #104,
not folded into it. It is called out here so Lane A and the guardian see it; it
was outside the two holes this doc was scoped to design.

---

## 3. #99 — the refutable-match-arm as-pattern fix

### 3.1 Shape

`case m of Just ((a,b) as w) -> … a … b … w …` lowers to a match over an
**owned** `m` with arm pattern
`SkyMaybe::Just(w @ (a, b))`. `w` moves the whole tuple; `a`,`b` move its
fields ⇒ E0382 when the body uses both sides.

### 3.2 The fix — bind the whole by move, reconstruct the parts from a clone

Mirror #96, adapted to the refutable (arm) position. For an arm-pattern alias
`name @ inner`:

- **Pattern position:** render `name @ <skeleton(inner)>`, where `skeleton`
  keeps `inner`'s **refutability structure** (ctor/literal/slice shape) but
  replaces every **binder** with `_`. So `name` binds the whole **by move** and
  the skeleton **tests but binds nothing** (no second move). When `inner` is
  **irrefutable** (only vars/wildcards/nested tuples of those), the skeleton
  adds no test → render just `name`.
- **Prelude** (the existing `emit_arm_head` prelude slot, §1.5): reconstruct
  `inner`'s real bindings from a **clone of the whole**:
  - `inner` irrefutable → `let <inner> = name.clone();` — routed through
    **`emit_binding_stmts`** (`emit_expr.rs:3237`) so a **nested** alias inside
    `inner` (`w @ (x @ (a,b), c)`) reuses the #96 clone-split recursively.
  - `inner` refutable → `let <inner> = name.clone() else { unreachable!() };`
    (`let-else`). The `else` is provably dead because the outer skeleton already
    matched, so it is sound; it is **required** because a refutable plain `let`
    is E0005. (Drop the `else` when irrefutable to avoid the
    `irrefutable_let_patterns` deny-lint — exactly the reference's discipline in
    `patternToRustArg`, Pattern.hs:136-139.)

Because a match-scrutinee value is `Clone` (derive-seal), `name.clone()` always
resolves.

### 3.3 Minimality — clone only when both sides are live

The clone of the whole is needed **only when the body reads both** `name` **and
≥1 binder of `inner`**. A cheap body free-variable scan gives three cases:

| Body uses            | Emit                                                        | Clones |
|----------------------|-------------------------------------------------------------|--------|
| `name` only          | bind `name` (drop the destructure)                          | 0      |
| `inner` binders only | render `inner` normally (drop the alias whole)              | 0      |
| **both**             | `name @ skeleton` + prelude `let <inner> = name.clone()…`   | 1      |

One clone, only in the genuinely-both-live case — the theoretical minimum (both
the whole and a part are needed as owned values simultaneously).

### 3.4 Totality

The arm's pattern binds `name` by move; the skeleton binds nothing; the prelude
binds `inner`'s vars from an independent clone. No value is moved twice.
`let-else`'s `else` is unreachable by construction. Total over every alias
nesting (nested aliases recurse through `emit_binding_stmts`). Both the
value-context and tail-context match emitters share `emit_arm_head`, so the fix
covers both automatically.

### 3.5 Landing site

Extend **`emit_arm_head`** (`emit_expr.rs:2834`) — or a helper it calls — to
detect `pat_contains_alias` and, when present, split into
(skeleton-pattern, reconstruction-prelude), composing with the existing
`str_binder_rebinds` / `list_binder_rebinds` preludes (an aliased arm can also
be in string/list mode; append, don't replace). Add a `wildcard_binders(pat)`
pattern transformer and an `is_irrefutable(pat)` predicate (the latter mirrors
the reference's `patternIsIrrefutable`, Pattern.hs:159 — conservative: a
multi-variant ctor / literal / enum-not-known-single-variant ⇒ refutable ⇒ keep
the `else`).

---

## 4. Reference (`../sky`) — port vs divergence

### 4.1 #104 — the reference uses a cruder use-count≥2 blanket clone; **diverge**

`ExprEmitter.hs`: `varLocalRead` (781-787) clones a local read **iff** it is in
`ecCloneVars` — the set of locals used **≥ 2 times** (`collectVarLocalsMulti`,
comment at line 294). Consequence: a var used twice clones **both** reads,
**including the last** — one redundant clone per multiply-used binding. It is
simple and total but **over-clones**.

**Divergence (recorded in the ledger):** skyc uses **true last-use** analysis
(§2.2) — clone every owned read **except the last**, which moves. Strictly fewer
clones than the reference (N−1 vs N), and it additionally excludes borrow
positions (§1.3, §2.3) which the reference's count does not. This is a
*strictly-better* divergence (Rust move semantics let us move the last use; the
reference's coarser set does not exploit it), in line with the
sanctioned-divergence policy (diverge only where strictly better, reason
recorded).

### 4.2 #99 — the reference has a **latent bug**; skyc must be **correct**

`ExprEmitter.hs:4206` — `patternToMatchString`:
`Can.PAlias pat _ -> patternToMatchString _recMap pat`. The reference **drops
the alias name entirely** — it renders `((a,b) as w)` as just `(a, b)` and never
binds `w`. A body that uses `w` would fail Rust with **E0425 "cannot find value
`w`"** (an *unbound*-name bug, not a double-move). The reference sidesteps E0382
by discarding the whole binding — which is **wrong** whenever the whole is used.

**Divergence / improvement (recorded in the ledger):** skyc **correctly binds
the whole** via the clone-split of §3.2. This is a correctness fix over the
reference's latent bug — do **not** port the reference's drop-the-alias
behaviour. (The reference's `let-else` + irrefutability discipline in
`patternToRustArg` *is* worth porting — it is exactly §3.2's refutable branch.)

### 4.3 What to port

- The **`let-else` + `patternIsIrrefutable`** discipline (Pattern.hs:113-171) →
  §3.2/§3.5 refutable reconstruction.
- The **borrow-vs-clone instinct** for list/slice binders already exists in both
  backends (`list_binder_rebinds`, ExprEmitter.hs:4120-4130) — #99's prelude
  slots beside it.

---

## 5. Red→green fixtures

Each fixture is a minimal Sky program that is **`skyc` exit-0** and must become
**`cargo` exit-0** after the fix (currently `cargo` E0382 / E0425). Add as
compile-and-run fixtures in the backend's E2E harness; keep each ≤ 15 lines.

### 5.1 #104 fixtures

1. **The documented repro — startsWith-then-reuse.**
   ```elm
   classify s =
       if String.startsWith "#" s then "heading: " ++ s else s
   ```
   `starts_with(_, s)` moves `s`; `++ s` (or the bare `else s`) reads it again.
   Green: first read clones, last read (the `++`/`else`) is a borrow/last-move.

2. **Value used twice as owned args (one binding, two by-value calls).**
   ```elm
   dup s = String.append (String.toUpper s) (String.toLower s)
   ```
   `to_upper(s)` and `to_lower(s)` both consume; green = first clones, second
   moves.

3. **Owned arg then read in a `Ctor` / list / tuple.**
   ```elm
   wrap s = ( String.length s, [ s, s ] )    -- length(s) consumes; then [s, s]
   ```
   Exercises multiple owned positions (call arg, two list elements) — green =
   clone all but the last, one move.

4. **Moved in one branch, live after (branch adversarial).**
   ```elm
   pick c s = let u = String.toUpper s in if c then u else s
   ```
   `toUpper(s)` consumes in the `let`; `else s` reads after → last read moves,
   the `let` read clones.

5. **Loop-body reuse (the loop constraint).** A tail-recursive accumulator that
   references an **outer** non-`Copy` binding on every iteration:
   ```elm
   joinAll : String -> List String -> String
   joinAll sep xs =
       case xs of
           [] -> ""
           h :: t -> String.append (String.append h sep) (joinAll sep t)
   ```
   `sep` is read on every recursion → must clone inside the loop, never move.

### 5.2 #99 fixtures

6. **The documented repro — tuple as-pattern, both sides used.**
   ```elm
   f m =
       case m of
           Just ((a, b) as w) -> a ++ b ++ toString w
           Nothing -> ""
   ```
   Green: `Just(w @ (_, _))` + prelude `let (a, b) = w.clone();`.

7. **Alias whole used only (drop the destructure → 0 clones).**
   ```elm
   g m = case m of
       Just ((a, b) as w) -> toString w
       Nothing -> ""
   ```
   Green: `Just(w)` — no clone, `a`/`b` unused.

8. **Alias parts used only (drop the alias → 0 clones).**
   ```elm
   h m = case m of
       Just ((a, b) as w) -> a ++ b
       Nothing -> ""
   ```
   Green: `Just((a, b))` — no clone, `w` unused.

9. **Nested as-pattern (recurse through the #96 clone-split).**
   ```elm
   k m = case m of
       Just (((a, b) as inner, c) as w) -> a ++ b ++ c ++ toString inner ++ toString w
       Nothing -> ""
   ```
   Green: `Just(w @ (_, _))` + `let (inner, c) = w.clone();` routed through
   `emit_binding_stmts` → `let inner = …; let (a, b) = inner.clone();`.

10. **Refutable inner under an alias (the `let-else` branch).**
    ```elm
    p m = case m of
        Just ((Ok x) as w) -> x ++ toString w
        _ -> ""
    ```
    Green: `Just(w @ Ok(_))` + prelude
    `let (Ok(x)) = w.clone() else { unreachable!() };` (Result is
    multi-variant ⇒ refutable ⇒ keep the `else`).

11. **Tail-context arm (shared `emit_arm_head` coverage).** Put fixture 6's
    `case` in a tail-recursive function so it lowers through `emit_expr_tail`,
    proving the shared choke point covers both emitters.

### 5.3 Regression tests

- One backend unit test per fixture asserting the emitted Rust contains the
  expected clone/move shape (e.g. #104-2 emits exactly one `.clone()`; #99-7
  emits **zero** `.clone()` and no `@`).
- The full E2E build+run of each fixture (the true seal check: `cargo` exit-0).
- A **negative** assertion for the minimality claim: #104-1 with the branch read
  removed (`s` used once) emits **zero** clones (byte-identical to pre-fix).

---

## 6. Lane A task breakdown (bite-sized)

**#104 (move-safety read pass)** — land behind the existing blanket-clone
kernels (they stay; double-clone is harmless, prune later):

- **A1.** Add `is_copy(&IrType) -> bool` (Copy ⇔ Int/Float/Bool/Char/Unit) in
  the backend; unit-test it over every `IrType` variant.
- **A2.** Add the means to get a local read's type: prefer a region/solved-type
  lookup helper on `EmitCtx`; if that is not reachable at `Expr::Var`, add an
  `IrType` field to `Expr::Let`/`Destructure` in `sky_ir` (the lowerer has the
  type in hand) — **smallest-footprint decision to be confirmed with the
  guardian at review** (§7).
- **A3.** Per-function **read-index pass**: walk each `Func` body once, building,
  for every local binding, its ordered owned-consume read sites and marking the
  last. Carry the "this read must clone" decision into emission (a set on
  `EmitCtx`, or a pre-annotated side table keyed by read-site region).
- **A4.** Position classifier `is_borrow_position` for the comparison-op / `++` /
  interpolation operands (§1.3) so those reads are excluded from the count.
- **A5.** Loop constraint (§2.4): reads inside a `TailLoop` body of an
  outer-defined binding always clone.
- **A6.** Wire `Expr::Var` (`emit_expr.rs:2792`) to emit `x.clone()` when the
  read is marked; bare otherwise.
- **A7.** Fixtures 1-5 + regressions + the negative minimality assertion.
- **A8.** (Optional polish, separate commit) branch-aware last-use so
  mutually-exclusive-arm last reads move instead of clone.

**#99 (refutable as-pattern)** — self-contained in the pattern renderer:

- **B1.** `wildcard_binders(&Pat) -> Pat` (keep structure, binders → `_`) and
  `is_irrefutable(&Pat) -> bool` (conservative; mirror
  `patternIsIrrefutable`).
- **B2.** In `emit_arm_head` (`emit_expr.rs:2834`), when `pat_contains_alias`:
  build the skeleton pattern + the reconstruction prelude, reusing
  `emit_binding_stmts` for the irrefutable branch and a `let-else` for the
  refutable branch; **compose** with the existing string/list rebind preludes.
- **B3.** Body-liveness trim (§3.3): drop the destructure when only the whole is
  live; drop the alias when only the parts are live.
- **B4.** Fixtures 6-11 + regressions (assert clone-count per case; assert
  tail-context coverage).

**Filed sibling (do NOT bundle):**

- **#104b.** Closure-capture-after-use / multi-capture clone-prelude in
  `emit_lambda` (`emit_expr.rs:4006`) — §2.5. Own task, own fixtures.

---

## 7. Seal-touching → Opus review before commit

Both fixes change what emitted Rust the seal admits, so per the seal protocol
each lands only after **Opus adversarial review**. Focus points for that review:

- **#104 totality proof** (§2.2 invariant): confirm "exactly one bare
  owned-read, and it is the last in evaluation order" holds under every
  control-flow shape the lowerer can produce — especially the **loop** carve-out
  (§2.4) and the **evaluation-order vs AST-order** mapping (Rust evaluates call
  args left-to-right; the pass must use *evaluation* order, not raw child order,
  where they differ).
- **The A2 decision** (region-type lookup vs new IR field): whichever is chosen,
  confirm the Copy predicate reads the *binding's* type, never a use-site's
  coerced type, and that a generic type-var falls to non-`Copy` (clone-safe).
- **#99 refutability**: confirm the skeleton keeps *exactly* the refutability
  tests of `inner` (so no arm starts matching values it must not) and that the
  `let-else` `else` arm is provably unreachable in every refutable case.
- **Interaction with the blanket-clone kernels** and #96/list/str preludes:
  confirm no double-move and no *lost* clone at the seams (e.g. an aliased arm
  that is also in list mode).
- **Divergence ledger**: record §4.1 (last-use beats reference use-count) and
  §4.2 (skyc binds the alias whole; reference drops it — latent reference bug)
  in `docs/divergences-from-sky.md`.

---

## 8. Answer to the framing question — one pass or two?

**Two separate fixes.**

- **#104** is a **cross-expression move/liveness analysis** over `Expr::Var`
  **read** sites, threaded into `Expr::Var` emission. Its machinery (per-function
  read-index pass, Copy predicate, borrow classifier, loop carve-out) is entirely
  its own.
- **#99** is a **local pattern-emission** fix in `emit_arm_head` /
  `render_pat` — a refutable sibling of #96, using clone-the-whole-then-
  reconstruct. It needs no cross-expression liveness.

They touch different files/functions, are independently testable, and **compose**
without ordering constraints (#99 produces the arm's bindings `w`/`a`/`b`; #104
then governs how the arm **body** reuses them). Their only shared premise is the
derive-seal's *non-`Copy` ⇒ `Clone`* guarantee. Combining them into one pass
would couple a pattern-renderer change to a whole-body dataflow pass for no
benefit and a larger review surface. Ship #99 first (smaller, self-contained,
immediately unblocks the as-pattern corpus), then #104 (the broader seal pass),
then file **#104b** (closure capture) as the remaining adjacent hole.
