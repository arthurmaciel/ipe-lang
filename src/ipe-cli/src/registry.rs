//! The registry Pages read-path: an HTTP fast-path over the static
//! `ipe-registry` GitHub Pages API, with a git-checkout fallback.
//!
//! The authoritative registry is a git repository (`arthurmaciel/ipe-registry`)
//! holding one `packages/<name>.toml` per package and one advisory TOML per
//! advisory. A `pages.yml` Action mirrors that tree to a static JSON read API at
//! `IPE_REGISTRY_URL` (default `https://arthurmaciel.github.io/ipe-registry`):
//! `/packages/<name>.json`, `/advisories/index.json`, `/advisories/<id>.json`.
//!
//! # The fast-path is an optimisation, never the trust root
//!
//! Reading the per-package JSON lets `ipe add` discover an entry without cloning
//! the whole index. But the resolved entry's pinned `rev` + `sha256` remain the
//! trust root: the resolver still git-fetches the pinned revision and verifies
//! the fetched tree's content hash before anything is written
//! ([`crate::resolve::resolve_and_add`]). The Pages JSON only decides *which*
//! version to fetch; it cannot make the resolver trust bytes it did not hash.
//!
//! # Fall back, never trust a partial
//!
//! Any network failure (offline, air-gapped, DNS, TLS, non-2xx, timeout) OR a
//! malformed/partial JSON response falls back to the git-checkout reader
//! ([`crate::index::read_entry`] against [`crate::resolve::index_root`], the
//! `IPE_INDEX_DIR` path). A partial Pages response is NEVER trusted as a complete
//! entry — the JSON is parsed through the same typed constructors as the TOML
//! path, so a missing or injection-shaped field is a hard error that triggers the
//! fallback rather than a silently-truncated entry.

use std::path::Path;

use crate::CliError;
use crate::advisory::Advisory;
use crate::index::{self, IndexEntry};

/// The environment variable naming the registry's static Pages read API base URL.
const REGISTRY_URL_ENV: &str = "IPE_REGISTRY_URL";

/// The default registry Pages read API base URL when [`REGISTRY_URL_ENV`] is
/// unset — the GitHub Pages site generated from `arthurmaciel/ipe-registry`.
const DEFAULT_REGISTRY_URL: &str = "https://arthurmaciel.github.io/ipe-registry";

/// The `Accept` media type for the static JSON read API.
const JSON_ACCEPT: &str = "application/json";

/// The registry Pages base URL: `IPE_REGISTRY_URL` when set (trailing slashes
/// trimmed), else [`DEFAULT_REGISTRY_URL`].
///
/// An explicitly-empty `IPE_REGISTRY_URL` disables the HTTP fast-path (the base
/// is empty, so the fast-path declines every request and the caller falls back
/// to the git checkout) — the air-gapped opt-out that keeps `ipe add` working
/// with no network attempt.
#[must_use]
pub fn registry_base_url() -> String {
    std::env::var(REGISTRY_URL_ENV).map_or_else(
        |_| DEFAULT_REGISTRY_URL.to_owned(),
        |value| value.trim_end_matches('/').to_owned(),
    )
}

/// A fetcher: given an absolute URL, return the response body, or `None`.
///
/// `None` signals any failure (offline, DNS, TLS, non-2xx, timeout). The
/// production fetcher is [`net_fetch`]; tests inject a stub so the read-path and
/// fallback logic are exercised without real HTTP.
pub type Fetch<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The production fetcher: a blocking JSON GET through [`crate::net`].
#[must_use]
pub fn net_fetch(url: &str) -> Option<String> {
    crate::net::get_with_accept(url, JSON_ACCEPT)
}

