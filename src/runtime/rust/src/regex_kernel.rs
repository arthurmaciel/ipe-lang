//! Regex kernels for `Ipe.Regex`. A pattern is compiled ONCE via
//! [`regex_compile`] into an opaque [`Regex`] handle; an invalid pattern is a
//! typed `Err`, never a silent no-match. Every operation takes the already
//! compiled [`Regex`], so no operation can re-encounter an unvalidated pattern
//! string (parse, don't validate).

use super::{IpeMaybe, IpeResult};
use std::sync::Arc;

/// `Ipe.Regex`'s opaque compiled-pattern handle. Newtype over an `Arc`-shared
/// [`regex::Regex`] so cloning is a refcount bump (a `Regex` value may flow
/// through several call sites).
///
/// Deliberately carries only `Clone`: `regex::Regex` is neither `PartialEq`,
/// `Eq`, `Hash`, `Ord` nor serde, so the opaque handle inherits none of those.
/// The absence is load-bearing — a `Ipe.Web` Model field of type `Regex`, a
/// `Dict`-key use, or a serde round-trip is a compile-time rejection, never a
/// silent wrong behaviour. `Debug` is derived (prints the source pattern),
/// backing `toString` through the runtime's `Debug`-based stringify fallback.
#[derive(Clone, Debug)]
pub struct Regex(Arc<regex::Regex>);

/// `Regex.compile : String -> Result Error Regex` — THE construction boundary.
/// Every [`Regex`] value traces back to one of these calls; an invalid pattern
/// surfaces here as a typed `Err`, never anywhere downstream as a silent
/// no-match.
#[must_use]
pub fn regex_compile<E: From<String>>(pattern: String) -> IpeResult<E, Regex> {
    match regex::Regex::new(&pattern) {
        Ok(re) => IpeResult::Ok(Regex(Arc::new(re))),
        Err(e) => IpeResult::Err(format!("Ipe.Regex: invalid pattern: {e}").into()),
    }
}

/// `Regex.match : Regex -> String -> Bool` — does the pattern match anywhere?
#[must_use]
pub fn regex_match(re: Regex, s: String) -> bool {
    re.0.is_match(&s)
}

/// `Regex.find : Regex -> String -> Maybe String` — first match, if any.
#[must_use]
pub fn regex_find(re: Regex, s: String) -> IpeMaybe<String> {
    match re.0.find(&s) {
        Some(m) => IpeMaybe::Just(m.as_str().to_string()),
        None => IpeMaybe::Nothing,
    }
}

/// `Regex.findAll : Regex -> String -> List String` — every match, in order.
#[must_use]
pub fn regex_find_all(re: Regex, s: String) -> Vec<String> {
    re.0.find_iter(&s).map(|m| m.as_str().to_string()).collect()
}

/// `Regex.replace : Regex -> String -> String -> String` — replace every match
/// with `replacement` (RE2 `$1` substitution syntax).
#[must_use]
pub fn regex_replace(re: Regex, replacement: String, s: String) -> String {
    re.0.replace_all(&s, replacement.as_str()).to_string()
}

/// `Regex.split : Regex -> String -> List String` — split on every match.
///
/// Mirrors Go's `regexp.Split(s, -1)` (split on every match) rather than
/// Rust's `Regex::split`. The two diverge on zero-width matches: Go's Split
/// skips the field a match would produce when that match ends at byte 0
/// (`if match[1] != 0`), so a leading zero-width match at position 0 does NOT
/// emit a leading empty string, while interior zero-width matches still split.
/// Rust's `Regex::split` instead emits a leading empty for the same input.
#[must_use]
pub fn regex_split(re: Regex, s: String) -> Vec<String> {
    // Go special-cases a non-empty pattern against empty input as one empty
    // field (`if len(re.expr) > 0 && len(s) == 0 { return []string{""} }`).
    if !re.0.as_str().is_empty() && s.is_empty() {
        return vec![String::new()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut beg: usize = 0;
    // `end` tracks the START offset of the most recent match, mirroring Go's
    // `end = match[0]`; the trailing field is suppressed when it reaches len(s).
    let mut end: usize = 0;
    for m in re.0.find_iter(&s) {
        end = m.start();
        // Skip the field for a match ending at byte 0 (Go: `if match[1] != 0`)
        // — drops the leading empty produced by a zero-width match at pos 0.
        if m.end() != 0 {
            out.push(s[beg..end].to_string());
        }
        beg = m.end();
    }
    if end != s.len() {
        out.push(s[beg..].to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(pattern: &str) -> Regex {
        match regex_compile::<String>(pattern.to_string()) {
            IpeResult::Ok(re) => re,
            IpeResult::Err(e) => panic!("expected a valid pattern, got Err: {e}"),
        }
    }

    #[test]
    fn compile_valid_pattern_is_ok() {
        assert!(matches!(
            regex_compile::<String>(r"^\d+$".to_string()),
            IpeResult::Ok(_)
        ));
    }

    #[test]
    fn compile_invalid_pattern_is_typed_err_not_silent() {
        // The core contract: an invalid pattern is an observable typed Err,
        // NOT a silently-degrading success.
        match regex_compile::<String>(r"[unclosed".to_string()) {
            IpeResult::Ok(_) => panic!("invalid pattern must NOT compile"),
            IpeResult::Err(e) => assert!(e.contains("invalid pattern")),
        }
    }

    #[test]
    fn test_match() {
        assert!(regex_match(ok(r"^\d+$"), "12345".to_string()));
        assert!(!regex_match(ok(r"^\d+$"), "abc".to_string()));
    }

    #[test]
    fn test_find() {
        let m = regex_find(ok(r"\d+"), "foo 42 bar".to_string());
        assert!(matches!(m, IpeMaybe::Just(ref s) if s == "42"));
        let none = regex_find(ok(r"\d+"), "no digits here".to_string());
        assert!(matches!(none, IpeMaybe::Nothing));
    }

    #[test]
    fn test_find_all() {
        let all = regex_find_all(ok(r"\d+"), "1 and 22 and 333".to_string());
        assert_eq!(
            all,
            vec!["1".to_string(), "22".to_string(), "333".to_string()]
        );
    }

    #[test]
    fn test_replace() {
        let r = regex_replace(ok(r"\d+"), "N".to_string(), "a1b2c3".to_string());
        assert_eq!(r, "aNbNcN");
    }

    #[test]
    fn test_split() {
        let parts = regex_split(ok(r",\s*"), "a, b,c,  d".to_string());
        assert_eq!(parts, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn compiled_regex_clones_share_one_pattern() {
        let re = ok(r"\d+");
        let re2 = re.clone();
        assert!(regex_match(re, "x9".to_string()));
        assert!(regex_match(re2, "7".to_string()));
    }
}
