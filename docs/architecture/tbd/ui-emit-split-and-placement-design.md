# `emit_ui_call`: pure/effect split + UI kernel-vs-`.ipe` placement

Status: design proposal, no implementation yet. The Rust and Ipê blocks below
are **illustrative sketches of proposed, not-yet-existing** types and are not
runnable today; existing paths (`emit_ui_call`, the runtime helpers) are quoted
verbatim from the current tree.

This note answers two coupled questions the owner raised about the single
largest function in the compiler:

1. **The deepening** — separate the *pure* "which widget is this, what does it
   emit" classification from the *effectful* string-building, so each is
   testable and reasonable in isolation (item R1/SND1 of
   `misc/disentanglement-opportunities.md`).
2. **The placement question** — should the UI view-builders (`Ui.column`,
   `Ui.text`, `Html` constructors, attributes) be native kernels or pure Ipê,
   per `misc/stdlib-kernel-vs-library.md`?

The two answers reinforce each other: the placement move (stage two) shrinks the
very table the deepening (stage one) builds.

## 1. The problem, measured

`emit_ui_call` (`src/compiler/backend/rust/src/emit_expr.rs:3161`) spans
**~3206 lines** (3161–6367), cyclomatic **240**, **166 return points**, fan_out
**321** — the largest function in the tree and the widest fan-out among
emitters. It dispatches every `Ui` / `Web` / `Tui` / `WebView` / `Cli`
(console) kernel in one `match k { … }` body.

Structurally it interleaves two concerns in every arm:

- **Pure classification** — *which* kernel this is, how many args it takes, and
  *which* runtime path + call shape it maps to.
- **Effectful codegen** — recursively emitting each argument expression
  (`emit_expr_at`) and `format!`-ing the pieces into the output buffer.

Because the two are fused, neither is testable alone: to assert "`Ui.column`
maps to `ipe_runtime::ui::helpers::ui_column_` with two list args" you must run
the whole emitter with a live `EmitCtx`. And a *missing* or *wrong* arm returns
the wrong string with no local signal — the SEAL (ipe-exit-0 ⇒ cargo-build)
catches it only downstream, at the emitted-Rust build, not at the point of the
defect. That is the soundness hazard: 166 exits, no single total function the
type system forces to cover every UI kernel.

### The arm-shape distribution (why this collapses cleanly)

A census of the 226 `KernelFn::` references in the body shows the arms are
overwhelmingly one uniform shape:

| Arm shape | Count | Description |
|---|---|---|
| Destructure `[x]`, emit, one `format!` | 86 | 1-arg widgets (`Ui.text`, `Html.render`, escapes) |
| Destructure `[x,y]`, emit both, one `format!` | 50 | 2-arg containers (`Ui.column`, `Ui.row`, `Ui.el`, `Ui.grid`) |
| Destructure `[x,y,z]` / `[..4]` / `[..5]` / `[..6]` | 10 | wider fixed-arity builders |
| **Uniform "emit each arg, format into one path"** | **~146** | *the mechanical majority* |
| `emit_arc_callback_field` (event-handler peel) | 9 sites | `Ui.onClick`, `Ui.onInput`, inputs with `onChange` |
| `emit_cfg_record_call` (inline-record cfg) | 4 | `Ui.layoutWith`, text inputs, sliders |
| Security seal (`ctx.uses_web` / `Diagnostic::Lower`) | 4 | `Ui.cells` web-shape rejection, etc. |
| Delegations (`emit_web_call`/`emit_tui_call`/…) | 4 | shape-router tail |

The emitted runtime paths are similarly concentrated: **181** of ~205 target
`ipe_runtime::ui::helpers::`, 13 `ui::input::`, 5 `ui::lazy::`, 4 `ui::render::`,
2 `ui::keyed::`. In other words **~90% of the body is a name→(runtime-path,
arity) table written out longhand as `match` arms**, with a thin ring of genuine
logic (callbacks, inline-record extraction, web-shape seals) around it.

## 2. The deepening — `ui_call_shape` (pure) + `emit_ui_plan` (effectful)

### 2.1 The plan data type

Introduce a pure, total classifier `ui_call_shape(k: KernelFn) ->
Option<UiEmitPlan>` that returns `None` for a non-UI kernel (preserving the
current early-return contract) and, for every UI kernel, a `UiEmitPlan`
**describing** what to emit without touching a codegen buffer. The plan is data.
(Illustrative sketch of the proposed type:)

