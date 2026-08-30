| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // The `Ipe.Regex` kernels live behind the `regex` feature (the `regex_kernel`
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // module gate); this parity suite compiles only when it is selected. CI's
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // `--features full` includes `regex`, so the fixtures still run there.
#![cfg(feature = "regex")]
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! Parity fixtures for Ipe.Regex kernels.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //!
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! go`
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! (`Regex_match` / `Regex_find` / `Regex_findAll` / `Regex_replace` /
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! `Regex_split`) which uses `regex` (RE2 semantics).
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //!
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! The pattern is compiled ONCE via `regex_compile`; an invalid pattern is a
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! typed `Err` here, not a silent no-match at each operation. The operations
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! therefore take the compiled `Regex` and can never re-encounter an
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! unvalidated pattern.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //!
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! Note: both Rust's `regex` crate are both
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! RE2-family; on the match / find / findAll / replace patterns exercised
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! here, behaviour is identical. `split` does NOT come free, though: Rust's
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! `Regex::split` and `regex` disagree on zero-width matches at
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! position 0 (Rust emits a leading empty field, reference drops it). `regex_split`
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! reimplements the `Split` algorithm directly rather than
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! delegating to `Regex::split` — see `regex_split_zero_width_matches_go`.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! Named-group syntax and a handful of Perl-extension flags (lookahead,
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! lookbehind) are the remaining known divergence points — none are exercised
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //! by the Ipê stdlib surface.

use ipe_runtime_rust::*;

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically /// Compile a pattern the parity fixtures assume is valid; a failure here is a
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically /// test bug, not a runtime concern.
fn re(pattern: &str) -> Regex {
    match regex_compile::<String>(pattern.to_string()) {
        IpeResult::Ok(r) => r,
        IpeResult::Err(e) => panic!("fixture pattern {pattern:?} must compile, got Err: {e}"),
    }
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── GOLDEN: Regex compile ───────────────────────────────────────────────────
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // The construction boundary: a valid pattern is `Ok`, an invalid pattern is a
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // typed `Err` — NEVER a silent success that later degrades every operation.

#[test]
fn regex_compile_valid_is_ok() {
    assert!(matches!(
        regex_compile::<String>(r"\d+".to_string()),
        IpeResult::Ok(_)
    ));
    assert!(matches!(
        regex_compile::<String>(r"^[a-z]+$".to_string()),
        IpeResult::Ok(_)
    ));
}

#[test]
fn regex_compile_invalid_is_typed_err() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // An unclosed class, a bare `(`, and a dangling `*` are all invalid RE2 —
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // each surfaces as an observable `Err`, not a `Regex` that quietly never
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // matches.
    for bad in [r"[unclosed", "(", "*"] {
        match regex_compile::<String>(bad.to_string()) {
            IpeResult::Ok(_) => panic!("invalid pattern {bad:?} must NOT compile"),
            IpeResult::Err(e) => assert!(
                e.contains("invalid pattern"),
                "error must name the invalid pattern, got: {e}"
            ),
        }
    }
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── GOLDEN: Regex match ─────────────────────────────────────────────────────
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // Golden: `Regex_match(pattern, s)` → bool
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // regexp.MatchString — returns true iff the pattern matches anywhere in s.

#[test]
fn regex_match_golden() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_match(`\d+`, "a1b22c") = true — there is a digit run
    assert!(
        regex_match(re(r"\d+"), "a1b22c".to_string()),
        r"\d+ must match in 'a1b22c'"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_match(`\d+`, "abc") = false — no digits present
    assert!(
        !regex_match(re(r"\d+"), "abc".to_string()),
        r"\d+ must not match in 'abc'"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_match(`^[a-z]+$`, "hello") = true
    assert!(regex_match(re(r"^[a-z]+$"), "hello".to_string()));
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_match(`^[a-z]+$`, "Hello") = false (case-sensitive)
    assert!(!regex_match(re(r"^[a-z]+$"), "Hello".to_string()));
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_match(`.+`, "") = false (empty string, needs at least one char)
    assert!(!regex_match(re(r".+"), String::new()));
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_match(`.*`, "") = true (zero-or-more matches empty string)
    assert!(regex_match(re(r".*"), String::new()));
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── GOLDEN: Regex find ──────────────────────────────────────────────────────
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // Golden: `Regex_find(pattern, s)` → IpeMaybe<String>
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // regexp.FindString — first leftmost match or Nothing.

#[test]
fn regex_find_golden() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_find(`\d+`, "a1b22c") = Just "1" (first digit run)
    assert_eq!(
        regex_find(re(r"\d+"), "a1b22c".to_string()),
        IpeMaybe::Just("1".to_string()),
        r"find \d+ in 'a1b22c' must return first match '1'"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_find(`\d+`, "abc") = Nothing
    assert_eq!(
        regex_find(re(r"\d+"), "abc".to_string()),
        IpeMaybe::Nothing,
        r"find \d+ in 'abc' must be Nothing"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_find(`[a-z]+`, "123abc456") = Just "abc"
    assert_eq!(
        regex_find(re(r"[a-z]+"), "123abc456".to_string()),
        IpeMaybe::Just("abc".to_string())
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_find(`\d{3}`, "12 345 6789") = Just "345"
    assert_eq!(
        regex_find(re(r"\d{3}"), "12 345 6789".to_string()),
        IpeMaybe::Just("345".to_string())
    );
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── GOLDEN: Regex findAll ───────────────────────────────────────────────────
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // Golden: `Regex_findAll(pattern, s)` → List String
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // regexp.FindAllString(s, -1) — all non-overlapping matches.

#[test]
fn regex_find_all_golden() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_findAll(`\d+`, "a1b22c") = ["1", "22"]
    assert_eq!(
        regex_find_all(re(r"\d+"), "a1b22c".to_string()),
        vec!["1".to_string(), "22".to_string()],
        r"findAll \d+ in 'a1b22c' must return ['1', '22']"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_findAll(`\d+`, "abc") = []
    assert!(regex_find_all(re(r"\d+"), "abc".to_string()).is_empty());
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_findAll(`[a-z]+`, "a1b22c") = ["a", "b", "c"]
    assert_eq!(
        regex_find_all(re(r"[a-z]+"), "a1b22c".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_findAll(`\w+`, "hello world") = ["hello", "world"]
    assert_eq!(
        regex_find_all(re(r"\w+"), "hello world".to_string()),
        vec!["hello".to_string(), "world".to_string()]
    );
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── GOLDEN: Regex replace ───────────────────────────────────────────────────
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // Golden: `Regex_replace(pattern, replacement, s)` → String
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // regexp.ReplaceAllString — replaces ALL non-overlapping matches.

#[test]
fn regex_replace_golden() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_replace(`\d+`, "N", "a1b22c") = "aNbNc"
    assert_eq!(
        regex_replace(re(r"\d+"), "N".to_string(), "a1b22c".to_string()),
        "aNbNc",
        r"replace \d+ with N in 'a1b22c' must give 'aNbNc'"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_replace(`\s+`, " ", "foo  bar   baz") = "foo bar baz"
    assert_eq!(
        regex_replace(re(r"\s+"), " ".to_string(), "foo  bar   baz".to_string()),
        "foo bar baz"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // No match → input unchanged
    assert_eq!(
        regex_replace(re(r"\d+"), "X".to_string(), "abc".to_string()),
        "abc"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Replace with empty string (deletion)
    assert_eq!(
        regex_replace(re(r"\d+"), String::new(), "a1b22c".to_string()),
        "abc"
    );
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── GOLDEN: Regex split ─────────────────────────────────────────────────────
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // Golden: `Regex_split(pattern, s)` → List String
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // re.Split(s, -1) — split on all non-overlapping matches.

#[test]
fn regex_split_golden() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_split(`\d+`, "a1b22c") = ["a", "b", "c"]
    assert_eq!(
        regex_split(re(r"\d+"), "a1b22c".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        r"split 'a1b22c' on \d+ must give ['a', 'b', 'c']"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_split(`,`, "a,b,c") = ["a", "b", "c"]
    assert_eq!(
        regex_split(re(","), "a,b,c".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Regex_split(`\s+`, "hello world foo") = ["hello", "world", "foo"]
    assert_eq!(
        regex_split(re(r"\s+"), "hello world foo".to_string()),
        vec!["hello".to_string(), "world".to_string(), "foo".to_string()]
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // No match → single-element list with the original string
    assert_eq!(
        regex_split(re(r"\d+"), "abc".to_string()),
        vec!["abc".to_string()]
    );
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── GOLDEN: Regex split on zero-width / empty-matching patterns ─────────────
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // `re.Split(s, -1)` — RE2 semantics. (1)
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // `allMatches` (shared by FindAll and Split) NEVER delivers an empty match that
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // starts where the previous match ended, so for `x*`-style patterns the only
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // matches are the non-empty runs plus a zero-width match at each "gap" not
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // adjacent to a prior match. (2) `Split` skips the field a match would
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // produce when that match ends at byte 0 (`if match[1] != 0`), dropping the
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // leading empty field a position-0 zero-width match would otherwise create.
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically //
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // Rust's `regex::Regex::find_iter` applies the SAME adjacent-empty-match
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // suppression, but Rust's `Regex::split` would still emit a leading "" for these
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // inputs. `regex_split` reimplements `Split` over `find_iter`, so each case
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // below yields `["a", "b", "c"]` with NO leading empty.

#[test]
fn regex_split_zero_width_matches_go() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Split("", "abc") — empty matches at 0,1,2,3; the position-0 field is
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // dropped → ["a", "b", "c"] (Rust's native Regex::split → ["", "a","b","c"]).
    assert_eq!(
        regex_split(re(""), "abc".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        "empty-pattern split drops the leading zero-width field"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Split("x*", "axxbxc") — matches (0,0),(1,3),(4,5),(6,6); the empty
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // matches at the run boundaries are suppressed (adjacent to a prior match).
    assert_eq!(
        regex_split(re(r"x*"), "axxbxc".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        r"x* split: zero-width handling"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Split(`\d*`, "a1b22c") — matches (0,0),(1,2),(3,5),(6,6) → ["a","b","c"].
    assert_eq!(
        regex_split(re(r"\d*"), "a1b22c".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        r"d* split: zero-width handling"
    );
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // # Split(`,?`, "a,b,c") — matches (0,0),(1,2),(3,4),(5,5) → ["a","b","c"].
    assert_eq!(
        regex_split(re(r",?"), "a,b,c".to_string()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        r",? split: zero-width handling"
    );
}

| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically // ── Edge cases ──────────────────────────────────────────────────────────────

#[test]
fn regex_handles_empty_string_inputs() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // parity: operations on empty string must not panic
    assert!(!regex_match(re(r"\d+"), String::new()));
    assert_eq!(regex_find(re(r"\d+"), String::new()), IpeMaybe::Nothing);
    assert!(regex_find_all(re(r"\d+"), String::new()).is_empty());
    assert_eq!(
        regex_replace(re(r"\d+"), "X".to_string(), String::new()),
        ""
    );
    assert_eq!(regex_split(re(r"\d+"), String::new()), vec![String::new()]);
}

#[test]
fn regex_match_anchors_work_like_go() {
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // regexp does partial matching by default (no implicit ^...$).
| map `{ a: 1, b: 2 }` | `map[a:1 b:2]` | keys sorted alphabetically     // Anchors must be explicit.
    assert!(regex_match(re(r"bar"), "foobar".to_string())); // partial match
    assert!(regex_match(re(r"^foo"), "foobar".to_string()));
    assert!(!regex_match(re(r"^bar"), "foobar".to_string()));
    assert!(regex_match(re(r"bar$"), "foobar".to_string()));
    assert!(!regex_match(re(r"foo$"), "foobar".to_string()));
}
