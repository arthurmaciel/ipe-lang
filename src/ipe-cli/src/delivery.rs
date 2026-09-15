//! The delivery grammar — one typed model of `ipe (build|release|watch) [shape]
//! [runtime] [host] [target] [--static]` and the single source of truth for
//! which combinations are valid.
//!
//! Two orthogonal axes place every program (spec § 0):
//!
//! * **shape** — what `view` renders (DOM / cells / lines / http / none). Pinned
//!   by the head of `main`, never redeclared. The optional leading `[shape]`
//!   positional is a *validated cross-check* against `main`, not a second source
//!   of truth.
//! * **runtime × host** — for the `web` shape only, whether the loop is
//!   co-located (`live`, the unnamed default) or sandboxed (`spa`), and which
//!   host carries it.
//!
//! Every invalid combination is a [`DeliveryError`] — a kind-teacher diagnostic
//! that names the problem, explains the two-axis picture in a sentence, and
//! gives the fix. The message set lives here so it is itself a single source of
//! truth (spec § 6).
//!
//! Invalid states are unrepresentable: a resolved [`Delivery`] can only be built
//! by [`Delivery::resolve`], which admits nothing the validity table rejects.

use core::fmt;

/// A rendering class, pinned by the head of `main` (spec § 1). The leading CLI
/// positional, when present, must name the same shape `main` selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `main : Task Error ()` — renders nothing; a native binary.
    Script,
    /// `main = Tui.tea …` — full-screen terminal cells.
    Tui,
    /// `main = Cli.tea …` — line-oriented terminal output.
    Cli,
    /// `main = Server.listen …` — an HTTP server.
    Server,
    /// `main = Web.tea …` — a DOM app, the only shape with a runtime choice.
    Web,
}

impl Shape {
    /// The canonical CLI word for this shape — the one vocabulary shared by CLI,
    /// errors, config, and docs.
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

    /// Parse a shape word. `None` for any token outside the closed set (so a
    /// leading positional that is not a shape word is read as an entry path, not
    /// a mistyped shape).
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "script" => Self::Script,
            "tui" => Self::Tui,
            "cli" => Self::Cli,
            "server" => Self::Server,
            "web" => Self::Web,
            _ => return None,
        })
    }

    /// The rendering family a shape's TEA `Cmd`/`Sub` imports fold onto — `Tui`
    /// and `Cli` share the `Terminal` family. Mirrors the compiler classifier so
    /// the CLI cross-check and the compiler agree on one vocabulary.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Tui | Self::Cli)
    }

    /// The control model this shape's `main` runs under — the compiler-derived
    /// answer to "how does this program drive itself". It is a projection of the
    /// same shape the compiler already pinned, never a second derivation: a
    /// view-ful shape (`Web`/`Tui`/`Cli`) runs the Elm-style
    /// model/update/view loop ([`ControlModel::Tea`]); a `Server` runs the
    /// declarative request/response model; a `Script` (a plain `Task Error ()`)
    /// runs directly to completion ([`ControlModel::Direct`]).
    #[must_use]
    pub const fn control_model(self) -> ControlModel {
        match self {
            Self::Web | Self::Tui | Self::Cli => ControlModel::Tea,
            Self::Server => ControlModel::Server,
            Self::Script => ControlModel::Direct,
        }
    }

    /// The delivery shape a compiler-classified `main` pins. The compiler is the
    /// single source of truth for the shape (spec § 0); this maps its
    /// [`ipe_canon::shape_source::MainShape`] onto the delivery vocabulary so the
    /// grammar cross-check and the packager routing speak the same words.
    #[must_use]
    pub const fn from_main(shape: ipe_canon::shape_source::MainShape) -> Self {
        use ipe_canon::shape_source::MainShape;
        match shape {
            MainShape::Script => Self::Script,
            MainShape::Tui => Self::Tui,
            MainShape::Cli => Self::Cli,
            MainShape::Server => Self::Server,
            MainShape::Web => Self::Web,
        }
    }
}

/// How a program drives itself, projected from its compiler-pinned [`Shape`].
///
/// A closed set: every shape maps to exactly one control model, so a disclosure
/// surface can name the model without a second derivation that could disagree
/// with the shape the compiler already pinned. This is the SSOT the `ipe audit`
/// disclosure reads — it never re-inspects `main` on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ControlModel {
    /// The Elm-style model/update/view loop — a `Web`/`Tui`/`Cli` shape.
    Tea,
    /// The declarative request/response model — a `Server` shape.
    Server,
    /// A plain `main : Task Error ()` that runs directly to completion — a
    /// `Script` shape.
    Direct,
}

impl ControlModel {
    /// The canonical word for this control model — the one vocabulary shared by
    /// the audit disclosure, its JSON verdict, the consent gate, and docs.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Tea => "tea",
            Self::Server => "server",
            Self::Direct => "direct",
        }
    }

    /// Parse a control-model word (the inverse of [`Self::word`]). `None` for any
    /// token outside the closed set — a consumer's `acceptsControl` entry that is
    /// not a known model must be rejected, never read as a permissive default.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "tea" => Self::Tea,
            "server" => Self::Server,
            "direct" => Self::Direct,
            _ => return None,
        })
    }

    /// Whether this control model is a *managed* one — the runtime drives the
    /// loop and every effect flows through a capability axis already gated. The
    /// managed models (`Tea`/`Server`) are the safe, implicitly-admitted default;
    /// only the elevated [`Self::Direct`] model (a self-driving `Task Error ()`
    /// program outside the managed loop) requires a consumer's explicit consent.
    #[must_use]
    pub const fn is_managed(self) -> bool {
        matches!(self, Self::Tea | Self::Server)
    }
}

/// The Web-shape runtime (spec § 2). Only `web` has a runtime choice; every
/// other shape has exactly one, so this axis is absent for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    /// The co-located server loop — LiveView-style diff/patch to a thin client,
    /// direct native effects. The **unnamed default**: it is never written on
    /// the CLI. `web` alone means live; typing `live` is a [`DeliveryError`].
    Live,
    /// The sandboxed client loop — wasm in a webview/browser, effects only via
    /// Web-API capabilities plus HTTP to a backend. The only web runtime word.
    Spa,
}

