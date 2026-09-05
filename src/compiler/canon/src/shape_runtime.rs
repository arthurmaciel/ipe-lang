//! Library single-source-of-truth: which standard-library module is admissible
//! in which (shape × runtime) placement.
//!
//! A program is placed on two orthogonal axes (spec § 0): its **shape** — what
//! `view` renders (DOM / cells / lines / http / none), pinned by the head of
//! `main` — and, for the Web shape, its **runtime** — whether the loop is
//! co-located with native effects (`live`, the served/desktop default) or
//! sandboxed in a browser (`spa`). Every other shape has one runtime, so its
//! runtime axis is fixed.
//!
//! One table, [`allowed_in`], classifies the placement-constrained stdlib module
//! families — native effects and browser-host capabilities — by the set of
//! placements each is admissible in. `resolve`, the LSP, and the docs consume
//! this one table, so no placement rule for these families is duplicated per
//! site. A disallowed import is a compile error at resolve time — before any
//! downstream cargo build (THE SEAL) can break, and before a native effect (a
//! DB handle, a secret) could ever be emitted into a sandboxed browser bundle.
//!
//! Scope: this table owns the families with no other gate — native effects and
//! browser-host capabilities. The SHAPE-RENDER surfaces (`Ipe.Tea.*` TEA
//! machinery, `Ipe.Ui.*` / `Ipe.Html` view libraries) are governed by the
//! dedicated shape gates (IPE-N0033 / N0035 / N0045 and the lowering shape
//! gates), which already encode the shape-fold rules (`Tui`/`Cli` share the
//! terminal family, `WebView` folds onto `Web`); this table classifies them as
//! [`ModuleClass::Pure`] rather than re-gate them with a second, fold-unaware
//! rule.
//!
//! Soundness direction (Security > ease of use): the table may over-restrict
//! toward rejection (a false "not allowed here"), but it must never admit a
//! native effect into a sandboxed runtime. Where the placement of a family is
//! ambiguous, the security-conservative (deny) reading is chosen. The
//! runtime-aware wasm link gate (IPE-N0029, a default-deny kernel allowlist) is
//! the defence-in-depth backstop that refuses any unclassified native kernel in
//! a sandboxed bundle even if a new effect module is not yet a row here.

/// A rendering shape, pinned by the head of `main` (spec § 1).
///
/// Mirrors [`crate::shape_source::MainShape`]; kept as its own type so this
/// module reads as a self-contained placement model and does not depend on the
/// classifier's direction of use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `main : Task Error ()` — renders nothing; a native binary.
    Script,
    /// `main = Tui.app …` — full-screen terminal cells.
    Tui,
    /// `main = Cli.app …` — line-oriented terminal output.
    Cli,
    /// `main = Server.listen …` — an HTTP server.
    Server,
    /// `main = Web.app …` — a DOM app, the only shape with a runtime choice.
    Web,
}

impl Shape {
    /// The delivery shape a compiler-classified `main` pins.
    #[must_use]
    pub const fn from_main(shape: crate::shape_source::MainShape) -> Self {
        use crate::shape_source::MainShape;
        match shape {
            MainShape::Script => Self::Script,
            MainShape::Tui => Self::Tui,
            MainShape::Cli => Self::Cli,
            MainShape::Server => Self::Server,
            MainShape::Web => Self::Web,
        }
    }

    /// The canonical CLI/error word for this shape — the one vocabulary shared
    /// by the CLI grammar, diagnostics, config, and docs.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Tui => "tui",
            Self::Cli => "cli",
            Self::Server => "server",
            Self::Web => "web",
        }
    }
}

/// The effect-locality axis of a placement (spec § 0, § 2). Only the Web shape
/// carries a genuine choice; every other shape has exactly one runtime, so its
/// value here is fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    /// The loop sits *at* native effects: served/desktop `live` for Web, and the
    /// single runtime of every non-Web shape (`terminal`, `server`, `binary`).
    /// Native effects (`Ipe.Db`, `Ipe.File`, a `Secret`) are admissible.
    CoLocated,
    /// The sandboxed client loop — wasm in a browser/webview (`web spa`). Effects
    /// reach the host only through Web-platform capabilities plus HTTP to a
    /// backend; a native effect has no denotation here and is denied.
    Spa,
}

/// A fully-placed program: a shape and its runtime. The runtime of every non-Web
/// shape is fixed to [`Runtime::CoLocated`]; only the Web shape admits both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The rendering shape.
    pub shape: Shape,
    /// The effect-locality runtime.
    pub runtime: Runtime,
}

