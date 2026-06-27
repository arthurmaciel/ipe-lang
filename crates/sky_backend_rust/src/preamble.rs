//! Fixed Rust-target templates emitted verbatim for every M0 program.
//!
//! The Sky → Rust codegen wraps the user's emitted types and functions in a
//! fixed prologue (header, imports, basic type aliases, runtime re-exports) and
//! a fixed epilogue (`Ffi.kernel` polyfill, list helpers, FFI-placeholder
//! banner, entry point). Those two fixed regions are produced here.
//!
//! The byte source of truth is the golden program at
//! `tests/golden/m0/main.rs`. Rather than hand-retype the bytes (and risk a
//! silent drift from the golden), the templates are sliced out of the embedded
//! golden at well-defined textual anchors. The crate's tests assert byte
//! equality against an independent, line-number-based reconstruction of the
//! same golden, so any anchor mistake is caught.

/// The golden M0 program, embedded at compile time. The fixed preamble and
/// epilogue are exact substrings of this file.
const GOLDEN: &str = include_str!("../../../tests/golden/m0/main.rs");

/// Stub returned by the slice helpers when an anchor is not found. The embedded
/// golden always contains both anchors, so this is unreachable in practice; it
/// exists only to keep the helpers panic-free.
const EMPTY: &str = "";

/// The fixed prologue emitted before the user's type and function definitions.
///
/// Spans the golden header, file-level attributes, runtime re-exports, imports,
/// basic type aliases, and the `// USER TYPES` section banner — everything up to
/// (and not including) the first user type definition.
#[must_use]
pub fn preamble() -> String {
    // The user type definitions begin immediately after the USER TYPES banner
    // block, which is `// USER TYPES` followed by the banner's closing `// ===`
    // rule and a single blank line. Cut at the blank line that terminates the
    // banner so the preamble owns the whole banner and the user types follow.
    const BANNER_TITLE: &str = "// USER TYPES\n";
    let Some(title_idx) = GOLDEN.find(BANNER_TITLE) else {
        return EMPTY.to_string();
    };
    let rest = GOLDEN.get(title_idx..).unwrap_or(EMPTY);
    // The first blank line after the title closes the banner; include it.
    let Some(blank) = rest.find("\n\n") else {
        return EMPTY.to_string();
    };
    let end = title_idx + blank + "\n\n".len();
    GOLDEN.get(..end).unwrap_or(EMPTY).to_string()
}

/// The fixed epilogue emitted after the user's function definitions.
///
/// Spans the `Ffi.kernel` polyfill, the list helpers, the FFI-placeholder
/// banner, and the entry point (`fn main`) — the tail of the golden program.
#[must_use]
pub fn epilogue() -> String {
    // Anchored on the ASCII prefix of the polyfill comment (the full comment
    // contains a UTF-8 em-dash; the prefix is unique and ASCII-only).
    const ANCHOR: &str = "// Ffi.kernel polyfill";
    let Some(start) = GOLDEN.find(ANCHOR) else {
        return EMPTY.to_string();
    };
    GOLDEN.get(start..).unwrap_or(EMPTY).to_string()
}

#[cfg(test)]
mod tests {
    use super::{epilogue, preamble};

    /// Independent copy of the golden, reconstructed by 1-indexed line ranges so
    /// the assertions don't merely echo the implementation's anchor logic.
    const GOLDEN: &str = include_str!("../../../tests/golden/m0/main.rs");

    #[test]
    fn preamble_matches_golden_lines_1_to_30() {
        // Lines 1..=30: header through the blank line closing the USER TYPES
        // banner (the next line, 31, is the first user type definition).
        let expected: String = GOLDEN.split_inclusive('\n').take(30).collect();
        assert_eq!(preamble(), expected);
    }

    #[test]
    fn epilogue_matches_golden_lines_139_to_end() {
        // Lines 139..=end: the `Ffi.kernel` polyfill through `fn main`.
        let expected: String = GOLDEN.split_inclusive('\n').skip(138).collect();
        assert_eq!(epilogue(), expected);
    }

    #[test]
    fn preamble_is_a_prefix_of_golden() {
        assert!(GOLDEN.starts_with(&preamble()));
    }

    #[test]
    fn epilogue_is_a_suffix_of_golden() {
        assert!(GOLDEN.ends_with(&epilogue()));
    }

    #[test]
    fn preamble_ends_with_user_types_banner() {
        assert!(
            preamble()
                .ends_with("// USER TYPES\n// ===========================================\n\n")
        );
    }

    #[test]
    fn epilogue_starts_with_polyfill_and_ends_with_main() {
        assert!(epilogue().starts_with("// Ffi.kernel polyfill"));
        assert!(epilogue().trim_end().ends_with('}'));
    }
}
