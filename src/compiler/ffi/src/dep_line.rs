//! Typed `[dependencies]` line for the emitted app crate's `Cargo.toml`.
//!
//! A [`DepLine`] is the one representation of a pinned dependency: the
//! manifest emitter renders it through [`std::fmt::Display`] and the cache
//! loader reads a stored line back through [`DepLine::parse`], which accepts
//! exactly the rendered form. Every spliced value is a decode-validated
//! newtype whose charset excludes TOML metacharacters, so no value can close
//! its string and inject manifest content.

use std::fmt;

use crate::diag::WireDefect;
use crate::pkginfo::{CrateVersion, FeatureName, PackageName, WrapperCratePath};

/// Where a dependency resolves from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepSource {
    /// An exact registry pin (`"=<version>"`); never empty.
    Registry(CrateVersion),
    /// An author-supplied wrapper crate bound by local path; never empty.
    Path(WrapperCratePath),
}

impl fmt::Display for DepSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(version) => write!(f, "={}", version.as_str()),
            Self::Path(path) => write!(f, "path {}", path.as_str()),
        }
    }
}

/// One pinned `[dependencies]` line: a package, its source, and its features.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepLine {
    name: PackageName,
    source: DepSource,
    features: Vec<FeatureName>,
}

impl DepLine {
    /// A registry dependency pinned to exactly `version`.
    ///
    /// # Errors
    /// [`WireDefect::UnpinnedDependency`] when `version` is empty.
    pub fn registry(
        name: PackageName,
        version: CrateVersion,
        features: Vec<FeatureName>,
    ) -> Result<Self, WireDefect> {
        if version.is_empty() {
            return Err(WireDefect::UnpinnedDependency {
                name: name.as_str().to_owned(),
            });
        }
        Ok(Self {
            name,
            source: DepSource::Registry(version),
            features,
        })
    }

    /// A wrapper-crate dependency bound by local absolute `path`.
    ///
    /// Infallible: a [`WrapperCratePath`] is absolute and normalized by
    /// construction, so there is no empty or unjailed path to refuse here.
    #[must_use]
    pub const fn path(
        name: PackageName,
        path: WrapperCratePath,
        features: Vec<FeatureName>,
    ) -> Self {
        Self {
            name,
            source: DepSource::Path(path),
            features,
        }
    }

    /// Parse a stored line, accepting exactly the form [`fmt::Display`] renders.
    ///
    /// The shapes are `name = "=V"`, `name = { version = "=V", features = [..] }`,
    /// `name = { path = "P" }` and `name = { path = "P", features = [..] }`.
    /// Anything else — extra keys, reordered keys, spacing drift, an empty
    /// feature list, a version or path outside its charset — is refused.
    ///
    /// # Errors
    /// [`WireDefect::InvalidDependencyLine`] for a line off the canonical
    /// grammar; the component newtype's own defect for an illegal name,
    /// version, or feature; [`WireDefect::InvalidWrapperPath`] for a path that
    /// is empty, relative, `..`-bearing, or outside its charset;
    /// [`WireDefect::UnpinnedDependency`] for an empty version.
    pub fn parse(line: &str) -> Result<Self, WireDefect> {
        let refuse = |reason: &'static str| WireDefect::InvalidDependencyLine {
            got: line.to_owned(),
            reason,
        };
        let (name, value) = line
            .split_once(" = ")
            .ok_or_else(|| refuse("no `<name> = <value>` separator"))?;
        let name = PackageName::parse(name)?;
        let parsed = match value.strip_prefix("{ ").and_then(|t| t.strip_suffix(" }")) {
            Some(table) => Self::parse_table(name, table, &refuse)?,
            None => {
                let version = value
                    .strip_prefix("\"=")
                    .and_then(|v| v.strip_suffix('"'))
                    .ok_or_else(|| {
                        refuse("the value is neither a `\"=<version>\"` pin nor an inline table")
                    })?;
                Self::registry(name, CrateVersion::parse(version)?, Vec::new())?
            }
        };
        if parsed.to_string() == line {
            Ok(parsed)
        } else {
            Err(refuse("the line is not in the canonical rendered form"))
        }
    }

    /// Parse the body of an inline table (between `{ ` and ` }`).
    fn parse_table(
        name: PackageName,
        table: &str,
        refuse: &impl Fn(&'static str) -> WireDefect,
    ) -> Result<Self, WireDefect> {
        let (head, features) = match table.split_once(", features = [") {
            Some((head, list)) => {
                let list = list
                    .strip_suffix(']')
                    .ok_or_else(|| refuse("the feature list is not closed by `]`"))?;
                (head, parse_features(list, refuse)?)
            }
            None => (table, Vec::new()),
        };
        if let Some(version) = quoted_value(head, "version = \"=") {
            Self::registry(name, CrateVersion::parse(version)?, features)
        } else if let Some(path) = quoted_value(head, "path = \"") {
            Ok(Self::path(name, WrapperCratePath::parse(path)?, features))
        } else {
            Err(refuse(
                "the inline table has neither a `version` nor a `path` key",
            ))
        }
    }

    /// The dependency's package name (the `[dependencies]` key).
    #[must_use]
    pub const fn name(&self) -> &PackageName {
        &self.name
    }

    /// Where the dependency resolves from.
    #[must_use]
    pub const fn source(&self) -> &DepSource {
        &self.source
    }

    /// The requested features, in rendered order.
    #[must_use]
    pub fn features(&self) -> &[FeatureName] {
        &self.features
    }

    /// The same dependency requesting `features` instead.
    #[must_use]
    pub fn with_features(&self, features: Vec<FeatureName>) -> Self {
        Self {
            name: self.name.clone(),
            source: self.source.clone(),
            features,
        }
    }
}

