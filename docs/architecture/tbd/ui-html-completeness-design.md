# Ipe.Ui / Ipe.Html Completeness — Reconciled Design (#76)

> **Guardian synthesis.** Reconciles three independent fresh designs (A1
> completeness+taxonomy, A2 invalid-states-at-registration, A3 soundness+prune)
> against the upstream Sky upstream learnings (B), critiqued in **strict principle
> order**: (1) security (2) correctness (3) soundness (4) efficiency
> (5) **COMPLETENESS** (6) readability, under **"PARSE, DON'T VALIDATE"** and
> **"MAKE INVALID STATES UNREPRESENTABLE."** COMPLETENESS is first-class: #76 is
> a totality problem, not a bug hunt.

---

## 1. Root cause (verified against HEAD)

The defect is one type in `src/compiler/canon/src/env.rs:104`:

```rust
Kernel(Option<StdlibKernel>, Symbol, Symbol)   // the Option is the bug
```

The `Option` makes **"reachable member with no backing"** a *representable
value*. A member listed as a `&str` in the hand-written `QUALIFIERS` array
(`env.rs:327`) is inserted into `qual_vars` with `id = None` whenever the
registry lookup misses. Name-resolution and type-check then pass — the member is
reachable — and the hole only detonates ~5 stages later at `lower_callee` (the
`Feature::Kernels` / IPE-L0108 fallback) or, worse, as exit-0-then-cargo-fail.
The member set is authored **twice** (the `QUALIFIERS`/`FUNC_ALIASES`/
`QUALIFIER_ALIASES` string arrays vs `StdlibKernel::ALL`), and the two drift,
with `id = None` as the drift sink.

The measured surface (diffing `qual_vars` names vs `decl()` names):
**~184 reachable-unbacked members** — Ui 39, Html 72, Font 22, Attr 22,
Border 12, Event 12, Background 5 (≈160 net after collapsing alias spellings).
`Region`/`Input`/`Grid` are doc-only and *unregistered* — unreachable, not
unbacked; they are a distinct "net-new module" bucket, not part of the hole.

Note the infra is already half-built: `stdlib_index` is derived anti-drift from
`StdlibKernel::ALL` (`env.rs:1159`), and three tripwire tests
(`canon_equals_registry`, `known_unbacked_disjoint_from_qual_vars`,
`stdlib_scheme_total_over_reachable`) already exist. This design *consolidates
and completes* that work rather than starting cold.

---

## 2. Chosen approach

**A single typed member manifest (SSOT) in the leaf `sky_kernels` crate whose
row type makes an unbacked-reachable member unrepresentable, structured as a
`PortStatus { Backed(Backing) | Deferred(reason) }` completeness ledger, with
`Backing` collapsing the tag-as-data and value-as-data families onto a handful
of generic kernels — grafting upstream Sky's narrow render/escape security kernels,
`SafeAttrName`, the depth cap, and the typed handler ADT.**

Concretely this is **Fresh Design A1's taxonomy and generic Tagged kernels,
organised under A2's `PortStatus`/`Deferred` ledger**, with A3's tag-faithful
soundness fix (kill the `html_p_` collapse), and the following upstream Sky grafts:
the ~5-kernel render/escape boundary, `SafeAttrName` parse-don't-validate,
`MAX_HTML_DEPTH` bounded descent, URL-scheme sanitiser + Go-equivalent escape
set, and typed `OnMsg`/`OnInput`/`OnForm` handler variants.

### Why this ordering of the six principles selects it

1. **Security (P1).** All tag/attr values in the manifest are compile-time
   literals from a *closed* table — never user strings — so no injection
   surface is added by the collapse. The only genuine sinks (`Html.raw`,
   `Html.script`, `Attr.style`, `Attr.attribute`, `Ui.htmlAttribute`) are
   gated: a member may only move `Deferred → Backed` once its runtime impl
   escapes/neutralises at construction, proven by a runtime escaping test.
   `SafeAttrName` (a newtype whose sole constructor runs the charset gate +
   `on*`/`srcdoc` denylist, consumed by *every* emit sink) and the URL-scheme
   walker are grafted verbatim from upstream — hand-audited, minimal, sink-total.
