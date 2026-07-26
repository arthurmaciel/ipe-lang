//! Regex kernels for Ipe.Regex. Invalid patterns NEVER panic — they
//! return identity / false / empty / Nothing per the Ipê stdlib contract.

use super::IpeMaybe;
use regex::Regex;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Hard cap on distinct compiled patterns we retain. Patterns are
/// user-controlled, so an UNBOUNDED cache would be a memory-DoS vector
/// (worse than the per-call recompile CPU cost it avoids — soundness
/// outranks efficiency). Once the cache is full we stop inserting and fall
/// back to a fresh compile, so memory stays bounded while the common case
/// (a small fixed set of hot patterns) is still cached.
const REGEX_CACHE_CAP: usize = 256;

/// Compile `pattern`, reusing a cached `Regex` when one exists. Returns
/// `None` for an invalid pattern — callers degrade to identity/false/empty
/// per the Ipê stdlib contract (NEVER panic). Total: the `Mutex` lock is
/// only ever held briefly here and any `PoisonError` is recovered via
/// `into_inner`, so a panic in another thread can't wedge this path.
fn compiled(pattern: &str) -> Option<Arc<Regex>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Regex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let map = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(re) = map.get(pattern) {
            return Some(Arc::clone(re));
        }
    }
    // Compile OUTSIDE the lock so a slow compile never blocks other lookups.
    let re = Arc::new(Regex::new(pattern).ok()?);
    let mut map = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if map.len() < REGEX_CACHE_CAP {
        // Another thread may have inserted concurrently; entry() keeps it total.
        map.entry(pattern.to_string())
            .or_insert_with(|| Arc::clone(&re));
    }
    Some(re)
}

/// Ipê `match : String -> String -> Bool`. Pattern first, then haystack.
#[must_use]
pub fn regex_match(pattern: String, s: String) -> bool {
    match compiled(&pattern) {
        Some(re) => re.is_match(&s),
        None => false,
    }
}

/// Ipê `find : String -> String -> Maybe String`
#[must_use]
pub fn regex_find(pattern: String, s: String) -> IpeMaybe<String> {
    match compiled(&pattern) {
        Some(re) => match re.find(&s) {
            Some(m) => IpeMaybe::Just(m.as_str().to_string()),
            None => IpeMaybe::Nothing,
        },
        None => IpeMaybe::Nothing,
    }
}

/// Ipê `findAll : String -> String -> List String`
#[must_use]
pub fn regex_find_all(pattern: String, s: String) -> Vec<String> {
    match compiled(&pattern) {
        Some(re) => re.find_iter(&s).map(|m| m.as_str().to_string()).collect(),
        None => Vec::new(),
    }
}

/// Ipê `replace : String -> String -> String -> String` (pattern, replacement, input).
#[must_use]
pub fn regex_replace(pattern: String, replacement: String, s: String) -> String {
    match compiled(&pattern) {
        Some(re) => re.replace_all(&s, replacement.as_str()).to_string(),
        None => s,
    }
}

/// Ipê `split : String -> String -> List String`.
///
/// Mirrors Go's `regexp.Split(s, -1)` (split on every match) rather than
/// Rust's `Regex::split`. The two diverge on zero-width matches: Go's Split
/// skips the field a match would produce when that match ends at byte 0
/// (`if match[1] != 0`), so a leading zero-width match at position 0 does NOT
/// emit a leading empty string, while interior zero-width matches still split.
/// Rust's `Regex::split` instead emits a leading empty for the same input.
#[must_use]
pub fn regex_split(pattern: String, s: String) -> Vec<String> {
    let Some(re) = compiled(&pattern) else {
        return vec![s];
    };
    // Go special-cases a non-empty pattern against empty input as one empty
    // field (`if len(re.expr) > 0 && len(s) == 0 { return []string{""} }`).
    if !pattern.is_empty() && s.is_empty() {
        return vec![String::new()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut beg: usize = 0;
    // `end` tracks the START offset of the most recent match, mirroring Go's
    // `end = match[0]`; the trailing field is suppressed when it reaches len(s).
    let mut end: usize = 0;
    for m in re.find_iter(&s) {
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

    #[test]
    fn test_match() {
        assert!(regex_match(r"^\d+$".to_string(), "12345".to_string()));
        assert!(!regex_match(r"^\d+$".to_string(), "abc".to_string()));
        // Invalid pattern -> false (never panic)
        assert!(!regex_match(
            r"[unclosed".to_string(),
            "anything".to_string()
        ));
    }

    #[test]
    fn test_find() {
        let m = regex_find(r"\d+".to_string(), "foo 42 bar".to_string());
        assert!(matches!(m, IpeMaybe::Just(ref s) if s == "42"));
        let none = regex_find(r"\d+".to_string(), "no digits here".to_string());
        assert!(matches!(none, IpeMaybe::Nothing));
        // Invalid pattern -> Nothing
        let bad = regex_find(r"[unclosed".to_string(), "x".to_string());
        assert!(matches!(bad, IpeMaybe::Nothing));
    }

    #[test]
    fn test_find_all() {
        let all = regex_find_all(r"\d+".to_string(), "1 and 22 and 333".to_string());
        assert_eq!(
            all,
            vec!["1".to_string(), "22".to_string(), "333".to_string()]
        );
        // Invalid pattern -> empty
        let bad = regex_find_all(r"[unclosed".to_string(), "1 2 3".to_string());
        assert!(bad.is_empty());
    }

    #[test]
    fn test_replace() {
        let r = regex_replace(r"\d+".to_string(), "N".to_string(), "a1b2c3".to_string());
        assert_eq!(r, "aNbNcN");
        // Invalid pattern -> identity (input unchanged)
        let bad = regex_replace(r"[unclosed".to_string(), "X".to_string(), "abc".to_string());
        assert_eq!(bad, "abc");
    }

    #[test]
    fn test_split() {
        let parts = regex_split(r",\s*".to_string(), "a, b,c,  d".to_string());
        assert_eq!(parts, vec!["a", "b", "c", "d"]);
        // Invalid pattern -> single-element list with the original string
        let bad = regex_split(r"[unclosed".to_string(), "abc".to_string());
        assert_eq!(bad, vec!["abc".to_string()]);
    }
}
