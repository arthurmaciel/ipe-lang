//! The `ipe watch` appearance-hot-swap classifier.
//!
//! On a source edit under `IPE_WATCH_HOT_APPEARANCE`, the watch loop already has
//! the just-produced emit and the previous one. This module decides, from those
//! two emitted Rust projects alone, whether the edit is **`AppearanceOnly`**
//! (only hoisted style-literal *values* changed) or **Logic** (anything else). On
//! `AppearanceOnly` the loop POSTs a `LiteralTable` patch to the running app and
//! skips the cargo rebuild; on Logic it recompiles as usual.
//!
//! ## Why the emit is the ground truth
//!
//! The emitter is the single source of what the running program does. Diffing
//! the *emitted Rust* (not a hand-rolled AST diff that could drift from it) makes
//! the classification conservative by construction: a hoisted style literal is
//! the only thing that can differ between two emits while everything else stays
//! byte-identical, because that is exactly the transform the hoist performs
//! (`__ipe_lit.get(N)` reads are stable; only the baked
//! `LiteralTable::from_defaults(&[…])` default *values* move). Any other source
//! change — new structure, a conditional, a Model-dependent value, an `update`
//! edit — perturbs the emitted Rust somewhere other than a defaults array's
//! contents, or changes a defaults array's *length*, and forces `Logic`.
//!
//! ## The invariant
//!
//! **Unprovable ⇒ Logic ⇒ recompile.** `AppearanceOnly` is returned *only* when,
//! after masking every `from_defaults(&[…])` array's contents, the two emits are
//! byte-identical AND every corresponding array has the same length. Nothing
//! that touches control flow, structure, a handler, or the Model can satisfy
//! that, so a logic edit can never be misclassified as appearance. A false
//! `Logic` is merely a slow rebuild; a false `AppearanceOnly` would be a
//! correctness bug — so the bias is always toward `Logic`.

use ipe_backend::EmittedProject;

/// The patch for one edited view.
///
/// Carries the running app's *current* baked-defaults signature (which keys the
/// runtime overlay — the app is not recompiled, so it still bakes these values)
/// plus the `(index, new_value)` deltas to apply.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewPatch {
    /// The PREVIOUS baked defaults, in emit order. This is the signature the
    /// running app's compiled `view` passes to `from_defaults`, so it is the key
    /// the runtime overlay matches — send the old values, not the new.
    pub defaults: Vec<String>,
    /// The appearance delta: `(index, new_value)` for each changed default.
    pub patch: Vec<(usize, String)>,
}

/// The classification of a source edit, derived from the emitted-Rust diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// The only delta is hoisted style-literal *values*; hot-swap without a
    /// recompile. One [`ViewPatch`] per view whose defaults changed (empty when
    /// two emits are byte-identical — a no-op edit, still on the fast path).
    AppearanceOnly(Vec<ViewPatch>),
    /// Anything else — recompile. This is the conservative fallback.
    Logic,
}

/// Classify an edit from the previous and new emitted projects.
///
/// Returns [`Classification::AppearanceOnly`] iff the two emits differ *only* in
/// the default values inside `LiteralTable::from_defaults(&[…])` arrays (same set
/// of files, same array count and lengths, everything outside the arrays
/// byte-identical); otherwise [`Classification::Logic`].
#[must_use]
pub fn classify(prev: &EmittedProject, next: &EmittedProject) -> Classification {
    // The Cargo.toml is code (deps, features) — any change there is Logic.
    if prev.cargo_toml != next.cargo_toml {
        return Classification::Logic;
    }
    // A file added or removed is a structural change.
    if prev.files.len() != next.files.len() {
        return Classification::Logic;
    }
    let mut view_patches = Vec::new();
    for (rel, prev_src) in &prev.files {
        let Some(next_src) = next.files.get(rel) else {
            // A renamed/removed path — structural.
            return Classification::Logic;
        };
        // Fast exit: identical file contributes nothing.
        if prev_src == next_src {
            continue;
        }
        match classify_file(prev_src, next_src) {
            FileDelta::AppearanceOnly(mut ps) => view_patches.append(&mut ps),
            FileDelta::Logic => return Classification::Logic,
        }
    }
    Classification::AppearanceOnly(view_patches)
}

