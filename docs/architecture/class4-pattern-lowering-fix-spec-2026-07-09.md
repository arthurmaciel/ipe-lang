# Class 4 fix spec — pattern & lowering completeness bugs

> Per `docs/architecture/campaign-classification-2026-07-09.md`, Class 4:
> SKY-I0001 interpolation ICE, #90 (SKY-L0114 ctor-payload-function), #158
> (SKY-L0112/SKY-L0116 nested-constructor-payload patterns), #102 (local-type
> shadow gives wrong diagnostic), #32 (Task arity ICE + Task-in-ADT-ctor gate).
> Classified MECHANICAL-but-diagnosis-first — every root cause below was
> confirmed by reading the actual code path AND by building `skyc` at HEAD and
> reproducing the failure with a fresh `.sky` fixture (not restated from the
> backlog's one-line summary).

## Sequencing — this class runs AFTER Class 1

**Do not start any item in this spec before
`docs/architecture/class1-inference-fix-spec-2026-07-09.md` lands and its test
matrix is green.** The two classes share files:

- `crates/sky_types/src/constrain.rs` — Class 1 touches
  `constrain_var_top_level` (~2015) and inserts a new solve-phase pass in
  `crates/sky_types/src/lib.rs`'s `infer_with_budget_attributed`. **Item D**
  below (`#102`) touches `canonicalise_with_env`/`inject_dep_type` in
  `crates/sky_canon/src/resolve.rs` — a DIFFERENT file, no overlap. **Item E**
  below (`#32`) touches `normalize_annotation_ty` (~1970-2038) in the SAME
  `constrain.rs` file as Class 1, but a different function with no shared
  lines — low collision risk, but re-diff after Class 1 lands since Class 1
  adds a new pass to `lib.rs` that could shift line numbers Item E cites.
- `crates/sky_lower/src/lower.rs` — Class 1's lower-phase change is scoped to
  the **untyped-binding arm** (`~3707-3786`, `split_unannotated_sig`
  `~3873-3898`). **Item B** below (`#90`) touches `lower_enum` (~3290-3336)
  and `reject_function_through_type_var`/`embeds_nonderivable_function`
  (~152-245, ~5536-5551). **Item C** below (`#158`) touches
  `lower_payload_pat`/`lower_destructure_pat`/`lower_arm_pat`/
  `lower_list_arm_pat` (~9213-9951) and `constrain_pattern`'s `PCtor` arm in
  `constrain.rs` (~2719-2766). **Item E** touches `ir_type_from_canon`'s
  `"Task"` arm (~4095-4175). None of these regions overlap Class 1's
  untyped-binding arm, but all four items in THIS spec share `lower.rs` with
  EACH OTHER — implement B, C, and E as three separate patches with a rebase
  check between each, not one giant diff.
- **Diagnostic code allocation** (`crates/sky_diagnostics/src/code.rs`): only
  Item E allocates new codes. Re-check the free numbers at implementation
  time — Class 1 does not allocate codes, but other in-flight work might.

Line numbers below are read against the current HEAD (branch `master`,
verified 2026-07-09 by building `skyc` and reproducing every repro live).
Re-verify anchors before editing if HEAD has moved.

---

## Item A — SKY-I0001 interpolation ICE: numeric case fixed, literal-argument class still open

### Status

**Partially fixed.** The exact repro named in the backlog
(`msg = """count={{String.fromInt 54}}"""`) is **already fixed** on master by
commit `65b9ce9` ("fix(canon): recognise numeric literals in `{{...}}`
interpolation args"), with a regression test
(`crates/skyc/tests/interp_literal.rs`), a golden fixture
(`tests/golden/m_interp_int_literal/`), and a `docs/divergences-from-sky.md`
entry. The `/tmp/sky-fuzz/FAILURES/seed-1016-*/` repro directory named in the
backlog no longer exists (confirmed — the fix already landed and consumed
that finding).

**However, the fix only closes the NUMERIC-literal sub-case.** The same root
cause — a bare-identifier fallback in `resolve_simple_interp_ref` leaking an
unresolved `VarLocal` past canonicalisation — still fires for **string**,
**boolean**, and (by the same reasoning) **character** literal arguments to a
qualified call inside `{{...}}`. Verified live against HEAD by building
`skyc` (`cargo build -p skyc --bin skyc`, binary at
`~/.cache/sky-rust-target/debug/skyc`) and compiling three fresh repros:

**Repro 1 — string literal argument (ICEs today):**

```sky
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

msg = """greeting={{String.toUpper "hi"}}"""

main =
    println msg
```

```
skyc: internal compiler error[SKY-I0001]: internal compiler error
  = note: unbound local `"hi"`
```

**Repro 2 — bool literal argument (ICEs today):**

```sky
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

msg = """flag={{Basics.toString True}}"""

main =
    println msg
```

```
skyc: internal compiler error[SKY-I0001]: internal compiler error
  = note: unbound local `True`
```

**Repro 3 — string literal containing a `.` (ICEs with an even more
confusing message, because the arg text is ALSO mis-split by the `.`-based
qualified-name detector before it ever reaches the bare-identifier
fallback):**

```sky
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

msg = """path={{String.length "a.b"}}"""

main =
    println msg
```

```
skyc: internal compiler error[SKY-I0001]: internal compiler error
  = note: unbound local `"a`
```

(Repro 3 confirms the fix must run BEFORE the `.`-split, exactly like the
already-landed numeric case — see the existing doc comment at
`crates/sky_canon/src/resolve.rs:3132-3133`: "This must precede the `.`-split
below, else a float like `1.5` is mis-parsed as `Access(1, 5)`." The same
sentence applies verbatim to a quoted string containing a literal `.`.)

### Root cause

`crates/sky_canon/src/resolve.rs`, `resolve_simple_interp_ref` (currently
~3118-3212). The function recognizes exactly ONE literal shape before
falling through to identifier/access resolution: a leading-ASCII-digit token
(lines ~3139-3152, the `65b9ce9` fix). Every OTHER literal shape a Sky
argument expression can be —

- a double-quoted string `"..."` (Sky `String`),
- the two capitalized nullary constructors `True` / `False` (Sky `Bool`),
- a single-quoted character `'x'` (Sky `Char`),

— has no recognizer. Each currently reaches one of two wrong branches:

1. If the literal contains no `.`, it falls to the final "bare identifier"
   branch (~3191-3211): `interner.intern(s)` interns the STRING INCLUDING ITS
   QUOTES/CAPITALIZATION as if it were a variable name, finds nothing in
   `env.vars`/`env.wildcard_vars`, and falls through to
   `canon::Expr_::VarLocal(sym)` — a genuinely unbound local that leaks past
   canonicalisation and trips the exact same `constrain` invariant the
   `65b9ce9` commit message describes ("the resolver is supposed to have
   resolved every local"), surfacing as the generic `SKY-I0001` ICE fallback
   (`diagnostic.rs:1025`, `_ => SKY_I0001`).
2. If the literal happens to contain a `.` (a string with a period inside
   it, e.g. `"a.b"`, or — theoretically — a char literal can't but a string
   commonly will), it is caught by the EARLIER `.`-split branch (~3153-3190)
   and mis-parsed as either a qualified reference (if the char before the
   first `.` is uppercase) or a `record.field` access — producing an even
   less diagnosable ICE (repro 3's `unbound local "a`` instead of `unbound
   local "hi"``).

### Fix

Extend the SAME "recognise the literal before the identifier/`.`-split
logic" strategy the `65b9ce9` commit already established, in the SAME
function, immediately after the existing digit-check block
(after line ~3152, still before the `.`-split at line ~3153):

```rust
// String literal `"..."`. A body that starts with `"` can never be a bare
// identifier or a `Module.member`/`record.field` access (Sky identifiers
// never start with `"`), so treat the WHOLE remaining text as a String
// literal if it is a well-formed quoted string (starts AND ends with `"`,
// with at least the two quote characters). This must precede the
// `.`-split below for the same reason the numeric-literal check does: a
// string containing a literal `.` (`"a.b"`) would otherwise be mis-split
// into a bogus qualified-name/record-access lookup (see repro 3 in
// class4-pattern-lowering-fix-spec-2026-07-09.md).
if s.starts_with('"') {
    if s.len() >= 2 && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        // Un-escape the same two escapes the lexer's raw-string reader
        // accepts inside a double-quoted literal: `\"` and `\\`. This is
        // deliberately narrower than the full string-literal escape grammar
        // (no `\n`/`\t`/unicode escapes) because interpolation args are a
        // best-effort convenience surface, not a second string parser; an
        // arg needing richer escaping should be a `let`-bound variable
        // instead (already-supported path).
        let unescaped = inner.replace("\\\"", "\"").replace("\\\\", "\\");
        return Ok(Located::new(span, canon::Expr_::Str(unescaped)));
    }
    // Unterminated/degenerate quote — literal `{{...}}` fallback rather
    // than a VarLocal that would ICE.
    return Ok(Located::new(span, canon::Expr_::Str(format!("{{{{{s}}}}}"))));
}
// Bool literal. `True`/`False` are the Prelude-exposed nullary
// constructors of `Bool` (Std.Basics), never ordinary Sky identifiers a
// user could shadow with a differently-typed local in this position
// (SKY-N0026-class reservation) — recognise them directly rather than
// falling through to the bare-identifier lookup, which finds them in
// `env.ctors`/`qual_ctors` (a different map than `env.vars`) and would
// otherwise emit an unbound `VarLocal`.
if s == "True" {
    return Ok(Located::new(span, canon::Expr_::Bool(true)));
}
if s == "False" {
    return Ok(Located::new(span, canon::Expr_::Bool(false)));
}
// Char literal `'x'`. Mirrors the string-literal case above; a body
// starting with `'` can never be an identifier.
if s.starts_with('\'') {
    if s.len() >= 3 && s.ends_with('\'') {
        let inner = &s[1..s.len() - 1];
        return Ok(Located::new(span, canon::Expr_::Char(inner.to_owned())));
    }
    return Ok(Located::new(span, canon::Expr_::Str(format!("{{{{{s}}}}}"))));
}
```

Confirm `canon::Expr_::Bool` and `canon::Expr_::Char` are the correct
variant shapes (`Char` already used elsewhere in this same file for pattern
literals — grep `canon::Expr_::Char` to confirm the carried-string
convention matches `Pat::Char`'s "single grapheme" invariant documented at
`sky_ir::ir.rs:1319-1321`).

Also extend `resolve_interp_ref`'s space-split branch (~3100-3113): it
currently splits on the FIRST space (`s.find(' ')`) to detect `func arg`.
A string-literal arg containing a space (`{{String.length "a b"}}`) will
split at the space INSIDE the quotes, producing `func_str = "String.length"`,
`arg_str = "a b"` (missing its closing structure) rather than the intended
whole-string arg `"a b"`. This is a narrower, secondary bug in the SAME
family — worth a fixed regression once the primary literal-recognition fix
lands, but out of THIS item's minimum scope (flag it in the new test file
below as a `#[ignore]`d known-gap test with a comment, not silently dropped,
per CLAUDE.md's no-deferral rule — "known broken edge case... is a reason to
start").

