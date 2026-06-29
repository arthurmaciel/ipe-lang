//! Cached Go-oracle format + staleness gate for the golden parity suite.
//!
//! The Go reference compiler's output for a golden is a pure function of
//! `(Main.sky, Go `sky` version)` — it does not depend on skyc at all. So rather
//! than re-running the Go backend on every parity check, each golden commits a
//! cached oracle value:
//!
//! * `tests/golden/<name>/expected_go.txt` — the oracle's clean program stdout
//!   (the bytes the Go-built binary prints, with NONE of the compiler's progress
//!   chatter), OR — when the Go oracle is itself buggy on this shape — skyc's
//!   own (correct) output, recorded as a documented divergence.
//! * `tests/golden/<name>/oracle.meta` — `sha256(Main.sky)` + the Go `sky`
//!   version string + an `oracle_divergence` flag (with a reason when set) + the
//!   captured exit code.
//!
//! Two halves live here:
//!
//! * The **write** side ([`Meta::serialize`], [`sha256_hex`],
//!   [`build_and_run_rust`]) is used by the `refresh-oracle` tool to (re)capture
//!   the cached value when a golden is added or changed.
//! * The **read** side ([`check_parity`]) is used by the golden tests. It NEVER
//!   invokes the Go backend: it re-hashes `Main.sky`, fails loudly if the hash
//!   no longer matches the cached one (a stale oracle the author forgot to
//!   refresh), and otherwise diffs skyc's stdout against the cached expected.
//!
//! Rigour: a stale hash is a hard failure (never a silent diff against an
//! out-of-date expectation), a missing oracle is a hard failure (never a skip),
//! and a Go-oracle *failure* is never cached as "correct" — it is routed to the
//! divergence branch by the refresh tool, with skyc's output recorded instead.

#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// File holding the cached oracle stdout, relative to a golden's directory.
pub const EXPECTED_FILE: &str = "expected_go.txt";
/// File holding the cached oracle metadata, relative to a golden's directory.
pub const META_FILE: &str = "oracle.meta";
/// The Sky entry point inside every golden directory.
pub const MAIN_SKY: &str = "Main.sky";

/// Lowercase hex SHA-256 of `bytes`.
///
/// Used to fingerprint `Main.sky` so a golden whose source changed without a
/// matching `refresh-oracle` run is caught by [`check_parity`] rather than
/// silently diffed against a stale expectation.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Parsed contents of an `oracle.meta` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    /// Lowercase hex SHA-256 of the `Main.sky` the cache was captured from.
    pub main_sky_sha256: String,
    /// The Go `sky --version` string at capture time (provenance only).
    pub go_sky_version: String,
    /// Process exit code recorded alongside `expected_go.txt`.
    pub exit_code: i32,
    /// `true` when `expected_go.txt` holds skyc's output because the Go oracle
    /// was buggy on this shape (see `divergence_reason`), NOT the Go output.
    pub oracle_divergence: bool,
    /// Human-readable reason, present only when `oracle_divergence` is `true`.
    pub divergence_reason: Option<String>,
}

impl Meta {
    /// Serialize to the on-disk `key = value` form. Deterministic: the same
    /// `Meta` always renders byte-identical output.
    #[must_use]
    pub fn serialize(&self) -> String {
        let reason_line = self
            .divergence_reason
            .as_ref()
            .map_or_else(String::new, |reason| {
                format!("divergence_reason = {reason}\n")
            });
        format!(
            "# Cached Go-oracle metadata. Regenerate with the refresh-oracle tool.\n\
             main_sky_sha256 = {sha}\n\
             go_sky_version = {version}\n\
             exit_code = {exit}\n\
             oracle_divergence = {divergence}\n\
             {reason_line}",
            sha = self.main_sky_sha256,
            version = self.go_sky_version,
            exit = self.exit_code,
            divergence = self.oracle_divergence,
        )
    }

