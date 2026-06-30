//! Regex kernels for Sky.Core.Regex. Invalid patterns NEVER panic — they
//! return identity / false / empty / Nothing per the Sky stdlib contract.

use super::SkyMaybe;
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
/// per the Sky stdlib contract (NEVER panic). Total: the `Mutex` lock is
/// only ever held briefly here and any `PoisonError` is recovered via
/// `into_inner`, so a panic in another thread can't wedge this path.
fn compiled(pattern: &str) -> Option<Arc<Regex>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Regex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let map = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(re) = map.get(pattern) {
            return Some(Arc::clone(re));
        }
    }
    // Compile OUTSIDE the lock so a slow compile never blocks other lookups.
    let re = Arc::new(Regex::new(pattern).ok()?);
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() < REGEX_CACHE_CAP {
        // Another thread may have inserted concurrently; entry() keeps it total.
        map.entry(pattern.to_string())
            .or_insert_with(|| Arc::clone(&re));
    }
    Some(re)
}

/// Sky `match : String -> String -> Bool`. Pattern first, then haystack.
pub fn regex_match(pattern: String, s: String) -> bool {
    match compiled(&pattern) {
        Some(re) => re.is_match(&s),
        None => false,
    }
}

/// Sky `find : String -> String -> Maybe String`
pub fn regex_find(pattern: String, s: String) -> SkyMaybe<String> {
    match compiled(&pattern) {
        Some(re) => match re.find(&s) {
            Some(m) => SkyMaybe::Just(m.as_str().to_string()),
            None => SkyMaybe::Nothing,
        },
        None => SkyMaybe::Nothing,
    }
}

/// Sky `findAll : String -> String -> List String`
pub fn regex_find_all(pattern: String, s: String) -> Vec<String> {
    match compiled(&pattern) {
        Some(re) => re.find_iter(&s).map(|m| m.as_str().to_string()).collect(),
        None => Vec::new(),
    }
}

/// Sky `replace : String -> String -> String -> String` (pattern, replacement, input).
pub fn regex_replace(pattern: String, replacement: String, s: String) -> String {
    match compiled(&pattern) {
        Some(re) => re.replace_all(&s, replacement.as_str()).to_string(),
        None => s,
    }
}

/// Sky `split : String -> String -> List String`
pub fn regex_split(pattern: String, s: String) -> Vec<String> {
    match compiled(&pattern) {
        Some(re) => re.split(&s).map(|x| x.to_string()).collect(),
        None => vec![s],
    }
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
        assert!(matches!(m, SkyMaybe::Just(ref s) if s == "42"));
        let none = regex_find(r"\d+".to_string(), "no digits here".to_string());
        assert!(matches!(none, SkyMaybe::Nothing));
        // Invalid pattern -> Nothing
        let bad = regex_find(r"[unclosed".to_string(), "x".to_string());
        assert!(matches!(bad, SkyMaybe::Nothing));
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
