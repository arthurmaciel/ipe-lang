//! The embedded Ipê standard-library source (`Ipe.*`).
//!
//! `ipe` is self-contained: the foundational `Ipe` modules are compiled
//! into the binary as their original Ipe source (a port of the Haskell
//! compiler's Template-Haskell embedding of `ipe-stdlib/`). The checked-in copies
//! under `crates/ipec/stdlib/Ipe/Core/` are byte-identical to the upstream
//! `ipe-stdlib` sources; embedding a copy (rather than `include_str!`-ing an
//! out-of-repo path) keeps the build portable and the toolchain hermetic.
//!
//! `Ipe.Basics` is the canonical implicit module (ADR 0047): its Tier-A surface
//! is auto-imported into every module. There is no `Ipe.Prelude` module — the
//! old value-flooding alias is removed, so `import Ipe.Prelude` does not resolve.
//! The source is ordinary Ipê: the same parser that reads user code reads it (the
//! `parses` test proves it), so it is the substrate the import resolver compiles.
#![forbid(unsafe_code)]

/// One embedded standard-library module: its dotted name and its Ipê source.
pub struct StdModule {
    /// The dotted module name as written in an `import`, e.g. `Ipe.Maybe`.
    pub name: &'static str,
    /// The module's Ipê source, embedded at compile time.
    pub source: &'static str,
}

/// `Ipe.Basics` — `identity` / `always` / `not` / `fst` / `snd` / `clamp`.
const BASICS: &str = include_str!("../Ipe/Basics.ipe");
/// `Ipe.Maybe` — combinators over the `Maybe` ADT.
const MAYBE: &str = include_str!("../Ipe/Maybe.ipe");
/// `Ipe.Result` — combinators over the `Result` ADT.
const RESULT: &str = include_str!("../Ipe/Result.ipe");
/// `Ipe.List` — list combinators.
const LIST: &str = include_str!("../Ipe/List.ipe");
/// `Ipe.String` — string combinators.
const STRING: &str = include_str!("../Ipe/String.ipe");
/// `Ipe.Char` — single-character helpers.
const CHAR: &str = include_str!("../Ipe/Char.ipe");
/// `Ipe.Dict` — string-keyed associative map.
const DICT: &str = include_str!("../Ipe/Dict.ipe");
/// `Ipe.Set` — unordered set of unique elements.
const SET: &str = include_str!("../Ipe/Set.ipe");
/// `Ipe.Bytes` — arbitrary byte buffer, distinct from `String`.
///
/// Divergence from Ipê: Ipê defines `type alias Bytes = String`; Ipê-Rust
/// makes `Bytes` a distinct primitive lowering to `Vec<u8>` (lossless for
/// non-UTF-8 binary). See `docs/architecture/divergence-policy.md`.
const BYTES: &str = include_str!("../Ipe/Bytes.ipe");
/// `Ipe.Crypto` — hashes / HMAC / RSA / AEAD / key-derivation / random.
const CRYPTO: &str = include_str!("../Ipe/Crypto.ipe");
/// `Ipe.Bitwise` — Int-only bitwise operations (kernel-qualified surface).
const BITWISE: &str = include_str!("../Ipe/Bitwise.ipe");
/// `Ipe.Task` — Task combinator surface.
const TASK: &str = include_str!("../Ipe/Task.ipe");
/// `Ipe.Io` — standard-I/O effect kernels.
const IO: &str = include_str!("../Ipe/Io.ipe");
/// `Ipe.Debug` — development-only `Debug.log` escape hatch (kernel-only).
const DEBUG: &str = include_str!("../Ipe/Debug.ipe");
/// `Ipe.Time` — time effect kernels.
const TIME: &str = include_str!("../Ipe/Time.ipe");
/// `Ipe.System` — process / environment effect kernels.
const SYSTEM: &str = include_str!("../Ipe/System.ipe");
/// `Ipe.Random` — entropy-backed and seeded randomness, compiled-source Layer-3.
///
/// Every member is either a point-free `Ffi.kernel "Random_*"` alias resolved by
/// `ipe_canon::resolve::detect_kernel_alias` to a registered `Random*`
/// `StdlibKernel` variant (`ipe_runtime::random::*`), or pure Ipê over those
/// aliases (`range`, the seeded wrappers, the opaque `Seed` ADT). Registered in
/// [`COMPILED_STD_MODULES`] (NOT `MODULES`); NOT in `STDLIB_MODULE_QUALIFIERS`,
/// so the disjointness invariant holds.
const RANDOM: &str = include_str!("../Ipe/Random.ipe");
/// `Ipe.File` — file-system effect kernels.
const FILE: &str = include_str!("../Ipe/File.ipe");
/// `Ipe.Http` — outbound HTTP client kernels + pure builders.
const HTTP: &str = include_str!("../Ipe/Http.ipe");
/// `Ipe.Process` — subprocess execution (no shell) effect kernels.
const PROCESS: &str = include_str!("../Ipe/Process.ipe");

