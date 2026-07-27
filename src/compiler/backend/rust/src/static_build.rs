//! Static-build support for the emitted project (design:
//! `docs/architecture/static-compilation.md`).
//!
//! A fully-resolved [`StaticPlan`] arrives from the CLI's typed resolver —
//! every refusal gate (allocator cliff acknowledgment, unsupported target,
//! app-shape) has already passed by the time a plan reaches this module. The
//! module then makes the emitted crate static-correct **by construction**:
//!
//! * [`staticize_manifest`] activates exactly one `alloc_*` feature in the
//!   emitted `Cargo.toml`'s `default = [...]` list (the mutually-exclusive
//!   feature family the golden base manifest declares), so a standalone
//!   `cargo build --target <triple>` selects the planned allocator without
//!   any `--features` flag.
//! * [`cargo_config`] renders the emitted crate's `.cargo/config.toml`
//!   supplying `+crt-static` for the target, headed by
//!   [`CARGO_CONFIG_MARKER`] so the driver can recognise (and, on a later
//!   non-static build, remove) a file it generated.
//! * [`manifest_is_webview`] is the typed app-shape probe the driver's
//!   webview-under-static refusal reads: an `Ipe.WebView` app links the
//!   system webview and can never be a static artifact.
//!
//! Anchored-`replacen` surgery with fail-loud [`Diagnostic::CompilerBug`] on
//! anchor drift, exactly like the sibling `*_cargo_toml` functions in
//! [`crate::project`].

use ipe_diagnostics::{DResult, Diagnostic};

/// Target triples the static path supports — a closed set.
///
/// Parse, don't validate: the CLI parses `--target` into this enum and
/// refuses anything else, so an unverifiable triple can never reach cargo
/// and violate the SEAL. Growing the set (Windows `+crt-static`, wasm) means
/// adding a variant alongside a CI lane that proves it end-to-end.
///
/// Toolchain requirements per variant:
/// - [`Self::X8664LinuxMusl`]: `musl-tools` (apt) + `x86_64-unknown-linux-musl`
///   rustup target. Verified in CI on `ubuntu-latest`.
/// - [`Self::Aarch64LinuxMusl`]: `gcc-aarch64-linux-gnu` + `musl-cross` (or
///   `aarch64-linux-musl-gcc` from `musl.cc`) + `aarch64-unknown-linux-musl`
///   rustup target + `qemu-user-static` for cross-run verification.
///   CI: `ubuntu-24.04-arm` (native runner) or `ubuntu-latest` with the cross
///   toolchain installed. See `.github/workflows/static.yml` `linux-static-arm64`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StaticTriple {
    /// `x86_64-unknown-linux-musl` — fully static ELF, zero runtime deps.
    /// The default when `--static` is given without `--target`.
    #[default]
    X8664LinuxMusl,
    /// `aarch64-unknown-linux-musl` — fully static `AArch64` ELF, zero runtime
    /// deps. Requires the `aarch64-unknown-linux-musl` rustup target and a
    /// musl-capable `AArch64` C linker (`aarch64-linux-musl-gcc` or equivalent).
    Aarch64LinuxMusl,
}

impl StaticTriple {
    /// The rustc target-triple spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X8664LinuxMusl => "x86_64-unknown-linux-musl",
            Self::Aarch64LinuxMusl => "aarch64-unknown-linux-musl",
        }
    }

    /// Parse a rustc triple into the supported set. `None` means the triple
    /// is not (yet) a supported static target — the CLI turns that into a
    /// typed refusal listing [`Self::SUPPORTED`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "x86_64-unknown-linux-musl" => Some(Self::X8664LinuxMusl),
            "aarch64-unknown-linux-musl" => Some(Self::Aarch64LinuxMusl),
            _ => None,
        }
    }

    /// Every supported triple, for refusal messages.
    pub const SUPPORTED: &'static [&'static str] =
        &["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"];
}