2. **Correctness (P2).** The **tag-as-data** collapse (A1 T2 / A3) fixes a
   silent wrong-render: today `helpers.rs` folds `h1|table|…` onto `html_p_`,
   emitting `<p>` for `<h1>`/`<table>`. That is the worst class (looks fine,
   renders wrong) and is fixed *first* among behaviour changes, guarded by an
   explicit `Html.h1 → <h1>` golden (not a diff-accept).
3. **Soundness (P3) — MAKE INVALID STATES UNREPRESENTABLE.** `Backing` carries
   a **non-optional** `StdlibKernel` (or `SpecialForm`). A `MemberSpec` row for
   an unbacked member *cannot be written* — there is no enum variant to name —
   so it is a Rust compile error inside `sky_kernels`. `id = None` is deleted
   from the type, not asserted-against at test time. Downstream,
   `constrain_var_kernel` takes `Backing` directly into the already-total
   `stdlib_scheme`, so no reachable path can return `None`.
4. **Efficiency (P4).** Generic `HtmlContainerNode`/`HtmlVoidNode` +
   `StyleValueAttr`/`NamedAttr`/… collapse ~145 of 184 members onto ~10 runtime
   fns and ~10 scheme arms — shrinking the binary, the match arms, and the
   review surface.
5. **COMPLETENESS (P5), first-class.** The manifest is the *single* enumeration.
   A member absent from it is absent from canon (honest use-site unknown-member
   error); a member present as `Backed` is provably backed; a member present as
   `Deferred(reason)` produces a **use-site diagnostic naming the port status**
   ("`Ui.mediaQuery` is part of the Ipe.Ui surface but not yet ported: needs
   CSS-emission runtime"). Nothing can sit in `id = None` limbo. A
   total-partition test + a pinned `DEFERRED_COUNT` make the *counted* residual
   explicit; a member that is neither backed nor deferred cannot exist.
6. **Readability (P6).** One declarative `MEMBERS` table replaces the three
   parallel string arrays + the scattered `lower.rs` string folds.

### The upstream Sky pure-Sky north-star (grafted as direction, not as the #76 fix)

Upstream's *actual* completeness mechanism is that `Ipe.Ui`/`Ipe.Html` are
**100% pure Ipê** compiled through ordinary lowering, backing only ~4-5
render/escape kernels + an opaque-ADT type bridge; the ~160 members need *no*
per-member id. That is genuinely superior on P4/P5/P6 and is the recorded
**north-star**: as the Rust canon/type/lower gain the ability to ingest the full
`sky-stdlib/Std/{Ui,Html}.ipe` ADT surface (List, records, closures-in-ADT-
fields — largely post-M0), each `Backed` batch below can be *retired to pure
Ipê*, thinning the manifest. It is **rejected as the immediate #76 deliverable**
(see §6) because the port is kernel-backed today and cannot yet compile that
source. Critically, the invariant is identical in both worlds: upstream reaches
completeness via **exhaustiveness over a closed ADT** (`collectStyle`'s total
`case attr of`); our non-optional `Backing` + total `stdlib_scheme` is the
Rust-side analogue of that same closed-set totality. We are building the same
gate, kernel-side, and it stays valid as members migrate to source.

---

## 3. Member taxonomy (batched categories + counts)

Partitioned by **what the name means + how much machinery it needs**. Counts are
the measured reachable-unbacked set; each category is one PR-sized batch that
moves N members `Deferred → Backed`.

| Cat | Name | Count | Backing shape | Notes |
|----|------|------:|---------------|-------|
| **T1** | Identical-alias | 12 | `Backing::Kernel` onto existing 1:1 kernel | `Event.onClick/onFocus/onBlur/onMouseOver/onMouseOut/onInput/onChange/onKeyDown/onKeyUp/onBool/onMsg` — already runtime-backed via `UiOn*`; today special-cased in `install_prelude_qualifiers`, becomes plain manifest rows. Cost = one row each. |
| **T2** | Tag-as-data (Html) | ~61 (50 container + 11 void) | `Backing::Tagged(HtmlContainerNode\|HtmlVoidNode, tag)` | `h1-h6, table/thead/tbody/tfoot/tr/td/th, ul/ol/li, nav/section/article/header/footer/main/aside, form/label/fieldset/legend, blockquote/figure/figcaption/details/summary/dialog, video/audio/canvas/iframe/progress/meter, script/body/pre/code/strong/em/small/textarea/select/option/span/div/a/button/p`; void `br/hr/meta/area/base/col/embed/source/track/wbr/img/input/link`. **Correctness-critical** — kills the `html_p_` collapse. Two generic kernels back all 61; only the emitted tag differs. |
| **T3** | Value-as-data attrs | ~50 | `Backing::Tagged(<StyleValueAttr family>, value)` | Font align/family/weight/decoration/spacing (22); Border style/shadow/glow/widthEach/hover* (12); Bg/Border/Font pseudo-state colors (`hoverColor/focusColor/activeColor/disabledColor`, 5+); Ui overflow axis (`clipX/clipY/scrollbarX/scrollbarY`), nearby (`above/below/onLeft/onRight/inFront/behind`), aspect (`aspectRatio/aspectRatioWH/square/widescreen`). Name is a CSS/enum value folded onto a value-carrying attr kernel; ~8 generic kernels back the family. |
| **T4** | Named-attribute `Attr.*` | 22 | `Backing::Tagged(NamedAttr\|BoolNamedAttr, key)` | `class/id/href/src/alt/value/name/placeholder/type_/for_/style/checked/disabled/readonly/required/multiple/selected/autofocus/tabindex`. `attribute`/`boolAttribute` stay generic (key is a runtime arg); `noAttr` nullary. **Audit first** (see risks): some may already resolve under the `Html` qualifier via `QUALIFIER_ALIASES`. |
| **T5** | Distinct new kernels | ~30 | `Backing::Kernel` / `Special`, one scheme each | `Ui.button/image/link` (record-arg), `Ui.input/form/paragraph/textColumn`, `Ui.onFile/onSubmit`, `Ui.htmlAttribute`, `Ui.paddingEach` (record), `Ui.mediaQuery/breakpoint/onPseudo`, `PseudoClass` values (`hover/focus/focusVisible/active/disabled`, nullary), `Breakpoint` values (`mobile/tablet/desktop/darkMode/lightMode/reducedMotion`, nullary). Group by shape to share schemes (3 record-arg builders; 2 nullary-value families). The pseudo-class/media-query subset needs the CSS-emission runtime and stays `Deferred` **longest**, with a precise reason. |
| **T6** | Prune (fail-closed) | small | *(removed from manifest)* | Genuine legacy duplicate spellings not used by any example/doc: `codeNode/headerNode/mainNode/footerNode/linkNode/headNode/htmlNode/titleNode/voidNode`, and `Html.toString` if it duplicates `render`. Deleting the row makes a reference fail-closed with an honest unknown-member error — **never** `id = None`. Prune only after `rg` over `examples/` + `docs/` confirms zero use; when in doubt, keep as a T2 alias rather than prune. |
| **G6** | Net-new modules | — | whole-module `Deferred` | `Region`/`Input`/`Grid` — unregistered today (like `PubSub`), so unreachable, not unbacked. Enter the ledger as whole-module `Deferred` entries; backed post-#76 (overlaps #78). |

**Collapse ratio:** ~145 of 184 members fold onto ~10 generic runtime fns +
~10 scheme arms (T1–T4); only ~30 (T5) are genuine per-member kernel work.

---

## 4. Ordered build tasks

Each batch lands fully green (`ipe build` + `cargo test -p sky_canon -p
sky_types` + the affected example) before the next. **The registry status gate
is Batch 0 and is a hard prerequisite** — it is the step that makes the invalid
state unrepresentable.

1. **BATCH 0 — the registry status gate (invalid-states-unrepresentable core).**
   In `sky_kernels` (leaf crate, no DAG cycle): introduce
   `enum Backing { Kernel(StdlibKernel), Tagged(StdlibKernel, &'static str), Special(SpecialForm) }`,
   `enum PortStatus { Backed(Backing), Deferred(DeferReason) }`,
   `struct MemberSpec { qualifier, member, status }`, and
   `const UI_HTML_SURFACE: &[MemberSpec]` enumerating **all ~184 members exactly
   once** plus `const DEFERRED_COUNT: usize`. Flip
   `VarHome::Kernel(Option<StdlibKernel>, …)` → `VarHome::Kernel(Backing, …)`
   and drop the `Option` from the canon AST `VarKernel` node + both `resolve.rs`
   construction seams. Rewrite `install_prelude_qualifiers` to **iterate the
   manifest and parse each row into a `VarHome`** — delete the `QUALIFIERS` /
   `FUNC_ALIASES` / `QUALIFIER_ALIASES` string arrays. `Backed` → `qual_vars`;
   `Deferred(reason)` → a new `Env::deferred_members` map that is *not* in
   `qual_vars`. This batch must be **byte-identical behaviour**: migrate the
   ~110 already-1:1-wired members unchanged and let the existing
   `canon_equals_registry` tripwire prove `qual_vars` is unchanged; enter all
   currently-unbacked members as `Deferred` so the tree compiles. (This is
   necessarily a single commit — the non-`Option` flip won't build until every
   reachable `id = None` member is `Deferred`. That is the point.)

2. **BATCH 0b — the completeness gate (count-assert / total-partition tests).**
   Land, replacing `canon_equals_registry`'s role: `ui_html_surface_is_total_partition`
   — (a) every `Backed(Backing::Kernel|Tagged(k,_))` member's `(qual,name) ∈
   StdlibKernel::ALL`; (b) every `Deferred` member ∉ `qual_vars` (fail-closed,
   subsumes `known_unbacked_disjoint_from_qual_vars`); (c) `Backed ∩ Deferred =
   ∅`, `Backed ∪ Deferred = manifest`; (d) the **exact sorted `Deferred` set**
   is asserted as a golden (not just the `DEFERRED_COUNT` integer — a diff shows
   precisely which member moved). Flip `stdlib_scheme`'s `_ => None` (line 1430)
   to total over the reachable `Backing` set so adding a `Backed` kernel *forces*
   a scheme arm (compile error until supplied); keep the existing
   `stdlib_scheme_total_over_reachable` as the proof. Add the `resolve.rs`
   use-site diagnostic for `deferred_members` (dedicated code, names the reason).
   **Only after 0b is green does any semantic backing begin.**

3. **BATCH T1 — identical-alias.** Manifest rows point `Event.*` at the existing
   `UiOn*` kernels; delete the ad-hoc `Event` special-case in
   `install_prelude_qualifiers` (now data). Flip 12 `Deferred → Backed`; drop
   `DEFERRED_COUNT`.

4. **BATCH T2 — tag-as-data (correctness-critical, first behaviour change).**
   Add two generic kernels `HtmlContainerNode` (Ipê-arity 2) + `HtmlVoidNode`
   (Ipê-arity 1), wired once through the 8 files; runtime gains
   `html_void_node_(tag, attrs)` beside the existing `html_node_(tag, attrs,
   kids)`. Lower injects the tag literal as arg0 at lower time (Ipê-visible
   arity stays 2/1 for the checker; runtime fn is 3/2 — internal injection,
   exactly like the existing baked-tag `html_div_`). **Delete** the
   `h1|table|… → html_p_` fold and the per-tag `html_div_/html_span_/html_p_`
   fns, collapsing them into the generic pair. Add the explicit
   `Html.h1 → <h1>` golden and an **eta golden** for a bare (uncalled) tag
   builder. Flip ~61 `Deferred → Backed`. Escape-gate `Html.raw`/`Html.script`
   here (`SafeAttrName` + `is_void`/verbatim-only-under-script/style).

5. **BATCH T3 — value-as-data attrs.** Add the small generic value-carrying attr
   kernels (`StyleValueAttr` per CSS-property group, `PseudoStateColorAttr`,
   `OverflowAxisAttr`, `NearbyElementAttr`, `AspectAttr`); manifest rows carry
   the parsed value/axis/location/pseudo as `Tagged` payload; lower injects the
   value literal. Flip ~50. CSS emission stays a single Rust runtime helper
   family per group (the analogue of upstream's total `collectStyle`).

6. **BATCH T4 — named `Attr.*`.** Add `NamedAttr(key,value)` /
   `BoolNamedAttr(key,bool)` generic kernels + rows for the 19 fixed-key attrs;
   keep `attribute`/`boolAttribute` generic; `noAttr` nullary. Escape-gate
   `Attr.style`/`Attr.attribute` (consume `SafeAttrName` at every sink). Flip 22.

7. **BATCH T5 — distinct kernels + events.** The ~30 genuinely-distinct kernels,
   each fully wired 8-file, grouped by shape (3 record-arg builders
   `button/image/link`; 2 nullary-value families `PseudoClass`/`Breakpoint`).
   Handlers use the **typed** `OnMsg String msg` / `OnInput String (String→msg)`
   / `OnForm String (FormData→msg)` ADT variants — **no** `OnRaw … any`
   laundering (see §6). `OnForm` returns `Option<msg>` (None on decode failure →
   no dispatch, no panic). The pseudo-class/media-query subset lands the
   CSS-emission runtime, then flips; hold it `Deferred` with a precise reason
   until that runtime exists.

8. **BATCH T6/G6 — prune + net-new.** `rg examples/ docs/` per legacy spelling;
   delete confirmed-dead rows; add a canon test asserting each pruned name is
   absent from `qual_vars`. Enter `Region`/`Input`/`Grid` as whole-module
   `Deferred` (backed with #78). Final `DEFERRED_COUNT` = the genuine post-v1
   residual.

9. **CLOSE-OUT — security + soundness grafts, verified.** Ensure across all
   batches: `MAX_HTML_DEPTH`-bounded descent in **both** renderer and sky-id
   stamper (truncate, never stack-overflow-abort); URL-scheme sanitiser
   (`javascript:`/`vbscript:`/`data:` neutralised except inert raster
   `data:image` on media attrs); Go-byte-equivalent escape set (`&#39;`/`&#34;`).
   Add a render golden per injection sink. Record the stringly-typed layout
   sentinel divergence (see §6) in `docs/divergences-from-sky.md`.

---

## 5. Files touched

```
src/compiler/kernels/src/surface.rs        (new — MemberSpec / PortStatus / Backing / UI_HTML_SURFACE / DEFERRED_COUNT)
src/compiler/kernels/src/lib.rs            (StdlibKernel variants: generic Tagged kernels; ALL derived)
src/compiler/canon/src/env.rs              (VarHome::Kernel non-Option Backing; deferred_members; iterate manifest)
src/compiler/canon/src/resolve.rs          (drop Option seams; Deferred use-site diagnostic)
src/compiler/canon/src/ast.rs              (VarKernel drops Option id)
src/compiler/canon/src/lib.rs              (total-partition test; retire canon_equals_registry role)
src/compiler/types/src/constrain.rs        (stdlib_scheme total over Backing; Backing → scheme)
src/compiler/lower/src/lower.rs            (delete legacy string dispatch + IPE-L0108 hole; tag/value injection)
src/compiler/backend/rust/src/naming.rs    (generic kernel names)
src/compiler/backend/rust/src/emit_expr.rs (single emit_html_tag arm; SafeAttrName sinks)
src/runtime/rust/src/ui/{helpers.rs,element.rs,render.rs}
src/runtime/rust/src/html.rs          (html_void_node_; depth cap; SafeAttrName; URL sanitiser)
docs/divergences-from-sky.md             (layout-sentinel divergence; numeric-entity escape spelling)
```

---

## 6. Rejected & why

**From the fresh designs (A):**

- **Keeping `Option<StdlibKernel>` / `id = None`** — rejected; it is the entire
  bug. It makes "reachable, unbacked" representable. Replaced by non-optional
  `Backing` (P3, MAKE INVALID STATES UNREPRESENTABLE).
- **The `html_p_` collapse (A3's target)** — rejected as **unsound
  correctness** (P2): rendering `<p>` for `<h1>`/`<table>` is a silent
  wrong-render. Replaced by tag-as-data (`HtmlContainerNode`/`HtmlVoidNode`),
  guarded by an explicit `Html.h1 → <h1>` golden, not a diff-accept.
- **Blind pruning to a generic unknown-member error (A1 T6 / A3 as the *default*
  for unbacked members)** — rejected on **COMPLETENESS** (P5). Pruning is
  correct only for genuinely-dead legacy spellings (confirmed by `rg`). For
  members that are *intended surface but not yet ported* (pseudo-class,
  media-query, Region/Input/Grid), a `Deferred(reason)` ledger entry giving a
  **use-site diagnostic that names the port status** is strictly more complete
  than a generic "no such member." A2's `PortStatus` supersedes A1/A3's prune-
  as-default here.
- **A2's `PortStatus` *without* A1's generic Tagged collapse** — rejected on
  **efficiency + readability** (P4/P6): enumerating 61 tag kernels + 50 value
  kernels one-per-member bloats the enum, the match arms, and the binary. A1's
  generic kernels fold ~145 members onto ~10 fns. Chosen design = A1's collapse
  *inside* A2's ledger.
- **Re-deriving qualifier from `dep_path.last()` / any second name list** —
  rejected; PARSE, DON'T VALIDATE. The manifest is the single enumeration and
  `qual_vars` is *derived* from it, so no two lists can drift.

**From upstream Sky (B) — grafts vs rejects:**

- **Pure-Ipê-source stdlib as the *immediate* #76 fix (B ADOPT)** — **rejected
  as the immediate deliverable, adopted as north-star.** Weighed critically: it
  is genuinely superior on P4/P5/P6 (0 per-member wiring, HM-checked like user
  code) and is what upstream actually does. But the Rust port is kernel-backed
  *today*; compiling the full `Std/{Ui,Html}.ipe` ADT surface needs canon/type/
  lower to ingest List, records, and closures-in-ADT-fields (largely post-M0).
  Shipping it now would block #76 on a multi-milestone dependency. The chosen
  kernel-backed manifest is the **bridge that thins toward pure Ipê**: each
  `Backed` batch can later be retired to source, and the closed-`Backing`
  totality gate is the exact analogue of upstream's closed-ADT `collectStyle`
  exhaustiveness — so nothing built here is thrown away. (The prior synthesis's
  L26 "route each Attribute → a per-attr kernel call" mischaracterises upstream,
  which does CSS emission in *pure-Ipê total case-of*; our per-group runtime
  helper is the kernel-side equivalent, not per-attr.)
- **~5-kernel render/escape boundary + `SafeAttrName` newtype (B ADOPT)** —
  **grafted** (P1). One hand-audited escaper/serializer whose sole constructor
  runs the charset gate + `on*`/`srcdoc` denylist, consumed by *every* sink, is
  a minimal reviewable attack surface and structurally closes the
  forgot-to-escape class.
- **`MAX_HTML_DEPTH` bounded descent + URL-scheme sanitiser + Go-equivalent
  escape set (B ADOPT)** — **grafted** (P1/P3). Model holds attacker-influenced
  data; bounded truncation beats a process-aborting stack overflow; the scheme
  walker catches `java\tscript:` that entity-escaping alone misses.
- **Typed `OnMsg`/`OnInput` handler ADT (B ADOPT)** — **grafted** (P3): handlers
  stay fully typed in `msg`, no `any`, no reflection.
- **`OnRaw String any` / `onSubmit : a -> Attribute msg` untyped laundering
  (B REJECT)** — **rejected** (P3, MAKE INVALID STATES UNREPRESENTABLE). This is
  the one spot upstream trades soundness for convenience — a Haskell/Go-era
  escape hatch where any garbage `a` binds and dispatch is reflect-based. P3
  outranks the P5 convenience. Replaced by typed `OnForm String (FormData→msg)`
  returning `Option<msg>`. Retain a genuinely-opaque escape hatch only if
  clearly named and non-default.
- **Silent `toSnakeCase(mod++"_"++name)` FFI-name fallthrough + dynamic
  `callPure` runtime-panic polyfill as the render path (B REJECT)** —
  **rejected** (P2, our exit-0-then-cargo-fail class, #45/#70). Render-kernel
  names go in an exhaustive closed-enum match; a typo is a `ipe` diagnostic,
  not a deferred cargo failure. The runtime polyfill stays only for the
  genuinely-dynamic FFI boundary.
- **Stringly-typed `__row`/`__grid`/`__gridTracks` layout sentinels +
  `AttrStyle String String` as the *model* (B ADAPT)** — **rejected as the
  model, adapted for parity**. Magic string keys make invalid states
  representable (a user can forge a marker; a typo is silently ignored). Model
  layout intent as typed `Attribute`/`Backing` variants; stringify only at the
  final render step. Record the divergence in `docs/divergences-from-sky.md`;
  do not block parity on it.
- **TH source-text scrape of the constrain table into the registry (B REJECT)** —
  **rejected**; a Haskell-ism irrelevant here (0 Ui/Html entries) and unsound as
  a pattern. One typed manifest SSOT, never regex-scrape source.
