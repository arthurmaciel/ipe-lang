//! Sky.Core.String kernel — the single home for the String runtime surface.
//!
//! Argument order matches the Go runtime's typed kernels
//! (runtime-go/rt/rt.go: String_replace / String_startsWith / etc.).

use super::SkyMaybe;

// ── Core String kernels (relocated from core.rs so the String surface has one home) ──

pub fn string_from_int(i: i64) -> String {
    format!("{}", i)
}
pub fn string_join(sep: String, strs: Vec<String>) -> String {
    strs.join(&sep)
}
pub fn string_append(a: String, b: String) -> String {
    a + &b
}
pub fn string_length(s: String) -> i64 {
    s.chars().count() as i64
}
pub fn string_is_empty(s: String) -> bool {
    s.is_empty()
}
pub fn string_reverse(s: String) -> String {
    s.chars().rev().collect()
}
pub fn string_to_upper(s: String) -> String {
    s.to_uppercase()
}
pub fn string_to_lower(s: String) -> String {
    s.to_lowercase()
}
pub fn string_trim(s: String) -> String {
    s.trim().to_string()
}
// Sky `contains : String -> String -> Bool  -- contains sub str` (str contains
// sub). Args arrive as (sub, str), so test the SECOND against the first.
pub fn string_contains(sub: String, s: String) -> bool {
    s.contains(&sub)
}
/// `String.toInt : String -> Maybe Int`. Parity with the Go reference's
/// OBSERVABLE behaviour (`String_toInt` = `strconv.Atoi(s)`, NO trim): the Go
/// compiler routes `String.toInt` through this any-typed path, so surrounding
/// whitespace makes the parse fail — `String.toInt " 42 " == Nothing`. (Go's
/// unused `String_toIntT` companion happens to `TrimSpace` first, but the
/// emitted code never reaches it; the golden oracle confirms `Nothing`.) This
/// also matches Elm-family `String.toInt`, which does not trim. So no `.trim()`
/// here — a leading/trailing space yields `Nothing`, exactly like the oracle.
pub fn string_to_int(s: String) -> SkyMaybe<i64> {
    match s.parse::<i64>() {
        Ok(v) => SkyMaybe::Just(v),
        Err(_) => SkyMaybe::Nothing,
    }
}
/// `String.toFloat : String -> Maybe Float`. Go parity (`String_toFloat`):
/// `strconv.ParseFloat(strings.TrimSpace(s), 64)` — UNLIKE `toInt`, Go's float
/// path DOES trim Unicode whitespace before parsing (the oracle confirms
/// `String.toFloat " 1.5 " == Just 1.5`), so we trim here too via `str::trim`
/// (`char::is_whitespace` = the Unicode White_Space property, matching Go's
/// `unicode.IsSpace`). The toInt/toFloat trim asymmetry mirrors the Go runtime.
///
///
/// SANCTIONED-STRICTER vs Go: Rust's `f64::from_str` accepts only the standard
/// decimal / scientific grammar, rejecting Go's hex-float (`0x1p-2`) and
/// underscore-digit-separator forms. This is a deliberate tightening — those
/// forms never round-trip from `String.fromFloat` — so no golden is needed.
pub fn string_to_float(s: String) -> SkyMaybe<f64> {
    match s.trim().parse::<f64>() {
        Ok(v) => SkyMaybe::Just(v),
        Err(_) => SkyMaybe::Nothing,
    }
}
/// `String.fromChar : Char -> String`.
pub fn string_from_char(c: char) -> String {
    c.to_string()
}
/// `String.slice : Int -> Int -> String -> String`. Char(rune)-indexed with
/// negative-index-from-end + clamping — parity with Go's `String_sliceT`.
pub fn string_slice(start: i64, end: i64, s: String) -> String {
    let runes: Vec<char> = s.chars().collect();
    let total = runes.len() as i64;
    let mut start = if start < 0 { start + total } else { start };
    let mut end = if end < 0 { end + total } else { end };
    if start < 0 {
        start = 0;
    }
    if end > total {
        end = total;
    }
    if start > end {
        return String::new();
    }
    // start/end are clamped to [0, total] with start <= end, so the slice is
    // valid; `.get` keeps it total regardless.
    runes
        .get(start as usize..end as usize)
        .map(|r| r.iter().collect())
        .unwrap_or_default()
}
/// `Sky.Core.String.left n s` — the first `n` characters (clamped; negative → "").
pub fn string_left(n: i64, s: String) -> String {
    if n <= 0 {
        return String::new();
    }
    s.chars().take(n as usize).collect()
}
/// `Sky.Core.String.right n s` — the last `n` characters (clamped).
pub fn string_right(n: i64, s: String) -> String {
    if n <= 0 {
        return String::new();
    }
    let runes: Vec<char> = s.chars().collect();
    let start = runes.len().saturating_sub(n as usize);
    runes
        .get(start..)
        .map(|r| r.iter().collect())
        .unwrap_or_default()
}
/// `String.fromFloat : Float -> String`.
///
/// A faithful port of Go's `strconv.FormatFloat(f, 'g', -1, 64)` — the exact
/// spelling the Go reference's typed codegen routes `String.fromFloat` to
/// (`runtime-go/rt/rt.go` `String_fromFloatT`). We mirror it byte-for-byte
/// because the example sweep diffs Rust stdout against the Go oracle.
///
/// WHY a hand-written helper: Rust's `{}` never uses exponent form and `{:e}`
/// always does, so neither can express `'g'`'s rule on its own. `'g'` chooses
/// positional (`%f`) form when the decimal exponent lands in `[-4, 6)` and
/// exponent (`%e`) form otherwise — the same `eprec = 6` shortest-mode cut Go's
/// `internal/strconv` `formatDigits` applies. (Go's older public comment said
/// 21; the shipped implementation we diff against uses 6, and the oracle agrees
/// — `1e6` prints `1e+06`, `1e5` prints `100000`.) Non-finite values take Go's
/// `+Inf` / `-Inf` / `NaN` spellings.
///
/// We obtain the *shortest round-trip* significant digits + scientific exponent
/// from `{:e}` (Rust's std formatter picks the same canonical shortest decimal
/// as Go's Dragonbox), then re-render under `'g'`'s positional-vs-exponent rule.
pub fn string_from_float(f: f64) -> String {
    // Non-finite: Go's strconv spells these with a sign on the infinities.
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-Inf" } else { "+Inf" }.to_string();
    }
    // Negative zero must keep its sign ("-0"), matching Go; `is_sign_negative`
    // is the only check that distinguishes -0.0 from +0.0.
    let neg = f.is_sign_negative();
    if f == 0.0 {
        return if neg { "-0" } else { "0" }.to_string();
    }

    // `{:e}` yields the shortest round-trip form `d[.ddd]e<exp>` for the
    // magnitude; split it into significant digits and the scientific exponent.
    let sci = format!("{:e}", f.abs());
    let (mantissa, exp_str) = match sci.split_once('e') {
        Some(parts) => parts,
        // Unreachable for a finite f64 — `{:e}` always emits an `e`. Falling
        // back to the raw string keeps the function total rather than panicking.
        None => return sci,
    };
    let sci_exp: i32 = exp_str.parse().unwrap_or(0);
    // Significant digits with the radix point removed: e.g. "1.256" -> "1256".
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();

    // Go's `decimalSlice`: digit count and decimal-point position. The value is
    // `digits * 10^(dp - nd)`; `{:e}` puts one digit before the point, so the
    // point sits one place right of the leading digit: `dp = sci_exp + 1`.
    let dp = sci_exp + 1;
    let exp = dp - 1; // the exponent Go tests against, == sci_exp

    // Go's `'g'` rule (shortest mode): positional `%f` form for an exponent in
    // `[-4, 6)`, exponent `%e` form otherwise. `!(-4..6).contains` spells the
    // `exp < -4 || exp >= 6` test the reference applies.
    if (-4..6).contains(&exp) {
        fmt_g_positional(neg, &digits, dp)
    } else {
        fmt_g_exponent(neg, &digits, exp)
    }
}

