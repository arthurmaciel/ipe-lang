//! The CSS declaration-value injection-safety policy — the single source of
//! truth for "is this CSS property value free of a breakout / script-sink /
//! exfiltration construct?".
//!
//! This leaf module holds ONLY the value-scan policy (`Css.safeValue`'s gate).
//! It lives here, in the dependency-free kernel crate, because two crates need
//! the identical decision and must never drift:
//!
//! * the **runtime** sanitizer (`css_safety::SafeCssValue::parse`) runs it at
//!   render as the authoritative boundary, and
//! * the **backend** consults it at emit time to decide whether a *direct
//!   literal* CSS value may be hoisted into a view's appearance literal table
//!   for the dev hot-swap loop — a value is hoist-eligible IFF this policy
//!   accepts it, so an un-sanitized value can never be baked as a default.
//!
//! Because the sanitizer keeps the caller's ORIGINAL bytes on success (it is a
//! pass/reject gate, not a transform), a value this policy accepts renders
//! byte-identically whether it took the direct path or the hoisted path, and
//! the runtime sanitizer re-runs on whatever the slot holds — a dev-pushed
//! patch is gated exactly as a compiled value is.
//!
//! Selectors and property *names* are deliberately NOT here: a selector is
//! structure (what a rule targets), not an appearance value, and never hoists.
//!
//! ## Policy: allowlist grammar, not denylist scan (parse, don't validate)
//!
//! The gate does NOT scan for known-bad substrings (fail-open: any construct
//! not on the list slips through). Instead it PARSES the value against a small
//! allowlisted CSS declaration-value grammar and accepts it IFF every token is
//! positively recognized — an unrecognized byte or an unrecognized function
//! rejects the whole value (fail-closed / deny-by-default). New sink keywords,
//! novel `url()` exfiltration schemes, and future breakout syntax are therefore
//! rejected without needing to be enumerated.
//!
//! The recognized grammar (a pragmatic subset of CSS Syntax Level 3 that covers
//! the declaration values Ipê's `Ipe.Css` builders produce — colours, lengths,
//! numbers, keywords, and the transform / gradient / `var` / `calc` function
//! forms):
//!
//! ```text
//! value    := token (sep token)*
//! sep      := whitespace | ',' | '/'          -- value / shorthand separators
//! token    := ident | number | hexcolor | string | function
//! function := ident '(' value? ')'            -- balanced; `url(...)` special-cased
//! ```
//!
//! A `url(...)` token is accepted only when its argument carries NO scheme
//! (no `:`) and no quote/paren/whitespace — i.e. a scheme-relative or path-only
//! reference. Any `url(https://…)`, `url(data:…)`, or `url(javascript:…)` is a
//! scheme URL (a CSS-level exfiltration / script-sink channel) and is rejected,
//! as is any function whose name is not on [`ALLOWED_FUNCTIONS`].

/// Function names accepted as `<ident>(` heads of a CSS value function token.
/// Every other function name rejects the value (deny-by-default): a value may
/// only invoke a colour / gradient / transform / sizing / custom-property
/// function from this closed set. `url` is present but its ARGUMENT is gated
/// separately (scheme-free only) by [`url_arg_is_safe`].
const ALLOWED_FUNCTIONS: &[&str] = &[
    // Colour.
    "rgb",
    "rgba",
    "hsl",
    "hsla",
    "hwb",
    "lab",
    "lch",
    "oklab",
    "oklch",
    "color",
    "color-mix",
    // Custom properties / computed values.
    "var",
    "calc",
    "min",
    "max",
    "clamp",
    "env",
    // Gradients.
    "linear-gradient",
    "radial-gradient",
    "conic-gradient",
    "repeating-linear-gradient",
    "repeating-radial-gradient",
    "repeating-conic-gradient",
    // Transforms.
    "translate",
    "translatex",
    "translatey",
    "translatez",
    "translate3d",
    "scale",
    "scalex",
    "scaley",
    "scalez",
    "scale3d",
    "rotate",
    "rotatex",
    "rotatey",
    "rotatez",
    "rotate3d",
    "skew",
    "skewx",
    "skewy",
    "matrix",
    "matrix3d",
    "perspective",
    // Sizing / grid.
    "repeat",
    "minmax",
    "fit-content",
    // Filters / effects.
    "blur",
    "brightness",
    "contrast",
    "drop-shadow",
    "grayscale",
    "hue-rotate",
    "invert",
    "opacity",
    "saturate",
    "sepia",
    "cubic-bezier",
    "steps",
    // Resource reference (argument gated scheme-free).
    "url",
    "format",
    "local",
    "attr",
    "counter",
    "counters",
];

