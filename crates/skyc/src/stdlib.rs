//! The embedded Sky standard-library source (`Sky.Core.*`).
//!
//! `skyc` is self-contained: the foundational `Sky.Core` modules are compiled
//! into the binary as their original Sky source (a port of the Haskell
//! compiler's Template-Haskell embedding of `sky-stdlib/`). The checked-in copies
//! under `crates/skyc/stdlib/Sky/Core/` are byte-identical to the upstream
//! `sky-stdlib` sources; embedding a copy (rather than `include_str!`-ing an
//! out-of-repo path) keeps the build portable and the toolchain hermetic.
//!
//! M4a embeds the foundational set — `Basics`, `Maybe`, `Result`, `List` — and
//! resolves `Sky.Core.Prelude` to `Basics` (the Prelude re-exports the
//! non-numeric basics, exactly as the reference compiler maps it). M4b adds
//! `Sky.Core.String` and `Sky.Core.Char`. The source is ordinary Sky: the same
//! parser that reads user code reads it (the `parses` test proves it), so it is
//! the substrate the import resolver compiles once whole-program
//! let-generalisation lands.

/// One embedded standard-library module: its dotted name and its Sky source.
pub struct StdModule {
    /// The dotted module name as written in an `import`, e.g. `Sky.Core.Maybe`.
    pub name: &'static str,
    /// The module's Sky source, embedded at compile time.
    pub source: &'static str,
}

/// `Sky.Core.Basics` — `identity` / `always` / `not` / `fst` / `snd` / `clamp`.
const BASICS: &str = include_str!("../stdlib/Sky/Core/Basics.sky");
/// `Sky.Core.Maybe` — combinators over the `Maybe` ADT.
const MAYBE: &str = include_str!("../stdlib/Sky/Core/Maybe.sky");
/// `Sky.Core.Result` — combinators over the `Result` ADT.
const RESULT: &str = include_str!("../stdlib/Sky/Core/Result.sky");
/// `Sky.Core.List` — list combinators.
const LIST: &str = include_str!("../stdlib/Sky/Core/List.sky");
/// `Sky.Core.String` — string combinators (M4b).
const STRING: &str = include_str!("../stdlib/Sky/Core/String.sky");
/// `Sky.Core.Char` — single-character helpers (M4b).
const CHAR: &str = include_str!("../stdlib/Sky/Core/Char.sky");
/// `Sky.Core.Dict` — string-keyed associative map (M4d).
const DICT: &str = include_str!("../stdlib/Sky/Core/Dict.sky");
/// `Sky.Core.Set` — unordered set of unique elements (M4d).
const SET: &str = include_str!("../stdlib/Sky/Core/Set.sky");
/// `Sky.Core.Bytes` — arbitrary byte buffer, distinct from `String` (M4e).
///
/// Divergence from Sky: Sky defines `type alias Bytes = String`; Sky-Rust
/// makes `Bytes` a distinct primitive lowering to `Vec<u8>` (lossless for
/// non-UTF-8 binary). See `docs/architecture/divergence-policy.md`.
const BYTES: &str = include_str!("../stdlib/Sky/Core/Bytes.sky");
/// `Sky.Core.Crypto` — hashes / HMAC / RSA / AEAD / key-derivation / random (M5a).
const CRYPTO: &str = include_str!("../stdlib/Sky/Core/Crypto.sky");
/// `Sky.Core.Task` — Task combinator surface (M5a).
const TASK: &str = include_str!("../stdlib/Sky/Core/Task.sky");
/// `Sky.Core.Io` — standard-I/O effect kernels (M5a).
const IO: &str = include_str!("../stdlib/Sky/Core/Io.sky");
/// `Sky.Core.Time` — time effect kernels (M5a).
const TIME: &str = include_str!("../stdlib/Sky/Core/Time.sky");
/// `Sky.Core.System` — process / environment effect kernels (M5a).
const SYSTEM: &str = include_str!("../stdlib/Sky/Core/System.sky");
/// `Sky.Core.Random` — entropy-backed randomness effect kernels (M5a).
const RANDOM: &str = include_str!("../stdlib/Sky/Core/Random.sky");
/// `Sky.Core.File` — file-system effect kernels (M5a).
const FILE: &str = include_str!("../stdlib/Sky/Core/File.sky");
/// `Sky.Core.Http` — outbound HTTP client kernels + pure builders (M5b).
const HTTP: &str = include_str!("../stdlib/Sky/Core/Http.sky");