### Verification

Before the fix: all three repros above ICE with `SKY-I0001`.

After the fix: all three compile and print the expected text:

- Repro 1 → `greeting=HI`
- Repro 2 → `flag=True`
- Repro 3 → `path=3`

### Regression tests

Extend `crates/skyc/tests/interp_literal.rs` (the existing home of the
`65b9ce9` regression) with three new golden fixtures, mirroring its existing
`tests/golden/m_interp_int_literal/` shape:

- `tests/golden/m_interp_string_literal/Main.sky` (repro 1 shape) — pure-skyc
  compile assertion always-on, `SKY_E2E`-gated build/run asserting the
  printed string.
- `tests/golden/m_interp_bool_literal/Main.sky` (repro 2 shape).
- `tests/golden/m_interp_dotted_string_literal/Main.sky` (repro 3 shape) —
  this one specifically pins the "precedes the `.`-split" ordering
  requirement; a regression here would silently reintroduce the exact class
  of bug `65b9ce9` closed for numerics.
- One `#[ignore]` test documenting the space-inside-string-arg gap
  (`{{String.length "a b"}}`), with a doc comment pointing at this spec, so
  it enters the pipeline as a filed-not-forgotten item per CLAUDE.md §4.
- Add a `docs/divergences-from-sky.md` entry mirroring the existing
  `65b9ce9` one (the reference `../sky` compiler also lacks string/bool/char
  literal recognition in `resolveInterpolationRef` and would ICE-equivalent
  on the same input — recognising these literals is strictly better, not a
  regression risk).

---

## Item B — #90: SKY-L0114 blocks legitimate function-payload constructors

### Status

Confirmed live against HEAD: BOTH of the two distinct sub-shapes the backlog
folds under `#90` currently reject with `SKY-L0114`, via **two different,
independent gates** in `crates/sky_lower/src/lower.rs`.

**Repro 1 — `Result.andMap`/`Maybe.andMap` (region-based gate,
`reject_function_through_type_var`):**

```sky
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Sky.Core.Result as Result
import Std.Log exposing (println)

add : Int -> Int -> Int
add a b = a + b

main =
    let
        r = Result.andMap (Ok 2) (Result.andMap (Ok 3) (Ok add))
    in
    case r of
        Ok n -> println (String.fromInt n)
        Err _ -> println "err"
```

```
skyc: error[SKY-L0114]: function value in a constructor payload not supported yet
  --> Main.sky:11:34
   |
11 |         r = Result.andMap (Ok 2) (Result.andMap (Ok 3) (Ok add))
   |                                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

**Repro 2 — `Sky.Test`'s own `Leaf String (() -> TestResult)` shape
(declaration-time gate, `lower_enum` Gate 2) — ex00's actual current first
blocker:**

```sky
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

type Thunked
    = Leaf String (() -> Int)

runIt : Thunked -> Int
runIt t =
    case t of
        Leaf _ f -> f ()

main =
    println (String.fromInt (runIt (Leaf "x" (\_ -> 42))))
```

```
skyc: error[SKY-L0114]: function value in a constructor payload not supported yet
 --> Main.sky:6:7
  |
6 |     = Leaf String (() -> Int)
  |       ^^^^ storing a function value in a constructor payload is not supported yet
```

### Root cause

Both gates were written under the assumption that ANY user-enum ctor payload
reaching a function value would produce Rust the backend cannot emit — true
when the whole-enum `#[derive(Clone, Debug, PartialEq)]` set is
**unconditional**. That assumption is now **stale**: the backend grew a
whole-program derivability fixpoint (`#87` seal,
`crates/sky_backend_rust/src/lib.rs:405-445` `enum_derivable` /
`ir_type_is_derivable` in `crates/sky_ir/src/ir.rs:817-892`) that gracefully
degrades a non-derivable enum's emission — `crates/sky_backend_rust/src/emit_types.rs:410-441`
(`self_derivable = ctx.enum_is_derivable(...)`; when `false`, NO
`#[derive(...)]` line is emitted at all, and `SkyStringify`'s hand-written
impl renders `"<fn>"` for the non-derivable field instead of calling
`.dispatch()`, at `emit_types.rs:509-518`). This exact machinery is what
already lets `Std.Cmd.RetryPolicy` (a record with a `shouldRetry : Error ->
Bool` function field) reach the backend today — it is the established,
tested precedent this fix generalises.

Verified the SAME machinery already correctly recognises a function value
embedded in `Maybe`/`Result` as non-derivable
(`crates/sky_ir/src/ir.rs:877-882`, `IrType::Maybe(e)`/`IrType::Result(a,b)`
recurse into their element types) — so the ONLY thing standing between
"legal, buildable Rust" and "clean rejection" for both repros is the
LOWERER's fail-closed gates never letting the program reach that machinery.

