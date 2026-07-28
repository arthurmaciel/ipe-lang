# Or-patterns (`|` alternatives) in `case … of`

Status: design-only (no code).

An **or-pattern** lets one `case` arm match several shapes:

```
case msg of
  Up | Down -> "vertical"
  Left | Right -> "horizontal"
```

and, with bindings, the alternatives may each bind the same variable:

```
case shape of
  Circle r | Square r -> area r
```

Every alternative of a single or-pattern must bind the **identical set of
variables at the identical types** — the OCaml / Rust / F# rule. That
constraint is what makes `area r` well-typed regardless of which alternative
matched.

Related designs (referenced by concept, not duplicated here):

- The Maranget usefulness pass in `src/compiler/types/src/exhaust.rs`
  (`UPat` / `Head`) — exhaustiveness (IPE-T0010) and redundancy (IPE-T0011).
  Or-patterns extend it by *row expansion*, which the algorithm already
  supports.
- The closed-union catch-all rule (sibling design under `tbd/`): or-patterns
  are the strongest ergonomic mitigation for it, making explicit enumeration
  of a large union cheap.
- **Pattern guards** are a **separate** feature (see Boundaries, §7). They are
  designed and shipped independently; this design does not depend on them, and
  where the two compose the guard binds looser than `|` (`A | B if c -> …`, the
  guard applies to the whole or-pattern).

This is a **departure from Elm** (which has no or-patterns) toward Rust / OCaml
parity; it is recorded in `docs/divergences-from-elm.md`.

---

## 1. Syntax and AST

### 1.1 Surface syntax

The alternative separator is a single `|` (**not** `||`). `|` already reads as
"one of these" in ADT sum declarations (`type Dir = Up | Down`), matches
Rust / OCaml / F# / Python, and maps 1:1 to Rust's native or-pattern; `||`
would overload one token with two structurally different meanings. The token
`Tok::Pipe` already exists in the lexer and is consumed today only in `type`
declarations and open-record row syntax — never inside a `case`-arm pattern —
so there is no lexer change and no grammar ambiguity at a pattern position.

An or-pattern is a sequence of two or more alternatives separated by `|`:

```
p1 | p2 | … | pn
```

Precedence: `|` binds **looser than everything else in a pattern** — looser than
constructor application, `::` (cons), and `as`. So:

- `Just a | Nothing` parses as `(Just a) | (Nothing)`, not `Just (a | Nothing)`.
- `x :: xs | []` parses as `(x :: xs) | ([])`.
- `(inner as w) | other` — an `as` inside an alternative binds tighter than
  `|`. (An `as` *outside* the whole or-pattern is discussed in §1.3.)

### 1.2 Where it is allowed — nesting

Or-patterns are allowed both at the **top level of a `case` arm** and
**nested** inside any constructor / tuple / list / cons sub-pattern position:

```
case pair of
  (Up | Down, n) -> n          -- nested inside a tuple element
  _              -> 0

case boxed of
  Wrap (Red | Green) -> "warm-ish"   -- nested inside a ctor payload
  Wrap Blue          -> "cool"
```

Nesting is standard in Rust and OCaml and falls out for free from the usefulness
algorithm (§5) and the recommended lowering (§4), so there is no reason to
forbid it. The one nesting restriction is the binding rule (§2): a nested
or-pattern's alternatives must still agree on their binder set, and that binder
set participates in the enclosing pattern's overall binder set.

### 1.3 AST representation — recommendation

**Add a first-class `POr` node to the pattern enum**, in both the source AST
(`syntax::Pattern_`) and the name-resolved AST (`canon::Pattern_`):

```
/// An or-pattern `p1 | p2 | …` — matches if ANY alternative matches. Every
/// alternative binds the identical set of variables at identical types
/// (enforced in canon/types); each alternative is an arbitrary sub-pattern
/// and recurses. Invariant: length ≥ 2 (a single pattern is never wrapped).
POr(Vec<Pattern>)
```

Rationale for a `POr` node over the alternative of *arm-level alternatives*
(making a `case` arm carry `Vec<Pattern>` instead of one `Pattern`):

- **Nesting.** `POr` composes anywhere a `Pattern` appears, so
  `(Up | Down, n)` needs no special case. Arm-level alternatives would only
  handle the top level and would need a second, redundant mechanism for the
  nested form.