impl Placement {
    /// The single placement every non-Web shape has: its one co-located runtime.
    /// Web has no single placement (it admits both runtimes), so this rejects it
    /// — callers with a Web shape must supply the resolved runtime explicitly.
    #[must_use]
    pub const fn sole_for(shape: Shape) -> Option<Self> {
        match shape {
            Shape::Script | Shape::Tui | Shape::Cli | Shape::Server => Some(Self {
                shape,
                runtime: Runtime::CoLocated,
            }),
            Shape::Web => None,
        }
    }

    /// The canonical placement phrase for a diagnostic — `script`, `terminal`,
    /// `server`, `web live`, or `web spa`. One vocabulary with the CLI grammar.
    #[must_use]
    pub const fn phrase(self) -> &'static str {
        match (self.shape, self.runtime) {
            (Shape::Script, _) => "script",
            (Shape::Tui | Shape::Cli, _) => "terminal",
            (Shape::Server, _) => "server",
            (Shape::Web, Runtime::CoLocated) => "web live",
            (Shape::Web, Runtime::Spa) => "web spa",
        }
    }
}

/// The placement family of a standard-library module (spec § 5).
///
/// The coarse classification the allow-table is keyed on. The classifier
/// ([`classify`]) maps a module dot-path onto one of these; only the
/// placement-constrained families (native effects and browser-host
/// capabilities) are named, and everything else is [`Self::Pure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleClass {
    /// A pure module — no effect, no render (`String`, `List`, `Dict`, `Json`,
    /// `Math`, `Result`, `Task`, …). Also the shape-render surfaces
    /// (`Ipe.Tea.*`, `Ipe.Ui.*`, `Ipe.Html`), which the dedicated shape gates
    /// (IPE-N0033 / N0035 / N0045 and the lowering shape gates) govern with the
    /// shape-fold rules this table deliberately does not duplicate. Admissible in
    /// every placement here.
    Pure,
    /// `Ipe.Browser.*` — Web-platform host capabilities (Geolocation, Camera,
    /// Microphone, Clipboard, …). Needs a JS host: admissible in the Web shape
    /// (live or spa, on any host). Rejected in the live-rendering terminal and
    /// server shapes, which have no browser and never will. The `script` shape
    /// is exempt: it renders nothing and is the build-time harness a decoder
    /// probe or an export check imports these modules from, so a browser module
    /// there is a no-render tool use, not a mis-placed live capability.
    BrowserHost,
    /// `Ipe.Db.*`, `Ipe.File.*`, the server `Ipe.Http.Server`, and `Auth` secret
    /// surfaces — direct native effects. Admissible only in a co-located runtime;
    /// **rejected in `spa`** (the DB/secret-to-browser leak the gate exists to
    /// prevent).
    NativeEffect,
    /// The portable client `Ipe.Http` fetch surface — admissible in any placement
    /// with a browser (Web live/spa) and in every co-located native placement.
    ClientHttp,
}

/// The verdict of the allow-table for one (module family × placement) cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admissibility {
    /// The module is admissible in this placement.
    Allow,
    /// The module is not admissible in this placement, with a reason that names
    /// the module family, the placement, and what to use instead.
    Deny(DenyReason),
}

/// Why a module family is denied in a placement (spec § 6).
///
/// A machine-readable reason the diagnostic layer turns into a kind-teacher
/// message. Each variant carries exactly the facts the message needs; the prose
/// lives with the diagnostic so the message set is itself a single source of
/// truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyReason {
    /// A native effect (`Ipe.Db` / `Ipe.File` / `Ipe.Http.Server` / a secret) was
    /// reached in a sandboxed `spa` runtime. Move it behind an HTTP boundary, or
    /// deliver as `web live` where the loop runs server-side.
    NativeEffectInSandbox,
    /// A `Ipe.Browser.*` host capability was reached in a placement with no JS
    /// host (`terminal` / `server` / `script`).
    BrowserOutsideBrowserHost {
        /// The placement that has no browser host.
        placement: Placement,
    },
}

