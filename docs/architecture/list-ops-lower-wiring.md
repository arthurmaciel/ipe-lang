# List-ops lower/kernel wiring (task #68, before-sweep blocker)

Status: DESIGN SPEC (authoritative). Doc-only. No code in this file.
Principles order: security > correctness > soundness > efficiency > completeness > readability.
Rules: (1) parse, don't validate. (2) make invalid states unrepresentable.
Parity default: Go byte/semantic parity; divergence only when strictly better and recorded here.

Public-artifact note: this is the public Rust port of the "Sky" Go backend. The
kernel-anchoring of `List.*` mirrors the upstream design; nothing here disparages it.

---

## 0. Problem restatement (verified against HEAD)

Probe `List.append/take/concat/zip` emits `error[SKY-L0108]: kernel function not
available yet` at `List.zip`. Root cause, traced end-to-end:

1. `crates/sky_canon/src/env.rs` (~line 260) installs `List` as a **prelude
   qualifier** whose member array is
   `[map, filter, foldl, foldr, length, head, tail, take, drop, append, concat,
   concatMap, reverse, member, any, all, range, zip, isEmpty, cons]`. Every one
   resolves to `VarHome::Kernel(id, List, name)` (`resolve_qual_var`,
   `crates/sky_canon/src/resolve.rs:1175`). **`indexedMap` and `find` are NOT in
   this array** — they fail earlier with `NoSuchMember`.
2. `id` is `Some(k)` only when a `KernelFn::List*` variant with matching
   `module/name` exists in `crates/sky_kernels/src/lib.rs` `KernelFn::ALL`
   (`stdlib_index`). Only 10 List variants exist:
   `Map/Filter/Foldl/Foldr/Length/Head/Tail/Member/Range/Reverse`. For every
   other member `id = None`.
3. `crates/sky_lower/src/lower.rs:lower_callee` fast-paths `Some(sk)`; on `None`
   it string-matches — and only those same 10 arms exist. `take/drop/append/
   concat/concatMap/zip/cons/isEmpty/any/all` fall through to
   `(_, _) => Err(unsupported(..., Feature::Kernels))` = **SKY-L0108**
   (`lower.rs:4446`).

So the names are wired in **canon** (qualifier member list) but unwired in the
**kernel registry**, **lowerer**, and **constrain scheme**. The pure-Sky
`Sky.Core.List` bodies (`sky-out/.sky-stdlib/Sky/Core/List.sky`) are never on the
resolution path for a qualified `List.x` call — the qualifier install shadows them
with `VarHome::Kernel`.

---

## 1. Decision summary

**Verdict for all nine ops: KERNEL. Pure-Sky routing is REJECTED.**

### The rule that decides (not per-op taste)

> A `List.x` call resolves through the canon prelude-qualifier install to
> `VarHome::Kernel` **unconditionally** — it never reaches the compiled
> `Sky.Core.List` source. Therefore the only wiring that (a) makes the name
> callable, (b) yields a **fail-closed constrain scheme** (mandatory under the
> exit-0 seal — no `Ty::Var(u32::MAX)` fallback), and (c) reuses the already-proven
> kernel emission path, is the **kernel** path: `KernelFn` variant + runtime fn +
> lower arm + `stdlib_scheme` arm.
>
> Pure-Sky routing would require, simultaneously: (i) re-pointing every affected
> canon `List` member from `VarHome::Kernel` to `VarHome::TopLevel([Sky,Core,List])`;
> (ii) guaranteeing `Sky.Core.List` is compiled and linked as a dep in **every**
> build graph (it currently is not the resolution target); (iii) surviving the
> "cannot infer T2" cross-module HOF-inference hole for the function-carrying
> members. This is a strictly larger, higher-risk change with **zero** security,
> correctness, or soundness benefit over the kernel path. Rejected.

Consequence: this is the same shape as the 10 already-wired List kernels. Low risk,
data-driven emission, fail-closed by construction.

---

## 2. Per-op table

Legend: HOF = takes a function argument. "runtime fn" is the `d(...)` 5th field
(emission is data-driven from it — no per-op emit arm needed).