- **Uniform recursion.** Every existing consumer (`is_irrefutable`,
  `refutable_span`, `to_upat`, `pattern_uses_unknown_ctor`, the lowerer's
  pattern walk) already recurses over `Pattern_`; each gets exactly one new
  arm. An arm-level vector would fork the shape of `CaseBranch` and touch every
  branch consumer instead.
- **Locality.** The binding-consistency check (§2) is a property of the `POr`
  node itself, checkable in isolation.

Invariant: `POr` carries **≥ 2** alternatives. The parser never wraps a lone
pattern in `POr` (a bare `p` stays `p`), mirroring the existing arity-≥-2
invariants on `PTuple`.

Top-level `as` over a whole or-pattern (`(A | B) as w`) is expressible because
`as` already wraps an arbitrary inner pattern (`PAlias(Box<Pattern>, …)`); the
inner is a `POr`. `w` then binds the whole matched value and is added to the
common binder set (it is bound identically on every alternative by
construction).

### 1.4 Parser change

`parse_pattern` gains one loosest-precedence layer, wrapping the current
cons/`as` result. After parsing a full pattern (through `::` and `as`), if the
next token is `Tok::Pipe`, collect one-or-more further alternatives:

```
parse_pattern      := parse_or
parse_or           := parse_cons_as ( "|" parse_cons_as )*
parse_cons_as      := <current parse_pattern body: ctor-app, "::", "as">
```

If exactly one alternative is parsed, return it unwrapped; otherwise return
`POr(alternatives)` spanning the first through the last. Because `|` is loosest,
each alternative is a complete cons/`as` pattern, so `x :: xs | []` and
`(inner as w) | y` parse as intended with no backtracking.

Guard against a stray leading/trailing `|` (`| A -> …` or `A | -> …`) with the
existing `MalformedCase` / pattern parse-error path — an empty alternative is a
parse error, not an empty `POr`.

---

## 2. The binding rule and its diagnostic

### 2.1 The rule (load-bearing)

Every alternative of an or-pattern must bind:

1. the **same set of variable names**, and
2. each name at the **same type** across all alternatives.

Example, accepted — both alternatives bind exactly `{ r }`, both at the payload
type of their (equally-typed) constructor:

```
Circle r | Square r -> area r
```

Rejected — the alternatives bind different sets:

```
Circle r | Nothing -> …   -- `r` bound on the left, unbound on the right
```

Rejected — mismatched types for the same name:

```
Wrap i | Tag s -> …       -- `i : Int` on the left, `s`≠`i`; and even
                          -- `Wrap x | Tag x` with x:Int vs x:String is rejected
```

Wildcards and literals bind nothing, so they impose no obligation of their own:
`_ | Nothing`, `0 | 1`, and `Up | Down` are all fine (empty common binder set).
An alternative that is a bare variable binds that one name and therefore must
appear in every sibling: `x | Nothing` is rejected (`x` unbound on the right),
but `Just x | Wrap x` is accepted when both payloads have the same type.

### 2.2 Where the check runs

Name binding is resolved in **canon**; the *set-equality* half of the rule
(same names) is a purely syntactic property of the resolved `POr` and is checked
there, fail-fast, before types run. The *same-type* half needs the solver, so it
is checked in **types**, after the constraint solver settles, when each binder's
type is known — the same stage as exhaustiveness (`exhaust.rs`). Splitting it
this way lets the cheap, always-decidable name-set mismatch be reported without
waiting on inference, while the type mismatch rides the existing post-solve pass.

Mechanically:

- **canon**: for a `POr([p1..pn])`, compute each alternative's bound-name set
  (`{ Symbol }`). If they are not all equal, emit the mismatch diagnostic
  naming the offending name(s) and the alternative that differs. The scope
  exported by the whole `POr` is that (now-equal) common set — one binding per
  name, so downstream scoping sees a single, unambiguous binder set.
- **types**: when solving the arm, unify the type variable of each binder
  *across alternatives* (name `r`'s occurrence in alt 1 unifies with `r`'s in
  alt 2, …). A unification failure here is the same-name-different-type case; it
  surfaces through the standard type-mismatch machinery, attributed to the
  or-pattern span.

### 2.3 The diagnostic

Reuse of an existing code is inappropriate (T0013 is ctor arity; T0010/T0011 are
coverage). **Introduce a new `TypeError` variant and code `IPE-T0019`** — the
next free code in the T range (T0017 is the highest defined; **T0018 is reserved
by the closed-union catch-all design** for the open-case lint, so or-patterns
take T0019).