/// The allocator linked into a static artifact.
///
/// `System` on musl is the acknowledged-cliff choice — representable here
/// because the CLI's resolver only constructs it after the two-key
/// acknowledgment (`--allow-slow-allocator` / `allowSlowAllocator`) passed.
/// `talc` has no variant: it is refused at CLI parse-resolution (hosted talc
/// needs an unsafe static arena — amendment A1), so an unsupported allocator
/// is unrepresentable in a plan.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StaticAllocator {
    /// Pure-Rust dlmalloc — the default for every static plan.
    Dlmalloc,
    /// C mimalloc — explicit opt-in (C toolchain + vendored C in the artifact).
    Mimalloc,
    /// The target libc's malloc — on musl this is the 0.14× cliff; only
    /// reachable through the acknowledgment gate.
    System,
}

impl StaticAllocator {
    /// The emitted cargo feature activating this allocator, or `None` for
    /// `System` (no feature ⇒ no `#[global_allocator]` item compiles).
    #[must_use]
    pub const fn feature(self) -> Option<&'static str> {
        match self {
            Self::Dlmalloc => Some("alloc_dlmalloc"),
            Self::Mimalloc => Some("alloc_mimalloc"),
            Self::System => None,
        }
    }
}

/// A fully-resolved static build plan. Constructed only by the CLI's
/// resolver; reaching the emitter means every refusal gate already passed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StaticPlan {
    pub triple: StaticTriple,
    pub allocator: StaticAllocator,
}

/// First line of every generated `.cargo/config.toml`.
///
/// The driver recognises a file starting with this marker as its own
/// output: it may overwrite or remove it, and will never touch a config a
/// user placed there by hand.
pub const CARGO_CONFIG_MARKER: &str =
    "# Generated by `ipe build --static` — do not edit; regenerated on every static build.";

/// Render the emitted crate's `.cargo/config.toml` for `plan`.
///
/// `+crt-static` produces the fully-static (static-pie) executable. No
/// `target-dir` key is ever written (the shared-target pin is the user's
/// concern), and no linker is pinned: the default driver links the verified
/// dep set (including the `zstd`/`ring` C units) when a musl-capable C
/// compiler is present — presence the CLI preflight has already checked.
#[must_use]
pub fn cargo_config(plan: &StaticPlan) -> String {
    // AArch64 links through the bundled `rust-lld` with a self-contained musl
    // startup, so a static AArch64 build needs only a C cross-compiler for the
    // C deps (zstd/ring) — no scarce `aarch64-linux-musl-gcc`. Native
    // x86_64-musl keeps the default linker.
    let cross_link = match plan.triple {
        StaticTriple::Aarch64LinuxMusl => {
            ", \"-C\", \"linker=rust-lld\", \"-C\", \"link-self-contained=yes\""
        }
        StaticTriple::X8664LinuxMusl => "",
    };
    format!(
        "{CARGO_CONFIG_MARKER}\n\
         [target.{triple}]\n\
         rustflags = [\"-C\", \"target-feature=+crt-static\"{cross_link}]\n",
        triple = plan.triple.as_str()
    )
}

/// Activate the chosen allocator feature in the emitted manifest.
///
/// Splices it as the LAST element of the `default = [...]` feature list —
/// the same generic closing-`]` anchor `server_cargo_toml` uses, so the
/// splice composes with every db/server/live/tui/webview surgery already
/// applied.
///
/// `System` is the identity: no feature ⇒ the emitted source's cfg-gated
/// allocator arms stay inert and the binary uses the target libc's malloc.
///
/// # Errors
///
/// [`Diagnostic::CompilerBug`] when the `default = [` anchor (or its closing
/// `]`) is absent, or when an `alloc_*` feature is already active — either
/// means the manifest drifted from the golden or the splice ran twice; both
/// are invariant breaches, never silently absorbed.
pub fn staticize_manifest(base: &str, allocator: StaticAllocator) -> DResult<String> {
    const DEFAULT_PREFIX: &str = "default = [";

    let Some(feature) = allocator.feature() else {
        return Ok(base.to_owned());
    };

    let pfx = base
        .find(DEFAULT_PREFIX)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::static_build::staticize_manifest",
            detail: format!("Cargo.toml anchor {DEFAULT_PREFIX:?} not found — golden drifted"),
        })?;
    let search_from = pfx + DEFAULT_PREFIX.len();
    let rel = base
        .get(search_from..)
        .and_then(|s| s.find(']'))
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::static_build::staticize_manifest",
            detail: "default feature list has no closing ']' — golden drifted".to_owned(),
        })?;
    let close = search_from + rel;
    let default_list = base.get(search_from..close).unwrap_or("");
    if default_list.contains("alloc_") {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::static_build::staticize_manifest",
            detail: format!(
                "default feature list already activates an allocator ({default_list:?}) — \
                 the static splice must run exactly once"
            ),
        });
    }

    let mut out = String::with_capacity(base.len() + feature.len() + 4);
    out.push_str(base.get(..close).unwrap_or(""));
    out.push_str(", \"");
    out.push_str(feature);
    out.push('"');
    out.push_str(base.get(close..).unwrap_or(""));
    Ok(out)
}