/// `Sky.Core.Path` — pure filesystem-path helpers, compiled-source Layer-3 (#202).
///
/// The members are point-free `Ffi.kernel "Path_*"` aliases resolved by the
/// #196 kernel-alias mechanism (`sky_canon::resolve::detect_kernel_alias`) to
/// the pure `PathBase`/`PathDir`/`PathExt`/`PathIsAbsolute` `StdlibKernel`
/// variants. Registered in [`COMPILED_STD_MODULES`] (NOT `MODULES`) so its body
/// is actually compiled; NOT in `STDLIB_MODULE_QUALIFIERS`, so the disjointness
/// invariant holds.
const PATH: &str = include_str!("../stdlib/Sky/Core/Path.sky");
/// `Sky.Core.Regex` — RE2 regex helpers, compiled-source Layer-3 (#194).
///
/// The members are point-free `Ffi.kernel "Regex_*"` aliases resolved by the
/// #196 kernel-alias mechanism (`sky_canon::resolve::detect_kernel_alias`) to
/// the pure `RegexMatch`/`RegexFind`/… `StdlibKernel` variants. Registered in
/// [`COMPILED_STD_MODULES`] (NOT `MODULES`) so its body is actually compiled;
/// NOT in `STDLIB_MODULE_QUALIFIERS`, so the disjointness invariant holds.
const REGEX: &str = include_str!("../stdlib/Sky/Core/Regex.sky");

/// Every embedded `Sky.Core` module, keyed by its dotted import name.
///
/// `Sky.Core.Prelude` is intentionally absent here: it is not a source file but
/// an alias for `Basics` (the Prelude re-exports the non-numeric basics), so
/// [`source`] maps it onto `Basics` rather than a distinct entry.
pub const MODULES: &[StdModule] = &[
    StdModule {
        name: "Sky.Core.Basics",
        source: BASICS,
    },
    StdModule {
        name: "Sky.Core.Maybe",
        source: MAYBE,
    },
    StdModule {
        name: "Sky.Core.Result",
        source: RESULT,
    },
    StdModule {
        name: "Sky.Core.List",
        source: LIST,
    },
    StdModule {
        name: "Sky.Core.String",
        source: STRING,
    },
    StdModule {
        name: "Sky.Core.Char",
        source: CHAR,
    },
    StdModule {
        name: "Sky.Core.Dict",
        source: DICT,
    },
    StdModule {
        name: "Sky.Core.Set",
        source: SET,
    },
    StdModule {
        name: "Sky.Core.Bytes",
        source: BYTES,
    },
    StdModule {
        name: "Sky.Core.Crypto",
        source: CRYPTO,
    },
    StdModule {
        name: "Sky.Core.Task",
        source: TASK,
    },
    StdModule {
        name: "Sky.Core.Io",
        source: IO,
    },
    StdModule {
        name: "Sky.Core.Time",
        source: TIME,
    },
    StdModule {
        name: "Sky.Core.System",
        source: SYSTEM,
    },
    StdModule {
        name: "Sky.Core.Random",
        source: RANDOM,
    },
    StdModule {
        name: "Sky.Core.File",
        source: FILE,
    },
    StdModule {
        name: "Sky.Core.Http",
        source: HTTP,
    },
];

/// The embedded Sky source for a dotted `Sky.Core` module name, or `None` when
/// the name is not one of the embedded modules.
///
/// `Sky.Core.Prelude` resolves to the `Basics` source (the Prelude is an alias
/// re-export of the non-numeric basics, matching the reference compiler's
/// `("Sky.Core.Prelude", "Basics")` mapping).
#[must_use]
pub fn source(module_name: &str) -> Option<&'static str> {
    if module_name == "Sky.Core.Prelude" {
        return Some(BASICS);
    }
    MODULES
        .iter()
        .find(|m| m.name == module_name)
        .map(|m| m.source)
}

