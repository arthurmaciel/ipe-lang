//! Security advisory database — reader and dependency-audit check.
//!
//! The registry (`arthurmaciel/ipe-registry`) hosts advisory files at
//! `advisories/<pkg>/<id>.toml`.  Each file is one advisory record:
//! a package name, an affected version range ([`semver::VersionReq`]), a
//! severity, a unique id, a short description, and an optional fixed-in
//! version.  The client reads them and cross-checks locked dependencies.
//!
//! # File layout in the registry
//!
//! ```text
//! advisories/
//!   http-client/
//!     IPE-2024-0001.toml
//!     IPE-2024-0002.toml
//!   some-other-pkg/
//!     IPE-2025-0003.toml
//! ```
//!
//! Each `.toml` must contain exactly these keys:
//!
//! ```toml
//! id          = "IPE-2024-0001"
//! package     = "http-client"
//! severity    = "high"          # "low", "medium", "high", or "critical"
//! affected    = ">=1.0.0, <1.2.3"
//! description = "Short description of the vulnerability."
//! fixed_in    = "1.2.3"         # optional; omit or leave empty when no fix
//! ```
//!
//! # Fail-closed policy
//!
//! | Condition                                   | Result                                |
//! |---------------------------------------------|---------------------------------------|
//! | Advisory DB absent (no `advisories/` dir)   | `Ok(())` — treated as empty DB        |
//! | Advisory DB dir present but unreadable      | `Err(AdvisoryDbUnreachable)` — refuse |
//! | A `.toml` file is malformed                 | `Err(AdvisoryDbMalformed)` — refuse   |
//! | A dep matches a `critical` or `high` advisory | `Err(AdvisoryVulnerable)` — reject  |
//! | A dep matches a `medium` or `low` advisory  | Warning to stderr; pass               |
//!
//! A malformed or unreadable advisory DB is never silently treated as "safe":
//! absent proof the dep is clean, the gate refuses (PRINCIPLES §1 Security).
//! The only exception is a genuinely absent `advisories/` directory — a
//! registry that has not yet published any advisories is not a risk signal.

use std::path::{Path, PathBuf};

use crate::CliError;

// ── Advisory ID ───────────────────────────────────────────────────────────────

/// A validated advisory identifier.
///
/// Must be non-empty and must not start with `-` (injection guard).  No other
/// structural constraint is imposed — the format is `IPE-<YEAR>-<SEQ>` by
/// convention but is not enforced by the type (forward-compatible with future
/// schemes).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AdvisoryId(String);

impl AdvisoryId {
    /// Parse a raw `id` string, rejecting empty values and leading-`-` tokens.
    ///
    /// # Errors
    /// [`CliError::AdvisoryDbMalformed`] when the value is empty or injection-shaped.
    fn parse(raw: &str, path: &Path) -> Result<Self, CliError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('-') {
            return Err(CliError::AdvisoryDbMalformed {
                path: path.to_path_buf(),
                detail: format!(
                    "advisory `id` must be a non-empty, non-`-`-leading string; got: {raw:?}"
                ),
            });
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The validated id string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AdvisoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ── Severity ─────────────────────────────────────────────────────────────────

/// Advisory severity, parsed from the `severity` field.
///
/// Variants are ordered from lowest to highest so `>=` comparisons work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational or low-impact; reported as a warning.
    Low,
    /// Moderate impact; reported as a warning.
    Medium,
    /// High impact; causes a typed rejection.
    High,
    /// Critical impact; causes a typed rejection.
    Critical,
}

impl Severity {
    /// Parse the `severity` field value (case-insensitive).
    ///
    /// # Errors
    /// [`CliError::AdvisoryDbMalformed`] on an unrecognised value.
    fn parse(raw: &str, path: &Path) -> Result<Self, CliError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "critical" => Ok(Self::Critical),
            other => Err(CliError::AdvisoryDbMalformed {
                path: path.to_path_buf(),
                detail: format!(
                    "advisory `severity` must be one of `low`, `medium`, `high`, `critical`; \
                     got: {other:?}"
                ),
            }),
        }
    }

    /// Whether this severity causes a typed rejection (vs. a warning).
    #[must_use]
    pub const fn is_rejection(self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }

    /// Human-readable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Advisory record ──────────────────────────────────────────────────────────