```rust
/// What emitting one UI kernel call produces — pure description, no I/O.
struct UiEmitPlan {
    /// Fully-qualified runtime function the call lowers to,
    /// e.g. "ipe_runtime::ui::helpers::ui_column_".
    path: &'static str,
    /// How the kernel's Ipê args map onto that call's Rust args.
    args: ArgPlan,
    /// Fail-closed guards that must hold before emission (see 2.3).
    guard: Guard,
}

enum ArgPlan {
    /// N positional args, each emitted with emit_expr_at and passed in order.
    /// Covers the ~146 mechanical arms; `arity` is the exact expected count.
    Positional { arity: u8 },
    /// One or more args peeled through emit_arc_callback_field (event handlers)
    /// then the rest positional. Carries which arg indices are callbacks.
    Callbacks { callback_idx: &'static [u8], arity: u8 },
    /// An inline-record cfg arg + trailing positional args
    /// (Ui.layoutWith, text inputs): field names + the record arg index.
    CfgRecord { record_idx: u8, fields: &'static [&'static str], arity: u8 },
    /// The kernel is not emitted here — delegate to another emitter.
    Delegate(UiDelegate),
}

enum UiDelegate { Web, Tui, WebView, Console }

enum Guard {
    None,
    /// Ui.cells and peers: reject in a Web/WebView build, fail-closed.
    RejectInWebShape,
}
```

`ArgPlan::Positional { arity }` alone subsumes ~146 arms. `Callbacks`,
`CfgRecord`, `Delegate`, and `Guard::RejectInWebShape` capture the ~24
non-trivial arms **as data** rather than as bespoke control flow.

### 2.2 The table

`ui_call_shape` is a single `match` whose arms are one-liners returning a
`UiEmitPlan` literal — no `emit_expr_at`, no `format!`, no `ctx`. (Illustrative;
the helper constructors `plan`/`guarded`/`plan_delegate` are proposed, not
existing:)

```rust
KernelFn::UiText   => plan("ipe_runtime::ui::helpers::ui_text_",   Positional { arity: 1 }),
KernelFn::UiColumn => plan("ipe_runtime::ui::helpers::ui_column_", Positional { arity: 2 }),
KernelFn::UiGrid   => plan("ipe_runtime::ui::helpers::ui_grid_",   Positional { arity: 2 }),
KernelFn::UiCells  => guarded("ipe_runtime::ui::helpers::ui_cells_", Positional { arity: 1 }, RejectInWebShape),
KernelFn::UiOnClick=> plan("ipe_runtime::ui::input::ui_on_click_",  Callbacks { callback_idx: &[0], arity: 1 }),
KernelFn::UiLayoutWith => plan("ipe_runtime::ui::render::ui_layout_with_vecs",
                               CfgRecord { record_idx: 0, fields: &["wrapperAttrs","rootAttrs"], arity: 2 }),
KernelFn::WebLink  => plan_delegate(UiDelegate::Web),
```

This is the same "one entry per kernel" descriptor shape the project already
adopted for the 4-shape model and is specifying for the kernel registry (`C2` →
`KernelDef`, below). The table is `const`-friendly and pure.

### 2.3 The interpreter

`emit_ui_plan` is the *only* effectful part — a short interpreter over the plan.
(Illustrative pseudocode:)

```rust
fn emit_ui_plan(
    ctx: &EmitCtx, plan: &UiEmitPlan, callee: &Callee, args: &[Expr],
    on_form: OnFormKind, indent: usize, child: u16, generics: GenericScope,
) -> DResult<Option<String>> {
    // 1. Guard first — fail closed.
    if let Guard::RejectInWebShape = plan.guard {
        if ctx.uses_web || ctx.uses_webview { return Err(ui_cells_web_shape(ctx)); }
    }
    // 2. Arity check once, uniformly (replaces 166 hand-written let-else arms).
    match &plan.args {
        ArgPlan::Delegate(d) => return dispatch_delegate(*d, ctx, callee, args, indent, child, generics),
        a => check_arity(callee, args, a.arity())?,   // one CompilerBug site, not 166
    }
    // 3. Emit each arg by its role, join, format into plan.path.
    let parts = emit_args(ctx, &plan.args, args, indent, child, generics)?;
    Ok(Some(format!("{}({})", plan.path, parts.join(", "))))
}
```