/// Read the index entry for `name`, preferring the Pages HTTP fast-path and
/// falling back to the git-checkout reader on any network failure, air-gap, or
/// malformed response.
///
/// This is the resolver's entry read-path. The returned [`IndexEntry`] carries
/// pinned `rev` + `sha256` per version — the trust root the resolver
/// hash-verifies after fetching, unchanged by which source produced the entry.
///
/// # Errors
/// [`CliError::Resolve`] when BOTH the fast-path declines (or is malformed) AND
/// the git-checkout reader cannot produce the entry — the same error the
/// git-only reader would have returned, so an absent package is still an honest
/// "not in the index".
pub fn read_entry_via_pages(name: &str, index_root: &Path) -> Result<IndexEntry, CliError> {
    read_entry_via_pages_with(name, index_root, &registry_base_url(), &net_fetch)
}

/// The testable core of [`read_entry_via_pages`].
///
/// The base URL and fetcher are injected so a stub can exercise the fast-path,
/// the malformed-response fallback, and the network-failure fallback without
/// real HTTP.
///
/// # Errors
/// [`CliError::Resolve`] when the fast-path declines (or is malformed) AND the
/// git-checkout reader cannot produce the entry.
pub fn read_entry_via_pages_with(
    name: &str,
    index_root: &Path,
    base_url: &str,
    fetch: Fetch<'_>,
) -> Result<IndexEntry, CliError> {
    if let Some(entry) = try_pages_entry(name, base_url, fetch) {
        return Ok(entry);
    }
    // Fall back to the reproducible git-checkout source of truth. A partial or
    // absent Pages response never short-circuits this — the checkout reader's
    // verdict (Present / absent-is-error) is authoritative on the fallback path.
    index::read_entry(index_root, name)
}

/// Attempt the Pages fast-path for `name`, returning the parsed entry only when
/// the fetch succeeds AND the JSON parses through the typed constructors.
///
/// `None` on an empty base URL (fast-path disabled), a fetch failure, or a
/// malformed/partial response — every case a fallback trigger, never a partial
/// trusted as complete.
fn try_pages_entry(name: &str, base_url: &str, fetch: Fetch<'_>) -> Option<IndexEntry> {
    if base_url.is_empty() {
        return None;
    }
    let url = format!("{base_url}/packages/{name}.json");
    let body = fetch(&url)?;
    // A malformed/partial JSON is discarded (→ fallback), never trusted.
    index::parse_entry_json(name, &body).ok()
}

/// The outcome of fetching advisories for one package over the Pages read-path.
///
/// The three variants keep "clean", "vulnerable-or-warned", and "DB unreachable"
/// distinct so the caller can enforce the fail-closed policy: an unreachable DB
/// is a loud WARN, never a silent all-clear.
#[derive(Debug)]
pub enum PagesAdvisoryOutcome {
    /// The advisory DB was reachable and every fetched advisory for the package
    /// was parsed. The vector holds the advisories whose range must still be
    /// tested against the locked version (possibly empty = no advisories).
    Fetched(Vec<Advisory>),
    /// The advisory index could not be fetched (offline, air-gapped, non-2xx).
    /// The caller WARNs and falls back to the git-checkout DB; it is NEVER
    /// treated as "no advisories → clean".
    Unreachable,
}

/// Fetch the advisories affecting `pkg_name` from the Pages advisory read-path.
///
/// Fetches `/advisories/index.json` (the id → package catalogue), selects the
/// ids whose `package` equals `pkg_name`, fetches each `/advisories/<id>.json`,
/// and parses it through [`crate::advisory::parse_advisory_json`].
///
/// **Fail-closed:** a malformed advisory index or a malformed per-advisory record
/// is a hard [`CliError::AdvisoryDbMalformed`] — a corrupt DB is never treated as
/// "clean". An unreachable index (network failure) returns
/// [`PagesAdvisoryOutcome::Unreachable`] so the caller can WARN and fall back,
/// rather than silently passing.
///
/// # Errors
/// [`CliError::AdvisoryDbMalformed`] when the index or an advisory record is
/// present but malformed.
pub fn fetch_advisories_via_pages(pkg_name: &str) -> Result<PagesAdvisoryOutcome, CliError> {
    fetch_advisories_via_pages_with(pkg_name, &registry_base_url(), &net_fetch)
}

