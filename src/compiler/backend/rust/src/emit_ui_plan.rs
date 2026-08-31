//! Pure classification of a `Ipe.Ui` / `Ipe.Html` / `Web` / `Tui` / `WebView` /
//! console kernel into a [`UiEmitPlan`] — a description of what emitting the
//! call produces, carrying no codegen buffer and touching no [`EmitCtx`].
//!
//! [`ui_call_shape`] is a total function over [`KernelFn`]: `Some(plan)` for
//! every UI-family kernel, `None` for anything else. The interpreter that turns
//! a plan into emitted Rust lives in [`crate::emit_expr::emit_ui_plan`]; the two
//! together replace the single `match` that previously fused classification and
//! codegen in one arm per kernel.
//!
//! The split makes the dispatch a **total** function the type system forces to
//! cover every UI kernel: a kernel that is UI-family yet yields no plan is a
//! test failure at the classifier (see `exhaustiveness_partition`), not a
//! wrong-shape emission discovered downstream when the emitted Rust fails to
//! build.
//!
//! The uniform majority — a call to one runtime path with N positionally
//! emitted arguments — is [`ArgPlan::Positional`], pure data (a path string and
//! an arity). The capability and security leaves — event-handler wiring, inline
//! record configs, the `Html` serialiser, the `Ui.cells` web-shape seal, the
//! deferred-subtree eta wrappers, and the shape-router delegations — carry too
//! much bespoke emission to encode as data without reproducing it byte for
//! byte; each is named by an [`ArgPlan::Native`] tag the interpreter dispatches
//! to a dedicated emitter.
//!
//! The table is keyed by [`KernelFn`] and lives beside the enum so a UI
//! kernel's emit shape can later be hosted as a field on its descriptor row
//! rather than duplicated in a second table.

use ipe_ir::KernelFn;

/// What emitting one UI-family kernel call produces — a pure description with no
/// I/O and no codegen buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UiEmitPlan {
    /// How the kernel's Ipê arguments map onto the emitted Rust call.
    pub args: ArgPlan,
    /// A fail-closed guard that must hold before emission.
    pub guard: Guard,
}

/// How a kernel's arguments become the emitted call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArgPlan {
    /// `arity` arguments, each emitted in order and passed positionally to
    /// `path`, i.e. `path(a0, a1, …, a{arity-1})`. `arity == 0` emits `path()`.
    ///
    /// `path` is the fully-qualified runtime function, e.g.
    /// `"ipe_runtime::ui::helpers::ui_node_"`.
    Positional { path: &'static str, arity: u8 },
    /// The kernel's emission is bespoke — a callback carrier, an inline record
    /// config, the HTML serialiser, a predicate-keyed tag/attribute family, a
    /// deferred-subtree eta wrapper, or a shape-router delegation. The
    /// interpreter dispatches on the tag to the matching emitter.
    Native(NativeUiEmit),
}

/// The bespoke emitters the interpreter dispatches to for the capability and
/// security leaves. Each variant corresponds to one emitter in
/// [`crate::emit_expr`]; the classifier only names which one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NativeUiEmit {
    /// `Ui.layoutWith` — inline `{ wrapperAttrs, rootAttrs }` record config.
    LayoutWith,
    /// `Html.render` / `Html.toString` — the HTML serialiser (`render_html`).
    HtmlSerialise,
    /// `Ui.button` — inline `{ onPress, label }` record config.
    Button,
    /// `Ui.link` — inline `{ url, label }` record config.
    Link,
    /// `Ui.image` — inline `{ src, description }` record config.
    Image,
    /// `Ui.paddingEach` — inline `{ top, right, bottom, left }` record.
    PaddingEach,
    /// `Border.widthEach` — inline `{ top, right, bottom, left }` record.
    BorderWidthEach,
    /// `Border.shadow` — inline `{ offsetX, offsetY, blur, spread, color }`.
    BorderShadow,
    /// `Border.innerShadow` — inline `{ offsetX, offsetY, blur, spread, color }`.
    BorderInnerShadow,
    /// `Input.text` and its type siblings (email, username, search, passwords).
    InputText,
    /// `Input.multiline`.
    InputMultiline,
    /// `Input.checkbox`.
    InputCheckbox,
    /// `Input.slider`.
    InputSlider,
    /// `Input.radio`.
    InputRadio,
    /// `Input.radioRow`.
    InputRadioRow,
    /// `Html.voidNode` — a runtime-tag void element (empty children vec).
    HtmlVoidNode,
    /// `Ui.onInput` — string-carrying event handler, peel-hoisted Arc callback.
    OnInput,
    /// `Ui.onChange` — string-carrying event handler, peel-hoisted Arc callback.
    OnChange,
    /// `Ui.onKeyDown` — string-carrying event handler, inline Arc wrap.
    OnKeyDown,
    /// `Ui.onKeyUp` — string-carrying event handler, inline Arc wrap.
    OnKeyUp,
    /// `Ui.onFile` — string-carrying event handler, inline Arc wrap.
    OnFile,
    /// `Event.onBool` — bool-carrying event handler, inline Arc wrap.
    OnBool,
    /// `Ui.onSubmit` — form handler dispatched by its lowered `OnFormKind`.
    OnSubmit,
    /// An `Ipe.Html.Events` builder, keyed by `html_event_shape` /
    /// `html_event_wire_name`.
    HtmlEvent,
    /// `Lazy.lazy` — one-argument deferred subtree, eta-wrapped.
    LazyLazy,
    /// `Lazy.lazy2` — two-argument deferred subtree, eta-wrapped.
    LazyLazy2,
    /// `Lazy.lazy3` — three-argument deferred subtree, eta-wrapped.
    LazyLazy3,
    /// `Lazy.lazy4` — four-argument deferred subtree, eta-wrapped.
    LazyLazy4,
    /// `Lazy.lazy5` — five-argument deferred subtree, eta-wrapped.
    LazyLazy5,
    /// `PubSub.publish` / `PubSub.publishNoEcho` — turbofished Task kernel.
    PubSubPublish,
    /// `Ui.widget` — the server-driven custom-element node. Bespoke because its
    /// handler argument must be re-wrapped to satisfy the runtime fn's
    /// `Send + Sync` bound (a boxed fn-value trait object is not `Sync`).
    Widget,
    /// A shape-router delegation to another emitter.
    Delegate(UiDelegate),
}

