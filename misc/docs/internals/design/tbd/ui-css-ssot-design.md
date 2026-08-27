# Ui/Css single source — one CSS-value renderer

Status: design proposal, no implementation yet. The Rust and Ipê blocks are
illustrative sketches; existing paths are quoted verbatim from the current tree.

`Ipe.Ui`'s inline-style path (`src/runtime/rust/src/ui/render.rs`) and
`Ipe.Css`'s stylesheet DSL (`src/stdlib/Ipe/Css.ipe`) each render CSS length and
colour values to strings — two hand-written formatters for the same CSS value
grammar. This note asks whether that is a real single-source-of-truth violation,
and, if so, how to collapse it to one renderer without weakening the CSS
injection sink (the `CssSafety` gates, native per ADR-0057).

## 1. Is there real duplication? — measured

Two independent formatters exist, over two DIFFERENT length/colour types.

**Runtime `ui::element` types** (`src/runtime/rust/src/ui/element.rs`) — the
`Ipe.Ui` layout vocabulary, formatted by `length_css` / `color_css` in
`render.rs`:

```rust
enum Length { Px(i64), Content, Fill(i64), Min(i64, Box<Length>),
              Max(i64, Box<Length>), Vh(i64), Vw(i64) }
enum Color  { Rgba(i64, i64, i64, f64) }
```

**`Ipe.Css` types** (`src/stdlib/Ipe/Css.ipe`) — the CSS value vocabulary,
formatted by `lengthToString` / `colorToString`:

```
type Length = Px Int | Rem Float | Em Float | Pct Float | Vh Int | Vw Int
            | Ch Float | Fr Int | Num Float | LenAuto | LenZero | LenRaw String
type Color  = Hex String | Rgb .. | Rgba Int Int Int Float | Hsl .. | Hsla ..
            | ColorTransparent | ColorCurrent | ColorRaw String
```

These are not the same type. `Ipe.Ui.Length` is a *layout-intent* vocabulary:
`Content` and `Fill(n)` are elm-ui portion semantics, not raw CSS units —
`Fill(_)` renders as `"100%"` and drives a flex-grow/flex-basis:0 portion model
emitted at the `AttrWidth`/`AttrHeight` arms, and `Min`/`Max` recurse into
`min(..)`/`max(..)`. `Ipe.Css.Length` is the raw CSS length grammar
(em/pct/ch/fr/calc/minmax/auto/zero). The issue's own phrasing is exact: the two
vocabularies have *already drifted* by design intent — that drift is correct and
must stay.

**The actual byte-level overlap** — the arms that format the *identical* CSS raw
value from the *identical* scalar — is small and enumerable:

| Value | `render.rs` (native) | `Ipe.Css` | Byte-identical? |
|---|---|---|---|
| px | `Px(n) → "{n}px"` | `Px n → fromInt n ++ "px"` | yes |
| vh | `Vh(n) → "{n}vh"` | `Vh n → fromInt n ++ "vh"` | yes |
| vw | `Vw(n) → "{n}vw"` | `Vw n → fromInt n ++ "vw"` | yes |
| rgba | `Rgba(r,g,b,a) → "rgba({r},{g},{b},{a})"` | `Rgba r g b a → "rgba(" ++ … ++ floatStr a ++ ")"` | **NOT guaranteed** |

Three length arms and one colour arm are the whole shared surface. Everything
else in each type is unique to that side.