/// `Ipe.Path` — pure filesystem-path helpers, compiled-source Layer-3.
///
/// The members are point-free `Ffi.kernel "Path_*"` aliases resolved by the
/// kernel-alias mechanism (`ipe_canon::resolve::detect_kernel_alias`) to
/// the pure `PathBase`/`PathDir`/`PathExt`/`PathIsAbsolute` `StdlibKernel`
/// variants. Registered in [`COMPILED_STD_MODULES`] (NOT `MODULES`) so its body
/// is actually compiled; NOT in `STDLIB_MODULE_QUALIFIERS`, so the disjointness
/// invariant holds.
const PATH: &str = include_str!("../Ipe/Path.ipe");
/// `Ipe.Html.Attributes` — HTML attribute builders, compiled-source Layer-3.
///
/// Every fixed-key builder (`class`/`id`/`checked`/…) is pure Ipê over the
/// three retained primitives `attribute`/`boolAttribute`/`noAttr`, which are
/// point-free `Ffi.kernel "Attr_attribute"`/`"Attr_boolAttribute"`/`"Attr_noAttr"`
/// aliases resolved by `ipe_canon::resolve::detect_kernel_alias` to the
/// `HtmlAttribute`/`HtmlBoolAttribute`/`HtmlNoAttr` kernels (runtime:
/// `ipe_runtime::html::html_named_attr_`/`html_bool_named_attr_`/`html_no_attr_`).
/// Registered in [`COMPILED_STD_MODULES`] (NOT `MODULES`); NOT in
/// `STDLIB_MODULE_QUALIFIERS`, so the disjointness invariant holds.
const HTML_ATTRIBUTES: &str = include_str!("../Ipe/Html/Attributes.ipe");
/// `Ipe.Html.Unsafe` — the un-escaped raw-HTML escape hatch, compiled-source
/// Layer-3.
///
/// The single member `unsafeRaw` is a point-free `Ffi.kernel "Html_unsafeRaw"`
/// alias resolved by `ipe_canon::resolve::detect_kernel_alias` to the retained
/// `HtmlRawNode` kernel (runtime: `ipe_runtime::ui::helpers::html_raw_node_`,
/// the `HRaw` verbatim sink). Only the surface home moved here from `Ipe.Html`;
/// the kernel, its `("Html", "unsafeRaw")` registry key, and the render-sink
/// behaviour are unchanged. Importing this dotted `Ipe.<M>.Unsafe` submodule
/// discloses the `unsafe` capability. Registered in [`COMPILED_STD_MODULES`]
/// (NOT `MODULES`); NOT in `STDLIB_MODULE_QUALIFIERS`, so the disjointness
/// invariant holds.
const HTML_UNSAFE: &str = include_str!("../Ipe/Html/Unsafe.ipe");
/// `Ipe.Db.Unsafe` — the raw-SQL / untyped-column-read escape hatches for
/// `Ipe.Db`, compiled-source Layer-3.
///
/// Every member is a point-free `Ffi.kernel "Db_*"` / `"Sql_unsafeFragment"`
/// alias resolved by `ipe_canon::resolve::detect_kernel_alias` to a retained
/// kernel: `unsafeExecRaw`/`unsafeQuery`/`unsafeGet*` to the unchanged `Db*`
/// kernels (runtime: `ipe_runtime::db::db_exec_raw`/`db_query_params`/
/// `db_get_*`), and the new `unsafeFragment` to the `SqlUnsafeFragment` kernel
/// (runtime: `ipe_runtime::db::sql_unsafe_fragment`, the un-validated
/// counterpart to `sql_column`). Only the surface home moved here from
/// `Ipe.Db` / `Ipe.Db.Sql`; the kernels, their registry keys, and their
/// runtime behaviour are unchanged. Importing this dotted `Ipe.<M>.Unsafe`
/// submodule discloses the `unsafe` capability. Registered in
/// [`COMPILED_STD_MODULES`] (NOT `MODULES`); NOT in `STDLIB_MODULE_QUALIFIERS`,
/// so the disjointness invariant holds.
const DB_UNSAFE: &str = include_str!("../Ipe/Db/Unsafe.ipe");
/// `Ipe.Secret.Unsafe` — the raw secret-reveal escape hatch for `Ipe.Secret`,
/// compiled-source Layer-3.
///
/// The single member `unsafeReveal` is a point-free `Ffi.kernel "Secret_reveal"`
/// alias resolved by `ipe_canon::resolve::detect_kernel_alias` to the retained
/// `SecretReveal` kernel (runtime: `ipe_runtime::secret::secret_reveal`, the
/// single greppable un-parse). Only the surface home moved here from
/// `Ipe.Secret`; the kernel, its registry key, and the sealed-newtype barrier
/// are unchanged. The scoped `Secret.use` stays on the native `Ipe.Secret`
/// surface. Importing this dotted `Ipe.<M>.Unsafe` submodule discloses the
/// `unsafe` capability. Registered in [`COMPILED_STD_MODULES`] (NOT `MODULES`);
/// NOT in `STDLIB_MODULE_QUALIFIERS`, so the disjointness invariant holds.
const SECRET_UNSAFE: &str = include_str!("../Ipe/Secret/Unsafe.ipe");
/// `Ipe.Html` — HTML element builders, compiled-source Layer-3.
///
/// Every element builder (`div`/`nav`/`br`/…) is pure Ipê over two retained
/// primitives — `node`/`voidNode` — which, with `text`/`doctype`/
/// `titleNode`/`styleNode`, are point-free `Ffi.kernel "Html_*"` aliases
/// resolved by `ipe_canon::resolve::detect_kernel_alias` to the retained
/// `HtmlNode`/`HtmlVoidNode`/… kernels (runtime: `ipe_runtime::ui::helpers::*`).
/// The serialiser (`render`/`toString`/`escapeHtml`/`escapeAttr`/`attrToString`)
/// and `renderStatic` stay native — the XSS barrier — and are re-aliased here so
/// `Html.render` keeps resolving. Event attributes stay in `Ipe.Html.Events`.
/// Registered in [`COMPILED_STD_MODULES`] (NOT `MODULES`); NOT in
/// `STDLIB_MODULE_QUALIFIERS`, so the disjointness invariant holds.
const HTML: &str = include_str!("../Ipe/Html.ipe");
/// `Ipe.Ui` — element / attribute / colour / layout surface, compiled-source
/// Layer-3.
///
/// The layout builders (`el`/`row`/`column`/`wrappedRow`/`grid`/`paragraph`/
/// `textColumn`/`form`/`input`) are pure Ipê over two retained primitives —
/// `node`/`taggedNode` — point-free `Ffi.kernel "Ui_*"` aliases resolved by
/// `ipe_canon::resolve::detect_kernel_alias` to the retained `UiNode`/
/// `UiTaggedNode` kernels (runtime: `ipe_runtime::ui::helpers::*`). Every other
/// member (`layout`/`spacing`/`button`/`link`/`image`/the `on*` events/the
/// security-gated `mediaQuery`/`breakpoint`/`onPseudo`/the `desc*` roles/…)
/// stays native and is re-aliased here through the same mechanism, so its
/// bespoke emit arm is unchanged. The `Ipe.Ui.*` sub-modules (Background/Border/
/// Font/Region/Input/Lazy/Keyed) stay native kernel qualifiers. Registered in
/// [`COMPILED_STD_MODULES`] (NOT `MODULES`); NOT in `STDLIB_MODULE_QUALIFIERS`,
/// so the disjointness invariant holds.
const UI: &str = include_str!("../Ipe/Ui.ipe");
/// `Ipe.Regex` — RE2 regex helpers, compiled-source Layer-3.
///
/// The members are point-free `Ffi.kernel "Regex_*"` aliases resolved by the
/// kernel-alias mechanism (`ipe_canon::resolve::detect_kernel_alias`) to
/// the pure `RegexMatch`/`RegexFind`/… `StdlibKernel` variants. Registered in
/// [`COMPILED_STD_MODULES`] (NOT `MODULES`) so its body is actually compiled;
/// NOT in `STDLIB_MODULE_QUALIFIERS`, so the disjointness invariant holds.
const REGEX: &str = include_str!("../Ipe/Regex.ipe");
/// `Ipe.Url` — typed, validated URLs, compiled-source Layer-3.
///
/// The members are point-free `Ffi.kernel "Url_*"` aliases resolved by the
/// kernel-alias mechanism (`ipe_canon::resolve::detect_kernel_alias`) to the
/// pure `UrlFromString`/`UrlScheme`/… `StdlibKernel` variants (runtime:
/// `ipe_runtime::url::*`). Registered in [`COMPILED_STD_MODULES`] (NOT
/// `MODULES`) so its body is actually compiled; NOT in
/// `STDLIB_MODULE_QUALIFIERS`, so the disjointness invariant holds.
const URL: &str = include_str!("../Ipe/Url.ipe");