/// The authoritative `Css.safeValue` decision. `true` iff `v` parses cleanly.
///
/// A value is accepted IFF it parses against the allowlisted CSS
/// declaration-value grammar (colours, lengths, numbers, keywords, and the
/// recognized function forms) with no unrecognized byte and no scheme-bearing
/// `url(...)`, in BOTH its raw and CSS-escape-decoded forms.
///
/// This is a pass/reject gate: the value's bytes are never rewritten, so a
/// `true` result means `v` is safe to emit verbatim. The runtime sanitizer and
/// the backend hoist-eligibility check both call exactly this function.
#[must_use]
pub fn css_value_is_safe(v: &str) -> bool {
    // Reject empty / whitespace-only values: there is no legitimate empty
    // declaration value, and accepting one masks a builder bug.
    if v.trim().is_empty() {
        return false;
    }
    // Parse against the allowlist grammar in the RAW form and again in the
    // CSS-escape-decoded form, so a hex-escaped payload (`\65 xpression`,
    // `\75 rl(...)`) cannot smuggle an unrecognized construct past the parse.
    // Decoding can only ever reveal MORE structure to the parser, never hide
    // it, so requiring both forms to parse is strictly fail-closed.
    let low = v.to_ascii_lowercase();
    value_parses(&low) && value_parses(&css_unescape(&low))
}

/// Parse `s` (already lowercased) as a whitespace/`,`/`/`-separated sequence of
/// recognized value tokens. Returns `false` on the first unrecognized byte,
/// unbalanced paren, unrecognized function name, or scheme-bearing `url(...)`.
fn value_parses(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while let Some(&c) = bytes.get(i) {
        // Separators between tokens: whitespace, comma, slash (shorthand),
        // and the percent sign (a bare `%` follows a number but tolerating it
        // as a separator keeps the tokenizer simple and adds no sink).
        if c.is_ascii_whitespace() || c == b',' || c == b'/' || c == b'%' {
            i += 1;
            continue;
        }
        // The `!important` priority flag: `!` is admitted ONLY when the trimmed
        // remainder of the value is exactly `important` (the sole legitimate
        // use of `!` in a declaration value). Any other `!…` is rejected.
        if c == b'!' {
            return s
                .get(i + 1..)
                .is_some_and(|rest| rest.trim_start() == "important");
        }
        // A quoted string token (e.g. a `content` value or a `url("…")` handled
        // inside `parse_function`): only reachable at top level for `content`,
        // and permitted only when it carries no breakout byte.
        if c == b'"' || c == b'\'' {
            match parse_string(bytes, i, c) {
                Some(next) => {
                    i = next;
                    continue;
                }
                None => return false,
            }
        }
        // A value token: an ident/number run, possibly a function `ident(...)`.
        match parse_token(bytes, i) {
            Some(next) => i = next,
            None => return false,
        }
    }
    true
}

