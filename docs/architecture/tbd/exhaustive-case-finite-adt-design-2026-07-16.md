# Closed-union `case` refuses catch-all arms (design + spec + plan)

Status: design-only (no code). Companion design:
`ipe-lint-tool-design-2026-07-16.md` — the two share the `@allow` directive
infrastructure, the `Suggestion` fix machinery, and the LSP quick-fix surface.

Related: `src/compiler/types/src/exhaust.rs` (the Maranget usefulness pass this
rule extends), the shipped language server (the code-action counterweight —
recorded in `docs/adr/0034-language-server-salsa-second-consumer.md`),
`src/compiler/diagnostics/explain/IPE-T0010.md` (whose teaching narrative this
rule makes true — see §2), `docs/divergences-from-elm.md` (ledger entry).

> References below to `ipe-lsp.md` mean the shipped language-server design now
> recorded in `docs/adr/0034-language-server-salsa-second-consumer.md`.

This is a **departure from Elm** (and from the Ipê reference, which follows
Elm here): both accept `_ ->` as a catch-all over any type. Ipê will refuse a
catch-all arm when the scrutinee is a closed, finite-variant union — so adding
a variant is a compile error at every match site, never a silent fall-through.

---

## Executive summary

| Question | Decision |
|---|---|
| The rule | In `case scrut of`, an arm whose **top-level pattern** is a catch-all (`_` or a bare variable) is rejected when the scrutinee's solved type is a **closed union** (user-declared ADT, `Maybe`, `Result`) *and* the catch-all absorbs ≥ 1 constructor not matched by an earlier arm. |
| Open domains | `Int` / `Float` / `String` / `Char` keep the wildcard (a finite literal set can never cover them). `Bool` and `List` are excluded from the rule in v1 (§3.3). |
| Where | The compiler — `sky_types::exhaust`, alongside IPE-T0010/T0011, post-solve. Acceptance-changing rules live in the compiler, never in the (advisory) lint tool. |
| Escape hatch | Yes — per-site `-- @allow(open-case) <reason>` directive (shared grammar/table with the lint tool). See §4 for the weighing. |
| Recommendation | **Adopt-with-opt-out** (§5). |
| Diagnostic | New **IPE-T0018** "catch-all hides constructors of a closed type", listing the absorbed constructors; progressive explain page; IPE-T0010's page corrected in the same change (§6). |
| Ergonomic counterweight | LSP code action + `ipe fix` suggestion that **expands the catch-all into one arm per absorbed constructor with the catch-all's own body** — semantics-preserving, `MachineApplicable` (§7). |

## 1. Why (the evolution-safety argument)

Exhaustiveness checking (IPE-T0010) guarantees a `case` covers every value.
A catch-all arm satisfies the checker while destroying the property teams
actually rely on: **when a union grows a variant, every `case` that must now
decide about it should fail to compile.** With `_ ->` present, the new variant
silently takes the catch-all branch — the classic TEA failure is a new `Msg`
variant silently no-op'ing through `update`'s `_ -> (model, Cmd.none)`.

This is "make invalid states unrepresentable" applied to program evolution:
the invalid state is a match site that *believes* it is exhaustive over
yesterday's constructor set. Rust culture handles this with
`#[deny(wildcard_enum_match_arm)]` on selected enums and `#[non_exhaustive]`
on the flip side; Elm has community lint rules
(`elm-review` `NoWildcardCase`-style) but nothing in the language. Ipê makes
the strong default the language default, with an explicit, reasoned opt-out.

## 2. Current state (facts)

- The parser accepts `_` (`Pattern_::PAnything`) and bare-variable patterns in
  any arm; `exhaust::to_upat` abstracts both to `UPat::Wild`, which covers
  everything — so a catch-all always satisfies IPE-T0010 today.
- A catch-all that absorbs **zero** constructors (all variants already
  matched) is already flagged **IPE-T0011** (redundant branch, Warning). The
  new rule targets the complementary case: a catch-all absorbing ≥ 1 variant.
