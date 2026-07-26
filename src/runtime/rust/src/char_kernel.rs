//! Ipe.Char kernels — single-code-point helpers.
//!
//! A Ipê `Char` lowers to a Rust `char` (one Unicode scalar value), so these
//! kernels take/return `char` directly — no `any` boxing. Mirrors the Go
//! runtime's `Char_*` functions (`runtime-go/rt/rt.go`):
//!
//! ## Predicates — exact Unicode `General_Category` parity with Go
//!
//! Go's `Char_is*` route through the `unicode` package, each keyed off a
//! precise `General_Category` (GC). Rust std's `char` predicates are BROADER and
//! would diverge:
//!
//! | Ipê      | Go                  | GC(s)                    | Rust std (rejected)                       |
//! |----------|---------------------|--------------------------|-------------------------------------------|
//! | isDigit  | `unicode.IsDigit`   | `Nd`                     | `is_numeric` = `Nd|Nl|No` (catches `'²'`) |
//! | isLower  | `unicode.IsLower`   | `Ll`                     | `is_lowercase` adds `Other_Lowercase`     |
//! | isUpper  | `unicode.IsUpper`   | `Lu`                     | `is_uppercase` adds `Other_Uppercase`     |
//! | isAlpha  | `unicode.IsLetter`  | `Lu|Ll|Lt|Lm|Lo` (`L*`)  | `is_alphabetic` adds `Nl|Other_Alphabetic`|
//!
//! Concretely: `'²'`/`'½'` → isDigit **false** (No, not Nd); `'ª'` → isLower
//! **false** (Lo, the feminine ordinal — `is_lowercase` wrongly counts its
//! `Other_Lowercase` property); `'é'` → isAlpha **true** (Ll ⊂ L*). We resolve
//! the exact GC via `unicode_general_category::get_general_category` and match
//! against Go's category sets. (Case MAPPING — `toLower`/`toUpper` below — is a
//! deliberate, sanctioned divergence: it stays full-Unicode per
//! docs/architecture/divergence-policy.md, so it is NOT touched here.)
//!
//! * `toLower`/`toUpper` return a single-rune **String** (the kernel registry
//!   shape is `Char -> String`), matching Go's `string(unicode.ToLower(r))`.
//! * `fromCode` out of the valid scalar range (negative, > 0x10FFFF, or a
//!   surrogate D800–DFFF that `char` cannot hold) yields the Unicode
//!   replacement character `'\u{FFFD}'` — same contract as Go.

use unicode_general_category::{GeneralCategory, get_general_category};

/// `isDigit` ← `unicode.IsDigit` = `General_Category` `Nd` (decimal digit) only.
#[must_use]
pub fn char_is_digit(c: char) -> bool {
    matches!(get_general_category(c), GeneralCategory::DecimalNumber)
}

/// `isLower` ← `unicode.IsLower` = `General_Category` `Ll` only.
#[must_use]
pub fn char_is_lower(c: char) -> bool {
    matches!(get_general_category(c), GeneralCategory::LowercaseLetter)
}

/// `isUpper` ← `unicode.IsUpper` = `General_Category` `Lu` only.
#[must_use]
pub fn char_is_upper(c: char) -> bool {
    matches!(get_general_category(c), GeneralCategory::UppercaseLetter)
}

/// `isAlpha` ← `unicode.IsLetter` = the letter categories `L*`
/// (`Lu | Ll | Lt | Lm | Lo`).
#[must_use]
pub fn char_is_alpha(c: char) -> bool {
    matches!(
        get_general_category(c),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
    )
}

/// `isAlphaNum` — a letter or a digit. Mirrors Elm's
/// `isUpper || isLower || isDigit`, here expressed over the existing
/// category-based `isAlpha`/`isDigit` so Unicode letters/digits classify
/// consistently with the rest of this module.
#[must_use]
pub fn char_is_alpha_num(c: char) -> bool {
    char_is_alpha(c) || char_is_digit(c)
}

/// `isHexDigit` — an ASCII hexadecimal digit (`0-9`, `a-f`, `A-F`). Matches
/// Elm's code-point ranges exactly (ASCII only, never Unicode digits).
#[must_use]
pub fn char_is_hex_digit(c: char) -> bool {
    c.is_ascii_hexdigit()
}

/// `isOctDigit` — an ASCII octal digit (`0-7`). Matches Elm's code-point range
/// exactly (ASCII only).
#[must_use]
pub fn char_is_oct_digit(c: char) -> bool {
    ('0'..='7').contains(&c)
}

#[must_use]
pub fn char_to_lower(c: char) -> String {
    c.to_lowercase().to_string()
}
#[must_use]
pub fn char_to_upper(c: char) -> String {
    c.to_uppercase().to_string()
}