/// Parse one value token starting at `start`: a run of token bytes, and — when
/// it is immediately followed by `(` — a balanced function call whose name is
/// on [`ALLOWED_FUNCTIONS`]. Returns the index just past the token, or `None`
/// if any byte is unrecognized or the function is not allowlisted.
fn parse_token(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    // Consume the ident/number/hexcolour run: the character class permitted in
    // a bare token. `:` is deliberately EXCLUDED so no scheme (`javascript:`,
    // `data:`) can appear anywhere outside the scheme-free `url()` argument.
    while bytes.get(i).is_some_and(|&b| is_token_byte(b)) {
        i += 1;
    }
    // A function call: the just-consumed run is the name, and `(` opens it.
    if bytes.get(i) == Some(&b'(') {
        let name = bytes.get(start..i)?;
        return parse_function(bytes, name, i);
    }
    // A bare token must have consumed at least one byte; otherwise `start`
    // points at an unrecognized byte and the value is rejected.
    if i > start { Some(i) } else { None }
}

/// Parse a balanced function call: `name` is the (lowercased) function name and
/// `open` indexes its `(`. The name must be on [`ALLOWED_FUNCTIONS`]. `url(...)`
/// gates its argument scheme-free via [`url_arg_is_safe`]; every other allowed
/// function recursively parses its argument list as a nested value.
fn parse_function(bytes: &[u8], name: &[u8], open: usize) -> Option<usize> {
    let name_str = core::str::from_utf8(name).ok()?;
    if !ALLOWED_FUNCTIONS.contains(&name_str) {
        return None;
    }
    // Find the matching close paren, tracking nesting so an inner function's
    // parens do not end this call early. Bail (reject) on an unbalanced call.
    let mut depth = 0usize;
    let mut close = None;
    let mut i = open;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let close = close?;
    let inner = bytes.get(open + 1..close)?;
    if name_str == "url" {
        if !url_arg_is_safe(inner) {
            return None;
        }
    } else {
        // The argument list is itself a value: parse it with the same grammar
        // so a smuggled construct inside a `calc(...)` / `var(...)` is caught.
        let inner_str = core::str::from_utf8(inner).ok()?;
        if !value_parses(inner_str) {
            return None;
        }
    }
    Some(close + 1)
}

/// A `url(...)` argument is safe ONLY when it references a resource with no
/// scheme: no `:` (which would introduce `javascript:`, `data:`, `https:` — the
/// CSS-level exfiltration / script-sink channels), no quote, no paren, no
/// whitespace, and no control byte. This is stricter than CSS (it forbids
/// quoted and absolute URLs) on purpose: a scheme-free, path-only reference is
/// the only `url()` shape that is not an exfil vector, so the gate admits
/// exactly that and rejects everything else (fail-closed).
fn url_arg_is_safe(inner: &[u8]) -> bool {
    let arg = inner.trim_ascii();
    if arg.is_empty() {
        return false;
    }
    // A leading `//` is a protocol-relative authority (`//host/path`) — a
    // network-fetch / exfiltration shape even without an explicit scheme. Only
    // a same-document / same-origin reference (relative path or root-absolute
    // `/path`, never `//host`) is admitted.
    if arg.starts_with(b"//") {
        return false;
    }
    arg.iter().all(|&b| is_url_path_byte(b))
}

/// Parse a quoted string token starting at the opening quote `q` at `start`.
/// Returns the index just past the closing quote, or `None` if the string is
/// unterminated or carries a breakout / control byte. CSS backslash escapes are
/// consumed as escaped pairs so `\"` does not close the string; the raw+decoded
/// double-parse in [`css_value_is_safe`] catches an escaped-hidden breakout.
fn parse_string(bytes: &[u8], start: usize, q: u8) -> Option<usize> {
    let mut i = start + 1;
    while let Some(&c) = bytes.get(i) {
        if c == b'\\' {
            // Skip the escaped byte (if any); the decoded-form parse re-scans.
            i += 2;
            continue;
        }
        if c == q {
            return Some(i + 1);
        }
        // No raw breakout / control byte inside a string token.
        if c == b'<' || c == b'>' || c == b'{' || c == b'}' || c == b';' || c < 0x20 {
            return None;
        }
        i += 1;
    }
    None
}