- `exhaust::check` runs inside `sky_types::infer` after the solver settles but
  **before** region types are read back — it currently judges a `case` from
  the arm patterns alone (`Sigs`, the per-module constructor tables) and never
  sees the scrutinee's type. A `case x of _ -> e` with no constructor arm is
  invisible to it today.
- **The explain page already teaches this rule.** `explain/IPE-T0010.md`
  states "Ipê has no catch-all `_` pattern, so you handle each constructor
  explicitly… when you later add a new constructor… every `case` over it will
  point you right at the gap." That is aspiration, not implementation: the
  compiler accepts `_` everywhere. This design closes the doc-vs-compiler gap
  in the direction the teaching narrative already committed to — scoped to
  closed unions, where the claim is actually desirable.
- Corpus impact: 407 catch-all arms (`_ ->`) across `examples/` +
  `src/stdlib` (many over open types — `String` route dispatch, JSON
  field fallbacks — which remain legal). The migration burden is real and is
  costed in §8.

## 3. The rule — precise specification

### 3.1 Definitions

- **Catch-all arm**: a `case` arm whose top-level pattern is `PAnything`,
  `PVar`, or a `PAlias` chain terminating in either. (A bare variable arm is
  semantically identical to `_` plus a binding; refusing only the literal `_`
  would make the rule trivially circumventable and unprincipled.)
- **Closed union** (finite-variant type): the scrutinee's solved type is
  `Ty::Con { home, name, .. }` where `(home, name)` has an entry in the
  merged module's constructor tables (`Sigs::union_ctors`) — user-declared
  `type` unions plus the Prelude-seeded `Maybe` and `Result`. Explicitly NOT
  closed unions for this rule: `Bool`, `List` (§3.3), all numeric/text
  primitives, records, tuples, functions, type variables, opaque FFI types.
- **Absorbed constructors**: the constructors matched by the catch-all row
  that no earlier arm covers — computed with the machinery already present:
  the missing-pattern witnesses of the arm matrix *excluding* the catch-all
  row (`useful(matrix_without_catchall, [Wild], …)`), exactly IPE-T0010's
  witness computation reused.

### 3.2 The judgment

For each `case`, for each catch-all arm over a closed-union scrutinee:

- absorbed = 0 → **IPE-T0011** (redundant branch — unchanged behavior);
- absorbed ≥ 1 → **IPE-T0018** (error): the arm hides those constructors.

The diagnostic lists the absorbed constructors (declaration order, capped by
the existing `WITNESS_CAP` discipline) — the same list feeds the expansion fix
(§7), so message and fix cannot disagree.

An arm-site `-- @allow(open-case) <reason>` directive suppresses IPE-T0018
for that arm only (§4).

### 3.3 Scope decisions (each an explicit trade, not an accident)

**Top-level column only.** The rule judges the arm's outermost pattern, not
nested payload positions: `case msg of SetColor _ -> …` stays legal even when
the payload is a closed union. Rationale: the evolution-safety payoff
concentrates at the top level (a new `Msg` variant must break `update`; a new
payload variant breaks the site that *dispatches on* the payload, wherever
that is), while nested enforcement explodes combinatorially — `case (a, b)`
over two 5-variant unions would demand 25 arms. **Stated limit:** adding a
variant to a type only ever matched in nested positions will not error at
those sites. The lint rule `wildcard-absorbs-variants` (companion design)
covers nested columns as an opt-in `warn` for teams that want pressure there
too — advisory where the compiler chooses not to legislate.

**`Bool` excluded.** `case b of True -> … ; _ -> …` is degenerate (one
absorbed variant, and `if` is the idiomatic spelling). `Bool` cannot grow a
variant, so the evolution argument is vacuous. The `case-bool-to-if` lint
handles the style angle.

**`List` excluded.** `Nil | Cons` is closed but cannot grow either; `case xs
of x :: rest -> … ; _ -> …` refusing `_` would force `[] ->` — near-zero
cost, but with zero evolution payoff there is no principled basis to refuse.
Both exclusions are revisitable; v1 draws the line at "types whose
constructor set can actually change".

