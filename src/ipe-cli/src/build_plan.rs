//! Typed resolution of a static-build request into a backend
//! [`StaticPlan`] — parse, don't validate (design:
//! `docs/architecture/static-compilation.md`).
//!
//! The request arrives through three precedence layers — CLI flags, env
//! (`IPE_STATIC` / `IPE_TARGET` / `IPE_ALLOC`), `ipe.toml` `[rust]` — merged
//! per-field (CLI > env > toml) and resolved ONCE into
//! `Result<Option<StaticPlan>, Refusal>` before any compilation starts.
//! Downstream code sees either `None` (a normal dynamic build) or a plan
//! whose every gate already passed; an illegal combination never constructs
//! a plan, it surfaces a loud, typed [`Refusal`].

use std::fmt;

use ipe_backend_rust::static_build::{StaticAllocator, StaticPlan, StaticTriple};

/// The user's allocator choice before AUTO resolution — a closed enum.
///
/// [`Self::parse`] rejects anything outside it (including `jemalloc` /
/// `snmalloc`, which the design documents and rejects) — no silent string
/// fall-through.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AllocatorChoice {
    /// Resolve by target: musl-static → dlmalloc.
    #[default]
    Auto,
    /// The target libc's malloc. On musl this is the 0.14× throughput cliff
    /// and requires the explicit acknowledgment key.
    System,
    /// Pure-Rust dlmalloc (the static default).
    Dlmalloc,
    /// Pure-Rust talc — parses (the enum stays closed and stable) but is
    /// refused at resolution until an arena design lands (amendment A1).
    Talc,
    /// C mimalloc — explicit opt-in carrying a C toolchain + vendored C.
    Mimalloc,
}

impl AllocatorChoice {
    /// Parse a user-supplied allocator name.
    ///
    /// # Errors
    /// [`Refusal::UnknownAllocator`] for any name outside the closed set.
    pub fn parse(s: &str) -> Result<Self, Refusal> {
        match s {
            "auto" => Ok(Self::Auto),
            "system" => Ok(Self::System),
            "dlmalloc" => Ok(Self::Dlmalloc),
            "talc" => Ok(Self::Talc),
            "mimalloc" => Ok(Self::Mimalloc),
            other => Err(Refusal::UnknownAllocator {
                got: other.to_owned(),
            }),
        }
    }
}

/// One precedence layer of the static request (CLI, env, or `ipe.toml`).
/// Every field is `Option` so a layer overrides only what it actually sets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StaticRequestLayer {
    /// `--static` / `IPE_STATIC` / `[rust] static`.
    pub static_build: Option<bool>,
    /// `--target` / `IPE_TARGET` / `[rust] target` — a rustc triple string,
    /// parsed into the closed [`StaticTriple`] set at resolution.
    pub target: Option<String>,
    /// `--allocator` / `IPE_ALLOC` / `[rust] allocator`.
    pub allocator: Option<AllocatorChoice>,
    /// `--allow-slow-allocator` / `[rust] allowSlowAllocator` — the second
    /// key of the two-key musl-malloc-cliff acknowledgment.
    pub allow_slow_allocator: Option<bool>,
    /// `--cfree` / `IPE_CFREE` / `[rust] cFree` — the C-free build axis,
    /// orthogonal to the triple and the allocator.
    pub c_free: Option<bool>,
}

impl StaticRequestLayer {
    /// Per-field precedence merge: `self` wins over `weaker`.
    #[must_use]
    pub fn or(self, weaker: Self) -> Self {
        Self {
            static_build: self.static_build.or(weaker.static_build),
            target: self.target.or(weaker.target),
            allocator: self.allocator.or(weaker.allocator),
            allow_slow_allocator: self.allow_slow_allocator.or(weaker.allow_slow_allocator),
            c_free: self.c_free.or(weaker.c_free),
        }
    }
}