// ===========================================================================
// Compiled-source stdlib modules (#98) — DISJOINT from `MODULES` above.
// ===========================================================================
//
// `MODULES` above is a PARSE-TEST fixture: those `Sky.Core.*` files are shadow
// copies whose real implementations are Rust kernels resolved by qualifier.
// `COMPILED_STD_MODULES` is the opposite: modules that are ACTUALLY compiled
// from Sky source through the ordinary parse → canon → infer → lower → emit
// pipeline (a Std-source module that defines AND pattern-matches its own data
// type — the exact thing a kernel cannot express).
//
// A module is EITHER kernel-qualified (a member of `STDLIB_MODULE_QUALIFIERS`)
// OR compiled-source (here) — never both. `compiled_vs_kernel_qualifier_disjoint`
// enforces that invariant; a name in both would be pre-installed as a kernel
// qualifier AND injected as a source dep, giving ambiguous resolution.

/// One compiled-from-source standard-library module: its dotted name and its
/// embedded Sky source.
pub struct CompiledStdModule {
    /// The dotted module name as written in an `import`, e.g. `Std.Palette`.
    pub dotted: &'static str,
    /// The module's Sky source, embedded at compile time.
    pub source: &'static str,
}

/// `Std.Palette` — the #98 spike: a Std-namespace module that defines `Shade`
/// and pattern-matches its own constructors in `toHex`.
const PALETTE: &str = include_str!("../stdlib/Std/Palette.sky");

/// `Std.Css` (#47) — the typed stylesheet DSL, compiled pure Sky source: it
/// defines AND pattern-matches its own `CssProp` / `CssRule` / `Length` /
/// `Color` / keyword-enum ADTs and folds them to a CSS string.  Its only Rust
/// surface is the four leaf security kernels under the `Sky.Core.CssSafety`
/// kernel qualifier (NOT under `Std.Css`, so the disjointness invariant holds).
const CSS: &str = include_str!("../stdlib/Std/Css.sky");

/// `Sky.Core.ToString` (#80) — naming-consistency surface (v0.15.48+).
///
/// Thin pure-Sky aliases to canonical kernels in their home modules so callers
/// can write `ToString.fromInt n` without memorising the per-type kernel
/// sub-namespace.  `fromTime` is OMITTED pending the `Time_timeString` Rust
/// kernel.  Disjoint from `STDLIB_MODULE_QUALIFIERS` (no `"ToString"` entry
/// exists in `STDLIB_MODULE_QUALIFIERS`).
const TOSTRING_CORE: &str = include_str!("../stdlib/Sky/Core/ToString.sky");

/// `Sky.Test` (#80) — lightweight in-process test framework.
///
/// Compiled pure-Sky source that defines the `Test` / `TestResult` ADTs and
/// all assertion helpers.  `expectErrorKind` / `kindName` are OMITTED pending
/// the `Sky.Core.Error` compiled-source migration; `summarise` is pure (no IO).
/// Disjoint from `STDLIB_MODULE_QUALIFIERS` (no `"Test"` entry exists there).
const SKY_TEST: &str = include_str!("../stdlib/Sky/Test.sky");

/// `Std.Live.Head` (#98) — typed `<head>` helpers for Sky.Live per-page injection.
///
/// Faithfully ported from `../sky/sky-stdlib/Std/Live/Head.sky`.
/// All helpers delegate to existing M7 kernel qualifiers (`Html` / `Attr`) —
/// no new kernel variants required.  `Std.Live.Head` is NOT in
/// `STDLIB_MODULE_QUALIFIERS` (that table only has `Std.Live` → `"Live"`),
/// so the disjointness invariant holds.
///
/// Unblocks `38-composite-ui-multibackend` (N0004: Std.Live.Head).
const STD_LIVE_HEAD: &str = include_str!("../stdlib/Std/Live/Head.sky");

/// `Std.Ui.Responsive` — device-class helpers for responsive layout branching.
///
/// Pure Sky source; no kernel calls.  Ported verbatim from
/// `../sky/sky-stdlib/Std/Ui/Responsive.sky`.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
/// Unblocks `37-composite-live-shop` (N0004: Std.Ui.Responsive).
const STD_UI_RESPONSIVE: &str = include_str!("../stdlib/Std/Ui/Responsive.sky");