/// Which sibling emitter a [`NativeUiEmit::Delegate`] routes to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiDelegate {
    /// `emit_web::emit_web_call` — the Web app-entry kernels.
    Web,
    /// `emit_tui::emit_tui_call` — `Terminal.appScreen`.
    Tui,
    /// `emit_webview::emit_webview_call` — `WebView.app`.
    WebView,
    /// `emit_console::emit_console_call` — `Terminal.appLines`.
    Console,
}

/// A fail-closed guard the interpreter checks before emission.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Guard {
    /// No precondition.
    None,
    /// `Ui.cells` and peers paint raw terminal cells with no browser
    /// denotation; reject them in a Web / `WebView` build (fail-closed) rather
    /// than let the runtime helper degrade to plain text and render wrong.
    RejectInWebShape,
    /// `Ui.widget` is the server-driven custom element: its up-event handler is
    /// carried over the seal codec, which exists only in a browser shape (`web`
    /// implies the `json` feature; `Terminal` / `Program` do not). Outside a
    /// browser shape the widget has NO transport — the node would be inert, a
    /// widget with no seam. Reject it fail-closed rather than emit a dead
    /// element (or trip the non-`json` runtime fallback's unconstrained type
    /// parameter). Admissible only under `Web.app` / `WebView.app`.
    RejectInNonWebShape,
}

#[cfg(test)]
impl ArgPlan {
    /// The Ipê-level argument count this plan expects, when it is a positional
    /// call. `None` for a [`ArgPlan::Native`] plan, whose arity is checked
    /// inside its own emitter.
    pub const fn positional_arity(self) -> Option<u8> {
        match self {
            Self::Positional { arity, .. } => Some(arity),
            Self::Native(_) => None,
        }
    }
}

/// A positional plan with no guard — the uniform majority.
const fn pos(path: &'static str, arity: u8) -> UiEmitPlan {
    UiEmitPlan {
        args: ArgPlan::Positional { path, arity },
        guard: Guard::None,
    }
}

/// A positional plan carrying a fail-closed guard.
const fn guarded(path: &'static str, arity: u8, guard: Guard) -> UiEmitPlan {
    UiEmitPlan {
        args: ArgPlan::Positional { path, arity },
        guard,
    }
}

/// A plan dispatched to a bespoke native emitter.
const fn native(kind: NativeUiEmit) -> UiEmitPlan {
    UiEmitPlan {
        args: ArgPlan::Native(kind),
        guard: Guard::None,
    }
}

/// A bespoke native plan carrying a fail-closed shape guard.
const fn guarded_native(kind: NativeUiEmit, guard: Guard) -> UiEmitPlan {
    UiEmitPlan {
        args: ArgPlan::Native(kind),
        guard,
    }
}

/// A plan delegated to a sibling shape emitter.
const fn delegate(to: UiDelegate) -> UiEmitPlan {
    native(NativeUiEmit::Delegate(to))
}

/// The style-kernel allowlist for dev appearance hot-swap (Step 1, style slice).
///
/// A literal passed *directly* to one of these kernels, in one of the returned
/// argument positions, is an inert **style-value** string — a font family or a
/// raw CSS property/value — that feeds rendering and depends on no `Model`
/// value and no control flow. Under `IPE_WATCH_HOT_APPEARANCE` such a literal is
/// hoisted into a per-view [`ipe_runtime::web::LiteralTable`] so a dev edit can
/// swap it as data. Every entry is a plain positional `Ui.*` style kernel whose
/// named positions carry a `String`; the surface is style values only — not the
/// attribute, text, layout-structure, or non-style-string kernels.
///
/// Returned positions are 0-based indices into the kernel's direct arguments.
/// A kernel absent from this table has no hoist-eligible position.
///
/// The set is deliberately narrow and self-documenting: widening it (attribute
/// strings, static text) is a later, separately measured step behind its own
/// conformance coverage.
pub const fn style_literal_arg_positions(k: KernelFn) -> &'static [usize] {
    match k {
        // `Font.family : String -> Attribute msg` — the font family list, a
        // pure style value (`ui_font_family_(family: String)`).
        KernelFn::FontFamily => &[0],
        // `Ui.style : String -> String -> Attribute msg` — a raw CSS
        // property/value pair (`ui_style_(property, value)`). Both are inert
        // style strings baked as table defaults.
        KernelFn::UiStyle => &[0, 1],
        _ => &[],
    }
}

