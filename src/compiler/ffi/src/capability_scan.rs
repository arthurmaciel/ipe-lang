//! Static capability inference over author-supplied wrapper Rust — the honesty
//! check on a `[rust.wrapper]` capability declaration (FFI Tier 2, spec §5).
//!
//! It lexes wrapper source with `proc-macro2`, the SAME token model
//! `tools/panic-scan` uses, so a capability-bearing path named inside a string
//! literal or a comment is invisible (a string is one opaque `Literal`; comments
//! are dropped) and one split across lines (`std::\nnet::TcpStream`) is still
//! found. That closes the trivial evasions a text scan would miss.
//!
//! # What this PROPOSES, and what enforces
//!
//! This scan is **imprecise by design** — it cannot see through a macro that
//! expands to `std::net`, a re-export (`use std::fs as f; f::read`), a capability
//! reached only through a dependency crate, or a call behind a type alias. It
//! therefore only *proposes* a coarse over-approximation and, more importantly,
//! answers one load-bearing yes/no: does this wrapper reach ANY
//! capability whose effects Ipê cannot yet contain at run? Because there is no
//! runtime sandbox around the emitted app in this release (the wrapper's Rust
//! runs with the user's full ambient authority at `ipe run`), a wrapper that
//! reaches such a capability CANNOT be soundly admitted — the install refuses it
//! rather than admit an unenforced capability. See [`ScanOutcome`].
//!
//! The scan is deliberately biased toward FALSE POSITIVES: over-flagging refuses
//! a wrapper that might have been safe (an annoyance the author resolves by
//! narrowing the wrapper), whereas a false negative would admit an unconstrained
//! capability. Every ambiguous or unenumerable construct (`extern`, `#[link]`,
//! `libc::`, `include!`, `#[path]`, a source that does not lex) is treated as an
//! **opaque** refuse trigger, never as "no capability found".

use std::collections::BTreeSet;

pub use ipe_kernels::Capability;
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use std::str::FromStr;

/// A capability-bearing standard-library path root and the [`Capability`] it
/// maps to. Matched as a `first :: second` token pair (`std :: net`,
/// `process :: Command`), so a path split across lines is still caught.
struct PathRule {
    /// The leading identifier (`std`, `process`, `fs`, …).
    first: &'static str,
    /// The identifier that must immediately follow `::`.
    second: &'static str,
    /// The capability this path root exercises.
    cap: Capability,
}

/// The capability-bearing `first::second` path roots. Coarse: `std::net`
/// anything is [`Capability::Network`], `std::fs` anything is
/// [`Capability::Filesystem`]. The list also covers the common bare roots a
/// wrapper reaches after `use std::net::*` (`TcpStream`, `File`, `Command`).
const PATH_RULES: &[PathRule] = &[
    // Network.
    PathRule { first: "std", second: "net", cap: Capability::Network },
    PathRule { first: "net", second: "TcpStream", cap: Capability::Network },
    PathRule { first: "net", second: "TcpListener", cap: Capability::Network },
    PathRule { first: "net", second: "UdpSocket", cap: Capability::Network },
    // Filesystem.
    PathRule { first: "std", second: "fs", cap: Capability::Filesystem },
    PathRule { first: "fs", second: "File", cap: Capability::Filesystem },
    PathRule { first: "fs", second: "OpenOptions", cap: Capability::Filesystem },
    // Subprocess.
    PathRule { first: "std", second: "process", cap: Capability::Subprocess },
    PathRule { first: "process", second: "Command", cap: Capability::Subprocess },
    // Environment.
    PathRule { first: "std", second: "env", cap: Capability::Env },
    // Clock.
    PathRule { first: "std", second: "time", cap: Capability::Clock },
    PathRule { first: "time", second: "Instant", cap: Capability::Clock },
    PathRule { first: "time", second: "SystemTime", cap: Capability::Clock },
];