`emit_ui_call` shrinks to: classify, then interpret. (Illustrative:)

```rust
fn emit_ui_call(...) -> DResult<Option<String>> {
    let Callee::Kernel(k) = callee else { return Ok(None); };
    let Some(plan) = ui_call_shape(*k) else { return Ok(None); };
    emit_ui_plan(ctx, &plan, callee, args, on_form, indent, child, generics)
}
```

The 166-exit body becomes a ~200-line pure table + a ~60-line interpreter. The
`CfgRecord` and `Callbacks` machinery already exist as `emit_cfg_record_call` /
`emit_arc_callback_field`; the interpreter calls them by role, so no emission
logic is duplicated or re-derived.

### 2.4 What this makes testable (the new test surface)

- **`ui_call_shape` is a pure total function** → unit-test *per widget* with no
  `EmitCtx`: assert `ui_call_shape(UiColumn)` yields the path
  `ipe_runtime::ui::helpers::ui_column_` with arity 2. ~205 cheap assertions
  that today require a full emit.
- **Exhaustiveness / partition test** (mirrors the `is_json`/`is_crypto`
  partition-test pattern): a test iterating `KernelFn::ALL` asserts *every*
  kernel for which `is_ui()||is_web()||is_tui()||is_webview()||is_console()`
  holds yields `Some(plan)`, and every other kernel yields `None`. This closes
  the soundness hole structurally: a newly-added UI kernel with no plan is a
  **test failure at registration**, not a wrong-shape emit found downstream by
  the SEAL. It is make-invalid-states-unrepresentable applied to the dispatch
  table.
- **Arity coherence test**: cross-check each plan's `arity` against the kernel's
  declared arity in `ipe_lower` — catches a plan/arity drift at test time.
- **Guard coverage test**: assert the `RejectInWebShape` set equals the set of
  kernels with no browser denotation (`Ui.cells` and peers), so a new
  terminal-only widget cannot silently ship a wrong web render.

The interpreter keeps its existing golden coverage; because the emitted strings
are byte-identical, **stage one is a behaviour-preserving refactor** — golden
re-bless is expected to be a no-op (`regen-goldens` clean) except where arm
ordering incidentally changed whitespace, which it should not.

## 3. The placement question — kernels or `.ipe`?

### 3.1 What the builders actually do

The owner's framing is confirmed by the runtime source. The UI builders are
allocation-bound tree construction, run **once per render**, not a compute hot
path. `src/runtime/rust/src/ui/helpers.rs` (verbatim):

```rust
pub fn ui_text_<M>(s: String) -> Element<M> { Element::Text(s) }

pub fn ui_column_<M: Clone>(attrs: Vec<Attribute<M>>, children: Vec<Element<M>>) -> Element<M> {
    let mut full = Vec::with_capacity(attrs.len() + 1);
    full.push(Attribute::AttrStyle("__col".to_owned(), "true".to_owned()));
    full.extend(attrs);
    Element::Node(Description::NoDescription, full, children)
}
```

Every container builder is: prepend one direction-marker `Attribute`, wrap the
children in `Element::Node`. `ui_text_` is a single-variant construction. These
are pure `Element`/`Attribute` **value** constructions — nothing touches the OS,
network, clock, entropy, or a foreign crate. By the placement policy's
capability-vs-computation line, they are squarely **computation/data**, not
capability.

### 3.2 Does the kernel buy any speedup? No.

The work is *allocating a tree node* — a `Vec` push and an enum construction.
A `.ipe` builder that constructs the same `Element`/`Attribute` value via the
type's data constructors compiles, through the existing concrete-over-generic
lowering, to the **same allocation** — a `Vec::with_capacity` + `push` + enum
construct. There is no arithmetic kernel, no SIMD, no vetted parser, no tight
loop the native form wins. The perf-sensitive work — the *diff* and *render* of
the `Element` tree — is already native and stays native (it is a `Web.*` /
reactor capability, not a builder). **The builder is cold, allocation-bound
setup that runs once per view; a kernel gives it no measurable edge over `.ipe`
over the same constructors.** Per the policy's own caveat ("moves *cold*
computation and *data*, not throughput primitives"), the builders are exactly
the cold half.