    /// Parse the on-disk `key = value` form. Blank lines and `#` comments are
    /// ignored; the value is everything after the first `=`, trimmed.
    ///
    /// # Errors
    /// Returns a human-readable message when a required key is missing or a
    /// value cannot be parsed (e.g. a non-integer `exit_code`).
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut sha: Option<String> = None;
        let mut version: Option<String> = None;
        let mut exit_code: Option<i32> = None;
        let mut divergence: Option<bool> = None;
        let mut reason: Option<String> = None;

        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("oracle.meta: line without `=`: {line:?}"));
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "main_sky_sha256" => sha = Some(value.to_owned()),
                "go_sky_version" => version = Some(value.to_owned()),
                "exit_code" => {
                    exit_code = Some(
                        value
                            .parse::<i32>()
                            .map_err(|e| format!("oracle.meta: bad exit_code {value:?}: {e}"))?,
                    );
                }
                "oracle_divergence" => {
                    divergence = Some(match value {
                        "true" => true,
                        "false" => false,
                        other => {
                            return Err(format!(
                                "oracle.meta: oracle_divergence must be true/false, got {other:?}"
                            ));
                        }
                    });
                }
                "divergence_reason" => reason = Some(value.to_owned()),
                other => return Err(format!("oracle.meta: unknown key {other:?}")),
            }
        }

        let main_sky_sha256 = sha.ok_or("oracle.meta: missing main_sky_sha256")?;
        let go_sky_version = version.ok_or("oracle.meta: missing go_sky_version")?;
        let exit_code = exit_code.ok_or("oracle.meta: missing exit_code")?;
        let oracle_divergence = divergence.ok_or("oracle.meta: missing oracle_divergence")?;
        if oracle_divergence && reason.is_none() {
            return Err(
                "oracle.meta: oracle_divergence=true requires divergence_reason".to_owned(),
            );
        }
        Ok(Self {
            main_sky_sha256,
            go_sky_version,
            exit_code,
            oracle_divergence,
            divergence_reason: reason,
        })
    }
}

/// Why a parity check failed. Every variant is a HARD failure — there is no
/// "skip" outcome, so a broken or stale golden can never pass silently.
#[derive(Clone, Debug)]
pub enum ParityError {
    /// `Main.sky` could not be read at `path`.
    MissingMainSky { path: PathBuf, detail: String },
    /// `oracle.meta` is absent — the golden was never registered with the
    /// refresh tool.
    MissingMeta { path: PathBuf, detail: String },
    /// `oracle.meta` is present but malformed.
    MalformedMeta { detail: String },
    /// `expected_go.txt` is absent though `oracle.meta` exists.
    MissingExpected { path: PathBuf, detail: String },
    /// The current `Main.sky` hash differs from the cached one: the source was
    /// edited without re-running the refresh tool.
    Stale {
        golden: String,
        cached: String,
        current: String,
    },
    /// skyc's stdout differs from the cached expected value.
    Mismatch {
        golden: String,
        divergence: bool,
        expected: String,
        actual: String,
    },
}

impl fmt::Display for ParityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMainSky { path, detail } => {
                write!(f, "cannot read {}: {detail}", path.display())
            }
            Self::MissingMeta { path, detail } => write!(
                f,
                "missing oracle for golden: {} ({detail}) — run refresh-oracle for this golden",
                path.display()
            ),
            Self::MalformedMeta { detail } => write!(f, "malformed oracle.meta: {detail}"),
            Self::MissingExpected { path, detail } => write!(
                f,
                "missing {}: {detail} — run refresh-oracle for this golden",
                path.display()
            ),
            Self::Stale {
                golden,
                cached,
                current,
            } => write!(
                f,
                "oracle stale for {golden} — run refresh-oracle (Main.sky sha256 {current} != cached {cached})"
            ),
            Self::Mismatch {
                golden,
                divergence,
                expected,
                actual,
            } => {
                let source = if *divergence {
                    "skyc-divergence-expected"
                } else {
                    "cached Go oracle"
                };
                write!(
                    f,
                    "{golden}: skyc stdout does not match {source}\n--- expected ---\n{expected}\n--- actual ---\n{actual}"
                )
            }
        }
    }
}

/// Read the cached oracle for the golden at `golden_dir` and compare it against
/// `skyc_stdout`. NEVER invokes the Go backend.
///
/// The staleness gate runs first: if `sha256(Main.sky)` differs from the cached
/// hash the function fails with [`ParityError::Stale`] BEFORE any diff, so a
/// changed source whose oracle was not refreshed can never be diffed against a
/// stale expectation.
///
/// # Errors
/// Returns a [`ParityError`] on any missing/stale/malformed input or on a
/// stdout mismatch. All are hard failures by design.
pub fn check_parity(
    golden_dir: &Path,
    golden_name: &str,
    skyc_stdout: &str,
) -> Result<(), ParityError> {
    let main_sky = golden_dir.join(MAIN_SKY);
    let source = std::fs::read(&main_sky).map_err(|e| ParityError::MissingMainSky {
        path: main_sky.clone(),
        detail: e.to_string(),
    })?;
    let current = sha256_hex(&source);

    let meta_path = golden_dir.join(META_FILE);
    let meta_text = std::fs::read_to_string(&meta_path).map_err(|e| ParityError::MissingMeta {
        path: meta_path.clone(),
        detail: e.to_string(),
    })?;
    let meta = Meta::parse(&meta_text).map_err(|detail| ParityError::MalformedMeta { detail })?;

    if meta.main_sky_sha256 != current {
        return Err(ParityError::Stale {
            golden: golden_name.to_owned(),
            cached: meta.main_sky_sha256,
            current,
        });
    }

    let expected_path = golden_dir.join(EXPECTED_FILE);
    let expected =
        std::fs::read_to_string(&expected_path).map_err(|e| ParityError::MissingExpected {
            path: expected_path,
            detail: e.to_string(),
        })?;

    if expected != skyc_stdout {
        return Err(ParityError::Mismatch {
            golden: golden_name.to_owned(),
            divergence: meta.oracle_divergence,
            expected,
            actual: skyc_stdout.to_owned(),
        });
    }
    Ok(())
}