/// A delivery host — where a resolved shape × runtime actually runs (spec § 2,
/// § 4). Not every host is valid for every runtime; the validity table decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Host {
    /// The implicit host: `web` (served, over SSE) or `web spa` (the browser).
    /// Never written — it is what an absent host token means.
    #[default]
    Default,
    /// `desktop`. Under `live` it is **webview-native** (the diff/patch pipeline
    /// over a local IPC bridge); under `spa` it is **webview-wasm** (the browser
    /// SPA wrapped in a `wry` shell).
    Desktop,
    /// `ios` — a wasm SPA in `WKWebView` plus a native shell. `spa` only.
    Ios,
    /// `android` — a wasm SPA in an Android `WebView` plus a native shell. `spa`
    /// only.
    Android,
}

impl Host {
    /// The canonical CLI word, or `None` for the implicit default host (which is
    /// never written).
    #[must_use]
    pub const fn word(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Desktop => Some("desktop"),
            Self::Ios => Some("ios"),
            Self::Android => Some("android"),
        }
    }

    /// Parse a host word. `None` for any token outside the closed host set.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "desktop" => Self::Desktop,
            "ios" => Self::Ios,
            "android" => Self::Android,
            _ => return None,
        })
    }
}

/// The compile engine a program runs under.
///
/// The delivery-layer name for the backend [`ipe_ir::Target`], carried here so
/// the validity matrix can reason over `(engine, delivery, triple)` in one place
/// without importing the whole kernel crate. A projection of the same enum,
/// never a second derivation.
///
/// A closed set kept in lock-step with [`ipe_ir::Target`] by [`Self::of`]: the
/// instant a variant is added there, this `match` stops compiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// The native host binary — full-kernel effects, `available_on == true`.
    Native,
    /// The sandboxed browser WASM client (`web spa`) — the default-deny
    /// allowlist. Effects only via Web-API substitutes; native effects denied.
    WasmClient,
}

impl Engine {
    /// Project a backend [`ipe_ir::Target`] onto the delivery-layer engine. The
    /// backend enum is the single source of truth; this maps it, so a new
    /// backend target forces a new arm here at compile time.
    #[must_use]
    pub const fn of(target: ipe_ir::Target) -> Self {
        match target {
            ipe_ir::Target::Native => Self::Native,
            ipe_ir::Target::WasmClient => Self::WasmClient,
        }
    }
}

/// A representable target triple — the closed set the delivery layer can name.
///
/// Parsed from a raw `--target`/positional string ONCE at the boundary (parse,
/// don't validate) so no unverifiable triple reaches the validity matrix or,
/// past it, cargo.
///
/// The set is deliberately wider than what the CLI *accepts* today: it can name
/// `wasm32-wasip1` so the matrix can state — and a test can pin — the
/// fail-closed refusals around co-located WASI before that accept-path exists.
/// A member being representable is not a licence to build it; the matrix
/// ([`admit_triple`]) and the CLI accept-set ([`crate::cli_args::ReleaseTarget`])
/// are the gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetTriple {
    /// The host's own triple — the implicit default (no `--target`).
    Host,
    /// `x86_64-unknown-linux-musl` — the static-musl x86-64 triple.
    X8664LinuxMusl,
    /// `aarch64-unknown-linux-musl` — the static-musl aarch64 triple.
    Aarch64LinuxMusl,
    /// `wasm32-unknown-unknown` — the browser sandbox triple (`web spa`).
    BrowserWasm,
    /// `wasm32-wasip1` — the co-located portable WASI triple. Representable so
    /// the matrix can refuse it fail-closed; its accept-path is gated on the
    /// WASI runtime port.
    Wasm32Wasip1,
}

impl TargetTriple {
    /// The rustc triple spelling. `Host` has no fixed spelling (it is the
    /// ambient host), so it renders as `host`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::X8664LinuxMusl => "x86_64-unknown-linux-musl",
            Self::Aarch64LinuxMusl => "aarch64-unknown-linux-musl",
            Self::BrowserWasm => "wasm32-unknown-unknown",
            Self::Wasm32Wasip1 => "wasm32-wasip1",
        }
    }

    /// Parse a rustc triple string into the representable set. `None` for any
    /// token outside the closed set — the caller turns that into a typed
    /// refusal. The empty string and an unsupported triple are both `None`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "x86_64-unknown-linux-musl" => Self::X8664LinuxMusl,
            "aarch64-unknown-linux-musl" => Self::Aarch64LinuxMusl,
            "wasm32-unknown-unknown" => Self::BrowserWasm,
            "wasm32-wasip1" => Self::Wasm32Wasip1,
            _ => return None,
        })
    }

    /// Whether this triple names a WebAssembly target (either wasm flavour).
    #[must_use]
    pub const fn is_wasm(self) -> bool {
        matches!(self, Self::BrowserWasm | Self::Wasm32Wasip1)
    }
}

/// A fully-resolved delivery target — a shape, its (web-only) runtime, and a
/// valid host.
///
/// Constructible only through [`Delivery::resolve`], so no invalid combination
/// can be built. The runtime is `None` for every non-web shape (the axis does
/// not exist for them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivery {
    shape: Shape,
    runtime: Option<Runtime>,
    host: Host,
}

impl Delivery {
    /// The resolved shape.
    #[must_use]
    pub const fn shape(self) -> Shape {
        self.shape
    }

    /// The resolved web runtime, or `None` for a non-web shape.
    #[must_use]
    pub const fn runtime(self) -> Option<Runtime> {
        self.runtime
    }

    /// The resolved host.
    #[must_use]
    pub const fn host(self) -> Host {
        self.host
    }

    /// `true` when this delivery links the system webview at runtime — the
    /// `web desktop` (webview-native) target. This is the single source of the
    /// backend `uses_webview` signal: the webview executor and the
    /// `default = ["webview"]` feature are emitted for exactly this delivery, and
    /// a served `web` (or any other shape) emits neither.
    #[must_use]
    pub const fn is_webview_native(self) -> bool {
        matches!(
            self,
            Self {
                shape: Shape::Web,
                runtime: Some(Runtime::Live),
                host: Host::Desktop,
            }
        )
    }