/// `Std.Ui.Chart` — pure-Sky charting helpers (line, area, bar, sparkline, heatmap).
///
/// Depends on `Ui.colorCss` (kernel `UiColorCss`) to convert `Color` values to
/// CSS strings inside SVG attributes.  Ported verbatim from
/// `../sky/sky-stdlib/Std/Ui/Chart.sky`.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
/// Unblocks `38-composite-ui-multibackend` (N0004: Std.Ui.Chart).
const STD_UI_CHART: &str = include_str!("../stdlib/Std/Ui/Chart.sky");

/// `Std.Ui.Grid` — typed CSS-grid track ADT + `columns`/`rows`/`tracks` builders.
///
/// Pure-Sky; uses the native `Ui.gridTracksRaw` kernel (`KernelFn::UiGridTracksRaw`)
/// that constructs `AttrGridTracks(cols, rows)`, rendered as `grid-template-columns`/
/// `grid-template-rows` by the web renderer and parsed by `tui/layout.rs`.
/// Ported from `../sky/sky-stdlib/Std/Ui/Grid.sky`; divergence recorded in
/// `docs/divergences-from-sky.md` (typed carrier vs reference's sentinel approach).
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
/// Unblocks `26-ui-showcase` (SKY-N0004: Std.Ui.Grid — Grid.columns/fr/px).
const STD_UI_GRID: &str = include_str!("../stdlib/Std/Ui/Grid.sky");

/// `Std.Ui.Transition` — typed CSS transition `Step`/`Easing` ADTs +
/// `attribute`/`attributeUnsafe` builders.
///
/// Pure-Sky; the `transitionRaw` primitive is a native `Std.Ui` kernel
/// (`KernelFn::UiTransitionRaw`) that constructs `AttrTransition shorthand
/// respect`, rendered by `runtime/src/sky_runtime/ui/render.rs`.  Ported from
/// `../sky/sky-stdlib/Std/Ui/Transition.sky`; the reference's
/// `import Std.Ui exposing (transitionRaw)` is qualified to `Ui.transitionRaw`
/// (mirrors the `Std.Ui.Grid` port's `Ui.style` call).
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
/// Unblocks `26-ui-showcase` (SKY-N0004: Std.Ui.Transition).
const STD_UI_TRANSITION: &str = include_str!("../stdlib/Std/Ui/Transition.sky");

/// `Std.Ui.Transform` — typed CSS transform / opacity helpers for `Ui.animate`
/// keyframes (issue #378). Pure Sky; uses only `Sky.Core.*` internals — no
/// native primitive needed. Not in `STDLIB_MODULE_QUALIFIERS` so disjointness
/// invariant holds. Unblocks `26-ui-showcase` (SKY-N0004: Std.Ui.Transform).
const STD_UI_TRANSFORM: &str = include_str!("../stdlib/Std/Ui/Transform.sky");

/// `Std.Ui.Animation` — typed CSS keyframe-animation `Iterations`/`FillMode`
/// ADTs + `Spec` record + `attribute`/`defaultSpec`/`with*` builders.
///
/// Pure-Sky; the `animateRaw` primitive is a native `Std.Ui` kernel
/// (`KernelFn::UiAnimateRaw`, `String -> String -> String -> Bool -> Attribute`)
/// that constructs `AttrAnimation name shorthand keyframes respect`, rendered
/// by `runtime/src/sky_runtime/ui/render.rs` (inline `animation:` property) and
/// injected as an `@keyframes` block by `live::style_inject::build_anim`.
/// Ported from `../sky/sky-stdlib/Std/Ui/Animation.sky`; the reference's
/// `import Std.Ui exposing (animateRaw)` is qualified to `Ui.animateRaw`
/// (mirrors the `Std.Ui.Transition` port's `Ui.transitionRaw` call).
/// Depends on the sibling `Std.Ui.Transition` (`Easing`) and `Std.Ui.Transform`
/// (`Prop`/`propsToCss`) ports.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
/// Unblocks `26-ui-showcase` (SKY-N0004: Std.Ui.Animation — Animation.attribute).
const STD_UI_ANIMATION: &str = include_str!("../stdlib/Std/Ui/Animation.sky");