**Construction-site codegen is ALSO already sound for both repros** (verified
by reading, not assumed): every `Expr::Lambda` literal ALWAYS renders as
`Box::new(move |...| ...)` (`crates/sky_backend_rust/src/emit_expr.rs:6573-6584`,
`emit_lambda`), and every bare reference to a top-level function used as a
first-class value renders through `Expr::FuncValue`/`emit_func_value`
(`emit_expr.rs:6494-6520`), which boxes/arcs it according to the target
type. `emit_ctor` (`emit_expr.rs:5401-5456`) just calls `emit_expr_at` on
each constructor argument and uses the result as-is (only wrapping in
`Box::new` for a cyclic self-edge field, unrelated to this fix) — so once
the LOWERER stops rejecting these two shapes, the already-boxed argument
expressions slot straight into the `Box<dyn Fn(...) -> R + Send + 'static>`
field type `IrType::Fun` already renders to
(`crates/sky_backend_rust/src/emit_types.rs:249-263`). No backend change is
needed for Item B (unlike Item C's list/cons sub-case below).

**Two independent gate sites, two independent fixes:**

#### B1 — `is_opaque_boxed_wrapper` exemption list (fixes Repro 1)

`crates/sky_lower/src/lower.rs:152-157`:

```rust
fn is_opaque_boxed_wrapper(interner: &Interner, name: Symbol) -> bool {
    matches!(
        interner.resolve(name),
        Some("Decoder" | "Task" | "Cmd" | "Sub")
    )
}
```

`Maybe`/`Result` are NOT in this list even though they share the exact same
property the doc comment above the function requires: they map to
hand-written runtime generic types (`sky_runtime::core::SkyMaybe<T>` /
`SkyResult<E, A>`) whose derives are PER-TRAIT-CONDITIONAL Rust derive
macros (`impl<T: Clone> Clone for SkyMaybe<T>`, etc.) — a
`SkyMaybe<Box<dyn Fn(...)->...>>` simply lacks those trait impls for that
one instantiation; it is not a compile error unless something actually
invokes `.clone()`/`{:?}`/`==`/serde on that SPECIFIC value. The existing
`clone_class` analysis (`lower.rs:360-420`, `AUD-04`) already classifies an
`IrType::Maybe`/`IrType::Result` wrapping a `Fun` as `CloneClass::NonClone`
(composite: `CloneOk` iff every component is), so the closure-capture
clone-insertion pass will not blindly emit a `.clone()` that fails — it
already fails closed with its own `SKY-L0125` diagnostic instead, on the
rare path where that would matter. Model-field usage (a Sky.Live
`Model` containing a `Maybe (Int -> Int)` field) is independently and
correctly still rejected by the SEPARATE `#91` Model-gate
(`crates/sky_backend_rust/src/emit_model_gate.rs`, `ir_type_is_serde`/
`ir_type_is_derivable`), because the Model's carrier there is a `Ty::Record`,
not the exempted `Ty::Con` head — `embeds_nonderivable_function`'s Record
branch (`lower.rs:205-222`) uses `ty_contains_fun`, which has NO exemption
logic and still fires regardless of this change. So this fix is narrowly
scoped and does not open a soundness hole.

**Fix:**

```rust
fn is_opaque_boxed_wrapper(interner: &Interner, name: Symbol) -> bool {
    matches!(
        interner.resolve(name),
        Some("Decoder" | "Task" | "Cmd" | "Sub" | "Maybe" | "Result")
    )
}
```

Update the doc comment above the function (currently ~123-151) to add
`Maybe`/`Result` to the "matches these opaque wrappers" list and explain
why they qualify (generic runtime type with per-trait-conditional derives,
not a per-instantiation synthesised concrete enum).

#### B2 — `lower_enum` Gate 2 removal (fixes Repro 2)

`crates/sky_lower/src/lower.rs:3290-3336`, `lower_enum`. Gate 2
(~3311-3318):

```rust
let ir = self.ir_type_from_canon(arg, &type_params)?;
// Gate 2: a function-bearing payload field cannot satisfy the
// enum's derives. The carrier is a constructor payload, so blame
// the constructor declaration with the payload-specific message
// (SKY-L0114) rather than the record-field one.
if ir_contains_fun(&ir) {
    return Err(unsupported(ctor.span, Feature::CtorPayloadFunction));
}
fields.push(ir);
```

This gate is COMPLETELY DISJOINT from the region-based gate B1 targets: it
fires at DECLARATION time (`lower_enum`, before any use site is visited) for
a constructor field whose CANONICAL declared type is directly function-typed
— `IrType::Generic(_)` (a genuinely polymorphic field, `type Box a = Mk a`)
returns `false` from `ir_contains_fun` (confirmed at
`lower.rs:283-337`, the `IrType::Generic(_) => false` arm), so Gate 2 NEVER
fires for the "through a type variable" shape the sibling M3a golden test
(`crates/skyc/tests/golden_m3a_function_payload_gate.rs`,
`tests/golden/m3a_function_payload_gate/Main.sky`) exercises — that test
goes through the OTHER gate (`reject_function_through_type_var`, still
unchanged by this fix, since `Box` is a genuine user enum not in
`is_opaque_boxed_wrapper`). **Removing Gate 2 therefore does not affect the
M3a test at all** — verified by inspection of `ir_contains_fun`'s `Generic`
arm and by the fact that the M3a golden fixture's `Box a = Mk a` is
polymorphic, never hitting `lower_enum`'s Gate 2 in the first place.

**Fix:** delete Gate 2 (the `if ir_contains_fun(&ir) { ... }` block,
~3311-3318) entirely. Rely on the backend's already-existing
`enum_derivable` fixpoint + graceful degradation to accept the program.
Update the function's doc comment (currently ~3273-3289, which explicitly
lists "a field whose type embeds a function... the constructor-payload-
function gap" as one of the "two fail-closed gates") to remove that bullet
and note the historical gate + why it was removed (superseded by the #87
backend seal — link to this spec).

`Feature::CtorPayloadFunction` / `SKY-L0114` itself MUST stay (it is still
the correct diagnostic for the case B1/B2 do NOT cover: a function value
laundered through a type variable into an ordinary, non-exempted user enum —
exactly what the M3a golden test pins). Do not delete the code or the
`Feature` variant; only the DECLARATION-time Gate 2 check in `lower_enum`
goes away.

### Verification

Before the fix: both repros above reject with `SKY-L0114`.

After the fix: both compile and produce the semantically correct output:

- Repro 1 → prints `7` (`add 3 2`).
- Repro 2 → prints `42`.

Also confirm (regression, not a new requirement) that the M3a golden test
(`type Box a = Mk a` applied at `Mk (\n -> n + 1)`) is UNCHANGED —
still rejects with `SKY-L0114` via the region-based gate, since B1/B2 do not
touch `Box`'s path (`Box` is not in `is_opaque_boxed_wrapper`, and its
declaration never reaches Gate 2 because its field is `Generic`, not `Fun`).

### Regression tests

- `tests/golden/m90_result_andmap_function_payload/Main.sky` (Repro 1
  shape) + a `crates/skyc/tests/golden_m90_ctor_payload_function.rs` test
  binary asserting `built.is_ok()` and, under `SKY_E2E`, that the emitted
  binary prints `7`.