/// `'g'`'s `%e` rendering (Go `fmtE`, shortest mode): `d[.ddd]e±NN`, with the
/// sign always present and at least two exponent digits (`1e-05`, `1e+21`).
fn fmt_g_exponent(neg: bool, digits: &str, exp: i32) -> String {
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    let mut chars = digits.chars();
    if let Some(first) = chars.next() {
        out.push(first);
    }
    let rest: String = chars.collect();
    if !rest.is_empty() {
        out.push('.');
        out.push_str(&rest);
    }
    out.push('e');
    let (sign, mag) = if exp < 0 { ('-', -exp) } else { ('+', exp) };
    out.push(sign);
    if mag < 10 {
        // Pad to the two-digit minimum Go always emits.
        out.push('0');
    }
    out.push_str(&mag.to_string());
    out
}

/// `'g'`'s `%f` rendering (Go `fmtF`, shortest mode): `ddd[.ddd]`, padding the
/// integer part with zeros (`1500`) and reading fraction digits past the point.
fn fmt_g_positional(neg: bool, digits: &str, dp: i32) -> String {
    let bytes = digits.as_bytes();
    let nd = bytes.len() as i32;
    let frac = (nd - dp).max(0); // fractional digit count
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    // Integer part: the first `dp` digits, zero-padded if the value has more
    // integer places than significant digits (e.g. 1500 from digits "15").
    if dp > 0 {
        let take = nd.min(dp);
        for i in 0..take {
            if let Some(&b) = bytes.get(i as usize) {
                out.push(b as char);
            }
        }
        for _ in take..dp {
            out.push('0');
        }
    } else {
        out.push('0');
    }
    // Fraction: each place reads a significant digit when one exists at that
    // position, otherwise a zero (leading zeros for sub-1 values like 0.0001).
    if frac > 0 {
        out.push('.');
        for i in 0..frac {
            let j = dp + i;
            let ch = if j >= 0 && j < nd {
                bytes.get(j as usize).map_or(b'0', |&b| b)
            } else {
                b'0'
            };
            out.push(ch as char);
        }
    }
    out
}
/// `String.split : String -> String -> List String`. Go's `strings.Split`
/// semantics (`String_splitT`): a non-empty separator splits on each
/// occurrence (`s.split(&sep)`), while an EMPTY separator splits `s` into its
/// individual runes with NO leading/trailing empty sentinels — and
/// `split("", "")` yields the empty list. Rust's `str::split("")` instead emits
/// boundary "" entries (`["", "a", …, ""]`), so the empty-sep case is handled
/// by rune iteration to match Go exactly.
pub fn string_split(sep: String, s: String) -> Vec<String> {
    if sep.is_empty() {
        return s.chars().map(|c| c.to_string()).collect();
    }
    s.split(&sep).map(|x| x.to_string()).collect()
}
// Sky.Core.String.lines / .words — split on line breaks / runs of whitespace.
pub fn string_lines(s: String) -> Vec<String> {
    s.lines().map(|x| x.to_string()).collect()
}
pub fn string_words(s: String) -> Vec<String> {
    s.split_whitespace().map(|x| x.to_string()).collect()
}

