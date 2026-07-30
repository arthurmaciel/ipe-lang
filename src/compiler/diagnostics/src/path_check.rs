//! Compile-time path validation shared between the compiler's canonicaliser
//! and the runtime's `path_from_string` boundary.
//!
//! This module contains the two predicates that constitute the `path "…"`
//! literal gate (IPE-P0063): a NUL-byte check and the `..`-traversal check
//! over the lexically-cleaned form. The same logic lives in
//! `ipe_runtime::path` (driving the runtime `Path.fromString` seal); this
//! copy in `ipe_diagnostics` lets the compiler's canon stage validate a
//! `path "…"` literal at compile time WITHOUT pulling in the full
//! `ipe_runtime` crate (which carries heavy optional dependencies: tokio,
//! serde, sqlx, …).
//!
//! # SSOT contract
//! The `clean` and `escapes_root` functions here MUST remain identical to
//! `ipe_runtime::path::{clean, escapes_root}`. A divergence would mean the
//! compiler accepts a literal that the runtime rejects, or vice versa — a
//! correctness hole. The two copies are guarded by `test_mirrors_runtime` in
//! the test block: any logic change must be applied to both files and the
//! mirror test re-verified.

const SEP: u8 = b'/';

/// Compile-time validation for a `path "…"` literal. Same predicate as
/// `ipe_runtime::path::path_from_string`: NUL-free AND non-escaping.
///
/// Returns the lexically-cleaned path string on success, or a `&'static str`
/// reason code on failure:
/// - `"nul"` — the string contains a NUL byte.
/// - `"traversal"` — the cleaned form is `".."` or starts with `"../"`.
///
/// # Errors
///
/// Returns `Err("nul")` when `s` contains a NUL byte, or `Err("traversal")`
/// when the lexically-cleaned form escapes its root via `..`.
///
/// # Examples (not runnable — illustrative only)
///
///     validate("src/Main.ipe")   // Ok("src/Main.ipe")
///     validate("../etc/passwd")  // Err("traversal")
///     validate("a\0b")           // Err("nul")
pub fn validate(s: &str) -> Result<String, &'static str> {
    if s.as_bytes().contains(&0) {
        return Err("nul");
    }
    let cleaned = clean(s);
    if escapes_root(&cleaned) {
        return Err("traversal");
    }
    Ok(cleaned)
}

/// Does a CLEANED, relative path climb above its base? True when the whole
/// path is `..` or it begins with `../`.
///
/// SSOT mirror of `ipe_runtime::path::escapes_root`.
pub(crate) fn escapes_root(cleaned: &str) -> bool {
    cleaned == ".." || cleaned.starts_with("../")
}

/// Faithful port of Go `path/filepath.Clean` (Unix).
///
/// SSOT mirror of `ipe_runtime::path::clean`.
pub(crate) fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let b = path.as_bytes();
    let n = b.len();
    let at = |i: usize| -> Option<u8> { b.get(i).copied() };
    let rooted = at(0) == Some(SEP);
    let mut out: Vec<u8> = Vec::with_capacity(n + 1);
    // `dotdot` is the write-fence below which `..` segments cannot pop.
    let mut dotdot = if rooted {
        out.push(SEP);
        1
    } else {
        0
    };
    let mut r = usize::from(rooted);
    while r < n {
        // Skip redundant separator or lone `.` — both advance one byte.
        let is_sep = at(r) == Some(SEP);
        let is_dot_only = at(r) == Some(b'.') && (r + 1 == n || at(r + 1) == Some(SEP));
        if is_sep || is_dot_only {
            r += 1;
        } else if at(r) == Some(b'.')
            && at(r + 1) == Some(b'.')
            && (r + 2 == n || at(r + 2) == Some(SEP))
        {
            // `..` segment.
            r += 2;
            if out.len() > dotdot {
                let mut w = out.len() - 1;
                while w > dotdot && out.get(w).copied() != Some(SEP) {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                if !out.is_empty() {
                    out.push(SEP);
                }
                out.push(b'.');
                out.push(b'.');
                dotdot = out.len();
            }
        } else {
            // Normal path component.
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(SEP);
            }
            while let Some(&c) = b.get(r) {
                if c == SEP {
                    break;
                }
                out.push(c);
                r += 1;
            }
        }
    }
    if out.is_empty() {
        return ".".to_string();
    }
    String::from_utf8(out).unwrap_or_else(|_| ".".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate: accepted paths ─────────────────────────────────────────────

    #[test]
    fn plain_relative_accepted() {
        assert_eq!(validate("src/Main.ipe"), Ok("src/Main.ipe".to_string()));
    }

    #[test]
    fn absolute_accepted() {
        assert_eq!(
            validate("/usr/share/data"),
            Ok("/usr/share/data".to_string())
        );
    }

    #[test]
    fn interior_dotdot_that_stays_in_bounds_accepted() {
        assert_eq!(validate("a/b/../c"), Ok("a/c".to_string()));
    }

    #[test]
    fn rooted_dotdot_cannot_escape_accepted() {
        assert_eq!(validate("/a/../../b"), Ok("/b".to_string()));
    }

    #[test]
    fn empty_cleans_to_dot() {
        assert_eq!(validate(""), Ok(".".to_string()));
    }

    // ── validate: rejected paths ─────────────────────────────────────────────

    #[test]
    fn nul_byte_rejected() {
        assert_eq!(validate("safe\0bad"), Err("nul"));
    }

    #[test]
    fn leading_dotdot_rejected() {
        assert_eq!(validate("../secret"), Err("traversal"));
    }

    #[test]
    fn bare_dotdot_rejected() {
        assert_eq!(validate(".."), Err("traversal"));
    }

    #[test]
    fn dotdot_that_resolves_to_escape_rejected() {
        // "a/../../etc" cleans to "../etc"
        assert_eq!(validate("a/../../etc"), Err("traversal"));
    }

    // ── clean mirrors ────────────────────────────────────────────────────────

    #[test]
    fn clean_collapses_repeated_separators() {
        assert_eq!(clean("a//b///c"), "a/b/c");
    }

    #[test]
    fn clean_empty_gives_dot() {
        assert_eq!(clean(""), ".");
    }
}