- `tests/golden/m90_concrete_ctor_function_field/Main.sky` (Repro 2 shape,
  mirroring `Sky.Test`'s `Leaf` exactly) — same harness, asserts prints `42`.
- A direct `Sky.Test` end-to-end smoke: build `examples/00-standard-libs`
  (or whichever fixture is the ex00 stand-in in this repo) and confirm the
  `SKY-L0114` failure at `Sky.Test:32` is gone and the NEXT blocker (if any)
  in the onion-peeling chain is a DIFFERENT, unrelated diagnostic — update
  `BACKLOG.md`'s sweep-front section with whatever that
  next blocker turns out to be, per the "onion-peeling" convention already
  used there.
- Unit test on `is_opaque_boxed_wrapper` directly (if not already covered by
  `function_inside_opaque_boxed_wrapper_is_accepted` in
  `crates/sky_lower/tests/unsupported.rs:587-640`): add a sibling test using
  `Maybe`/`Result` as the wrapper name, same shape as the existing
  `Decoder` case.
- Regression proving Gate 2's removal does not silently accept a
  non-derivable enum used somewhere the backend genuinely cannot degrade
  (e.g. a Sky.Live `Model` type ALIAS directly being the function-carrying
  enum, not just a field of it) — add a golden fixture setting a
  `Live.app`'s `model` field type to a `Test`-shaped enum with a function
  payload and confirm the EXISTING `#91` Model-gate still rejects it (with
  ITS OWN diagnostic, not a cargo failure) — this is the safety-net
  cross-check for B2, not a new gate.

---

## Item C — #158: nested-constructor-payload function-argument / record / cons patterns

### Status

Confirmed live against HEAD — two independent, currently-rejected shapes,
BOTH raised from the SAME function, `lower_payload_pat`
(`crates/sky_lower/src/lower.rs:9218-9274`).

**Repro 1 — cons/list sub-pattern nested in a ctor payload (`SKY-L0116`):**

```sky
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

f : Maybe (List Int) -> Int
f m =
    case m of
        Just (h :: t) -> h
        _ -> 0

main =
    println (String.fromInt (f (Just [1, 2, 3])))
```

```
skyc: error[SKY-L0116]: refutable pattern-discrimination shape not supported yet
 --> Main.sky:8:14
  |
8 |         Just (h :: t) -> h
  |              ^ this refutable pattern-discrimination shape is not
  |                supported yet — discriminating with cons / list patterns
  |                or guarded arms needs machinery that is not in place yet
```

**Repro 2 — record sub-pattern nested in a ctor payload (`SKY-L0112`):**

```sky
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Std.Log exposing (println)

type alias Person = { name : String, age : Int }

f : Result String Person -> String
f r =
    case r of
        Ok { name } -> name
        Err _ -> "none"

main =
    println (f (Ok { name = "Ada", age = 30 }))
```

```
skyc: error[SKY-L0112]: nested constructor payload patterns not supported yet
  --> Main.sky:10:12
   |
10 |         Ok { name } -> name
   |            ^^^^^^^^ a record pattern is supported at a `case` scrutinee
   |                      or a `let` destructure, but not yet nested inside a
   |                      constructor payload or a tuple element — that needs
   |                      the carrier's record type threaded to the lowerer
```

(The error message for Repro 2 already names the exact fix — "that needs the
carrier's record type threaded to the lowerer" — confirming the diagnosis
below independently.)

### Root cause (shared plumbing gap)

`lower_payload_pat` (and its sibling `lower_destructure_pat`,
`lower.rs:9283-9320`) is a **plain, non-`&self`, static `fn`** — it has NO
access to the type-solver's per-span region map
(`Lowerer::region_ty`, `lower.rs:3061-3064`, `self.types.regions.get(&(home,
span))`). The TOP-LEVEL analogue,
`lower_binder_pat` (`lower.rs:9330-9353`), IS an instance method and DOES
consult `self.region_ty(value.span)` to recover a record scrutinee's
complete solved field set before delegating to `lower_record_pat`
(`lower.rs:9458+`) — that is exactly why `case r of { name } -> ...`
(a top-level record binder) already works, while `case r of Ok { name } ->
...` (the SAME record pattern one level deeper, inside a `Ok` payload) does
not: the NESTED position has no `value` expression to read a region from,
and `lower_payload_pat` has no `&self` to look one up even if it did.

**The missing region entry does not exist yet either.** Region annotations
are inserted at `crates/sky_types/src/constrain.rs:2550`
(`constrain_expr`, for every EXPRESSION) and, as a targeted precedent, at
`constrain.rs:2570-2576` inside `constrain_lambda` (every LAMBDA PARAMETER
pattern's span is explicitly recorded — "so the lowerer can source a
record-param's complete field set from its solved type"). **No equivalent
insertion exists for a `case`-arm constructor pattern's SUB-PATTERNS.**
`constrain_pattern`'s `PCtor` arm (`constrain.rs:2719-2766`) DOES compute a
fresh, correctly-instantiated `VarId` per sub-pattern (`arg_vars`, from
`instantiate_ctor`, at line 2744) and recursively constrains each
sub-pattern against it (line 2746-2748) — the SOLVED TYPE for a nested
sub-pattern's position is available at solve time, it is simply never
persisted into `self.regions` the way lambda params are.

For the **cons/list sub-case (Repro 1)** there is a SECOND, independent
obstacle even once region-threading exists: `IrType::List(elem)` renders to
Rust's `Vec<T>` (`crates/sky_backend_rust/src/emit_types.rs`), and Rust
cannot slice-pattern-match a raw `Vec<T>` FIELD inline inside a constructor
tuple-pattern (`MyEnum::Ctor([h, t @ ..])` is not valid Rust against a
`Vec<T>` field — only against an actual slice/array, which requires an
explicit `.as_slice()` coercion APPLIED TO THE SCRUTINEE, the mechanism the
existing TOP-LEVEL list-`case` path already uses via `ScrutMode::Whole {
list_mode: true, .. }` in `emit_match_scrutinee`). Verified: `render_pat`
(`crates/sky_backend_rust/src/emit_expr.rs:5936-6019`) DOES already render a
`Pat::Slice` generically wherever it is invoked, INCLUDING recursively
inside `Pat::Ctor`'s args (`emit_expr.rs:5994-5997` calls `render_pat` on
every ctor arg with no special-casing) — meaning if the lowerer were simply
patched to emit `Pat::Ctor{ args: [Pat::Slice{..}] }` without ALSO adding a
per-position slice coercion in the backend, the emitted Rust would be a
compile error (`expected struct Vec, found array pattern` class), not a
clean build. This is why Repro 1's fix (below) needs a backend change, while
Repro 2's fix does not.

### Fix — C1: record sub-pattern nested in a ctor payload (Repro 2)

**Step 1 — thread a region for every constructor sub-pattern.**
`crates/sky_types/src/constrain.rs`, `constrain_pattern`'s `PCtor` arm,
inside the loop at lines 2746-2748:

```rust
for (sub, av) in args.iter().zip(arg_vars) {
    self.constrain_pattern(local, sub, av)?;
}
```

change to:

```rust
for (sub, av) in args.iter().zip(arg_vars) {
    self.constrain_pattern(local, sub, av)?;
    // Record this sub-pattern's own instantiated field type so the
    // lowerer can recover a NESTED record/list sub-pattern's complete
    // shape the same way a top-level `case`/`let` binder already does
    // (see the identical precedent in constrain_lambda, ~2574-2575).
    // Class 4 item C (docs/architecture/class4-pattern-lowering-fix-spec-2026-07-09.md).
    self.regions.insert((self.current_home.clone(), sub.span), av);
}
```

Also add the identical insertion to the `PTuple` sub-pattern loop a few
lines below (~2779+ — "constrain each sub-pattern against its element's
variable") so a record nested inside a TUPLE element (`(Ok {name}, y)`)
gets the same treatment for free, since Item C's fix in the lowerer (next
step) will handle `PTuple` and `PCtor` uniformly.

**Step 2 — make the payload-pattern lowerers instance methods and consult
the new region.** `crates/sky_lower/src/lower.rs`:

- Change `fn lower_payload_pat(p: &canon::Pattern) -> DResult<Pat>`
  (~9218) to `fn lower_payload_pat(&self, p: &canon::Pattern) -> DResult<Pat>`.
- Change `fn lower_destructure_pat(p: &canon::Pattern) -> DResult<Pat>`
  (~9283) to take `&self` likewise.
- Change `fn lower_arm_pat(p: &canon::Pattern) -> DResult<Pat>` (~9831,
  the entry point for `case`-arm heads) to take `&self` — it is the
  top-level caller of `lower_payload_pat` for a `PCtor` arm's args
  (~9850-9853).
- Change `fn lower_list_arm_pat(p: &canon::Pattern) -> DResult<Pat>`
  (~9893) to take `&self` (calls `lower_payload_pat` for its element/head
  sub-patterns, ~9901/9906).
- Update every `Self::lower_payload_pat(...)` / `Self::lower_arm_pat(...)` /
  `Self::lower_list_arm_pat(...)` / `Self::lower_destructure_pat(...)` call
  site (both the recursive ones inside these functions and the external
  callers) to `self.lower_payload_pat(...)` etc. Grep
  `Self::lower_(payload_pat|destructure_pat|arm_pat|list_arm_pat)` to find
  every site; there are roughly 10 call sites total per the earlier `rg`
  survey of this function family.
- In `lower_payload_pat`'s `PRecord` arm (currently ~9267, unconditionally
  `Err(unsupported(p.span, Feature::NestedPayloadPatterns))`), replace with:

  ```rust
  canon::Pattern_::PRecord(fields) => {
      let ty = self.region_ty(p.span).ok_or_else(|| {
          bug(
              "sky_lower::lower_payload_pat",
              "nested record sub-pattern has no solved region type",
          )
      })?;
      self.lower_record_pat(fields, ty, p.span)
  }
  ```

  This reuses `lower_record_pat` (`lower.rs:9458+`) UNCHANGED — it already
  builds a complete `Pat::Record` from a field-pun list and a `Ty::Record`,
  exactly the machinery `lower_binder_pat` already calls at the top level.
