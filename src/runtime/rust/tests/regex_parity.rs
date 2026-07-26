//! Go≡Rust parity fixtures for Ipe.Regex kernels.
//!
//! Every assertion mirrors the Go oracle in `runtime-go/rt/rt.go`
//! (`Regex_match` / `Regex_find` / `Regex_findAll` / `Regex_replace` / `Regex_split`)
//! which uses Go's `regexp` package — RE2 semantics.
//!
//! Divergence note: Go's `regexp` and Rust's `regex` crate are both
//! RE2-family; on the match / find / findAll / replace patterns exercised
//! here, behaviour is identical. `split` does NOT come free, though: Rust's
//! `Regex::split` and Go's `regexp.Split` disagree on zero-width matches at
//! position 0 (Rust emits a leading empty field, Go drops it). `regex_split`
//! therefore reimplements Go's `Split` algorithm directly rather than
//! delegating to `Regex::split` — see `regex_split_zero_width_matches_go`.
//! Named-group syntax and a handful of Perl-extension flags (lookahead,
//! lookbehind) are the remaining known divergence points — none are exercised
//! by the Ipê stdlib surface.

use ipe_runtime_rust::*;

// ── GOLDEN: Regex match ─────────────────────────────────────────────────────
//
// Go oracle: `Regex_match(pattern, s)` → bool
// regexp.MatchString — returns true iff the pattern matches anywhere in s.

#[test]
fn regex_match_golden() {
    // Go: Regex_match(`\d+`, "a1b22c") = true — there is a digit run
    assert!(
        regex_match(r"\d+".to_string(), "a1b22c".to_string()),
        r"\d+ must match in 'a1b22c'"
    );
    // Go: Regex_match(`\d+`, "abc") = false — no digits present
    assert!(
        !regex_match(r"\d+".to_string(), "abc".to_string()),
        r"\d+ must not match in 'abc'"
    );
    // Go: Regex_match(`^[a-z]+$`, "hello") = true
    assert!(regex_match(r"^[a-z]+$".to_string(), "hello".to_string()));
    // Go: Regex_match(`^[a-z]+$`, "Hello") = false (case-sensitive)
    assert!(!regex_match(r"^[a-z]+$".to_string(), "Hello".to_string()));
    // Go: Regex_match(`.+`, "") = false (empty string, needs at least one char)
    assert!(!regex_match(r".+".to_string(), String::new()));
    // Go: Regex_match(`.*`, "") = true (zero-or-more matches empty string)
    assert!(regex_match(r".*".to_string(), String::new()));
    // Invalid pattern → false (Go: `_, _ := regexp.MatchString` returns false + err)
    assert!(
        !regex_match(r"[unclosed".to_string(), "x".to_string()),
        "invalid pattern must return false without panicking"
    );
}

// ── GOLDEN: Regex find ──────────────────────────────────────────────────────
//
// Go oracle: `Regex_find(pattern, s)` → IpeMaybe<String>
// regexp.FindString — first leftmost match or Nothing.

#[test]
fn regex_find_golden() {
    // Go: Regex_find(`\d+`, "a1b22c") = Just "1" (first digit run)
    assert_eq!(
        regex_find(r"\d+".to_string(), "a1b22c".to_string()),
        IpeMaybe::Just("1".to_string()),
        r"find \d+ in 'a1b22c' must return first match '1'"
    );
    // Go: Regex_find(`\d+`, "abc") = Nothing
    assert_eq!(
        regex_find(r"\d+".to_string(), "abc".to_string()),
        IpeMaybe::Nothing,
        r"find \d+ in 'abc' must be Nothing"
    );
    // Go: Regex_find(`[a-z]+`, "123abc456") = Just "abc"
    assert_eq!(
        regex_find(r"[a-z]+".to_string(), "123abc456".to_string()),
        IpeMaybe::Just("abc".to_string())
    );
    // Go: Regex_find(`\d{3}`, "12 345 6789") = Just "345"
    assert_eq!(
        regex_find(r"\d{3}".to_string(), "12 345 6789".to_string()),
        IpeMaybe::Just("345".to_string())
    );
    // Invalid pattern → Nothing
    assert_eq!(
        regex_find(r"[unclosed".to_string(), "x".to_string()),
        IpeMaybe::Nothing
    );
}