/// Classify a standard-library module dot-path into its placement family.
///
/// The path is the dotted form (`"Ipe.Db.Store"`, `"Ipe.Browser.Camera"`,
/// `"String"`). Only the placement-constrained families this table owns —
/// native effects (`Ipe.Db` / `Ipe.File` / `Ipe.Http.Server` / `Ipe.Auth`) and
/// browser-host capabilities (`Ipe.Browser.*`) — are recognised; everything
/// else (pure stdlib, and the shape-render surfaces owned by the dedicated
/// shape gates) is [`ModuleClass::Pure`], admissible everywhere by this table.
///
/// A user/dep module path (not `Ipe.`-prefixed) is likewise [`ModuleClass::Pure`]
/// — a user module carries no stdlib placement constraint of its own, and its
/// own imports are gated when that module is itself resolved.
#[must_use]
pub fn classify(path: &str) -> ModuleClass {
    // Non-`Ipe.` paths: the auto-imported pure prelude, or a user/dep module.
    // Neither carries a placement constraint here, so both are treated as pure —
    // a user module's own stdlib imports are gated when it is resolved.
    let Some(rest) = path.strip_prefix("Ipe.") else {
        return ModuleClass::Pure;
    };

    // The first `Ipe.` segment decides the effect/capability family. Matching on
    // the segment head keeps every sub-module of a family (`Ipe.Db`,
    // `Ipe.Db.Store`, `Ipe.Db.Sql`) on the same row.
    //
    // Division of labour: the SHAPE-RENDER surfaces — the `Ipe.Tea.*` TEA
    // app/Cmd/Sub machinery and the shape view libraries (`Ipe.Ui.*` cells,
    // `Ipe.Html`) — are governed by the dedicated shape gates that already know
    // the shape-fold rules (`Tui`/`Cli` share the terminal family, `WebView`
    // folds onto `Web`): the resolver's Program/TEA gate (IPE-N0033), the
    // cross-shape `Cmd`/`Sub` gate (IPE-N0035), the runtime-branched-`main` gate
    // (IPE-N0045), and the lowering shape gates for the raw view leaves
    // (IPE-L0132 / IPE-L0153 / IPE-L0147). This table therefore classifies those
    // as [`ModuleClass::Pure`] here to avoid double-gating them with a second,
    // fold-unaware rule. What this table uniquely owns is the placement families
    // with no other gate: native effects and browser-host capabilities.
    let head = rest.split('.').next().unwrap_or(rest);
    match head {
        // Native effects — direct DB, file, server-http, and the secret surface.
        "Db" | "File" | "Auth" => ModuleClass::NativeEffect,
        // `Ipe.Http.Server` is a native effect (it binds a socket and serves);
        // the plain client `Ipe.Http` fetch surface is portable.
        "Http" => {
            if rest.starts_with("Http.Server") {
                ModuleClass::NativeEffect
            } else {
                ModuleClass::ClientHttp
            }
        }
        // Web-platform host capabilities — need a JS host.
        "Browser" => ModuleClass::BrowserHost,
        // Every other `Ipe.*` module — pure stdlib, or a shape-render surface
        // owned by the dedicated shape gates above. A NEW restricted-EFFECT
        // module MUST be added as its own head here rather than left to fall
        // through as pure; the runtime-aware wasm link gate (IPE-N0029, a
        // default-deny kernel allowlist) is the defence-in-depth backstop that
        // still refuses an unclassified native kernel in a sandboxed bundle.
        _ => ModuleClass::Pure,
    }
}