- Apply the identical replacement in `lower_destructure_pat`'s `PRecord`
  arm (currently ~9313).

**No backend change needed for C1.** `Pat::Record` nested inside
`Pat::Ctor.args` is ALREADY a documented, permitted IR shape
(`sky_ir::ir.rs:1332-1335`, the `Pat::Ctor` doc comment: "M3b-2: nested ctor
/ tuple / record sub-patterns are all permitted"), and `render_pat`
(`emit_expr.rs:5984-6000`) already recurses into `Pat::Ctor`'s args via the
SAME `render_pat` call that reaches `Pat::Record(fields) =>
render_record_pat(ctx, fields)` (line 6001) — ordinary Rust struct-pattern
nesting inside an enum tuple-variant pattern (`MyEnum::Variant(RecXY { name,
.. })`) is valid Rust with zero special casing required.

### Fix — C2: cons/list sub-pattern nested in a ctor payload (Repro 1)

This needs BOTH the lowering-side region-threading from C1 (reused as-is —
no new plumbing beyond what C1 already adds) AND a genuine backend
extension, because (per Root Cause above) a nested `Pat::Slice` cannot be
embedded directly in Rust pattern syntax against a `Vec<T>` field.

**Correctness constraint that rules out a naive "bind + panic" prelude:**
the nested list pattern is REFUTABLE — `Just []` is a valid value of `Maybe
(List Int)` that does NOT match `Just (h :: t)` — so a well-formed program
with a fallback arm (`f (Just (h::t)) = h; f _ = 0`, or the `case`-with-
wildcard shape in Repro 1) MUST fall through to that fallback when the outer
constructor matches but the inner list shape doesn't. A prelude that
`unreachable!()`s on a length mismatch would be UNSOUND (a real runtime
panic on well-typed input). The correct Rust shape is a **match guard** —
guards fall through to the next arm on `false`, exactly matching Sky's
semantics, and Rust's own exhaustiveness checker treats a guarded arm as
non-exhaustive (consistent with the case needing a trailing wildcard/other
arm, which every reachable Sky program of this shape already has, per its
own exhaustiveness check upstream).

**Step 1 — add an arm guard to the IR.** `crates/sky_ir/src/ir.rs`, `Arm`
(~1279): add a field:

```rust
pub struct Arm {
    pub pat: Pat,
    pub body: Expr,
    /// An optional boolean guard evaluated after `pat` matches; `false`
    /// falls through to the next arm (native Rust `match` guard semantics).
    /// `None` for every pre-existing arm shape (byte-identical emission).
    /// Introduced for Class 4 item C2 — a cons/list sub-pattern nested in a
    /// constructor payload lowers to a plain `Pat::Var` binder for that
    /// position PLUS a guard checking the bound `Vec`'s length/shape, with
    /// the named sub-bindings (`h`, `t`) recovered via indexing in the arm
    /// body's prelude rather than embedded in the pattern itself (Rust
    /// cannot slice-pattern a `Vec<T>` ENUM FIELD inline — only an actual
    /// slice/array, which needs a scrutinee-level `.as_slice()` coercion
    /// this position does not have).
    pub guard: Option<Expr>,
}
```

Update every existing `Arm` constructor call site (`Match::new_flat` and
any other `Arm { .. }` literal in `sky_lower`/`sky_ir`) to set
`guard: None` — this is an additive, non-breaking field; grep `Arm {` across
`crates/sky_lower/src/lower.rs` and `crates/sky_ir/src/ir.rs` to enumerate
every site (expect on the order of 5-15 sites given the existing arm-
construction helpers).

**Step 2 — lowering: desugar the nested cons/list sub-pattern into a
fresh-var binder + guard + body-prelude indexing.** In
`lower_payload_pat`'s `PList`/`PCons` arm (currently ~9270-9272,
unconditionally `Err(unsupported(p.span, Feature::NestedCtorDiscrimination))`),
this single-sub-pattern function cannot itself introduce an arm-level guard
(a guard belongs to the WHOLE arm, not one sub-position) — so the desugaring
must happen one level up, where the FULL ctor pattern + its eventual arm
body are both in scope: `lower_arm_pat`'s `PCtor` arm (~9843-9859) and the
equivalent in `lower_case`'s general (non-single-arm) path.