/// Captured stdout + exit code from running a built program.
#[derive(Clone, Debug)]
pub struct RunResult {
    /// The program's standard output, decoded lossily from UTF-8.
    pub stdout: String,
    /// The process exit code (`None` if killed by a signal).
    pub exit_code: Option<i32>,
}

/// Turn an arbitrary golden name into a cargo-package-safe suffix.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Rewrite the emitted `Cargo.toml` so its package — and hence its binary — is
/// unique to this golden, letting every golden's binary coexist in the one
/// shared cargo target. Returns the unique package name.
fn rewrite_package_name(emitted_dir: &Path, golden_name: &str) -> Result<String, String> {
    const ANCHOR: &str = "name = \"sky-app\"";

    let manifest = emitted_dir.join("Cargo.toml");
    let original = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    if !original.contains(ANCHOR) {
        return Err(format!(
            "emitted manifest {} did not contain the expected `{ANCHOR}` anchor",
            manifest.display()
        ));
    }
    let unique = format!("sky-app-e2e-{}", sanitize(golden_name));
    let rewritten = original.replacen(ANCHOR, &format!("name = \"{unique}\""), 1);
    std::fs::write(&manifest, rewritten)
        .map_err(|e| format!("cannot write {}: {e}", manifest.display()))?;
    Ok(unique)
}

/// Parse `cargo build --message-format=json` stdout for the produced binary.
fn find_executable(json_stdout: &str, unique_pkg: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for line in json_stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        let Some(exe) = value.get("executable").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let pkg_id = value
            .get("package_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if pkg_id.contains(unique_pkg) {
            found = Some(exe.to_owned());
        }
    }
    found
}

/// Build the emitted Rust project at `emitted_dir` into the shared cargo target
/// and run the resulting binary, returning its stdout + exit code.
///
/// This is the Result-returning core shared by the test harness (which wraps it
/// in test assertions) and the `refresh-oracle` tool (which uses it to capture
/// skyc's output on the Go-divergence path).
///
/// # Errors
/// Returns a message if the manifest cannot be retargeted, `cargo build` fails
/// (the message carries cargo's stderr), the binary cannot be located in the
/// JSON output, or the binary cannot be executed.
pub fn build_and_run_rust(golden_name: &str, emitted_dir: &Path) -> Result<RunResult, String> {
    let unique_pkg = rewrite_package_name(emitted_dir, golden_name)?;

    // No CARGO_TARGET_DIR override: the build inherits the global shared target
    // from ~/.cargo/config.toml, so deps are reused, not recompiled per golden.
    let build = Command::new("cargo")
        .arg("build")
        .arg("--message-format=json")
        .current_dir(emitted_dir)
        .output()
        .map_err(|e| format!("{golden_name}: failed to spawn `cargo build`: {e}"))?;
    if !build.status.success() {
        return Err(format!(
            "{golden_name}: emitted project must build\n--- cargo stderr ---\n{}",
            String::from_utf8_lossy(&build.stderr)
        ));
    }

    let json_stdout = String::from_utf8_lossy(&build.stdout);
    let exe = find_executable(&json_stdout, &unique_pkg).ok_or_else(|| {
        format!("{golden_name}: no `executable` artifact for package `{unique_pkg}` in cargo JSON")
    })?;

    let run = Command::new(&exe)
        .output()
        .map_err(|e| format!("{golden_name}: emitted binary `{exe}` must run: {e}"))?;
    Ok(RunResult {
        stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
        exit_code: run.status.code(),
    })
}

#[cfg(test)]
mod tests {
    use super::{Meta, ParityError, check_parity, sha256_hex};
    use std::path::PathBuf;

