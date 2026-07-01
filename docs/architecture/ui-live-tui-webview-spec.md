# Std.Ui / Sky.Live / Sky.Tui / Sky.Webview — authoritative backend-wiring spec

Single source of truth for the locked parallel-executor swarm. Synthesised from
the 4 guardian designs (ui / live / tui / webview). Where the 4 disagreed, this
document records the DECISION and its rationale (PRINCIPLES order
security > correctness > soundness > efficiency > completeness > readability, plus
parse-don't-validate, make-invalid-states-unrepresentable, and SIMPLER-wins).

All paths absolute-from-repo-root `/home/arthur/Documentos/comp/sky-rust`.

Verified ground truth (checked against the tree, not assumed):
- Runtime is written + vetted. `runtime/src/sky_runtime/ui/element.rs` (`Element<M>`,
  `Attribute<M>` + 8 plain enums, INVARIANT pinned at L15), `html.rs` (`Html<M>` L7,
  `Attribute<M>` L22, `Event<M>` L43, `render_html` L152, `SafeAttrName::parse` L332,
  `sanitise_url_attr`), `tui/{app,layout,...}.rs` (`tui_app` String view + `tui_app_ui`
  Element view), `live/mod.rs`, `webview.rs` (`webview_app<...FView: Fn(Model)->Html<Msg>...>`
  L55/L186, `WebviewWindowCfg{title,size}` L38, Cmd dropped via `warn_dropped_cmd_if_real`
  L125). `ui/mod.rs` is deliberately NOT glob-re-exported (its `Attribute` collides with
  `html::Attribute`).
- `runtime/src/sky_runtime/ui/render.rs` does NOT yet exist — it is the new render kernel.
- Crate side is greenfield: `IrType` has opaque precedent `Decoder(Box)`/`Cmd(Box)`/`Sub(Box)`/
  `Db`/`ServerRequest`/`ServerResponse` (ir.rs L461-499) but NO Ui/Element/Html/app-kernel
  wiring. `KernelFn` enum L712, Server cluster L1475-1509, exhaustive `matches!` cluster L1639.
  Module flags `uses_tea` L66, `uses_server` L77 (defaults L2627-2628).
- `plugins/sky-compiler/scripts/flock-edit.sh` provides `acquire`/`release`/`with` per-file locks.

---

## 1. THE SHARED Element/Html CONTRACT (canonical, reconciled)

### 1.1 Decision on IR representation of the UI opaque types

The 4 designs diverged: ui proposed 2 variants (`Ui{ctor,msg}` + `UiPlain(kind)`);
live proposed 6 flat variants + a leaf enum; tui proposed `Html(Box)`+`Element(Box)`;
webview only consumes `html::Html`.

**DECISION — adopt the ui design's two-variant family (prompt-mandated + wins on
make-invalid-states-unrepresentable + SIMPLER):**

```rust
// crates/sky_ir/src/ir.rs — add to enum IrType (after ServerResponse, L499)

/// msg-parametric Std.Ui / Std.Html opaque type. `ctor` selects which runtime
/// enum; `msg` is the M type arg. Illegal combinations are unrepresentable —
/// the tag enum admits exactly the five UI type constructors.
Ui { ctor: UiCtor, msg: Box<IrType> },
/// nullary Std.Ui support enum (no type param).
UiPlain(UiPlain),

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum UiCtor { Html, Element, UiAttribute, HtmlAttribute, HtmlEvent }

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum UiPlain { Length, Color, HAlign, VAlign, Location, PseudoClass, Description, LayoutContext }
```

Rationale: the tag enums make the ~13 UI types a closed, non-extensible set at the
type level (invalid-states-unrepresentable); two `IrType` variants keep the
exhaustive `match` surface small (SIMPLER); it is the option the prompt fixed.

`LiveReq` and `LiveRoute` are NOT UI-msg-parametric enums — they are plain opaque
handles like `ServerRequest`. They get their own dedicated variants (Live-owned):

```rust
LiveReq,     // -> sky_runtime::LiveReq
LiveRoute,   // -> sky_runtime::live::route::Route
```

### 1.2 IrType -> Rust spelling (emit_types.rs::render_type, before the `Fun` arm)

| IrType | Rust spelling |
|---|---|
| `Ui{Html, m}` | `sky_runtime::html::Html<M>` |
| `Ui{Element, m}` | `sky_runtime::ui::element::Element<M>` |
| `Ui{UiAttribute, m}` | `sky_runtime::ui::element::Attribute<M>` |
| `Ui{HtmlAttribute, m}` | `sky_runtime::html::Attribute<M>` |
| `Ui{HtmlEvent, m}` | `sky_runtime::html::Event<M>` |
| `UiPlain(Length)` | `sky_runtime::ui::element::Length` |
| `UiPlain(Color)` | `sky_runtime::ui::element::Color` |
| `UiPlain(HAlign/VAlign/Location/PseudoClass/Description/LayoutContext)` | matching `sky_runtime::ui::element::*` |
| `LiveReq` | `sky_runtime::LiveReq` |
| `LiveRoute` | `sky_runtime::live::route::Route` |

