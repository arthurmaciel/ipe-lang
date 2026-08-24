//! Contained-relative-path newtype: a path proven to resolve strictly inside a
//! given project root.
//!
//! The single constructor [`ContainedRelPath::parse`] rejects any `..`
//! component, any absolute prefix, and any path that canonicalises to a
//! location outside the project root. A value of this type carries the proof
//! that the path it wraps does not escape the project directory, so downstream
//! traversal never re-encounters an unvalidated path.
//!
//! This type closes the `sourceRoot`-escape defect class: a manifest
//! `sourceRoot` that resolves outside the project directory is unrepresentable
//! past the manifest boundary — the escape is a typed [`PathEscape`] error,
//! never a silent traversal.

use std::path::{Component, Path, PathBuf};

/// Why a candidate path was rejected by [`ContainedRelPath::parse`].
#[derive(Debug, PartialEq, Eq)]
pub enum PathEscape {
    /// The path contains a `..` component. Even a benign-looking
    /// `src/../../src` is rejected at the component level before touching
    /// the filesystem.
    ParentTraversal,
    /// The path has an absolute prefix (starts with `/`, a drive letter, or a
    /// UNC root). An absolute prefix reroots the join entirely outside the
    /// base, discarding the project root silently.
    Absolute,
    /// The path passes the component check but, after canonicalisation against
    /// `root`, its resolved form is not a descendant of the canonicalised root.
    /// Covers exotic escapes via symlinks pointing outside the project tree.
    NotUnderRoot,
}

impl std::fmt::Display for PathEscape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParentTraversal => f.write_str(
                "the path contains a `..` component and would escape the project directory",
            ),
            Self::Absolute => {
                f.write_str("the path is absolute and would reroot outside the project directory")
            }
            Self::NotUnderRoot => f.write_str(
                "the path resolves outside the project directory \
                 (possibly via a symlink that points above the project root)",
            ),
        }
    }
}

/// A path proven to resolve strictly inside a given project root.
///
/// The only constructor is [`ContainedRelPath::parse`]; every value has already
/// been checked against its root. Use [`ContainedRelPath::resolved`] to obtain
/// the contained absolute path for filesystem operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainedRelPath(PathBuf);

impl ContainedRelPath {
    /// Parse a manifest-supplied relative path against `root`.
    ///
    /// Rejects any `..` or absolute component before touching the filesystem,
    /// then canonicalises the joined path and asserts the result is a
    /// descendant of the canonicalised `root`. Returns [`PathEscape`] on any
    /// violation — never a silent traversal outside the project.
    ///
    /// # Errors
    ///
    /// - [`PathEscape::Absolute`] if `raw` has an absolute prefix.
    /// - [`PathEscape::ParentTraversal`] if `raw` contains a `..` component.
    /// - [`PathEscape::NotUnderRoot`] if the canonicalised result is not under
    ///   the canonicalised root (symlink escape, exotic OS path, etc.). When
    ///   `canonicalize` fails (the path does not exist yet), the un-canonicalised
    ///   joined path is checked with [`Path::starts_with`] as a conservative
    ///   fallback — fail-closed.
    pub fn parse(root: &Path, raw: &str) -> Result<Self, PathEscape> {
        let candidate = Path::new(raw);

        // Component-level pre-check: reject before any filesystem call.
        for component in candidate.components() {
            match component {
                Component::RootDir | Component::Prefix(_) => {
                    return Err(PathEscape::Absolute);
                }
                Component::ParentDir => {
                    return Err(PathEscape::ParentTraversal);
                }
                Component::CurDir | Component::Normal(_) => {}
            }
        }

        let joined = root.join(candidate);

        let canon_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());

        // Canonicalise the joined path; if it does not exist yet, fall back to
        // a structural starts_with check — the component scan above already
        // rejected `..` and absolute prefixes.
        let Ok(canon_joined) = std::fs::canonicalize(&joined) else {
            if joined.starts_with(&canon_root) {
                return Ok(Self(joined));
            }
            return Err(PathEscape::NotUnderRoot);
        };