Concretely: extend `lower_arm_pat` (and any other direct caller of
`lower_payload_pat` for `PCtor` args that ALSO owns the arm body, i.e. the
per-arm loop in `Match::new_flat`'s caller) so that when a ctor's argument
pattern is `PList`/`PCons` (checked BEFORE delegating to
`lower_payload_pat`, which keeps rejecting a `PList`/`PCons` reached via any
OTHER path — e.g. still nested two levels deep, `Ok (Just (h::t))`, out of
scope for this item, see Residual Scope below):

1. Mint a fresh internal `Symbol` (reuse whatever synthetic-name minting
   convention the codebase already uses elsewhere, e.g.
   `any_param_binders`-style pooled fresh symbols mentioned in the AUD-01
   fix, or a simple `format!("__sky_nested_{n}")` interned symbol).
2. Replace that ctor argument's pattern with `Pat::Var(fresh)`.
3. Build a guard expression checking the required shape against `fresh`:
   for a CLOSED list pattern `[a, b]`, `fresh.len() == N`; for an OPEN cons
   chain `a :: b :: rest`, `fresh.len() >= N` (mirroring
   `consChainLength`'s existing per-arm length-guard precedent already
   used for the TOP-LEVEL cons-pattern arity gate, referenced in
   CLAUDE.md's "Closed in v0.15" ledger — reuse or port the identical
   counting logic here rather than re-deriving it).
4. Combine this guard with any PRE-EXISTING guard on the same arm via
   `&&` (a future arm might already carry one from an unrelated feature;
   keep the combination generic, not additive-only-when-empty).
5. Prepend to the arm body (as an IR-level `Expr::Let` chain, not a
   rendered-text prelude — mirroring AUD-04's rule that all such rewrites
   happen on the IR, never on rendered strings) one binding per named
   element (`h = fresh[0].clone()`) and, for an open tail, one binding for
   the rest (`t = fresh[1..].to_vec()`) — expressed as new `Expr`
   variants/kernel calls the backend already has (list indexing / slicing
   primitives used elsewhere in the runtime — reuse `rt`-equivalent helper
   or a direct Rust index expression via a new small `Expr::ListSliceFrom`/
   reuse existing `KernelFn` list helpers if one already covers "drop N,
   return rest" — check `List.drop` reuse first before adding a new IR
   node, per "reuse over new machinery").

**Step 3 — backend: render the guard.** `crates/sky_backend_rust/src/emit_expr.rs`,
`emit_match` (~5473-5499): after building `pat` via `emit_arm_head`, if
`arm.guard` is `Some(g)`, render it and emit `{pat} if {guard} =>
{arm_body}` instead of the current unconditional `{pat} => {arm_body}`.
`emit_tuple_arm_head`'s analogous per-tuple-arm rendering needs the same
treatment if C2 is also to support a cons pattern nested inside a tuple
column (out of THIS item's minimum scope per the empirically-verified
repros, which are both single-ctor shapes — note as a natural follow-on,
not silently dropped).

### Residual scope (documented, not silently dropped)

This item's fix, as specced, covers exactly the two repros verified against
HEAD: a record OR cons/list sub-pattern ONE level nested inside a SINGLE
constructor payload position (`Ok {name}`, `Just (h::t)`). It does **not**
cover:

- Two levels of nesting (`Ok (Just {name})`, `Just (Just (h::t))`) — the
  region-threading in C1's Step 1 only inserts ONE region per DIRECT
  sub-pattern of a `PCtor`; a sub-pattern that is ITSELF a `PCtor` recurses
  through `constrain_pattern`'s own `PCtor` arm again, so as long as the
  SAME insertion is added there (it already is, per Step 1 — the insertion
  runs on every level of `PCtor` recursion since `constrain_pattern` is
  called recursively), two-level nesting of a RECORD sub-pattern (C1)
  should already work for free. Verify with a dedicated test (see below)
  rather than assuming; do not claim it fixed without that test passing.
- A cons/list sub-pattern nested inside a TUPLE column (`(Just (h::t), y)`)
  — `tuple_case_supported` (`lower.rs:9389-9440`) has its own, SEPARATE
  per-column list-mode gate; C2's arm-level guard mechanism does not
  automatically reach the tuple-column emission path
  (`emit_tuple_arm_head`). File as a natural follow-on to this item if a
  future sweep needs it — do not claim it fixed here.

### Verification

Before the fix: both repros reject (`SKY-L0116` for Repro 1, `SKY-L0112`
for Repro 2).

After the fix:

- Repro 2 compiles and prints `Ada`.
- Repro 1 compiles and prints `1`. Additionally verify the FALLTHROUGH
  case explicitly with a second repro:

  ```sky
  f : Maybe (List Int) -> Int
  f m =
      case m of
          Just (h :: t) -> h
          _ -> 0

  main =
      println (String.fromInt (f (Just [])))   -- must print 0, NOT panic
  ```

  This is the load-bearing correctness check for the guard-based design —
  it must print `0`, never panic, never emit a non-exhaustive-match runtime
  error.

### Regression tests

- `tests/golden/m158_nested_record_payload/Main.sky` (Repro 2) +
  `crates/skyc/tests/golden_m158_nested_patterns.rs` asserting build success
  and (`SKY_E2E`) the printed output.
- `tests/golden/m158_nested_cons_payload/Main.sky` (Repro 1) — same harness.
- `tests/golden/m158_nested_cons_payload_fallthrough/Main.sky` (the
  `Just []` fallthrough repro above) — THE decisive soundness test; must be
  present before this item is considered done, not an optional nice-to-have.
- `tests/golden/m158_nested_record_two_levels/Main.sky` (`Ok (Just
  {name})`) — proves or disproves the "should work for free" claim above;
  if it does NOT pass, downgrade the spec's claim and either fix the extra
  level or file it explicitly as residual scope with a reason.
- A `sky_lower` unit test on the new `Arm.guard` field's default (`None`)
  for every PRE-EXISTING arm-construction path, confirming byte-identical
  IR for every program that doesn't touch this feature (mirrors the "empty
  clause stays byte-identical" invariant used throughout the M2c/M2d work).

---

## Item D — #102: local `type X` shadowing a dep-imported `X`

### Status

Confirmed live against HEAD with a fresh two-module repro:

```sky
-- Dep.sky
module Dep exposing (Color(..))

type Color = Red | Green | Blue
```

```sky
-- Main.sky
module Main exposing (main)
import Sky.Core.Prelude exposing (..)
import Dep exposing (Color(..))
import Std.Log exposing (println)

type Color = Warm | Cool

describe : Color -> String
describe c =
    case c of
        Warm -> "warm"
        Cool -> "cool"

main =
    println (describe Warm)
```

```
skyc: error[SKY-T0001]: type mismatch
  --> Main.sky:11:9
   |
11 |         Warm -> "warm"
   |         ^^^^ expected Dep.Color, found Main.Color
```

This is a genuinely confusing message for a program whose ACTUAL mistake is
"I declared `type Color` locally without noticing `Dep` already brought one
into scope" — the fix target is a clean rejection AT LINE 6
(`type Color = Warm | Cool`), naming the shadow, not a downstream type
mismatch three functions later that never mentions the word "shadow" or
"duplicate."

### Root cause

`crates/sky_canon/src/resolve.rs`, `canonicalise_with_env`
(~964-1014). Two DIFFERENT mechanisms exist for the SAME class of problem
and only one of them actually rejects:

- **Dep-vs-dep clash (correctly rejected today):** `inject_dep_type`
  (~1426-1446) is called once per `import` while building `type_home_map`
  from EVERY dep's exports (`inject_dep_exports`, called before
  `canonicalise_with_env`). It explicitly checks
  `type_home_map.get(&type_name)` against the NEW home and returns
  `NameError::DuplicateType` (`SKY-N0012`) on a mismatch — "two deps expose
  the same unqualified type name" is caught cleanly.
- **Local-vs-dep clash (silently mishandled today):** when
  `canonicalise_with_env` later processes THIS module's OWN `type`
  declarations (~978-981):

  ```rust
  for u in &m.unions {
      type_home_map
          .entry(u.value.name.value)
          .or_insert_with(|| home.clone());
  }
  ```

  `.entry().or_insert_with()` is a **silent no-op** when
  `type_home_map` already has an entry for that name (exactly the case
  here — `inject_dep_exports` already put `Dep`'s `Color` in there before
  this loop runs). So `type_home_map["Color"]` KEEPS pointing at `Dep`'s
  home. Meanwhile the SECOND loop a few lines down (~995-1014,
  `seen_types`/`register_union`) unconditionally registers the LOCAL
  `Color` union's constructors into the environment — `seen_types` starts
  as a FRESH, EMPTY map every call (line 995), never seeded from
  `type_home_map`'s pre-existing dep entries, so it has no way to detect
  this specific clash either.

  The net effect: the environment ends up with the LOCAL module's ctors
  (`Warm`/`Cool`) registered as belonging to `Color`, while
  `type_home_map["Color"]` (consulted by `canonicalise_type` when resolving
  the `Color` type ANNOTATION on `describe : Color -> String`) still
  resolves to `Dep`'s `Color`. The annotation and the ctors now disagree
  about which `Color` is meant — surfacing three functions later as an
  ordinary `SKY-T0001` type mismatch between "the annotation's `Color`"
  (`Dep.Color`) and "the case-arm ctor's `Color`" (`Main.Color`), with no
  hint that the actual problem is a local declaration shadowing an import.

  Type ALIASES (`m.aliases`, ~1016-1049+) have the SAME gap by omission:
  they are never inserted into `type_home_map` at all (aliases expand
  inline, no nominal home needed for `Type::Con` resolution) and are only
  checked against the SAME per-call fresh `seen_types` map — a local `type
  alias Color = ...` shadowing a dep-imported `Color` union is equally
  unrejected, for the identical underlying reason (no check against
  `type_home_map`'s pre-existing entries).

### Fix

Add an explicit clash check, mirroring `inject_dep_type`'s existing
pattern EXACTLY, before either of the two existing per-module loops touch
`type_home_map`/`seen_types`. In `canonicalise_with_env`
(`crates/sky_canon/src/resolve.rs`, insert immediately before the current
~978 loop):

```rust
// #102: a LOCAL type/alias declaration whose name already has a
// `type_home_map` entry from a DIFFERENT home is a dep-imported type being
// shadowed. Reject it here, at the declaration, with the SAME
// `NameError::DuplicateType` (SKY-N0012) `inject_dep_type` (~1426) already
// uses for a dep-vs-dep clash — closing the asymmetry where THAT clash was
// caught cleanly but this one silently mis-registered the environment,
// surfacing later as an unrelated SKY-T0001 (docs/architecture/
// class4-pattern-lowering-fix-spec-2026-07-09.md, item D).
for u in &m.unions {
    let type_name = u.value.name.value;
    if let Some(existing) = type_home_map.get(&type_name) {
        if existing.as_slice() != home.as_slice() {
            return Err(Diagnostic::Name {
                span: u.value.name.span,
                msg: NameError::DuplicateType {
                    name: name_str(interner, type_name)?,
                    first: Span::DUMMY, // matches inject_dep_type's convention: no source span survives from the dep side
                },
            });
        }
        // existing == home: unreachable in practice at this point (this
        // module's own unions have not been inserted yet), kept only for
        // defensive symmetry with inject_dep_type.
    }
}
for a in &m.aliases {
    let alias_name = a.value.name.value;
    if let Some(existing) = type_home_map.get(&alias_name)
        && existing.as_slice() != home.as_slice()
    {
        return Err(Diagnostic::Name {
            span: a.value.name.span,
            msg: NameError::DuplicateType {
                name: name_str(interner, alias_name)?,
                first: Span::DUMMY,
            },
        });
    }
}
```

Then leave the EXISTING ~978-981 loop (`type_home_map.entry(...)
.or_insert_with(...)`) and the existing ~995-1014 `seen_types` loop
UNCHANGED — they still correctly handle same-module duplicate detection
(`type Color = A; type Color = B` in ONE module) with better span
attribution (the FIRST declared span, not `Span::DUMMY`), since by the time
they run, the new check above has already ruled out the dep-shadow case.

Note the check must run for `m.unions` BEFORE `m.aliases` inserts anything,
and BOTH must run BEFORE the pre-existing ~978 loop mutates
`type_home_map` — otherwise a two-union-in-one-module case would spuriously
see its OWN first union's freshly-inserted entry and misreport a shadow.
Structuring it as a separate, standalone pre-pass (as written above,
reading `type_home_map` but not yet writing this module's own entries into
it) avoids that ordering hazard entirely.

### Verification

Before the fix: the repro above produces `SKY-T0001` "expected Dep.Color,
found Main.Color" at line 11 (a case arm three lines from the actual
mistake).