// ── GOLDEN: Regex findAll ───────────────────────────────────────────────────
//
// Go oracle: `Regex_findAll(pattern, s)` → List String
// regexp.FindAllString(s, -1) — all non-overlapping matches.

#[test]
fn regex_find_all_golden() {
    // Go: Regex_findAll(`\d+`, "a1b22c") = ["1", "22"]
    assert_eq!(
        regex_find_all(r"\d+".to_string(), "a1b22c".to_string()),
        vec!["1".to_string(), "22".to_string()],
        r"findAll \d+ in 'a1b22c' must return ['1', '22']"
    );
    // Go: Regex_findAll(`\d+`, "abc") = []
    assert!(regex_find_all(r"\d+".to_string(), "abc".to_string()).is_empty());
    // Go: Regex_findAll(`[a-z]+`, "a1b22c") = ["a", "b", "c"]
    assert_eq!(
        regex_find_all(r"[a-z]+".to_string(), "a1b22c".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
    // Go: Regex_findAll(`\w+`, "hello world") = ["hello", "world"]
    assert_eq!(
        regex_find_all(r"\w+".to_string(), "hello world".to_string()),
        vec!["hello".to_string(), "world".to_string()]
    );
    // Invalid pattern → empty list
    assert!(regex_find_all(r"[unclosed".to_string(), "x".to_string()).is_empty());
}

// ── GOLDEN: Regex replace ───────────────────────────────────────────────────
//
// Go oracle: `Regex_replace(pattern, replacement, s)` → String
// regexp.ReplaceAllString — replaces ALL non-overlapping matches.

#[test]
fn regex_replace_golden() {
    // Go: Regex_replace(`\d+`, "N", "a1b22c") = "aNbNc"
    assert_eq!(
        regex_replace(r"\d+".to_string(), "N".to_string(), "a1b22c".to_string()),
        "aNbNc",
        r"replace \d+ with N in 'a1b22c' must give 'aNbNc'"
    );
    // Go: Regex_replace(`\s+`, " ", "foo  bar   baz") = "foo bar baz"
    assert_eq!(
        regex_replace(
            r"\s+".to_string(),
            " ".to_string(),
            "foo  bar   baz".to_string()
        ),
        "foo bar baz"
    );
    // No match → input unchanged
    assert_eq!(
        regex_replace(r"\d+".to_string(), "X".to_string(), "abc".to_string()),
        "abc"
    );
    // Replace with empty string (deletion)
    assert_eq!(
        regex_replace(r"\d+".to_string(), String::new(), "a1b22c".to_string()),
        "abc"
    );
    // Invalid pattern → input unchanged (Go: `return s`)
    assert_eq!(
        regex_replace(r"[unclosed".to_string(), "X".to_string(), "abc".to_string()),
        "abc"
    );
}

// ── GOLDEN: Regex split ─────────────────────────────────────────────────────
//
// Go oracle: `Regex_split(pattern, s)` → List String
// re.Split(s, -1) — split on all non-overlapping matches.

#[test]
fn regex_split_golden() {
    // Go: Regex_split(`\d+`, "a1b22c") = ["a", "b", "c"]
    assert_eq!(
        regex_split(r"\d+".to_string(), "a1b22c".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        r"split 'a1b22c' on \d+ must give ['a', 'b', 'c']"
    );
    // Go: Regex_split(`,`, "a,b,c") = ["a", "b", "c"]
    assert_eq!(
        regex_split(",".to_string(), "a,b,c".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
    // Go: Regex_split(`\s+`, "hello world foo") = ["hello", "world", "foo"]
    assert_eq!(
        regex_split(r"\s+".to_string(), "hello world foo".to_string()),
        vec!["hello".to_string(), "world".to_string(), "foo".to_string()]
    );
    // No match → single-element list with the original string
    assert_eq!(
        regex_split(r"\d+".to_string(), "abc".to_string()),
        vec!["abc".to_string()]
    );
    // Invalid pattern → single-element list (Go: `return []any{s}`)
    assert_eq!(
        regex_split(r"[unclosed".to_string(), "abc".to_string()),
        vec!["abc".to_string()]
    );
}

// ── GOLDEN: Regex split on zero-width / empty-matching patterns ─────────────
//
// Go oracle: `re.Split(s, -1)`. Two behaviours combine here. (1) Go's
// `allMatches` (shared by FindAll and Split) NEVER delivers an empty match that
// starts where the previous match ended, so for `x*`-style patterns the only
// matches are the non-empty runs plus a zero-width match at each "gap" not
// adjacent to a prior match. (2) Go's `Split` skips the field a match would
// produce when that match ends at byte 0 (`if match[1] != 0`), dropping the
// leading empty field a position-0 zero-width match would otherwise create.
//
// Rust's `regex::Regex::find_iter` applies the SAME adjacent-empty-match
// suppression, but Rust's `Regex::split` would still emit a leading "" for these
// inputs. `regex_split` reimplements Go's `Split` over `find_iter`, so each case
// below yields `["a", "b", "c"]` with NO leading empty — matching Go exactly.

#[test]
fn regex_split_zero_width_matches_go() {
    // Go: Split("", "abc") — empty matches at 0,1,2,3; the position-0 field is
    // dropped → ["a", "b", "c"] (Rust's native Regex::split → ["", "a","b","c"]).
    assert_eq!(
        regex_split(String::new(), "abc".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        "empty-pattern split drops the leading zero-width field (Go parity)"
    );
    // Go: Split("x*", "axxbxc") — matches (0,0),(1,3),(4,5),(6,6); the empty
    // matches at the run boundaries are suppressed (adjacent to a prior match).
    assert_eq!(
        regex_split(r"x*".to_string(), "axxbxc".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        r"x* split must match Go's zero-width handling"
    );
    // Go: Split(`\d*`, "a1b22c") — matches (0,0),(1,2),(3,5),(6,6) → ["a","b","c"].
    assert_eq!(
        regex_split(r"\d*".to_string(), "a1b22c".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        r"\d* split must match Go's zero-width handling"
    );
    // Go: Split(`,?`, "a,b,c") — matches (0,0),(1,2),(3,4),(5,5) → ["a","b","c"].
    assert_eq!(
        regex_split(r",?".to_string(), "a,b,c".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        r",? split must match Go's zero-width handling"
    );
}

// ── Edge cases ──────────────────────────────────────────────────────────────

#[test]
fn regex_handles_empty_string_inputs() {
    // Go parity: operations on empty string must not panic
    assert!(!regex_match(r"\d+".to_string(), String::new()));
    assert_eq!(
        regex_find(r"\d+".to_string(), String::new()),
        IpeMaybe::Nothing
    );
    assert!(regex_find_all(r"\d+".to_string(), String::new()).is_empty());
    assert_eq!(
        regex_replace(r"\d+".to_string(), "X".to_string(), String::new()),
        ""
    );
    assert_eq!(
        regex_split(r"\d+".to_string(), String::new()),
        vec![String::new()]
    );
}

#[test]
fn regex_match_anchors_work_like_go() {
    // Go's regexp does partial matching by default (no implicit ^...$).
    // Anchors must be explicit.
    assert!(regex_match(r"bar".to_string(), "foobar".to_string())); // partial match
    assert!(regex_match(r"^foo".to_string(), "foobar".to_string()));
    assert!(!regex_match(r"^bar".to_string(), "foobar".to_string()));
    assert!(regex_match(r"bar$".to_string(), "foobar".to_string()));
    assert!(!regex_match(r"foo$".to_string(), "foobar".to_string()));
}