/// Classify one kernel into its emit plan.
///
/// Returns `None` for a non-UI-family kernel, preserving the caller's
/// early-return contract; `Some(plan)` for every kernel where
/// `is_ui() || is_web() || is_tui() || is_webview() || is_console()` holds. The
/// two properties — every UI-family kernel classified, no other kernel
/// classified — are the exhaustiveness partition the tests below assert.
#[allow(clippy::too_many_lines)] // one declarative row per UI kernel — the table is the point
pub const fn ui_call_shape(k: KernelFn) -> Option<UiEmitPlan> {
    use NativeUiEmit as N;
    let plan = match k {
        // ── Render entry + HTML serialiser ────────────────────────────────
        KernelFn::UiLayout => pos("ipe_runtime::ui::render::ui_layout", 2),
        KernelFn::UiLayoutWith => native(N::LayoutWith),
        KernelFn::HtmlRender | KernelFn::HtmlToString => native(N::HtmlSerialise),
        KernelFn::HtmlEscapeText => pos("ipe_runtime::html::html_escape_text_", 1),
        KernelFn::HtmlEscapeAttr => pos("ipe_runtime::html::html_escape_attr_", 1),
        KernelFn::HtmlAttrToString => pos("ipe_runtime::html::html_attr_to_string_", 1),

        // ── Ipe.Ui element builders ───────────────────────────────────────
        KernelFn::UiNone => pos("ipe_runtime::ui::helpers::ui_none_", 0),
        KernelFn::UiText => pos("ipe_runtime::ui::helpers::ui_text_", 1),
        KernelFn::UiHtml => pos("ipe_runtime::ui::helpers::ui_html_", 1),
        KernelFn::UiCells => guarded(
            "ipe_runtime::ui::helpers::ui_cells_",
            1,
            Guard::RejectInWebShape,
        ),
        // ── Ipe.Ui.Cells Cells-typed builders. No shape guard: the type system
        // rejects `Cells msg` where `Element msg` is expected (IPE-T0001).
        KernelFn::UiCellsNone => pos("ipe_runtime::tui::cells_none_", 0),
        KernelFn::UiCellsText => pos("ipe_runtime::tui::cells_text_", 1),
        KernelFn::UiCellsEl => pos("ipe_runtime::tui::cells_el_", 2),
        KernelFn::UiCellsRow => pos("ipe_runtime::tui::cells_row_", 2),
        KernelFn::UiCellsColumn => pos("ipe_runtime::tui::cells_column_", 2),
        KernelFn::UiCellsCells => pos("ipe_runtime::tui::cells_cells_", 1),
        // `Ui.widget ce state on_up` — the server-driven custom-element node.
        // A bespoke arm, not a plain positional call: `ui_widget_`'s handler
        // parameter carries `F: Fn(Up) -> M + Send + Sync + 'static`, which the
        // codegen's default `Box<dyn Fn + Send>` fn-value rendering does NOT
        // satisfy (a trait object is `Sync` only if its bound list says so). The
        // emitter re-wraps the handler in a fresh closure at the call site — the
        // same technique the `OnSubmit` / `String` / `Bool` event arms use.
        //
        // Guarded: the up-event handler rides the seal codec, present only in a
        // browser shape (`web`/`webview` force the `json` feature). In a
        // `Terminal` / `Program` build the widget has no transport, so it is a
        // fail-closed shape refusal rather than a dead node.
        KernelFn::UiWidget => guarded_native(N::Widget, Guard::RejectInNonWebShape),
        KernelFn::UiNode => pos("ipe_runtime::ui::helpers::ui_node_", 3),
        KernelFn::UiTaggedNode => pos("ipe_runtime::ui::helpers::ui_tagged_node_", 4),
        KernelFn::UiAbove => pos("ipe_runtime::ui::helpers::ui_above_", 1),
        KernelFn::UiBelow => pos("ipe_runtime::ui::helpers::ui_below_", 1),
        KernelFn::UiOnLeft => pos("ipe_runtime::ui::helpers::ui_on_left_", 1),
        KernelFn::UiOnRight => pos("ipe_runtime::ui::helpers::ui_on_right_", 1),
        KernelFn::UiInFront => pos("ipe_runtime::ui::helpers::ui_in_front_", 1),
        KernelFn::UiBehind => pos("ipe_runtime::ui::helpers::ui_behind_", 1),
        KernelFn::UiButton => native(N::Button),
        KernelFn::UiLink => native(N::Link),
        KernelFn::UiImage => native(N::Image),

        // ── Ipe.Ui attribute builders ─────────────────────────────────────
        KernelFn::UiSpacing => pos("ipe_runtime::ui::helpers::ui_spacing_", 1),
        KernelFn::UiPadding => pos("ipe_runtime::ui::helpers::ui_padding_", 1),
        KernelFn::UiPaddingXY => pos("ipe_runtime::ui::helpers::ui_padding_xy_", 2),
        KernelFn::UiPaddingEach => native(N::PaddingEach),
        KernelFn::UiWidth => pos("ipe_runtime::ui::helpers::ui_width_", 1),
        KernelFn::UiHeight => pos("ipe_runtime::ui::helpers::ui_height_", 1),
        KernelFn::UiCenterX => pos("ipe_runtime::ui::helpers::ui_center_x_", 0),
        KernelFn::UiCenterY => pos("ipe_runtime::ui::helpers::ui_center_y_", 0),
        KernelFn::UiAlignLeft => pos("ipe_runtime::ui::helpers::ui_align_left_", 0),
        KernelFn::UiAlignRight => pos("ipe_runtime::ui::helpers::ui_align_right_", 0),
        KernelFn::UiAlignTop => pos("ipe_runtime::ui::helpers::ui_align_top_", 0),
        KernelFn::UiAlignBottom => pos("ipe_runtime::ui::helpers::ui_align_bottom_", 0),
        KernelFn::UiPointer => pos("ipe_runtime::ui::helpers::ui_pointer_", 0),
        KernelFn::UiClip => pos("ipe_runtime::ui::helpers::ui_clip_", 0),
        KernelFn::UiClipX => pos("ipe_runtime::ui::helpers::ui_clip_x_", 0),
        KernelFn::UiClipY => pos("ipe_runtime::ui::helpers::ui_clip_y_", 0),
        KernelFn::UiScrollbars => pos("ipe_runtime::ui::helpers::ui_scrollbars_", 0),
        KernelFn::UiScrollbarX => pos("ipe_runtime::ui::helpers::ui_scrollbar_x_", 0),
        KernelFn::UiScrollbarY => pos("ipe_runtime::ui::helpers::ui_scrollbar_y_", 0),
        KernelFn::UiGridColumns => pos("ipe_runtime::ui::helpers::ui_grid_columns_", 1),

        // ── Length builders ───────────────────────────────────────────────
        KernelFn::UiPx => pos("ipe_runtime::ui::helpers::ui_px_", 1),
        KernelFn::UiFill => pos("ipe_runtime::ui::helpers::ui_fill_", 0),
        KernelFn::UiContent => pos("ipe_runtime::ui::helpers::ui_content_", 0),
        KernelFn::UiShrink => pos("ipe_runtime::ui::helpers::ui_shrink_", 0),
        KernelFn::UiFillPortion => pos("ipe_runtime::ui::helpers::ui_fill_portion_", 1),
        KernelFn::UiVh => pos("ipe_runtime::ui::helpers::ui_vh_", 1),
        KernelFn::UiVw => pos("ipe_runtime::ui::helpers::ui_vw_", 1),
        KernelFn::UiMinimum => pos("ipe_runtime::ui::helpers::ui_minimum_", 2),
        KernelFn::UiMaximum => pos("ipe_runtime::ui::helpers::ui_maximum_", 2),

        // ── Color builders ────────────────────────────────────────────────
        KernelFn::UiRgb => pos("ipe_runtime::ui::helpers::ui_rgb_", 3),
        KernelFn::UiRgba => pos("ipe_runtime::ui::helpers::ui_rgba_", 4),
        KernelFn::UiWhite => pos("ipe_runtime::ui::helpers::ui_white_", 0),
        KernelFn::UiBlack => pos("ipe_runtime::ui::helpers::ui_black_", 0),
        KernelFn::UiTransparent => pos("ipe_runtime::ui::helpers::ui_transparent_", 0),
        KernelFn::UiColorCss => pos("ipe_runtime::ui::helpers::ui_color_css_", 1),

        // ── Background sub-module ─────────────────────────────────────────
        KernelFn::BackgroundColor => pos("ipe_runtime::ui::helpers::ui_background_color_", 1),
        KernelFn::BackgroundImage => pos("ipe_runtime::ui::helpers::ui_background_image_", 1),
        KernelFn::BackgroundLinearGradient => pos(
            "ipe_runtime::ui::helpers::ui_background_linear_gradient_",
            2,
        ),

        // ── Border sub-module ─────────────────────────────────────────────
        KernelFn::BorderWidth => pos("ipe_runtime::ui::helpers::ui_border_width_", 1),
        KernelFn::BorderRounded => pos("ipe_runtime::ui::helpers::ui_border_rounded_", 1),
        KernelFn::BorderColor => pos("ipe_runtime::ui::helpers::ui_border_color_", 1),
        KernelFn::BorderWidthEach => native(N::BorderWidthEach),
        KernelFn::BorderShadow => native(N::BorderShadow),
        KernelFn::BorderGlow => pos("ipe_runtime::ui::helpers::ui_border_glow_", 2),
        KernelFn::BorderInnerShadow => native(N::BorderInnerShadow),

        // ── Font sub-module ───────────────────────────────────────────────
        KernelFn::FontSize => pos("ipe_runtime::ui::helpers::ui_font_size_", 1),
        KernelFn::FontColor => pos("ipe_runtime::ui::helpers::ui_font_color_", 1),
        KernelFn::FontFamily => pos("ipe_runtime::ui::helpers::ui_font_family_", 1),
        KernelFn::FontBold => pos("ipe_runtime::ui::helpers::ui_font_bold_", 0),
        KernelFn::FontItalic => pos("ipe_runtime::ui::helpers::ui_font_italic_", 0),

        // ── Aspect-ratio + misc Ui attrs ──────────────────────────────────
        KernelFn::UiSquare => pos("ipe_runtime::ui::helpers::ui_square_", 0),
        KernelFn::UiWidescreen => pos("ipe_runtime::ui::helpers::ui_widescreen_", 0),
        KernelFn::UiCinemascope => pos("ipe_runtime::ui::helpers::ui_cinemascope_", 0),
        KernelFn::UiName => pos("ipe_runtime::ui::helpers::ui_name_", 1),
        KernelFn::UiStyle => pos("ipe_runtime::ui::helpers::ui_style_", 2),
        KernelFn::UiTransitionRaw => pos("ipe_runtime::ui::helpers::ui_transition_raw_", 2),
        KernelFn::UiGridTracksRaw => pos("ipe_runtime::ui::helpers::ui_grid_tracks_raw_", 2),
        KernelFn::UiAnimateRaw => pos("ipe_runtime::ui::helpers::ui_animate_raw_", 4),
        KernelFn::UiAspectRatio => pos("ipe_runtime::ui::helpers::ui_aspect_ratio_", 1),
        KernelFn::UiAspectRatioWH => pos("ipe_runtime::ui::helpers::ui_aspect_ratio_wh_", 2),
        KernelFn::UiHtmlAttribute => pos("ipe_runtime::ui::helpers::ui_html_attribute_", 2),

        // ── Breakpoint + pseudo-class constants ───────────────────────────
        KernelFn::UiMobile => pos("ipe_runtime::ui::helpers::ui_mobile_", 0),
        KernelFn::UiTablet => pos("ipe_runtime::ui::helpers::ui_tablet_", 0),
        KernelFn::UiDesktop => pos("ipe_runtime::ui::helpers::ui_desktop_", 0),
        KernelFn::UiDarkMode => pos("ipe_runtime::ui::helpers::ui_dark_mode_", 0),
        KernelFn::UiLightMode => pos("ipe_runtime::ui::helpers::ui_light_mode_", 0),
        KernelFn::UiReducedMotion => pos("ipe_runtime::ui::helpers::ui_reduced_motion_", 0),
        KernelFn::UiHover => pos("ipe_runtime::ui::helpers::ui_hover_", 0),
        KernelFn::UiFocus => pos("ipe_runtime::ui::helpers::ui_focus_", 0),
        KernelFn::UiFocusVisible => pos("ipe_runtime::ui::helpers::ui_focus_visible_", 0),
        KernelFn::UiActive => pos("ipe_runtime::ui::helpers::ui_active_", 0),
        KernelFn::UiDisabled => pos("ipe_runtime::ui::helpers::ui_disabled_", 0),
        KernelFn::UiOnPseudo => pos("ipe_runtime::ui::helpers::ui_on_pseudo_", 2),
        KernelFn::UiBreakpoint => pos("ipe_runtime::ui::helpers::ui_breakpoint_", 3),
        KernelFn::UiMediaQuery => pos("ipe_runtime::ui::helpers::ui_media_query_", 3),

        // ── Background / Border / Font pseudo-class attrs ─────────────────
        KernelFn::BackgroundHoverColor => pos("ipe_runtime::ui::helpers::ui_bg_hover_color_", 1),
        KernelFn::BackgroundFocusColor => pos("ipe_runtime::ui::helpers::ui_bg_focus_color_", 1),
        KernelFn::BackgroundActiveColor => pos("ipe_runtime::ui::helpers::ui_bg_active_color_", 1),
        KernelFn::BackgroundDisabledColor => {
            pos("ipe_runtime::ui::helpers::ui_bg_disabled_color_", 1)
        }
        KernelFn::BorderSolid => pos("ipe_runtime::ui::helpers::ui_border_solid_", 0),
        KernelFn::BorderDashed => pos("ipe_runtime::ui::helpers::ui_border_dashed_", 0),
        KernelFn::BorderDotted => pos("ipe_runtime::ui::helpers::ui_border_dotted_", 0),
        KernelFn::BorderHoverColor => pos("ipe_runtime::ui::helpers::ui_border_hover_color_", 1),
        KernelFn::BorderFocusColor => pos("ipe_runtime::ui::helpers::ui_border_focus_color_", 1),
        KernelFn::BorderActiveColor => pos("ipe_runtime::ui::helpers::ui_border_active_color_", 1),
        KernelFn::BorderHoverWidth => pos("ipe_runtime::ui::helpers::ui_border_hover_width_", 1),
        KernelFn::BorderHoverRounded => {
            pos("ipe_runtime::ui::helpers::ui_border_hover_rounded_", 1)
        }
        KernelFn::FontWeight => pos("ipe_runtime::ui::helpers::ui_font_weight_", 1),
        KernelFn::FontSemiBold => pos("ipe_runtime::ui::helpers::ui_font_semi_bold_", 0),
        KernelFn::FontRegular => pos("ipe_runtime::ui::helpers::ui_font_regular_", 0),
        KernelFn::FontLight => pos("ipe_runtime::ui::helpers::ui_font_light_", 0),
        KernelFn::FontExtraBold => pos("ipe_runtime::ui::helpers::ui_font_extra_bold_", 0),
        KernelFn::FontBlack => pos("ipe_runtime::ui::helpers::ui_font_black_", 0),
        KernelFn::FontUnderline => pos("ipe_runtime::ui::helpers::ui_font_underline_", 0),
        KernelFn::FontNoDecoration => pos("ipe_runtime::ui::helpers::ui_font_no_decoration_", 0),
        KernelFn::FontLineThrough => pos("ipe_runtime::ui::helpers::ui_font_line_through_", 0),
        KernelFn::FontLetterSpacing => pos("ipe_runtime::ui::helpers::ui_font_letter_spacing_", 1),
        KernelFn::FontWordSpacing => pos("ipe_runtime::ui::helpers::ui_font_word_spacing_", 1),
        KernelFn::FontAlignLeft => pos("ipe_runtime::ui::helpers::ui_font_align_left_", 0),
        KernelFn::FontAlignRight => pos("ipe_runtime::ui::helpers::ui_font_align_right_", 0),
        KernelFn::FontAlignCenter => pos("ipe_runtime::ui::helpers::ui_font_align_center_", 0),
        KernelFn::FontCenter => pos("ipe_runtime::ui::helpers::ui_font_center_", 0),
        KernelFn::FontJustify => pos("ipe_runtime::ui::helpers::ui_font_justify_", 0),
        KernelFn::FontSansSerif => pos("ipe_runtime::ui::helpers::ui_font_sans_serif_", 0),
        KernelFn::FontSerif => pos("ipe_runtime::ui::helpers::ui_font_serif_", 0),
        KernelFn::FontMonospace => pos("ipe_runtime::ui::helpers::ui_font_monospace_", 0),
        KernelFn::FontHoverColor => pos("ipe_runtime::ui::helpers::ui_font_hover_color_", 1),
        KernelFn::FontFocusColor => pos("ipe_runtime::ui::helpers::ui_font_focus_color_", 1),
        KernelFn::FontActiveColor => pos("ipe_runtime::ui::helpers::ui_font_active_color_", 1),
        KernelFn::FontDisabledColor => pos("ipe_runtime::ui::helpers::ui_font_disabled_color_", 1),
        KernelFn::FontHoverSize => pos("ipe_runtime::ui::helpers::ui_font_hover_size_", 1),

        // ── Region / describe accessibility constructors ──────────────────
        KernelFn::RegionMainContent => pos("ipe_runtime::ui::helpers::ui_region_main_content_", 0),
        KernelFn::RegionNavigation => pos("ipe_runtime::ui::helpers::ui_region_navigation_", 0),
        KernelFn::RegionFooter => pos("ipe_runtime::ui::helpers::ui_region_footer_", 0),
        KernelFn::RegionAside => pos("ipe_runtime::ui::helpers::ui_region_aside_", 0),
        KernelFn::RegionHeading => pos("ipe_runtime::ui::helpers::ui_region_heading_", 1),
        KernelFn::RegionLabel => pos("ipe_runtime::ui::helpers::ui_region_label_", 1),
        KernelFn::RegionAnnounce => pos("ipe_runtime::ui::helpers::ui_region_announce_", 0),
        KernelFn::RegionAnnounceUrgently => {
            pos("ipe_runtime::ui::helpers::ui_region_announce_urgently_", 0)
        }
        KernelFn::UiDescribe => pos("ipe_runtime::ui::helpers::ui_describe_", 1),
        KernelFn::UiDescNone => pos("ipe_runtime::ui::helpers::ui_desc_none_", 0),
        KernelFn::UiDescParagraph => pos("ipe_runtime::ui::helpers::ui_desc_paragraph_", 0),
        KernelFn::UiDescMain => pos("ipe_runtime::ui::helpers::ui_desc_main_", 0),
        KernelFn::UiDescNavigation => pos("ipe_runtime::ui::helpers::ui_desc_navigation_", 0),
        KernelFn::UiDescContentInfo => pos("ipe_runtime::ui::helpers::ui_desc_content_info_", 0),
        KernelFn::UiDescComplementary => pos("ipe_runtime::ui::helpers::ui_desc_complementary_", 0),
        KernelFn::UiDescLivePolite => pos("ipe_runtime::ui::helpers::ui_desc_live_polite_", 0),
        KernelFn::UiDescLiveAssertive => {
            pos("ipe_runtime::ui::helpers::ui_desc_live_assertive_", 0)
        }
        KernelFn::UiDescHeading => pos("ipe_runtime::ui::helpers::ui_desc_heading_", 1),
        KernelFn::UiDescLabel => pos("ipe_runtime::ui::helpers::ui_desc_label_", 1),

        // ── Input label constructors + input builders ─────────────────────
        KernelFn::InputLabelAbove => pos("ipe_runtime::ui::input::input_label_above_", 2),
        KernelFn::InputLabelBelow => pos("ipe_runtime::ui::input::input_label_below_", 2),
        KernelFn::InputLabelLeft => pos("ipe_runtime::ui::input::input_label_left_", 2),
        KernelFn::InputLabelRight => pos("ipe_runtime::ui::input::input_label_right_", 2),
        KernelFn::InputLabelHidden => pos("ipe_runtime::ui::input::input_label_hidden_", 1),
        KernelFn::InputPlaceholder => pos("ipe_runtime::ui::input::input_placeholder_", 2),
        KernelFn::InputText
        | KernelFn::InputEmail
        | KernelFn::InputUsername
        | KernelFn::InputSearch
        | KernelFn::InputCurrentPassword
        | KernelFn::InputNewPassword => native(N::InputText),
        KernelFn::InputMultiline => native(N::InputMultiline),
        KernelFn::InputCheckbox => native(N::InputCheckbox),
        KernelFn::InputSlider => native(N::InputSlider),
        KernelFn::InputOption => pos("ipe_runtime::ui::input::input_option_", 2),
        KernelFn::InputRadio => native(N::InputRadio),
        KernelFn::InputRadioRow => native(N::InputRadioRow),

        // ── Ipe.Html element builders ─────────────────────────────────────
        KernelFn::HtmlTextNode => pos("ipe_runtime::ui::helpers::html_text_node_", 1),
        KernelFn::HtmlRawNode => pos("ipe_runtime::ui::helpers::html_raw_node_", 1),
        KernelFn::HtmlNode => pos("ipe_runtime::ui::helpers::html_node_", 3),
        KernelFn::HtmlVoidNode => native(N::HtmlVoidNode),
        KernelFn::HtmlDoctype => pos("ipe_runtime::ui::helpers::html_doctype_", 1),
        KernelFn::HtmlTitleNode => pos("ipe_runtime::ui::helpers::html_title_node_", 1),
        KernelFn::HtmlStyleNode => pos("ipe_runtime::ui::helpers::html_style_node_", 2),
        KernelFn::HtmlScriptNode => pos("ipe_runtime::ui::helpers::html_script_node_", 1),

        // ── Plain-message event attrs ─────────────────────────────────────
        KernelFn::UiOnClick => pos("ipe_runtime::ui::helpers::ui_on_click_", 1),
        KernelFn::UiOnFocus => pos("ipe_runtime::ui::helpers::ui_on_focus_", 1),
        KernelFn::UiOnBlur => pos("ipe_runtime::ui::helpers::ui_on_blur_", 1),
        KernelFn::UiOnMouseOver => pos("ipe_runtime::ui::helpers::ui_on_mouse_over_", 1),
        KernelFn::UiOnMouseOut => pos("ipe_runtime::ui::helpers::ui_on_mouse_out_", 1),

        // ── Callback-carrying event attrs ─────────────────────────────────
        KernelFn::UiOnInput => native(N::OnInput),
        KernelFn::UiOnChange => native(N::OnChange),
        KernelFn::UiOnKeyDown => native(N::OnKeyDown),
        KernelFn::UiOnKeyUp => native(N::OnKeyUp),
        KernelFn::UiOnFile => native(N::OnFile),
        KernelFn::UiOnBool => native(N::OnBool),
        KernelFn::UiOnSubmit => native(N::OnSubmit),

        // ── Generic HTML attributes ───────────────────────────────────────
        KernelFn::HtmlAttribute => pos("ipe_runtime::html::html_named_attr_", 2),
        KernelFn::HtmlBoolAttribute => pos("ipe_runtime::html::html_bool_named_attr_", 2),
        KernelFn::HtmlNoAttr => pos("ipe_runtime::html::html_no_attr_", 0),

        // ── Keyed diff-identity containers ────────────────────────────────
        KernelFn::KeyedColumn => pos("ipe_runtime::ui::keyed::keyed_column_", 2),
        KernelFn::KeyedRow => pos("ipe_runtime::ui::keyed::keyed_row_", 2),

        // ── Deferred-subtree helpers ──────────────────────────────────────
        KernelFn::LazyLazy => native(N::LazyLazy),
        KernelFn::LazyLazy2 => native(N::LazyLazy2),
        KernelFn::LazyLazy3 => native(N::LazyLazy3),
        KernelFn::LazyLazy4 => native(N::LazyLazy4),
        KernelFn::LazyLazy5 => native(N::LazyLazy5),

        // ── PubSub publish (Task-shaped, web bus) ─────────────────────────
        KernelFn::PubSubPublish | KernelFn::PubSubPublishNoEcho => native(N::PubSubPublish),

        // ── Shape-router delegations ──────────────────────────────────────
        KernelFn::WebApp
        | KernelFn::WebAppRouted
        | KernelFn::WebEmbed
        | KernelFn::WebAppWith
        | KernelFn::WebRoute
        | KernelFn::WebRenderStatic => delegate(UiDelegate::Web),
        KernelFn::TerminalAppScreen => delegate(UiDelegate::Tui),
        KernelFn::WebViewApp => delegate(UiDelegate::WebView),
        KernelFn::TerminalAppLines => delegate(UiDelegate::Console),

        // ── Debug.explain — dev-only, Web/WebView only ────────────────────
        // `Debug.explain : Attribute msg` draws visible outlines on the element
        // and all descendants without changing layout.  Reject in Terminal /
        // Program shapes (fail-closed) — there is no DOM to outline.
        KernelFn::DebugExplain => guarded(
            "ipe_runtime::ui::helpers::debug_explain_",
            0,
            Guard::RejectInNonWebShape,
        ),

        // ── Predicate-keyed HTML families ─────────────────────────────────
        _ if k.html_event_shape().is_some() => native(N::HtmlEvent),

        // Not a UI-family kernel — no plan.
        _ => return None,
    };
    Some(plan)
}