After the fix: the SAME repro produces `SKY-N0012` (`DuplicateType`) at
line 6, column 6 (`type Color = Warm | Cool`), naming `Color` as the
colliding identifier.

Also verify NO regression on the legitimate, currently-accepted case: two
DIFFERENT modules each locally declaring their OWN non-imported `type X`
(no shadow at all) still compiles — e.g. `Dep.sky` declares `type Color`
and is NEVER imported by `Main.sky`, while `Main.sky` independently
declares its own unrelated `type Color`; `type_home_map` in `Main`'s
resolution never gained a `Dep.Color` entry in the first place (dep exports
are only injected for modules actually `import`ed), so the new check's
`type_home_map.get(&type_name)` correctly returns `None` and nothing
rejects.

### Regression tests

Add to `crates/sky_canon`'s existing name-resolution test suite (grep for
the existing `DuplicateType`/`SKY-N0012` test module, likely alongside
`inject_dep_type`'s own coverage, to place these as siblings):

- `local_type_shadowing_dep_imported_type_is_duplicate_type` — the exact
  repro above via the in-process multi-module test harness (not a golden
  fixture, if `sky_canon` has a smaller in-crate harness for this — check
  existing dep-vs-dep `DuplicateType` tests for the established harness
  shape before adding a golden fixture unnecessarily).
- `local_type_alias_shadowing_dep_imported_type_is_duplicate_type` — same
  shape, but the LOCAL declaration is `type alias Color = Int` instead of
  a union, proving the alias-side gap is ALSO closed.
- `two_modules_each_declaring_unrelated_same_named_type_without_import_is_fine`
  — the non-regression control case above (no import, no clash).
- `same_module_duplicate_type_still_uses_first_declared_span` — confirms
  the EXISTING same-module duplicate path (unchanged ~995-1014 loop) is
  untouched by this fix and still reports the better (non-`DUMMY`) span.
- One golden fixture (`tests/golden/m102_local_type_shadows_dep/`) with the
  exact two-module repro, wired through `crates/skyc/tests/` the same way
  other cross-module `DuplicateType`/`SKY-N0012` golden tests already are,
  asserting the diagnostic fires at the LOCAL declaration's span, not a
  downstream `SKY-T0001`.

---

## Item E — #32: Task arity ICE (annotations) + Task/Cmd/Sub-in-ADT-ctor gate

### Status

A **fully-detailed, unexecuted implementation plan already exists** at
`docs/superpowers/plans/2026-07-02-m5a-task-followups.md` (dated 2026-07-02,
authored against HEAD `691e275`). Verified against CURRENT HEAD (2026-07-09):