/// The per-file classification result.
enum FileDelta {
    /// Only defaults-array values changed; one [`ViewPatch`] per changed array.
    AppearanceOnly(Vec<ViewPatch>),
    /// Any other textual difference.
    Logic,
}

/// The literal marker that opens a defaults array in the emitted Rust. Kept in
/// sync with the emitter's `literal_table_prologue`
/// (`from_defaults(&[…])`). If the emitter's spelling ever changes, no array is
/// recognised and every edit conservatively falls to `Logic` — safe, never a
/// false `AppearanceOnly`.
const DEFAULTS_OPEN: &str = "from_defaults(&[";

/// Diff two versions of one emitted file. Everything outside the
/// `from_defaults(&[…])` array contents must be byte-identical, and each
/// corresponding array must have the same number of elements; then the delta is
/// exactly the changed default values.
fn classify_file(prev_src: &str, next_src: &str) -> FileDelta {
    // Unparsable ⇒ conservative (Logic).
    let Some(prev_arrays) = scan_defaults_arrays(prev_src) else {
        return FileDelta::Logic;
    };
    let Some(next_arrays) = scan_defaults_arrays(next_src) else {
        return FileDelta::Logic;
    };
    // A differing number of defaults arrays is a structural change (a hoisted
    // view was added or removed).
    if prev_arrays.len() != next_arrays.len() {
        return FileDelta::Logic;
    }

    // The masked regions are: (1) every `from_defaults(&[…])` array's contents,
    // and (2) every hoisted-read TOTAL-FALLBACK literal
    // (`__ipe_lit.get(N).parse::<T>().unwrap_or(<literal>)`). A typed style value
    // is emitted at BOTH sites — the baked default AND the read's `unwrap_or`
    // fallback — from the SAME source literal, so a value edit moves them
    // together. The fallback is inert (it fires only if a patch fails to parse,
    // which never happens in dev/prod), and it always equals the corresponding
    // default, so masking it cannot admit a false `AppearanceOnly`: any genuine
    // logic change still perturbs the skeleton outside these masks. The `String`
    // hoist reads via `.to_string()` (no fallback), so it has no such site.
    let Some(prev_masks) = mask_spans(prev_src, &prev_arrays) else {
        return FileDelta::Logic;
    };
    let Some(next_masks) = mask_spans(next_src, &next_arrays) else {
        return FileDelta::Logic;
    };
    // A differing number of hoisted-read sites is itself structural.
    if prev_masks.len() != next_masks.len() {
        return FileDelta::Logic;
    }

    // If the skeletons (source with every masked region blanked) differ,
    // something other than a hoisted style value moved (structure, control flow,
    // a handler, a read index) → Logic.
    if skeleton(prev_src, &prev_masks) != skeleton(next_src, &next_masks) {
        return FileDelta::Logic;
    }

    let mut patches = Vec::new();
    for (pa, na) in prev_arrays.iter().zip(next_arrays.iter()) {
        // Same array count in a matching skeleton, but re-check length per array:
        // a length change is a literal added/removed inside one view → Logic.
        if pa.values.len() != na.values.len() {
            return FileDelta::Logic;
        }
        let mut patch = Vec::new();
        for (idx, (pv, nv)) in pa.values.iter().zip(na.values.iter()).enumerate() {
            if pv != nv {
                patch.push((idx, nv.clone()));
            }
        }
        if !patch.is_empty() {
            patches.push(ViewPatch {
                defaults: pa.values.clone(),
                patch,
            });
        }
    }
    FileDelta::AppearanceOnly(patches)
}

/// One `from_defaults(&[…])` occurrence located in a source file.
struct DefaultsArray {
    /// Byte offset of the first element char inside the array (just past `&[`).
    inner_start: usize,
    /// Byte offset of the closing `]`.
    inner_end: usize,
    /// The parsed (unescaped) default values, in order.
    values: Vec<String>,
}

/// A half-open byte span `[start, end)` of `src` masked out of the skeleton.
#[derive(Clone, Copy)]
struct MaskSpan {
    start: usize,
    end: usize,
}