The precedent is already in-tree: `Ipe.Ui.Grid`, `Ipe.Ui.Chart`,
`Ipe.Ui.Events`, `Ipe.Ui.Transition`, `Ipe.Ui.Transform`, `Ipe.Ui.Responsive`,
`Ipe.Ui.Animation` are **already pure `.ipe`** — they build `Track` ADTs and
`Attribute` values over the `Ui` surface and emit CSS strings, with no kernel.
Only the `Ui` *core* (column/row/el/text/grid/paragraph…) and
`Html`/`Html.Attributes`/`Html.Events` remain kernels. The split is historical
(byte-exact runtime parity), not principled.

### 3.3 Recommendation

**Move the UI view-builders toward `.ipe`, keeping native only the genuine
capability/perf leaves.** Concretely:

- **Move to `.ipe` (data/computation):** `Ui.column`, `Ui.row`, `Ui.wrappedRow`,
  `Ui.el`, `Ui.grid`, `Ui.paragraph`, `Ui.textColumn`, `Ui.text`, `Ui.none`,
  `Ui.html`, and the `Html`/`Html.Attributes` data constructors and `Html.Events`
  attribute builders — everything that only assembles `Element`/`Attribute`
  values. These become pure Ipê over a **small** set of retained core
  constructors.
- **Keep native (capability / genuine leaf):**
  - The `Element` / `Attribute` / `Html` **carrier types + their base
    constructors** (`Element::Node/Text/Empty/Raw/Cells`) — the irreducible
    values the `.ipe` layer builds on.
  - **Event-handler wiring** (`Ui.onClick`, `onInput`, `onSubmit`, file/bool
    events) — these carry the `msg` callback across the reactor boundary
    (`emit_arc_callback_field`, the `Arc<dyn Fn>` carrier); that is a reactor
    capability seam, not pure value construction, and depends on first-class
    function support (see `first-class-functions-design`).
  - **`Html.render` / `Html.escapeText` / `Html.escapeAttr` / `Html.attrToString`**
    — the HTML *serialiser* is a security-sensitive escaper (injection barrier);
    it stays native and vetted per the policy's security-parser carve-out.
  - **`Ui.cells`** web-shape rejection — the fail-closed seal is compiler logic,
    not a builder; it stays in the emitter (as a `Guard`, per stage one).
  - The **diff/render** engine — already native, untouched.

### 3.4 How much of `emit_ui_call` evaporates

Of the ~205 emitted UI paths, **181 route to `ipe_runtime::ui::helpers::`** and
are pure value builders. The event-handler arms (~13 `ui::input::` + the 9
callback sites), the 4 `ui::render::` layout arms, the `Html` serialiser/escaper
arms, and the `Ui.cells` seal are the capability/security leaves that stay.

A conservative reading: the pure-builder arms — the container builders, `text`,
`none`, `html`, keyed variants, and the `Html`/`Attributes` data constructors —
are **roughly 70–80% of the arms** and of the body's line count. Once those
builders live in `.ipe` and are emitted as ordinary Ipê function calls (through
the generic call path, not a bespoke `KernelFn` arm), **~70–80% of
`emit_ui_call`'s remaining table simply disappears** — there is no kernel to
classify, so no plan entry. What is left is the capability ring: event wiring,
layout-cfg records, the HTML serialiser, the web-shape seal, and the shape
delegations — on the order of **40–60 arms**, an emitter of a few hundred lines
rather than 3200.

So the two stages compound: stage one turns the 3200-line body into a
table + interpreter; stage two deletes ~70–80% of the *table*. The endpoint is a
small pure classifier over the genuinely-native UI seam, plus a large,
overridable, no-recompile `.ipe` view library.

### 3.5 The double reduction

Moving the builders to `.ipe` shrinks **two** surfaces at once, both up the
principle order:

- **Security (attack surface):** every builder that stops being a kernel is one
  fewer native function in the audited/attack surface. The native UI surface
  contracts to the carrier constructors + the event/serialiser/seal leaves.