/// `Ipe.Url.Parser` — typed routing patterns over a parsed `Url`,
/// compiled-source Layer-3.
///
/// Pure Ipê source; no `Ffi.kernel` calls and no stored function values. A
/// `Pattern` is pure data (segment matchers plus query keys); `parse` matches it
/// against the shipped `Ipe.Url` accessors (`path`/`query`) — splitting path
/// segments and query pairs once over the already-parsed `Url`, no re-parse — and
/// yields the ordered `Captures`. Registered in [`COMPILED_STD_MODULES`] (NOT
/// `MODULES`); NOT in `STDLIB_MODULE_QUALIFIERS`, so the disjointness invariant
/// holds.
const URL_PARSER: &str = include_str!("../Ipe/Url/Parser.ipe");

/// `Ipe.Markdown` — markdown → `Ipe.Ui` Element renderer, compiled-source
/// Layer-3.
///
/// Pure Ipê source; no `Ffi.kernel` calls. The entire renderer (block
/// parser + inline span parser + `Ui.*` tree builder) is expressed in Ipê.
/// Output routes exclusively through typed `Ipe.Ui` constructors so no raw
/// HTML, scripts, or event handlers can reach the DOM — safe to feed
/// untrusted markdown without an extra sanitisation step.
///
/// NOT in `STDLIB_MODULE_QUALIFIERS`, so the disjointness invariant holds.
const STD_MARKDOWN: &str = include_str!("../Ipe/Markdown.ipe");

/// Every embedded `Ipe` module, keyed by its dotted import name.
pub const MODULES: &[StdModule] = &[
    StdModule {
        name: "Ipe.Basics",
        source: BASICS,
    },
    StdModule {
        name: "Ipe.Maybe",
        source: MAYBE,
    },
    StdModule {
        name: "Ipe.Result",
        source: RESULT,
    },
    StdModule {
        name: "Ipe.List",
        source: LIST,
    },
    StdModule {
        name: "Ipe.String",
        source: STRING,
    },
    StdModule {
        name: "Ipe.Char",
        source: CHAR,
    },
    StdModule {
        name: "Ipe.Dict",
        source: DICT,
    },
    StdModule {
        name: "Ipe.Set",
        source: SET,
    },
    StdModule {
        name: "Ipe.Bytes",
        source: BYTES,
    },
    StdModule {
        name: "Ipe.Crypto",
        source: CRYPTO,
    },
    StdModule {
        name: "Ipe.Bitwise",
        source: BITWISE,
    },
    StdModule {
        name: "Ipe.Task",
        source: TASK,
    },
    StdModule {
        name: "Ipe.Io",
        source: IO,
    },
    StdModule {
        name: "Ipe.Debug",
        source: DEBUG,
    },
    StdModule {
        name: "Ipe.Time",
        source: TIME,
    },
    StdModule {
        name: "Ipe.System",
        source: SYSTEM,
    },
    StdModule {
        name: "Ipe.File",
        source: FILE,
    },
    StdModule {
        name: "Ipe.Http",
        source: HTTP,
    },
    StdModule {
        name: "Ipe.Process",
        source: PROCESS,
    },
];

/// The embedded Ipê source for a dotted `Ipe` module name, or `None` when
/// the name is not one of the embedded modules.
#[must_use]
pub fn source(module_name: &str) -> Option<&'static str> {
    MODULES
        .iter()
        .find(|m| m.name == module_name)
        .map(|m| m.source)
}

// ===========================================================================
// Compiled-source stdlib modules — DISJOINT from `MODULES` above.
// ===========================================================================
//
// `MODULES` above is a PARSE-TEST fixture: those `Ipe.*` files are shadow
// copies whose real implementations are Rust kernels resolved by qualifier.
// `COMPILED_STD_MODULES` is the opposite: modules that are ACTUALLY compiled
// from Ipê source through the ordinary parse → canon → infer → lower → emit
// pipeline (a Std-source module that defines AND pattern-matches its own data
// type — the exact thing a kernel cannot express).
//
// A module is EITHER kernel-qualified (a member of `STDLIB_MODULE_QUALIFIERS`)
// OR compiled-source (here) — never both. `compiled_vs_kernel_qualifier_disjoint`
// enforces that invariant; a name in both would be pre-installed as a kernel
// qualifier AND injected as a source dep, giving ambiguous resolution.

/// One compiled-from-source standard-library module: its dotted name and its
/// embedded Ipê source.
pub struct CompiledStdModule {
    /// The dotted module name as written in an `import`, e.g. `Ipe.Palette`.
    pub dotted: &'static str,
    /// The module's Ipê source, embedded at compile time.
    pub source: &'static str,
}

/// `Ipe.Palette` — a Std-namespace module that defines `Shade`
/// and pattern-matches its own constructors in `toHex`.
const PALETTE: &str = include_str!("../Ipe/Palette.ipe");

/// `Ipe.Tuple` — pure pair helpers (elm/core `Tuple` parity).
///
/// Pure Ipê source; no `Ffi.kernel` calls — every helper pattern-matches or
/// builds a 2-tuple.  Not in `STDLIB_MODULE_QUALIFIERS` so the disjointness
/// invariant holds.
const TUPLE: &str = include_str!("../Ipe/Tuple.ipe");

