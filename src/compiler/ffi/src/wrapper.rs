//! The `[rust.wrapper]` manifest surface — author-supplied wrapper crates.
//!
//! A wrapper crate is a normal local Cargo crate the package author owns; Ipê
//! binds its inspected public symbols exactly as it binds a crates.io crate.
//! The author never mints a `ForeignCall`: the driver generates the interface
//! from the crate's inspected symbols, so this surface only names WHERE the
//! crate lives (a package-jailed relative path) and WHICH public symbols to
//! bind (validated identifiers).
//!
//! Everything untrusted passes a parse-don't-validate newtype at this boundary:
//! [`WrapperPath`] rejects an absolute path or a `..` escape at decode, each
//! `expose` entry becomes a [`RustIdent`], and each `capabilities` entry is
//! parsed into the closed [`Capability`] vocabulary — a typo'd capability is a
//! LOUD rejection at decode, never a raw string a later reconcile silently fails
//! to compare. The declaration is the author's *claim*; the install-time gate
//! reconciles it against the inferred set and enforces it (see
//! [`crate::capability_scan`]).

use std::collections::BTreeSet;

use ipe_kernels::Capability;

use crate::diag::{Diagnostic, SourceDefect};
use crate::naming::RustIdent;

/// A validated, package-jailed relative path to a local wrapper Cargo crate.
///
/// The ONLY form that can name a wrapper crate: a non-empty relative path with
/// no leading `/`, no `..` component, and every character inside the safe set
/// `[A-Za-z0-9._/-]`. A value of this type is, by existence, confined to the
/// package tree, so no path an author writes can reach a crate outside the
/// project or splice a metacharacter into an argv / a `path = "…"` TOML value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapperPath(String);

impl WrapperPath {
    /// Validate and wrap a wrapper-crate path relative to the package root.
    ///
    /// # Errors
    ///
    /// `IPE-F4411` when the path is empty, absolute, escapes the package via a
    /// `..` component, or carries a character outside `[A-Za-z0-9._/-]`.
    pub fn parse(s: &str) -> Result<Self, Diagnostic> {
        let reject = |defect: SourceDefect| Diagnostic::SourceRejected {
            source: s.to_owned(),
            defect,
        };
        if s.is_empty() || s.starts_with('/') {
            return Err(reject(SourceDefect::WrapperPathEscapes {
                got: s.to_owned(),
            }));
        }
        let charset_ok = s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'));
        if !charset_ok {
            return Err(reject(SourceDefect::WrapperPathCharsetIllegal {
                got: s.to_owned(),
            }));
        }
        // A `..` SEGMENT escapes the package; a dot inside a name (`a..b`,
        // `my.crate`) is a plain filename character and stays. Splitting on `/`
        // isolates the escape to a whole segment.
        if s.split('/').any(|seg| seg == "..") {
            return Err(reject(SourceDefect::WrapperPathEscapes {
                got: s.to_owned(),
            }));
        }
        Ok(Self(s.to_owned()))
    }

    /// The validated relative path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A fully-validated `[rust.wrapper]` declaration.
///
/// A value of this type carries a package-jailed [`WrapperPath`] and a
/// non-empty list of [`RustIdent`] symbols to bind. The `capabilities`
/// declaration is preserved verbatim for a later inference/enforcement gate but
/// is not consulted here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapperManifest {
    path: WrapperPath,
    expose: Vec<RustIdent>,
    /// The author's declared capability set, parsed into the closed
    /// [`Capability`] vocabulary at decode. An unknown name is refused here, so
    /// a value of this type can only carry real capabilities the reconcile gate
    /// can compare — an unenforceable capability can never hide behind a typo.
    capabilities: BTreeSet<Capability>,
}