Always render `ui::element::Attribute` and `html::Attribute` by FULLY-QUALIFIED path
(never a bare `Attribute` alias) — they collide, and `ui/mod.rs` is intentionally
not glob-exported. This is soundness trap **T2** (§6).

### 1.3 Runtime types reused AS-IS (no fork, read-only)

`Element<M>` variants (element.rs L157): `Empty | Text(String) | Node(Description, Vec<Attribute<M>>, Vec<Element<M>>) | TaggedNode(String, Description, Vec<Attribute<M>>, Vec<Element<M>>) | Raw(Html<M>)`.

`ui::Attribute<M>` variants (element.rs L103-151, 40 variants): `NoAttribute, AttrWidth(Length), AttrHeight(Length), AttrAlignX(HAlign), AttrAlignY(VAlign), AttrNearby(Location,Element<M>), AttrPadding(i64×4), AttrSpacing(i64), AttrStyle(String,String), AttrDescribe(Description), AttrClass(String), AttrEvent(html::Attribute<M>), AttrAttribute(String,String), AttrFont*(…), AttrBg{Color,Image,Gradient}, AttrBorder{Width,WidthEach(i64×4),Color,Rounded,Style,Shadow(i64×4,Color),InsetShadow(…)}, AttrPointer, AttrOverflow(String,String), AttrPseudoRule(PseudoClass,String), AttrTransition(String,bool), AttrAnimation(String,String,String,bool)`.

`Html<M>` variants (html.rs L7): `HElement(String, Vec<Attribute<M>>, Vec<Html<M>>) | HText(String) | HRaw(String)`.

`html::Attribute<M>` (html.rs L22): `Attr(String,String) | BoolAttr(String,bool) | EventAttr(Event<M>) | NoAttr`.

`Event<M>` (html.rs L43): `OnMsg(String,M) | OnString(String, Arc<dyn Fn(String)->M + Send + Sync>) | OnBool(String, Arc<dyn Fn(bool)->M + Send + Sync>) | OnRaw(String, Arc<dyn Any + Send + Sync>) | OnForm(String, Arc<dyn Fn(FormData)->Option<M> + Send + Sync>)`.

INVARIANT (element.rs L15): runtime variant names + field order MUST stay identical to
`../sky/sky-stdlib/Std/Ui.sky` (L39-190) and `Std/Html.sky`. The opaque alias HIDES drift
— a mismatch mis-renders at runtime instead of failing to build. The Element-render
byte-diff golden (§5) is the ONLY safety net. Any new Std.Ui variant is a lockstep
runtime + mapping change.

### 1.4 Constructors stay pure-Sky; render/layout is a kernel (the pivotal decision)

`Std.Ui` and `Std.Html` are 100% pure Sky (zero `Ffi.kernel`). Every constructor
(`el/row/column/text/button/link/image/input/form/paragraph/textColumn/grid/none/html`,
every `Background/Border/Font/Region/Input/Grid/Transition/Transform/Animation/Responsive/Css`
helper, every `Length`/`Color` builder, every `Html.*`, every `Events.on*`) returns a
clean `Element msg` / `Attribute msg` / `Html msg` / `Event msg` over the mapped runtime
enums. These compile as ordinary Sky — **no kernel, no `any`.**

The ONLY part that cannot be represented soundly in Sky-over-Rust is the render/layout
chain (`renderElement`/`layout`/`layoutWith` return `any` in the stdlib; `Raw any` /
`AttrEvent any` fields). **DECISION (security > correctness > soundness):** port the
render chain to a runtime kernel:

- New `runtime/src/sky_runtime/ui/render.rs`: `ui_layout` / `ui_layout_with`, signature
  `Element<M> -> Html<M>` (Sky sig `layout : List (Ui.Attribute msg) -> Element msg -> Html msg`,
  the stdlib `-> any` sanctioned to `-> Html msg`). Internal helpers `render_node_as`,
  `build_style_string`, `width_css_in`, `height_css_in`, `collect_html_attrs`,
  `pick_semantic_tag`, `tag_for_description` are private to this file — no Sky binding,
  no kernel enum entry.

This (a) removes the `any`-return hazard, (b) co-locates CSS `style="…"` string emission
(`AttrStyle`/`AttrBgImage`/gradients/`AttrAttribute` = user strings entering a CSS/attr
sink) with the audited `render_html` escape gate, and (c) is byte-verified against Go by
goldens. It is simpler AND more secure, so it wins on principle order.

### 1.5 Enum-suppression + ctor-redirect (Maybe/Result mechanism, NOT synthetic EnumDef)

These ADTs are SUPPRESSED (runtime already defines them), not injected like `SqlValue`.
Add the type names to lower.rs's "do not emit a Sky enum def" set (parallel to Maybe/Result).
User constructors/patterns redirect to the runtime variant paths:

**Type-name -> IrType (lower.rs, module-origin-keyed — see T2):**