/// True for a byte permitted in a bare (non-function, non-string) value token:
/// ASCII letters, digits, `#` (hex colour), `.` `+` `-` (numbers/signs),
/// `_` and `-` (idents / custom-property names), `\` (a CSS backslash escape;
/// admitted in the RAW parse so the escape's DECODED bytes are what the grammar
/// judges — [`css_value_is_safe`] requires the decoded form to parse too, so an
/// escape can only DEFER the decision to the decode pass, never bypass it), and
/// non-ASCII (Unicode idents, e.g. a localized keyword). Deliberately EXCLUDES
/// `:` `;` `{` `}` `<` `>` `(` `)` `,` `/` `%` `"` `'` `*` `@` — every
/// declaration/ruleset/at-rule breakout and every scheme-introducing byte.
const fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'#' | b'.' | b'+' | b'-' | b'_' | b'\\') || b >= 0x80
}

/// True for a byte permitted inside a scheme-free `url()` path argument: token
/// bytes (including `\`, judged in the decode pass) plus the small set of
/// path/query characters (`/`, `?`, `&`, `=`, `~`). Excludes `:` (scheme),
/// quotes, parens, whitespace, and control bytes.
const fn is_url_path_byte(b: u8) -> bool {
    is_token_byte(b) || matches!(b, b'/' | b'?' | b'&' | b'=' | b'~')
}

/// Decode CSS backslash escapes (CSS Syntax Level 3 §4.3.7) for DETECTION
/// purposes only — the decoded string is never emitted; the caller keeps its
/// ORIGINAL string on success. A value that hides an unrecognized construct
/// behind a hex escape (`\75 rl(...)`, `\3a` for `:`) decodes to its literal
/// form here, so the second [`value_parses`] pass sees — and rejects — it.
///
/// Best-effort / fail-closed, not a spec-complete CSS tokenizer: an escape
/// decoding to an invalid Unicode scalar value is dropped rather than
/// reconstructed (erring toward "the parser sees less structure" is the safe
/// direction — a dropped escape can only make a value MORE likely to be
/// rejected, never sneak a construct past).
fn css_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 {
            match chars.peek() {
                Some(h) if h.is_ascii_hexdigit() => {
                    hex.push(*h);
                    chars.next();
                }
                _ => break,
            }
        }
        if !hex.is_empty() {
            // One trailing whitespace char is consumed as the escape's delimiter
            // (CSS Syntax §4.3.7), if present.
            if matches!(chars.peek(), Some(w) if w.is_whitespace()) {
                chars.next();
            }
            if let Ok(cp) = u32::from_str_radix(&hex, 16)
                && let Some(ch) = char::from_u32(cp)
            {
                out.push(ch);
            }
            continue;
        }
        // `\` followed by a non-hex char: CSS escapes that literal char.
        if let Some(next) = chars.next() {
            out.push(next);
        }
        // trailing lone backslash (no following char): dropped.
    }
    out
}

#[cfg(test)]
mod tests {
    use super::css_value_is_safe;

    #[test]
    fn rejects_breakouts_and_script_sinks() {
        assert!(!css_value_is_safe("expression(alert(1))"));
        assert!(!css_value_is_safe("0; background:url(javascript:alert(1))"));
        assert!(!css_value_is_safe("red</style><script>alert(1)</script>"));
        assert!(!css_value_is_safe("url( javascript:alert(1))"));
        assert!(!css_value_is_safe("behavior:url(x.htc)"));
        assert!(!css_value_is_safe("-moz-binding:url(x)"));
        assert!(!css_value_is_safe("a { color: red }"));
        assert!(!css_value_is_safe("@import url(x)"));
        // CSS-hex-escaped bypass (`\65 ` -> 'e') caught by the decoded re-parse.
        assert!(!css_value_is_safe("\\65 xpression(alert(1))"));
    }

