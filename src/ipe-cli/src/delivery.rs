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

/// A fully-resolved delivery target — a shape, its (web-only) runtime, and a
/// valid host. Constructible only through [`Delivery::resolve`], so no invalid
/// combination can be built. The runtime is `None` for every non-web shape (the
/// axis does not exist for them).
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
}

impl fmt::Display for Delivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.shape.word())?;
        if let Some(Runtime::Spa) = self.runtime {
            f.write_str(" spa")?;
        }
        if let Some(word) = self.host.word() {
            write!(f, " {word}")?;
        }
        Ok(())
    }
}

/// A pedagogical delivery refusal (spec § 6): each variant names the problem,
/// explains the two-axis big picture in a sentence, and suggests the fix. This
/// enum is the message-set single source of truth — every delivery diagnostic
/// is one of these, phrased once.
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
        ];
        for c in &cases {
            assert!(c.to_string().len() > 40, "a refusal is a lesson: {c}");
        }
    }
}