/// `Ipe.Random.Generator` — composable, seeded, reproducible random
/// generators (elm/random `Generator` parity).
///
/// Pure Ipê source: defines the `Seed` union + a `Generator` type alias and
/// builds every combinator over the seeded primitives it draws through
/// `Ffi.kernel "Random_seededIntRaw"` / `"Random_seededFloatRaw"` (the pure
/// `RandomSeededInt`/`RandomSeededFloat` kernels, `ipe_runtime::random::*`).
/// The `Ipe.Random` KERNEL qualifier owns those primitives; this
/// compiled-source module is the DISTINCT `Ipe.Random.Generator` path, so the
/// disjointness invariant holds.
const RANDOM_GENERATOR: &str = include_str!("../Ipe/Random/Generator.ipe");

/// `Ipe.Css` — the typed stylesheet DSL, compiled pure Ipê source: it
/// defines AND pattern-matches its own `CssProp` / `CssRule` / `Length` /
/// `Color` / keyword-enum ADTs and folds them to a CSS string.  Its only Rust
/// surface is the four leaf security kernels under the `Ipe.CssSafety`
/// kernel qualifier (NOT under `Ipe.Css`, so the disjointness invariant holds).
const CSS: &str = include_str!("../Ipe/Css.ipe");

/// `Ipe.ToString` — naming-consistency surface.
///
/// Thin pure-Ipê aliases to canonical kernels in their home modules so callers
/// can write `ToString.fromInt n` without memorising the per-type kernel
/// sub-namespace.  `fromTime` is OMITTED pending the `Time_timeString` Rust
/// kernel.  Disjoint from `STDLIB_MODULE_QUALIFIERS` (no `"ToString"` entry
/// exists in `STDLIB_MODULE_QUALIFIERS`).
const TOSTRING_CORE: &str = include_str!("../Ipe/ToString.ipe");

/// `Ipe.Test` — lightweight in-process test framework.
///
/// Compiled pure-Ipê source that defines the `Test` / `TestResult` ADTs and
/// all assertion helpers.  `expectErrorKind` / `kindName` are OMITTED pending
/// the `Ipe.Error` compiled-source migration; `summarise` is pure (no IO).
/// Disjoint from `STDLIB_MODULE_QUALIFIERS` (no `"Test"` entry exists there).
const IPE_TEST: &str = include_str!("../Ipe/Test.ipe");

/// `Ipe.Web.Head` — typed `<head>` helpers for Ipe.Web per-page injection.
///
/// Helpers delegate to the `Html` kernel qualifier and the compiled-source
/// `Ipe.Html.Attributes` builders — no new kernel variants required.
/// `Ipe.Web.Head` is NOT in `STDLIB_MODULE_QUALIFIERS` (that table only has
/// `Ipe.Web` → `"Web"`), so the disjointness invariant holds.
const STD_LIVE_HEAD: &str = include_str!("../Ipe/Web/Head.ipe");

/// `Ipe.Web.Head.Unsafe` — the verbatim JSON-LD `<script>` injection hatch,
/// compiled-source Layer-3.
///
/// The single member `unsafeJsonLd` is a pure-Ipê definition over the `Html`
/// kernel qualifier (`Html.script`/`Html.text`) — the same body it had on
/// `Ipe.Web.Head`; only the surface home moved here so the raw-script sink no
/// longer resolves off a plain `Ipe.Web.Head` import. Importing this dotted
/// `Ipe.<M>.Unsafe` submodule discloses the `unsafe` capability. The emitted
/// output and the render sink are unchanged. `Ipe.Web.Head.Unsafe` is NOT in
/// `STDLIB_MODULE_QUALIFIERS` (no kernel qualifier), so the disjointness
/// invariant holds.
const STD_LIVE_HEAD_UNSAFE: &str = include_str!("../Ipe/Web/Head/Unsafe.ipe");

/// `Ipe.Ui.Responsive` — device-class helpers for responsive layout branching.
///
/// Pure Ipê source; no kernel calls.  Ported verbatim from
/// `../ipe/ipe-stdlib/Std/Ui/Responsive.ipe`.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
/// Unblocks `37-composite-live-shop` (N0004: Ipe.Ui.Responsive).
const STD_UI_RESPONSIVE: &str = include_str!("../Ipe/Ui/Responsive.ipe");

/// `Ipe.Ui.Chart` — pure-Ipê charting helpers (line, area, bar, sparkline, heatmap).
///
/// Depends on `Ui.colorCss` (kernel `UiColorCss`) to convert `Color` values to
/// CSS strings inside SVG attributes.  Ported verbatim from
/// `../ipe/ipe-stdlib/Std/Ui/Chart.ipe`.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
/// Unblocks `38-composite-ui-multibackend` (N0004: Ipe.Ui.Chart).
const STD_UI_CHART: &str = include_str!("../Ipe/Ui/Chart.ipe");

/// `Ipe.Ui.Grid` — typed CSS-grid track ADT + `columns`/`rows`/`tracks` builders.
///
/// Pure-Ipê; uses the native `Ui.gridTracks` kernel (`KernelFn::UiGridTracksRaw`)
/// that constructs `AttrGridTracks(cols, rows)`, rendered as `grid-template-columns`/
/// `grid-template-rows` by the web renderer and parsed by `tui/layout.rs`.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_UI_GRID: &str = include_str!("../Ipe/Ui/Grid.ipe");

/// `Ipe.Ui.Transition` — typed CSS transition `Step`/`Easing` ADTs +
/// `attribute`/`attributeUnsafe` builders.
///
/// Pure-Ipê; the `transition` primitive is a native `Ipe.Ui` kernel
/// (`KernelFn::UiTransitionRaw`) that constructs `AttrTransition shorthand
/// respect`, rendered by `src/runtime/rust/src/ui/render.rs`.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_UI_TRANSITION: &str = include_str!("../Ipe/Ui/Transition.ipe");

/// `Ipe.Ui.Transform` — typed CSS transform / opacity helpers for `Ui.animate`
/// keyframes. Pure Ipê; uses only `Ipe.*` internals — no
/// native primitive needed. Not in `STDLIB_MODULE_QUALIFIERS` so disjointness
/// invariant holds. Unblocks `26-ui-showcase` (IPE-N0004: Ipe.Ui.Transform).
const STD_UI_TRANSFORM: &str = include_str!("../Ipe/Ui/Transform.ipe");