/// Every masked region of `src`, in ascending, non-overlapping source order: the
/// contents of each `from_defaults(&[…])` array plus each hoisted-read
/// `unwrap_or(<literal>)` fallback argument. Returns `None` (⇒ `Logic`) if a
/// hoisted read's fallback cannot be located, so an unrecognised emit shape is
/// conservative.
fn mask_spans(src: &str, arrays: &[DefaultsArray]) -> Option<Vec<MaskSpan>> {
    let mut spans: Vec<MaskSpan> = arrays
        .iter()
        .map(|a| MaskSpan {
            start: a.inner_start,
            end: a.inner_end,
        })
        .collect();
    spans.extend(scan_hoisted_read_fallbacks(src)?);
    spans.sort_by_key(|s| s.start);
    Some(spans)
}

/// Rebuild `src` with every masked span blanked to a fixed placeholder, so two
/// files whose ONLY differences fall inside masked spans produce identical
/// skeletons. `spans` must be ascending and non-overlapping.
fn skeleton(src: &str, spans: &[MaskSpan]) -> String {
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0usize;
    for s in spans {
        if s.start < cursor {
            continue; // defensive: skip any overlap rather than slice-panic
        }
        out.push_str(&src[cursor..s.start]);
        out.push('\u{0}'); // fixed placeholder for the whole masked region
        cursor = s.end;
    }
    out.push_str(&src[cursor..]);
    out
}

/// The literal opening a hoisted style-value read in the emitted Rust.
const HOISTED_READ_OPEN: &str = "__ipe_lit.get(";
/// The total-fallback call spliced after a typed hoisted read's `parse::<T>()`.
const UNWRAP_OR_OPEN: &str = ".unwrap_or(";

/// Locate every typed hoisted-read total-fallback argument
/// (`__ipe_lit.get(N).parse::<T>().unwrap_or(<literal>)`) and return the span of
/// its `<literal>` argument (the bytes strictly inside the `unwrap_or(...)`
/// parens). A `String` hoist reads via `.to_string()` and has no `unwrap_or`, so
/// it contributes nothing. Returns `None` if an `unwrap_or` cannot be balanced,
/// so a malformed shape falls to `Logic`.
fn scan_hoisted_read_fallbacks(src: &str) -> Option<Vec<MaskSpan>> {
    let bytes = src.as_bytes();
    let mut spans = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = src[from..].find(HOISTED_READ_OPEN) {
        let read_at = from + rel;
        let after_read = read_at + HOISTED_READ_OPEN.len();
        // The fallback, if any, is the FIRST `.unwrap_or(` after this read but
        // before the NEXT hoisted read. A `String` hoist has none in that window.
        let window_end = src[after_read..]
            .find(HOISTED_READ_OPEN)
            .map_or(src.len(), |n| after_read + n);
        if let Some(u_rel) = src[after_read..window_end].find(UNWRAP_OR_OPEN) {
            let arg_start = after_read + u_rel + UNWRAP_OR_OPEN.len();
            let arg_end = match_close_paren(bytes, arg_start)?;
            spans.push(MaskSpan {
                start: arg_start,
                end: arg_end,
            });
        }
        from = after_read;
    }
    Some(spans)
}

/// Given `pos` just past an opening `(`, return the byte offset of its matching
/// `)` (the byte AT the close). Balances nested parens; ignores parens inside
/// string literals so a `")"` in a fallback string does not miscount. Returns
/// `None` if unbalanced (⇒ conservative `Logic`).
fn match_close_paren(bytes: &[u8], mut pos: usize) -> Option<usize> {
    let mut depth: usize = 1;
    while let Some(&b) = bytes.get(pos) {
        match b {
            b'"' => {
                let (_s, next) = parse_string_literal(bytes, pos)?;
                pos = next;
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(pos);
                }
            }
            _ => {}
        }
        pos += 1;
    }
    None
}

/// Locate every `from_defaults(&[…])` array in `src` and parse its elements.
///
/// Returns `None` (⇒ caller falls to `Logic`) if any occurrence cannot be parsed
/// as a well-formed list of Rust string literals — never a guess. A file with no
/// occurrences returns `Some(empty)`.
fn scan_defaults_arrays(src: &str) -> Option<Vec<DefaultsArray>> {
    let bytes = src.as_bytes();
    let mut arrays = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = src[search_from..].find(DEFAULTS_OPEN) {
        let open = search_from + rel;
        let inner_start = open + DEFAULTS_OPEN.len();
        let (values, inner_end) = parse_string_array(bytes, inner_start)?;
        arrays.push(DefaultsArray {
            inner_start,
            inner_end,
            values,
        });
        search_from = inner_end;
    }
    Some(arrays)
}