#[cfg(test)]
mod tests {
    use ipe_ir::KernelFn;

    use super::{ArgPlan, Guard, NativeUiEmit, UiDelegate, ui_call_shape};

    /// Every kernel `ui_call_shape` classifies as UI-family. Mirrors the
    /// `is_ui() || …` guard that fronts the emitter.
    fn is_ui_family(k: KernelFn) -> bool {
        k.is_ui() || k.is_web() || k.is_tui() || k.is_webview() || k.is_console()
    }

    /// A widget's plan is the positional shape the emitter renders as
    /// `path(args)`.
    #[test]
    fn positional_widgets_carry_path_and_arity() {
        let cases = [
            (KernelFn::UiText, "ipe_runtime::ui::helpers::ui_text_", 1u8),
            (KernelFn::UiNode, "ipe_runtime::ui::helpers::ui_node_", 3),
            (
                KernelFn::UiTaggedNode,
                "ipe_runtime::ui::helpers::ui_tagged_node_",
                4,
            ),
            (KernelFn::UiRgb, "ipe_runtime::ui::helpers::ui_rgb_", 3),
            (KernelFn::UiRgba, "ipe_runtime::ui::helpers::ui_rgba_", 4),
            (KernelFn::UiNone, "ipe_runtime::ui::helpers::ui_none_", 0),
            (KernelFn::UiLayout, "ipe_runtime::ui::render::ui_layout", 2),
            (
                KernelFn::KeyedColumn,
                "ipe_runtime::ui::keyed::keyed_column_",
                2,
            ),
            (
                KernelFn::InputOption,
                "ipe_runtime::ui::input::input_option_",
                2,
            ),
        ];
        for (k, path, arity) in cases {
            let plan = ui_call_shape(k).expect("UI kernel must classify");
            assert_eq!(
                plan.args,
                ArgPlan::Positional { path, arity },
                "{k:?} plan shape"
            );
            assert_eq!(plan.guard, Guard::None, "{k:?} guard");
        }
    }