- **Name-set mismatch** (the canon half): `IPE-T0019`
  "each alternative of an or-pattern must bind the same variables". The message
  lists which names are bound by some but not all alternatives, and points at
  the first alternative that diverges. A progressive `explain/IPE-T0019.md` page
  follows the compiler-as-kind-teacher convention (ELI10 → the OCaml/Rust rule →
  why: the arm body reads `r` without knowing which alternative matched, so `r`
  must exist and have one type on every path).
- **Same-name / different-type** (the types half): surfaces through the standard
  type-mismatch diagnostic, its span the or-pattern, with a note cross-linking
  IPE-T0019 so the teaching is unified.

Both are **errors** (not warnings): the arm body is otherwise ill-typed or
references an unbound name, so accepting the program is unsound.

---

## 3. Typecheck

Threading inference through an or-pattern:

1. **Scrutinee type.** All alternatives are checked against the **same expected
   type** — the scrutinee's type (or, when nested, the type of the sub-position
   the `POr` occupies). This is the ordinary "each alternative unifies with the
   position it sits in" rule; nothing special beyond visiting each alternative
   with the same expected type.
2. **Binder-set consistency.** Per §2.2, the binders of alternative *i* are
   unified name-by-name with those of alternative *1*, so the arm body sees one
   binder environment. Because canon already proved the *names* equal, this
   unification is total over the shared name set — every name resolved in the
   body has exactly one type variable, unified across all alternatives.
3. **Body.** The arm body is checked **once**, in the common binder environment.
   It is never checked per-alternative — a direct consequence of the binding
   rule and the reason or-patterns don't multiply type-checking cost.

Inference is otherwise unchanged: literal alternatives constrain the scrutinee
to their literal's type exactly as a lone literal pattern does; a nested
`POr` inside a ctor payload constrains that payload position and recurses.

---

## 4. Lowering

### 4.1 The decision