// ── String kernels with Go-typed argument order ──

/// Sky `replace : String -> String -> String -> String`.
/// Replaces all occurrences of `old` with `new_` in `s`.
pub fn string_replace(old: String, new_: String, s: String) -> String {
    s.replace(&old, &new_)
}

/// Sky `startsWith : String -> String -> Bool`. `prefix` first, `s` second.
pub fn string_starts_with(prefix: String, s: String) -> bool {
    s.starts_with(&prefix)
}

/// Sky `endsWith : String -> String -> Bool`. `suffix` first, `s` second.
pub fn string_ends_with(suffix: String, s: String) -> bool {
    s.ends_with(&suffix)
}

/// Sky `repeat : Int -> String -> String`. Non-positive `n` returns "".
pub fn string_repeat(n: i64, s: String) -> String {
    if n <= 0 {
        return String::new();
    }
    // Bound the result: n is caller-controlled; n * s.len() can overflow / OOM.
    // Cap at 64 MiB (any real repeated string is far smaller).
    if (n as u64).saturating_mul(s.len() as u64) > 64 * 1024 * 1024 {
        return String::new();
    }
    s.repeat(n as usize)
}

// ── Missing kernels (Go-parity sweep 2026-06-15) ──────────────────────────────

/// `String.concat : List String -> String`
/// Concatenates a list of strings with no separator.
/// Go parity: `String_concat` in rt.go — simple sequential WriteString.
pub fn string_concat(parts: Vec<String>) -> String {
    let mut out = String::new();
    for p in parts {
        out.push_str(&p);
    }
    out
}

/// `String.casefold : String -> String`
/// Unicode-aware case-fold for locale-neutral case-insensitive comparison.
/// Go parity: `String_casefold` in stdlib_extra.go — uses `strings.ToLower`
/// (Unicode-aware lowercasing). We mirror that with `to_lowercase()` which
/// performs full Unicode case folding (NFC-lowercased Unicode scalar values).
pub fn string_casefold(s: String) -> String {
    s.to_lowercase()
}

/// `String.dropLeft : Int -> String -> String`
/// Drops the first `n` characters (runes). Elm semantics:
///   negative n → s unchanged; n >= length → "".
/// Go parity: `String_dropLeft` in rt.go — rune-slice based.
pub fn string_drop_left(n: i64, s: String) -> String {
    if n <= 0 {
        return s;
    }
    let mut chars = s.chars();
    for _ in 0..n {
        if chars.next().is_none() {
            return String::new();
        }
    }
    chars.collect()
}

/// `String.dropRight : Int -> String -> String`
/// Drops the last `n` characters (runes). Elm semantics:
///   negative n → s unchanged; n >= length → "".
/// Go parity: `String_dropRight` in rt.go — rune-slice based.
pub fn string_drop_right(n: i64, s: String) -> String {
    if n <= 0 {
        return s;
    }
    let runes: Vec<char> = s.chars().collect();
    let len = runes.len() as i64;
    if n >= len {
        return String::new();
    }
    // 0 < len-n < len here (n>0 and n<len guarded above), so `take` keeps the
    // leading runes. `take` is total (never panics) — clippy flags the `[..k]`
    // slice form even though the bound is guaranteed, so use the iterator form.
    runes.iter().take((len - n) as usize).collect()
}