/// `Ipe.Ui.Animation` — typed CSS keyframe-animation `Iterations`/`FillMode`
/// ADTs + `Spec` record + `attribute`/`defaultSpec`/`with*` builders.
///
/// Pure-Ipê; the `animate` primitive is a native `Ipe.Ui` kernel
/// (`KernelFn::UiAnimateRaw`, `String -> String -> String -> Bool -> Attribute`)
/// that constructs `AttrAnimation name shorthand keyframes respect`, rendered
/// by `src/runtime/rust/src/ui/render.rs` (inline `animation:` property) and
/// injected as an `@keyframes` block by `web::style_inject::build_anim`.
/// Depends on the sibling `Ipe.Ui.Transition` (`Easing`) and `Ipe.Ui.Transform`
/// (`Prop`/`propsToCss`).
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
/// Unblocks `26-ui-showcase` (IPE-N0004: Ipe.Ui.Animation — Animation.attribute).
const STD_UI_ANIMATION: &str = include_str!("../Ipe/Ui/Animation.ipe");

/// `Ipe.Codec` — one invariant codec that drives the JSON direction.
///
/// Pure Ipê source: defines the `Codec a` nominal union (an encoder plus a
/// decode-runner, both stored on the clonable shared-function carrier) and the
/// `map` bijection over the existing `Ipe.Json.Encode` / `Ipe.Json.Decode`
/// kernels — no new kernel, no `Ffi.kernel` call. The decode side is stored as
/// `String -> Result Error a` (a runner) rather than a bare `Decoder a` because
/// the runtime JSON decoder is a single-shot non-clonable carrier that cannot be
/// held in a reusable value. Not in `STDLIB_MODULE_QUALIFIERS` so the
/// disjointness invariant holds.
const STD_CODEC: &str = include_str!("../Ipe/Codec.ipe");

/// `Ipe.Money` — currency-typed Money on `Ipe.Decimal` + ISO 4217 enum.
///
/// Compiled pure-Ipê source: defines the `Money` / `Currency` ADTs and
/// pattern-matches their own constructors.  All `Ffi.callPure` calls from
/// the upstream Haskell stdlib have been replaced with pure Ipe
/// case-expressions / recursions.  The FX rate registry is stubbed.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
/// Unblocks `00-standard-libs` (N0004: Ipe.Money).
const STD_MONEY: &str = include_str!("../Ipe/Money.ipe");

/// `Ipe.WebSocket` — outbound WebSocket client (compiled source).
///
/// Defines 3 ADTs (`WebSocket`, `WebSocketMessage`, `CloseCode`) and routes its
/// I/O through `Ffi.kernel "WebSocket_*"` / `"Sub_subscribeWebSocket"` aliases.
/// RESOLVES (ipe-0 AND cargo-0): the six Task-tier `WebSocket_*` kernels plus
/// `Sub_subscribeWebSocket` are registered (`ipe_runtime::ws_client::*`). The
/// Sub-tier kernel is `any`-typed; the backend peephole splits it on the literal
/// `kind` into the four typed `sub_subscribe_ws_*` runtime fns. `connectWith`'s
/// `WebSocketCfg` record folds to the runtime `WsClientCfg` struct (mirrors the
/// `CacheCfg` fold). The `ws_client` runtime module + `tokio-tungstenite` dep are
/// gated behind the `websocket_client` feature the backend adds via
/// `uses_websocket`. Resolved via the `Ffi.kernel` alias fast-path, so the
/// `WebSocket` qualifier stays out of `STDLIB_MODULE_QUALIFIERS`.
const IPE_CORE_WEBSOCKET: &str = include_str!("../Ipe/WebSocket.ipe");

/// `Ipe.Env` — build-time-embedded public config (compiled source).
///
/// Defines `public : String -> Maybe String`, routed through the
/// `Ffi.kernel "Env_public"` alias to the registered `EnvPublic` kernel. The
/// generated `env_public.rs` (per-project, keyed on `ipe.toml`'s `[wasm]
/// publicEnv` allowlist) is what actually backs it — see
/// `ipe_backend_rust::project::render_env_public_rs`. Resolved via the
/// `Ffi.kernel` alias fast-path, so the `Env` qualifier stays out of
/// `STDLIB_MODULE_QUALIFIERS`.
const IPE_CORE_ENV: &str = include_str!("../Ipe/Env.ipe");

/// `Ipe.Cache` — in-memory LRU + TTL cache (compiled source).
///
/// Defines `type Cache k v = Cache Int` ADT.  RESOLVES (ipe-0 AND
/// cargo-0): the seven `Cache_*` kernels are registered
/// (`ipe_runtime::cache::*`; a faithful port of the reference's Go+Rust cache
/// kernels).  The opaque `Cache k v` is backed by the non-generic runtime
/// `IpeCacheHandle` (the phantom `k`/`v` are dropped, mirroring the reference's
/// `runtimeOpaqueTypes` mapping); `CacheCfg` / the `stats` return record fold to
/// the runtime `CacheCfg` / `CacheStats` structs (mirroring the reference's
/// struct-alias registry).
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_CACHE: &str = include_str!("../Ipe/Cache.ipe");

/// `Ipe.Compression` — gzip + zstd compression (compiled source).
///
/// KERNEL-BLOCKED: no `Compression_*` kernel variants exist — member use
/// fails closed with IPE-N0028.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_COMPRESSION: &str = include_str!("../Ipe/Compression.ipe");

/// `Ipe.Config` — typed TOML/YAML/JSON decoders (compiled source).
///
/// Defines `type Decoder a` — the SHARED opaque decoder carrier
/// (`IrType::Decoder`, runtime `ipe_runtime::json::Decoder<E, T>`), the same one
/// `Ipe.Json.Decode` names as a bare reserved builtin. RESOLVES (ipe-0
/// AND cargo-0): (a) `Ipe.Config`'s `Decoder` re-declaration is exempted
/// from IPE-N0026 via `ipe_canon`'s `STDLIB_DEFINABLE_CARRIER_TYPES` (trusted
/// `EmbeddedStdlib` origin only — user shadowing stays rejected); the ABOVE-guard
/// `Decoder` lowerer arm + `is_opaque_boxed_wrapper` make the re-declaration
/// lower to the shared carrier with no competing enum. (b) The 16 `Config_*`
/// kernels are registered across every anti-drift site; the 11 combinator/
/// primitive kernels emit the shared JSON `decode_*` runtime fns, the 5 format/
/// nullable/load kernels emit `ipe_runtime::config_decode::*` (vendored
/// unconditionally, same posture as Csv/Cache/Compression).
/// Not in `STDLIB_MODULE_QUALIFIERS`.
const STD_CONFIG: &str = include_str!("../Ipe/Config.ipe");