| (home module, type name) | IrType |
|---|---|
| `(Std.Ui, Element)` | `Ui{Element}` |
| `(Std.Ui, Attribute)` | `Ui{UiAttribute}` |
| `(Std.Ui, Length/Color/HAlign/VAlign/Location/PseudoClass/Description/LayoutContext)` | `UiPlain(_)` |
| `(Std.Html, Html)` | `Ui{Html}` |
| `(Std.Html.Attributes, Attribute)` | `Ui{HtmlAttribute}` |
| `(Std.Html.Events, Event)` | `Ui{HtmlEvent}` |
| `(Sky.Live.*, Request/LiveReq)` | `LiveReq` |
| `(Sky.Live.*, Route)` | `LiveRoute` |

**Constructor-symbol redirect (lower.rs ctor lowering):**

| Sky ctor | Runtime variant path |
|---|---|
| `Empty/Text/Node/TaggedNode/Raw` | `sky_runtime::ui::element::Element::{Empty,Text,Node,TaggedNode,Raw}` |
| `NoAttribute … AttrAnimation` (40) | `sky_runtime::ui::element::Attribute::*` |
| `Px/Fill/FillPortion/Content/Shrink/Minimum/Maximum/Vh/Vw` | `sky_runtime::ui::element::Length::*` |
| `Rgba` (+ rgb/white/black/transparent helpers build it) | `sky_runtime::ui::element::Color::*` |
| `AlignLeft/…`, `Above/Below/…`, pseudo/desc/layout ctors | matching `sky_runtime::ui::element::*` |
| `HElement/HText/HRaw` | `sky_runtime::html::Html::*` |
| `Attr/BoolAttr/EventAttr/NoAttr` | `sky_runtime::html::Attribute::*` |
| `OnMsg/OnString/OnBool/OnRaw/OnForm` | `sky_runtime::html::Event::*` |

Event handlers box as `Arc::new(move |…| …)` (capturing closures), matching the
`OnString`/`OnBool`/`OnForm` precedent — never bare fn-pointers (trap **T6**).

### 1.6 Render kernels (the shared render surface)

| Sky binding (constrain.rs kernel_ty) | KernelFn | runtime fn (ui/render.rs) |
|---|---|---|
| `layout : List (Ui.Attribute msg) -> Element msg -> Html msg` | `UiLayout` | `ui_layout` |
| `layoutWith : { wrapperAttrs, rootAttrs } -> Element msg -> Html msg` | `UiLayoutWith` | `ui_layout_with` |
| `htmlRender : Html msg -> String` | `HtmlRender` | `html_render_` (exists) |
| `htmlEscapeText : String -> String` | `HtmlEscapeText` | `html_escape_text_` (exists) |
| `htmlEscapeAttr : String -> String` | `HtmlEscapeAttr` | `html_escape_attr_` (exists) |
| `htmlAttrToString : Html.Attribute msg -> String` | `HtmlAttrToString` | `html_attr_to_string_` (exists) |

### 1.7 View type by surface (hard contract point — trap T4)

- **Live** view: `Model -> Html msg` (`FView: Fn(Model)->Html<Msg>`, mod.rs). User wraps
  `Ui.layout [] (…)` inside `view` to convert Element -> Html.
- **Webview** view: `Model -> Html msg` (`webview_app` `FView: Fn(Model)->Html<Msg>`, webview.rs L68/L199). Same `Ui.layout` wrap convention.
- **Tui** view: `Model -> Element msg` — consumed RAW by `tui_app_ui` via
  `tui::layout::render_with_focus`; NO `Ui.layout` wrap (Tui does its own cell layout).

---

## 2. SHARED-REGISTRY ADDITIONS (deconflicted union, additive, non-overlapping)

These files are orchestrator-integrated. **DECISION: all shared-file additions land in
the FOUNDATIONAL (sequential) phase (§4), including every surface's KernelFn variants and
their naming/pretty/emit/constrain arms** — because a `KernelFn` variant without its
exhaustive-match arms will not compile, the variants + arms are indivisible. The parallel
phase then touches ONLY disjoint per-surface files. Any unavoidable residual shared touch
goes through `flock-edit.sh` (§6). Below is the exact union so nothing overlaps.

### `crates/sky_ir/src/ir.rs`
- `enum IrType` (after L499): `Ui { ctor: UiCtor, msg: Box<IrType> }`, `UiPlain(UiPlain)`,
  `LiveReq`, `LiveRoute`; plus tag enums `UiCtor`, `UiPlain`.
- `enum KernelFn` (app-surface block, contiguous, after Server cluster): `UiLayout`,
  `UiLayoutWith`, `HtmlRender`, `HtmlEscapeText`, `HtmlEscapeAttr`, `HtmlAttrToString`,
  `LiveApp`, `LiveAppRouted`, `LiveRoute`, `LiveRenderStatic`, `TuiProgram`, `TuiApp`,
  `WebviewApp` (13 variants).
- `impl KernelFn`: extend the exhaustive `matches!` cluster at L1639 with all 13 (or add
  `is_live()` / `is_tui()` / `is_webview()` predicates mirroring `is_server`). Do NOT
  fold Ui/Tui/Live/Webview into `is_tea()`. **No catch-all.**