/// `Std.Money` — currency-typed Money on `Std.Decimal` + ISO 4217 enum.
///
/// Compiled pure-Sky source: defines the `Money` / `Currency` ADTs and
/// pattern-matches their own constructors.  All `Ffi.callPure` calls from
/// the upstream Haskell stdlib have been replaced with pure Sky
/// case-expressions / recursions.  The FX rate registry is stubbed.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
/// Unblocks `00-standard-libs` (N0004: Std.Money).
const STD_MONEY: &str = include_str!("../stdlib/Std/Money.sky");

/// `Sky.Core.Pure` — uniform `() -> Task Error a` companion surface.
///
/// The point-free helpers `uuidV4`/`uuidV7` route through internal
/// `Ffi.kernel "Uuid_v4"`/`"Uuid_v7"` aliases (resolved by the #196 kernel-alias
/// mechanism, `sky_canon::resolve::detect_kernel_alias`).  ARITY-BLOCKED: the
/// `uuidV4Kernel`/`uuidV7Kernel` helpers annotate an arity-0 `Task Error String`
/// value over the arity-1 `Uuid_v4`/`Uuid_v7` kernels (`() -> Task Error String`),
/// so they are rejected with SKY-T0001 at type-check until an
/// arity-0-alias-of-nullary-effect-kernel lowering exists.  See
/// `docs/divergences-from-sky.md` §B-FfiKernelAliasSealed.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const SKY_CORE_PURE: &str = include_str!("../stdlib/Sky/Core/Pure.sky");

/// `Sky.Core.WebSocket` — outbound WebSocket client (compiled source).
///
/// Defines 3 ADTs (`WebSocket`, `WebSocketMessage`, `CloseCode`) and routes its
/// I/O through `Ffi.kernel "WebSocket_*"` / `"Sub_subscribeWebSocket"` aliases.
/// KERNEL-BLOCKED (#196): none of the `WebSocket_*` kernels nor
/// `Sub_subscribeWebSocket` have a `StdlibKernel` variant, so importing a member
/// fails closed with SKY-N0028 (`docs/divergences-from-sky.md`
/// §B-FfiKernelAliasSealed).  Unblocked once the kernels are registered.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const SKY_CORE_WEBSOCKET: &str = include_str!("../stdlib/Sky/Core/WebSocket.sky");

/// `Std.Cache` — in-memory LRU + TTL cache (compiled source).
///
/// Defines `type Cache k v = Cache Int` ADT.  RESOLVES (#210, skyc-0 AND
/// cargo-0): the seven `Cache_*` kernels are registered
/// (`sky_runtime::cache::*`; a faithful port of the reference's Go+Rust cache
/// kernels).  The opaque `Cache k v` is backed by the non-generic runtime
/// `SkyCacheHandle` (the phantom `k`/`v` are dropped, mirroring the reference's
/// `runtimeOpaqueTypes` mapping); `CacheCfg` / the `stats` return record fold to
/// the runtime `CacheCfg` / `CacheStats` structs (mirroring the reference's
/// struct-alias registry).
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_CACHE: &str = include_str!("../stdlib/Std/Cache.sky");

/// `Std.Compression` — gzip + zstd compression (compiled source).
///
/// KERNEL-BLOCKED (#196): no `Compression_*` kernel variants exist — member use
/// fails closed with SKY-N0028.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_COMPRESSION: &str = include_str!("../stdlib/Std/Compression.sky");

/// `Std.Config` — typed TOML/YAML/JSON decoders (compiled source).
///
/// Defines `type Decoder a`.  DOUBLE-BLOCKED (#196): (a) `Decoder` collides with
/// the reserved parametric builtin type (SKY-N0026 at its declaration — the Rust
/// lowerer's `ir_type_from_ty` reserves `Decoder`), and (b) no `Config_*` kernel
/// variants exist (SKY-N0028).  Both must be resolved before it compiles.
/// Not in `STDLIB_MODULE_QUALIFIERS`.
const STD_CONFIG: &str = include_str!("../stdlib/Std/Config.sky");