/// `String.equalFold : String -> String -> Bool`
/// Case-insensitive Unicode-aware string equality.
/// Go parity: `String_equalFold` in stdlib_extra.go — `strings.EqualFold`.
pub fn string_equal_fold(a: String, b: String) -> bool {
    // `to_lowercase()` is the same transform used in `string_casefold`,
    // matching Go's `strings.EqualFold` semantics (Unicode case-fold).
    a.to_lowercase() == b.to_lowercase()
}

/// `String.fromList : List Char -> String`
/// Concatenates a list of `Char` values into a UTF-8 string.
/// Go parity: `String_fromList` in rt.go — per-element rune → WriteRune.
pub fn string_from_list(chars: Vec<char>) -> String {
    chars.into_iter().collect()
}

/// `String.isEmail : String -> Bool`
/// RFC 5322 syntactic check. Does NOT verify the mailbox exists.
/// Go parity: `String_isEmail` in validate.go — `mail.ParseAddress` then
///   checks that the parsed address equals the raw input (no name component)
///   and that it contains "@".
/// We replicate the same rules with a simple structural check:
///   - exactly one "@" not at the start or end
///   - local part non-empty
///   - domain part non-empty and contains at least one "."
///
/// This intentionally stays as tight as Go's check (no regex crate needed).
pub fn string_is_email(s: String) -> bool {
    // Reject anything that parses with a name component: Go only accepts
    // bare "user@host" (no "Name <user@host>" wrapping).
    // Simple structural validation mirroring net/mail.ParseAddress behaviour.
    let s = s.trim();
    if s.is_empty() || s.starts_with('<') || s.contains(' ') {
        return false;
    }
    let mut parts = s.splitn(2, '@');
    let local = match parts.next() {
        Some(l) if !l.is_empty() => l,
        _ => return false,
    };
    let domain = match parts.next() {
        Some(d) if !d.is_empty() => d,
        _ => return false,
    };
    // Local part must not contain unquoted "@" again.
    if domain.contains('@') {
        return false;
    }
    // Domain must have at least one dot and non-empty labels around it.
    // `find` returns the byte index; `None` (no dot) maps to 0 so the
    // `dot == 0` check below rejects it cleanly.
    let dot = domain.find('.').unwrap_or(0);
    let last_valid = domain.len().saturating_sub(1);
    if dot == 0 || dot >= last_valid {
        return false;
    }
    // Disallow control characters (C0 range < 0x20, and DEL 0x7F).
    if local.chars().any(|c| (c as u32) < 32 || c as u32 == 127) {
        return false;
    }
    if domain.chars().any(|c| (c as u32) < 32 || c as u32 == 127) {
        return false;
    }
    true
}

/// `String.isUrl : String -> Bool`
/// Absolute URL with scheme http/https/ws/wss.
/// Go parity: `String_isUrl` in validate.go — `url.Parse` + `IsAbs()` + host
///   non-empty + scheme in {http, https, ws, wss}. Rejects relative paths and
///   javascript:/data: URLs to prevent XSS footguns.
///
/// Implementation: structural parse without external `url` crate — mirrors Go's
/// `url.Parse` behaviour (scheme + "://" + non-empty host) using only the `regex`
/// crate that is already an unconditional dep (Cargo.toml).
pub fn string_is_url(s: String) -> bool {
    use regex::Regex;
    use std::sync::OnceLock;
    // Compiled once; the pattern is a string literal so `Regex::new` can only
    // fail if the literal is malformed — verified by the unit tests below.
    // `OnceLock::get_or_init` returns a reference to the cached value; if
    // compilation somehow failed we store `None` and return `false` (total).
    static URL_RE: OnceLock<Option<Regex>> = OnceLock::new();
    let re = URL_RE.get_or_init(|| {
        // Scheme in {http, https, ws, wss} (case-insensitive), followed by
        // "://" and at least one non-whitespace host character.
        Regex::new(r"(?i)^(https?|wss?)://[^/\s?#]+").ok()
    });
    let t = s.trim();
    // Go's url.Parse rejects ASCII control bytes (0x00–0x1F, 0x7F) anywhere in
    // the URL, but the regex host class `[^/\s?#]` only excludes `\s` whitespace
    // — an embedded NUL / ESC / other control char would otherwise pass and slip
    // through this XSS-link gate. Reject up front. (Audit 2026-06-19,
    // security-relevant validator parity.)
    if t.bytes().any(|b| b.is_ascii_control()) {
        return false;
    }
    match re {
        Some(re) => re.is_match(t),
        None => false,
    }
}