    /// `true` when a static (musl) artifact is admissible for this delivery.
    /// Only the co-located, no-webview shapes qualify: a webview host links the
    /// system webview at runtime, and a `spa`/mobile host is a wasm/bundle
    /// target where a musl triple is moot.
    #[must_use]
    pub const fn allows_static(self) -> bool {
        match self.shape {
            Shape::Script | Shape::Tui | Shape::Cli | Shape::Server => true,
            Shape::Web => matches!(
                self,
                Self {
                    runtime: Some(Runtime::Live),
                    host: Host::Default,
                    ..
                }
            ),
        }
    }

    /// Resolve a `main`-pinned shape and the parsed runtime/host tokens into a
    /// valid [`Delivery`], applying the defaults (`web` → live, every host →
    /// its implicit default) and rejecting every invalid combination with a
    /// pedagogical [`DeliveryError`].
    ///
    /// `runtime`/`host` apply to `web` only; a runtime or non-default host on a
    /// non-web shape is refused. For `web`, an absent runtime means live.
    ///
    /// # Errors
    /// [`DeliveryError`] naming the exact invalid combination and its fix.
    pub fn resolve(
        shape: Shape,
        runtime: Option<Runtime>,
        host: Host,
    ) -> Result<Self, DeliveryError> {
        if !matches!(shape, Shape::Web) {
            if runtime.is_some() {
                return Err(DeliveryError::RuntimeOnNonWeb { shape });
            }
            if host != Host::Default {
                return Err(DeliveryError::HostOnNonWeb { shape, host });
            }
            return Ok(Self {
                shape,
                runtime: None,
                host: Host::Default,
            });
        }

        // Web: absent runtime is the unnamed live default.
        let runtime = runtime.unwrap_or(Runtime::Live);
        match runtime {
            Runtime::Live => match host {
                // Served live (implicit) or webview-native desktop.
                Host::Default | Host::Desktop => {}
                Host::Ios | Host::Android => {
                    return Err(DeliveryError::LiveHostNotMobile { host });
                }
            },
            Runtime::Spa => {} // every host is valid for spa.
        }
        Ok(Self {
            shape,
            runtime: Some(runtime),
            host,
        })
    }

    /// Resolve a delivery from the compiler-pinned shape and the parsed CLI
    /// tail, applying the `[shape]` cross-check and the `--static` gate.
    ///
    /// `pinned` is the shape the compiler derived from `main` (the single source
    /// of truth). `stated` is the optional leading `[shape]` positional: when
    /// present it must name the same shape as `pinned`, else the
    /// [`DeliveryError::ShapeMismatch`] lesson. `tokens` carries the parsed
    /// `[runtime] [host]`; `wants_static` is the `--static` request, refused for
    /// a delivery that has no static musl form.
    ///
    /// # Errors
    /// [`DeliveryError`] for a shape mismatch, an invalid runtime/host
    /// combination, or a `--static` request the delivery cannot honour.
    pub fn resolve_checked(
        pinned: Shape,
        stated: Option<Shape>,
        tokens: &DeliveryTokens,
        wants_static: bool,
    ) -> Result<Self, DeliveryError> {
        if let Some(stated) = stated
            && stated != pinned
        {
            return Err(DeliveryError::ShapeMismatch { stated, pinned });
        }
        let delivery = Self::resolve(pinned, tokens.runtime, tokens.host)?;
        if wants_static && !delivery.allows_static() {
            return Err(DeliveryError::StaticNotAllowed { delivery });
        }
        Ok(delivery)
    }

    /// Enforce the biconditional that couples the delivery runtime to the compile
    /// target: a `spa` delivery compiles to wasm, and a wasm target carries only a
    /// `spa` delivery — `runtime() == Some(Runtime::Spa)` IFF `wasm_target`.
    ///
    /// The runtime and the target are derived from independent sources (the
    /// delivery grammar vs the `--target`/`IPE_TARGET`/`[wasm].mode` chain); this
    /// is the single point that refuses their disagreement. It is load-bearing for
    /// security: the native-deny backstops that keep native effects out of a
    /// sandboxed client are keyed to the wasm target, so a `spa` delivery that
    /// slipped through as a native build would ship those effects into the
    /// sandbox. Absent proof the two agree, the build is refused.
    ///
    /// # Errors
    /// [`DeliveryError::SpaRequiresWasmTarget`] for a `spa` delivery with a native
    /// target; [`DeliveryError::WasmTargetRequiresSpa`] for a wasm target without a
    /// `spa` delivery.
    pub const fn reconcile_wasm_target(self, wasm_target: bool) -> Result<(), DeliveryError> {
        let is_spa = matches!(self.runtime, Some(Runtime::Spa));
        match (is_spa, wasm_target) {
            (true, false) => Err(DeliveryError::SpaRequiresWasmTarget),
            (false, true) => Err(DeliveryError::WasmTargetRequiresSpa),
            (true, true) | (false, false) => Ok(()),
        }
    }