/// The text between `prefix` and a closing `"` that ends `head`.
fn quoted_value<'a>(head: &'a str, prefix: &str) -> Option<&'a str> {
    head.strip_prefix(prefix)?.strip_suffix('"')
}

/// Parse a rendered `"a", "b"` feature list; an empty list is refused.
fn parse_features(
    list: &str,
    refuse: &impl Fn(&'static str) -> WireDefect,
) -> Result<Vec<FeatureName>, WireDefect> {
    list.split(", ")
        .map(|quoted| {
            let bare = quoted
                .strip_prefix('"')
                .and_then(|q| q.strip_suffix('"'))
                .ok_or_else(|| refuse("a feature is not a double-quoted string"))?;
            FeatureName::parse(bare)
        })
        .collect()
}

impl fmt::Display for DepLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.name.as_str();
        if self.features.is_empty() {
            return match &self.source {
                DepSource::Registry(version) => write!(f, "{name} = \"={}\"", version.as_str()),
                DepSource::Path(path) => write!(f, "{name} = {{ path = \"{}\" }}", path.as_str()),
            };
        }
        match &self.source {
            DepSource::Registry(version) => {
                write!(f, "{name} = {{ version = \"={}\"", version.as_str())?;
            }
            DepSource::Path(path) => write!(f, "{name} = {{ path = \"{}\"", path.as_str())?,
        }
        f.write_str(", features = [")?;
        for (i, feature) in self.features.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "\"{}\"", feature.as_str())?;
        }
        f.write_str("] }")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rendered_shape_round_trips() {
        for line in [
            "semver = \"=1.0.26\"",
            "async-stripe = { version = \"=1.0.0-rc.6\", features = [\"serialize\", \"dep:x\"] }",
            "demo = { path = \"/home/u/wrap crate\" }",
            "demo = { path = \"/w\", features = [\"a\"] }",
        ] {
            let parsed = DepLine::parse(line);
            assert!(parsed.is_ok(), "{line} must parse: {parsed:?}");
            assert_eq!(parsed.map(|d| d.to_string()).ok().as_deref(), Some(line));
        }
    }

    #[test]
    fn the_parsed_parts_are_typed() {
        let parsed = DepLine::parse("x = { version = \"=2.1.0\", features = [\"b\", \"a\"] }");
        assert!(parsed.is_ok(), "canonical line must parse: {parsed:?}");
        let Ok(dep) = parsed else { return };
        assert_eq!(dep.name().as_str(), "x");
        assert!(matches!(dep.source(), DepSource::Registry(v) if v.as_str() == "2.1.0"));
        let features: Vec<&str> = dep.features().iter().map(FeatureName::as_str).collect();
        assert_eq!(features, ["b", "a"]);
    }

    #[test]
    fn off_grammar_lines_are_refused() {
        for line in [
            "garbage",
            "",
            "x=\"=1.0\"",
            "x = \"1.0\"",
            "x = \"=1.0",
            "x = { version = \"=1.0\" }",
            "x = { version = \"=1.0\", features = [] }",
            "x = { version = \"=1.0\", features = [a] }",
            "x = { version = \"=1.0\", features = [\"a\"], default-features = false }",
            "x = { features = [\"a\"], version = \"=1.0\" }",
            "x = { git = \"https://example.invalid/x\" }",
            "x = {version = \"=1.0\"}",
            "x = \"=1.0\" ",
        ] {
            assert!(
                matches!(
                    DepLine::parse(line),
                    Err(WireDefect::InvalidDependencyLine { .. })
                ),
                "{line:?} must be refused as off-grammar: {:?}",
                DepLine::parse(line)
            );
        }
    }

    #[test]
    fn injection_bearing_components_are_refused_by_their_newtype() {
        assert!(matches!(
            DepLine::parse("x\ny = \"=1.0\""),
            Err(WireDefect::InvalidIdent { .. })
        ));
        assert!(matches!(
            DepLine::parse("x = \"=1.0\"\n[dependencies.evil]\"\""),
            Err(WireDefect::InvalidVersion { .. })
        ));
        assert!(matches!(
            DepLine::parse("x = { path = \"/a\\\"b\" }"),
            Err(WireDefect::InvalidWrapperPath { .. })
        ));
        assert!(matches!(
            DepLine::parse("x = { version = \"=1.0\", features = [\"a}\"] }"),
            Err(WireDefect::InvalidFeature { .. })
        ));
        assert!(matches!(
            DepLine::parse("x = { version = \"=1.0\", features = [\"a\",\"b\"] }"),
            Err(WireDefect::InvalidFeature { .. })
        ));
    }

    #[test]
    fn an_unpinned_line_is_refused() {
        assert!(matches!(
            DepLine::parse("x = \"=\""),
            Err(WireDefect::UnpinnedDependency { .. })
        ));
    }

    // A stored path line is re-jailed on load: only an absolute, normalized
    // path parses, so a tampered cache cannot bind a directory the install
    // jail never canonicalized.
    #[test]
    fn an_unjailed_path_line_is_refused() {
        for path in [
            "",
            "wrappers/engine",
            "./wrappers/engine",
            "../evil",
            "/",
            "/w/../etc",
            "/w/.",
            "/w//engine",
            "/w/engine/",
        ] {
            let line = format!("x = {{ path = \"{path}\" }}");
            assert!(
                matches!(
                    DepLine::parse(&line),
                    Err(WireDefect::InvalidWrapperPath { .. })
                ),
                "{line:?} must be refused as an unjailed wrapper path"
            );
        }
    }
}