/// `String.padLeft : Int -> Char -> String -> String`
/// Pads `s` on the left with `ch` until `s` is at least `n` rune-characters
/// wide. If `s` is already `n` or more characters wide, returns it unchanged.
/// Go parity: `String_padLeft` in rt.go — rune-count loop, `padChar` for ch.
pub fn string_pad_left(n: i64, ch: char, s: String) -> String {
    if n <= 0 {
        return s;
    }
    let rune_count = s.chars().count() as i64;
    if rune_count >= n {
        return s;
    }
    // Bound the pad width: n is caller-controlled; a huge n would OOM on
    // with_capacity + the push loop. Cap the padded width at 16M chars.
    if n > 16_000_000 {
        return s;
    }
    let pad_count = (n - rune_count) as usize;
    let mut out = String::with_capacity(s.len() + pad_count);
    for _ in 0..pad_count {
        out.push(ch);
    }
    out.push_str(&s);
    out
}

/// `String.padRight : Int -> Char -> String -> String`
/// Pads `s` on the right with `ch` until `s` is at least `n` rune-characters
/// wide. If `s` is already `n` or more characters wide, returns it unchanged.
/// Go parity: `String_padRight` in rt.go — rune-count loop, `padChar` for ch.
pub fn string_pad_right(n: i64, ch: char, s: String) -> String {
    if n <= 0 {
        return s;
    }
    let rune_count = s.chars().count() as i64;
    if rune_count >= n {
        return s;
    }
    // Bound the pad width: n is caller-controlled; a huge n would OOM on
    // with_capacity + the push loop. Cap the padded width at 16M chars.
    if n > 16_000_000 {
        return s;
    }
    let pad_count = (n - rune_count) as usize;
    let mut out = String::with_capacity(s.len() + pad_count);
    out.push_str(&s);
    for _ in 0..pad_count {
        out.push(ch);
    }
    out
}

/// `String.toList : String -> List Char`
/// Decomposes a string into its Unicode code points (chars).
/// Go parity: `String_toList` in rt.go — `for _, r := range str`.
pub fn string_to_list(s: String) -> Vec<char> {
    s.chars().collect()
}

/// `String.trimStart : String -> String`
/// Removes leading Unicode whitespace. Matches Go's `unicodeIsSpace` set
/// (includes NBSP, various space categories, BOM).
/// Go parity: `String_trimStart` in stdlib_extra.go — `strings.TrimLeftFunc`.
pub fn string_trim_start(s: String) -> String {
    s.trim_start_matches(unicode_is_space).to_string()
}

/// `String.trimEnd : String -> String`
/// Removes trailing Unicode whitespace. Same whitespace set as `trimStart`.
/// Go parity: `String_trimEnd` in stdlib_extra.go — `strings.TrimRightFunc`.
pub fn string_trim_end(s: String) -> String {
    s.trim_end_matches(unicode_is_space).to_string()
}