**The rgba arm is the live divergence.** `render.rs` formats the `f64` alpha with
Rust's `{a}` `Display`; `Ipe.Css` formats it with `floatStr = String.fromFloat`,
which is the hand-written `string_from_float` (`src/runtime/rust/src/string.rs`)
— a deliberate Go-`'g'`-shortest port that diverges from Rust `Display` on a
range of values (`1e6` → `"1e+06"` vs `"1000000"`; positional/exponent cut at
exponent 6). For the integer-alpha values the current tests exercise (`1.0` →
`"1"` under both) the two agree, so no test is red today. But the two formatters
CAN produce different bytes for the same colour, and nothing asserts they don't.
That is a genuine SSOT violation on the one arm — narrow, but real, and exactly
the class the SSOT rule exists to forbid ("assert their equality in a test; never
hand-sync").

**Verdict: worth doing, but scoped tightly.** There is no case for a wholesale
"route `Ipe.Ui` through `Ipe.Css`" refactor — the two `Length` types are
legitimately different and coupling them would drag layout-intent semantics into
the CSS value type (a Readability + Correctness loss to fix a 4-arm duplication).
The right fix is a *single-source for the shared raw-value formatting* — px, vh,
vw, and the rgba spelling — enforced by an equality test, not a shared type.

## 2. What single-sources vs what stays native

Split the concern in three, along the principle boundary.

**Stays native, unchanged (security — ADR-0057 carve-out).** The `CssSafety`
sink (`SafeCssValue` / `SafeCssPropertyName` / `SafeCssSelector` /
`SafeCssMediaQuery` / `strip_style_close`, in `css_safety.rs`) is a native
security-defence leaf and is out of scope. This design touches *what raw string a
formatter emits before the gate*, never the gate. The gate's drop-on-fail posture
and byte-exact behaviour are invariant.

**Stays native (layout semantics, not value formatting).** Everything in
`build_style_string` that is not a bare scalar→CSS spelling stays exactly where
it is: the `Fill(n)` → `flex-grow`/`flex-basis:0`/`min-*:0` portion model, the
`Content → "auto"` intent mapping, `Min`/`Max` recursion, the overflow per-axis
gates, the `__col`/`__row`/`__grid` direction markers, `TaggedNode`/`Description`
semantic-tag selection, and the `saturating_add` border-width arm. These are
`Ipe.Ui` layout intent, not CSS value grammar — they have no home in `Ipe.Css`
and must not move.

**Single-sourced (the shared value spellings).** Exactly the unit/colour
spellings that both sides emit:

- the px/vh/vw unit suffixes on an integer scalar, and
- the `rgba(r,g,b,a)` spelling, INCLUDING its float-alpha rendering.

The SSOT primitive is one small native leaf — call it `css_value` — that owns
these spellings, imported by BOTH `render.rs` and (via the existing kernel path)
`Ipe.Css`. `Ipe.Css` already reaches native code for `String.fromFloat`
(`string_from_float`), so its `floatStr` and any shared spelling resolve to the
same native functions the `render.rs` path uses — no second float algorithm, no
second rgba concatenation. Concretely:

```rust
// src/runtime/rust/src/css_value.rs  (new native leaf — spellings only, NO gate)
pub fn px(n: i64) -> String { format!("{n}px") }
pub fn vh(n: i64) -> String { format!("{n}vh") }
pub fn vw(n: i64) -> String { format!("{n}vw") }
/// rgba spelling — alpha via `string_from_float`, the ONE float renderer.
pub fn rgba(r: i64, g: i64, b: i64, a: f64) -> String {
    format!("rgba({r},{g},{b},{}", …); // uses string::string_from_float(a)
}
```

`render.rs::length_css`/`color_css` call these; `Ipe.Css.colorToString`'s rgba
arm resolves (through its kernel spelling) to the same `string_from_float`. One
spelling, two call sites, zero hand-sync.

**SSOT enforcement — the equality test (the load-bearing part).** Because the two
types cannot share a single Rust enum, the anti-drift guarantee is a test, per
PRINCIPLES ("assert their equality … never hand-sync"): a runtime test that, for
the overlapping value space (px/vh/vw over a scalar sweep, and rgba over an
integer×float alpha sweep), asserts `render.rs`'s output byte-equals `Ipe.Css`'s
output. This is the tripwire that makes the drift a CI failure instead of a
silent bug — the same discipline the kernel-registry tripwires use.

## 3. Byte-identity + security

**Byte-identity.** px/vh/vw already agree, so single-sourcing them is a pure
no-op refactor: goldens and the emit tests stay byte-identical. The rgba arm is
the one place a re-bless is *possible*: routing native alpha through
`string_from_float` changes `render.rs`'s alpha spelling from Rust `Display` to
Go-`'g'`-shortest. For every integer alpha (the tested corpus: `1.0`, `0.2`
round-trips) the bytes are unchanged; a re-bless would only appear for a
fractional alpha whose two spellings differ, and the correct spelling is the
`string_from_float` one (it matches the `Ipe.Css` stylesheet path AND the
example-sweep Go oracle that `Ipe.Ui` HTML is diffed against). So: unchanged
output on the tested corpus, and any re-bless is a *convergence toward the oracle*
with the sink untouched — a justified, security-neutral re-bless.

**Security — the sink is provably intact.** The single-sourced leaf emits
spellings only; it performs NO validation and is inserted strictly *upstream* of
the existing `SafeCssValue`/`SafeCssPropertyName` gates in `build_style_string`
and upstream of `Ipe.Css`'s `prop` smart constructor. Every value still flows
through the identical gate it does today (the px/vh/vw/rgba strings are
gate-passing by construction — no breakout chars — so gate behaviour is unchanged
on them). No `CssSafety` function is edited, moved, or re-ordered. The
injection barrier is byte-for-byte the same code; this refactor is invisible to
it.

## 4. Sliced implementation plan (guardian-gated)

Each slice is independently landable, byte-identical-or-justified-rebless, and
gets a `security-soundness-guardian` review because it touches a file feeding the
CSS sink.

1. **Extract the value leaf + the equality test (no behaviour change).** Add
   `css_value.rs` with `px`/`vh`/`vw` (identical spellings), point
   `render.rs::length_css` at them. Add the runtime equality test comparing
   `render.rs` vs `Ipe.Css` on the px/vh/vw sweep. Byte-identical; goldens
   unchanged. Guardian confirms the leaf has no validation and sits upstream of
   the gate.
2. **Single-source the rgba spelling.** Move `color_css`'s rgba arm to
   `css_value::rgba`, routing alpha through `string_from_float`; extend the
   equality test to the rgba integer×float alpha sweep. This is the one slice
   that may re-bless a golden (only for a fractional alpha, converging to the
   oracle). Guardian confirms the sink is untouched and the re-bless, if any, is
   oracle-convergent.
3. **Lock the tripwire.** Ensure the equality test is in the full-gate set so any
   future arm added to one formatter and not the other fails CI — the anti-drift
   seal that replaces hand-sync.

Out of scope (explicitly NOT done): merging the two `Length`/`Color` types,
moving `build_style_string`'s layout semantics, and any edit to `css_safety.rs`.
The companion `Ipe.Html` split noted in the umbrella issue is a separate leaf
(`render_html` + escapers stay native) and is not addressed here.

## 5. Principle trace

Security is untouched (the sink is upstream-of and unedited — the highest
principle is held invariant). Correctness improves: one rgba spelling removes the
possibility of `Ipe.Ui` and `Ipe.Css` disagreeing on the same colour, and the
equality test makes any regression a CI failure (SSOT / make-invalid-states-
unrepresentable — the drift becomes unrepresentable-without-a-red-test). Soundness
and Efficiency are neutral. Readability improves marginally (one spelling home).
The refactor is refused where it would trade a higher principle for SSOT — merging
the layout-intent `Length` into the CSS `Length` is rejected because it would hurt
Correctness/Readability to erase a 4-arm duplication the SSOT rule already lets a
one-line test cover.