/// Read the env layer (`IPE_STATIC` / `IPE_TARGET` / `IPE_ALLOC`).
///
/// # Errors
/// [`Refusal::InvalidBool`] / [`Refusal::UnknownAllocator`] — a set-but-
/// malformed variable is refused, never silently ignored.
pub fn env_layer() -> Result<StaticRequestLayer, Refusal> {
    let static_build = match std::env::var("IPE_STATIC") {
        Ok(v) => Some(parse_bool("IPE_STATIC", &v)?),
        Err(_) => None,
    };
    let target = std::env::var("IPE_TARGET").ok();
    let allocator = match std::env::var("IPE_ALLOC") {
        Ok(v) => Some(AllocatorChoice::parse(&v)?),
        Err(_) => None,
    };
    let c_free = match std::env::var("IPE_CFREE") {
        Ok(v) => Some(parse_bool("IPE_CFREE", &v)?),
        Err(_) => None,
    };
    Ok(StaticRequestLayer {
        static_build,
        target,
        allocator,
        allow_slow_allocator: None,
        c_free,
    })
}

/// Parse a boolean request value (`ipe.toml` key or env var).
///
/// # Errors
/// [`Refusal::InvalidBool`] naming the source and the malformed value.
pub fn parse_bool(source: &'static str, v: &str) -> Result<bool, Refusal> {
    match v {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        other => Err(Refusal::InvalidBool {
            source,
            got: other.to_owned(),
        }),
    }
}

/// A typed reason a static-build request cannot be honoured. Surfaced
/// through `CliError::StaticRefusal`; every message is actionable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The allocator name is outside the closed set.
    UnknownAllocator { got: String },
    /// The requested triple is not a supported static target.
    UnknownStaticTarget { got: String },
    /// `--target` was given without `--static` — cross-compiling a dynamic
    /// build is not a wired path, and silently ignoring the flag would lie.
    TargetRequiresStatic { got: String },
    /// A non-default allocator was requested for a dynamic build — dynamic
    /// allocator selection is not a wired path.
    AllocatorRequiresStatic { got: AllocatorChoice },
    /// `system` malloc on a musl target without the acknowledgment key: the
    /// 0.14× cliff must be constructible only on purpose, by two keys.
    MuslMallocCliff,
    /// talc as a hosted `#[global_allocator]` needs an unsafe static arena
    /// with a hard heap cap — deferred until an arena design passes the
    /// no-unsafe gate (design amendment A1).
    TalcRequiresArenaDesign,
    /// The program is an `Ipe.WebView` app; it links the system webview and
    /// can never be a static artifact.
    WebviewStatic,
    /// The rustup toolchain has no std for the target.
    TargetNotInstalled { triple: &'static str },
    /// The dependency graph carries C compile units (`zstd`, `ring`) and no
    /// musl-capable C compiler is reachable.
    MuslCCompilerMissing { triple: &'static str },
    /// `mimalloc` was requested alongside `--cfree`: mimalloc vendors and
    /// links C, so it is unrepresentable in a C-free plan.
    MimallocUnderCfree,
    /// `--cfree` was requested, but the dependency-graph swaps that make the
    /// build actually C-free (a pure-Rust rustls provider for `ring`, a
    /// pure-Rust path for `zstd`) have not landed. The plan axis exists; the
    /// build would still pull C, so honouring the flag would be a lie —
    /// refused until those swaps are wired.
    CfreeNotYetWired,
    /// A boolean request value (env var or `ipe.toml` key) is malformed.
    InvalidBool { source: &'static str, got: String },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownAllocator { got } => write!(
                f,
                "unknown allocator {got:?} — expected one of: auto, system, dlmalloc, talc, mimalloc"
            ),
            Self::UnknownStaticTarget { got } => write!(
                f,
                "{got:?} is not a supported static target — supported: {}",
                StaticTriple::SUPPORTED.join(", ")
            ),
            Self::TargetRequiresStatic { got } => write!(
                f,
                "--target {got} requires --static (cross-compiling a dynamic build is not supported)"
            ),
            Self::AllocatorRequiresStatic { got } => write!(
                f,
                "--allocator {got:?} requires --static (allocator selection applies to static builds)"
            ),
            Self::MuslMallocCliff => write!(
                f,
                "refusing system malloc on a musl-static target: musl's malloc is ~7x slower on \
                 allocation-heavy workloads. Pass --allow-slow-allocator (or set \
                 [rust] allowSlowAllocator = true in ipe.toml) to accept, or drop \
                 --allocator system to get the dlmalloc default"
            ),
            Self::TalcRequiresArenaDesign => write!(
                f,
                "the talc allocator is not wired yet: a hosted talc #[global_allocator] needs a \
                 static arena design that has not landed. Use the dlmalloc default instead"
            ),
            Self::WebviewStatic => write!(
                f,
                "an Ipe.WebView app cannot be built --static: it links the system webview \
                 (WebKit/WebView2), which has no static form"
            ),
            Self::TargetNotInstalled { triple } => write!(
                f,
                "the target {triple} is not installed — run: rustup target add {triple}"
            ),
            Self::MuslCCompilerMissing { triple } => write!(
                f,
                "no musl-capable C compiler found for {triple} (the emitted project's zstd/ring \
                 dependencies compile C). Install one (Debian/Ubuntu: apt install musl-tools) or \
                 set CC_{}",
                triple.replace('-', "_")
            ),
            Self::MimallocUnderCfree => write!(
                f,
                "--allocator mimalloc cannot combine with --cfree: mimalloc vendors and links C. \
                 Drop --cfree, or use the dlmalloc default (pure Rust)"
            ),
            Self::CfreeNotYetWired => write!(
                f,
                "--cfree is not wired yet: the pure-Rust dependency swaps that make the build \
                 link no C have not landed, so the build would still pull C. Drop --cfree"
            ),
            Self::InvalidBool { source, got } => {
                write!(f, "{source}: expected true/false/1/0, got {got:?}")
            }
        }
    }
}