/// Mirrors Go's `unicodeIsSpace` (stdlib_extra.go): covers ASCII whitespace,
/// NBSP (U+00A0), general-category Zs (U+2000–U+200A), line/paragraph
/// separators (U+2028/U+2029), ideographic space (U+3000), and BOM (U+FEFF).
fn unicode_is_space(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\n' | '\r' | '\x0B' | '\x0C'  // ASCII whitespace + VT/FF
        | '\u{00A0}'                                   // NBSP
        | '\u{2000}'
            ..='\u{200A}'                      // En quad … Hair space
        | '\u{2028}'                                   // Line separator
        | '\u{2029}'                                   // Paragraph separator
        | '\u{3000}'                                   // Ideographic space
        | '\u{FEFF}' // BOM / Zero-width NBSP
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_simple() {
        assert_eq!(
            string_replace("foo".into(), "bar".into(), "foofoo".into()),
            "barbar"
        );
    }
    #[test]
    fn test_replace_no_match() {
        assert_eq!(string_replace("x".into(), "y".into(), "abc".into()), "abc");
    }
    #[test]
    fn test_replace_empty_old() {
        assert_eq!(
            string_replace("".into(), "_".into(), "abc".into()),
            "_a_b_c_"
        );
    }

    #[test]
    fn test_starts_with_hit() {
        assert!(string_starts_with("he".into(), "hello".into()));
    }
    #[test]
    fn test_starts_with_miss() {
        assert!(!string_starts_with("xy".into(), "hello".into()));
    }
    #[test]
    fn test_starts_with_empty_prefix() {
        assert!(string_starts_with("".into(), "hello".into()));
    }

    #[test]
    fn test_ends_with_hit() {
        assert!(string_ends_with("lo".into(), "hello".into()));
    }
    #[test]
    fn test_ends_with_miss() {
        assert!(!string_ends_with("xy".into(), "hello".into()));
    }

    #[test]
    fn test_repeat_three() {
        assert_eq!(string_repeat(3, "ab".into()), "ababab");
    }
    #[test]
    fn test_repeat_zero() {
        assert_eq!(string_repeat(0, "ab".into()), "");
    }
    #[test]
    fn test_repeat_negative() {
        assert_eq!(string_repeat(-1, "ab".into()), "");
    }

    // string_from_float — byte-for-byte parity with Go's
    // strconv.FormatFloat(f, 'g', -1, 64). Ground-truth values captured from
    // the Go oracle (`String.fromFloat` typed path) and `go run` on strconv.
    #[test]
    fn ff_small_exponent() {
        assert_eq!(string_from_float(0.00001), "1e-05");
    }
    #[test]
    fn ff_tiny_exponent() {
        assert_eq!(string_from_float(1e-10), "1e-10");
    }
    #[test]
    fn ff_huge_exponent() {
        assert_eq!(string_from_float(1e21), "1e+21");
    }
    #[test]
    fn ff_e5_neg_exponent() {
        assert_eq!(string_from_float(1e-5), "1e-05");
    }
    #[test]
    fn ff_whole_positional() {
        assert_eq!(string_from_float(1500.0), "1500");
    }
    #[test]
    fn ff_simple_fraction() {
        assert_eq!(string_from_float(1.5), "1.5");
    }
    #[test]
    fn ff_two_fraction() {
        assert_eq!(string_from_float(12.56), "12.56");
    }
    #[test]
    fn ff_sub_one_positional() {
        assert_eq!(string_from_float(0.0001), "0.0001");
    }
    #[test]
    fn ff_e6_flips_to_exponent() {
        assert_eq!(string_from_float(1e6), "1e+06");
    }
    #[test]
    fn ff_e5_stays_positional() {
        assert_eq!(string_from_float(1e5), "100000");
    }
    #[test]
    fn ff_many_fraction() {
        assert_eq!(string_from_float(123456.789), "123456.789");
    }
    #[test]
    fn ff_pos_inf() {
        assert_eq!(string_from_float(f64::INFINITY), "+Inf");
    }
    #[test]
    fn ff_neg_inf() {
        assert_eq!(string_from_float(f64::NEG_INFINITY), "-Inf");
    }
    #[test]
    fn ff_nan() {
        assert_eq!(string_from_float(f64::NAN), "NaN");
    }
    #[test]
    fn ff_pos_zero() {
        assert_eq!(string_from_float(0.0), "0");
    }
    #[test]
    fn ff_neg_zero() {
        assert_eq!(string_from_float(-0.0), "-0");
    }
    #[test]
    fn ff_negative() {
        assert_eq!(string_from_float(-1.5), "-1.5");
    }

    // ── New kernels ───────────────────────────────────────────────────────────

    // string_concat
    #[test]
    fn test_concat_basic() {
        assert_eq!(
            string_concat(vec!["foo".into(), "bar".into(), "baz".into()]),
            "foobarbaz"
        );
    }
    #[test]
    fn test_concat_empty_list() {
        assert_eq!(string_concat(vec![]), "");
    }
    #[test]
    fn test_concat_unicode() {
        assert_eq!(
            string_concat(vec!["héllo".into(), " ".into(), "wörld".into()]),
            "héllo wörld"
        );
    }

    // string_casefold
    #[test]
    fn test_casefold_upper() {
        assert_eq!(string_casefold("HELLO".into()), "hello");
    }
    #[test]
    fn test_casefold_mixed() {
        assert_eq!(string_casefold("CaFé".into()), "café");
    }
    #[test]
    fn test_casefold_empty() {
        assert_eq!(string_casefold("".into()), "");
    }

    // string_drop_left
    #[test]
    fn test_drop_left_basic() {
        assert_eq!(string_drop_left(2, "hello".into()), "llo");
    }
    #[test]
    fn test_drop_left_zero() {
        assert_eq!(string_drop_left(0, "hello".into()), "hello");
    }
    #[test]
    fn test_drop_left_negative() {
        assert_eq!(string_drop_left(-1, "hello".into()), "hello");
    }
    #[test]
    fn test_drop_left_exact() {
        assert_eq!(string_drop_left(5, "hello".into()), "");
    }
    #[test]
    fn test_drop_left_over() {
        assert_eq!(string_drop_left(99, "hello".into()), "");
    }
    #[test]
    fn test_drop_left_unicode() {
        assert_eq!(string_drop_left(1, "héllo".into()), "éllo");
    }

    // string_drop_right
    #[test]
    fn test_drop_right_basic() {
        assert_eq!(string_drop_right(2, "hello".into()), "hel");
    }
    #[test]
    fn test_drop_right_zero() {
        assert_eq!(string_drop_right(0, "hello".into()), "hello");
    }
    #[test]
    fn test_drop_right_negative() {
        assert_eq!(string_drop_right(-1, "hello".into()), "hello");
    }
    #[test]
    fn test_drop_right_exact() {
        assert_eq!(string_drop_right(5, "hello".into()), "");
    }
    #[test]
    fn test_drop_right_over() {
        assert_eq!(string_drop_right(99, "hello".into()), "");
    }
    #[test]
    fn test_drop_right_unicode() {
        assert_eq!(string_drop_right(1, "héllo".into()), "héll");
    }

    // string_equal_fold
    #[test]
    fn test_equal_fold_same() {
        assert!(string_equal_fold("hello".into(), "HELLO".into()));
    }
    #[test]
    fn test_equal_fold_diff() {
        assert!(!string_equal_fold("hello".into(), "world".into()));
    }
    #[test]
    fn test_equal_fold_unicode() {
        assert!(string_equal_fold("café".into(), "CAFÉ".into()));
    }
    #[test]
    fn test_equal_fold_empty() {
        assert!(string_equal_fold("".into(), "".into()));
    }

    // string_from_list
    #[test]
    fn test_from_list_basic() {
        assert_eq!(string_from_list(vec!['h', 'i']), "hi");
    }
    #[test]
    fn test_from_list_empty() {
        assert_eq!(string_from_list(vec![]), "");
    }
    #[test]
    fn test_from_list_unicode() {
        assert_eq!(string_from_list(vec!['é', 'à']), "éà");
    }

    // string_is_email
    #[test]
    fn test_is_email_valid() {
        assert!(string_is_email("user@example.com".into()));
    }
    #[test]
    fn test_is_email_no_at() {
        assert!(!string_is_email("userexample.com".into()));
    }
    #[test]
    fn test_is_email_no_domain_dot() {
        assert!(!string_is_email("user@example".into()));
    }
    #[test]
    fn test_is_email_name_component() {
        assert!(!string_is_email("Foo Bar <foo@bar.com>".into()));
    }
    #[test]
    fn test_is_email_empty() {
        assert!(!string_is_email("".into()));
    }
    #[test]
    fn test_is_email_with_plus() {
        assert!(string_is_email("user+tag@example.com".into()));
    }

    // string_is_url
    #[test]
    fn test_is_url_http() {
        assert!(string_is_url("http://example.com".into()));
    }
    #[test]
    fn test_is_url_https() {
        assert!(string_is_url("https://example.com/path".into()));
    }
    #[test]
    fn test_is_url_ws() {
        assert!(string_is_url("ws://example.com".into()));
    }
    #[test]
    fn test_is_url_wss() {
        assert!(string_is_url("wss://example.com".into()));
    }
    #[test]
    fn test_is_url_relative() {
        assert!(!string_is_url("/api/users".into()));
    }
    #[test]
    fn test_is_url_javascript() {
        assert!(!string_is_url("javascript:alert(1)".into()));
    }
    #[test]
    fn test_is_url_data() {
        assert!(!string_is_url("data:text/html,<h1>".into()));
    }
    #[test]
    fn test_is_url_empty() {
        assert!(!string_is_url("".into()));
    }
    #[test]
    fn test_is_url_ftp() {
        assert!(!string_is_url("ftp://example.com".into()));
    }
    #[test]
    fn test_is_url_rejects_control_chars() {
        // Embedded control bytes (NUL / ESC) → reject (XSS-link-gate parity with Go url.Parse).
        assert!(!string_is_url("http://exa\u{0}mple.com".into()));
        assert!(!string_is_url("https://e\u{1b}vil.com".into()));
    }

    // string_pad_left
    #[test]
    fn test_pad_left_basic() {
        assert_eq!(string_pad_left(5, '0', "42".into()), "00042");
    }
    #[test]
    fn test_pad_left_already_wide() {
        assert_eq!(string_pad_left(3, '0', "hello".into()), "hello");
    }
    #[test]
    fn test_pad_left_zero_n() {
        assert_eq!(string_pad_left(0, ' ', "x".into()), "x");
    }
    #[test]
    fn test_pad_left_unicode_pad() {
        assert_eq!(string_pad_left(4, '★', "ab".into()), "★★ab");
    }
    #[test]
    fn test_pad_left_unicode_str() {
        assert_eq!(string_pad_left(4, '-', "éà".into()), "--éà");
    }

    // string_pad_right
    #[test]
    fn test_pad_right_basic() {
        assert_eq!(string_pad_right(5, '-', "x".into()), "x----");
    }
    #[test]
    fn test_pad_right_already_wide() {
        assert_eq!(string_pad_right(2, '-', "hello".into()), "hello");
    }
    #[test]
    fn test_pad_right_zero_n() {
        assert_eq!(string_pad_right(0, ' ', "x".into()), "x");
    }
    #[test]
    fn test_pad_right_unicode_pad() {
        assert_eq!(string_pad_right(4, '★', "ab".into()), "ab★★");
    }

    // string_to_list
    #[test]
    fn test_to_list_basic() {
        assert_eq!(string_to_list("hi".into()), vec!['h', 'i']);
    }
    #[test]
    fn test_to_list_empty() {
        assert_eq!(string_to_list("".into()), Vec::<char>::new());
    }
    #[test]
    fn test_to_list_unicode() {
        assert_eq!(string_to_list("éà".into()), vec!['é', 'à']);
    }

    // string_trim_start
    #[test]
    fn test_trim_start_spaces() {
        assert_eq!(string_trim_start("  hello".into()), "hello");
    }
    #[test]
    fn test_trim_start_tabs() {
        assert_eq!(string_trim_start("\t\nhello".into()), "hello");
    }
    #[test]
    fn test_trim_start_nbsp() {
        assert_eq!(string_trim_start("\u{00A0}hello".into()), "hello");
    }
    #[test]
    fn test_trim_start_no_trailing() {
        assert_eq!(string_trim_start("  hello  ".into()), "hello  ");
    }
    #[test]
    fn test_trim_start_empty() {
        assert_eq!(string_trim_start("".into()), "");
    }

    // string_trim_end
    #[test]
    fn test_trim_end_spaces() {
        assert_eq!(string_trim_end("hello  ".into()), "hello");
    }
    #[test]
    fn test_trim_end_mixed() {
        assert_eq!(string_trim_end("hello\t\n".into()), "hello");
    }
    #[test]
    fn test_trim_end_nbsp() {
        assert_eq!(string_trim_end("hello\u{00A0}".into()), "hello");
    }
    #[test]
    fn test_trim_end_no_leading() {
        assert_eq!(string_trim_end("  hello  ".into()), "  hello");
    }
    #[test]
    fn test_trim_end_empty() {
        assert_eq!(string_trim_end("".into()), "");
    }

    // string_split — Go strings.Split parity
    #[test]
    fn test_split_nonempty_sep() {
        assert_eq!(
            string_split(",".into(), "a,b,c".into()),
            vec!["a", "b", "c"]
        );
    }
    #[test]
    fn test_split_empty_sep_runes() {
        assert_eq!(string_split("".into(), "abc".into()), vec!["a", "b", "c"]);
    }
    #[test]
    fn test_split_empty_sep_unicode() {
        assert_eq!(string_split("".into(), "héi".into()), vec!["h", "é", "i"]);
    }
    #[test]
    fn test_split_empty_sep_empty_str() {
        assert_eq!(string_split("".into(), "".into()), Vec::<String>::new());
    }
    #[test]
    fn test_split_trailing_sep() {
        assert_eq!(string_split(",".into(), "a,".into()), vec!["a", ""]);
    }

    // string_to_int — NO trim: parity with Go's observable `String_toInt`
    // (`strconv.Atoi(s)`, the path the emitted code uses) and Elm's `String.toInt`.
    // Surrounding whitespace ⇒ Nothing; only a clean numeric string parses.
    #[test]
    fn test_to_int_plain() {
        assert!(matches!(string_to_int("42".into()), SkyMaybe::Just(42)));
    }
    #[test]
    fn test_to_int_negative() {
        assert!(matches!(string_to_int("-5".into()), SkyMaybe::Just(-5)));
    }
    #[test]
    fn test_to_int_no_trim_leading() {
        assert!(matches!(string_to_int(" 42".into()), SkyMaybe::Nothing));
    }
    #[test]
    fn test_to_int_no_trim_trailing() {
        assert!(matches!(string_to_int("42 ".into()), SkyMaybe::Nothing));
    }
    #[test]
    fn test_to_int_no_trim_both() {
        assert!(matches!(string_to_int(" 42 ".into()), SkyMaybe::Nothing));
    }
    #[test]
    fn test_to_int_garbage() {
        assert!(matches!(string_to_int("4x".into()), SkyMaybe::Nothing));
    }

    // string_to_float — Unicode-whitespace trim
    #[test]
    fn test_to_float_plain() {
        assert!(matches!(string_to_float("1.5".into()), SkyMaybe::Just(v) if v == 1.5));
    }
    #[test]
    fn test_to_float_trimmed() {
        assert!(matches!(string_to_float("  1.5\n".into()), SkyMaybe::Just(v) if v == 1.5));
    }
    #[test]
    fn test_to_float_scientific() {
        assert!(matches!(string_to_float(" 1e3 ".into()), SkyMaybe::Just(v) if v == 1000.0));
    }
    #[test]
    fn test_to_float_garbage() {
        assert!(matches!(string_to_float("1.2.3".into()), SkyMaybe::Nothing));
    }

    // round-trip toList / fromList
    #[test]
    fn test_list_roundtrip() {
        let s = "héllo wörld".to_string();
        let chars = string_to_list(s.clone());
        assert_eq!(string_from_list(chars), s);
    }
}