- `struct Module`: add `pub uses_ui`, `pub uses_live`, `pub uses_tui`, `pub uses_webview`
  bools (beside `uses_server` L77); default all `false` (L2627-2628 constructor).

### `crates/sky_ir/src/pretty.rs`
- Exhaustive `kernel_name` arms for the 13 KernelFns (`UiLayout => "Ui.layout"`, …,
  `WebviewApp => "Webview.app"`); pretty arms for `IrType::Ui`/`UiPlain`/`LiveReq`/`LiveRoute`.
  No `_ =>`.

### `crates/sky_types/src/constrain.rs` (`kernel_ty`)
- Add TYPE entries keyed by **`(home, name)`** (memory: multi-module `constrain` bare-Symbol
  keying is a known blocker — MUST key by module here so `Ui.layout` never conflates with a
  user `layout`): `(Std.Ui, layout)`, `(Std.Ui, layoutWith)`, `(Std.Html, render/escapeHtml/escapeAttr/attrToString)`,
  `(Live, app/route/renderStatic)`, `(Tui, program/app)`, `(Webview, app)`.
- App cfg records return real `Ty::Record` with function-typed fields; return `Task Error ()`.
  Tui cfg: 5 closed required fields + row-open tail for optional `guard/canvasWidth/canvasHeight`;
  `onKey` param structurally required to be `{ kind : String, value : String }`.
- Pre-intern field symbols: `init update view subscriptions onKey routes notFound head consoleAuth window title size kind value wrapperAttrs rootAttrs`.

### `crates/sky_backend_rust/src/naming.rs`
- `UiLayout => "ui_layout"`, `UiLayoutWith => "ui_layout_with"`, `HtmlRender => "html_render_"`,
  `HtmlEscapeText => "html_escape_text_"`, `HtmlEscapeAttr => "html_escape_attr_"`,
  `HtmlAttrToString => "html_attr_to_string_"`, `LiveApp => "live_app"`,
  `LiveAppRouted => "live_app_routed"`, `LiveRoute => "route::Route::new"`,
  `LiveRenderStatic => "live_render_static"`, `TuiProgram => "tui_app"`, `TuiApp => "tui_app_ui"`,
  `WebviewApp => "webview_app"`. Exhaustive; keep the match total (guard, no catch-all).

### `crates/sky_backend_rust/src/emit_types.rs` (`render_type`, L68)
- Arms for `IrType::Ui{…}` (5 ctor cases -> qualified runtime names, §1.2), `IrType::UiPlain(_)`,
  `IrType::LiveReq`, `IrType::LiveRoute`. Place before the generic `Fun` arm. Fully-qualified
  paths for both `Attribute` families (T2).

### `crates/sky_lower/src/lower.rs`
- `ir_type_from_canon` (L1179 region): module-origin-keyed type mapping table (§1.5) —
  resolve `Symbol -> home module`, never bare `resolve(name)` (T2).
- Ctor-symbol redirect table (§1.5) + enum-def suppression set additions.
- Kernel resolve (`(module,name)` table): `(Ui,layout)->UiLayout`, `(Ui,layoutWith)->UiLayoutWith`,
  `(Html,render)->HtmlRender`, `(Html,escapeHtml)->HtmlEscapeText`, `(Html,escapeAttr)->HtmlEscapeAttr`,
  `(Html,attrToString)->HtmlAttrToString`, `(Live,app)->LiveApp` (routed-vs-single decided in emit),
  `(Live,route)->LiveRoute`, `(Live,renderStatic)->LiveRenderStatic`, `(Tui,program)->TuiProgram`,
  `(Tui,app)->TuiApp`, `(Webview,app)->WebviewApp`.
- Feature-flag setters: `expr_uses_ui_kernel` / `expr_uses_live_kernel` / `expr_uses_tui_kernel` /
  `expr_uses_webview_kernel` (mirror `expr_uses_server_kernel`), set `module.uses_*` on any
  reachable matching call. Conservative reachability, not literal position.

### `crates/sky_backend_rust/src/emit_expr.rs`
- Four ONE-LINE dispatch arms (+ 4 `mod` decls) delegating to the per-surface emit files:
  `KernelFn::{UiLayout,UiLayoutWith,HtmlRender,HtmlEscapeText,HtmlEscapeAttr,HtmlAttrToString} => emit_ui::…`,
  `KernelFn::{LiveApp,LiveAppRouted,LiveRoute,LiveRenderStatic} => emit_live::…`,
  `KernelFn::{TuiProgram,TuiApp} => emit_tui::…`, `KernelFn::WebviewApp => emit_webview::…`.
  All bodies live in the disjoint per-surface files (§3). Exhaustive, no `_ =>`.