    /// The `(engine, delivery, triple)` validity matrix — a typed total function
    /// that admits exactly the legal combinations and refuses every other with a
    /// pedagogical [`DeliveryError`]. It is the structural successor to
    /// [`Self::reconcile_wasm_target`]: it subsumes the `spa` IFF wasm
    /// biconditional and adds the third axis (the triple), so the two wasm
    /// flavours — the sandboxed browser client (`wasm32-unknown-unknown`) and the
    /// co-located portable WASI target (`wasm32-wasip1`) — are kept cleanly
    /// separate at one place.
    ///
    /// Fail-closed by construction: the `match` is exhaustive and has NO
    /// permissive catch-all — a tuple not enumerated legal hits a refusal arm.
    /// A new [`Engine`], [`Host`], or [`TargetTriple`] member forces a new arm at
    /// compile time, so a forgotten combination cannot ship as an open default.
    /// Co-located WASI (`wasm32-wasip1`) has no accept-path yet — its runtime
    /// port has not landed — so every WASI triple is refused here; opening it
    /// before the runtime exists would break THE SEAL (an effectful co-located
    /// program would `ipe`-accept then fail `cargo build --target wasm32-wasip1`).
    ///
    /// # Errors
    /// [`DeliveryError`] naming the exact illegal `(engine, delivery, triple)`
    /// cell and its fix.
    pub const fn admit_triple(
        self,
        engine: Engine,
        triple: TargetTriple,
    ) -> Result<(), DeliveryError> {
        let is_spa = matches!(self.runtime, Some(Runtime::Spa));
        match engine {
            // The native host binary: a WASM triple has no native form, and a
            // `spa` delivery must not resolve to a native engine (the wasm-keyed
            // sandbox backstops would be skipped). Otherwise the static-triple
            // gate decides which co-located triples are admissible.
            Engine::Native => {
                if is_spa {
                    return Err(DeliveryError::SpaRequiresWasmTarget);
                }
                match triple {
                    TargetTriple::BrowserWasm | TargetTriple::Wasm32Wasip1 => {
                        Err(DeliveryError::NativeEngineRefusesWasmTriple { triple })
                    }
                    TargetTriple::Host => Ok(()),
                    TargetTriple::X8664LinuxMusl | TargetTriple::Aarch64LinuxMusl => {
                        // A musl static triple mirrors the `--static` gate: a
                        // webview-native (`web desktop`) delivery links the
                        // system webview and has no static form.
                        if self.allows_static() {
                            Ok(())
                        } else {
                            Err(DeliveryError::WebviewHasNoStaticTriple { delivery: self })
                        }
                    }
                }
            }
            // The sandboxed browser client: exactly `web spa` on the browser
            // triple, and nothing else. It NEVER widens to the WASI triple —
            // that would be a sandbox escape hatch.
            Engine::WasmClient => {
                if !is_spa {
                    return Err(DeliveryError::WasmTargetRequiresSpa);
                }
                match triple {
                    TargetTriple::BrowserWasm => Ok(()),
                    TargetTriple::Wasm32Wasip1 => Err(DeliveryError::SpaRefusesWasiTriple),
                    TargetTriple::Host
                    | TargetTriple::X8664LinuxMusl
                    | TargetTriple::Aarch64LinuxMusl => {
                        Err(DeliveryError::SpaRequiresBrowserTriple { triple })
                    }
                }
            }
        }
    }
}

impl fmt::Display for Delivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.shape.word())?;
        if self.runtime == Some(Runtime::Spa) {
            f.write_str(" spa")?;
        }
        if let Some(word) = self.host.word() {
            write!(f, " {word}")?;
        }
        Ok(())
    }
}

/// A pedagogical delivery refusal (spec § 6).
///
/// Each variant names the problem, explains the two-axis big picture in a
/// sentence, and suggests the fix. This enum is the message-set single source of
/// truth — every delivery diagnostic is one of these, phrased once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryError {
    /// The leading `[shape]` positional names a different shape than `main`.
    /// The shape is pinned by `main`; the positional is only a cross-check.
    ShapeMismatch {
        /// The shape the positional named.
        stated: Shape,
        /// The shape `main`'s entry actually selects.
        pinned: Shape,
    },
    /// The literal token `live` was written. `live` is the unnamed default — it
    /// is never spelled out.
    LiveNotAWord,
    /// A runtime word (`spa`) was given for a non-web shape, which has no runtime
    /// axis.
    RuntimeOnNonWeb {
        /// The non-web shape that was given a runtime word.
        shape: Shape,
    },
    /// A host word was given for a non-web shape, which has no host axis.
    HostOnNonWeb {
        /// The non-web shape that was given a host word.
        shape: Shape,
        /// The host word given.
        host: Host,
    },
    /// A mobile host (`ios`/`android`) was given for the live runtime. Mobile is
    /// a sandboxed `spa` target only.
    LiveHostNotMobile {
        /// The mobile host that live does not carry.
        host: Host,
    },
    /// `--static` was requested for a delivery that cannot be a static musl
    /// binary (a webview host, or a `spa`/mobile wasm/bundle target).
    StaticNotAllowed {
        /// The delivery that has no static form.
        delivery: Delivery,
    },
    /// An unknown token appeared where a runtime, host, or target was expected.
    UnknownToken {
        /// The offending token.
        got: String,
    },
    /// A `spa` delivery resolved to a native compile target. A sandboxed client
    /// must compile to wasm — the native-deny backstops that keep native effects
    /// out of the sandbox are keyed to the wasm target, so a native `spa` build
    /// would ship those effects into a sandboxed client.
    SpaRequiresWasmTarget,
    /// A wasm compile target resolved without a `spa` delivery. The wasm client
    /// target exists only to carry a sandboxed `spa` app; a non-`spa` shape has
    /// no wasm form, so the two were derived from disagreeing sources.
    WasmTargetRequiresSpa,
    /// A WASM triple was requested for the native engine. The native binary has
    /// no WebAssembly form; the browser client compiles to
    /// `wasm32-unknown-unknown`, and the co-located WASI target to
    /// `wasm32-wasip1` — neither is a native build.
    NativeEngineRefusesWasmTriple {
        /// The WASM triple asked for on the native engine.
        triple: TargetTriple,
    },
    /// A `web spa` client asked for a triple other than the browser sandbox
    /// triple. The sandboxed client compiles only to `wasm32-unknown-unknown`.
    SpaRequiresBrowserTriple {
        /// The non-browser triple asked for on a `spa` delivery.
        triple: TargetTriple,
    },
    /// A `web spa` client asked for the `wasm32-wasip1` (WASI) triple. The
    /// browser sandbox never widens to WASI: WASI is the co-located portable
    /// target with native-ish effects, the exact opposite of the browser
    /// sandbox's default-deny surface. Allowing it would be a sandbox escape.
    SpaRefusesWasiTriple,
    /// A musl static triple was requested for a delivery that links the system
    /// webview at runtime (`web desktop`), which has no static binary. Mirrors
    /// the `--static` × webview refusal on the triple axis.
    WebviewHasNoStaticTriple {
        /// The webview-native delivery that has no static triple.
        delivery: Delivery,
    },
}

/// The runtime/host/target tokens parsed out of a delivery positional tail,
/// before validity resolution. `target` is a raw Rust triple kept for the
/// packager/static layer; runtime/host are the typed axes.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DeliveryTokens {
    /// The parsed web runtime (`Some(Spa)` if `spa` was written; `None` = live
    /// default). Only meaningful for the web shape.
    pub runtime: Option<Runtime>,
    /// The parsed host, defaulting to the implicit host.
    pub host: Host,
    /// A raw target triple token, if one was given as a positional.
    pub target: Option<String>,
}