**Type variables / unknown scrutinees.** A generic scrutinee (`Ty::Var`) or a
constructor set the checker cannot resolve is never judged — the rule fails
open (no IPE-T0018), consistent with `check_case`'s existing
unknown-constructor skip. Wrongly *rejecting* a legal program would be a
correctness bug; wrongly permitting a catch-all is only a missed lint.

### 3.4 Interaction with filed pattern features

- **Or-patterns** (divergences-from-sky §6.3): `Red | Green | Blue -> e`
  makes explicit enumeration dramatically cheaper for grouped handling —
  the single strongest ergonomic mitigation for this rule. Sequencing them
  before (or with) enforcement is strongly preferred (§8 phase order).
- **Pattern guards** (§6.4): a guarded catch-all (`v if p v -> …`) does not
  count as covering (the guard may be false) per that design's soundness
  floor; IPE-T0018 judges only unguarded catch-alls, and a guarded one never
  silences the rule for the arms below it.

## 4. The escape hatch — options weighed

| Option | For | Against |
|---|---|---|
| **(a) No escape** (strict) | Maximum guarantee; simplest spec; matches the IPE-T0010 page's current absolutist phrasing | 50+-variant unions (`Ipe.Money`'s ISO-4217 currency enum is in our own stdlib) make full enumeration genuinely hostile: `case currency of USD -> "$" ; <49 more arms>`. Default-heavy domains (key dispatch, protocol opcodes as ADTs) become boilerplate farms. Ported Elm code breaks with no local remedy. Teams under deadline will encode the catch-all as `let v = scrut in` tricks — the rule loses and teaches circumvention. |
| **(b) Per-site opt-out** (directive) | Keeps the strong default; the exception is local, greppable, reasoned, and auditable (`unused-allow` fires when it rots); zero new syntax (reuses the lint directive grammar/table) | A suppressible error is weaker than an unsuppressible one; a team can blanket-`@allow` (mitigated by `forbidSuppression` config + review culture) |
| **(c) Reject the feature** (keep Elm semantics + ship only the lint) | Zero migration; Elm parity | The guarantee only exists for teams running lint with the rule at deny — precisely the teams that least need it. The language's own stdlib and examples wouldn't uphold it. The IPE-T0010 explain page stays false. |
| (d) Size threshold (enforce only for unions ≤ N variants) | Softens the big-enum pain automatically | A magic number in the language semantics; adding the N+1th variant *changes whether other code compiles* — a spooky action the author of the union can't see. Rejected outright. |

## 5. Recommendation — **adopt-with-opt-out** (option b)

Reasoning, in PRINCIPLES order:

- **Correctness/soundness**: unaffected either way — exhaustiveness (IPE-T0010)
  already guarantees no value escapes; this rule is about *evolution*
  robustness, not runtime safety. That is exactly why an opt-out is
  admissible here at all: `@allow(open-case)` can never make a program crash,
  unlike a hypothetical opt-out on IPE-T0010 itself (which stays
  unsuppressible, always).
- **Completeness/ergonomics**: strict (a) is honest about its goal but fails
  the `Ipe.Money` test inside our own stdlib; the directive keeps the strong
  default while giving the 50-variant `case` a one-line, reasoned exit that
  reviewers see. Or-patterns (§3.4) will further shrink the legitimate
  opt-out set over time.
- **The default matters more than the maximum**: (c) demotes the guarantee to
  opt-in tooling; experience with warning-tier discipline is that defaults
  win. On-by-default in the compiler + a visible, reasoned escape is the
  point on the curve where the guarantee is real *and* the language stays
  writable.
- **Teaching coherence**: the compiler's own explain narrative already sells
  the no-silent-fall-through philosophy; (b) makes the narrative true with
  one honest amendment ("…unless you explicitly say the case is open, with a
  reason").

## 6. Diagnostic design — IPE-T0018

- **Code**: `IPE-T0018` (next free in the T range), `TypeError` variant
  `CatchAllOverClosedUnion { union: Box<str>, absorbed: Box<[Box<str>]> }`,
  `Severity::Error`.
- **Span**: the catch-all arm's pattern (not the scrutinee — the fix is at
  the arm).