/// `Ipe.Csv` — CSV encode + decode (compiled source).
///
/// Defines `type alias Csv` + pure Ipê builders.  KERNEL-BLOCKED: no
/// `Csv_*` kernel variants exist — member use fails closed with IPE-N0028.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_CSV: &str = include_str!("../Ipe/Csv.ipe");

/// `Ipe.Email` — provider-abstract email send (compiled source).
///
/// Defines `type EmailProvider` + `type alias EmailMessage` ADTs.  KERNEL-BLOCKED:
/// no `Email_*` kernel variant exists — member use fails closed with
/// IPE-N0028.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_EMAIL: &str = include_str!("../Ipe/Email.ipe");

/// `Ipe.Web.Console` — typed console identity + builder helpers (compiled source).
///
/// Pure Ipê; no Ffi.kernel calls.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_LIVE_CONSOLE: &str = include_str!("../Ipe/Web/Console.ipe");

/// `Ipe.PubSub` — Task-shaped publish, callable from any context (compiled source).
///
/// Routes through `Ffi.kernel "PubSub_publish"` / `"PubSub_publishNoEcho"`.
/// RESOLVES (ipe-0 AND cargo-0): `PubSubPublish`/`PubSubPublishNoEcho` have a
/// type scheme (`String -> a -> Task Error Int`) and a dedicated emit arm
/// (`pubsub_publish::<_, IpeError>(topic, payload)`).  A member use exits ipe-0
/// AND cargo-0.  The payload `a` is a genuine monomorphized type var (concrete-
/// over-generic), never erased.  See `docs/divergences-from-sky.md`
/// §B-FfiKernelAliasSealed for the closed completeness gap.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_PUBSUB: &str = include_str!("../Ipe/PubSub.ipe");

/// `Ipe.Trace` — opt-in distributed-tracing spans (compiled source).
///
/// KERNEL-BLOCKED: no `Trace_*` kernel variants exist — member use fails
/// closed with IPE-N0028.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_TRACE: &str = include_str!("../Ipe/Trace.ipe");

/// `Ipe.Locale` — opaque BCP-47 locale handle + locale-aware case mapping.
///
/// `Locale.fromTag`/`Locale.toTag` route through `Ffi.kernel "Locale_*"` aliases
/// resolved by the kernel-alias mechanism to the registered
/// `LocaleFromTag`/`LocaleToTag` `StdlibKernel` variants
/// (`ipe_runtime::locale::*`).  `String.toUpperIn`/`toLowerIn` route to the
/// registered `StringToUpperIn`/`StringToLowerIn` variants likewise.
/// Registered in [`COMPILED_STD_MODULES`] (NOT `MODULES`); NOT in
/// `STDLIB_MODULE_QUALIFIERS`, so the disjointness invariant holds.
/// The runtime `locale` feature is required; programs that never import this
/// module pay no ICU4X dependency cost.
const LOCALE: &str = include_str!("../Ipe/Locale.ipe");

/// `Ipe.Ui.Events` — pure Ipê re-exports of `Ipe.Ui` event helpers (compiled source).
///
/// Pure Ipê; no Ffi.kernel calls.  RESOLVES (ipe-0 AND cargo-0): the
/// `onSubmit`/`onInput` re-exports are typed to the Rust kernels'
/// function-arg schemes (`(a -> msg) -> Attribute msg` /
/// `(String -> msg) -> Attribute msg`) — see `docs/divergences-from-sky.md`
/// §B-UiEventsFnArg.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_UI_EVENTS: &str = include_str!("../Ipe/Ui/Events.ipe");