/// A parsed advisory record from `advisories/<pkg>/<id>.toml`.
///
/// Constructed only via [`parse_advisory`] — there is no public constructor —
/// so a value in this type is always fully validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advisory {
    /// The unique advisory identifier (e.g. `IPE-2024-0001`).
    pub id: AdvisoryId,
    /// The affected package name (must match the directory name).
    pub package: String,
    /// The affected version range.
    pub affected: semver::VersionReq,
    /// Severity determines warn-vs-reject.
    pub severity: Severity,
    /// Short description of the vulnerability.
    pub description: String,
    /// The first fixed version, if one exists.
    pub fixed_in: Option<semver::Version>,
}

// ── Parser ───────────────────────────────────────────────────────────────────

/// Parse the text of one `advisories/<pkg>/<id>.toml` into a typed
/// [`Advisory`].  Every required field absent or malformed is a typed
/// [`CliError::AdvisoryDbMalformed`] — never a partial or silent pass.
///
/// Unknown keys are ignored (forward-compatible with future extensions).
///
/// # Errors
/// [`CliError::AdvisoryDbMalformed`] for any missing required field or a
/// malformed value.
pub fn parse_advisory(text: &str, path: &Path) -> Result<Advisory, CliError> {
    // Use the `toml` crate already in the dependency graph (see Cargo.toml).
    // Parse into a raw Value so we can extract typed fields without a serde
    // derive on Advisory (keeping the type free of serde coupling).
    let table: toml::Table = text.parse().map_err(|e| CliError::AdvisoryDbMalformed {
        path: path.to_path_buf(),
        detail: format!("TOML parse error: {e}"),
    })?;

    let required_str = |key: &str| -> Result<&str, CliError> {
        table
            .get(key)
            .and_then(toml::Value::as_str)
            .ok_or_else(|| CliError::AdvisoryDbMalformed {
                path: path.to_path_buf(),
                detail: format!("advisory is missing required string field `{key}`"),
            })
    };

    let id = AdvisoryId::parse(required_str("id")?, path)?;
    let package = required_str("package")?.trim().to_owned();
    if package.is_empty() {
        return Err(CliError::AdvisoryDbMalformed {
            path: path.to_path_buf(),
            detail: "advisory `package` must be a non-empty string".to_owned(),
        });
    }
    let severity = Severity::parse(required_str("severity")?, path)?;
    let affected_raw = required_str("affected")?;
    let affected =
        affected_raw
            .parse::<semver::VersionReq>()
            .map_err(|e| CliError::AdvisoryDbMalformed {
                path: path.to_path_buf(),
                detail: format!("advisory `affected` is not a valid version requirement: {e}"),
            })?;
    let description = required_str("description")?.trim().to_owned();
    if description.is_empty() {
        return Err(CliError::AdvisoryDbMalformed {
            path: path.to_path_buf(),
            detail: "advisory `description` must be a non-empty string".to_owned(),
        });
    }
    let fixed_in = match table.get("fixed_in").and_then(toml::Value::as_str) {
        None | Some("") => None,
        Some(raw) => {
            let v = raw.trim().parse::<semver::Version>().map_err(|e| {
                CliError::AdvisoryDbMalformed {
                    path: path.to_path_buf(),
                    detail: format!("advisory `fixed_in` is not a valid semver version: {e}"),
                }
            })?;
            Some(v)
        }
    };

    Ok(Advisory {
        id,
        package,
        affected,
        severity,
        description,
        fixed_in,
    })
}

// ── Reader ───────────────────────────────────────────────────────────────────

/// Read all advisories for `pkg_name` from `advisory_db_root`.
///
/// Mirrors the index reader pattern in `index.rs`: reads from
/// `<advisory_db_root>/advisories/<pkg_name>/` and returns a `Vec<Advisory>`.
///
/// Returns `Ok(vec![])` when the package directory does not exist (no
/// advisories for that package).  Returns an error when the directory exists
/// but is unreadable, or any `.toml` file inside is malformed (fail-closed:
/// an unreadable advisory cannot be treated as "safe").
///
/// # Errors
/// [`CliError::AdvisoryDbUnreachable`] when the directory cannot be listed.
/// [`CliError::AdvisoryDbMalformed`] when a `.toml` file is malformed.
pub fn read_advisories_for(
    advisory_db_root: &Path,
    pkg_name: &str,
) -> Result<Vec<Advisory>, CliError> {
    let dir = advisory_db_dir(advisory_db_root, pkg_name);
    read_advisory_dir(&dir, pkg_name)
}