/// Parse a comma-separated list of Rust double-quoted string literals starting at
/// `pos` (the first byte after `&[`), through the closing `]`. Returns the
/// unescaped values and the byte offset of the `]`, or `None` on anything
/// unexpected (so the caller conservatively classifies `Logic`).
///
/// Only the escapes the emitter can produce via `{:?}` on a `String` are handled:
/// `\"`, `\\`, `\n`, `\r`, `\t`, `\0`, and `\u{…}`. Any other escape or a
/// non-string element yields `None`.
fn parse_string_array(bytes: &[u8], mut pos: usize) -> Option<(Vec<String>, usize)> {
    let mut values = Vec::new();
    loop {
        pos = skip_ws(bytes, pos);
        match bytes.get(pos)? {
            b']' => return Some((values, pos)),
            b'"' => {
                let (s, next) = parse_string_literal(bytes, pos)?;
                values.push(s);
                pos = skip_ws(bytes, next);
                match bytes.get(pos)? {
                    b',' => pos += 1,
                    b']' => return Some((values, pos)),
                    _ => return None,
                }
            }
            // An empty array (`from_defaults(&[])`) closes immediately; anything
            // else here (a non-string element) is unrecognised → conservative.
            _ => return None,
        }
    }
}

/// Skip ASCII whitespace, returning the next non-space offset.
fn skip_ws(bytes: &[u8], mut pos: usize) -> usize {
    while let Some(b) = bytes.get(pos) {
        if b.is_ascii_whitespace() {
            pos += 1;
        } else {
            break;
        }
    }
    pos
}

/// Parse one Rust double-quoted string literal beginning at `bytes[pos] == '"'`.
/// Returns the unescaped `String` and the offset just past the closing quote.
fn parse_string_literal(bytes: &[u8], pos: usize) -> Option<(String, usize)> {
    if *bytes.get(pos)? != b'"' {
        return None;
    }
    let mut i = pos + 1;
    let mut out = String::new();
    loop {
        match *bytes.get(i)? {
            b'"' => return Some((out, i + 1)),
            b'\\' => {
                let esc = *bytes.get(i + 1)?;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'0' => out.push('\0'),
                    b'u' => {
                        // `\u{XXXX}` — the only multi-byte escape `{:?}` emits.
                        let (ch, next) = parse_unicode_escape(bytes, i + 2)?;
                        out.push(ch);
                        i = next;
                        continue;
                    }
                    _ => return None,
                }
                i += 2;
            }
            // A raw byte in the string body. Recover the char via UTF-8 so
            // multi-byte source values (e.g. an accented CSS content string)
            // round-trip; a lone continuation byte is invalid → None.
            _ => {
                let ch = next_utf8_char(bytes, i)?;
                let len = ch.len_utf8();
                out.push(ch);
                i += len;
            }
        }
    }
}

/// Parse a `{HHHH}` payload of a `\u{…}` escape at `pos` (just past the `u`),
/// returning the decoded char and the offset just past the closing `}`.
fn parse_unicode_escape(bytes: &[u8], pos: usize) -> Option<(char, usize)> {
    if *bytes.get(pos)? != b'{' {
        return None;
    }
    let mut i = pos + 1;
    let mut code: u32 = 0;
    let mut digits = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'}' {
            if digits == 0 {
                return None;
            }
            let ch = char::from_u32(code)?;
            return Some((ch, i + 1));
        }
        let d = (b as char).to_digit(16)?;
        code = code.checked_mul(16)?.checked_add(d)?;
        digits += 1;
        if digits > 6 {
            return None;
        }
        i += 1;
    }
    None
}