**Lower `POr` to a native Rust or-pattern, with the body emitted once.** The
backend already renders each Ipê `case` arm as its own Rust `match` arm and
relies on rustc to resolve overlap and ordering across arms (this is how nested
constructor arms like `Som (Som x)` / `Som Non` / `Non` are emitted one-to-one
today). Rust's `match` natively supports `A | B => body` and `Wrap(Red | Green)
=> body`, including bindings shared across alternatives (`Circle(r) | Square(r)
=> area(r)`), with **exactly one** copy of the body. So the natural lowering is:

- Add a `Pat::Or(Vec<Pat>)` variant to the IR pattern enum, recursing like the
  other nesting nodes (`Ctor` / `Tuple` / `Record` / `Slice`).
- Lower `canon::POr([p1..pn])` to `Pat::Or([lower(p1)..lower(pn)])`.
- The backend renders `Pat::Or([a, b, …])` as `a | b | …` in the emitted Rust,
  joining the rendered sub-patterns with ` | `.

The arm body is lowered and emitted **once**, wired to the single `Pat::Or`
arm — no duplication.

### 4.2 Why not desugar to duplicated arms

The alternative — desugaring `A | B -> e` into two arms `A -> e` and `B -> e` —
is rejected because it **duplicates the body `e`**. Duplication:

- **Doubles code size** for every alternative (compounding under nesting: an
  `n`-way `POr` inside another multiplies).
- **Duplicates side effects at the syntax level.** Ipê expressions are pure, so
  this is not an evaluation hazard here, but the emitted Rust grows and rustc
  must monomorphise the duplicated body repeatedly — pure code-size and
  compile-time waste.

If the language had chosen a desugaring backend (no native `match`), the sound
fix would be a **shared join point**: bind the body to a local continuation
(`let k = || e in`) and have each duplicated arm tail-call `k`, so the body is
emitted once. But since the backend *is* native Rust `match`, `Pat::Or` gives us
the join-point-free, zero-duplication result directly and more simply. **Favor
the first-class IR or-pattern.**

### 4.3 Interaction with the existing arm machinery

- **List / cons alternatives.** Cons/list patterns lower today to a `Pat::Slice`
  (or a length-guarded binder) rather than a `Pat` discriminant. A `POr` whose
  alternatives are slice-shaped lowers to `Pat::Or` of `Pat::Slice`s, which Rust
  accepts (`[] | [_] => …`). Where an alternative would instead need an
  **arm-level guard** (the length-guard fallback for a cons nested in a ctor
  payload), that guard cannot be attached to one branch of a Rust or-pattern; in
  that specific residual case the lowerer falls back to duplicated arms **with a
  shared-continuation join point** (per §4.2) so the guard can differ per
  alternative without duplicating the body. This residual is expected to be rare
  and is called out here so the lowerer stays total rather than emitting an
  invalid guarded or-pattern.
- **Destructure head.** An or-pattern is **refutable** (it discriminates), so it
  never takes the single-arm `Destructure` path; `POr` is not irrefutable
  (§4.4) and always routes through the `Match` path.

### 4.4 `is_irrefutable`

`POr` is irrefutable **iff every alternative is irrefutable** — in practice
never, because two distinct irrefutable alternatives are redundant (the second
is unreachable) and a lone one is unwrapped. Define `is_irrefutable(POr(alts)) =
alts.iter().all(is_irrefutable)` for totality, but a well-formed `POr` (≥ 2
distinct alternatives) is treated as refutable. This keeps the param-irrefutable
gate (IPE-T0015) correctly **rejecting** an or-pattern in a binding position (a
function parameter or `let` binder), where it has no meaning.

---

## 5. Exhaustiveness and redundancy (the crux)

The Maranget usefulness algorithm in `exhaust.rs` operates on a matrix of `UPat`
rows (`Wild | Ctor(Head, subpats)`). Or-patterns integrate by **row expansion**,
which the algorithm already supports natively.

### 5.1 Coverage: `A | B` covers both

In the abstraction step (`to_upat`), a `POr([p1..pn])` does **not** become a new
`UPat` kind. Instead, when a pattern row is inserted into the matrix, an
alternative-bearing row is **expanded into one row per alternative**:

```
row  [ … , (A | B) , … ]
expands to
row  [ … , A , … ]
row  [ … , B , … ]
```

An or-pattern at column *c* multiplies that one arm into `n` matrix rows that are
identical except at column *c*. A nested `POr` (`Wrap (Red | Green)`) expands the
same way after the outer ctor is specialised: specialising `Wrap` exposes the
payload column, whose `Red | Green` then expands into two rows. Expansion is
recursive and the cartesian product over multiple `POr`s in one row is taken
(two independent 2-way `POr`s in one arm produce 4 rows) — this is the standard
treatment and matches Maranget §Or-patterns.

Consequence for **IPE-T0010 (non-exhaustive)**: an arm
`Red | Green | Blue -> e` over `type Color = Red | Green | Blue` expands to
three rows covering all three constructors, so the wildcard-usefulness query
returns "not useful" — the `case` is **exhaustive**, no witness. This is exactly
the property that makes or-patterns the ergonomic mitigation for the
closed-union catch-all rule: enumerate the union in one arm and it counts as
full coverage.

### 5.2 Redundancy: a covered alternative is flagged

Redundancy (**IPE-T0011**) is the same usefulness query applied per row: a row
is redundant when it is *not useful* against the rows above it. Because each
alternative becomes its own row, redundancy is detected at **alternative
granularity**:

```
case c of
  Red | Green -> a
  Green | Blue -> b    -- the `Green` alternative here is redundant:
                       -- already covered by row 1