/// Every compiled-source stdlib module, keyed by its dotted import name.
///
/// Disjoint from [`MODULES`] (parse fixtures) and from `ipe_canon`'s
/// `STDLIB_MODULE_QUALIFIERS` (kernel qualifiers) — see the module comment.
pub const COMPILED_STD_MODULES: &[CompiledStdModule] = &[
    CompiledStdModule {
        dotted: "Ipe.Palette",
        source: PALETTE,
    },
    CompiledStdModule {
        dotted: "Ipe.Tuple",
        source: TUPLE,
    },
    CompiledStdModule {
        dotted: "Ipe.Random",
        source: RANDOM,
    },
    CompiledStdModule {
        dotted: "Ipe.Random.Generator",
        source: RANDOM_GENERATOR,
    },
    CompiledStdModule {
        dotted: "Ipe.Css",
        source: CSS,
    },
    CompiledStdModule {
        dotted: "Ipe.ToString",
        source: TOSTRING_CORE,
    },
    CompiledStdModule {
        dotted: "Ipe.Test",
        source: IPE_TEST,
    },
    CompiledStdModule {
        dotted: "Ipe.Web.Head",
        source: STD_LIVE_HEAD,
    },
    // Ipe.Web.Head.Unsafe — Layer-3 source; the single `unsafeJsonLd` verbatim
    // JSON-LD `<script>` hatch, relocated out of `Ipe.Web.Head`. Its body is pure
    // Ipê over the `Html` kernel qualifier — no new kernel. Importing it discloses
    // the `unsafe` capability.
    CompiledStdModule {
        dotted: "Ipe.Web.Head.Unsafe",
        source: STD_LIVE_HEAD_UNSAFE,
    },
    CompiledStdModule {
        dotted: "Ipe.Ui.Responsive",
        source: STD_UI_RESPONSIVE,
    },
    CompiledStdModule {
        dotted: "Ipe.Ui.Chart",
        source: STD_UI_CHART,
    },
    CompiledStdModule {
        dotted: "Ipe.Ui.Grid",
        source: STD_UI_GRID,
    },
    CompiledStdModule {
        dotted: "Ipe.Ui.Transition",
        source: STD_UI_TRANSITION,
    },
    CompiledStdModule {
        dotted: "Ipe.Ui.Transform",
        source: STD_UI_TRANSFORM,
    },
    CompiledStdModule {
        dotted: "Ipe.Ui.Animation",
        source: STD_UI_ANIMATION,
    },
    CompiledStdModule {
        dotted: "Ipe.Codec",
        source: STD_CODEC,
    },
    CompiledStdModule {
        dotted: "Ipe.Money",
        source: STD_MONEY,
    },
    CompiledStdModule {
        dotted: "Ipe.WebSocket",
        source: IPE_CORE_WEBSOCKET,
    },
    CompiledStdModule {
        dotted: "Ipe.Env",
        source: IPE_CORE_ENV,
    },
    CompiledStdModule {
        dotted: "Ipe.Cache",
        source: STD_CACHE,
    },
    CompiledStdModule {
        dotted: "Ipe.Compression",
        source: STD_COMPRESSION,
    },
    CompiledStdModule {
        dotted: "Ipe.Config",
        source: STD_CONFIG,
    },
    CompiledStdModule {
        dotted: "Ipe.Csv",
        source: STD_CSV,
    },
    CompiledStdModule {
        dotted: "Ipe.Email",
        source: STD_EMAIL,
    },
    CompiledStdModule {
        dotted: "Ipe.Web.Console",
        source: STD_LIVE_CONSOLE,
    },
    CompiledStdModule {
        dotted: "Ipe.PubSub",
        source: STD_PUBSUB,
    },
    CompiledStdModule {
        dotted: "Ipe.Trace",
        source: STD_TRACE,
    },
    CompiledStdModule {
        dotted: "Ipe.Ui.Events",
        source: STD_UI_EVENTS,
    },
    // Ipe.Regex — Layer-3 source, `Ffi.kernel "Regex_*"` aliases route
    // to the registered pure `Regex*` kernels (`ipe_runtime::regex_kernel::*`).
    CompiledStdModule {
        dotted: "Ipe.Regex",
        source: REGEX,
    },
    // Ipe.Path — Layer-3 source, `Ffi.kernel "Path_*"` aliases route
    // to the registered pure `Path*` kernels (`ipe_runtime::path::*`).
    CompiledStdModule {
        dotted: "Ipe.Path",
        source: PATH,
    },
    // Ipe.Html.Attributes — Layer-3 source; fixed-key builders are pure Ipê over
    // the retained `Ffi.kernel "Attr_*"` primitives (`ipe_runtime::html::*`).
    CompiledStdModule {
        dotted: "Ipe.Html.Attributes",
        source: HTML_ATTRIBUTES,
    },
    // Ipe.Html.Unsafe — Layer-3 source; the single `unsafeRaw` escape hatch is a
    // `Ffi.kernel "Html_unsafeRaw"` alias to the unchanged `HtmlRawNode` kernel.
    // Importing it discloses the `unsafe` capability.
    CompiledStdModule {
        dotted: "Ipe.Html.Unsafe",
        source: HTML_UNSAFE,
    },
    // Ipe.Db.Unsafe — Layer-3 source; the raw-SQL / untyped-read escape hatches
    // are `Ffi.kernel "Db_*"` / `"Sql_unsafeFragment"` aliases to unchanged (and
    // one new) kernels. Importing it discloses the `unsafe` capability.
    CompiledStdModule {
        dotted: "Ipe.Db.Unsafe",
        source: DB_UNSAFE,
    },
    // Ipe.Secret.Unsafe — Layer-3 source; the single `unsafeReveal` escape hatch
    // is a `Ffi.kernel "Secret_reveal"` alias to the unchanged `SecretReveal`
    // kernel. Importing it discloses the `unsafe` capability. The scoped
    // `Secret.use` stays on the native `Ipe.Secret` surface (capability-neutral).
    CompiledStdModule {
        dotted: "Ipe.Secret.Unsafe",
        source: SECRET_UNSAFE,
    },
    // Ipe.Html — Layer-3 source; element builders are pure Ipê over the retained
    // `Ffi.kernel "Html_node"` / `"Html_voidNode"` primitives, with the native
    // serialiser (`render`/`escape*`) re-aliased (`ipe_runtime::ui::helpers::*`).
    CompiledStdModule {
        dotted: "Ipe.Html",
        source: HTML,
    },
    // Ipe.Ui — Layer-3 source; the layout builders are pure Ipê over the retained
    // `Ffi.kernel "Ui_node"` / `"Ui_taggedNode"` primitives, with every other
    // member re-aliased to its unchanged native kernel (`ipe_runtime::ui::*`).
    CompiledStdModule {
        dotted: "Ipe.Ui",
        source: UI,
    },
    // Ipe.Markdown — pure Ipê markdown→Ui renderer; no kernel calls.
    CompiledStdModule {
        dotted: "Ipe.Markdown",
        source: STD_MARKDOWN,
    },
    // Ipe.Url — Layer-3 source, `Ffi.kernel "Url_*"` aliases route to the
    // registered pure `Url*` kernels (`ipe_runtime::url::*`).
    CompiledStdModule {
        dotted: "Ipe.Url",
        source: URL,
    },
    // Ipe.Url.Parser — pure-Ipê routing combinators over the shipped `Ipe.Url`
    // accessors; no kernel calls.
    CompiledStdModule {
        dotted: "Ipe.Url.Parser",
        source: URL_PARSER,
    },
    // Ipe.Locale — opaque BCP-47 locale handle + locale-aware case mapping.
    // `Locale.fromTag`/`Locale.toTag` resolve via `Ffi.kernel "Locale_*"`;
    // `String.toUpperIn`/`toLowerIn` resolve via `Ffi.kernel "String_toUpperIn"`
    // / `"String_toLowerIn"`.  The runtime module is `ipe_runtime::locale::*`
    // (feature `locale`).  Disjoint from `STDLIB_MODULE_QUALIFIERS` (no
    // `"Locale"` entry there), so the invariant holds.
    CompiledStdModule {
        dotted: "Ipe.Locale",
        source: LOCALE,
    },
];