    /// The capability and security leaves classify to their bespoke tag rather
    /// than a positional path.
    #[test]
    fn capability_leaves_classify_native() {
        let cases = [
            (KernelFn::UiButton, NativeUiEmit::Button),
            (KernelFn::UiLayoutWith, NativeUiEmit::LayoutWith),
            (KernelFn::HtmlRender, NativeUiEmit::HtmlSerialise),
            (KernelFn::HtmlToString, NativeUiEmit::HtmlSerialise),
            (KernelFn::UiOnInput, NativeUiEmit::OnInput),
            (KernelFn::UiOnSubmit, NativeUiEmit::OnSubmit),
            (KernelFn::InputText, NativeUiEmit::InputText),
            (KernelFn::InputEmail, NativeUiEmit::InputText),
            (KernelFn::BorderShadow, NativeUiEmit::BorderShadow),
            (KernelFn::LazyLazy, NativeUiEmit::LazyLazy),
            (KernelFn::PubSubPublish, NativeUiEmit::PubSubPublish),
            (KernelFn::WebApp, NativeUiEmit::Delegate(UiDelegate::Web)),
            (
                KernelFn::TerminalAppScreen,
                NativeUiEmit::Delegate(UiDelegate::Tui),
            ),
            (
                KernelFn::WebViewApp,
                NativeUiEmit::Delegate(UiDelegate::WebView),
            ),
            (
                KernelFn::TerminalAppLines,
                NativeUiEmit::Delegate(UiDelegate::Console),
            ),
        ];
        for (k, kind) in cases {
            let plan = ui_call_shape(k).expect("UI kernel must classify");
            assert_eq!(plan.args, ArgPlan::Native(kind), "{k:?} native tag");
        }
    }