/// Resolve the merged request into `Ok(None)` (dynamic build), a
/// [`StaticPlan`], or a [`Refusal`]. Pure — the AUTO table and every
/// combination gate live here and nowhere else.
///
/// # Errors
/// The [`Refusal`] naming the first gate the request fails.
pub fn resolve(merged: &StaticRequestLayer) -> Result<Option<StaticPlan>, Refusal> {
    let static_build = merged.static_build.unwrap_or(false);

    if !static_build {
        if let Some(target) = &merged.target {
            return Err(Refusal::TargetRequiresStatic {
                got: target.clone(),
            });
        }
        match merged.allocator.unwrap_or_default() {
            AllocatorChoice::Auto | AllocatorChoice::System => return Ok(None),
            got @ (AllocatorChoice::Dlmalloc
            | AllocatorChoice::Talc
            | AllocatorChoice::Mimalloc) => {
                return Err(Refusal::AllocatorRequiresStatic { got });
            }
        }
    }

    let triple = match &merged.target {
        None => StaticTriple::default(),
        Some(t) => {
            StaticTriple::parse(t).ok_or_else(|| Refusal::UnknownStaticTarget { got: t.clone() })?
        }
    };

    let c_free = merged.c_free.unwrap_or(false);

    let allocator = match merged.allocator.unwrap_or_default() {
        // AUTO on a musl-static target resolves to dlmalloc: pure Rust,
        // clears the musl-malloc cliff.
        AllocatorChoice::Auto | AllocatorChoice::Dlmalloc => StaticAllocator::Dlmalloc,
        // mimalloc vendors and links C; it cannot exist in a C-free plan.
        // Refuse here rather than emit a manifest that would pull C.
        AllocatorChoice::Mimalloc if c_free => return Err(Refusal::MimallocUnderCfree),
        AllocatorChoice::Mimalloc => StaticAllocator::Mimalloc,
        AllocatorChoice::System => {
            if merged.allow_slow_allocator.unwrap_or(false) {
                StaticAllocator::System
            } else {
                return Err(Refusal::MuslMallocCliff);
            }
        }
        AllocatorChoice::Talc => return Err(Refusal::TalcRequiresArenaDesign),
    };

    // The C-free plan axis is wired end to end (flag → layer → plan →
    // preflight-skip), but the dependency-graph swaps that make the build
    // actually link no C are a follow-up. Until they land, honouring `--cfree`
    // would emit a C-carrying manifest while skipping the C-compiler preflight
    // — a build that lies about its promise and then fails at link time.
    // Refuse loudly instead. (The mimalloc-under-cfree conflict above is a more
    // specific, more actionable refusal, so it is reported first.)
    if c_free {
        return Err(Refusal::CfreeNotYetWired);
    }

    Ok(Some(StaticPlan {
        triple,
        allocator,
        c_free,
    }))
}