        if !canon_joined.starts_with(&canon_root) {
            return Err(PathEscape::NotUnderRoot);
        }

        Ok(Self(canon_joined))
    }

    /// The resolved absolute path, guaranteed to lie under the root it was
    /// parsed with.
    #[must_use]
    pub fn resolved(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fresh temp dir with an `src/` subdirectory and return the root.
    fn fresh_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ipe_crp_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).expect("create src/");
        root
    }

    // ── Happy-path ────────────────────────────────────────────────────────────

    #[test]
    fn accepts_in_tree_relative_path() {
        let root = fresh_root("accept");
        let result = ContainedRelPath::parse(&root, "src");
        let _ = std::fs::remove_dir_all(&root);
        let crp = result.expect("'src' is in-tree and must be accepted");
        assert!(crp.resolved().starts_with(&root));
    }

    #[test]
    fn accepts_nested_in_tree_path() {
        let root = fresh_root("nested");
        std::fs::create_dir_all(root.join("src/Lib")).expect("create Lib/");
        let result = ContainedRelPath::parse(&root, "src/Lib");
        let _ = std::fs::remove_dir_all(&root);
        result.expect("nested in-tree path must be accepted");
    }

    #[test]
    fn accepts_dot_as_root_itself() {
        let root = fresh_root("dot");
        let result = ContainedRelPath::parse(&root, ".");
        let _ = std::fs::remove_dir_all(&root);
        result.expect("'.' resolves to the root and must be accepted");
    }

    // ── Refusals ──────────────────────────────────────────────────────────────

    /// A manifest `sourceRoot = "../.."` must be a typed refusal, never a
    /// traversal outside the project directory. This proves the primary escape
    /// vector is unrepresentable past the manifest boundary.
    #[test]
    fn rejects_parent_traversal_dotdot() {
        let root = fresh_root("dotdot");
        let result = ContainedRelPath::parse(&root, "../..");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            result.expect_err("'../..' must be rejected"),
            PathEscape::ParentTraversal,
            "a `..` component must yield ParentTraversal"
        );
    }

    #[test]
    fn rejects_single_dotdot() {
        let root = fresh_root("single_dotdot");
        let result = ContainedRelPath::parse(&root, "..");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            result.expect_err("'..' must be rejected"),
            PathEscape::ParentTraversal
        );
    }

    #[test]
    fn rejects_absolute_path_unix() {
        let root = fresh_root("abs");
        let result = ContainedRelPath::parse(&root, "/etc/passwd");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            result.expect_err("absolute path must be rejected"),
            PathEscape::Absolute,
            "an absolute path must yield Absolute"
        );
    }

    #[test]
    fn rejects_embedded_dotdot() {
        // `src/../../secret` embeds a `..` that escapes through the root.
        let root = fresh_root("embedded_dotdot");
        let result = ContainedRelPath::parse(&root, "src/../../secret");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            result.expect_err("embedded '../../' must be rejected"),
            PathEscape::ParentTraversal
        );
    }

    /// A symlink in the project tree that points outside the project root must
    /// be caught by the canonicalisation step and yield `NotUnderRoot`.
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_via_not_under_root() {
        let root = fresh_root("symlink_escape");
        // Create a symlink inside the project pointing to a location outside it.
        let link = root.join("escape");
        std::os::unix::fs::symlink("/tmp", &link).expect("create symlink");
        let result = ContainedRelPath::parse(&root, "escape");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            result.expect_err("symlink pointing outside root must be rejected"),
            PathEscape::NotUnderRoot,
            "a symlink escaping the project root must yield NotUnderRoot"
        );
    }

    // ── PathEscape Display ────────────────────────────────────────────────────

    #[test]
    fn path_escape_display_is_human_readable() {
        let msgs = [
            PathEscape::ParentTraversal.to_string(),
            PathEscape::Absolute.to_string(),
            PathEscape::NotUnderRoot.to_string(),
        ];
        for m in &msgs {
            assert!(!m.is_empty(), "PathEscape display must not be empty");
        }
    }
}