| op | verdict | rationale (one line) | arity | runtime fn | parity note |
|---|---|---|---|---|---|
| `append`    | KERNEL | non-HOF; canon kernel-anchored; needs new iterative runtime fn | 2 | `list_append` (NEW) | `append [1] [2] = [1,2]`; matches Go/Elm concatenation |
| `concat`    | KERNEL | non-HOF; flatten `List (List a)`; new runtime fn | 1 | `list_concat` (NEW) | `concat [[1,2],[3]] = [1,2,3]`; Elm `List.concat` |
| `take`      | KERNEL | non-HOF; new runtime fn; i64→usize guarded | 2 | `list_take` (NEW) | `take n` with `n>len` → whole list; `n<=0` → `[]` (Elm) |
| `drop`      | KERNEL | non-HOF; **runtime fn already exists** | 2 | `list_drop` (reuse) | `drop n<=0` → identity (verified `list.rs:42`); Elm |
| `zip`       | KERNEL | non-HOF; **runtime fn already exists** | 2 | `list_zip` (reuse) | unequal lengths → truncate to shorter (verified `Iterator::zip`, `list.rs:117`); Elm |
| `cons`      | KERNEL | non-HOF; new runtime fn (prepend) | 2 | `list_cons` (NEW) | `cons 0 [1,2] = [0,1,2]`; same as `::` operator |
| `isEmpty`   | KERNEL | non-HOF predicate; new trivial runtime fn | 1 | `list_is_empty` (NEW) | `isEmpty [] = True`; Elm |
| `concatMap` | KERNEL | **HOF** — kernel is the *only* option (T2 hole for pure-Sky); runtime fn exists | 2 | `list_concat_map` (reuse) | flat-map; `list.rs:114`; Elm |
| `indexedMap`| KERNEL | **HOF**; runtime fn exists; **also missing from canon member array** | 2 | `list_indexed_map` (reuse) | `indexedMap f [a,b] = [f 0 a, f 1 b]`; Elm |

Curried-scheme note: `arity` in `d(...)` is the applied-arg count; the constrain
`Ty` is fully curried (see §4). Precedent: `ListMember` is `d(...,2,...)` with a
curried `fun(var0, fun(list var0, bool))` scheme.

---

## 3. Inference-hole ("cannot infer T2") analysis — proof per op