/// Toolchain preflight for a resolved static plan.
///
/// The target's std must be installed and — unless the plan is C-free —
/// a C compiler that targets the plan's triple must be reachable, because the
/// default emitted dependency graph carries C compile units (`zstd`, `ring`).
/// Runs before the compile pipeline so the failure is an actionable refusal,
/// not a cryptic cargo error minutes later.
///
/// The probed compiler names are derived from the plan's triple
/// ([`StaticTriple::cc_candidates`]) so an aarch64 static build does not probe
/// x86_64-only compiler names (which would spuriously pass or fail depending
/// on host tooling). Under a C-free plan the C-compiler check is skipped
/// entirely — there is no C unit to compile.
///
/// Fail-soft when `rustup` itself is absent (non-rustup toolchains may well
/// have the target); the C-compiler check honours the standard `CC_*` /
/// `TARGET_CC` overrides before probing `PATH`.
///
/// # Errors
/// [`Refusal::TargetNotInstalled`] / [`Refusal::MuslCCompilerMissing`].
pub fn preflight(plan: &StaticPlan) -> Result<(), Refusal> {
    let installed = rustup_installed_targets();
    let cc_present = std::env::var_os(format!("CC_{}", plan.triple.as_str().replace('-', "_")))
        .is_some()
        || std::env::var_os("TARGET_CC").is_some()
        || plan
            .triple
            .cc_candidates()
            .iter()
            .any(|name| binary_on_path(name));
    preflight_with(plan, installed.as_deref(), cc_present)
}

/// [`preflight`]'s pure core.
///
/// The toolchain observations are injected as data so the gates are
/// unit-testable without a rustup installation. `installed` = `None` means
/// rustup is absent (fail-soft: skip the check). `c_cc_present` is ignored
/// when the plan is C-free — the C units it would satisfy are not in the
/// graph.
///
/// # Errors
/// [`Refusal::TargetNotInstalled`] / [`Refusal::MuslCCompilerMissing`].
pub fn preflight_with(
    plan: &StaticPlan,
    installed: Option<&[String]>,
    c_cc_present: bool,
) -> Result<(), Refusal> {
    if let Some(targets) = installed
        && !targets.iter().any(|t| t == plan.triple.as_str())
    {
        return Err(Refusal::TargetNotInstalled {
            triple: plan.triple.as_str(),
        });
    }
    if !plan.c_free && !c_cc_present {
        return Err(Refusal::MuslCCompilerMissing {
            triple: plan.triple.as_str(),
        });
    }
    Ok(())
}

/// The installed rustup targets, or `None` when `rustup` cannot be run
/// (absent or failing — both fail-soft).
fn rustup_installed_targets() -> Option<Vec<String>> {
    let output = std::process::Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(text.lines().map(str::to_owned).collect())
}

/// Whether an executable named `name` exists on `PATH`.
fn binary_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

#[cfg(test)]
mod tests {
    use super::{
        AllocatorChoice, Refusal, StaticRequestLayer, parse_bool, preflight_with, resolve,
    };
    use ipe_backend_rust::static_build::{StaticAllocator, StaticPlan, StaticTriple};

    fn layer(
        static_build: Option<bool>,
        target: Option<&str>,
        allocator: Option<AllocatorChoice>,
        ack: Option<bool>,
    ) -> StaticRequestLayer {
        StaticRequestLayer {
            static_build,
            target: target.map(str::to_owned),
            allocator,
            allow_slow_allocator: ack,
            c_free: None,
        }
    }