impl DeliveryTokens {
    /// Parse the delivery tail — the positional tokens that follow an optional
    /// `[shape]` — into typed axes. Order is `[runtime] [host] [target]`; each is
    /// optional. `spa` is the only runtime word (`live` is refused as a word);
    /// `desktop`/`ios`/`android` are hosts; anything else is taken as a target
    /// triple (a second unknown non-triple token is [`DeliveryError::UnknownToken`]).
    ///
    /// # Errors
    /// [`DeliveryError::LiveNotAWord`] if `live` is written;
    /// [`DeliveryError::UnknownToken`] for a token that is neither `spa`, a host,
    /// nor a plausible target where a target has already been taken.
    pub fn parse(tokens: &[String]) -> Result<Self, DeliveryError> {
        let mut out = Self::default();
        for tok in tokens {
            if tok == "live" {
                return Err(DeliveryError::LiveNotAWord);
            }
            if tok == "spa" {
                out.runtime = Some(Runtime::Spa);
                continue;
            }
            if let Some(host) = Host::from_word(tok) {
                out.host = host;
                continue;
            }
            if out.target.is_none() && looks_like_target(tok) {
                out.target = Some(tok.clone());
                continue;
            }
            return Err(DeliveryError::UnknownToken { got: tok.clone() });
        }
        Ok(out)
    }
}