/// Decode the UTF-8 char beginning at `bytes[pos]`, or `None` if the bytes are
/// not a valid UTF-8 sequence there.
fn next_utf8_char(bytes: &[u8], pos: usize) -> Option<char> {
    let rest = bytes.get(pos..)?;
    std::str::from_utf8(rest)
        .ok()
        .and_then(|s| s.chars().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipe_backend::EmittedProject;
    use std::collections::BTreeMap;

    fn project(files: &[(&str, &str)], cargo: &str) -> EmittedProject {
        let mut map = BTreeMap::new();
        for (path, body) in files {
            // `RelPath` validates a relative path; the emitter only ever emits
            // valid ones, so any test path here is well-formed.
            let rel = ipe_backend::RelPath::new(*path).expect("test rel path");
            map.insert(rel, (*body).to_owned());
        }
        EmittedProject {
            files: map,
            cargo_toml: cargo.to_owned(),
        }
    }

    fn table(defaults: &[&str]) -> String {
        let rendered: Vec<String> = defaults.iter().map(|d| format!("{d:?}")).collect();
        format!("from_defaults(&[{}])", rendered.join(", "))
    }

    /// Mirror the emitter's TYPED-Int hoist for a single-value view: the baked
    /// `from_defaults(&["N"])` prologue AND the read site whose `unwrap_or(Ni64)`
    /// fallback re-embeds the same source literal (the second changing site that
    /// the masking must absorb).
    fn typed_int_view(n: i64) -> String {
        format!(
            "fn view() {{ let __ipe_lit = ipe_runtime::web::LiteralTable::{}; \
             ui_padding_(__ipe_lit.get(0).parse::<i64>().unwrap_or({n}i64)) }}",
            table(&[&n.to_string()]),
        )
    }

    /// Assert `classify` returned `AppearanceOnly` and hand back its patches, so
    /// callers extract fields via [`first_patch`] (never a panicking index). A
    /// `Logic` result is the test's failure — surfaced by asserting the variant
    /// matches before destructuring; the `else` arm is dead once the assertion
    /// holds and returns an empty vec (no `panic!`/`unreachable!`).
    fn expect_appearance(prev: &EmittedProject, next: &EmittedProject) -> Vec<ViewPatch> {
        let got = classify(prev, next);
        assert!(
            matches!(got, Classification::AppearanceOnly(_)),
            "expected AppearanceOnly, got {got:?}"
        );
        if let Classification::AppearanceOnly(ps) = got {
            ps
        } else {
            Vec::new()
        }
    }

    /// The first view patch, asserting there is at least one. Returns an owned
    /// clone so no diverging fallback is needed for the (asserted-away) empty
    /// case.
    fn first_patch(ps: &[ViewPatch]) -> ViewPatch {
        assert!(!ps.is_empty(), "expected at least one view patch");
        ps.first().cloned().unwrap_or_default()
    }

    // ── (a) padding 12 → 16: AppearanceOnly, 1-entry patch ────────────────
    #[test]
    fn padding_value_edit_is_appearance_only() {
        let prev = project(
            &[(
                "src/main.rs",
                &format!("fn view() {{ let __ipe_lit = {}; body }}", table(&["12"])),
            )],
            "cargo",
        );
        let next = project(
            &[(
                "src/main.rs",
                &format!("fn view() {{ let __ipe_lit = {}; body }}", table(&["16"])),
            )],
            "cargo",
        );
        let ps = expect_appearance(&prev, &next);
        assert_eq!(ps.len(), 1);
        let p = first_patch(&ps);
        assert_eq!(p.defaults, vec!["12".to_owned()]);
        assert_eq!(p.patch, vec![(0, "16".to_owned())]);
    }

    // The real TYPED-Int emit shape: the source literal appears BOTH in the baked
    // defaults AND in the read's `unwrap_or(Ni64)` fallback. Both move on a
    // `padding 12 -> 16` edit; the fallback mask must absorb the second site so
    // the edit stays AppearanceOnly (this is the exact shape a naive
    // defaults-only diff misclassified as Logic).
    #[test]
    fn typed_int_padding_edit_with_read_fallback_is_appearance_only() {
        let prev = project(&[("src/main.rs", &typed_int_view(12))], "cargo");
        let next = project(&[("src/main.rs", &typed_int_view(16))], "cargo");
        let ps = expect_appearance(&prev, &next);
        assert_eq!(ps.len(), 1);
        let p = first_patch(&ps);
        assert_eq!(p.defaults, vec!["12".to_owned()]);
        assert_eq!(p.patch, vec![(0, "16".to_owned())]);
    }

    // But a change to the read site's STRUCTURE (not just the paired fallback
    // literal) is still Logic — e.g. a different kernel around the read. The mask
    // only blanks the `unwrap_or(...)` argument, never the surrounding call.
    #[test]
    fn changed_read_kernel_is_logic() {
        let prev = project(
            &[(
                "src/main.rs",
                &format!(
                    "let __ipe_lit = ipe_runtime::web::LiteralTable::{}; \
                     ui_padding_(__ipe_lit.get(0).parse::<i64>().unwrap_or(12i64))",
                    table(&["12"])
                ),
            )],
            "cargo",
        );
        let next = project(
            &[(
                "src/main.rs",
                &format!(
                    "let __ipe_lit = ipe_runtime::web::LiteralTable::{}; \
                     ui_spacing_(__ipe_lit.get(0).parse::<i64>().unwrap_or(12i64))",
                    table(&["12"])
                ),
            )],
            "cargo",
        );
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }

    // A colour edit across a multi-element array patches only the changed index.
    #[test]
    fn colour_channel_edit_patches_single_index() {
        let prev = project(&[("src/main.rs", &table(&["255", "0", "0"]))], "cargo");
        let next = project(&[("src/main.rs", &table(&["255", "128", "0"]))], "cargo");
        let ps = expect_appearance(&prev, &next);
        assert_eq!(ps.len(), 1);
        let p = first_patch(&ps);
        assert_eq!(p.defaults, vec!["255", "0", "0"]);
        assert_eq!(p.patch, vec![(1, "128".to_owned())]);
    }

    // A font-family (String-valued) edit — the String hoist surface.
    #[test]
    fn font_family_string_edit_is_appearance_only() {
        let prev = project(&[("src/main.rs", &table(&["monospace"]))], "cargo");
        let next = project(&[("src/main.rs", &table(&["serif"]))], "cargo");
        let ps = expect_appearance(&prev, &next);
        assert_eq!(first_patch(&ps).patch, vec![(0, "serif".to_owned())]);
    }

    // ── (b) structural edit: add an element ⇒ Logic ───────────────────────
    #[test]
    fn structural_edit_outside_array_is_logic() {
        let prev = project(
            &[(
                "src/main.rs",
                &format!("div([]) ; let __ipe_lit = {};", table(&["12"])),
            )],
            "cargo",
        );
        // A new `span([])` node appears — skeleton differs.
        let next = project(
            &[(
                "src/main.rs",
                &format!("div([]) ; span([]) ; let __ipe_lit = {};", table(&["12"])),
            )],
            "cargo",
        );
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }

    // Adding a hoisted literal (array grows) is structural → Logic.
    #[test]
    fn array_length_growth_is_logic() {
        let prev = project(&[("src/main.rs", &table(&["12"]))], "cargo");
        let next = project(&[("src/main.rs", &table(&["12", "16"]))], "cargo");
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }

    // A whole new hoisted view (a second defaults array appears) → Logic.
    #[test]
    fn new_defaults_array_is_logic() {
        let prev = project(&[("src/main.rs", &table(&["12"]))], "cargo");
        let next = project(
            &[(
                "src/main.rs",
                &format!("{} ... {}", table(&["12"]), table(&["red"])),
            )],
            "cargo",
        );
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }

    // ── (c) control-flow / conditional edit ⇒ Logic ───────────────────────
    #[test]
    fn conditional_edit_is_logic() {
        let prev = project(
            &[(
                "src/main.rs",
                &format!("if a {{ x }} let __ipe_lit = {};", table(&["12"])),
            )],
            "cargo",
        );
        let next = project(
            &[(
                "src/main.rs",
                &format!(
                    "if a {{ x }} else {{ y }} let __ipe_lit = {};",
                    table(&["12"])
                ),
            )],
            "cargo",
        );
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }

    // ── (d) Model-dependent value change ⇒ Logic ──────────────────────────
    // A Model-dependent value is NOT hoisted (only compile-time-constant style
    // literals are), so the change lands in ordinary emitted code — skeleton
    // differs → Logic.
    #[test]
    fn model_dependent_value_change_is_logic() {
        let prev = project(
            &[(
                "src/main.rs",
                &format!("text(model.count) ; {};", table(&["12"])),
            )],
            "cargo",
        );
        let next = project(
            &[(
                "src/main.rs",
                &format!("text(model.count + 1) ; {};", table(&["12"])),
            )],
            "cargo",
        );
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }

    // ── (e) update / logic edit ⇒ Logic ───────────────────────────────────
    #[test]
    fn update_body_edit_is_logic() {
        let prev = project(
            &[
                ("src/main.rs", &table(&["12"])),
                ("src/update.rs", "fn update(m) { m }"),
            ],
            "cargo",
        );
        let next = project(
            &[
                ("src/main.rs", &table(&["12"])),
                ("src/update.rs", "fn update(m) { m + 1 }"),
            ],
            "cargo",
        );
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }

    // A dependency/Cargo.toml change is Logic even with identical source files.
    #[test]
    fn cargo_toml_change_is_logic() {
        let prev = project(&[("src/main.rs", &table(&["12"]))], "cargo-a");
        let next = project(&[("src/main.rs", &table(&["12"]))], "cargo-b");
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }

    // A file added is structural.
    #[test]
    fn file_added_is_logic() {
        let prev = project(&[("src/main.rs", &table(&["12"]))], "cargo");
        let next = project(
            &[
                ("src/main.rs", &table(&["12"])),
                ("src/extra.rs", "fn extra() {}"),
            ],
            "cargo",
        );
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }

    // A renamed file (same count, different path) is structural.
    #[test]
    fn file_renamed_is_logic() {
        let prev = project(&[("src/a.rs", &table(&["12"]))], "cargo");
        let next = project(&[("src/b.rs", &table(&["12"]))], "cargo");
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }

    // A no-op edit (byte-identical emit) is AppearanceOnly with no patches — it
    // stays on the fast path (no recompile for a whitespace-only source edit
    // that the emitter normalises away).
    #[test]
    fn identical_emit_is_empty_appearance_patch() {
        let p = project(&[("src/main.rs", &table(&["12"]))], "cargo");
        assert_eq!(
            classify(&p, &p.clone()),
            Classification::AppearanceOnly(vec![])
        );
    }

    // Two views, only one edited: one patch, keyed by that view's own defaults.
    #[test]
    fn only_the_edited_view_is_patched() {
        let prev = project(
            &[(
                "src/main.rs",
                &format!("A {} B {} C", table(&["12"]), table(&["red"])),
            )],
            "cargo",
        );
        let next = project(
            &[(
                "src/main.rs",
                &format!("A {} B {} C", table(&["16"]), table(&["red"])),
            )],
            "cargo",
        );
        let ps = expect_appearance(&prev, &next);
        assert_eq!(ps.len(), 1);
        let p = first_patch(&ps);
        assert_eq!(p.defaults, vec!["12".to_owned()]);
        assert_eq!(p.patch, vec![(0, "16".to_owned())]);
    }

    // Escaped values (quotes/backslashes) round-trip through the parser so the
    // patch carries the unescaped source value the runtime overlay expects.
    #[test]
    fn escaped_value_edit_roundtrips_unescaped() {
        let prev = project(&[("src/main.rs", &table(&["a\"b"]))], "cargo");
        let next = project(&[("src/main.rs", &table(&["a\"c"]))], "cargo");
        let ps = expect_appearance(&prev, &next);
        let p = first_patch(&ps);
        assert_eq!(p.defaults, vec!["a\"b".to_owned()]);
        assert_eq!(p.patch, vec![(0, "a\"c".to_owned())]);
    }

    // A non-ASCII (multi-byte) value round-trips.
    #[test]
    fn multibyte_value_roundtrips() {
        let prev = project(&[("src/main.rs", &table(&["café"]))], "cargo");
        let next = project(&[("src/main.rs", &table(&["thé"]))], "cargo");
        let ps = expect_appearance(&prev, &next);
        assert_eq!(first_patch(&ps).patch, vec![(0, "thé".to_owned())]);
    }

    // A file with no hoisted literals whose body changes is Logic (no arrays to
    // absorb the delta).
    #[test]
    fn non_hoisted_file_body_change_is_logic() {
        let prev = project(&[("src/main.rs", "fn view() { plain }")], "cargo");
        let next = project(&[("src/main.rs", "fn view() { plainer }")], "cargo");
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }

    // Empty defaults arrays on both sides with a body change → Logic (nothing to
    // hoist, the change is real code).
    #[test]
    fn empty_arrays_with_body_change_is_logic() {
        let prev = project(&[("src/main.rs", &format!("x {} y", table(&[])))], "cargo");
        let next = project(&[("src/main.rs", &format!("z {} y", table(&[])))], "cargo");
        assert_eq!(classify(&prev, &next), Classification::Logic);
    }
}