    #[test]
    fn allocator_parse_is_closed() {
        assert_eq!(AllocatorChoice::parse("auto"), Ok(AllocatorChoice::Auto));
        assert_eq!(
            AllocatorChoice::parse("dlmalloc"),
            Ok(AllocatorChoice::Dlmalloc)
        );
        assert!(matches!(
            AllocatorChoice::parse("jemalloc"),
            Err(Refusal::UnknownAllocator { .. })
        ));
        assert!(matches!(
            AllocatorChoice::parse("snmalloc"),
            Err(Refusal::UnknownAllocator { .. })
        ));
        assert!(matches!(
            AllocatorChoice::parse(""),
            Err(Refusal::UnknownAllocator { .. })
        ));
    }

    #[test]
    fn no_request_resolves_to_dynamic() {
        assert_eq!(resolve(&StaticRequestLayer::default()), Ok(None));
    }

    #[test]
    fn auto_static_picks_musl_dlmalloc() {
        let plan = resolve(&layer(Some(true), None, None, None));
        assert_eq!(
            plan,
            Ok(Some(StaticPlan {
                triple: StaticTriple::X8664LinuxMusl,
                allocator: StaticAllocator::Dlmalloc,
                c_free: false,
            }))
        );
    }

    #[test]
    fn explicit_musl_target_accepted_unknown_target_refused() {
        assert!(
            resolve(&layer(
                Some(true),
                Some("x86_64-unknown-linux-musl"),
                None,
                None
            ))
            .is_ok()
        );
        assert!(
            resolve(&layer(
                Some(true),
                Some("aarch64-unknown-linux-musl"),
                None,
                None
            ))
            .is_ok()
        );
        assert!(matches!(
            resolve(&layer(
                Some(true),
                Some("riscv64gc-unknown-linux-musl"),
                None,
                None
            )),
            Err(Refusal::UnknownStaticTarget { .. })
        ));
        assert!(matches!(
            resolve(&layer(Some(true), Some("x86_64-apple-darwin"), None, None)),
            Err(Refusal::UnknownStaticTarget { .. })
        ));
    }

    #[test]
    fn target_without_static_is_refused() {
        assert!(matches!(
            resolve(&layer(None, Some("x86_64-unknown-linux-musl"), None, None)),
            Err(Refusal::TargetRequiresStatic { .. })
        ));
    }

    #[test]
    fn nondefault_allocator_without_static_is_refused() {
        assert!(matches!(
            resolve(&layer(None, None, Some(AllocatorChoice::Mimalloc), None)),
            Err(Refusal::AllocatorRequiresStatic { .. })
        ));
        // auto/system are the dynamic identity — no refusal.
        assert_eq!(
            resolve(&layer(None, None, Some(AllocatorChoice::System), None)),
            Ok(None)
        );
    }

    #[test]
    fn system_on_musl_needs_the_two_key_acknowledgment() {
        assert_eq!(
            resolve(&layer(
                Some(true),
                None,
                Some(AllocatorChoice::System),
                None
            )),
            Err(Refusal::MuslMallocCliff)
        );
        assert_eq!(
            resolve(&layer(
                Some(true),
                None,
                Some(AllocatorChoice::System),
                Some(true)
            )),
            Ok(Some(StaticPlan {
                triple: StaticTriple::X8664LinuxMusl,
                allocator: StaticAllocator::System,
                c_free: false,
            }))
        );
    }

    #[test]
    fn talc_is_refused_until_the_arena_design_lands() {
        assert_eq!(
            resolve(&layer(Some(true), None, Some(AllocatorChoice::Talc), None)),
            Err(Refusal::TalcRequiresArenaDesign)
        );
    }

    #[test]
    fn mimalloc_optin_resolves() {
        assert_eq!(
            resolve(&layer(
                Some(true),
                None,
                Some(AllocatorChoice::Mimalloc),
                None
            )),
            Ok(Some(StaticPlan {
                triple: StaticTriple::X8664LinuxMusl,
                allocator: StaticAllocator::Mimalloc,
                c_free: false,
            }))
        );
    }