/// Whether the emitted manifest is an `Ipe.WebView` app.
///
/// Read from the `default = [...]` feature list the backend computed (the
/// machine-readable app-shape record every `*_cargo_toml` surgery
/// maintains). The driver's static path refuses webview apps before writing
/// any file: they link the system WebKit/WebView2 and cannot be static.
///
/// # Errors
///
/// [`Diagnostic::CompilerBug`] when the manifest has no `default = [` list —
/// a drifted manifest, not a decidable shape.
pub fn manifest_is_webview(cargo_toml: &str) -> DResult<bool> {
    const DEFAULT_PREFIX: &str = "default = [";
    let pfx = cargo_toml
        .find(DEFAULT_PREFIX)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::static_build::manifest_is_webview",
            detail: format!("Cargo.toml anchor {DEFAULT_PREFIX:?} not found — golden drifted"),
        })?;
    let search_from = pfx + DEFAULT_PREFIX.len();
    let rel = cargo_toml
        .get(search_from..)
        .and_then(|s| s.find(']'))
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::static_build::manifest_is_webview",
            detail: "default feature list has no closing ']' — golden drifted".to_owned(),
        })?;
    let default_list = cargo_toml.get(search_from..search_from + rel).unwrap_or("");
    Ok(default_list.contains("\"webview\""))
}

#[cfg(test)]
mod tests {
    use super::{
        CARGO_CONFIG_MARKER, StaticAllocator, StaticPlan, StaticTriple, cargo_config,
        manifest_is_webview, staticize_manifest,
    };
    use ipe_diagnostics::DResult;

    /// The golden base manifest — the exact text every emitted `Cargo.toml`
    /// starts from.
    const CARGO_TOML: &str = include_str!("../../../../../tests/golden/basics/Cargo.toml");

    fn default_line(manifest: &str) -> &str {
        manifest
            .lines()
            .find(|l| l.starts_with("default = ["))
            .unwrap_or("")
    }