/// Bare capability-bearing type/const identifiers a wrapper reaches after a
/// glob or explicit `use` (`use std::net::TcpStream; … TcpStream::connect`).
/// These are ONE identifier, so they are matched by name alone — imprecise (a
/// user type coincidentally named `File` would over-flag), which is the safe
/// direction: over-flagging refuses, it never admits.
const BARE_IDENTS: &[(&str, Capability)] = &[
    ("TcpStream", Capability::Network),
    ("TcpListener", Capability::Network),
    ("UdpSocket", Capability::Network),
    ("OpenOptions", Capability::Filesystem),
];

/// The set of capabilities whose runtime effects Ipê CANNOT contain in this
/// release — there is no sandbox around the emitted app at `ipe run`, so a
/// wrapper reaching one of these runs it with the user's full ambient
/// authority. A wrapper that declares or is inferred to touch any of these is
/// refused at install: enforcement is infeasible, so the capability is not
/// admitted unenforced (spec §5, "refuse rather than admit unenforced").
///
/// [`Capability::Clock`] and [`Capability::Random`] are deliberately absent:
/// they are non-determinism, not exfiltration, and carry no isolation surface —
/// admitting them unenforced leaks no authority. Every other axis stays here
/// until a runtime jail exists to scope it.
#[must_use]
pub fn is_runtime_unenforceable(cap: Capability) -> bool {
    match cap {
        Capability::Network
        | Capability::Filesystem
        | Capability::Database
        | Capability::Env
        | Capability::Subprocess
        | Capability::NativeFfi => true,
        Capability::Clock | Capability::Random => false,
    }
}

/// Why a wrapper's source cannot be admitted from the scan alone — an OPAQUE
/// construct the token scan cannot see past, so the proposed set can no longer
/// be trusted as complete. Each is a refuse trigger, never a "no capability".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opacity {
    /// A source file that does not lex as Rust tokens. A malformed (or
    /// deliberately un-lexable) file must refuse, never scan to an empty set.
    DoesNotLex {
        /// The file whose contents did not tokenise.
        file: String,
    },
    /// An `extern` block or `#[link]`/`libc::` reference — native FFI the scan
    /// is blind past. Its presence is exactly [`Capability::NativeFfi`]: the
    /// wrapper crosses into opaque native code whose effects cannot be inferred.
    NativeFfi {
        /// The file the construct was found in.
        file: String,
        /// The 1-based line.
        line: usize,
    },
    /// An `include!` or `#[path = "…"]` module — the scan cannot enumerate the
    /// source it pulls in, so the file set is not closed and a capability could
    /// hide in the unscanned text.
    UnenumerableModule {
        /// The file the construct was found in.
        file: String,
        /// The 1-based line.
        line: usize,
        /// A short label (`include!`, `#[path]`).
        construct: &'static str,
    },
}

impl std::fmt::Display for Opacity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DoesNotLex { file } => write!(
                f,
                "`{file}` does not lex as Rust — its capabilities cannot be scanned, so the \
                 wrapper cannot be admitted (fix the source or narrow the wrapper)"
            ),
            Self::NativeFfi { file, line } => write!(
                f,
                "{file}:{line}: native FFI (`extern` / `#[link]` / `libc`) — the wrapper crosses \
                 into opaque native code whose effects cannot be inferred or contained"
            ),
            Self::UnenumerableModule {
                file,
                line,
                construct,
            } => write!(
                f,
                "{file}:{line}: `{construct}` pulls in source the scan cannot enumerate, so a \
                 hidden capability cannot be ruled out"
            ),
        }
    }
}

/// The outcome of scanning ONE wrapper source file (or unioned across files):
/// the coarse proposed capability set plus every opacity trigger found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanOutcome {
    /// The coarse over-approximating capability set the token scan proposes.
    /// Imprecise — a lower bound on the truth for the tokens it can see, an
    /// upper bound where a bare-ident match over-flags. Used for the diagnostic
    /// and the reconcile, never as the sole enforcement.
    pub proposed: BTreeSet<Capability>,
    /// Opaque constructs that make the scan untrustworthy as a completeness
    /// claim. A non-empty list is a hard refuse trigger regardless of
    /// `proposed`.
    pub opacities: Vec<Opacity>,
}