    /// The `Ui.cells` seal is the guarded plan — fail-closed in a web shape.
    #[test]
    fn ui_cells_carries_web_shape_guard() {
        let plan = ui_call_shape(KernelFn::UiCells).expect("Ui.cells must classify");
        assert_eq!(plan.guard, Guard::RejectInWebShape);
        assert_eq!(
            plan.args,
            ArgPlan::Positional {
                path: "ipe_runtime::ui::helpers::ui_cells_",
                arity: 1,
            },
        );
    }

    /// The dispatch is a total partition: every UI-family kernel yields a plan,
    /// and no other kernel does. A UI kernel added without a plan arm fails
    /// here — at the classifier — rather than downstream when the emitted Rust
    /// fails to build.
    #[test]
    fn exhaustiveness_partition() {
        for &k in KernelFn::ALL {
            let classified = ui_call_shape(k).is_some();
            assert_eq!(
                classified,
                is_ui_family(k),
                "{k:?}: classified={classified} but is_ui_family={}",
                is_ui_family(k),
            );
        }
    }

    /// The web-shape guard set is exactly `UiCells` — the one kernel that
    /// produces `Element msg` but has no browser denotation. The `UiCells*`
    /// builders produce `Cells msg`; misuse is caught at the type level
    /// (IPE-T0001) rather than by a runtime guard.
    #[test]
    fn reject_in_web_shape_guard_is_exactly_ui_cells() {
        for &k in KernelFn::ALL {
            let guarded = ui_call_shape(k).is_some_and(|p| p.guard == Guard::RejectInWebShape);
            let expected = matches!(k, KernelFn::UiCells);
            assert_eq!(guarded, expected, "{k:?}: guarded={guarded}");
        }
    }

