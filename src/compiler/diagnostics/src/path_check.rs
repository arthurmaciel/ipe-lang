//! Compile-time validation for the `path "…"` literal gate (IPE-P0063).
//!
//! The algorithm is NOT defined here — it lives once in the dependency-free
//! `ipe_path_core` crate, which the runtime `Path.fromString` seal
//! (`ipe_runtime::path`) also consumes. This module is the compiler's thin
//! entry point onto that single source of truth, so the two sites can never
//! drift.
//!
//! [`validate`] is the all-targets compile-time gate: because the compiler does
//! not know the final target OS, it rejects any path that would traverse under
//! EITHER the Unix (`/`) or the Windows (`\`/`/`) separator regime. That is
//! stricter than the runtime's per-target seal by construction, so a literal the
//! compiler accepts is accepted by the runtime on every target.

/// Compile-time validation for a `path "…"` literal — the all-targets gate.
///
/// Delegates to [`ipe_path_core::validate`]. Returns the cleaned path string on
/// success, or a [`ipe_path_core::PathRejection`] that the canon stage renders
/// into an `InvalidPathLiteral` diagnostic.
///
/// # Errors
///
/// Returns `Err(PathRejection::Nul)` for a NUL byte, or
/// `Err(PathRejection::Traversal)` for any `..` escape (under either separator
/// regime) or Windows trailing-dot/space `..` disguise.
pub fn validate(s: &str) -> Result<String, ipe_path_core::PathRejection> {
    ipe_path_core::validate(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── accepted paths ───────────────────────────────────────────────────────

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

    // ── rejected under the Unix regime ───────────────────────────────────────

    #[test]
    fn nul_byte_rejected() {
        assert_eq!(
            validate("safe\0bad"),
            Err(ipe_path_core::PathRejection::Nul)
        );
    }

    #[test]
    fn leading_dotdot_rejected() {
        assert_eq!(
            validate("../secret"),
            Err(ipe_path_core::PathRejection::Traversal)
        );
    }

    #[test]
    fn bare_dotdot_rejected() {
        assert_eq!(validate(".."), Err(ipe_path_core::PathRejection::Traversal));
    }

    #[test]
    fn dotdot_that_resolves_to_escape_rejected() {
        assert_eq!(
            validate("a/../../etc"),
            Err(ipe_path_core::PathRejection::Traversal)
        );
    }

    // ── rejected under the Windows regime — the all-targets guarantee ─────────
    //    Each of these is a Unix-clean no-op (`\` is a plain byte on Unix) yet a
    //    traversal on Windows; the compile-time gate must reject them so no such
    //    literal is ever emitted for a Windows build.

    #[test]
    fn win_backslash_traversal_rejected() {
        assert_eq!(
            validate("..\\secret"),
            Err(ipe_path_core::PathRejection::Traversal)
        );
    }

    #[test]
    fn win_drive_relative_dotdot_rejected() {
        assert_eq!(
            validate("C:..\\x"),
            Err(ipe_path_core::PathRejection::Traversal)
        );
    }

    #[test]
    fn win_trailing_dot_space_disguise_rejected() {
        assert_eq!(
            validate(".. \\x"),
            Err(ipe_path_core::PathRejection::Traversal)
        );
    }

    #[test]
    fn win_triple_dot_disguise_rejected() {
        assert_eq!(
            validate("..."),
            Err(ipe_path_core::PathRejection::Traversal)
        );
    }

    #[test]
    fn win_mixed_separator_traversal_rejected() {
        assert_eq!(
            validate("a\\..\\..\\b"),
            Err(ipe_path_core::PathRejection::Traversal)
        );
    }
}