impl ScanOutcome {
    /// Fold another file's outcome into this one (the multi-file union).
    fn absorb(&mut self, other: ScanOutcome) {
        self.proposed.extend(other.proposed);
        self.opacities.extend(other.opacities);
    }

    /// The proposed capabilities that Ipê cannot enforce at run — the subset of
    /// [`Self::proposed`] for which admission would be unenforced.
    #[must_use]
    pub fn unenforceable(&self) -> BTreeSet<Capability> {
        self.proposed
            .iter()
            .copied()
            .filter(|&c| is_runtime_unenforceable(c))
            .collect()
    }

    /// Whether the scan alone forces a refuse: it found an opaque construct, or
    /// it proposed a capability Ipê cannot yet contain at run.
    #[must_use]
    pub fn must_refuse(&self) -> bool {
        !self.opacities.is_empty() || !self.unenforceable().is_empty()
    }
}

/// Scan one wrapper source file for capability-bearing paths and opacity
/// triggers. A file that does not lex yields a [`Opacity::DoesNotLex`] — NEVER
/// an empty proposed set, which would silently under-propose.
///
/// `file` is a display label used only in diagnostics; `src` is the file text.
#[must_use]
pub fn scan_source(file: &str, src: &str) -> ScanOutcome {
    let Ok(ts) = TokenStream::from_str(src) else {
        return ScanOutcome {
            proposed: BTreeSet::new(),
            opacities: vec![Opacity::DoesNotLex {
                file: file.to_owned(),
            }],
        };
    };
    let mut outcome = ScanOutcome::default();
    scan_stream(file, ts, &mut outcome);
    outcome
}

/// Union the scan over a whole set of `(file, source)` pairs — the wrapper's
/// every `.rs` file. The proposed set is the union; any file's opacity trigger
/// makes the whole wrapper refuse.
#[must_use]
pub fn scan_sources<'a>(sources: impl IntoIterator<Item = (&'a str, &'a str)>) -> ScanOutcome {
    let mut acc = ScanOutcome::default();
    for (file, src) in sources {
        acc.absorb(scan_source(file, src));
    }
    acc
}