The `Sky.Core.List` header documents the trap: Sky lambdas lower to `func(any) any`,
and a cross-module call whose recursive body consumes the lambda's `any`-typed
return surfaces `cannot infer T2` at the call site (Limitation #18 / Gap 4). This is
why `map/filter/foldl/foldr/find/any/all/concatMap/indexedMap/sortBy` cannot be
pure-Sky today.

Because **all nine ops are wired as kernels**, none are exposed to the pure-Sky T2
path — the constrain scheme (§4) supplies the polymorphic type directly and the
runtime fn is monomorphised by rustc per call site (exactly as the 10 existing List
kernels are). For completeness, the classification had we attempted pure-Sky:

- **non-HOF, would NOT trip T2** (blocked only by canon anchoring + linking):
  `append, concat, take, drop, zip, cons, isEmpty`. These take no function argument,
  so no `func(any) any` return flows into a recursive body. Pure-Sky would be *type*-
  safe but is still rejected per the §1 rule (linking + no benefit).
- **HOF, WOULD trip T2** (doubly blocked): `concatMap` (`(a -> List b)`),
  `indexedMap` (`(Int -> a -> b)`). Their pure-Sky bodies in `List.sky` call
  `append`/recurse over `fn`'s `any`-typed result — the exact `cannot infer T2`
  shape. Kernel is mandatory, not merely preferred.

No op is left unschemable: every one has a concrete polymorphic `Ty` in §4, so
`constrain_var_kernel` never falls to `Ty::Var(u32::MAX)`.

### Adjacent gaps discovered (same SKY-L0108 class) — FILE, do not silently skip

`any`, `all`, `find` are HOFs currently in the same broken state (`any`/`all` in the
canon member array but no `KernelFn`; `find` absent from the array). Runtime already
has `list_any`/`list_all` (`list.rs:126/129`); `find` needs a new `list_find`. Under
the no-deferral principle these are the **same family** and should be wired
identically in the same PR (kernel path). They are called out here so the fix is
tracked; committed scope of this spec remains the nine.

---

## 4. Constrain `stdlib_scheme` entries (fail-closed)

Add to `crates/sky_types/src/constrain.rs` `stdlib_scheme` (the `K::List*` block near
line 2196). Use the helper forms already in that scope: `fun`, `list`, `var`, `int()`,
`bool_ty()`, `tuple2`. These are FIRST_SCHEMED (new holes → first scheme); no legacy
`(Some("List"), Some(...))` counterpart is required, but adding matching legacy arms
(near line 2990) is harmless defense-in-depth and keeps the two tables visually
parallel. Fail-closed guarantee: a `KernelFn` variant lacking a `stdlib_scheme` arm
returns `None` → `constrain_var_kernel` raises `Diagnostic::Lower` (SKY error), never
exit-0-then-cargo-fail.

```rust
// append : List a -> List a -> List a
K::ListAppend    => fun(list(var(0)), fun(list(var(0)), list(var(0)))),
// concat : List (List a) -> List a
K::ListConcat    => fun(list(list(var(0))), list(var(0))),
// take : Int -> List a -> List a
K::ListTake      => fun(int(), fun(list(var(0)), list(var(0)))),
// drop : Int -> List a -> List a
K::ListDrop      => fun(int(), fun(list(var(0)), list(var(0)))),
// zip : List a -> List b -> List (a, b)
K::ListZip       => fun(list(var(0)), fun(list(var(1)), list(tuple2(var(0), var(1))))),
// cons : a -> List a -> List a
K::ListCons      => fun(var(0), fun(list(var(0)), list(var(0)))),
// isEmpty : List a -> Bool
K::ListIsEmpty   => fun(list(var(0)), bool_ty()),
// concatMap : (a -> List b) -> List a -> List b
K::ListConcatMap => fun(fun(var(0), list(var(1))), fun(list(var(0)), list(var(1)))),
// indexedMap : (Int -> a -> b) -> List a -> List b
K::ListIndexedMap => fun(
    fun(int(), fun(var(0), var(1))),
    fun(list(var(0)), list(var(1))),
),
```

No obligation/tie wiring needed: none of the nine carry an `Ord`/`Eq` bound
(`member`, which needs `PartialEq`, is already wired). `concatMap`/`indexedMap`
closures are `Fn + Clone` — the lowerer already emits `Fn + Clone` closure kernels
(`map`/`filter`/`foldl`), so no new closure machinery.

---

## 5. Kernel registry lines (`crates/sky_kernels/src/lib.rs`)

Add nine `KernelFn` enum variants and their `d(...)` decls (mirror the existing
`Self::ListMap => d("List", "map", 2, Pure, "list_map_consume")` shape), and add all
nine to `KernelFn::ALL`.

```rust
Self::ListAppend     => d("List", "append",     2, Pure, "list_append"),
Self::ListConcat     => d("List", "concat",     1, Pure, "list_concat"),
Self::ListTake       => d("List", "take",       2, Pure, "list_take"),
Self::ListDrop       => d("List", "drop",       2, Pure, "list_drop"),
Self::ListZip        => d("List", "zip",        2, Pure, "list_zip"),
Self::ListCons       => d("List", "cons",       2, Pure, "list_cons"),
Self::ListIsEmpty    => d("List", "isEmpty",    1, Pure, "list_is_empty"),
Self::ListConcatMap  => d("List", "concatMap",  2, Pure, "list_concat_map"),
Self::ListIndexedMap => d("List", "indexedMap", 2, Pure, "list_indexed_map"),
```

Registering these in `ALL` makes canon's `stdlib_index` yield `id = Some(k)`, so the
lower fast-path resolves them without the string arm. The string arms in §6 are added
anyway for defense-in-depth (belt-and-suspenders: if `stdlib_index` ever regresses to
`id = None`, the string arm still resolves rather than falling to SKY-L0108).

---

## 6. Lower arms (`crates/sky_lower/src/lower.rs`, List kernel block ~line 3974)

```rust
("List", "append")     => Ok(Callee::Kernel(KernelFn::ListAppend)),
("List", "concat")     => Ok(Callee::Kernel(KernelFn::ListConcat)),
("List", "take")       => Ok(Callee::Kernel(KernelFn::ListTake)),
("List", "drop")       => Ok(Callee::Kernel(KernelFn::ListDrop)),
("List", "zip")        => Ok(Callee::Kernel(KernelFn::ListZip)),
("List", "cons")       => Ok(Callee::Kernel(KernelFn::ListCons)),
("List", "isEmpty")    => Ok(Callee::Kernel(KernelFn::ListIsEmpty)),
("List", "concatMap")  => Ok(Callee::Kernel(KernelFn::ListConcatMap)),
("List", "indexedMap") => Ok(Callee::Kernel(KernelFn::ListIndexedMap)),
```

---

## 7. Canon member-array fix (`crates/sky_canon/src/env.rs` ~line 260)

`indexedMap` is **absent** from the `List` prelude-qualifier member array → it fails
at canon with `NoSuchMember` before ever reaching the lowerer. Add it:

```
"indexedMap",   // add to the ("List", &[ ... ]) member array
```

The other eight (`append/concat/take/drop/zip/cons/isEmpty/concatMap`) are already in
the array. (`find` is also absent — add it if the adjacent-gap batch in §3 is
included; out of committed scope otherwise.)

---

## 8. New runtime fns (`runtime/src/sky_runtime/list.rs`)

Four new fns. All iterative (constant stack — strictly better than the pure-Sky
O(N)-stack recursion the Go backend uses; see §10). No `unsafe`, no indexing, no
`unwrap`, no overflow, no panic vector — verified below.

```rust
/// Sky `append : List a -> List a -> List a`.
pub fn list_append<T>(mut xs: Vec<T>, ys: Vec<T>) -> Vec<T> {
    xs.extend(ys);
    xs
}

/// Sky `concat : List (List a) -> List a`.
pub fn list_concat<T>(xss: Vec<Vec<T>>) -> Vec<T> {
    xss.into_iter().flatten().collect()
}

/// Sky `take : Int -> List a -> List a`. Elm semantics: n<=0 → []; n>=len → whole.
pub fn list_take<T>(n: i64, mut xs: Vec<T>) -> Vec<T> {
    // n.max(0) is >= 0, so the `as usize` cast is total on 64-bit targets
    // (i64::MAX fits in usize). truncate(k) with k >= len is a no-op → whole list.
    xs.truncate(n.max(0) as usize);
    xs
}

/// Sky `cons : a -> List a -> List a`. Prepend; same result as the `::` operator.
pub fn list_cons<T>(x: T, mut xs: Vec<T>) -> Vec<T> {
    xs.insert(0, x); // O(n) shift; total, no panic
    xs
}

/// Sky `isEmpty : List a -> Bool`.
pub fn list_is_empty<T>(xs: Vec<T>) -> bool {
    xs.is_empty()
}
```

Soundness ledger for the new fns:
- `list_take`: the only cast is `n.max(0) as usize`. `n.max(0) >= 0`; on 64-bit
  `i64` non-negative values fit `usize`. `truncate` clamps internally → no OOB, no
  panic, no overflow. Total.
- `list_append`/`list_concat`: `extend`/`flatten` are total; ownership consumes both
  inputs (no `Clone` bound required). Iterative.
- `list_cons`: `Vec::insert(0, _)` is total (index 0 always valid). O(n) shift is
  acceptable for a cons of a materialised list; no panic.
- `list_is_empty`: total.

Reused fns (already vetted, iterative, `collect`-based — no recursion, constant
stack): `list_drop` (`list.rs:42`, guards `n<=0`), `list_zip` (`:117`,
`Iterator::zip` stops at shorter), `list_concat_map` (`:114`), `list_indexed_map`
(`:108`, `i as i64`).

Ownership/Clone: none of the nine require a `Clone`/`PartialEq`/`Ord` bound on the
element type except the closure `Fn + Clone` on `concat_map`/`indexed_map`, already
supported. This keeps monomorphisation clean and avoids spurious bounds.

---

## 9. Go byte/semantic parity confirmation

| op | edge input | expected (Elm/Go) | ipê fn behaviour |
|---|---|---|---|
| `take` | `take 5 [1,2]` | `[1,2]` | `truncate(5)` no-op → `[1,2]` ✓ |
| `take` | `take 0 [1,2]` / `take (-3) [1,2]` | `[]` | `n.max(0)=0`, `truncate(0)` → `[]` ✓ |
| `drop` | `drop (-3) [1,2]` | `[1,2]` | `list_drop` guards `n<=0` → xs ✓ |
| `drop` | `drop 5 [1,2]` | `[]` | `skip(5).collect()` → `[]` ✓ |
| `zip`  | `zip [1,2,3] [9,8]` | `[(1,9),(2,8)]` | `Iterator::zip` truncates to shorter ✓ |
| `concat` | `concat [[1,2],[],[3]]` | `[1,2,3]` | `flatten` ✓ |
| `append` | `append [] [1]` | `[1]` | `extend` ✓ |
| `cons` | `cons 0 []` | `[0]` | `insert(0,0)` → `[0]` ✓ |
| `isEmpty` | `isEmpty []` / `isEmpty [1]` | `True` / `False` | `is_empty()` ✓ |
| `concatMap` | `concatMap (\x -> [x,x]) [1,2]` | `[1,1,2,2]` | `flat_map` ✓ |
| `indexedMap` | `indexedMap (\i x -> i) [9,9,9]` | `[0,1,2]` | `enumerate`, `i as i64` ✓ |

Output-identical to Go across all edges. No sanctioned output divergence.

---

## 10. Recorded divergence (efficiency-only, output-identical)

Record in `docs/divergences-from-sky.md`:

> ipê wires the non-HOF `List` ops (`append/concat/take/drop/zip/cons/isEmpty`) as
> **iterative Rust kernels (constant stack)**, whereas the Go "Sky" backend classifies
> them as non-tail-recursive pure-Sky (O(N) call-stack). Output is byte-identical;
> ipê has a strictly better stack profile (no 200k+-element stack-depth risk).
> Reason: `List.*` is kernel-anchored in ipê canonicalisation, so the kernel path is
> the only exit-0-safe wiring — the improved stack behaviour is a free consequence,
> not a behavioural change. `concatMap`/`indexedMap` are kernels in both backends.

---

## 11. Golden plan (TDD — RED today, GREEN after)

Add one golden that exercises all nine ops. Fails today at `List.zip` with
SKY-L0108 (canon+lower gap); after wiring, compiles and runs.

`goldens/.../m_list_ops_wiring/src/Main.sky`:

```elm
module Main exposing (main)

import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)


toStr : List Int -> String
toStr xs =
    "[" ++ String.join "," (List.map String.fromInt xs) ++ "]"


main =
    let
        a       = List.append [ 1, 2 ] [ 3, 4 ]        -- [1,2,3,4]
        c       = List.concat [ [ 1 ], [ 2, 3 ], [] ]   -- [1,2,3]
        t       = List.take 2 [ 9, 8, 7 ]               -- [9,8]
        tNeg    = List.take (-1) [ 9, 8, 7 ]            -- []
        d       = List.drop 5 [ 9, 8, 7 ]               -- []
        dNeg    = List.drop (-1) [ 9, 8 ]              -- [9,8]
        cn      = List.cons 0 [ 1, 2 ]                  -- [0,1,2]
        cm      = List.concatMap (\x -> [ x, x ]) [ 1, 2 ]   -- [1,1,2,2]
        im      = List.indexedMap (\i _ -> i) [ 5, 5, 5 ]    -- [0,1,2]
        z       = List.zip [ 1, 2, 3 ] [ 4, 5 ]        -- length 2
        emptyT  = List.isEmpty ([] : List Int)          -- True
        emptyF  = List.isEmpty [ 1 ]                    -- False
    in
    println
        (toStr a ++ " " ++ toStr c ++ " " ++ toStr t ++ " " ++ toStr tNeg
            ++ " " ++ toStr d ++ " " ++ toStr dNeg ++ " " ++ toStr cn
            ++ " " ++ toStr cm ++ " " ++ toStr im
            ++ " zip=" ++ String.fromInt (List.length z)
            ++ " " ++ (if emptyT then "T" else "F")
            ++ " " ++ (if emptyF then "T" else "F"))
```

Expected stdout (single line):

```
[1,2,3,4] [1,2,3] [9,8] [] [] [9,8] [0,1,2] [1,1,2,2] [0,1,2] zip=2 T F
```

(If the golden harness requires `zip`'s tuples printed directly, extend `toStr` with
a tuple formatter; `List.length z` keeps the golden element-type-agnostic and still
proves `zip` truncated to the shorter length = 2.)

Also add a **fail-closed regression** asserting each of the nine names resolves to a
`Some(k)` scheme in `constrain.rs` (extend the existing FIRST_SCHEMED gate list), so a
future refactor that drops a `stdlib_scheme` arm trips the "was-a-hole" tripwire
instead of silently reopening SKY-L0108 or `Ty::Var(MAX)`.

---

## 12. Ordered implementation checklist (TDD)

1. **RED**: add the golden `m_list_ops_wiring` (§11). Verify it fails today at
   `List.zip` with SKY-L0108 (confirms the gap before touching code).
2. `runtime/src/sky_runtime/list.rs`: add `list_append`, `list_concat`, `list_take`,
   `list_cons`, `list_is_empty` (§8). Add a `#[test]` per fn covering the §9 edges
   (empty, negative, over-length, truncation).
3. `crates/sky_kernels/src/lib.rs`: add 9 `KernelFn` variants + 9 `d(...)` decls +
   9 `ALL` entries (§5).
4. `crates/sky_types/src/constrain.rs`: add 9 `K::List*` arms to `stdlib_scheme`
   (§4). Extend the FIRST_SCHEMED "were-holes" gate list with the 9 variants.
5. `crates/sky_canon/src/env.rs`: add `"indexedMap"` to the `List` member array (§7).
6. `crates/sky_lower/src/lower.rs`: add 9 string arms to the List kernel block (§6).
7. **GREEN**: build the golden; assert stdout matches §11 exactly.
8. Re-run the kernel parity/tripwire tests + the FIRST_SCHEMED gate.
9. Record the §10 divergence in `docs/divergences-from-sky.md`.
10. File the adjacent `any`/`all`/`find` gap (§3) as a same-family follow-up (add
    `list_find`; wire `any`/`all` kernels) — do not leave them as latent SKY-L0108.

---

## 13. Already-correct — DO NOT TOUCH

**10 already-wired List kernels** (KernelFn + lower arm + scheme all present;
`constrain.rs:2196+`, `kernels lib.rs:626+`, `lower.rs:3974+`):
`map, filter, foldl, foldr, length, head, tail, member, range, reverse`.

**Intentionally kernel-only HOFs (pure-Sky blocked by the T2 hole — the SOUNDNESS
TRAP; keep kernel-anchored):** `map, filter, foldl, foldr, concatMap, indexedMap`,
and (once wired per §3) `find, any, all, filterMap, sortBy`. Their pure-Sky bodies in
`Sky.Core.List` recurse over a `func(any) any` lambda return → `cannot infer T2` at
cross-module call sites. Do NOT migrate them to pure-Sky routing until typed lambda
lowering closes Limitation #18 / Gap 4.

**Do NOT** attempt to route any `List.x` to the `Sky.Core.List` pure-Sky source while
the prelude-qualifier install anchors `List.*` to `VarHome::Kernel` — that path is
inert and would silently keep resolving to the kernel (or, if the anchor is removed,
reopen the T2 hole for the HOFs).

---

## 14. Adversarial self-review

- **Silent miscompile path?** None. The exit-0 seal is preserved by the fail-closed
  scheme: a missing `stdlib_scheme` arm → `None` → `Diagnostic::Lower` (SKY error),
  never exit-0-then-cargo-fail. Emission is data-driven from the `d(...)` runtime_fn
  string, so a wired `KernelFn` with a wrong runtime name would fail at **cargo**
  (unknown fn) — caught by the golden build, not shipped. The FIRST_SCHEMED gate
  (step 4) prevents a future silent regression to `Ty::Var(MAX)`.
- **Any pure-Sky routing that trips T2?** None — zero ops are routed pure-Sky. The
  two HOFs (`concatMap`, `indexedMap`) that *would* trip T2 are kernels, so they get
  their type from the scheme, not from a cross-module inferred lambda return.
- **Any op left unschemable?** No — §4 gives a concrete polymorphic `Ty` for all nine;
  none needs an `Ord`/`Eq` obligation the scheme can't express.
- **`list_take` cast safety?** `n.max(0) as usize` is total on 64-bit; `truncate`
  clamps. No panic, no overflow. (32-bit targets: `i64` values > `usize::MAX` would
  saturate on cast, but `truncate` past `len` is still a no-op → correct result; the
  backend targets 64-bit regardless.)
- **`indexedMap` canon gap?** Caught: §7 adds it to the member array; without that
  step the golden fails at canon `NoSuchMember`, not at lower — step 1's RED probe
  should be read carefully to confirm the failure site once `indexedMap` is exercised.
- **Stack safety?** All nine kernels are iterative (new fns and reused fns are
  `collect`/`extend`/`insert`-based) → constant stack; strictly better than the
  documented pure-Sky O(N)-stack profile. Recorded (§10).

No inline fixes required after review — the spec is internally consistent.
```