    /// The non-web-shape guard set covers kernels that have no denotation
    /// outside a browser shape: `Ui.widget` (no up-event transport) and
    /// `Debug.explain` (no DOM to outline).  A new browser-only kernel that
    /// omits the guard fails here rather than silently emitting dead code.
    #[test]
    fn reject_in_non_web_shape_guard_is_exactly_browser_only_kernels() {
        for &k in KernelFn::ALL {
            let guarded = ui_call_shape(k).is_some_and(|p| p.guard == Guard::RejectInNonWebShape);
            let expected = matches!(k, KernelFn::UiWidget | KernelFn::DebugExplain);
            assert_eq!(guarded, expected, "{k:?}: non-web-guarded={guarded}");
        }
    }

    /// Every positional plan's arity equals the kernel's authoritative arity in
    /// its [`KernelDef`] descriptor row — the single source of truth. A plan
    /// that drifts from the declared arity fails here at test time rather than
    /// emitting a call with the wrong argument count.
    ///
    /// Native plans are exempt: their arity is enforced inside their own
    /// emitter (several destructure a config record rather than take positional
    /// args, so the Ipê-level arity and the emitted-call argument count differ).
    #[test]
    fn positional_arity_matches_kernel_def() {
        for &k in KernelFn::ALL {
            let Some(plan) = ui_call_shape(k) else {
                continue;
            };
            let Some(arity) = plan.args.positional_arity() else {
                continue;
            };
            assert_eq!(
                arity,
                k.def().arity,
                "{k:?}: plan arity {arity} != KernelDef arity {}",
                k.def().arity,
            );
        }
    }
}