- **Readability + no-recompile:** the builders become editable/overridable Ipê
  (materialise + DCE + packaging, per the placement note's "what makes
  no-recompile real"), and both `emit_ui_call` and the kernel registry
  (`decl`/`capability`/`stdlib_scheme`/two `kernel_name`s) shed ~180 UI rows.

## 4. Sequenced plan

Each stage is independently landable and green under the two-tier gate; each is
behaviour-preserving (golden re-bless expected to be a no-op or trivial).

- **Stage one — extract `ui_call_shape` + `emit_ui_plan`.**
  Introduce `UiEmitPlan` and the pure table; collapse the 166-exit body to the
  table + interpreter; add the per-widget unit tests, the exhaustiveness
  partition test, the arity-coherence test, and the guard-coverage test. **No
  `.ipe` moves, no emission change** — byte-identical goldens. Guardian-reviewed
  (soundness-load-bearing dispatch). This lands the deepening and closes the
  "missing arm returns wrong shape" hazard structurally.

- **Stage two — move pure builders to `.ipe`, in dependency order.**
  Author the `.ipe` view library over the retained core constructors and delete
  the corresponding kernels + their `emit_ui_call` plan entries + all anti-drift
  registry rows. Order (leaves-first, each a landable slice):
  1. **`Html.Attributes` + `Html` data constructors** (no children-recursion,
     no events) — smallest, proves the round-trip; coordinates with the
     `Html.raw`/`Html.render` round-trip work (keep the *serialiser* native,
     move the *constructors*).
  2. **Leaf `Ui` builders** (`Ui.text`, `Ui.none`, `Ui.html`).
  3. **Container builders** (`Ui.el`, `Ui.row`, `Ui.column`, `Ui.grid`,
     `Ui.wrappedRow`, `Ui.paragraph`, `Ui.textColumn`, keyed variants) — the
     bulk of the 181.
  4. **Re-measure** `emit_ui_call` and the kernel-registry row count; confirm
     the ~70–80% table reduction.
  Event wiring, layout-cfg, the HTML serialiser/escaper, and the `Ui.cells` seal
  **stay native** and remain in the (now-small) stage-one table.
  Stage two depends on the materialise/DCE/packaging mechanisms being adequate
  for a large auto-imported `.ipe` layer; if DCE is not yet free-for-unused, the
  move is still correct but pays emitted-binary cost — so slice it after those
  land, or gate each slice on a size check.

## 5. Coordination with the Kernel Row (`KernelDef`) design

A sibling design agent is specifying `KernelDef` — one descriptor row per kernel
(name, capability, arity, type-scheme), deriving `decl` / `capability` /
`stdlib_scheme` / the two `kernel_name`s (C1/C2). `UiEmitPlan` is the
**emit-shape** facet of the same "one entry per kernel" idea, and the two must
not fork:

- `UiEmitPlan.args.arity()` and `KernelDef`'s arity are the **same fact** —
  the stage-one arity-coherence test should assert against `KernelDef` once it
  exists, not a second hand-written arity. Until then it asserts against the
  `ipe_lower` arity table (SSOT today).
- Ideally `UiEmitPlan` becomes a **field on `KernelDef`** for UI kernels
  (`emit: Option<UiEmitPlan>`), so a UI kernel's name, capability, arity,
  scheme, *and* emit shape live in one row — the full make-invalid-states
  -unrepresentable win. Stage one should build `ui_call_shape` so it can later
  be *hosted by* `KernelDef` rather than duplicating it: keep the plan table
  keyed by `KernelFn`, side-by-side, ready to fold in.
- Stage two's kernel deletions shrink `KernelDef`'s row count too — the two
  designs should sequence so the UI-builder rows are removed once, in stage two,
  not migrated into `KernelDef` and then deleted.

## Affected issues

References use bare issue numbers (no tracker prefix) to satisfy doc hygiene.

- **Issue 541** — *direct reuse.* The exhaustiveness partition test in 2.4 is
  the `is_json`/`is_crypto`/`is_secret` bidirectional-partition pattern this
  issue asks to generalise; apply the same shape to `is_ui`/`ui_call_shape`. No
  conflict — same mechanism, new site.
- **Issue 666** — *coordinate, don't collide.* Adds `Html.raw`/`Html.render`
  (Html↔String round-trip). Keep the **serialiser** (`Html.render`, escapers)
  native (§3.3); move only the **constructors** to `.ipe` in stage-two step 1.
  Land 666 first (it adds a kernel), then stage two moves the constructor set
  around the retained serialiser.
- **Issue 665** — *dependency for the event leaf.* First-class functions gate
  the event-handler carrier (`emit_arc_callback_field`, `Arc<dyn Fn>`). Event
  wiring therefore **stays native** (§3.3); do not move `Ui.onClick` et al. to
  `.ipe` until that carrier lands. No conflict — the split keeps events native.
- **Issue 294** — *aligned goal.* Whole-compiler readability/naming audit (P6).
  This design serves it by dissolving the single largest function; the stage-one
  naming (`UiEmitPlan`, `ui_call_shape`, `emit_ui_plan`) should feed the audit.
- **Issue 139** — *downstream beneficiary.* `ipe lint` over `.ipe`; a larger
  `.ipe` UI layer (stage two) is more surface the future linter analyses
  natively rather than as opaque kernels. Positive interference.
- **Issues 663 / 664 / 397** — *same placement lesson, different modules.*
  Codec, Analytics, and the Parser combinator library are the "author in the
  right bucket" cases in the placement note; this design is the worked UI
  instance of the rule they should follow (combinators/data → `.ipe`, capability
  seam → native). No direct code overlap; shared principle.
- **Issue 661** — *SEAL-adjacent, no overlap.* `Ipe.Cache` emits undefined
  symbols (exit-0-then-cargo-fail). The stage-one exhaustiveness test is the
  *pattern* that prevents this class for UI kernels (a resolved-but-unemitted
  kernel becomes a test failure); 661 is the same class in a different
  subsystem — a parallel partition test there is warranted but out of scope.
- **Issue 672** — *drift-class kin.* `Ipe.Random` source-vs-kernel drift is the
  same "kernel recognised but not fully backed" hazard the stage-one
  exhaustiveness test forecloses for UI. Reference only.
- **Issues 333 / 473 / 317** — *web/JS boundary; net-neutral to positive.*
  JS-interop typed boundary, playground Ipê-native rebuild, in-browser
  playground. A larger pure-`.ipe` UI layer is more code the playground can
  compile in-browser without kernel plumbing; the `Ui.cells` web-shape seal
  (kept native) is the kind of boundary discipline 333 wants. No conflict.
- **Issue 284** — *backend-agnostic win.* A direct WASM backend would need its
  own `emit_ui_call` equivalent; a pure-`.ipe` builder layer (stage two) is
  emitted through the *generic* call path, so a second backend inherits the
  builders for free and only re-implements the small capability leaf. Reduces
  284's surface.
- **Issues 396 / 651 / 292 / 671 / 674 / 470 / 641 / 561 / 240** — *no
  interference.* FFI-to-Rust boundary (396/651), sandbox matrix/seccomp
  (292/671/674), hosted ipe-index (470), `Db.open` (641), diagnostics quality
  bar (561), git-history pruning (240) touch neither `emit_ui_call` nor UI
  placement. Listed for completeness; no action.

## Related placement findings — `Ipe.Html` split and CSS-value single source

Two findings from reviewing the Html/Css/Ui render paths, folded into this design.

### `Ipe.Html` follows the same builders-vs-serializer line

`Ipe.Html` is kernel-backed today (`src/runtime/rust/src/html.rs`). Split it exactly as the UI builders split:

- **to `.ipe`:** the `Html` tree constructors and pure attribute helpers — data construction, no I/O.
- **stays a native leaf:** `render_html` and the escapers (`escape_text` / `escape_attr` / `escape_html`) and `assign_ipe_ids`. This is the XSS / injection output-encoding barrier; Security is the top of the precedence order, so the encoder stays native and audited.

The HTML string path is already single-source: an `Element` (the Ui tree) lowers through `ui/render.rs` into `Html` markup nodes, and `html.rs::render_html` is the one serializer and escaper. Ui delegates to it — there is no parallel HTML serializer to unify.

### CSS-value rendering is duplicated — unify onto `Ipe.Css`

Two independent `Length` / `Color` models with two renderers cover the same domain:

- `Ipe.Css` (`.ipe`): opaque `Length` / `Color` built via typed builders, with pure `lengthToString` / `colorToString`.
- `ui/render.rs` (native): its own `Length` / `Color` enums with `length_css` / `color_css`.

They have already drifted — Ui carries layout-intent variants (`Content` / `Fill`); Css carries `em` / `pct` / `ch` / `fr` / `calc` / `minmax` — so the shared raw-value formatting (`Px`, `Vh`, `rgba(...)`) can diverge, a single-source-of-truth violation. When the Ui builders move to `.ipe`, render raw values through `Ipe.Css`'s typed `Length` / `Color` and its renderers, keeping only Ui's genuine layout-intent wrappers on top, and retire the native `length_css` / `color_css`. One CSS-value vocabulary and one renderer for both inline styles and stylesheets.