/// `toCode 'A' -> 65` — the Unicode code point as an integer.
#[must_use]
pub fn char_to_code(c: char) -> i64 {
    i64::from(c as u32)
}

/// `fromCode 65 -> 'A'`. Out-of-range / surrogate -> U+FFFD (matches Go).
#[must_use]
pub fn char_from_code(n: i64) -> char {
    if !(0..=0x0010_FFFF).contains(&n) {
        return '\u{FFFD}';
    }
    char::from_u32(n as u32).unwrap_or('\u{FFFD}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicates() {
        assert!(char_is_alpha('A'));
        assert!(!char_is_alpha('1'));
        assert!(char_is_digit('7'));
        assert!(!char_is_digit('A'));
        assert!(char_is_lower('a'));
        assert!(!char_is_lower('A'));
        assert!(char_is_upper('Z'));
        assert!(!char_is_upper('z'));
    }

    /// Exact-General_Category parity with Go's `unicode.Is*` — the cases where
    /// Rust std's broader predicates would diverge.
    #[test]
    fn predicates_match_go_general_categories() {
        // isDigit = Nd only.
        // U+00B2 SUPERSCRIPT TWO and U+00BD VULGAR FRACTION ONE HALF are
        // category No — Go's IsDigit rejects; Rust `is_numeric` would accept.
        assert!(!char_is_digit('\u{00B2}'));
        assert!(!char_is_digit('\u{00BD}'));
        // U+2167 SMALL ROMAN NUMERAL EIGHT is Nl, not Nd.
        assert!(!char_is_digit('\u{2167}'));
        // U+0664 ARABIC-INDIC DIGIT FOUR is Nd.
        assert!(char_is_digit('\u{0664}'));

        // isLower = Ll only.
        // U+00AA FEMININE ORDINAL INDICATOR is Lo with the Other_Lowercase
        // property — Go's IsLower rejects; Rust `is_lowercase` would accept.
        assert!(!char_is_lower('\u{00AA}'));
        // U+00E9 LATIN SMALL LETTER E WITH ACUTE is Ll.
        assert!(char_is_lower('\u{00E9}'));

        // isUpper = Lu only.
        // U+2160 ROMAN NUMERAL ONE is Nl, not Lu.
        assert!(!char_is_upper('\u{2160}'));
        // U+00C9 LATIN CAPITAL LETTER E WITH ACUTE is Lu.
        assert!(char_is_upper('\u{00C9}'));

        // isAlpha = L* (Lu | Ll | Lt | Lm | Lo).
        // U+00E9 is Ll, U+01C5 (LATIN CAPITAL LETTER D WITH SMALL LETTER Z
        // WITH CARON) is Lt, U+30AB KATAKANA LETTER KA is Lo — all letters.
        assert!(char_is_alpha('\u{00E9}'));
        assert!(char_is_alpha('\u{01C5}'));
        assert!(char_is_alpha('\u{30AB}'));
        // U+00B2 SUPERSCRIPT TWO is No, not a letter.
        assert!(!char_is_alpha('\u{00B2}'));
    }

    #[test]
    fn case_conversion_returns_string() {
        assert_eq!(char_to_lower('A'), "a");
        assert_eq!(char_to_upper('a'), "A");
    }

    #[test]
    fn code_roundtrip() {
        assert_eq!(char_to_code('A'), 65);
        assert_eq!(char_from_code(65), 'A');
        assert_eq!(char_from_code(0x1F600), '\u{1F600}'); // 😀
    }

    #[test]
    fn from_code_out_of_range_is_replacement() {
        assert_eq!(char_from_code(-1), '\u{FFFD}');
        assert_eq!(char_from_code(0x0011_0000), '\u{FFFD}');
        assert_eq!(char_from_code(0xD800), '\u{FFFD}'); // lone surrogate
    }

    #[test]
    fn alpha_num_hex_oct_match_elm() {
        // isAlphaNum: letters + digits, not punctuation/space.
        assert!(char_is_alpha_num('a'));
        assert!(char_is_alpha_num('Z'));
        assert!(char_is_alpha_num('7'));
        assert!(!char_is_alpha_num('-'));
        assert!(!char_is_alpha_num(' '));

        // isHexDigit: 0-9 a-f A-F only (ASCII).
        assert!(char_is_hex_digit('0'));
        assert!(char_is_hex_digit('9'));
        assert!(char_is_hex_digit('a'));
        assert!(char_is_hex_digit('F'));
        assert!(!char_is_hex_digit('g'));
        assert!(!char_is_hex_digit('G'));

        // isOctDigit: 0-7 only.
        assert!(char_is_oct_digit('0'));
        assert!(char_is_oct_digit('7'));
        assert!(!char_is_oct_digit('8'));
        assert!(!char_is_oct_digit('9'));
        assert!(!char_is_oct_digit('a'));
    }
}