/// The path of the per-package advisory directory inside the DB root.
fn advisory_db_dir(advisory_db_root: &Path, pkg_name: &str) -> PathBuf {
    advisory_db_root.join("advisories").join(pkg_name)
}

/// Read all `.toml` advisory files inside `dir` for `pkg_name`.
///
/// Absent directory → `Ok(vec![])`.  Unreadable directory → error (fail-closed).
fn read_advisory_dir(dir: &Path, pkg_name: &str) -> Result<Vec<Advisory>, CliError> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No advisories directory for this package — clean.
            return Ok(Vec::new());
        }
        Err(e) => {
            return Err(CliError::AdvisoryDbUnreachable {
                detail: format!(
                    "could not list advisory directory for `{pkg_name}` at {}: {e}",
                    dir.display()
                ),
            });
        }
    };

    let mut advisories = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| CliError::AdvisoryDbUnreachable {
            detail: format!("could not read advisory directory entry for `{pkg_name}`: {e}"),
        })?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            // Skip non-TOML files (READMEs, etc.) without error.
            continue;
        }
        let text = crate::io_bounded::read_to_string_capped(
            &path,
            crate::io_bounded::SMALL_FILE_READ_CAP,
        )
        .map_err(|e| match e {
            CliError::FileTooLarge { path: p, max } => CliError::AdvisoryDbMalformed {
                path: p,
                detail: format!(
                    "advisory file exceeds the {max}-byte read ceiling — malformed or not an advisory"
                ),
            },
            CliError::Io { source, .. } => CliError::AdvisoryDbUnreachable {
                detail: format!(
                    "could not read advisory file {}: {source}",
                    path.display()
                ),
            },
            other => other,
        })?;
        advisories.push(parse_advisory(&text, &path)?);
    }
    Ok(advisories)
}

// ── Audit check ──────────────────────────────────────────────────────────────