impl WrapperManifest {
    /// Validate a raw `[rust.wrapper]` decode into the typed surface.
    ///
    /// `expose` and `capabilities` arrive as raw strings the manifest reader
    /// lifted verbatim; the path and every exposed symbol pass their gate here.
    ///
    /// # Errors
    ///
    /// The [`WrapperPath`] gate's `IPE-F4411`, an [`crate::diag::WireDefect`]
    /// for an ill-formed exposed identifier, [`SourceDefect::WrapperExposeEmpty`]
    /// when no symbols are exposed, or [`SourceDefect::WrapperCapabilityUnknown`]
    /// when a declared capability is not in the closed vocabulary.
    pub fn parse(
        path: &str,
        expose: &[String],
        capabilities: &[String],
    ) -> Result<Self, Diagnostic> {
        let path = WrapperPath::parse(path)?;
        if expose.is_empty() {
            return Err(Diagnostic::SourceRejected {
                source: path.as_str().to_owned(),
                defect: SourceDefect::WrapperExposeEmpty,
            });
        }
        let expose = expose
            .iter()
            .map(|s| RustIdent::parse(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|defect| Diagnostic::WireMalformed {
                context: format!("[rust.wrapper] path `{}`", path.as_str()),
                defect,
            })?;
        let capabilities =
            crate::capability_scan::parse_declared(capabilities).map_err(|unknown| {
                Diagnostic::SourceRejected {
                    source: path.as_str().to_owned(),
                    defect: SourceDefect::WrapperCapabilityUnknown { got: unknown.0 },
                }
            })?;
        Ok(Self {
            path,
            expose,
            capabilities,
        })
    }

    /// The package-jailed wrapper-crate path.
    #[must_use]
    pub const fn path(&self) -> &WrapperPath {
        &self.path
    }

    /// The validated symbols to bind.
    #[must_use]
    pub fn expose(&self) -> &[RustIdent] {
        &self.expose
    }

    /// The exposed symbols as plain strings — the shape the inspector's
    /// `--expose` argv consumes.
    #[must_use]
    pub fn expose_names(&self) -> Vec<String> {
        self.expose.iter().map(|i| i.as_str().to_owned()).collect()
    }

    /// The author's declared, typed capability set.
    #[must_use]
    pub const fn capabilities(&self) -> &BTreeSet<Capability> {
        &self.capabilities
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_and_exposed_symbols_parse() {
        let m = WrapperManifest::parse(
            "wrappers",
            &["make_engine".to_owned(), "Engine".to_owned()],
            &[],
        )
        .expect("parses");
        assert_eq!(m.path().as_str(), "wrappers");
        assert_eq!(m.expose_names(), ["make_engine", "Engine"]);
        assert!(m.capabilities().is_empty());
    }

    #[test]
    fn a_nested_relative_path_is_accepted() {
        let m =
            WrapperManifest::parse("crates/my-wrapper", &["f".to_owned()], &[]).expect("parses");
        assert_eq!(m.path().as_str(), "crates/my-wrapper");
    }

    #[test]
    fn declared_capabilities_are_parsed_into_the_closed_vocabulary() {
        let m = WrapperManifest::parse(
            "w",
            &["f".to_owned()],
            &["network".to_owned(), "filesystem".to_owned()],
        )
        .expect("parses");
        assert!(m.capabilities().contains(&Capability::Network));
        assert!(m.capabilities().contains(&Capability::Filesystem));
        assert_eq!(m.capabilities().len(), 2);
    }

    #[test]
    fn an_unknown_declared_capability_is_refused_at_decode() {
        // A typo'd capability must be a LOUD rejection, never a raw string the
        // reconcile then silently fails to compare (a fail-open hole).
        let r = WrapperManifest::parse("w", &["f".to_owned()], &["netwrok".to_owned()]);
        assert!(
            matches!(
                r,
                Err(Diagnostic::SourceRejected {
                    defect: SourceDefect::WrapperCapabilityUnknown { .. },
                    ..
                })
            ),
            "{r:?}"
        );
    }

    #[test]
    fn an_absolute_path_is_refused_at_decode() {
        let r = WrapperPath::parse("/etc/passwd");
        assert!(matches!(
            r,
            Err(Diagnostic::SourceRejected {
                defect: SourceDefect::WrapperPathEscapes { .. },
                ..
            })
        ));
    }

    #[test]
    fn a_dotdot_escape_is_refused_at_decode() {
        for escape in ["../evil", "wrappers/../../etc", ".."] {
            let r = WrapperPath::parse(escape);
            assert!(
                matches!(
                    r,
                    Err(Diagnostic::SourceRejected {
                        defect: SourceDefect::WrapperPathEscapes { .. },
                        ..
                    })
                ),
                "{escape:?} must be refused"
            );
        }
    }

    #[test]
    fn a_dot_inside_a_filename_is_not_an_escape() {
        // A `.` that is part of a segment name (never a whole `..` segment) is a
        // legal filename character.
        let ok = WrapperPath::parse("my.wrapper/sub.dir").expect("parses");
        assert_eq!(ok.as_str(), "my.wrapper/sub.dir");
    }

    #[test]
    fn a_metacharacter_in_the_path_is_refused() {
        for bad in ["a b", "a;rm -rf", "a$(x)", "a\"b", "a\nb"] {
            let r = WrapperPath::parse(bad);
            assert!(
                matches!(
                    r,
                    Err(Diagnostic::SourceRejected {
                        defect: SourceDefect::WrapperPathCharsetIllegal { .. },
                        ..
                    })
                ),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn an_empty_expose_list_is_refused() {
        let r = WrapperManifest::parse("wrappers", &[], &[]);
        assert!(matches!(
            r,
            Err(Diagnostic::SourceRejected {
                defect: SourceDefect::WrapperExposeEmpty,
                ..
            })
        ));
    }

    #[test]
    fn an_ill_formed_exposed_symbol_is_refused() {
        let r = WrapperManifest::parse("wrappers", &["9bad".to_owned()], &[]);
        assert!(matches!(r, Err(Diagnostic::WireMalformed { .. })));
    }
}