/// `Std.Csv` — CSV encode + decode (compiled source).
///
/// Defines `type alias Csv` + pure Sky builders.  KERNEL-BLOCKED (#196): no
/// `Csv_*` kernel variants exist — member use fails closed with SKY-N0028.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_CSV: &str = include_str!("../stdlib/Std/Csv.sky");

/// `Std.Email` — provider-abstract email send (compiled source).
///
/// Defines `type EmailProvider` + `type alias EmailMessage` ADTs.  KERNEL-BLOCKED
/// (#196): no `Email_*` kernel variant exists — member use fails closed with
/// SKY-N0028.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_EMAIL: &str = include_str!("../stdlib/Std/Email.sky");

/// `Std.Live.Console` — typed console identity + builder helpers (compiled source).
///
/// Pure Sky; no Ffi.kernel calls.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_LIVE_CONSOLE: &str = include_str!("../stdlib/Std/Live/Console.sky");

/// `Std.PubSub` — Task-shaped publish, callable from any context (compiled source).
///
/// Routes through `Ffi.kernel "PubSub_publish"` / `"PubSub_publishNoEcho"`.
/// LOWERING-BLOCKED (#196): the `PubSubPublish`/`PubSubPublishNoEcho` kernels are
/// in the registry (the runtime `pubsub_publish` exists) but have NO lower/emit
/// arm, so a member use fails closed with SKY-L0108 at lowering (never a
/// cargo-fail).  Unblocked once the TEA lower + emit arms are added.  See
/// `docs/divergences-from-sky.md` §B-FfiKernelAliasSealed.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_PUBSUB: &str = include_str!("../stdlib/Std/PubSub.sky");

/// `Std.Trace` — opt-in distributed-tracing spans (compiled source).
///
/// KERNEL-BLOCKED (#196): no `Trace_*` kernel variants exist — member use fails
/// closed with SKY-N0028.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_TRACE: &str = include_str!("../stdlib/Std/Trace.sky");

/// `Std.Ui.Events` — pure Sky re-exports of `Std.Ui` event helpers (compiled source).
///
/// Pure Sky; no Ffi.kernel calls.  RESOLVES (#196, skyc-0 AND cargo-0): the
/// `onSubmit`/`onInput` re-exports were re-typed to the Rust kernels'
/// function-arg schemes (`(a -> msg) -> Attribute msg` /
/// `(String -> msg) -> Attribute msg`) — see `docs/divergences-from-sky.md`
/// §B-UiEventsFnArg.
/// Not in `STDLIB_MODULE_QUALIFIERS` so disjointness invariant holds.
const STD_UI_EVENTS: &str = include_str!("../stdlib/Std/Ui/Events.sky");