- **Message anatomy** (rendered via the existing report renderer):

  ```
  error[IPE-T0018]: this catch-all hides constructors of `Msg`
    --> src/Main.ipe:41:9
     |
  41 |         _ -> ( model, Cmd.none )
     |         ^ absorbs: Decrement, Reset
     |
  help: `Msg` is a closed type — handle each constructor so a future
        variant is caught here instead of silently falling through
  suggestion: replace `_` with one arm per absorbed constructor
        (machine-applicable — `ipe fix` can do this)
  note: to keep this case deliberately open, write
        `-- @allow(open-case) <reason>` on the arm
  ```

- **Explain page** (`explain/IPE-T0018.md`), per the compiler-as-kind-teacher
  standard — progressive, ELI10 first:
  1. *The moment it protects you from*: add `Reset` to `Msg`, the app builds,
     the button does nothing — walk the silent-fall-through story.
  2. *The rule*: catch-alls are refused where the compiler knows the full
     list; shown with a 3-variant before/after.
  3. *The easy fix*: the expansion (each new arm gets the old catch-all's
     body — behavior identical today, a compile error the day the type
     grows).
  4. *When a catch-all is right*: big stable enums, genuinely-default
     semantics — the `@allow(open-case) reason` directive, with a good and a
     bad reason example.
  5. *Deep end*: what "closed" means (declaration-site constructor set), why
     `Int`/`String` arms still need `_`, top-level-column scoping and the
     nested-position lint, the IPE-T0011 relationship (absorbed = 0).
- **IPE-T0010 page correction (same change)**: its "Ipê has no catch-all `_`
  pattern" paragraph is today false and after this change *almost* true —
  rewrite to state the real rule (no catch-all over closed unions; `_`
  required/allowed over open domains) and cross-link IPE-T0018.

## 7. LSP pairing — the ergonomic counterweight

Two code actions, both through `ipe-lsp.md`'s G2 `VerifiedEdit` gate (an
offered edit that fails the parse→canon→type round-trip is unrepresentable):

**(1) "Replace catch-all with the hidden arms"** — on IPE-T0018.
Rewrites the catch-all arm into one arm per absorbed constructor, each with
the **catch-all arm's own body**:

```elm
_ -> ( model, Cmd.none )
-- becomes
Decrement -> ( model, Cmd.none )
Reset     -> ( model, Cmd.none )
```

- Semantics-preserving by construction (the same body runs for the same
  values), so the CLI `Suggestion` is `Applicability::MachineApplicable` —
  `ipe fix` / `ipe lint --fix`-grade confidence.
- Payload-carrying constructors get wildcard sub-patterns (`SetColor _ ->`) —
  top-level explicitness without forcing nested enumeration (§3.3).
- A bare-variable catch-all (`other -> reportUnknown other`) expands via the
  supported `as`-alias pattern: `(SetColor _) as other -> reportUnknown other`
  — the binding survives, still machine-applicable.