/// The testable core of [`fetch_advisories_via_pages`].
///
/// The base URL and fetcher are injected so the reachable, unreachable, and
/// malformed paths are exercised without real HTTP.
///
/// # Errors
/// [`CliError::AdvisoryDbMalformed`] when the advisory index or a per-advisory
/// record is present but malformed.
pub fn fetch_advisories_via_pages_with(
    pkg_name: &str,
    base_url: &str,
    fetch: Fetch<'_>,
) -> Result<PagesAdvisoryOutcome, CliError> {
    if base_url.is_empty() {
        return Ok(PagesAdvisoryOutcome::Unreachable);
    }
    let index_url = format!("{base_url}/advisories/index.json");
    let Some(index_body) = fetch(&index_url) else {
        // Network failure / air-gap: unreachable, NOT clean.
        return Ok(PagesAdvisoryOutcome::Unreachable);
    };

    // A present-but-malformed index is fail-closed: refuse, never treat as empty.
    let ids = crate::advisory::parse_advisory_index_json(&index_body, pkg_name)?;

    let mut advisories = Vec::with_capacity(ids.len());
    for id in ids {
        let record_url = format!("{base_url}/advisories/{id}.json");
        let Some(record_body) = fetch(&record_url) else {
            // The index named this advisory but the record is unreachable. The
            // index proved an advisory exists, so we cannot claim the dep clean:
            // fail closed (unreachable), not silent-pass.
            return Ok(PagesAdvisoryOutcome::Unreachable);
        };
        advisories.push(crate::advisory::parse_advisory_json(&record_body, &id)?);
    }
    Ok(PagesAdvisoryOutcome::Fetched(advisories))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::advisory::Severity;

    const FIXTURE_REV: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

    fn temp_index(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe-registry-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("packages")).expect("packages dir");
        dir
    }

    fn write_git_entry(root: &Path, name: &str, version: &str) {
        let text = format!(
            "name = \"{name}\"\npublisher = \"git-source\"\n\n[[version]]\n\
             version = \"{version}\"\nsource = \"https://example.invalid/{name}\"\n\
             rev = \"{FIXTURE_REV}\"\nsha256 = \"00\"\ncapabilities = [\"network\"]\n"
        );
        std::fs::write(root.join("packages").join(format!("{name}.toml")), text)
            .expect("write git entry");
    }

    fn pages_entry_json(name: &str, publisher: &str, version: &str) -> String {
        format!(
            r#"{{ "name": "{name}", "publisher": "{publisher}",
                  "versions": [ {{ "version": "{version}",
                                   "source": "https://example.invalid/{name}",
                                   "rev": "{FIXTURE_REV}", "sha256": "00",
                                   "capabilities": ["network"] }} ] }}"#
        )
    }

    // ── Pages read-path ───────────────────────────────────────────────────────

    #[test]
    fn pages_fast_path_resolves_from_http() {
        // A healthy Pages response resolves without touching the git checkout.
        let root = temp_index("pages-ok");
        // No git entry on disk — proving the fast-path did the work.
        let body = pages_entry_json("http-extras", "pages-source", "1.2.0");
        let fetch = move |url: &str| {
            assert!(url.ends_with("/packages/http-extras.json"), "url: {url}");
            Some(body.clone())
        };
        let entry = read_entry_via_pages_with("http-extras", &root, "https://reg.example", &fetch)
            .expect("fast-path resolves");
        assert_eq!(entry.publisher, "pages-source");
        assert_eq!(entry.versions.len(), 1);
        let first = entry.versions.first().expect("one version");
        assert_eq!(first.version.to_string(), "1.2.0");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn network_failure_falls_back_to_git() {
        // The fetcher returns None (offline). The git-checkout entry is used.
        let root = temp_index("net-fail");
        write_git_entry(&root, "http-extras", "0.9.0");
        let fetch = |_url: &str| None;
        let entry = read_entry_via_pages_with("http-extras", &root, "https://reg.example", &fetch)
            .expect("git fallback resolves");
        assert_eq!(entry.publisher, "git-source");
        let first = entry.versions.first().expect("one version");
        assert_eq!(first.version.to_string(), "0.9.0");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn malformed_pages_json_falls_back_to_git_not_trusted() {
        // A partial/truncated Pages response is NOT trusted as complete: the
        // git-checkout entry is used instead of a half-parsed one.
        let root = temp_index("malformed");
        write_git_entry(&root, "http-extras", "0.9.0");
        // Missing `versions` — a partial mirror the fast-path must discard.
        let fetch =
            |_url: &str| Some(r#"{ "name": "http-extras", "publisher": "attacker" }"#.to_owned());
        let entry = read_entry_via_pages_with("http-extras", &root, "https://reg.example", &fetch)
            .expect("git fallback resolves");
        assert_eq!(
            entry.publisher, "git-source",
            "the malformed Pages entry must not be trusted; git is authoritative"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn injection_shaped_pages_rev_is_rejected_and_falls_back() {
        // A Pages JSON with a non-immutable `rev` fails the typed constructor and
        // triggers the fallback — the JSON path enforces the same trust boundary
        // as the TOML path.
        let root = temp_index("bad-rev");
        write_git_entry(&root, "http-extras", "0.9.0");
        // A non-immutable `rev` ("main") the typed `PinnedRev` constructor rejects.
        let body = r#"{ "name": "http-extras", "publisher": "attacker",
                  "versions": [ { "version": "1.0.0",
                                   "source": "https://example.invalid/x",
                                   "rev": "main", "sha256": "00" } ] }"#;
        let fetch = move |_url: &str| Some(body.to_owned());
        let entry = read_entry_via_pages_with("http-extras", &root, "https://reg.example", &fetch)
            .expect("git fallback resolves");
        assert_eq!(entry.publisher, "git-source");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_base_url_disables_fast_path() {
        // An explicitly-empty base URL skips HTTP entirely (air-gapped opt-out)
        // and reads straight from the git checkout.
        let root = temp_index("empty-base");
        write_git_entry(&root, "http-extras", "0.9.0");
        // Record whether the fetcher was ever consulted: with an empty base URL
        // the fast-path must be skipped entirely, so this stays false.
        let called = std::cell::Cell::new(false);
        let fetch = |_url: &str| {
            called.set(true);
            None
        };
        let entry = read_entry_via_pages_with("http-extras", &root, "", &fetch)
            .expect("git fallback resolves");
        assert!(
            !called.get(),
            "fetch must not run when the base URL is empty"
        );
        assert_eq!(entry.publisher, "git-source");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn absent_everywhere_is_an_honest_error() {
        // No Pages, no git entry — the same "not in the index" error the git-only
        // reader returns, not a spurious success.
        let root = temp_index("absent");
        let fetch = |_url: &str| None;
        let err = read_entry_via_pages_with("ghost", &root, "https://reg.example", &fetch)
            .expect_err("absent everywhere is an error");
        assert!(format!("{err}").contains("ghost"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Advisory fetch ────────────────────────────────────────────────────────

    fn advisory_record_json(id: &str, pkg: &str, severity: &str, affected: &str) -> String {
        format!(
            r#"{{ "id": "{id}", "package": "{pkg}", "severity": "{severity}",
                  "affected": "{affected}", "description": "Test advisory." }}"#
        )
    }

    #[test]
    fn advisory_in_range_is_fetched_and_flagged() {
        // The index names one advisory for the package; its record is fetched and
        // its range covers the locked version → surfaced to the caller.
        let index_json =
            r#"{ "advisories": [ { "id": "IPE-2024-0001", "package": "http-client" } ] }"#;
        let record =
            advisory_record_json("IPE-2024-0001", "http-client", "high", ">=1.0.0, <1.2.0");
        let fetch = move |url: &str| {
            if url.ends_with("/advisories/index.json") {
                Some(index_json.to_owned())
            } else if url.ends_with("/advisories/IPE-2024-0001.json") {
                Some(record.clone())
            } else {
                None
            }
        };
        let outcome = fetch_advisories_via_pages_with("http-client", "https://reg.example", &fetch)
            .expect("advisory fetch succeeds");
        let advisories = fetched(outcome);
        assert_eq!(advisories.len(), 1);
        let adv = advisories.first().expect("one advisory");
        assert_eq!(adv.severity, Severity::High);
        let locked = semver::Version::parse("1.1.0").expect("valid version");
        assert!(adv.affected.matches(&locked), "1.1.0 must be in range");
    }

    #[test]
    fn advisory_index_unreachable_is_warn_not_clean() {
        // The index fetch fails (offline). The outcome is Unreachable so the
        // caller WARNs and falls back — never a silent all-clear.
        let fetch = |_url: &str| None;
        let outcome = fetch_advisories_via_pages_with("http-client", "https://reg.example", &fetch)
            .expect("unreachable is not an error");
        assert!(
            matches!(outcome, PagesAdvisoryOutcome::Unreachable),
            "an unreachable advisory DB must be Unreachable, not Fetched(empty)"
        );
    }

    #[test]
    fn malformed_advisory_index_is_fail_closed() {
        // A present-but-malformed index is a hard error, never treated as empty.
        let fetch = |url: &str| {
            if url.ends_with("/advisories/index.json") {
                Some("this is not json ===".to_owned())
            } else {
                None
            }
        };
        let err = fetch_advisories_via_pages_with("http-client", "https://reg.example", &fetch)
            .expect_err("malformed index must be an error");
        assert!(
            matches!(err, CliError::AdvisoryDbMalformed { .. }),
            "expected AdvisoryDbMalformed, got {err:?}"
        );
    }

    #[test]
    fn advisory_record_unreachable_after_index_is_fail_closed() {
        // The index proves an advisory exists but the record cannot be fetched.
        // We cannot claim the dep clean → Unreachable (fail closed), not empty.
        let index_json =
            r#"{ "advisories": [ { "id": "IPE-2024-0001", "package": "http-client" } ] }"#;
        let fetch = move |url: &str| {
            if url.ends_with("/advisories/index.json") {
                Some(index_json.to_owned())
            } else {
                None
            }
        };
        let outcome = fetch_advisories_via_pages_with("http-client", "https://reg.example", &fetch)
            .expect("record-unreachable is not an error");
        assert!(
            matches!(outcome, PagesAdvisoryOutcome::Unreachable),
            "a named-but-unreachable record must be Unreachable, not Fetched(empty)"
        );
    }

    #[test]
    fn advisory_index_omits_other_packages() {
        // The index lists advisories for another package only; ours is clean.
        let index_json =
            r#"{ "advisories": [ { "id": "IPE-2024-0009", "package": "other-pkg" } ] }"#;
        let fetch = move |url: &str| {
            if url.ends_with("/advisories/index.json") {
                Some(index_json.to_owned())
            } else {
                None
            }
        };
        let outcome = fetch_advisories_via_pages_with("http-client", "https://reg.example", &fetch)
            .expect("advisory fetch succeeds");
        let advisories = fetched(outcome);
        assert!(
            advisories.is_empty(),
            "no advisory for our package → empty, and the reachable index proves it clean"
        );
    }

    /// Extract the [`PagesAdvisoryOutcome::Fetched`] payload, failing the test
    /// with a readable assertion on any other variant. The `else` arm is dead
    /// after the assert; it returns an empty vec purely to satisfy the type
    /// without a `panic!`/`unreachable!` macro (the workspace forbids both).
    fn fetched(outcome: PagesAdvisoryOutcome) -> Vec<Advisory> {
        assert!(
            matches!(outcome, PagesAdvisoryOutcome::Fetched(_)),
            "expected Fetched, got Unreachable"
        );
        let PagesAdvisoryOutcome::Fetched(advisories) = outcome else {
            return Vec::new();
        };
        advisories
    }
}