```

The `Green` row of arm 2 is not useful against arm 1's rows → **IPE-T0011**, and
the diagnostic points at the *specific redundant alternative* (its span), not the
whole arm — arm 2 is still reachable via `Blue`. An or-pattern **all** of whose
alternatives are already covered flags the whole arm redundant, as today.

An **internally-redundant** or-pattern (`Red | Red`, or `Red | (Red as x)`) is
likewise caught: the second `Red` row is not useful against the first. This is
reported as IPE-T0011 against the duplicate alternative.

### 5.3 Why row expansion is airtight

Expanding `A | B` into two rows makes the matrix *semantically identical* to
writing the two arms out by hand — the usefulness algorithm's soundness and
completeness theorems (coverage ⇔ wildcard-not-useful; redundancy ⇔
row-not-useful) carry over unchanged, because the matrix is literally the
same one the hand-written form produces. There is no new fixpoint, no new
signature-completion rule, and no interaction with the OPEN-type handling
(`Int` / `Char` / `Str` alternatives expand to their literal rows exactly as
lone literal patterns do; they still never complete an open signature). This is
the load-bearing reason to expand rather than to invent a `Head::Or` — a bespoke
head would require re-proving coverage; expansion inherits the existing proof.

The `pattern_uses_unknown_ctor` guard recurses into `POr` alternatives: if any
alternative references a constructor outside the module's known unions, the whole
`case` is excluded from the matrix walk (consistent with the existing
unknown-scrutinee skip), so no expansion is attempted against an incomplete
signature.

---

## 6. Test plan

Golden (accept + correct behaviour) and negative (rejected with the right code)
fixtures, mirroring the existing `case` fixture layout:

**Golden**

1. **Matches either alternative** — `Up | Down -> "v"` over a 4-variant `Dir`;
   run each constructor, assert the shared body fires for both alternatives.
2. **Shared binding** — `Circle r | Square r -> area r`; assert the bound `r`
   is read correctly whichever alternative matched.
3. **Exhaustive via or-pattern** — `Red | Green | Blue -> e` over
   `Color = Red | Green | Blue` with **no** wildcard arm; assert it compiles
   (IPE-T0010 satisfied), proving §5.1.
4. **Nested or-pattern** — `Wrap (Red | Green) -> …` and
   `(Up | Down, n) -> n`; assert both parse, type, and run.
5. **Top-level `as` over an or-pattern** — `(A | B) as whole -> use whole`.
6. **Literal alternatives** — `0 | 1 -> "small"` with a `_ ->` fallback (Int is
   open); assert exhaustiveness still requires the fallback.

**Negative**

7. **Binding-set mismatch** — `Circle r | Nothing -> …` → **IPE-T0019**, message
   names `r`.
8. **Same-name / different-type** — `Wrap i | Tag i -> …` with `i:Int` vs
   `i:String` → type-mismatch (cross-linked to IPE-T0019).
9. **Redundant alternative** — `Red | Green -> a` then `Green | Blue -> b` →
   **IPE-T0011** at the second `Green`.
10. **Internally redundant** — `Red | Red -> …` → **IPE-T0011** at the
    duplicate.
11. **Or-pattern in a binding position** — `\(A | B) -> …` or
    `let (A | B) = …` → **IPE-T0015** (refutable parameter), proving §4.4.
12. **Malformed** — leading/trailing `|` (`| A -> …`, `A | -> …`) → parse error.

---

## 7. Boundaries — out of scope

- **Pattern guards** (`pattern if cond -> body`) are a **separate** feature with
  their own design. They are not required by or-patterns and are not designed
  here. Where the two eventually compose, the guard binds looser than `|`
  (`A | B if cond -> …`, the guard applies to the whole or-pattern), matching
  Rust; and a guarded row is treated as **non-covering** by the exhaustiveness
  check (a guard may be false) — that is the guards design's soundness floor,
  not this one's. This design must not weaken the exhaustiveness pass in any way
  that assumes guards.
- **Range patterns** (`1..9`), **binding `@` on literals**, and any pattern
  extension beyond the `|` alternative are out of scope.
- **Or-patterns in `type` declarations / open-record rows** are unrelated uses
  of `|` and unchanged; this design touches only `|` at a `case`-arm pattern
  position.

---

## Summary

| Question | Decision |
|---|---|
| AST | First-class `POr(Vec<Pattern>)` node (≥ 2 alts) in `syntax` + `canon`, and `Pat::Or(Vec<Pat>)` in IR; nesting allowed everywhere a pattern appears. |
| Syntax | Single `|`, loosest pattern precedence (looser than ctor-app, `::`, `as`). |
| Binding rule | Every alternative binds the identical name set at identical types; name-set checked in canon (fail-fast), type equality in types (post-solve). |
| Diagnostic | New **IPE-T0019** for the name-set mismatch (T0018 reserved by the closed-union design); type mismatch rides the standard type-error path, cross-linked. |
| Lowering | Native Rust or-pattern via `Pat::Or`, body emitted **once** — no duplication; residual guarded-alternative case falls back to a shared-continuation join point. |
| Exhaustiveness/redundancy | Maranget **row expansion**: `A \| B` becomes two matrix rows, so it covers both (IPE-T0010) and a covered alternative is flagged per-alternative (IPE-T0011); expansion inherits the existing usefulness proof unchanged. |