/// The single source of truth: is a module of family `class` admissible in
/// `placement`?
///
/// Total and exhaustive over [`ModuleClass`] — there is **no wildcard fallthrough
/// that could silently admit a newly-added family**. Every family names its own
/// arm; adding a [`ModuleClass`] variant forces a decision here.
// `match_same_arms`: `Pure` and `ClientHttp` both resolve to `Allow`, but they
// are distinct families with distinct rationales (a pure module is admissible
// because it has no effect; client HTTP because an outbound request is available
// in every placement). Keeping the arms separate documents that decision and
// forces a fresh judgement if either family's admissibility ever narrows.
#[allow(clippy::match_same_arms)]
#[must_use]
pub const fn allowed_in(class: ModuleClass, placement: Placement) -> Admissibility {
    use Admissibility::{Allow, Deny};
    match class {
        // Pure modules (and shape-render surfaces owned by the dedicated shape
        // gates): admissible everywhere here.
        ModuleClass::Pure => Allow,

        // Browser host capabilities: the Web shape (live served/desktop or spa
        // browser/ios/android/desktop) has a JS host. The live-rendering terminal
        // and server shapes never do, so they are rejected. `script` renders
        // nothing and is the build-time harness (decoder probes, export checks)
        // these modules are imported from, so it is exempt — a no-render tool use,
        // not a mis-placed live capability.
        ModuleClass::BrowserHost => match placement.shape {
            Shape::Web | Shape::Script => Allow,
            Shape::Tui | Shape::Cli | Shape::Server => {
                Deny(DenyReason::BrowserOutsideBrowserHost { placement })
            }
        },

        // Native effects: co-located only. The security invariant — a native
        // effect (DB handle, secret) must never be emitted into a sandboxed
        // browser bundle. Denied in `spa`; admissible in every co-located
        // runtime (live, terminal, server, script).
        ModuleClass::NativeEffect => match placement.runtime {
            Runtime::CoLocated => Allow,
            Runtime::Spa => Deny(DenyReason::NativeEffectInSandbox),
        },

        // Portable client HTTP fetch: every browser placement and every
        // co-located native placement. Admissible everywhere the program can make
        // an outbound request, which is every placement in the model.
        ModuleClass::ClientHttp => Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn co(shape: Shape) -> Placement {
        Placement {
            shape,
            runtime: Runtime::CoLocated,
        }
    }

    fn web(runtime: Runtime) -> Placement {
        Placement {
            shape: Shape::Web,
            runtime,
        }
    }

    #[test]
    fn pure_modules_are_admissible_everywhere() {
        for path in [
            "String", "List", "Dict", "Json", "Math", "Ipe.Json", "Ipe.Task",
        ] {
            assert_eq!(classify(path), ModuleClass::Pure, "{path}");
        }
        for p in [
            co(Shape::Script),
            co(Shape::Tui),
            co(Shape::Cli),
            co(Shape::Server),
            web(Runtime::CoLocated),
            web(Runtime::Spa),
        ] {
            assert_eq!(allowed_in(ModuleClass::Pure, p), Admissibility::Allow);
        }
    }

    /// The shape-render surfaces (`Ipe.Tea.*`, `Ipe.Ui.*`, `Ipe.Html`) defer to
    /// the dedicated shape gates, so this table classifies them as `Pure` — it
    /// does not re-gate them with a second, fold-unaware rule.
    #[test]
    fn shape_render_surfaces_defer_to_the_shape_gates_as_pure() {
        for path in [
            "Ipe.Tea.Web",
            "Ipe.Tea.Web.Cmd",
            "Ipe.Tea.Tui",
            "Ipe.Tea.Cli",
            "Ipe.Tea.Cli.Ui",
            "Ipe.Tea.Terminal.Cmd",
            "Ipe.Tea.Terminal.Color",
            "Ipe.Tea.WebView",
            "Ipe.Ui",
            "Ipe.Ui.Cells",
            "Ipe.Html",
        ] {
            assert_eq!(classify(path), ModuleClass::Pure, "{path}");
        }
    }

    #[test]
    fn native_effect_denied_in_spa_allowed_co_located() {
        for path in [
            "Ipe.Db",
            "Ipe.Db.Store",
            "Ipe.File",
            "Ipe.Http.Server",
            "Ipe.Auth",
        ] {
            assert_eq!(classify(path), ModuleClass::NativeEffect, "{path}");
        }
        // Denied in the sandboxed spa runtime — the DB/secret-to-browser leak.
        assert_eq!(
            allowed_in(ModuleClass::NativeEffect, web(Runtime::Spa)),
            Admissibility::Deny(DenyReason::NativeEffectInSandbox)
        );
        // Admissible in every co-located placement.
        for p in [
            co(Shape::Script),
            co(Shape::Tui),
            co(Shape::Cli),
            co(Shape::Server),
            web(Runtime::CoLocated),
        ] {
            assert_eq!(
                allowed_in(ModuleClass::NativeEffect, p),
                Admissibility::Allow
            );
        }
    }

    #[test]
    fn client_http_is_portable_but_server_http_is_native() {
        assert_eq!(classify("Ipe.Http"), ModuleClass::ClientHttp);
        assert_eq!(classify("Ipe.Http.Server"), ModuleClass::NativeEffect);
        // Client fetch is admissible even in the sandbox.
        assert_eq!(
            allowed_in(ModuleClass::ClientHttp, web(Runtime::Spa)),
            Admissibility::Allow
        );
    }

    #[test]
    fn browser_host_needs_a_browser() {
        assert_eq!(
            classify("Ipe.Browser.Geolocation"),
            ModuleClass::BrowserHost
        );
        // Any Web placement has a browser; `script` is the exempt no-render
        // build-time harness.
        for r in [Runtime::CoLocated, Runtime::Spa] {
            assert_eq!(
                allowed_in(ModuleClass::BrowserHost, web(r)),
                Admissibility::Allow
            );
        }
        assert_eq!(
            allowed_in(ModuleClass::BrowserHost, co(Shape::Script)),
            Admissibility::Allow
        );
        // No browser in the live-rendering terminal and server shapes.
        for shape in [Shape::Tui, Shape::Cli, Shape::Server] {
            assert_eq!(
                allowed_in(ModuleClass::BrowserHost, co(shape)),
                Admissibility::Deny(DenyReason::BrowserOutsideBrowserHost {
                    placement: co(shape)
                })
            );
        }
    }

    #[test]
    fn sole_placement_is_none_for_web_some_otherwise() {
        assert_eq!(Placement::sole_for(Shape::Web), None);
        for shape in [Shape::Script, Shape::Tui, Shape::Cli, Shape::Server] {
            assert_eq!(
                Placement::sole_for(shape),
                Some(Placement {
                    shape,
                    runtime: Runtime::CoLocated
                })
            );
        }
    }
}