/// Walk a token stream, recording capability paths, bare idents, and opacity
/// triggers. Recurses into every delimited group (fn bodies, `impl` blocks) so
/// a construct nowhere near the top level is still seen.
fn scan_stream(file: &str, ts: TokenStream, out: &mut ScanOutcome) {
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    // A `#` immediately followed by a `[ … ]` group whose text is a `path = …`
    // attribute is an unenumerable-module trigger.
    for i in 0..toks.len() {
        match &toks[i] {
            // `extern` keyword — an `extern` block or `extern "C" fn` is native
            // FFI. Bare `extern crate` is NOT native FFI, so require that the
            // next non-string token is not `crate`.
            TokenTree::Ident(id) if *id == "extern" => {
                let next_is_crate = matches!(toks.get(i + 1), Some(TokenTree::Ident(n)) if *n == "crate");
                if !next_is_crate {
                    out.opacities.push(Opacity::NativeFfi {
                        file: file.to_owned(),
                        line: id.span().start().line,
                    });
                    out.proposed.insert(Capability::NativeFfi);
                }
            }
            // `libc` path root — the canonical raw-syscall crate.
            TokenTree::Ident(id) if *id == "libc" => {
                if let Some(TokenTree::Punct(p)) = toks.get(i + 1) {
                    if p.as_char() == ':' {
                        out.opacities.push(Opacity::NativeFfi {
                            file: file.to_owned(),
                            line: id.span().start().line,
                        });
                        out.proposed.insert(Capability::NativeFfi);
                    }
                }
            }
            // `include!` / `include_str!`-style macro pulling in unscanned text.
            // Only `include!` and `include_bytes!`/`include_str!` matter for
            // hiding *code*; `include!` is the code one. Flag `include`.
            TokenTree::Ident(id) if *id == "include" => {
                if let Some(TokenTree::Punct(bang)) = toks.get(i + 1) {
                    if bang.as_char() == '!' {
                        out.opacities.push(Opacity::UnenumerableModule {
                            file: file.to_owned(),
                            line: id.span().start().line,
                            construct: "include!",
                        });
                    }
                }
            }
            // `#[path = "…"]` attribute — a module whose source is elsewhere.
            TokenTree::Punct(p) if p.as_char() == '#' => {
                if let Some(TokenTree::Group(g)) = toks.get(i + 1) {
                    if g.delimiter() == Delimiter::Bracket {
                        let inner: String = g
                            .stream()
                            .to_string()
                            .chars()
                            .filter(|c| !c.is_whitespace())
                            .collect();
                        if inner.starts_with("path=") || inner.starts_with("link") {
                            let (construct, cap): (&'static str, Option<Capability>) =
                                if inner.starts_with("link") {
                                    // `#[link(...)]` links a native library.
                                    out.proposed.insert(Capability::NativeFfi);
                                    ("#[link]", Some(Capability::NativeFfi))
                                } else {
                                    ("#[path]", None)
                                };
                            let _ = cap;
                            out.opacities.push(Opacity::UnenumerableModule {
                                file: file.to_owned(),
                                line: p.span().start().line,
                                construct,
                            });
                        }
                    }
                }
            }
            // A capability-bearing `first :: second` path pair.
            TokenTree::Ident(id) => {
                let name = id.to_string();
                // Bare capability idents (matched by name alone).
                for (bare, cap) in BARE_IDENTS {
                    if name == *bare {
                        out.proposed.insert(*cap);
                    }
                }
                // `first :: second` — id, `:`, `:`, second-ident.
                if let (
                    Some(TokenTree::Punct(c1)),
                    Some(TokenTree::Punct(c2)),
                    Some(TokenTree::Ident(second)),
                ) = (toks.get(i + 1), toks.get(i + 2), toks.get(i + 3))
                {
                    if c1.as_char() == ':' && c2.as_char() == ':' {
                        let second_name = second.to_string();
                        for rule in PATH_RULES {
                            if name == rule.first && second_name == rule.second {
                                out.proposed.insert(rule.cap);
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        if let TokenTree::Group(g) = &toks[i] {
            scan_stream(file, g.stream(), out);
        }
    }
}

/// The install-time reconcile verdict for a wrapper: either admissible (its
/// effects are all containable, and the declaration covers what the scan sees)
/// or refused, carrying every reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The wrapper may be installed. `declared` is echoed for the consent
    /// summary; `native_ffi` flags whether the author consented to
    /// [`Capability::NativeFfi`] (surfaced loudly) — always `false` here in this
    /// release, since native-ffi is unenforceable and thus refused, but the
    /// field keeps the consent surface explicit.
    Admit {
        /// The author's declared set (the consent surface).
        declared: BTreeSet<Capability>,
    },
    /// The wrapper is refused. Every reason is listed so the author sees the
    /// whole picture, not just the first blocker.
    Refuse {
        /// The reasons the wrapper cannot be admitted.
        reasons: Vec<RefuseReason>,
        /// The scan's proposed set, surfaced so the author can reconcile their
        /// declaration against what the wrapper actually reaches.
        proposed: BTreeSet<Capability>,
    },
}

/// One reason a wrapper is refused at install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefuseReason {
    /// The author DECLARED a capability Ipê cannot enforce at run (no runtime
    /// jail exists in this release). Refuse rather than admit unenforced.
    DeclaredUnenforceable {
        /// The declared-but-unenforceable capability.
        cap: Capability,
    },
    /// The scan INFERRED the wrapper reaches a capability Ipê cannot enforce at
    /// run — whether or not it was declared. The wrapper's runtime effects are
    /// uncontained, so it cannot be admitted.
    InferredUnenforceable {
        /// The inferred-but-unenforceable capability.
        cap: Capability,
    },
    /// The scan hit an opaque construct (native FFI, an unenumerable module, a
    /// non-lexing file) that makes the inference untrustworthy as a completeness
    /// claim.
    Opaque {
        /// The opacity trigger.
        opacity: Opacity,
    },
    /// The wrapper crate declares a non-`std` Cargo dependency. A dependency's
    /// capabilities live in source the scan never opens (`reqwest::get` is
    /// Network the wrapper's own `.rs` never names), so a wrapper with external
    /// deps is opaque and refused in this release.
    NonStdDependency {
        /// The dependency name.
        name: String,
    },
}

impl std::fmt::Display for RefuseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeclaredUnenforceable { cap } => write!(
                f,
                "declares `{}`, which has no runtime enforcement in this release — a wrapper's \
                 Rust runs unsandboxed at `ipe run`, so this capability cannot be contained yet",
                cap.as_str()
            ),
            Self::InferredUnenforceable { cap } => write!(
                f,
                "reaches `{}` (inferred from its source), which has no runtime enforcement in \
                 this release — its effects would be uncontained at run",
                cap.as_str()
            ),
            Self::Opaque { opacity } => write!(f, "{opacity}"),
            Self::NonStdDependency { name } => write!(
                f,
                "depends on `{name}` — a dependency's capabilities live in source this scan \
                 cannot see, so a wrapper with external dependencies cannot be admitted yet"
            ),
        }
    }
}

/// Reconcile a wrapper's declared capability set against the scan of its source,
/// and decide admissibility under the current enforcement reality.
///
/// The rule (FFI Tier 2 §5, hardened by the security review): there is no
/// runtime sandbox around the emitted app in this release, so any capability on
/// a runtime-enforced axis ([`is_runtime_unenforceable`]) is *infeasible to
/// enforce* and MUST NOT be admitted — whether the author declared it or the
/// scan inferred it. Only wrappers whose declared AND inferred sets are confined
/// to the containable axes ({clock, random}, or empty) install. `non_std_deps`
/// is the wrapper crate's non-`std` dependency names — any is opaque and
/// refuses.
///
/// This is strictly a security improvement over the prior state, where such a
/// wrapper installed with NO gate at all: it turns "silently unconstrained" into
/// "refused until the runtime jail lands".
#[must_use]
pub fn reconcile(
    declared: &BTreeSet<Capability>,
    scan: &ScanOutcome,
    non_std_deps: &[String],
) -> Verdict {
    let mut reasons = Vec::new();

    // A declared unenforceable capability: the author's own claim names an
    // effect we cannot contain.
    for &cap in declared {
        if is_runtime_unenforceable(cap) {
            reasons.push(RefuseReason::DeclaredUnenforceable { cap });
        }
    }
    // An inferred unenforceable capability the declaration did NOT already
    // account for (avoid a duplicate reason for the same axis).
    for cap in scan.unenforceable() {
        if !declared.contains(&cap) {
            reasons.push(RefuseReason::InferredUnenforceable { cap });
        }
    }
    // Opaque constructs — the scan cannot vouch for completeness.
    for opacity in &scan.opacities {
        reasons.push(RefuseReason::Opaque {
            opacity: opacity.clone(),
        });
    }
    // Non-std dependencies — capabilities can hide in a dependency's source.
    for name in non_std_deps {
        reasons.push(RefuseReason::NonStdDependency {
            name: name.clone(),
        });
    }

    if reasons.is_empty() {
        Verdict::Admit {
            declared: declared.clone(),
        }
    } else {
        Verdict::Refuse {
            reasons,
            proposed: scan.proposed.clone(),
        }
    }
}

/// Parse a raw `[rust.wrapper] capabilities = [...]` list into a typed set,
/// fail-closed on an unknown name. A typo'd capability is a LOUD rejection, never
/// a capability the reconcile then silently fails to compare.
///
/// # Errors
///
/// The offending token, when a name is not one of the closed capability
/// vocabulary (`Capability::from_str`'s [`ipe_kernels::UnknownCapability`]).
pub fn parse_declared(names: &[String]) -> Result<BTreeSet<Capability>, ipe_kernels::UnknownCapability> {
    let mut set = BTreeSet::new();
    for name in names {
        set.insert(Capability::from_str(name)?);
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(o: &ScanOutcome) -> Vec<&'static str> {
        o.proposed.iter().map(|c| c.as_str()).collect()
    }

    #[test]
    fn a_std_net_path_proposes_network() {
        let src = "pub fn f() { let _ = std::net::TcpStream::connect(\"x\"); }";
        let o = scan_source("lib.rs", src);
        assert!(o.proposed.contains(&Capability::Network), "{:?}", caps(&o));
        assert!(o.opacities.is_empty());
        assert!(o.must_refuse(), "network is unenforceable at run");
    }

    #[test]
    fn std_fs_and_process_and_env_are_each_proposed() {
        let src = "fn f() { std::fs::read(\"a\"); std::process::Command::new(\"x\"); std::env::var(\"Y\"); }";
        let o = scan_source("lib.rs", src);
        assert!(o.proposed.contains(&Capability::Filesystem));
        assert!(o.proposed.contains(&Capability::Subprocess));
        assert!(o.proposed.contains(&Capability::Env));
    }

    #[test]
    fn a_path_split_across_lines_is_still_found() {
        // The token model does not care about the newline between `std` and `::`.
        let src = "fn f() { let _ = std\n  ::\n  net::UdpSocket::bind(\"x\"); }";
        let o = scan_source("lib.rs", src);
        assert!(o.proposed.contains(&Capability::Network), "{:?}", caps(&o));
    }

    #[test]
    fn a_capability_named_in_a_string_or_comment_is_invisible() {
        // `std::net` appears only inside a string literal and a comment — the
        // token model never sees it as a path, so no false positive.
        let src = "fn f() -> &'static str {\n    // std::net::TcpStream is not used here\n    \"std::net::TcpStream\"\n}";
        let o = scan_source("lib.rs", src);
        assert!(
            o.proposed.is_empty(),
            "a string/comment mention must not propose: {:?}",
            caps(&o)
        );
        assert!(!o.must_refuse());
    }

    #[test]
    fn an_extern_block_is_native_ffi_and_refuses() {
        let src = "extern \"C\" { fn getpid() -> i32; }";
        let o = scan_source("lib.rs", src);
        assert!(o.proposed.contains(&Capability::NativeFfi));
        assert!(
            matches!(o.opacities.first(), Some(Opacity::NativeFfi { .. })),
            "{:?}",
            o.opacities
        );
        assert!(o.must_refuse());
    }

    #[test]
    fn an_extern_crate_is_not_native_ffi() {
        // `extern crate foo;` is a 2015-edition import, not an FFI block.
        let src = "extern crate serde; pub fn f() -> i32 { 1 }";
        let o = scan_source("lib.rs", src);
        assert!(
            !o.proposed.contains(&Capability::NativeFfi),
            "extern crate must not trip native-ffi: {:?}",
            o.opacities
        );
        assert!(o.opacities.is_empty());
    }

    #[test]
    fn a_libc_reference_is_native_ffi() {
        let src = "fn f() { unsafe { libc::exit(0); } }";
        let o = scan_source("lib.rs", src);
        assert!(o.proposed.contains(&Capability::NativeFfi));
        assert!(o.must_refuse());
    }

    #[test]
    fn an_include_macro_is_an_unenumerable_module() {
        let src = "include!(\"generated.rs\"); pub fn f() -> i32 { 1 }";
        let o = scan_source("lib.rs", src);
        assert!(
            matches!(
                o.opacities.first(),
                Some(Opacity::UnenumerableModule { construct: "include!", .. })
            ),
            "{:?}",
            o.opacities
        );
        assert!(o.must_refuse());
    }

    #[test]
    fn a_path_attribute_is_an_unenumerable_module() {
        let src = "#[path = \"other.rs\"]\nmod other;\npub fn f() -> i32 { 1 }";
        let o = scan_source("lib.rs", src);
        assert!(
            matches!(
                o.opacities.first(),
                Some(Opacity::UnenumerableModule { construct: "#[path]", .. })
            ),
            "{:?}",
            o.opacities
        );
        assert!(o.must_refuse());
    }

    #[test]
    fn a_non_lexing_source_refuses_never_proposes_empty() {
        // An unbalanced delimiter does not tokenise — must be DoesNotLex, not a
        // silent empty proposed set (the fail-closed rule on the lex path).
        let src = "pub fn f( { ) unbalanced";
        let o = scan_source("evil.rs", src);
        assert!(o.proposed.is_empty());
        assert!(
            matches!(o.opacities.first(), Some(Opacity::DoesNotLex { .. })),
            "{:?}",
            o.opacities
        );
        assert!(o.must_refuse(), "a non-lexing file must force a refuse");
    }

    #[test]
    fn a_pure_compute_wrapper_proposes_nothing_and_installs() {
        let src = "pub struct Engine { seed: i64 }\n\
                   pub fn make(seed: i64) -> Engine { Engine { seed } }\n\
                   pub fn describe(e: Engine) -> String { format!(\"engine<{}>\", e.seed) }";
        let o = scan_source("lib.rs", src);
        assert!(o.proposed.is_empty(), "{:?}", caps(&o));
        assert!(!o.must_refuse(), "a pure wrapper must be admissible");
    }

    #[test]
    fn clock_and_random_are_proposed_but_do_not_force_refuse() {
        // `std::time` proposes Clock, which is NOT runtime-unenforceable — a
        // pure clock read leaks no authority, so it does not force a refuse.
        let src = "fn f() { let _ = std::time::Instant::now(); }";
        let o = scan_source("lib.rs", src);
        assert!(o.proposed.contains(&Capability::Clock));
        assert!(
            !o.must_refuse(),
            "clock is non-determinism, not exfiltration — admissible unenforced"
        );
    }

    #[test]
    fn scan_sources_unions_across_files() {
        let a = ("a.rs", "fn f() { std::fs::read(\"x\"); }");
        let b = ("b.rs", "fn g() { std::net::TcpStream::connect(\"y\"); }");
        let o = scan_sources([a, b]);
        assert!(o.proposed.contains(&Capability::Filesystem));
        assert!(o.proposed.contains(&Capability::Network));
    }

    #[test]
    fn parse_declared_types_the_set_and_rejects_an_unknown_name() {
        let ok = parse_declared(&["network".to_owned(), "filesystem".to_owned()]).expect("parses");
        assert!(ok.contains(&Capability::Network));
        assert!(ok.contains(&Capability::Filesystem));
        let err = parse_declared(&["filesytem".to_owned()]).unwrap_err();
        assert_eq!(err.0, "filesytem");
    }

    fn set(caps: &[Capability]) -> BTreeSet<Capability> {
        caps.iter().copied().collect()
    }

    #[test]
    fn a_pure_wrapper_with_no_declaration_is_admitted() {
        let scan = scan_source("lib.rs", "pub fn make(x: i64) -> i64 { x + 1 }");
        let v = reconcile(&BTreeSet::new(), &scan, &[]);
        assert!(matches!(v, Verdict::Admit { .. }), "{v:?}");
    }

    #[test]
    fn a_declared_network_wrapper_is_refused_no_runtime_jail() {
        // Even an HONESTLY declared network wrapper cannot install: there is no
        // runtime sandbox to contain the socket at `ipe run`.
        let scan = scan_source("lib.rs", "pub fn f() { std::net::TcpStream::connect(\"x\"); }");
        let v = reconcile(&set(&[Capability::Network]), &scan, &[]);
        match v {
            Verdict::Refuse { reasons, .. } => {
                // Declared-unenforceable fires; inferred is de-duped since it was
                // declared.
                assert!(
                    reasons
                        .iter()
                        .any(|r| matches!(r, RefuseReason::DeclaredUnenforceable { cap } if *cap == Capability::Network)),
                    "{reasons:?}"
                );
                assert!(
                    !reasons
                        .iter()
                        .any(|r| matches!(r, RefuseReason::InferredUnenforceable { .. })),
                    "declared axis must not also fire inferred: {reasons:?}"
                );
            }
            Verdict::Admit { .. } => panic!("a network wrapper must be refused"),
        }
    }

    #[test]
    fn an_undeclared_but_inferred_network_wrapper_is_refused() {
        // The author declared nothing, but the scan finds a socket — the hidden
        // effect is refused, and the proposed set is surfaced.
        let scan = scan_source("lib.rs", "pub fn f() { std::net::TcpStream::connect(\"x\"); }");
        let v = reconcile(&BTreeSet::new(), &scan, &[]);
        match v {
            Verdict::Refuse { reasons, proposed } => {
                assert!(
                    reasons
                        .iter()
                        .any(|r| matches!(r, RefuseReason::InferredUnenforceable { cap } if *cap == Capability::Network)),
                    "{reasons:?}"
                );
                assert!(proposed.contains(&Capability::Network), "surfaces proposed");
            }
            Verdict::Admit { .. } => panic!("an inferred network wrapper must be refused"),
        }
    }

    #[test]
    fn a_clock_only_wrapper_is_admitted() {
        // Clock is non-determinism, not exfiltration — admissible even unenforced.
        let scan = scan_source("lib.rs", "pub fn f() -> u128 { std::time::Instant::now().elapsed().as_nanos() }");
        let v = reconcile(&set(&[Capability::Clock]), &scan, &[]);
        assert!(matches!(v, Verdict::Admit { .. }), "{v:?}");
    }

    #[test]
    fn a_non_std_dependency_refuses_even_a_pure_looking_wrapper() {
        let scan = scan_source("lib.rs", "pub fn f() -> i64 { reqwest_get() }");
        let v = reconcile(&BTreeSet::new(), &scan, &["reqwest".to_owned()]);
        match v {
            Verdict::Refuse { reasons, .. } => assert!(
                reasons
                    .iter()
                    .any(|r| matches!(r, RefuseReason::NonStdDependency { name } if name == "reqwest")),
                "{reasons:?}"
            ),
            Verdict::Admit { .. } => panic!("a wrapper with a non-std dep must be refused"),
        }
    }

    #[test]
    fn an_opaque_extern_wrapper_is_refused() {
        let scan = scan_source("lib.rs", "extern \"C\" { fn getpid() -> i32; }");
        let v = reconcile(&BTreeSet::new(), &scan, &[]);
        assert!(
            matches!(&v, Verdict::Refuse { reasons, .. } if reasons.iter().any(|r| matches!(r, RefuseReason::Opaque { .. }))),
            "{v:?}"
        );
    }

    #[test]
    fn is_runtime_unenforceable_partitions_the_vocabulary() {
        for cap in Capability::ALL {
            let unenf = is_runtime_unenforceable(*cap);
            match cap {
                Capability::Clock | Capability::Random => assert!(!unenf, "{cap:?}"),
                _ => assert!(unenf, "{cap:?} must be refused until a runtime jail exists"),
            }
        }
    }
}