    #[test]
    fn cfree_is_refused_until_the_dep_swaps_land() {
        // The plan axis is wired, but honouring --cfree before the pure-Rust
        // dependency swaps land would emit a C-carrying build that skips the
        // C-compiler preflight — refused loudly, never silently degraded.
        assert_eq!(
            resolve(&StaticRequestLayer {
                static_build: Some(true),
                c_free: Some(true),
                ..StaticRequestLayer::default()
            }),
            Err(Refusal::CfreeNotYetWired)
        );
        // mimalloc under --cfree is the more specific conflict, reported first.
        assert_eq!(
            resolve(&StaticRequestLayer {
                static_build: Some(true),
                allocator: Some(AllocatorChoice::Mimalloc),
                c_free: Some(true),
                ..StaticRequestLayer::default()
            }),
            Err(Refusal::MimallocUnderCfree)
        );
    }

    #[test]
    fn precedence_cli_beats_env_beats_toml() {
        let cli = layer(None, None, Some(AllocatorChoice::Mimalloc), None);
        let env = layer(Some(true), None, Some(AllocatorChoice::System), None);
        let toml = layer(
            Some(false),
            None,
            Some(AllocatorChoice::Dlmalloc),
            Some(true),
        );
        let merged = cli.or(env).or(toml);
        assert_eq!(
            merged,
            layer(
                Some(true), // env (CLI unset)
                None,
                Some(AllocatorChoice::Mimalloc), // CLI
                Some(true),                      // toml (others unset)
            )
        );
    }

    #[test]
    fn bool_values_parse_closed() {
        assert_eq!(parse_bool("IPE_STATIC", "1"), Ok(true));
        assert_eq!(parse_bool("IPE_STATIC", "true"), Ok(true));
        assert_eq!(parse_bool("IPE_STATIC", "0"), Ok(false));
        assert_eq!(parse_bool("IPE_STATIC", "false"), Ok(false));
        assert!(matches!(
            parse_bool("IPE_STATIC", "yes"),
            Err(Refusal::InvalidBool { .. })
        ));
    }

    #[test]
    fn preflight_gates_target_and_cc() {
        let plan = StaticPlan {
            triple: StaticTriple::X8664LinuxMusl,
            allocator: StaticAllocator::Dlmalloc,
            c_free: false,
        };
        let musl = "x86_64-unknown-linux-musl".to_owned();
        let gnu = "x86_64-unknown-linux-gnu".to_owned();
        assert!(preflight_with(&plan, Some(&[gnu.clone(), musl.clone()]), true).is_ok());
        assert!(matches!(
            preflight_with(&plan, Some(&[gnu]), true),
            Err(Refusal::TargetNotInstalled { .. })
        ));
        assert!(matches!(
            preflight_with(&plan, Some(&[musl]), false),
            Err(Refusal::MuslCCompilerMissing { .. })
        ));
        // rustup absent → fail-soft on the target check, cc still gated.
        assert!(preflight_with(&plan, None, true).is_ok());
        assert!(preflight_with(&plan, None, false).is_err());
    }

    #[test]
    fn preflight_skips_cc_probe_under_cfree() {
        // A C-free plan has no C unit to compile, so a missing C compiler is
        // not a refusal — only the target-installed check still applies.
        let plan = StaticPlan {
            triple: StaticTriple::Aarch64LinuxMusl,
            allocator: StaticAllocator::Dlmalloc,
            c_free: true,
        };
        let musl = "aarch64-unknown-linux-musl".to_owned();
        assert!(preflight_with(&plan, Some(&[musl]), false).is_ok());
        assert!(preflight_with(&plan, None, false).is_ok());
        // The target check is still enforced under C-free.
        assert!(matches!(
            preflight_with(
                &plan,
                Some(&["x86_64-unknown-linux-musl".to_owned()]),
                false
            ),
            Err(Refusal::TargetNotInstalled { .. })
        ));
    }
}