/// Every compiled-source stdlib module, keyed by its dotted import name.
///
/// Disjoint from [`MODULES`] (parse fixtures) and from `sky_canon`'s
/// `STDLIB_MODULE_QUALIFIERS` (kernel qualifiers) — see the module comment.
pub const COMPILED_STD_MODULES: &[CompiledStdModule] = &[
    CompiledStdModule {
        dotted: "Std.Palette",
        source: PALETTE,
    },
    CompiledStdModule {
        dotted: "Std.Css",
        source: CSS,
    },
    CompiledStdModule {
        dotted: "Sky.Core.ToString",
        source: TOSTRING_CORE,
    },
    CompiledStdModule {
        dotted: "Sky.Test",
        source: SKY_TEST,
    },
    CompiledStdModule {
        dotted: "Std.Live.Head",
        source: STD_LIVE_HEAD,
    },
    CompiledStdModule {
        dotted: "Std.Ui.Responsive",
        source: STD_UI_RESPONSIVE,
    },
    CompiledStdModule {
        dotted: "Std.Ui.Chart",
        source: STD_UI_CHART,
    },
    CompiledStdModule {
        dotted: "Std.Ui.Grid",
        source: STD_UI_GRID,
    },
    CompiledStdModule {
        dotted: "Std.Ui.Transition",
        source: STD_UI_TRANSITION,
    },
    CompiledStdModule {
        dotted: "Std.Ui.Transform",
        source: STD_UI_TRANSFORM,
    },
    CompiledStdModule {
        dotted: "Std.Ui.Animation",
        source: STD_UI_ANIMATION,
    },
    CompiledStdModule {
        dotted: "Std.Money",
        source: STD_MONEY,
    },
    CompiledStdModule {
        dotted: "Sky.Core.Pure",
        source: SKY_CORE_PURE,
    },
    CompiledStdModule {
        dotted: "Sky.Core.WebSocket",
        source: SKY_CORE_WEBSOCKET,
    },
    CompiledStdModule {
        dotted: "Std.Cache",
        source: STD_CACHE,
    },
    CompiledStdModule {
        dotted: "Std.Compression",
        source: STD_COMPRESSION,
    },
    CompiledStdModule {
        dotted: "Std.Config",
        source: STD_CONFIG,
    },
    CompiledStdModule {
        dotted: "Std.Csv",
        source: STD_CSV,
    },
    CompiledStdModule {
        dotted: "Std.Email",
        source: STD_EMAIL,
    },
    CompiledStdModule {
        dotted: "Std.Live.Console",
        source: STD_LIVE_CONSOLE,
    },
    CompiledStdModule {
        dotted: "Std.PubSub",
        source: STD_PUBSUB,
    },
    CompiledStdModule {
        dotted: "Std.Trace",
        source: STD_TRACE,
    },
    CompiledStdModule {
        dotted: "Std.Ui.Events",
        source: STD_UI_EVENTS,
    },
    // #194: Sky.Core.Regex — Layer-3 source, `Ffi.kernel "Regex_*"` aliases route
    // to the registered pure `Regex*` kernels (`sky_runtime::regex_kernel::*`).
    CompiledStdModule {
        dotted: "Sky.Core.Regex",
        source: REGEX,
    },
    // #202: Sky.Core.Path — Layer-3 source, `Ffi.kernel "Path_*"` aliases route
    // to the registered pure `Path*` kernels (`sky_runtime::path::*`).
    CompiledStdModule {
        dotted: "Sky.Core.Path",
        source: PATH,
    },
];

/// The embedded Sky source for a compiled-source stdlib module named by its path
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
    use sky_intern::Interner;

    /// Every embedded `Sky.Core` module must PARSE with the same front end that
    /// reads user code — the proof that the compiler can read its own embedded
    /// standard library (the foundation the import resolver builds on).
    #[test]
    fn every_embedded_module_parses() {
        for m in MODULES {
            let mut interner = Interner::new();
            let parsed = sky_parse::parse_module(m.source, &mut interner);
            assert!(
                parsed.is_ok(),
                "embedded module {} must parse: {:?}",
                m.name,
                parsed.err()
            );
        }
    }

    /// The `Sky.Core.Prelude` alias resolves to the `Basics` source.
    #[test]
    fn prelude_aliases_basics() {
        assert_eq!(source("Sky.Core.Prelude"), Some(BASICS));
        assert_eq!(source("Sky.Core.Basics"), Some(BASICS));
    }

    /// An unknown `Sky.Core` module is not embedded.
    #[test]
    fn unknown_module_is_absent() {
        assert_eq!(source("Sky.Core.Nope"), None);
    }

    /// Every compiled-source module must PARSE with the real front end — the
    /// PARSE-DON'T-VALIDATE floor: a module cannot enter any build graph until it
    /// is proven to parse with the same parser that reads user code.
    #[test]
    fn every_compiled_source_module_parses() {
        for m in COMPILED_STD_MODULES {
            let mut interner = Interner::new();
            let parsed = sky_parse::parse_module(m.source, &mut interner);
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
            let clash = sky_canon::STDLIB_MODULE_QUALIFIERS
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

    /// Segment lookup resolves a compiled-source module and rejects a non-member.
    #[test]
    fn compiled_source_segment_lookup() {
        let palette = vec!["Std".to_owned(), "Palette".to_owned()];
        assert!(is_compiled_source_segments(&palette));
        assert!(compiled_std_source_segments(&palette).is_some());

        let log = vec!["Std".to_owned(), "Log".to_owned()];
        assert!(!is_compiled_source_segments(&log), "Std.Log is a kernel");

        let nope = vec!["Std".to_owned(), "Nope".to_owned()];
        assert!(!is_compiled_source_segments(&nope));
    }
}