- Arms are emitted in declaration order, indentation copied from the replaced
  arm, and the result must be `ipe fmt`-idempotent (G2's fmt-clean clause).

**(2) "Add missing arms"** — on IPE-T0010 (no catch-all present).
Inserts one arm per missing witness (the diagnostic already carries them) with
placeholder bodies as LSP snippet tabstops — `HasPlaceholders`, never
auto-applied by `ipe fix`, offered interactively in the editor. This is the
generator `ipe-lsp.md` Q3(b) already names ("Add missing arm(s)"); this spec
pins its input to the witness list so the checker and the action cannot
disagree about what is missing.

A third quick-fix on IPE-T0018 offers the escape: *"Keep this case open
(requires reason)"* — inserts `-- @allow(open-case) <tabstop>` above the arm
(`HasPlaceholders`).

## 8. Implementation plan

Where it lives: `sky_types::exhaust`, extending `check_case`. Ordered phases,
each green before the next:

- **Phase 1 — plumb the scrutinee type.** Move the `exhaust::check` call in
  `sky_types::infer` to after region read-back and pass it the regions map
  (or a `ty_of(home, span)` lookup) + each def's home path. Today the pass
  judges arms without the scrutinee's type; the all-catch-all `case` (`case x
  of _ -> e`) is undecidable without it. No behavior change in this phase —
  pure plumbing, existing goldens byte-identical.
  *Gate:* full golden suite unchanged; a debug assertion that every analysed
  `case`'s scrutinee span resolves to a type.
- **Phase 2 — the directive carrier.** The lexer trivia table for
  `-- @allow(<id>) <reason>` (shared deliverable with the lint tool — build
  once, in whichever design lands first), carried on the parse output through
  canon to the exhaust pass.
  *Gate:* directive-parsing unit tests incl. malformed-directive rejection.
- **Phase 3 — the rule + diagnostic.** `Head`-matrix computation of absorbed
  constructors (reuse `useful` on the matrix minus the catch-all row);
  `TypeError::CatchAllOverClosedUnion`; IPE-T0018 code + title + explain page
  + `ALL_CODES` count; IPE-T0010 page correction; `@allow(open-case)`
  suppression honoring Phase 2's table; the `Suggestion` (expansion text) on
  the diagnostic.
  *Gate:* fixture matrix — {user ADT, Maybe, Result} × {`_`, bare var,
  alias-of-var} × {absorbed 0 (→T0011), 1, many} × {allowed, not-allowed}
  × {Bool/List/Int scrutinee → no finding}; goldens for message rendering.
- **Phase 4 — corpus migration (the honest cost).** Run the compiler over
  `examples/` + `src/stdlib` + `tests/golden/`; every IPE-T0018 is
  resolved by expansion (the machine-applicable fix), by a reasoned
  `@allow(open-case)` (expected: `Ipe.Money` currency dispatch, decoder
  fallbacks), or — where a golden deliberately tests catch-all lowering — a
  golden updated to a legal shape. **The enforcement commit and the migration
  land together**; the sweep must be green in the same change (§0: no
  skipped examples, no weakened gate).
  *Gate:* full examples sweep + golden suite + `IPE_E2E=1` seal run.
- **Phase 5 — LSP actions.** §7's three actions in `sky_lsp` (lands with the
  LSP plan's Phase 3; the `ipe fix` CLI path ships in Phase 3 above, so the
  ergonomic counterweight exists from the first enforcing release even
  without an editor).
  *Gate:* G2 round-trip tests per action; fmt-idempotence assertion.

Sequencing note: or-patterns (divergences-from-sky §6.3) SHOULD land before
or alongside Phase 4 — grouped-handling sites migrate to `A | B -> e` instead
of duplicated arms, and the migration diff shrinks accordingly. Not a hard
dependency (the expansion fix works without them), but the ergonomic delta is
large.

## 9. Boundary with the lint tool (coherence contract)

One sentence each way: **anything that changes what the compiler accepts
lives in `sky_types` (this rule, IPE-T0018); anything advisory lives in
`sky_lint`** (the nested-position variant of this same analysis,
`wildcard-absorbs-variants`, default `warn`). Both consume the same witness
machinery, the same directive table, the same `Suggestion` model, and the
same LSP gate — a finding can move between the two tiers by policy without
rewriting its analysis.

## 10. Open decisions (need a user call before implementation)

1. **Confirm the recommendation** (adopt-with-opt-out vs strict vs
   lint-only). Everything in §8 assumes opt-out.
2. **Directive id**: `open-case` (proposed) vs the raw code
   (`@allow(IPE-T0018)`) — proposal: the name; codes stay renderable but
   names are the human surface, matching the lint tool's convention.
3. **`Maybe`/`Result` inclusion**: proposed **in** (they are closed unions
   and two-variant enumeration costs one short line), but they are the
   highest-frequency migration sites — a carve-out would shrink Phase 4
   substantially at the cost of rule uniformity.
4. **Timing vs the C.1 rename and the examples-sweep freeze**: Phase 4
   touches many examples; it should not overlap another whole-corpus
   campaign.