- Both `CompilerBug` sites the plan targets are STILL PRESENT, unchanged in
  behavior:
  - `crates/sky_types/src/constrain.rs:2032-2037` — `normalize_annotation_ty`'s
    `n => Err(Diagnostic::CompilerBug { .. })` arm for a `Task` annotation
    applied to a number of type arguments other than 1 or 2. The stale doc
    comment the plan calls out ("canonicalisation rules out arity-0 or
    arity-3+ applications", now at `constrain.rs:1966-1968`) is ALSO still
    present, still false for the same reason the plan documents
    (canonicalisation validates alias arity only, never non-alias
    type-constructor arity like `Task`).
  - `crates/sky_lower/src/lower.rs:4169-4175` — `ir_type_from_canon`'s
    `"Task" => Err(bug(...))` arm for the SAME mis-arity shape reached
    through a constructor FIELD type (never passes through
    `normalize_annotation_ty`).
  - `lower_enum` (`lower.rs:3290-3336`) has NO gate today rejecting a
    well-formed `Task Error a` (arity 2) embedded in a constructor payload
    — it lowers straight through to a `Variant` carrying an `IrType::Task`
    field, which — absent Item B's fix — would ALSO currently trip Item B's
    now-removed Gate 2's SIBLING concern for Task specifically (Gate 2 only
    checked `ir_contains_fun`, which recurses THROUGH `IrType::Task(inner)`
    rather than treating the `Task` head itself as non-derivable — see
    `ir_contains_fun`'s `IrType::Task(inner) | IrType::Cmd(inner) |
    IrType::Sub(inner) => ir_contains_fun(inner)` arm, `lower.rs:287`).
    **This confirms the plan's Task 2 is still a genuine, unfixed gap
    distinct from Item B**, and — because Item B removes `lower_enum`'s
    Gate 2 entirely — Item E's new gate is now the ONLY thing standing
    between a `type Job = Job (Task Error Int)` declaration and
    cargo-failing Rust once Item B lands. **Item E must land no earlier
    than, and should be sequenced immediately after, Item B** — if Item B
    lands first without Item E, `type Job = Job (Task Error Int)` would go
    from "rejected via the (wrong, function-focused) Gate 2 message" to
    "silently accepted and emitted as cargo-failing Rust" for the window
    between the two — treat B and E as one merge unit, or land E's `lower_enum`
    gate in the SAME commit as B's Gate 2 removal.
- The plan's PROPOSED diagnostic codes are **stale**: `SKY-T0015` and
  `SKY-L0119` are now BOTH already allocated to unrelated diagnostics
  (`TypeError::RefutablePatternParameter` / "parameter pattern must be
  irrefutable", and `Feature::LetBoundAppCfg` / "app entry cfg must be an
  inline record literal", respectively — confirmed via
  `crates/sky_diagnostics/src/code.rs`). The next free codes at current
  HEAD are **`SKY-T0016`** and **`SKY-L0127`** (confirmed: neither exists
  anywhere in `sky_diagnostics`; `code.rs`'s taxonomy count is currently 86
  — `taxonomy_has_eighty_six_codes`, bump to 88 for these two new codes).

### Root cause

Exactly as the existing plan documents (re-verified, not re-derived):

1. **Annotation-path arity ICE.** `normalize_annotation_ty` handles `Task`
   applied to 1 arg (internal unary form) and 2 args (`Task Error a`,
   validating the error channel) explicitly, then falls to a wildcard `n`
   arm that unconditionally raises `Diagnostic::CompilerBug` — reachable
   from source because canonicalisation validates arity only for type
   ALIASES (`NameError::AliasArity`), never for a non-alias type
   constructor application like `Task Error Int Bool` (verified against
   `resolve.rs`'s alias-arity check, which has no equivalent for bare
   `Con` applications).
2. **Constructor-field mis-arity ICE.** The identical mis-arity shape
   reached through a constructor FIELD type (`type J a = J (Task Error a
   Bool)`) never passes through `normalize_annotation_ty` at all — it
   reaches `ir_type_from_canon`'s `"Task"` dispatch directly, which has
   explicit 1-arg and 2-arg arms and then a bare `"Task" => Err(bug(...))`
   catch-all.
3. **Task/Cmd/Sub-in-ctor-payload silent mis-lower.** A WELL-FORMED `Task
   Error Int` (arity 2) embedded in a constructor payload passes every
   CURRENT `lower_enum` gate (Gate 1's polymorphism check does not apply;
   Gate 2, even before Item B removes it, only rejects via
   `ir_contains_fun`, which recurses PAST `IrType::Task`'s head rather than
   flagging it) and lowers to a `Variant` carrying `IrType::Task`. Backend
   emission (`emit_types.rs`) unconditionally attempts the enum's
   `#[derive(Clone, Debug, PartialEq)]` set UNLESS `enum_derivable` already
   flags it `false` — and `ir_type_is_derivable`'s `IrType::Task(_) |
   IrType::Cmd(_) | IrType::Sub(_) => false` arm (`sky_ir/src/ir.rs:855-857`)
   DOES already correctly classify these as non-derivable, so **the #87
   backend seal ALREADY protects this specific case gracefully** — meaning,
   unlike Item B, this sub-item's underlying "does the backend degrade
   correctly" question is ALREADY answered yes by the SAME machinery. The
   remaining problem is purely that no LOWERING-level diagnostic exists yet
   for this shape at all today it currently reaches the backend and
   (thanks to #87) degrades gracefully rather than cargo-failing — BUT
   see the sequencing note above: this is only true as long as `lower_enum`
   doesn't ALSO acquire a new Task-specific rejection. Re-verify at
   implementation time whether accepting `type Job = Job (Task Error Int)`
   (building successfully via graceful degradation, matching Item B's
   precedent exactly) is preferable to REJECTING it with a new `SKY-L0127`
   as the 2026-07-02 plan originally proposed. **Recommendation: prefer
   ACCEPTING it (consistent with Item B's direction — the backend can
   already degrade this gracefully, so rejecting it would be a regression
   relative to what Item B just taught the compiler to do for functions).**
   This is a delta from the 2026-07-02 plan's original design (which
   pre-dates Item B's diagnosis) — flag this explicitly to whoever
   implements Item E, and resolve it via the verification step below
   BEFORE writing the `SKY-L0127` gate, not after.

### Fix

**Sub-fix E1 (`SKY-T0016`, was the plan's `SKY-T0015` — annotation arity).**
Follow `docs/superpowers/plans/2026-07-02-m5a-task-followups.md` Task 1
VERBATIM, with these substitutions against current HEAD:

- Every occurrence of `SKY-T0015`/`SKY_T0015` → `SKY-T0016`/`SKY_T0016`.
- `code.rs:149` anchor (insert-after point) → insert after the CURRENT last
  `SKY_T00xx` const (`SKY_T0015` itself, now taken by
  `RefutablePatternParameter` — insert the new `SKY_T0016` const
  immediately after it in file order, matching numeric order).
- `constrain.rs:1326` anchor (the `n` arm) → current location
  `constrain.rs:2032-2037` (verified above).
- `taxonomy_has_seventy_five_codes`/`75`/`76` count anchors → current count
  is 86 (`taxonomy_has_eighty_six_codes`); bump to 87 for this sub-fix
  alone (88 total once E2 also lands — see below), renaming the test
  function each time per the existing convention.
- The plan's Step 8 doc-comment fix (correcting the stale "canonicalisation
  rules out arity-0/3+" claim) applies to the CURRENT comment location,
  `constrain.rs:1966-1968`.

Everything else in the plan's Task 1 (the `TypeError::TaskArity { found:
usize }` variant, the four exhaustive-match wiring sites, the explain page,
the golden fixtures, the `assert_gate` test harness) is directly reusable
as written — it was never invalidated by anything that landed since
2026-07-02, only its line/code anchors shifted.

**Sub-fix E2 (`SKY-L0127`, was the plan's `SKY-L0119` — ctor-field mis-arity
ICE).** Follow the plan's Task 2 for the mis-arity-in-a-ctor-field half
ONLY (the `task_arity_in_canon` predicate + the Gate 0a insertion in
`lower_enum` reusing `TypeError::TaskArity`/`SKY-T0016` from E1), with the
same code/line-anchor substitutions. This half is UNCONDITIONALLY worth
keeping regardless of how the Task/Cmd/Sub-in-payload question (below)
resolves — a mis-arity `Task` in a ctor field is ALWAYS wrong, never a
legitimate program.

**Sub-fix E3 (Task/Cmd/Sub-in-ctor-payload — DECISION NEEDED before
implementation, not a straight port of the plan).** The 2026-07-02 plan's
original design (reject with a new `Feature::CtorPayloadTask`/`SKY-L0127`)
pre-dates Item B's diagnosis that the #87 backend seal ALREADY gracefully
degrades a non-derivable enum. Before writing this gate, re-run the plan's
own fixture —

```sky
type Job = Job (Task Error Int)

run : Job -> Int
run j =
    case j of
        Job _ -> 0

main =
    println (String.fromInt (run (Job (Task.succeed 1))))
```

— against HEAD-with-Item-B-but-not-yet-E3, and observe whether it (a)
builds and runs cleanly (in which case, per this spec's recommendation,
DO NOT add a new rejection — document the acceptance as intentional,
symmetric with Item B's `Result`/`Maybe`/function precedent, and skip E3's
gate entirely), or (b) still emits cargo-failing Rust for some OTHER reason
not yet identified (in which case implement the plan's original
`ir_embeds_async_opaque` gate as written, renumbering `SKY-L0119` →
`SKY-L0127`). Either outcome is a clean, mechanical next step; do not guess
— run the fixture and let the actual `cargo build` result decide.

### Verification

Before the fix: `doThing : Task Error Int Bool` (3 args) and `doThing :
Task` (0 args) both ICE with the generic `SKY-I0001`-style `CompilerBug`
message ("please report", not a normal diagnostic). `type Boxed a = Boxed
(Task Error a Bool)` also ICEs (a different `CompilerBug` site).

After E1+E2: all three produce a clean `SKY-T0016` diagnostic naming the
found argument count, at the correct span (the annotation for the first
two, the constructor declaration for the third).

After E3 (whichever branch the decision above selects): `type Job = Job
(Task Error Int)` either (a) builds and runs, printing `1`, or (b) rejects
with a clean `SKY-L0127`, never a cargo failure and never a `CompilerBug`.

### Regression tests

Reuse the plan's own test steps VERBATIM (`golden_m5a_task_gates.rs`
extension for E1, a new `golden_m5a_ctor_task_gate.rs` for E2/E3), renumbering
codes and re-anchoring lines as described. Additionally:

- Add one NEW test not in the original plan: a direct `cargo test -p
  sky_diagnostics` run confirming the taxonomy count assertion matches
  the actual number of codes added THIS TIME (87 after E1, 88 if E3 adds a
  code, 87 if E3 concludes "no new code needed") — do not blindly copy the
  plan's hardcoded `76`/`77` expectations, which are for numbers that
  are no longer accurate.
- Add a one-line note to `docs/superpowers/plans/2026-07-02-m5a-task-followups.md`
  itself (or a new dated addendum) recording that it was executed via this
  spec with renumbered codes, so a future reader doesn't try to execute the
  stale numbers again.