/// Cross-check one locked dependency version against the advisory DB.
///
/// Reads all advisories for `pkg_name` from `advisory_db_root` and tests
/// `locked_version` against each advisory's `affected` range.
///
/// **Fail-closed policy:**
/// - `high` or `critical` severity match → typed [`CliError::AdvisoryVulnerable`] (reject).
/// - `low` or `medium` severity match → warning printed to stderr; function returns `Ok(())`.
/// - Malformed or unreadable advisory file → propagated as error (refuse to pass silently).
/// - Package directory absent → `Ok(())` (no advisories recorded).
///
/// # Errors
/// [`CliError::AdvisoryVulnerable`] on a high/critical match.
/// [`CliError::AdvisoryDbMalformed`] on a malformed advisory file.
/// [`CliError::AdvisoryDbUnreachable`] on an unreadable advisory directory.
pub fn check_dep_advisories(
    advisory_db_root: &Path,
    pkg_name: &str,
    locked_version: &semver::Version,
) -> Result<(), CliError> {
    let advisories = read_advisories_for(advisory_db_root, pkg_name)?;
    for adv in &advisories {
        if !adv.affected.matches(locked_version) {
            continue;
        }
        if adv.severity.is_rejection() {
            return Err(CliError::AdvisoryVulnerable(Box::new(
                crate::AdvisoryVulnerablePayload {
                    package: pkg_name.to_owned(),
                    version: locked_version.to_string(),
                    id: adv.id.to_string(),
                    severity: adv.severity.as_str(),
                    description: adv.description.clone(),
                    fixed_in: adv.fixed_in.as_ref().map(ToString::to_string),
                },
            )));
        }
        // low/medium: warn, continue scanning.
        eprintln!(
            "{}",
            crate::style::gutter(&format!(
                "warning: dependency `{pkg_name}` v{locked_version} matches {}-severity advisory \
                 {} — {}{}\n  \
                 Upgrade to satisfy the advisory; this version is currently allowed \
                 (advisory severity: {}).",
                adv.severity,
                adv.id,
                adv.description,
                adv.fixed_in
                    .as_ref()
                    .map(|v| format!(" Fixed in: {v}."))
                    .unwrap_or_default(),
                adv.severity,
            ))
        );
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_path() -> PathBuf {
        PathBuf::from("advisories/test-pkg/IPE-2024-0001.toml")
    }

    // ── parse_advisory ────────────────────────────────────────────────────────

    #[test]
    fn parse_advisory_minimal_valid() {
        let text = r#"
id          = "IPE-2024-0001"
package     = "test-pkg"
severity    = "high"
affected    = ">=1.0.0, <1.2.0"
description = "A serious vulnerability."
"#;
        let adv = parse_advisory(text, &fake_path()).expect("valid advisory must parse");
        assert_eq!(adv.id.as_str(), "IPE-2024-0001");
        assert_eq!(adv.package, "test-pkg");
        assert_eq!(adv.severity, Severity::High);
        assert_eq!(adv.description, "A serious vulnerability.");
        assert!(adv.fixed_in.is_none());
    }

    #[test]
    fn parse_advisory_with_fixed_in() {
        let text = r#"
id          = "IPE-2024-0002"
package     = "test-pkg"
severity    = "critical"
affected    = ">=0.1.0, <0.2.0"
description = "Critical bug."
fixed_in    = "0.2.0"
"#;
        let adv = parse_advisory(text, &fake_path()).expect("valid advisory");
        assert_eq!(adv.severity, Severity::Critical);
        assert_eq!(adv.fixed_in, Some(semver::Version::parse("0.2.0").unwrap()));
    }

    #[test]
    fn parse_advisory_low_severity() {
        let text = r#"
id          = "IPE-2024-0003"
package     = "test-pkg"
severity    = "low"
affected    = ">=0.5.0, <0.6.0"
description = "Minor issue."
"#;
        let adv = parse_advisory(text, &fake_path()).expect("valid advisory");
        assert_eq!(adv.severity, Severity::Low);
        assert!(!adv.severity.is_rejection());
    }

    #[test]
    fn parse_advisory_missing_required_field() {
        // Missing `description`
        let text = r#"
id       = "IPE-2024-0004"
package  = "test-pkg"
severity = "high"
affected = ">=1.0.0, <2.0.0"
"#;
        let err = parse_advisory(text, &fake_path()).expect_err("must fail — missing description");
        assert!(
            matches!(err, CliError::AdvisoryDbMalformed { .. }),
            "wrong error type: {err:?}"
        );
    }

    #[test]
    fn parse_advisory_bad_severity() {
        let text = r#"
id          = "IPE-2024-0005"
package     = "test-pkg"
severity    = "super-critical"
affected    = ">=1.0.0, <2.0.0"
description = "Desc."
"#;
        let err = parse_advisory(text, &fake_path()).expect_err("bad severity must fail");
        assert!(
            matches!(err, CliError::AdvisoryDbMalformed { .. }),
            "wrong error type: {err:?}"
        );
    }

    #[test]
    fn parse_advisory_bad_affected_req() {
        let text = r#"
id          = "IPE-2024-0006"
package     = "test-pkg"
severity    = "high"
affected    = "not-a-semver-req"
description = "Desc."
"#;
        let err = parse_advisory(text, &fake_path()).expect_err("bad affected must fail");
        assert!(
            matches!(err, CliError::AdvisoryDbMalformed { .. }),
            "wrong error type: {err:?}"
        );
    }

    #[test]
    fn parse_advisory_bad_fixed_in() {
        let text = r#"
id          = "IPE-2024-0007"
package     = "test-pkg"
severity    = "high"
affected    = ">=1.0.0, <2.0.0"
description = "Desc."
fixed_in    = "not-a-version"
"#;
        let err = parse_advisory(text, &fake_path()).expect_err("bad fixed_in must fail");
        assert!(
            matches!(err, CliError::AdvisoryDbMalformed { .. }),
            "wrong error type: {err:?}"
        );
    }

    #[test]
    fn parse_advisory_bad_toml() {
        let text = "this is not toml ===";
        let err = parse_advisory(text, &fake_path()).expect_err("bad TOML must fail");
        assert!(
            matches!(err, CliError::AdvisoryDbMalformed { .. }),
            "wrong error type: {err:?}"
        );
    }

    // ── Severity ordering ────────────────────────────────────────────────────

    #[test]
    fn severity_ordering() {
        assert!(Severity::Low < Severity::Medium);
        assert!(Severity::Medium < Severity::High);
        assert!(Severity::High < Severity::Critical);
    }

    #[test]
    fn severity_is_rejection() {
        assert!(!Severity::Low.is_rejection());
        assert!(!Severity::Medium.is_rejection());
        assert!(Severity::High.is_rejection());
        assert!(Severity::Critical.is_rejection());
    }

    // ── check_dep_advisories (fs-backed) ─────────────────────────────────────

    fn temp_advisory_db(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe-advisory-test-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create advisory db dir");
        dir
    }

    fn write_advisory(db: &Path, pkg: &str, id: &str, content: &str) {
        let dir = db.join("advisories").join(pkg);
        std::fs::create_dir_all(&dir).expect("create advisory pkg dir");
        std::fs::write(dir.join(format!("{id}.toml")), content).expect("write advisory file");
    }

    #[test]
    fn check_dep_no_advisory_dir_is_clean() {
        let db = temp_advisory_db("no-dir");
        let version = semver::Version::parse("1.0.0").unwrap();
        // No `advisories/some-pkg/` dir at all.
        let result = check_dep_advisories(&db, "some-pkg", &version);
        assert!(
            result.is_ok(),
            "absent advisory dir must be clean: {result:?}"
        );
    }

    #[test]
    fn check_dep_in_affected_range_high_is_rejected() {
        let db = temp_advisory_db("high-reject");
        write_advisory(
            &db,
            "http-client",
            "IPE-2024-0001",
            r#"
id          = "IPE-2024-0001"
package     = "http-client"
severity    = "high"
affected    = ">=1.0.0, <1.2.0"
description = "SSRF vulnerability."
fixed_in    = "1.2.0"
"#,
        );
        let version = semver::Version::parse("1.1.0").unwrap();
        let err = check_dep_advisories(&db, "http-client", &version)
            .expect_err("high-severity in-range must be rejected");
        assert!(
            matches!(err, CliError::AdvisoryVulnerable(_)),
            "wrong error type: {err:?}"
        );
        if let CliError::AdvisoryVulnerable(p) = &err {
            assert_eq!(p.package, "http-client");
            assert_eq!(p.id, "IPE-2024-0001");
            assert_eq!(p.severity, "high");
        }
    }

    #[test]
    fn check_dep_outside_affected_range_is_clean() {
        let db = temp_advisory_db("outside-range");
        write_advisory(
            &db,
            "http-client",
            "IPE-2024-0001",
            r#"
id          = "IPE-2024-0001"
package     = "http-client"
severity    = "high"
affected    = ">=1.0.0, <1.2.0"
description = "SSRF vulnerability."
fixed_in    = "1.2.0"
"#,
        );
        // 1.2.0 is NOT in [1.0.0, 1.2.0).
        let version = semver::Version::parse("1.2.0").unwrap();
        let result = check_dep_advisories(&db, "http-client", &version);
        assert!(
            result.is_ok(),
            "out-of-range version must be clean: {result:?}"
        );
    }

    #[test]
    fn check_dep_critical_in_range_is_rejected() {
        let db = temp_advisory_db("critical");
        write_advisory(
            &db,
            "crypto-lib",
            "IPE-2024-0002",
            r#"
id          = "IPE-2024-0002"
package     = "crypto-lib"
severity    = "critical"
affected    = "<2.0.0"
description = "Key material leak."
"#,
        );
        let version = semver::Version::parse("1.9.9").unwrap();
        let err = check_dep_advisories(&db, "crypto-lib", &version)
            .expect_err("critical must be rejected");
        assert!(matches!(err, CliError::AdvisoryVulnerable(_)));
    }

    #[test]
    fn malformed_advisory_file_is_typed_error_not_silent_pass() {
        let db = temp_advisory_db("malformed");
        write_advisory(
            &db,
            "broken-pkg",
            "IPE-BAD-0001",
            "this is not valid toml ===",
        );
        let version = semver::Version::parse("1.0.0").unwrap();
        let err = check_dep_advisories(&db, "broken-pkg", &version)
            .expect_err("malformed advisory must be a typed error");
        assert!(
            matches!(err, CliError::AdvisoryDbMalformed { .. }),
            "expected AdvisoryDbMalformed, got: {err:?}"
        );
    }

    #[test]
    fn low_severity_in_range_returns_ok() {
        let db = temp_advisory_db("low-warn");
        write_advisory(
            &db,
            "info-leak",
            "IPE-2024-0003",
            r#"
id          = "IPE-2024-0003"
package     = "info-leak"
severity    = "low"
affected    = ">=1.0.0, <2.0.0"
description = "Minor information disclosure."
"#,
        );
        let version = semver::Version::parse("1.5.0").unwrap();
        // low severity → warn only, Ok(()).
        let result = check_dep_advisories(&db, "info-leak", &version);
        assert!(result.is_ok(), "low severity must not reject: {result:?}");
    }
}