/// A Rust target triple is a hyphenated identifier (`x86_64-unknown-linux-musl`,
/// `wasm32-unknown-unknown`, `aarch64-apple-ios`). This is the coarse shape test
/// that separates a target positional from a mistyped runtime/host word; the
/// static/packager layers validate the exact triple against their curated sets.
fn looks_like_target(tok: &str) -> bool {
    tok.contains('-')
        && tok
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl fmt::Display for DeliveryError {
    #[allow(clippy::too_many_lines)] // one arm per pedagogical message; each is a whole lesson.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShapeMismatch { stated, pinned } => write!(
                f,
                "you asked for `{}`, but `main` is a `{}` app. A program's shape is \
                 fixed by the head of `main` (what `view` renders) — the CLI word only \
                 double-checks it. Drop the `{}` word, or change `main` to a `{}` entry.",
                stated.word(),
                pinned.word(),
                stated.word(),
                stated.word(),
            ),
            Self::LiveNotAWord => write!(
                f,
                "`live` is the default runtime, so it is never written. The web shape \
                 runs live (a co-located server loop) unless you opt into `spa` (a \
                 sandboxed client). Write `web` for served-live, or `web desktop` for \
                 live on the desktop.",
            ),
            Self::RuntimeOnNonWeb { shape } => write!(
                f,
                "`spa` is a web runtime, but this is a `{}` app. Only the `web` shape \
                 has a runtime choice (live vs spa) — every other shape runs one way. \
                 Drop the runtime word.",
                shape.word(),
            ),
            Self::HostOnNonWeb { shape, host } => write!(
                f,
                "`{}` is a web host, but this is a `{}` app. Hosts (desktop/ios/android) \
                 belong to the `web` shape's delivery axis; a `{}` app has one host. \
                 Drop the host word.",
                host.word().unwrap_or("default"),
                shape.word(),
                shape.word(),
            ),
            Self::LiveHostNotMobile { host } => write!(
                f,
                "`{host}` is a `spa` host, not a live host. Mobile ships a sandboxed \
                 client (`web spa {host}`); live is the co-located server loop (served \
                 or `web desktop`). Write `web spa {host}` for mobile.",
                host = host.word().unwrap_or("default"),
            ),
            Self::StaticNotAllowed { delivery } => match delivery.host() {
                Host::Desktop if delivery.runtime() == Some(Runtime::Live) => write!(
                    f,
                    "`web desktop` links the system webview at runtime, so it has no \
                     static binary. Use `web` (served-live), `tui`, `cli`, or `server` \
                     for a static musl binary, or ship the desktop app bundle.",
                ),
                _ => write!(
                    f,
                    "`{delivery}` targets wasm or a native bundle, so `--static` (a musl \
                     binary) does not apply. `--static` is for the co-located, \
                     no-webview shapes: `script`, `tui`, `cli`, `server`, or served `web`.",
                ),
            },
            Self::UnknownToken { got } => write!(
                f,
                "`{got}` is not a runtime, host, or target. The web runtime word is \
                 `spa` (live is the default). Hosts are `desktop`, `ios`, `android`. \
                 Targets are a Rust triple (or `--static` for musl).",
            ),
            Self::SpaRequiresWasmTarget => write!(
                f,
                "a `spa` delivery is a sandboxed client that must compile to wasm, but \
                 the target resolved to native. The sandbox's native-deny guards are \
                 keyed to the wasm target, so a native `spa` build would ship native \
                 effects into the sandbox. Build for wasm — pass `--target wasm`, set \
                 `IPE_TARGET=wasm`, or set `[wasm] mode` in `package.ipe` — or drop \
                 `spa` for a co-located live delivery.",
            ),
            Self::WasmTargetRequiresSpa => write!(
                f,
                "a wasm compile target was requested, but the delivery is not `spa`. \
                 The wasm client target exists only to carry a sandboxed `spa` app; \
                 every other shape has no wasm form. Deliver `web spa` to build for \
                 wasm, or drop the wasm target (`--target`/`IPE_TARGET`/`[wasm] mode`) \
                 for a native build.",
            ),
            Self::NativeEngineRefusesWasmTriple { triple } => write!(
                f,
                "`{}` is a WebAssembly triple, but this build targets the native \
                 binary, which has no WASM form. The browser client compiles to \
                 `wasm32-unknown-unknown` (deliver `web spa`); the co-located WASI \
                 target compiles to `wasm32-wasip1`. Drop the WASM triple for a \
                 native build, or pick the delivery that carries it.",
                triple.as_str(),
            ),
            Self::SpaRequiresBrowserTriple { triple } => write!(
                f,
                "a `web spa` client compiles only to `wasm32-unknown-unknown`, but \
                 `{}` was requested. The sandboxed browser client has exactly one \
                 triple — its wasm sandbox. Drop the triple (it is implied by \
                 `spa`), or drop `spa` for the delivery that carries `{}`.",
                triple.as_str(),
                triple.as_str(),
            ),
            Self::SpaRefusesWasiTriple => write!(
                f,
                "a `web spa` client cannot target `wasm32-wasip1`. The browser \
                 sandbox denies native effects and reaches the world only through \
                 Web-API capabilities; WASI is the co-located, native-ish target \
                 for a `tui`/`cli`/`server`/served-`web` program, never the browser \
                 sandbox. Deliver `web spa` to `wasm32-unknown-unknown`, or use a \
                 co-located shape for a WASI build.",
            ),
            Self::WebviewHasNoStaticTriple { delivery } => write!(
                f,
                "`{delivery}` links the system webview at runtime, so it has no \
                 static (musl) triple. Use `web` (served-live), `tui`, `cli`, or \
                 `server` for a static musl binary, or ship the desktop app bundle.",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn web_no_tokens_is_served_live_default() {
        let t = DeliveryTokens::parse(&tokens(&[])).unwrap();
        let d = Delivery::resolve(Shape::Web, t.runtime, t.host).unwrap();
        assert_eq!(d.runtime(), Some(Runtime::Live));
        assert_eq!(d.host(), Host::Default);
        assert!(!d.is_webview_native());
        assert!(
            d.allows_static(),
            "served live is a co-located static target"
        );
        assert_eq!(d.to_string(), "web");
    }

    #[test]
    fn web_desktop_is_webview_native_and_not_static() {
        let t = DeliveryTokens::parse(&tokens(&["desktop"])).unwrap();
        let d = Delivery::resolve(Shape::Web, t.runtime, t.host).unwrap();
        assert!(d.is_webview_native());
        assert!(!d.allows_static());
        assert_eq!(d.to_string(), "web desktop");
    }

    #[test]
    fn web_spa_hosts_all_resolve() {
        for host in ["desktop", "ios", "android"] {
            let t = DeliveryTokens::parse(&tokens(&["spa", host])).unwrap();
            let d = Delivery::resolve(Shape::Web, t.runtime, t.host).unwrap();
            assert_eq!(d.runtime(), Some(Runtime::Spa));
            assert!(!d.is_webview_native(), "spa is never webview-native");
            assert!(!d.allows_static());
        }
        let t = DeliveryTokens::parse(&tokens(&["spa"])).unwrap();
        let d = Delivery::resolve(Shape::Web, t.runtime, t.host).unwrap();
        assert_eq!(d.host(), Host::Default);
        assert!(!d.allows_static());
    }

    #[test]
    fn live_is_never_a_word() {
        assert_eq!(
            DeliveryTokens::parse(&tokens(&["live"])).unwrap_err(),
            DeliveryError::LiveNotAWord
        );
        assert_eq!(
            DeliveryTokens::parse(&tokens(&["spa", "live"])).unwrap_err(),
            DeliveryError::LiveNotAWord
        );
    }

    #[test]
    fn live_refuses_mobile_hosts() {
        for host in [Host::Ios, Host::Android] {
            assert_eq!(
                Delivery::resolve(Shape::Web, None, host).unwrap_err(),
                DeliveryError::LiveHostNotMobile { host }
            );
        }
    }

    #[test]
    fn runtime_or_host_on_non_web_is_refused() {
        for shape in [Shape::Script, Shape::Tui, Shape::Cli, Shape::Server] {
            assert_eq!(
                Delivery::resolve(shape, Some(Runtime::Spa), Host::Default).unwrap_err(),
                DeliveryError::RuntimeOnNonWeb { shape }
            );
            assert_eq!(
                Delivery::resolve(shape, None, Host::Desktop).unwrap_err(),
                DeliveryError::HostOnNonWeb {
                    shape,
                    host: Host::Desktop
                }
            );
        }
    }

    #[test]
    fn non_web_shapes_are_static_capable() {
        for shape in [Shape::Script, Shape::Tui, Shape::Cli, Shape::Server] {
            let d = Delivery::resolve(shape, None, Host::Default).unwrap();
            assert_eq!(d.runtime(), None);
            assert!(d.allows_static());
        }
    }

    #[test]
    fn target_triple_positional_is_kept() {
        let t = DeliveryTokens::parse(&tokens(&["spa", "wasm32-unknown-unknown"])).unwrap();
        assert_eq!(t.runtime, Some(Runtime::Spa));
        assert_eq!(t.target.as_deref(), Some("wasm32-unknown-unknown"));
    }

    #[test]
    fn unknown_token_is_pedagogical() {
        assert_eq!(
            DeliveryTokens::parse(&tokens(&["wut"])).unwrap_err(),
            DeliveryError::UnknownToken {
                got: "wut".to_owned()
            }
        );
    }

    #[test]
    fn control_model_is_the_projection_of_the_pinned_shape() {
        // The view-ful shapes run the TEA loop; a server is declarative; a script
        // runs direct. Pins the SSOT projection against drift.
        assert_eq!(Shape::Web.control_model(), ControlModel::Tea);
        assert_eq!(Shape::Tui.control_model(), ControlModel::Tea);
        assert_eq!(Shape::Cli.control_model(), ControlModel::Tea);
        assert_eq!(Shape::Server.control_model(), ControlModel::Server);
        assert_eq!(Shape::Script.control_model(), ControlModel::Direct);
        // Every model has a stable, distinct word.
        assert_eq!(ControlModel::Tea.word(), "tea");
        assert_eq!(ControlModel::Server.word(), "server");
        assert_eq!(ControlModel::Direct.word(), "direct");
    }

    #[test]
    fn control_model_word_round_trips_and_rejects_unknown() {
        for model in [
            ControlModel::Tea,
            ControlModel::Server,
            ControlModel::Direct,
        ] {
            assert_eq!(ControlModel::from_word(model.word()), Some(model));
        }
        // A token outside the closed set is None — never a permissive default.
        assert_eq!(ControlModel::from_word("Direct"), None); // case-sensitive
        assert_eq!(ControlModel::from_word("telepathy"), None);
        assert_eq!(ControlModel::from_word(""), None);
    }

    #[test]
    fn only_direct_is_the_elevated_unmanaged_model() {
        // The managed models run under the runtime's loop; Direct drives itself.
        assert!(ControlModel::Tea.is_managed());
        assert!(ControlModel::Server.is_managed());
        assert!(!ControlModel::Direct.is_managed());
    }

    #[test]
    fn shape_words_round_trip() {
        for shape in [
            Shape::Script,
            Shape::Tui,
            Shape::Cli,
            Shape::Server,
            Shape::Web,
        ] {
            assert_eq!(Shape::from_word(shape.word()), Some(shape));
        }
        assert_eq!(Shape::from_word("nope"), None);
    }

    #[test]
    fn messages_teach_not_slap() {
        let cases = [
            DeliveryError::LiveNotAWord,
            DeliveryError::ShapeMismatch {
                stated: Shape::Tui,
                pinned: Shape::Web,
            },
            DeliveryError::RuntimeOnNonWeb { shape: Shape::Cli },
            DeliveryError::LiveHostNotMobile { host: Host::Ios },
            DeliveryError::StaticNotAllowed {
                delivery: Delivery::resolve(Shape::Web, None, Host::Desktop).unwrap(),
            },
            DeliveryError::SpaRequiresWasmTarget,
            DeliveryError::WasmTargetRequiresSpa,
        ];
        for c in &cases {
            assert!(c.to_string().len() > 40, "a refusal is a lesson: {c}");
        }
    }

    #[test]
    fn spa_delivery_refuses_native_target() {
        // The verified fail-open: a `web spa` app whose target resolved to native
        // would silently skip the wasm-keyed sandbox backstops. Refuse it.
        let spa = Delivery::resolve(Shape::Web, Some(Runtime::Spa), Host::Default).unwrap();
        assert_eq!(
            spa.reconcile_wasm_target(false),
            Err(DeliveryError::SpaRequiresWasmTarget),
        );
        // The legal pairing (spa ⇒ wasm) is admitted.
        assert_eq!(spa.reconcile_wasm_target(true), Ok(()));
    }

    #[test]
    fn wasm_target_refuses_non_spa_delivery() {
        // The symmetric half: a wasm target must carry a `spa` delivery. Every
        // non-`spa` shape has no wasm form, so the two disagreed at their sources.
        let live = Delivery::resolve(Shape::Web, Some(Runtime::Live), Host::Default).unwrap();
        assert_eq!(
            live.reconcile_wasm_target(true),
            Err(DeliveryError::WasmTargetRequiresSpa),
        );
        assert_eq!(live.reconcile_wasm_target(false), Ok(()));

        // A non-web shape has no runtime axis at all: native-only, wasm refused.
        for shape in [Shape::Script, Shape::Cli, Shape::Server, Shape::Tui] {
            let d = Delivery::resolve(shape, None, Host::Default).unwrap();
            assert_eq!(
                d.reconcile_wasm_target(true),
                Err(DeliveryError::WasmTargetRequiresSpa),
                "a non-web {shape:?} shape has no wasm form",
            );
            assert_eq!(d.reconcile_wasm_target(false), Ok(()));
        }
    }

    #[test]
    fn spa_wasm_biconditional_is_exhaustive() {
        // Both agreeing corners pass; both disagreeing corners are refused —
        // the invariant is `spa` IFF wasm, with no admitted middle.
        let spa = Delivery::resolve(Shape::Web, Some(Runtime::Spa), Host::Default).unwrap();
        let live = Delivery::resolve(Shape::Web, Some(Runtime::Live), Host::Default).unwrap();
        assert!(spa.reconcile_wasm_target(true).is_ok());
        assert!(live.reconcile_wasm_target(false).is_ok());
        assert!(spa.reconcile_wasm_target(false).is_err());
        assert!(live.reconcile_wasm_target(true).is_err());
    }

    // === The engine × host × triple validity matrix (#2461) ===

    fn spa() -> Delivery {
        Delivery::resolve(Shape::Web, Some(Runtime::Spa), Host::Default).unwrap()
    }
    fn served_live() -> Delivery {
        Delivery::resolve(Shape::Web, Some(Runtime::Live), Host::Default).unwrap()
    }
    fn web_desktop() -> Delivery {
        Delivery::resolve(Shape::Web, Some(Runtime::Live), Host::Desktop).unwrap()
    }

    #[test]
    fn target_triple_parse_is_closed() {
        // Every representable triple round-trips; everything else is `None` —
        // parse, don't validate. The empty string and one-past-the-last-legal
        // (`wasm64`, gnu, a bogus wasm flavour) are all refused at the boundary.
        assert_eq!(
            TargetTriple::parse("x86_64-unknown-linux-musl"),
            Some(TargetTriple::X8664LinuxMusl)
        );
        assert_eq!(
            TargetTriple::parse("aarch64-unknown-linux-musl"),
            Some(TargetTriple::Aarch64LinuxMusl)
        );
        assert_eq!(
            TargetTriple::parse("wasm32-unknown-unknown"),
            Some(TargetTriple::BrowserWasm)
        );
        assert_eq!(
            TargetTriple::parse("wasm32-wasip1"),
            Some(TargetTriple::Wasm32Wasip1)
        );
        assert_eq!(TargetTriple::parse(""), None);
        assert_eq!(TargetTriple::parse("x86_64-unknown-linux-gnu"), None);
        assert_eq!(TargetTriple::parse("wasm32-wasi"), None);
        assert_eq!(TargetTriple::parse("wasm64-unknown-unknown"), None);
        // The spellings are stable and match rustc's.
        assert_eq!(TargetTriple::Wasm32Wasip1.as_str(), "wasm32-wasip1",);
        assert!(TargetTriple::BrowserWasm.is_wasm());
        assert!(TargetTriple::Wasm32Wasip1.is_wasm());
        assert!(!TargetTriple::X8664LinuxMusl.is_wasm());
        assert!(!TargetTriple::Host.is_wasm());
    }

    #[test]
    fn engine_projects_the_backend_target() {
        // The delivery-layer engine is a projection of the backend target, never
        // a second derivation — pins the SSOT map against drift.
        assert_eq!(Engine::of(ipe_ir::Target::Native), Engine::Native);
        assert_eq!(Engine::of(ipe_ir::Target::WasmClient), Engine::WasmClient);
    }

    // --- Refusals first: every illegal cell turned away with a typed diagnostic.

    #[test]
    fn spa_refuses_wasi_triple() {
        // THE headline separation: the browser sandbox never widens to WASI.
        assert_eq!(
            spa().admit_triple(Engine::WasmClient, TargetTriple::Wasm32Wasip1),
            Err(DeliveryError::SpaRefusesWasiTriple),
        );
    }

    #[test]
    fn colocated_wasi_is_refused_until_runtime() {
        // Co-located WASI has NO accept-path in this increment (the runtime port
        // has not landed). Every co-located delivery asking for `wasm32-wasip1`
        // on the only engines that exist today (Native/WasmClient) is refused —
        // proving no accept-path opened that could break THE SEAL.
        for d in [
            Delivery::resolve(Shape::Script, None, Host::Default).unwrap(),
            Delivery::resolve(Shape::Tui, None, Host::Default).unwrap(),
            Delivery::resolve(Shape::Cli, None, Host::Default).unwrap(),
            Delivery::resolve(Shape::Server, None, Host::Default).unwrap(),
            served_live(),
        ] {
            assert_eq!(
                d.admit_triple(Engine::Native, TargetTriple::Wasm32Wasip1),
                Err(DeliveryError::NativeEngineRefusesWasmTriple {
                    triple: TargetTriple::Wasm32Wasip1
                }),
                "co-located WASI must stay refused for {d} until the runtime lands",
            );
        }
    }

    #[test]
    fn spa_requires_browser_triple() {
        // A `spa` client on any non-browser triple is refused.
        for t in [
            TargetTriple::Host,
            TargetTriple::X8664LinuxMusl,
            TargetTriple::Aarch64LinuxMusl,
        ] {
            assert_eq!(
                spa().admit_triple(Engine::WasmClient, t),
                Err(DeliveryError::SpaRequiresBrowserTriple { triple: t }),
            );
        }
    }

    #[test]
    fn native_engine_refuses_wasm_triples() {
        // Both wasm flavours have no native form.
        for t in [TargetTriple::BrowserWasm, TargetTriple::Wasm32Wasip1] {
            assert_eq!(
                served_live().admit_triple(Engine::Native, t),
                Err(DeliveryError::NativeEngineRefusesWasmTriple { triple: t }),
            );
        }
    }

    #[test]
    fn native_engine_refuses_spa_delivery() {
        // A `spa` delivery must not resolve to the native engine — the
        // wasm-keyed sandbox backstops would be skipped. Subsumes the
        // biconditional's `SpaRequiresWasmTarget` half on the triple axis.
        assert_eq!(
            spa().admit_triple(Engine::Native, TargetTriple::Host),
            Err(DeliveryError::SpaRequiresWasmTarget),
        );
    }

    #[test]
    fn wasm_client_engine_refuses_non_spa_delivery() {
        // The symmetric half: the browser client engine only carries `spa`.
        for d in [
            served_live(),
            web_desktop(),
            Delivery::resolve(Shape::Cli, None, Host::Default).unwrap(),
        ] {
            assert_eq!(
                d.admit_triple(Engine::WasmClient, TargetTriple::BrowserWasm),
                Err(DeliveryError::WasmTargetRequiresSpa),
                "the browser client engine must refuse the non-spa delivery {d}",
            );
        }
    }

    #[test]
    fn webview_native_has_no_static_triple() {
        // Mirrors the `--static` × webview refusal on the triple axis: a
        // webview-native delivery has no musl binary.
        for t in [TargetTriple::X8664LinuxMusl, TargetTriple::Aarch64LinuxMusl] {
            assert_eq!(
                web_desktop().admit_triple(Engine::Native, t),
                Err(DeliveryError::WebviewHasNoStaticTriple {
                    delivery: web_desktop()
                }),
            );
        }
    }

    // --- Legal cells: the enumerated admissions.

    #[test]
    fn browser_spa_admits_browser_wasm() {
        assert_eq!(
            spa().admit_triple(Engine::WasmClient, TargetTriple::BrowserWasm),
            Ok(()),
        );
    }

    #[test]
    fn native_colocated_admits_host_and_musl() {
        for d in [
            Delivery::resolve(Shape::Script, None, Host::Default).unwrap(),
            Delivery::resolve(Shape::Tui, None, Host::Default).unwrap(),
            Delivery::resolve(Shape::Cli, None, Host::Default).unwrap(),
            Delivery::resolve(Shape::Server, None, Host::Default).unwrap(),
            served_live(),
        ] {
            assert_eq!(d.admit_triple(Engine::Native, TargetTriple::Host), Ok(()));
            assert_eq!(
                d.admit_triple(Engine::Native, TargetTriple::X8664LinuxMusl),
                Ok(()),
                "{d} is static-capable on x86-64 musl",
            );
            assert_eq!(
                d.admit_triple(Engine::Native, TargetTriple::Aarch64LinuxMusl),
                Ok(()),
            );
        }
    }

    #[test]
    fn matrix_subsumes_the_biconditional() {
        // The matrix must agree with `reconcile_wasm_target` on all four bool
        // corners, so the two stay in lock-step until the callsites migrate —
        // the matrix is a strict superset, never a regression of a refusal.
        // (engine, triple) stands in for the bool: WasmClient+BrowserWasm ==
        // wasm_target true; Native+Host == wasm_target false.
        let spa = spa();
        let live = served_live();
        // spa + wasm  <=>  reconcile(true) ok
        assert_eq!(
            spa.admit_triple(Engine::WasmClient, TargetTriple::BrowserWasm)
                .is_ok(),
            spa.reconcile_wasm_target(true).is_ok(),
        );
        // spa + native  <=>  reconcile(false) err
        assert_eq!(
            spa.admit_triple(Engine::Native, TargetTriple::Host)
                .is_err(),
            spa.reconcile_wasm_target(false).is_err(),
        );
        // live + wasm  <=>  reconcile(true) err
        assert_eq!(
            live.admit_triple(Engine::WasmClient, TargetTriple::BrowserWasm)
                .is_err(),
            live.reconcile_wasm_target(true).is_err(),
        );
        // live + native  <=>  reconcile(false) ok
        assert_eq!(
            live.admit_triple(Engine::Native, TargetTriple::Host)
                .is_ok(),
            live.reconcile_wasm_target(false).is_ok(),
        );
    }

    #[test]
    fn matrix_refusals_teach_not_slap() {
        let cases = [
            DeliveryError::NativeEngineRefusesWasmTriple {
                triple: TargetTriple::Wasm32Wasip1,
            },
            DeliveryError::SpaRequiresBrowserTriple {
                triple: TargetTriple::X8664LinuxMusl,
            },
            DeliveryError::SpaRefusesWasiTriple,
            DeliveryError::WebviewHasNoStaticTriple {
                delivery: web_desktop(),
            },
        ];
        for c in &cases {
            assert!(c.to_string().len() > 40, "a refusal is a lesson: {c}");
        }
    }
}