### `crates/sky_canon/src/env.rs` (`QUALIFIERS`, L182 region)
- `("Ui", &[el,row,column,wrappedRow,el,text,button,link,image,input,form,paragraph,textColumn,grid,gridColumns,none,html,layout,layoutWith,rgb,rgba,white,black,transparent,px,fill,fillPortion,content,shrink,minimum,maximum,vh,vw,padding,paddingXY,paddingEach,spacing,centerX,centerY,alignLeft,alignRight,alignTop,alignBottom,pointer,clip,scrollbars,above,below,onLeft,onRight,inFront,behind,htmlAttribute,onClick,onSubmit,onInput,onChange,onFocus,onMouseOver,onMouseOut,onKeyDown,mediaQuery,breakpoint,aspectRatio,onPseudo,hover,focus,focusVisible,active,disabled,mobile,tablet,desktop,darkMode,…])`
- `("Html", &[node,text,raw,div,span,a,…])`, `("Attr", &[…])`, `("Event", &[onClick,onInput,onSubmit,…])`
- `("Live", &[app,route,renderStatic,api,lifecycle])`, `("Tui", &[app,program])`, `("Webview", &[app,WindowCfg,AppCfg,defaultWindow,withTitle,withSize])`
- Submodule qualifiers: `Background Border Font Region Input Grid Transition Transform Animation Responsive Css Keyed Lazy Chart`.
- Note: `Std.Ui`/`Std.Html`/`Std.Html.Events` are compiled from stdlib source; confirm the
  sky-rust build embeds `Std/Ui.sky` et al. (the way `../sky` does). Constructors resolve by
  import; only the app/render kernel qualifiers are strictly needed in QUALIFIERS.

### `crates/sky_backend_rust/src/preamble.rs`
- Emitted-crate alias/`pub use` block for the UI runtime names. Qualify `ui::Attribute`
  (do NOT glob-import it — `html::Attribute` collision).
- **Entry-driver selection (SHARED, §4):** `fn main` epilogue must emit
  `block_on_current_thread(sky_main())` when `module.uses_webview` (tao/Cocoa/GTK main-thread
  requirement, webview.rs L16). All other surfaces keep the existing spawned-thread `block_on`.
  Tui stays on the default driver for v1 (it already runs correctly under it); revisit only if
  the Tui executor proves otherwise. One branch, keyed on `uses_webview`.

### `crates/sky_backend_rust/src/project.rs`
- `maybe_add_ui_feature` / `maybe_add_live_feature` / `maybe_add_tui_feature` /
  `maybe_add_webview_feature`, mirroring `maybe_add_server_feature`, reusing the closing-`]`
  anchor splice. Each adds its feature name to the emitted project's `sky_runtime` dep feature
  list, and appends the needed `pub mod …` lines to the emitted `mod.rs`.
  - `live` pulls `["server", http_client, tokio net/sync/rt-multi-thread, tower-http, serde, futures]`.
  - `tui` pulls `["unicode-width","crossterm","tokio"]` + `tea` module/aliases.
  - `webview` pulls `["wry","tao","live"]` (transitive — declared in runtime/Cargo.toml).
  - **Dedup rule:** the feature list is a set of strings — when `uses_webview` and `uses_live`
    both hold, add both names once; never emit a duplicate `"live"` entry (webview's transitive
    `live` must not be double-declared). Reuse the anchor logic with a set, not append-blind.

### module-flags plumbing (skyc / driver)
- Thread `uses_ui/uses_live/uses_tui/uses_webview` from lowerer -> backend ctx wherever
  `uses_server`/`uses_tea` are threaded.

---

## 3. DISJOINT PER-SURFACE FILE PARTITION (no two surfaces own the same non-shared file)

### Std.Ui (foundational surface)
- `runtime/src/sky_runtime/ui/render.rs` (NEW) — `ui_layout`/`ui_layout_with` + private
  `render_node_as`/`build_style_string`/`width_css_in`/`height_css_in`/`collect_html_attrs`/
  `pick_semantic_tag`/`tag_for_description`. The render engine. Security-critical (T3, T5).
- `runtime/src/sky_runtime/ui/mod.rs` — ONE additive line `pub mod render;` (Std.Ui-owned edit).
- `crates/sky_backend_rust/src/emit_ui.rs` (NEW) — emit bodies for `UiLayout`/`UiLayoutWith`
  + the 4 Html kernels.
- `crates/sky_backend_rust/tests/golden/ui_render/**` + `crates/skyc/tests/golden_stdui_render.rs`.

### Sky.Live
- `crates/sky_backend_rust/src/emit_live.rs` (NEW) — `LiveApp`/`LiveAppRouted`/`LiveRoute`/
  `LiveRenderStatic` peepholes: app record-splice (single-page vs routed via literal `page`
  field detection, `set_page` closure, store cfg, init-arg adapt), form-event peepholes.