/// The embedded Ipê source for a compiled-source stdlib module named by its path
/// SEGMENTS (e.g. `["Std", "Palette"]`), or `None` when the segments name no
/// compiled-source module.
///
/// Segment-based (rather than `Symbol`-based) so it composes directly with the
/// build driver's `Vec<String>` module paths without an interner round-trip.
#[must_use]
pub fn compiled_std_source_segments(segments: &[String]) -> Option<&'static str> {
    let dotted = segments.join(".");
    COMPILED_STD_MODULES
        .iter()
        .find(|m| m.dotted == dotted)
        .map(|m| m.source)
}

/// Whether `segments` name a compiled-source stdlib module.
#[must_use]
pub fn is_compiled_source_segments(segments: &[String]) -> bool {
    compiled_std_source_segments(segments).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipe_intern::Interner;

    /// Every embedded `Ipe` module must PARSE with the same front end that
    /// reads user code — the proof that the compiler can read its own embedded
    /// standard library (the foundation the import resolver builds on).
    #[test]
    fn every_embedded_module_parses() {
        for m in MODULES {
            let mut interner = Interner::new();
            let parsed = ipe_parse::parse_module(m.source, &mut interner);
            assert!(
                parsed.is_ok(),
                "embedded module {} must parse: {:?}",
                m.name,
                parsed.err()
            );
        }
    }

    /// `Ipe.Basics` resolves to its embedded source; the removed `Ipe.Prelude`
    /// alias does not resolve to anything (no backward-compatible mapping).
    #[test]
    fn basics_resolves_and_prelude_is_gone() {
        assert_eq!(source("Ipe.Basics"), Some(BASICS));
        assert_eq!(source("Ipe.Prelude"), None);
    }

    /// An unknown `Ipe` module is not embedded.
    #[test]
    fn unknown_module_is_absent() {
        assert_eq!(source("Ipe.Nope"), None);
    }

    /// Every compiled-source module must PARSE with the real front end — the
    /// PARSE-DON'T-VALIDATE floor: a module cannot enter any build graph until it
    /// is proven to parse with the same parser that reads user code.
    #[test]
    fn every_compiled_source_module_parses() {
        for m in COMPILED_STD_MODULES {
            let mut interner = Interner::new();
            let parsed = ipe_parse::parse_module(m.source, &mut interner);
            assert!(
                parsed.is_ok(),
                "compiled-source module {} must parse: {:?}",
                m.dotted,
                parsed.err()
            );
        }
    }

    /// Load-bearing invariant (design §2.1): a module is EITHER a kernel
    /// qualifier OR a compiled-source module, never both. A name in both would be
    /// pre-installed as a kernel qualifier AND injected as a source dep — an
    /// ambiguous resolution / silent miscompile.
    #[test]
    fn compiled_vs_kernel_qualifier_disjoint() {
        for m in COMPILED_STD_MODULES {
            let segments: Vec<&str> = m.dotted.split('.').collect();
            let clash = ipe_canon::STDLIB_MODULE_QUALIFIERS
                .iter()
                .any(|(path, _)| *path == segments.as_slice());
            assert!(
                !clash,
                "{} is BOTH a compiled-source module and a kernel qualifier — \
                 the two tables must be disjoint",
                m.dotted
            );
        }
    }

    /// Anti-drift (parse-level floor): every EXPORTED VALUE of a compiled-source
    /// stdlib module has a source home — either a local top-level binding (a pure
    /// body OR an `Ffi.kernel "…"` alias) OR a name pulled in by an `import …
    /// exposing (name)` re-export. An exported value that is neither is the
    /// source-vs-kernel drift class: a member declared in `exposing (...)` with no
    /// body, which fails name-resolution at every call site. Catching it here makes
    /// a declared-but-homeless export a build-time (CI) failure, pre-cargo.
    ///
    /// Scope: VALUES only. An exported TYPE may legitimately be a kernel-provided
    /// opaque (e.g. `Ipe.Path`'s `Path`) with no source declaration — its home is
    /// the kernel registry, which this parse-only check cannot see. The deeper
    /// "every export — types included — resolves through the real pipeline" guard
    /// is `compiled_source_modules_resolve_all_exports` in the `ipe` crate, which
    /// canonicalises each module against the kernel env. `exposing (..)` (export
    /// all) has nothing to cross-check and is skipped.
    #[test]
    fn every_exported_value_has_a_home() {
        use ipe_syntax::{Exposed, Exposing};

        for m in COMPILED_STD_MODULES {
            let mut interner = Interner::new();
            let parsed = ipe_parse::parse_module(m.source, &mut interner);
            assert!(
                parsed.is_ok(),
                "compiled-source module {} must parse before its exports \
                 can be checked: {:?}",
                m.dotted,
                parsed.err(),
            );
            let Ok(parsed) = parsed else { continue };

            let exposed = match &parsed.exposing.value {
                Exposing::All => continue,
                Exposing::List(items) => items,
            };

            // A name re-exported from an `import … exposing (name)` (or from an
            // export-all import, which surfaces every name of the imported module)
            // is a resolvable home even without a local declaration.
            let imported = |name| {
                parsed.imports.iter().any(|imp| match &imp.exposing.value {
                    Exposing::All => true,
                    Exposing::List(items) => items.iter().any(|e| match &e.value {
                        Exposed::Value(n) | Exposed::Type(n, _) => *n == name,
                    }),
                })
            };

            for item in exposed {
                if let Exposed::Value(name) = &item.value {
                    let local = parsed.values.iter().any(|v| v.value.name.value == *name);
                    let rendered = interner.resolve(*name).unwrap_or("<?>");
                    assert!(
                        local || imported(*name),
                        "{}: exports value `{rendered}` but the module neither \
                         defines a top-level binding for it nor re-exports it \
                         from an import — a declared-but-homeless export \
                         (source-vs-kernel drift)",
                        m.dotted,
                    );
                }
            }
        }
    }

    /// Segment lookup resolves a compiled-source module and rejects a non-member.
    #[test]
    fn compiled_source_segment_lookup() {
        let palette = vec!["Ipe".to_owned(), "Palette".to_owned()];
        assert!(is_compiled_source_segments(&palette));
        assert!(compiled_std_source_segments(&palette).is_some());

        let log = vec!["Ipe".to_owned(), "Log".to_owned()];
        assert!(!is_compiled_source_segments(&log), "Ipe.Log is a kernel");

        let nope = vec!["Ipe".to_owned(), "Nope".to_owned()];
        assert!(!is_compiled_source_segments(&nope));
    }
}