    /// A throwaway directory unique to one test, holding a synthetic golden.
    fn fresh_golden(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oracle_unit_{}_{}_{tag}",
            std::process::id(),
            // A monotonically-increasing counter so two tests with the same tag
            // in the same process never collide.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let created = std::fs::create_dir_all(&dir);
        assert!(created.is_ok(), "must create scratch golden dir");
        dir
    }

    fn write(dir: &std::path::Path, name: &str, body: &str) {
        let wrote = std::fs::write(dir.join(name), body);
        assert!(wrote.is_ok(), "must write {name}");
    }

    #[test]
    fn sha256_of_empty_is_the_known_constant() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn meta_round_trips_through_serialize_then_parse() {
        let meta = Meta {
            main_sky_sha256: "abc123".to_owned(),
            go_sky_version: "sky dev".to_owned(),
            exit_code: 0,
            oracle_divergence: false,
            divergence_reason: None,
        };
        let parsed = Meta::parse(&meta.serialize());
        assert_eq!(parsed.ok(), Some(meta));
    }

    #[test]
    fn meta_divergence_requires_a_reason() {
        let text =
            "main_sky_sha256 = x\ngo_sky_version = v\nexit_code = 0\noracle_divergence = true\n";
        let parsed = Meta::parse(text);
        assert!(
            parsed.is_err(),
            "divergence=true without a reason must fail"
        );
    }

    #[test]
    fn parity_matches_when_hash_and_stdout_agree() {
        let dir = fresh_golden("match");
        let src = "main = 1\n";
        write(&dir, "Main.sky", src);
        write(&dir, "expected_go.txt", "1\n");
        let meta = Meta {
            main_sky_sha256: sha256_hex(src.as_bytes()),
            go_sky_version: "sky dev".to_owned(),
            exit_code: 0,
            oracle_divergence: false,
            divergence_reason: None,
        };
        write(&dir, "oracle.meta", &meta.serialize());

        let outcome = check_parity(&dir, "match", "1\n");
        assert!(outcome.is_ok(), "matching parity must pass: {outcome:?}");
    }

    #[test]
    fn parity_fails_loudly_when_main_sky_changed_without_refresh() {
        let dir = fresh_golden("stale");
        // The cached hash is of the OLD source; Main.sky now holds new source.
        let meta = Meta {
            main_sky_sha256: sha256_hex(b"main = 1\n"),
            go_sky_version: "sky dev".to_owned(),
            exit_code: 0,
            oracle_divergence: false,
            divergence_reason: None,
        };
        write(&dir, "Main.sky", "main = 2\n");
        write(&dir, "expected_go.txt", "1\n");
        write(&dir, "oracle.meta", &meta.serialize());

        // Even though stdout would still match the stale expected, the staleness
        // gate must fire FIRST so we never diff against an out-of-date oracle.
        let outcome = check_parity(&dir, "stale", "1\n");
        assert!(
            matches!(outcome, Err(ParityError::Stale { .. })),
            "changed source must be a hard Stale failure, got {outcome:?}"
        );
    }

    #[test]
    fn parity_fails_when_oracle_meta_is_missing() {
        let dir = fresh_golden("nometa");
        write(&dir, "Main.sky", "main = 1\n");
        // No oracle.meta, no expected_go.txt — never a silent skip.
        let outcome = check_parity(&dir, "nometa", "1\n");
        assert!(
            matches!(outcome, Err(ParityError::MissingMeta { .. })),
            "a golden with no cached oracle must fail, got {outcome:?}"
        );
    }

    #[test]
    fn parity_uses_divergence_expected_when_go_is_buggy() {
        let dir = fresh_golden("divergence");
        let src = "main = 42\n";
        write(&dir, "Main.sky", src);
        // Go was buggy here, so expected holds skyc's CORRECT output.
        write(&dir, "expected_go.txt", "42\n");
        let meta = Meta {
            main_sky_sha256: sha256_hex(src.as_bytes()),
            go_sky_version: "sky dev".to_owned(),
            exit_code: 0,
            oracle_divergence: true,
            divergence_reason: Some("Go oracle panics on this shape".to_owned()),
        };
        write(&dir, "oracle.meta", &meta.serialize());

        // skyc's correct output matches the divergence expected → pass.
        assert!(check_parity(&dir, "divergence", "42\n").is_ok());

        // A wrong skyc output still fails, and the mismatch is flagged as
        // divergence-sourced (not "cached Go oracle").
        let bad = check_parity(&dir, "divergence", "WRONG\n");
        assert!(
            matches!(
                bad,
                Err(ParityError::Mismatch {
                    divergence: true,
                    ..
                })
            ),
            "divergence mismatch must be flagged as such, got {bad:?}"
        );
    }
}