    #[test]
    fn rejects_url_exfiltration_channels() {
        // A data-scheme SVG and any remote URL are CSS exfiltration /
        // script-sink channels — every scheme-bearing `url()` is rejected.
        assert!(!css_value_is_safe(
            "url(data:image/svg+xml,<svg/onload=alert(1)>)"
        ));
        assert!(!css_value_is_safe(
            "url(data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=)"
        ));
        assert!(!css_value_is_safe("url(https://evil.example/x.png)"));
        assert!(!css_value_is_safe("url(http://evil.example/beacon)"));
        assert!(!css_value_is_safe("url(//evil.example/x)"));
        assert!(!css_value_is_safe("url('https://evil.example/x')"));
        assert!(!css_value_is_safe("url(\"https://evil.example/x\")"));
        // A hex-escaped `url(` head cannot smuggle a scheme past the parse.
        assert!(!css_value_is_safe("\\75 rl(https://evil.example/x)"));
    }

    #[test]
    fn accepts_benign_values() {
        assert!(css_value_is_safe("#ff6600"));
        assert!(css_value_is_safe("8px"));
        assert!(css_value_is_safe("rgba(0,0,0,0.2)"));
        assert!(css_value_is_safe("1px solid #ccc"));
        assert!(css_value_is_safe("translateX(100px)"));
        assert!(css_value_is_safe("red"));
        assert!(css_value_is_safe("transparent"));
        assert!(css_value_is_safe("50%"));
        assert!(css_value_is_safe("0.5"));
        assert!(css_value_is_safe("-2px"));
        assert!(css_value_is_safe("360deg"));
    }

    #[test]
    fn accepts_function_value_forms() {
        // The transform / gradient / var / calc / color-mix forms the `Ipe.Css`
        // builders produce must all parse cleanly.
        assert!(css_value_is_safe("linear-gradient(90deg, red, blue)"));
        assert!(css_value_is_safe("var(--brand)"));
        assert!(css_value_is_safe("var(--brand, #fff)"));
        assert!(css_value_is_safe("calc(100% - 8px)"));
        assert!(css_value_is_safe("translate(10px, 20px)"));
        assert!(css_value_is_safe("scale(1.5)"));
        assert!(css_value_is_safe("rotate(45deg)"));
        assert!(css_value_is_safe("repeat(3, 1fr)"));
        assert!(css_value_is_safe(
            "color-mix(in srgb, currentColor 8%, transparent)"
        ));
        assert!(css_value_is_safe("clamp(1rem, 2vw, 3rem)"));
        // A scheme-free, path-only `url()` is the one accepted URL shape.
        assert!(css_value_is_safe("url(assets/bg.png)"));
        assert!(css_value_is_safe("url(/static/img/logo.svg)"));
    }

    #[test]
    fn accepts_important_flag_and_rejects_other_bang() {
        // The `!important` priority flag is the sole legitimate `!` in a value.
        assert!(css_value_is_safe("red !important"));
        assert!(css_value_is_safe("rgba(0,0,0,0.2) !important"));
        assert!(css_value_is_safe("1px solid #ccc!important"));
        // Any other `!…` is rejected — `!` is not a general value byte.
        assert!(!css_value_is_safe("red !imporant"));
        assert!(!css_value_is_safe("red ! url(x)"));
        assert!(!css_value_is_safe("!"));
    }

    #[test]
    fn rejects_unknown_functions() {
        // A function not on the allowlist is rejected even with a benign
        // argument — deny-by-default over the function name.
        assert!(!css_value_is_safe("evilfn(1px)"));
        assert!(!css_value_is_safe("image-set(x)"));
    }

    #[test]
    fn rejects_empty_value() {
        assert!(!css_value_is_safe(""));
        assert!(!css_value_is_safe("   "));
    }
}