    #[test]
    fn base_manifest_declares_the_alloc_family_but_activates_none() {
        assert!(CARGO_TOML.contains(r#"alloc_dlmalloc = ["dep:dlmalloc"]"#));
        assert!(CARGO_TOML.contains(r#"alloc_mimalloc = ["dep:mimalloc"]"#));
        assert!(
            !default_line(CARGO_TOML).contains("alloc_"),
            "default build must use the system allocator"
        );
        assert!(
            !CARGO_TOML.contains("static_alloc"),
            "the inherited static_alloc default must be gone"
        );
    }

    #[test]
    fn dlmalloc_splice_activates_exactly_one_alloc_feature() -> DResult<()> {
        let out = staticize_manifest(CARGO_TOML, StaticAllocator::Dlmalloc)?;
        let def = default_line(&out);
        assert!(def.contains(r#""alloc_dlmalloc""#), "{def}");
        assert_eq!(def.matches("alloc_").count(), 1, "{def}");
        Ok(())
    }

    #[test]
    fn mimalloc_splice_activates_exactly_one_alloc_feature() -> DResult<()> {
        let out = staticize_manifest(CARGO_TOML, StaticAllocator::Mimalloc)?;
        let def = default_line(&out);
        assert!(def.contains(r#""alloc_mimalloc""#), "{def}");
        assert_eq!(def.matches("alloc_").count(), 1, "{def}");
        Ok(())
    }

    #[test]
    fn system_splice_is_the_identity() -> DResult<()> {
        assert_eq!(
            staticize_manifest(CARGO_TOML, StaticAllocator::System)?,
            CARGO_TOML
        );
        Ok(())
    }

    #[test]
    fn double_splice_is_a_compiler_bug() -> DResult<()> {
        let once = staticize_manifest(CARGO_TOML, StaticAllocator::Dlmalloc)?;
        assert!(staticize_manifest(&once, StaticAllocator::Dlmalloc).is_err());
        Ok(())
    }

    #[test]
    fn anchor_miss_is_a_compiler_bug() {
        assert!(
            staticize_manifest("[package]\nname = \"x\"\n", StaticAllocator::Dlmalloc).is_err()
        );
    }

    #[test]
    fn splice_composes_with_server_style_appends() -> DResult<()> {
        // A db+server-shaped default list still gets exactly one allocator,
        // appended last.
        let base = CARGO_TOML.replacen(
            r#"default = ["tokio", "crypto", "json"]"#,
            r#"default = ["tokio", "crypto", "json", "db", "server"]"#,
            1,
        );
        let out = staticize_manifest(&base, StaticAllocator::Dlmalloc)?;
        assert!(
            default_line(&out)
                .contains(r#""tokio", "crypto", "json", "db", "server", "alloc_dlmalloc""#)
        );
        Ok(())
    }

    #[test]
    fn cargo_config_pins_crt_static_and_nothing_else() {
        let cfg = cargo_config(&StaticPlan {
            triple: StaticTriple::X8664LinuxMusl,
            allocator: StaticAllocator::Dlmalloc,
        });
        assert!(cfg.starts_with(CARGO_CONFIG_MARKER));
        assert!(cfg.contains("[target.x86_64-unknown-linux-musl]"));
        assert!(cfg.contains(r#""target-feature=+crt-static""#));
        assert!(
            !cfg.contains("target-dir"),
            "must not shadow the user's pin"
        );
        assert!(
            !cfg.contains("linker"),
            "no linker pin — preflight owns tooling checks"
        );
    }

    #[test]
    fn triple_parse_is_closed() {
        assert_eq!(
            StaticTriple::parse("x86_64-unknown-linux-musl"),
            Some(StaticTriple::X8664LinuxMusl)
        );
        assert_eq!(
            StaticTriple::parse("aarch64-unknown-linux-musl"),
            Some(StaticTriple::Aarch64LinuxMusl)
        );
        assert_eq!(StaticTriple::parse("x86_64-unknown-linux-gnu"), None);
        assert_eq!(StaticTriple::parse("aarch64-unknown-linux-gnu"), None);
        assert_eq!(StaticTriple::parse(""), None);
    }

    #[test]
    fn aarch64_musl_cargo_config_pins_crt_static() {
        let cfg = cargo_config(&StaticPlan {
            triple: StaticTriple::Aarch64LinuxMusl,
            allocator: StaticAllocator::Dlmalloc,
        });
        assert!(cfg.starts_with(CARGO_CONFIG_MARKER));
        assert!(cfg.contains("[target.aarch64-unknown-linux-musl]"));
        assert!(cfg.contains(r#""target-feature=+crt-static""#));
        assert!(
            !cfg.contains("target-dir"),
            "must not shadow the user's pin"
        );
        assert!(
            cfg.contains("linker=rust-lld"),
            "aarch64 pins rust-lld self-contained (portable, no scarce musl-cross-gcc)"
        );
    }

    #[test]
    fn webview_shape_is_read_from_the_default_list() -> DResult<()> {
        assert!(!manifest_is_webview(CARGO_TOML)?);
        let webview = CARGO_TOML.replacen(
            r#"default = ["tokio", "crypto", "json"]"#,
            r#"default = ["tokio", "crypto", "json", "live", "webview"]"#,
            1,
        );
        assert!(manifest_is_webview(&webview)?);
        // The feature *definition* (`webview = []`) alone is not the shape.
        assert!(CARGO_TOML.contains("webview = []"));
        Ok(())
    }

    #[test]
    fn webview_probe_without_default_list_is_a_compiler_bug() {
        assert!(manifest_is_webview("[package]\n").is_err());
    }
}
