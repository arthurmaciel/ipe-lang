#![forbid(unsafe_code)]
//! Every crate the repository builds is held to the `mem_forget` deny.
//!
//! The root `[workspace.lints.clippy]` table is the lint SSOT, but a crate that
//! keeps its own `[lints]` table does not inherit it. Each workspace member
//! therefore either inherits the workspace table (`[lints] workspace = true`)
//! or mirrors `clippy::mem_forget = "deny"` and `rust::unsafe_code = "deny"` in
//! its own table. Each standalone crate in [`STANDALONE`] (its own
//! `[workspace]`, so never a member) mirrors `mem_forget` the same way.

use std::path::{Path, PathBuf};

/// Repository crates detached from the workspace, relative to the repo root.
const STANDALONE: &[&str] = &["tools/ipe-index/Cargo.toml", "editors/zed-ipe/Cargo.toml"];

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or(manifest_dir)
}

/// The parsed manifest at `path`; a read or TOML error names the path.
fn manifest(path: &Path) -> std::io::Result<toml::Table> {
    let text = std::fs::read_to_string(path)?;
    text.parse::<toml::Table>()
        .map_err(|e| std::io::Error::other(format!("{}: {e}", path.display())))
}

/// The level a lint table sets for `lint`, in either `lint = "deny"` or
/// `lint = { level = "deny", .. }` form.
fn level<'a>(table: Option<&'a toml::Value>, lint: &str) -> Option<&'a str> {
    match table?.get(lint)? {
        toml::Value::String(s) => Some(s.as_str()),
        toml::Value::Table(t) => t.get("level")?.as_str(),
        _ => None,
    }
}

/// Why `lints` fails to hold its crate to the required denies, if it does.
fn coverage_gap(lints: Option<&toml::Value>, require_unsafe: bool) -> Option<String> {
    let Some(lints) = lints else {
        return Some("no `[lints]` table".to_owned());
    };
    if lints.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
        return None;
    }
    if level(lints.get("clippy"), "mem_forget") != Some("deny") {
        return Some("own `[lints.clippy]` lacks `mem_forget = \"deny\"`".to_owned());
    }
    if require_unsafe && level(lints.get("rust"), "unsafe_code") != Some("deny") {
        return Some("own `[lints.rust]` lacks `unsafe_code = \"deny\"`".to_owned());
    }
    None
}

#[test]
fn the_workspace_table_denies_mem_forget() {
    let root = manifest(&repo_root().join("Cargo.toml")).expect("the root manifest parses");
    let clippy = root
        .get("workspace")
        .and_then(|w| w.get("lints"))
        .and_then(|l| l.get("clippy"));
    assert_eq!(level(clippy, "mem_forget"), Some("deny"));
}

#[test]
fn every_member_inherits_or_mirrors_the_denies() {
    let root_dir = repo_root();
    let root = manifest(&root_dir.join("Cargo.toml")).expect("the root manifest parses");
    let members: Vec<&str> = root
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(toml::Value::as_array)
        .map(|a| a.iter().filter_map(toml::Value::as_str).collect())
        .unwrap_or_default();
    assert!(!members.is_empty(), "the root manifest lists no members");
    let gaps: Vec<String> = members
        .iter()
        .filter_map(|m| {
            let path = root_dir.join(m).join("Cargo.toml");
            coverage_gap(
                manifest(&path)
                    .expect("a member manifest parses")
                    .get("lints"),
                true,
            )
            .map(|g| format!("{m}: {g}"))
        })
        .collect();
    assert!(gaps.is_empty(), "members escaping the lint SSOT: {gaps:#?}");
}

#[test]
fn every_standalone_crate_mirrors_mem_forget() {
    let root_dir = repo_root();
    let gaps: Vec<String> = STANDALONE
        .iter()
        .filter_map(|m| {
            let table = manifest(&root_dir.join(m)).expect("a standalone manifest parses");
            assert!(
                table.contains_key("workspace"),
                "{m} is no longer standalone"
            );
            coverage_gap(table.get("lints"), false).map(|g| format!("{m}: {g}"))
        })
        .collect();
    assert!(
        gaps.is_empty(),
        "standalone crates escaping the deny: {gaps:#?}"
    );
}

#[test]
fn an_own_table_without_the_deny_is_refused() {
    let own: toml::Table =
        "[lints.clippy]\nunwrap_used = \"allow\"\n[lints.rust]\nunsafe_code = \"deny\"\n"
            .parse()
            .expect("the fixture is valid TOML");
    assert!(coverage_gap(own.get("lints"), true).is_some());
    let inherits: toml::Table = "[lints]\nworkspace = true\n"
        .parse()
        .expect("the fixture is valid TOML");
    assert!(coverage_gap(inherits.get("lints"), true).is_none());
    let table_form: toml::Table =
        "[lints.clippy]\nmem_forget = { level = \"deny\", priority = 1 }\n[lints.rust]\nunsafe_code = \"deny\"\n"
            .parse()
            .expect("the fixture is valid TOML");
    assert!(coverage_gap(table_form.get("lints"), true).is_none());
    assert!(coverage_gap(None, true).is_some());
}