- `crates/skyc/tests/live_e2e.rs` + `crates/skyc/tests/goldens/live/**`.
- Live writes NO runtime code (reuses live/* as-is).

### Sky.Tui
- `crates/sky_backend_rust/src/emit_tui.rs` (NEW) — `TuiProgram`/`TuiApp` cfg destructure +
  the onKey `{kind,value}` adapter closure (fail-closed typed diagnostic on any other shape).
- `runtime/tests/tui_render_golden.rs` (NEW) — `render_with_focus` ANSI-cell byte-diff.
- `crates/skyc/tests/golden/m7_tui_program/**` + `crates/skyc/tests/golden/m7_tui_app_element/**`.
- Tui writes NO runtime code (reuses tui/* as-is).

### Sky.Webview
- `crates/sky_backend_rust/src/emit_webview.rs` (NEW) — `emit_webview_app`: destructure the
  single `AppCfg` record into the 5 positional runtime args + `WebviewWindowCfg{title,size}`;
  fail-closed non-literal-cfg gate (T-WV3).
- `runtime/src/sky_runtime/webview.rs` — Webview-OWNED runtime edit: close the Cmd/Sub dispatch
  gap (`UserEvent::Msg(Msg)` + tokio multi-thread handle + `event_loop.create_proxy()` +
  interval subs; wire `subscriptions`). Linux wry/tao GTK path verification.
- `crates/sky_backend_rust/tests/golden/webview_app/**` + webview.rs `#[cfg(test)]` render byte-diff.
- `examples/*-webview-*` xvfb smoke fixture.

**Partition check:** `ui/render.rs` (Ui), `webview.rs` (Webview) are the only runtime edits and
they are distinct files; `emit_ui.rs`/`emit_live.rs`/`emit_tui.rs`/`emit_webview.rs` are four
distinct files; each surface's goldens live under distinct dirs. `ui/mod.rs` and `emit_expr.rs`
`mod` lines are the only shared touch — done in the foundational phase. No collisions.

---

## 4. INTEGRATION ORDER (freeze the contract, then fan out)

### Phase 0 — FOUNDATIONAL (sequential, single executor, FREEZE before fan-out)
Land, in this order, and FREEZE the shared registries:
1. `ir.rs`: `IrType::Ui`/`UiPlain`/`LiveReq`/`LiveRoute` + tag enums; all 13 `KernelFn`
   variants; exhaustive `impl` clusters; 4 `uses_*` module flags + defaults.
2. `emit_types.rs`: `render_type` arms (qualified runtime names, T2).
3. `lower.rs`: module-origin-keyed type mapping (T2) + ctor redirect + enum suppression +
   kernel resolve + 4 `expr_uses_*` feature setters.
4. `constrain.rs`: `kernel_ty` entries keyed `(home,name)` for ALL surfaces.
5. `naming.rs` + `pretty.rs`: exhaustive arms for all 13 KernelFns + new IrTypes.
6. `preamble.rs`: UI alias block + entry-driver selection; `env.rs`: qualifiers;
   `project.rs`: the 4 feature injectors + dedup.
7. `emit_expr.rs`: 4 one-line dispatch arms + 4 `mod` decls (bodies are stubs returning a
   `CompilerBug`/`todo`-free typed error until their per-surface file lands — but land the
   Std.Ui `emit_ui.rs` body in this phase, see 8).
8. **`runtime/src/sky_runtime/ui/render.rs`** — port `renderElement`/`layout`/`buildStyleString`
   (T1, T3, T5); wire `ui/mod.rs`; land `emit_ui.rs`.

**FREEZE GATE (go/no-go for fan-out):** the Std.Ui constructor golden + the backend-independent
Element-render byte-diff golden (§5.1/§5.2) MUST pass, AND the whole workspace must `cargo build`
clean with the new exhaustive matches. Only after this gate do the shared registries freeze and
parallel work begin. Rationale: the Element/Html contract is FOUNDATIONAL — Live/Tui/Webview all
consume `Ui{…}`/`UiPlain`, the render kernel, and the view-type contract; a moving contract during
fan-out is an exit-0-then-cargo-fail generator.

### Phase 1 — PARALLEL FAN-OUT (three executors, disjoint files only)
After freeze, Live / Tui / Webview run concurrently, each writing ONLY its disjoint files (§3):
- Live: `emit_live.rs` + live goldens.
- Tui: `emit_tui.rs` + `tui_render_golden.rs` + tui goldens. (`TuiProgram` has NO Std.Ui
  dependency and can even start during Phase 0; `TuiApp` needs the frozen Element contract.)
- Webview: `emit_webview.rs` + `webview.rs` Cmd/Sub close + webview goldens + xvfb smoke.

No shared-file writes in Phase 1 under normal execution (all shared arms landed in Phase 0). If
a genuinely new shared touch is discovered, it goes through `flock-edit.sh` (§6).

---

## 5. PER-SURFACE GOLDEN / TEST PLAN (deterministic, honest)

Honest-test rule (repeated memory lesson): every golden MUST include the DIVERGING reps
(URL schemes, `data:` images, CSS `url(javascript:…)`, void elements, `<textarea value>`,
keyed reorder, empty collections, route-arity underflow) — not just a clean `<div>` — or the
XSS/style/bounds gates are untested.

### 5.1 Std.Ui constructor golden (`golden_stdui_render.rs`)
Fixtures exercising every constructor + submodule attr -> compile -> run -> emitted HTML string
byte-equals the Go oracle (`../sky`). Security probes REQUIRED: `AttrBgImage "url(javascript:…)"`,
`Ui.htmlAttribute "onclick" "x"`, `link {url="javascript:alert(1)"}` -> assert sanitised/dropped.

### 5.2 Element-render byte-diff golden (SHARED FOUNDATION sentinel)
`Element` tree -> `ui_layout` -> `Html` -> `render_html`, backend-independent. Locks the
variant-name mapping (§1.5). Tui/Webview inherit a proven contract. THIS is the freeze-gate
sentinel; also the sentinel for INVARIANT drift (element.rs L15).

### 5.3 Live initial-render byte-diff (`live_e2e.rs`)
Fixed Model -> `view` -> full page (`<!DOCTYPE>` + wrapper + `<style>` + body) byte-equals Go,
on: counter (single-page `live_app`), routed app (`live_app_routed` + `set_page`),
form-with-password (`OnForm` decode). Deterministic (path-based `sky-id`, no clock/RNG in render).
Adversarial: `AttrAttribute "onerror" "x"`, `href="javascript:…"`, `HText "<script>"` -> escaped;
route with fewer `:params` than ctor arity -> `""` fill, no panic.

### 5.4 Tui ANSI-cell byte-diff (`tui_render_golden.rs`)
Fixed `ui::Element<TestMsg>` tree -> `render_with_focus` at `(80,24)` -> ANSI frame byte-equals
the checked-in fixture (non-TTY falls back to 80×24, deterministic, no PTY). Plus Go-parity vs
`runtime-go/rt/tui_ui.go` for the same tree (record sanctioned divergence if any).

### 5.5 Webview initial-render byte-diff (`webview.rs #[cfg(test)]`)
`render(&view,&model)` on a fixed view -> `(body,index)` HTML byte-exact. Shares the `render_html`
oracle with Live, so Live≡Webview initial HTML is provable by comparing both goldens. Plus emit
golden: `webview_app(` 5-arg destructure + `WebviewWindowCfg{…}` + current-thread driver +
`Cargo.toml` contains `"webview"` (no `"live"` double-entry).

### 5.6 Interaction / boot (where interaction matters)
- Live browser round-trip (chromium + xvfb): click `+` -> SSE patch flips count; form submit ->
  `OnForm` decodes typed record; route nav -> `set_page`. Negative: malformed form -> no Msg, no
  crash (fail-closed). Assert via diff/patch, not screenshot pixels.
- Webview IPC round-trip (headless, no window): synthetic IPC JSON `{skyId,event,args}` ->
  `parse_ipc -> index.resolve -> update` -> assert model transition. Plus wry/tao Linux xvfb boot
  smoke: build `--features webview` against `webkit2gtk-4.1`, run under `xvfb-run -a` with a
  bounded self-close (`SKY_WEBVIEW_SMOKE_EXIT_MS`), assert exit 0 + no panic/link error. Plus a
  Cmd/Sub dispatch test (instrumented `UserEvent::Msg` counter behind `#[cfg(test)]`).
- Tui PTY interaction (follow-on, optional): scripted keystrokes under `portable-pty`,
  timeout-bounded, `kill -KILL` on timeout.

### 5.7 Feature-injection + build gate
UI-only, Live, Tui, Webview fixtures each produce a `Cargo.toml` with exactly the right feature
set (assert no `live` double-declare under `webview`). Every fixture must `skyc`-emit AND
`cargo build` (bounded per-fixture; no full local sweep — push to CI). Green skyc + red cargo is
a BLOCKING failure (exit-0-then-cargo-fail).

---

## 6. SOUNDNESS + ANTI-RACE PROTOCOL

### Anti-race protocol
- Shared files (§2) are written in Phase 0 by ONE executor; frozen before fan-out. Any residual
  shared touch in Phase 1 MUST wrap the Edit/Write in
  `plugins/sky-compiler/scripts/flock-edit.sh acquire <abs-path>` … `release <abs-path>`
  (or `with <abs-path> -- <cmd>`). Disjoint per-surface files (§3) need no lock.
- Exhaustive kernel dispatch: every new `KernelFn`/`IrType` variant gets a real arm in
  `naming.rs`/`pretty.rs`/`emit_types.rs`/`emit_expr.rs`/`lower.rs`; guard with a hard error,
  NEVER `_ =>` (project non-regression rule). A missing arm is a compile error by construction.
- Feature injection is load-bearing against exit-0-then-cargo-fail: `uses_*` set on any reachable
  kernel call -> project.rs injects the feature. A UI/Live/Tui/Webview program that doesn't get its
  feature is a green-skyc / red-cargo failure. The §5.7 gate is the sentinel.

### Soundness rules
- `#![forbid(unsafe_code)]` stays clean on all compiler crates. No new `unsafe`. `crossterm`/`wry`/
  `tao`/`webkit2gtk` vendored `unsafe` is dependency-internal, not ours.
- No `unwrap`/`expect`/`panic!`/`todo!`/`unreachable!` on reachable paths. `ui/render.rs` must have
  no raw indexing, no `unwrap`, no narrowing `as`; the `AttrBorderWidthEach(i64×4)` sum uses
  `saturating_add` (T5 — debug-overflow panic per prior ui-element gate). Route param access is
  `.get(i).cloned().unwrap_or_default()`, never `params[i]`. Form decode stays `Option<M>` fail-closed.
- Errors are typed `Result Error` / `Task Error` — never `Result String`. No `.(T)`; use `rt.Coerce`
  precedent. Record field enumeration by `_fieldIndex`.

### Blocking soundness traps (block-on-sight)
- **T1 (`any`-render):** never compile `renderElement`/`layout` from Sky; never add `IrType::Any`/
  `Box<dyn Any>` (defeats `Html<M>: PartialEq` the diff needs, reintroduces coercion panics). Render
  is the `ui_layout` kernel. `Raw`/`AttrEvent` args flow as concrete `Html<M>`/`html::Attribute<M>`.
- **T2 (Attribute/Event name collision):** `resolve(name)` returns bare `"Attribute"` for BOTH
  `Std.Ui` and `Std.Html.Attributes`, and `"Event"` for `Std.Html.Events`. Key type mapping on
  MODULE ORIGIN. Render both `Attribute` families by fully-qualified path. Compounds with the open
  multi-module `constrain` bare-Symbol blocker (memory) — key `kernel_ty` by `(home,name)` in lockstep.
- **T3 (CSS injection in `build_style_string`):** `AttrStyle`/`AttrBgImage`/gradient/`AttrAttribute`
  are user strings entering `style="…"`. Fold them and let `render_html`'s `escape_attr`/
  `sanitise_url_attr` gate; never emit a bespoke style attribute that bypasses the sink; neutralise
  `url(javascript:…)` / `expression(…)`.
- **T4 (view type by surface):** Tui view = `Model -> Element msg` (raw); Live/Webview =
  `Model -> Html msg` (via `Ui.layout` wrap). Wire the correct runtime entry per surface.
- **T5 (border-width overflow):** `saturating_add` the `AttrBorderWidthEach` sum.
- **T6 (event-handler boxing):** handlers box as `Arc<dyn Fn + Send + Sync>` (capturing), not
  fn-pointers, or `onChange = \s -> …` apps fail to compile.
- **Live init-arg adapt:** default to `()`-discarding wrap; pass-through only when init param-0 solves
  to `LiveReq` (else `FInit: Fn(LiveReq)` unsatisfied -> cargo-fail).
- **Live `page`-field detection is name-literal:** routed mode keys on a field named exactly `page`;
  read Model fields from `view`'s solved type, not a heuristic.
- **Tui onKey adapter:** build the nominal `{kind,value}` struct from onKey's param type via
  type-directed emit; on any other shape raise a typed pre-cargo diagnostic (fail-closed), never a
  guessed struct. v1 rejects optional `guard`/`canvasWidth`/`canvasHeight` with a clear diagnostic
  (SIMPLER, fail-closed) rather than silently dropping.
- **Tui.app gating:** `TuiApp` (Element view) must not emit before the Std.Ui contract is frozen
  (Phase 0), else it references unwired constructors -> green skyc / red cargo. `TuiProgram` (String
  view) is Std.Ui-independent and may ship first.
- **Webview main-thread (T-WV1):** emit `block_on_current_thread` when `uses_webview`; spawned-thread
  `block_on` for a webview program aborts tao/Cocoa/GTK. Fail SAFE — if in doubt, current-thread.
- **Webview Cmd/Sub (T-WV2):** must land `UserEvent::Msg` + tokio-handle dispatch + interval subs;
  do NOT sign off with effects dropped (correctness violation vs §5 loop contract).
- **Webview non-literal cfg (T-WV3):** `emit_webview_app` assumes a resolvable record; a non-literal
  `Webview.app cfgVar` must fail-closed with a typed diagnostic, never a mis-projection. (Literal-gate
  for v0.1; struct-field path a follow-up — the parametric `AppCfg_R[Model,Msg]` route re-opens the
  `Foo_R[any]`-cast-panic family, so prefer literal for now.)

---

## Filed rewrite-opportunity items (legacy, do NOT rewrite as part of this wiring)
1. `runtime/src/sky_runtime/tui/app.rs` — stringly-typed key wire protocol
   (`on_key: Fn(String,String)->Msg`, `kind == "mouse"` / `value.contains('Z')` substring tests).
   Proposed: thread the existing `key.rs` `TuiKey` ADT typed through the channel; convert
   `TuiKey -> KeyEvent` at the single Sky boundary.
2. `runtime/src/sky_runtime/ui/element.rs` — `AttrAttribute(String,String)`/`AttrStyle(String,String)`
   carry attacker-influenceable name+value as bare strings; safety rests on a documented render-time
   contract. Proposed: a `SafeAttrName` newtype smart constructor at the `htmlAttribute`/`style`
   kernel boundary so an unsafe name is unrepresentable in the Element tree.
3. `runtime/src/sky_runtime/webview.rs::parse_ipc` — loose `(String,String,Vec<String>)` IPC triple.
   Proposed: parse once into `IpcEvent { sky_id: SkyId, event: WireEvent, args: WireArgs }` with
   `WireEvent` an ADT mirroring the HandlerIndex wire-arg table.
