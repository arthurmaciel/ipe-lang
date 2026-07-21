//! The `<crate>_bindings.rs` wrapper emitter — the third artifact.
//!
//! Every binding renders as a `pub fn` bracketed by the BEGIN/END sentinels
//! from [`crate::naming`], keyed off the same `wrapper_ref_name` the `.ipei`
//! and `kernel.json` emitters use, so the DCE filter can drop an unreached
//! wrapper by name without parsing Rust.
//!
//! Soundness posture: every SYNC foreign call runs inside
//! `std::panic::catch_unwind` (a foreign panic becomes a typed `Err`, never a
//! process abort observed by well-typed Ipê); every ASYNC call runs inside
//! `tokio::task::spawn`, whose `JoinError` is the equivalent panic boundary.
//! `catch_unwind` is sound only under `panic = "unwind"`, so the module top
//! carries a `#[cfg(panic = "abort")] compile_error!` fence that fails the
//! build on the *effective* panic strategy — a manifest text-scan
//! ([`cargo_profile_panic_is_unwind`], kept as an advisory pre-check) cannot
//! see a workspace profile or a `RUSTFLAGS=-Cpanic=abort` override.
//!
//! The emitter is a total function over an already-validated [`PkgInfo`]: a
//! binding it cannot render soundly emits NOTHING (over-drop), never a
//! wrapper cargo would reject.

use crate::carrier::{Carrier, ClosureRet, ClosureSig, EnumDef, StructDef};
use crate::naming::{RustIdent, arg_name, rust_kernel_name, rust_safe_ident, wrapper_fn_ident};
use crate::num_coerce::{is_numeric_rust, num_saturate, num_widen_scalar};
use crate::pkginfo::{Effect, EnumArm, EnumVariantKind, FnInfo, FnShape, Param, PkgInfo};

/// A rendered coercion lifting an expression of the raw foreign type into the
/// wrapper's declared type.
pub type RetCoercion = Box<dyn Fn(&str) -> String>;

fn identity_coercion() -> RetCoercion {
    Box::new(str::to_owned)
}

// ── small text helpers ──────────────────────────────────────────────────────

/// Make an extern-crate reference absolute (`csv::X` → `::csv::X`).
///
/// An absolute path can never collide with a same-named runtime kernel module
/// re-exported at the app crate root. Rewrites `<crate>::` only at a path
/// start (preceded by a non-identifier char that is not `:`), so nested
/// generics (`Vec<csv::X>`) and already-absolute paths stay correct.
#[must_use]
pub fn absolutize_crate(krate: &str, s: &str) -> String {
    let pat = format!("{krate}::");
    let mut out = String::new();
    let mut prev = ' ';
    let mut rest = s;
    while !rest.is_empty() {
        let ident_prev = prev.is_alphanumeric() || prev == '_';
        if rest.starts_with(&pat) && !ident_prev && prev != ':' {
            out.push_str("::");
            out.push_str(&pat);
            prev = ':';
            rest = rest.get(pat.len()..).unwrap_or("");
        } else {
            let mut cs = rest.chars();
            if let Some(c) = cs.next() {
                out.push(c);
                prev = c;
            }
            rest = cs.as_str();
        }
    }
    out
}

/// Render a Rust string literal, escaping `\` and `"` so an enum-variant /
/// tag name containing those bytes cannot break out of the literal.
fn rust_str_lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// If the type is `Wrapper<inner>`, return `inner` (trimmed).
fn strip_generic1<'a>(wrapper: &str, s: &'a str) -> Option<&'a str> {
    let rest = s.trim().strip_prefix(wrapper)?.strip_prefix('<')?;
    if rest.is_empty() {
        return None;
    }
    rest.strip_suffix('>').map(str::trim)
}

/// For a `Result<T, E>` string return the Ok type `T`; otherwise the input
/// unchanged. Respects nested angle brackets so `Result<Vec<T>, E>` works.
fn ok_type_of_result(s: &str) -> String {
    let t = s.trim();
    t.strip_prefix("Result<").map_or_else(
        || t.to_owned(),
        |rest| first_type_arg(rest).trim().to_owned(),
    )
}

/// The first top-level type argument of a comma/angle-delimited list.
fn first_type_arg(rest: &str) -> &str {
    let mut depth = 0_i32;
    for (i, c) in rest.char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' | ',' if depth == 0 => return rest.get(..i).unwrap_or(rest),
            '>' | ')' => depth -= 1,
            _ => {}
        }
    }
    rest
}

/// Strip a leading `&`/`&mut ` borrow from a Rust type string.
fn strip_ref(s: &str) -> &str {
    let s1 = s.trim_start_matches(['&', ' ']);
    s1.strip_prefix("mut ").unwrap_or(s1).trim()
}

/// Convert a pkg path to the Rust crate import ident
/// (`some/crate-name` → `crate_name`).
#[must_use]
pub fn pkg_to_crate_import(path: &str) -> String {
    let last = path.split('/').rfind(|s| !s.is_empty()).unwrap_or(path);
    last.replace('-', "_")
}

// ── panic-profile advisory pre-check ────────────────────────────────────────

/// Advisory fast pre-check: `true` when the manifest text declares NO
/// `panic = "abort"` in any profile table.
///
/// The SOUND gate is the `#[cfg(panic = "abort")] compile_error!` fence
/// emitted in every bindings file — a text-scan cannot see a workspace-root
/// profile or a `RUSTFLAGS=-Cpanic=abort`; this merely produces a friendlier
/// early error when the emitted manifest itself selects abort.
#[must_use]
pub fn cargo_profile_panic_is_unwind(cargo_toml_text: &str) -> bool {
    !cargo_toml_text.lines().any(line_declares_abort)
}

fn line_declares_abort(line: &str) -> bool {
    let Some(rest) = line.trim_start_matches(' ').strip_prefix("panic") else {
        return false;
    };
    let Some(value) = rest.trim_start_matches([' ', '\t']).strip_prefix('=') else {
        return false;
    };
    let normalised: String = value
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '"' | '\''))
        .collect();
    normalised == "abort"
}

// ── Ipê-type → Rust-type mapping ────────────────────────────────────────────

/// Map an Ipê type string to the wrapper-side Rust type. Opaque names fall
/// back to `String`; callers that know the raw Rust type prefer it via
/// [`resolve_rust_type`].
fn ipe_type_to_rust(st: &str) -> String {
    if let Some(inner) = st.strip_prefix("List ") {
        return format!("Vec<{}>", ipe_type_to_rust(inner));
    }
    if let Some(inner) = st.strip_prefix("Maybe ") {
        return format!("IpeMaybe<{}>", ipe_type_to_rust(inner));
    }
    if let Some(rest) = st.strip_prefix("Result ") {
        let mut w = rest.split_whitespace();
        return match (w.next(), w.next(), w.next()) {
            (Some(e), Some(a), None) => format!(
                "IpeResult<{}, {}>",
                ipe_type_to_rust(e),
                ipe_type_to_rust(a)
            ),
            _ => "IpeResult<IpeError, String>".to_owned(),
        };
    }
    if let Some(rest) = st.strip_prefix("Dict String ") {
        return format!("HashMap<String, {}>", ipe_type_to_rust(rest));
    }
    if let Some(rest) = st.strip_prefix("Task Error ") {
        return format!("IpeTask<IpeError, {}>", ipe_type_to_rust(rest));
    }
    match st {
        "Int" => "i64",
        "Float" => "f64",
        "Bool" => "bool",
        "Char" => "char",
        "()" => "()",
        "Bytes" => "Vec<u8>",
        // The typed Ipê error lands on the runtime's `IpeError`, never the
        // opaque-fallback String.
        "Error" => "IpeError",
        // "String" maps to itself; every unrecognised opaque falls back to
        // String too (the raw-Rust-type override is preferred upstream).
        _ => "String",
    }
    .to_owned()
}

/// True when [`ipe_type_to_rust`] gives a faithful (non-fallback) mapping —
/// a primitive/container Ipê understands, not an opaque crate type that
/// merely defaults to `String`.
fn is_known_ipe(st: &str) -> bool {
    matches!(
        st,
        "String" | "Int" | "Float" | "Bool" | "Char" | "Bytes" | "()"
    ) || ["List ", "Maybe ", "Result ", "Dict String ", "Task Error "]
        .iter()
        .any(|p| st.starts_with(p))
}

/// Resolve an Ipê surface type to the Rust type used in a wrapper PARAMETER
/// (and static-method receiver) position. Known Ipê types use their direct
/// mapping (owned values — `String`, not `&str`; the call site borrows).
/// Genuinely-opaque types use the inspector's raw Rust type, absolutized.
fn resolve_rust_type(krate: &str, st: &str, rt_override: &str) -> String {
    let owned_inner = |raw: &str| -> String { absolutize_crate(krate, strip_ref(raw)) };
    // `Maybe <opaque>` / `List <opaque>`: the Ipê mapping would collapse the
    // opaque inner to `String` — take the inner from the raw override instead
    // so the declared container element is the real `::crate::T`.
    if let Some(rest) = st.strip_prefix("Maybe ")
        && !is_known_ipe(rest.trim())
        && let Some(opt_inner) = strip_generic1("Option", rt_override)
    {
        return format!("IpeMaybe<{}>", owned_inner(opt_inner));
    }
    if let Some(rest) = st.strip_prefix("List ")
        && !is_known_ipe(rest.trim())
        && let Some(vec_inner) = strip_generic1("Vec", rt_override)
    {
        return format!("Vec<{}>", owned_inner(vec_inner));
    }
    if is_known_ipe(st) {
        return ipe_type_to_rust(st);
    }
    if !rt_override.is_empty() {
        return absolutize_crate(krate, rt_override);
    }
    "String".to_owned()
}

/// True when the Rust type is a `Copy` primitive, so a field getter reads it
/// as `recv.field` (a `.clone()` on a Copy type is a clippy deny).
fn is_copy_rust(t: &str) -> bool {
    matches!(
        t.trim(),
        "i8" | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "isize"
            | "usize"
            | "f32"
            | "f64"
            | "bool"
            | "char"
    )
}

// ── sequence (slice/array/Vec) classification ───────────────────────────────

/// Shape of a slice/array Rust type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeqShape {
    Slice,
    Owned,
    Arr(usize),
    RefArr(usize),
}

/// Element kind: the byte fast path, or a general coercible element carrying
/// its (Rust type, Ipê type).
#[derive(Debug, Clone, PartialEq, Eq)]
enum SeqElem {
    U8,
    General { rust: String, ipe: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeqKind {
    shape: SeqShape,
    elem: SeqElem,
}

/// Sane array-size ceiling: a pathological `[u8; 999999999999]` from
/// adversarial rustdoc JSON must not emit an absurd array type — above the
/// ceiling the caller falls through to the opaque path.
const MAX_ARRAY_LEN: usize = 65536;

fn parse_array_len(digits: &str) -> Option<usize> {
    let t = digits.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    t.parse::<usize>().ok().filter(|&n| n <= MAX_ARRAY_LEN)
}

/// The `N]`-terminated length of a `[u8; N]` remainder.
fn digits_before_close(rest: &str) -> Option<usize> {
    let (digits, tail) = rest.split_once(']')?;
    if !tail.is_empty() {
        return None;
    }
    parse_array_len(digits)
}

/// A coercible List element: the closed numeric set, `bool`/`char`/`String`,
/// or a bare opaque name (Clone-ness already verified inspector-side).
/// `str`/`OsString`/`PathBuf` are denied explicitly.
fn is_coercible_elem(e: &str) -> bool {
    const ADMIT_PRIM: [&str; 15] = [
        "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
        "f32", "f64", "bool",
    ];
    let t = e.trim();
    if t.is_empty() || t.contains(['&', ' ', '<', '[', ',']) {
        return false;
    }
    ADMIT_PRIM.contains(&t)
        || t == "char"
        || t == "String"
        || (t
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
            && !matches!(t, "str" | "OsString" | "PathBuf"))
}

fn ipe_of_elem(e: &str) -> String {
    match e {
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128"
        | "isize" => "Int".to_owned(),
        "f32" | "f64" => "Float".to_owned(),
        "bool" => "Bool".to_owned(),
        "char" => "Char".to_owned(),
        _ => e.to_owned(),
    }
}

/// Classify a raw Rust type as an Ipê-coercible sequence (mirrors the
/// inspector's gate). `None` for `&mut [T]` and non-coercible elements.
fn seq_kind(raw: &str) -> Option<SeqKind> {
    let s = raw.trim();
    if s.starts_with("&mut ") {
        return None;
    }
    match s {
        "&[u8]" => {
            return Some(SeqKind {
                shape: SeqShape::Slice,
                elem: SeqElem::U8,
            });
        }
        "Vec<u8>" => {
            return Some(SeqKind {
                shape: SeqShape::Owned,
                elem: SeqElem::U8,
            });
        }
        _ => {}
    }
    if let Some(rest) = s.strip_prefix("&[u8; ")
        && let Some(n) = digits_before_close(rest)
    {
        return Some(SeqKind {
            shape: SeqShape::RefArr(n),
            elem: SeqElem::U8,
        });
    }
    if let Some(rest) = s.strip_prefix("[u8; ")
        && let Some(n) = digits_before_close(rest)
    {
        return Some(SeqKind {
            shape: SeqShape::Arr(n),
            elem: SeqElem::U8,
        });
    }
    seq_kind_general(s)
}

fn seq_kind_general(s: &str) -> Option<SeqKind> {
    let admit = |shape: SeqShape, e: &str| -> Option<SeqKind> {
        let t = e.trim();
        is_coercible_elem(t).then(|| SeqKind {
            shape,
            elem: SeqElem::General {
                rust: t.to_owned(),
                ipe: ipe_of_elem(t),
            },
        })
    };
    if let Some(rest) = s.strip_prefix("Vec<") {
        return rest
            .strip_suffix('>')
            .and_then(|e| admit(SeqShape::Owned, e));
    }
    if let Some(rest) = s.strip_prefix("&[") {
        let inner = rest.strip_suffix(']')?;
        return match inner.split_once(';') {
            Some((e, n)) => parse_array_len(n).and_then(|k| admit(SeqShape::RefArr(k), e)),
            None => admit(SeqShape::Slice, inner),
        };
    }
    if let Some(rest) = s.strip_prefix('[') {
        let inner = rest.strip_suffix(']')?;
        return match inner.split_once(';') {
            Some((e, n)) => parse_array_len(n).and_then(|k| admit(SeqShape::Arr(k), e)),
            None => None,
        };
    }
    None
}

// ── return-type translation ─────────────────────────────────────────────────

/// Translate a raw Rust (Ok-)type into the wrapper's declared inner return
/// type plus the coercion lifting an expression of the raw type into it.
///
/// Driven by the inspector's real Rust type — the source of truth for opaque
/// types — never the lossy Ipê type.
#[must_use]
pub fn translate_rust_ret(raw0: &str) -> (String, RetCoercion) {
    let raw = raw0.trim().to_owned();
    if raw.is_empty() || raw == "()" {
        return ("()".to_owned(), identity_coercion());
    }
    if let Some(comps) = tuple_components(&raw) {
        return translate_tuple_ret(&comps);
    }
    if let Some(sk) = seq_kind(&raw) {
        return translate_seq_ret(&sk);
    }
    if let Some(inner) = strip_generic1("Option", &raw) {
        let (dt, co) = translate_rust_ret(inner);
        let just = co("v");
        return (
            format!("IpeMaybe<{dt}>"),
            Box::new(move |e| {
                format!(
                    "match {e} {{ Some(v) => IpeMaybe::Just({just}), None => IpeMaybe::Nothing }}"
                )
            }),
        );
    }
    if let Some(inner) = strip_generic1("Vec", &raw) {
        let (dt, co) = translate_rust_ret(inner);
        let mapped = co("x");
        return (
            format!("Vec<{dt}>"),
            Box::new(move |e| {
                if mapped == "x" {
                    e.to_owned()
                } else {
                    format!("{e}.into_iter().map(|x| {mapped}).collect()")
                }
            }),
        );
    }
    if let Some(probe) = num_widen_scalar(&raw, "x") {
        let carrier = probe.carrier.to_owned();
        return (
            carrier,
            Box::new(move |e| num_widen_scalar(&raw, e).map_or_else(|| e.to_owned(), |w| w.expr)),
        );
    }
    if raw == "bool" || raw == "String" {
        return (raw, identity_coercion());
    }
    // A serde-bound return reduced to Value travels to Ipê as JSON text.
    // `to_string` on a Value is total; empty string is the safe floor for the
    // impossible failure.
    if raw == "serde_json::Value" {
        return (
            "String".to_owned(),
            Box::new(|e| format!("serde_json::to_string(&({e})).unwrap_or_default()")),
        );
    }
    if raw.starts_with('&') {
        let inner = strip_ref(&raw).to_owned();
        if inner == "str" || inner == "String" {
            return (
                "String".to_owned(),
                Box::new(|e| format!("{e}.to_string()")),
            );
        }
        let (dt, _) = translate_rust_ret(&inner);
        return (dt, Box::new(|e| format!("{e}.to_owned()")));
    }
    (raw, identity_coercion())
}

fn translate_seq_ret(sk: &SeqKind) -> (String, RetCoercion) {
    match &sk.elem {
        SeqElem::U8 => {
            // Owned/plain-array values are borrowed into the helper;
            // slice/ref-array values already are references.
            let borrow = matches!(sk.shape, SeqShape::Owned | SeqShape::Arr(_));
            (
                "Vec<i64>".to_owned(),
                Box::new(move |e| {
                    if borrow {
                        format!("from_u8_slice(&{e})")
                    } else {
                        format!("from_u8_slice({e})")
                    }
                }),
            )
        }
        SeqElem::General { rust, .. } => {
            let (elem_decl, elem_co) = translate_rust_ret(rust);
            let decl = format!("Vec<{elem_decl}>");
            if elem_decl == *rust {
                // Element maps to itself: owned is identity, borrowed clones.
                let owned = matches!(sk.shape, SeqShape::Owned);
                return (
                    decl,
                    Box::new(move |e| {
                        if owned {
                            e.to_owned()
                        } else {
                            format!("{e}.to_vec()")
                        }
                    }),
                );
            }
            // Per-element scalar coercion; slice/ref-array elements arrive as
            // `&T` — `|&x|` copies them out (all such elements are Copy).
            let by_value = matches!(sk.shape, SeqShape::Owned | SeqShape::Arr(_));
            let mapped = elem_co("x");
            (
                decl,
                Box::new(move |e| {
                    if by_value {
                        format!("{e}.into_iter().map(|x| {mapped}).collect::<Vec<_>>()")
                    } else {
                        format!("{e}.iter().map(|&x| {mapped}).collect::<Vec<_>>()")
                    }
                }),
            )
        }
    }
}

/// Split a Rust tuple type (`(A, B, ..)`) into its top-level component types,
/// respecting nested `<>`/`()` so `(u64, Result<T, E>)` yields two parts. A
/// non-tuple, the unit `()`, or a single-element `(T,)`/`(T)` group returns
/// `None` — only a genuine 2+-arity tuple is a multi-result carrier here.
fn tuple_components(raw: &str) -> Option<Vec<String>> {
    let t = raw.trim();
    let inner = t.strip_prefix('(')?.strip_suffix(')')?;
    // Reject a balanced-but-non-tuple parse like `(A, B) -> C` where the outer
    // parens do not wrap the whole type: the strip above only removes the first
    // `(` and last `)`, so verify the remaining depth never dips to zero early.
    let mut depth = 0_i32;
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0usize;
    for (i, c) in inner.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => {
                if depth == 0 {
                    return None; // unbalanced — the outer parens were not a wrapper
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                parts.push(inner.get(start..i).unwrap_or("").trim().to_owned());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    let tail = inner.get(start..).unwrap_or("").trim();
    if !tail.is_empty() {
        parts.push(tail.to_owned());
    }
    // A 1-tuple `(T,)` leaves a trailing empty part that we skipped, so a single
    // real component means "not a multi-result tuple" — leave it to the scalar
    // paths. Only 2+ components are the multi-result shape.
    if parts.len() < 2 { None } else { Some(parts) }
}

/// Translate a multi-result Rust tuple into its wrapper-declared inner type and
/// a coercion that destructures the raw tuple and coerces each component. Each
/// component rides its own [`translate_rust_ret`], so a `(u64, u32)` becomes
/// `(i64, i64)` with per-slot saturating widening — matching the `(Int, Int)`
/// the Ipê signature declares.
fn translate_tuple_ret(comps: &[String]) -> (String, RetCoercion) {
    let mut decls: Vec<String> = Vec::with_capacity(comps.len());
    let mut coercers: Vec<RetCoercion> = Vec::with_capacity(comps.len());
    for c in comps {
        let (d, co) = translate_rust_ret(c);
        decls.push(d);
        coercers.push(co);
    }
    let decl = format!("({})", decls.join(", "));
    (
        decl,
        Box::new(move |e| {
            let binders: Vec<String> = (0..coercers.len()).map(|i| format!("t{i}")).collect();
            let coerced: Vec<String> = coercers
                .iter()
                .enumerate()
                .map(|(i, co)| co(&format!("t{i}")))
                .collect();
            format!(
                "{{ let ({}) = {e}; ({}) }}",
                binders.join(", "),
                coerced.join(", ")
            )
        }),
    )
}

/// The raw Rust Ok-type a binding's result carries, with the fallible/effectful
/// `Result<Ok, E>` layer peeled. Mirrors [`WrapperCx::effective_raw_result`] +
/// the Ok-peel so the interface admission gate reasons over the exact type the
/// wrapper body will coerce.
fn effective_ok_raw(f: &FnInfo) -> String {
    let raw = f.results().first().map_or("", Param::rust_type_str);
    let base = if raw.is_empty() {
        let ipe = f.results().first().map_or("()", |r| {
            if r.foreign_ty.is_empty() {
                "()"
            } else {
                r.foreign_ty.as_str()
            }
        });
        ipe_type_to_rust(ipe)
    } else {
        raw.to_owned()
    };
    match f.effect() {
        Effect::Fallible | Effect::Effectful => ok_type_of_result(&base),
        Effect::Pure => base,
    }
}

/// Whether a binding's result is a multi-result tuple every component of which
/// the wrapper can soundly coerce — the admission predicate for the interface
/// tuple gate.
///
/// Sound only for a tuple of NUMERIC scalar components: each rides a total
/// saturating widen into its `Int`/`Float` carrier, so the emitted `(i64, ..)`
/// matches the `(Int, ..)` the Ipê signature declares. A tuple carrying a
/// String, an opaque handle, or a nested container is NOT admitted here — its
/// ownership/lifetime wiring is not in the tuple emitter, so it stays dropped
/// (over-drop, never an unsound bind).
#[must_use]
pub fn multi_result_tuple_is_coercible(f: &FnInfo) -> bool {
    // A self-returning setter or a borrow-reader threads a handle, handled by
    // their own gates — this predicate is only for the plain multi-result case.
    if f.self_returning() || f.is_borrow_reader() {
        return false;
    }
    let raw = effective_ok_raw(f);
    tuple_components(&raw).is_some_and(|comps| comps.iter().all(|c| is_numeric_rust(c.trim())))
}

// ── per-function wrapper context ────────────────────────────────────────────

/// Everything the per-shape emitters share for one binding, computed once.
struct WrapperCx<'a> {
    krate: &'a str,
    f: &'a FnInfo,
    /// `{kernel}_{ref_name}` — the human-facing label in the region comment.
    label: String,
    /// The emitted `pub fn` identifier.
    rust_name: String,
    /// Per-param raw Rust types (the inspector's `rustType` tier).
    raw_param_types: Vec<String>,
    /// Declared wrapper param types (own-ref params already de-borrowed).
    param_types: Vec<String>,
    /// Param slots declared owned whose call site re-borrows (`&argN`).
    own_ref_idx: Vec<usize>,
    is_instance: bool,
    is_static: bool,
    is_display_bridge: bool,
}

impl<'a> WrapperCx<'a> {
    fn new(krate: &'a str, kernel_name: &str, f: &'a FnInfo) -> Self {
        let ref_name = f.wrapper_ref_name();
        let surface_types: Vec<String> = f.params().iter().map(|p| p.ipe_type.clone()).collect();
        let raw_param_types: Vec<String> = f
            .params()
            .iter()
            .map(|p| p.rust_type_str().to_owned())
            .collect();
        let declared_raw: Vec<String> = surface_types
            .iter()
            .enumerate()
            .map(|(j, st)| {
                let rt = raw_param_types.get(j).map_or("", String::as_str);
                resolve_rust_type(krate, st, rt)
            })
            .collect();
        let is_instance = !f.recv_type().is_empty()
            && !f.method_name().is_empty()
            && f.params().first().is_some_and(|p| p.name == "self");
        let is_static = !f.recv_type().is_empty() && !f.method_name().is_empty() && !is_instance;
        let is_display_bridge = f.method_name() == "to_string" && is_instance;
        // Opaque borrow param → own-by-value (async `'static` escape, or a
        // mono-produced owned Ipê surface behind a `&C` wrapper param); the
        // call site re-borrows.
        let own_ref_idx: Vec<usize> = declared_raw
            .iter()
            .enumerate()
            .filter(|&(j, t)| {
                is_own_ref_ty(t)
                    && (f.effect() == Effect::Effectful || {
                        let s = surface_types.get(j).map_or("", String::as_str);
                        !s.is_empty() && !s.starts_with('&') && !is_known_ipe(s)
                    })
            })
            .map(|(j, _)| j)
            .collect();
        let param_types: Vec<String> = declared_raw
            .iter()
            .enumerate()
            .map(|(j, t)| {
                if own_ref_idx.contains(&j) {
                    t.get(1..).unwrap_or("").trim().to_owned()
                } else {
                    t.clone()
                }
            })
            .collect();
        Self {
            krate,
            f,
            label: format!("{kernel_name}_{ref_name}"),
            rust_name: wrapper_fn_ident(kernel_name, &ref_name),
            raw_param_types,
            param_types,
            own_ref_idx,
            is_instance,
            is_static,
            is_display_bridge,
        }
    }

    fn n_params(&self) -> usize {
        self.f.params().len()
    }

    fn raw_param(&self, j: usize) -> &str {
        self.raw_param_types.get(j).map_or("", String::as_str)
    }

    fn decl_param(&self, j: usize) -> &str {
        self.param_types.get(j).map_or("", String::as_str)
    }

    /// The declared wrapper parameter list. The instance receiver is bound
    /// `mut` so `&mut self` methods auto-ref (`#![allow(unused_mut)]`
    /// silences the no-op case); a Display bridge takes `impl Display`.
    fn param_decl(&self) -> String {
        if self.is_display_bridge {
            return "arg0: impl std::fmt::Display".to_owned();
        }
        if self.param_types.is_empty() {
            return "_: ()".to_owned();
        }
        let decls: Vec<String> = self
            .param_types
            .iter()
            .enumerate()
            .map(|(j, t)| {
                if j == 0 && self.is_instance {
                    format!("mut arg0: {t}")
                } else {
                    format!("{}: {t}", arg_name(j))
                }
            })
            .collect();
        decls.join(", ")
    }

    /// Shape-aware argument coercion at the call site.
    fn arg_call(&self, j: usize) -> String {
        let base = arg_name(j);
        if self.own_ref_idx.contains(&j) {
            return format!("&{base}");
        }
        let raw_ty = self.raw_param(j);
        let decl_ty = self.decl_param(j);
        if let Some(sk) = seq_kind(raw_ty) {
            return match (sk.shape, &sk.elem) {
                (SeqShape::Slice, SeqElem::U8) => format!("&to_u8_vec(&{base})"),
                (SeqShape::Owned, SeqElem::U8) => format!("to_u8_vec(&{base})"),
                (SeqShape::Arr(_), _) => format!("b{j}"),
                (SeqShape::RefArr(_), _) => format!("&b{j}"),
                (SeqShape::Slice, SeqElem::General { .. }) => format!("{base}.as_slice()"),
                (SeqShape::Owned, SeqElem::General { .. }) => base,
            };
        }
        if let Some(inner) = strip_generic1("Option", raw_ty) {
            // The wrapper takes IpeMaybe<declInner>; adapt the inner value to
            // the foreign Option<rawInner>.
            let opt = format!("ipe_maybe_to_option({base})");
            return match inner {
                "&str" => format!("{opt}.as_deref()"),
                "&String" => format!("{opt}.as_ref()"),
                _ if is_numeric_rust(inner) => {
                    format!("{opt}.map(|x| {})", num_saturate(inner, "x"))
                }
                _ if inner.starts_with('&') => format!("{opt}.as_ref()"),
                _ => opt,
            };
        }
        // A serde-bound param names the deserialised local the prelude binds.
        if raw_ty == "serde_json::Value" && decl_ty == "String" {
            return format!("sv_{j}");
        }
        if decl_ty == "String" && (raw_ty == "String" || raw_ty == "str") {
            // Owned by value. A BARE `str` raw can only be a conversion/
            // generic-bound substitute (an unsized by-value `str` param is
            // unrepresentable in a real signature): the host param is generic
            // (`impl Into<Id>` / `impl AsRef<str>` / `impl Display`), so the
            // owned `String` satisfies the bound where a `&String` would not
            // (`String: Into<Id>` via `Id: From<String>`; no such impl for
            // `&String`).
            return base;
        }
        if decl_ty == "String" && raw_ty == "&str" {
            return format!("{base}.as_ref()"); // &str/&Path/&OsStr via AsRef
        }
        if decl_ty == "String" {
            return format!("&{base}"); // borrowed &String
        }
        if raw_ty.is_empty() || raw_ty == decl_ty {
            return base;
        }
        if is_numeric_rust(raw_ty) && (decl_ty == "i64" || decl_ty == "f64") {
            return num_saturate(raw_ty, &base);
        }
        base
    }

    fn call_args(&self, from: usize) -> String {
        let args: Vec<String> = (from..self.n_params()).map(|j| self.arg_call(j)).collect();
        args.join(", ")
    }

    /// The declared inner return type + the coercion into it, from the raw
    /// Rust result (falling back to the Ipê mapping for synthetic bridges).
    fn effective_raw_result(&self) -> String {
        let raw = self.f.results().first().map_or("", Param::rust_type_str);
        if raw.is_empty() {
            let ipe = self.f.results().first().map_or("()", |r| {
                if r.foreign_ty.is_empty() {
                    "()"
                } else {
                    r.foreign_ty.as_str()
                }
            });
            ipe_type_to_rust(ipe)
        } else {
            raw.to_owned()
        }
    }

    fn ret_inner_and_coerce(&self) -> (String, RetCoercion) {
        if self.f.self_returning() {
            // Owned-threading setter: the wrapper returns the receiver by
            // value; the raw `&mut Self`/`()` return is discarded in the body.
            let t = self
                .param_types
                .first()
                .cloned()
                .unwrap_or_else(|| "()".to_owned());
            return (t, identity_coercion());
        }
        let eff_raw = self.effective_raw_result();
        // Fallible AND effectful both unwrap the Result's Ok type — the body
        // binds the unwrapped Ok value, so the inner type is the Ok type.
        let (t, co) = match self.f.effect() {
            Effect::Fallible | Effect::Effectful => {
                translate_rust_ret(&ok_type_of_result(&eff_raw))
            }
            Effect::Pure => translate_rust_ret(&eff_raw),
        };
        (absolutize_crate(self.krate, &t), co)
    }

    fn resolve_recv(&self) -> String {
        resolve_rust_type(self.krate, self.f.recv_type(), self.f.recv_rust_type())
    }

    /// The foreign call expression (instance / static / bridge / free forms).
    fn call_expr(&self) -> String {
        let fn_name = rust_safe_ident(self.f.name());
        // A serde-reduced generic return needs the explicit turbofish or Rust
        // cannot infer the reduced `T` (E0283).
        let serde_turbofish = if self.effective_raw_result() == "serde_json::Value" {
            "::<serde_json::Value>"
        } else {
            ""
        };
        if self.is_instance {
            return format!("arg0.{fn_name}{serde_turbofish}({})", self.call_args(1));
        }
        if self.is_static && self.f.name() == "from_string" {
            // The Display/FromStr bridge renders UFCS.
            return format!(
                "<{} as std::str::FromStr>::from_str({})",
                self.resolve_recv(),
                self.call_args(0)
            );
        }
        if self.is_static {
            let recv = self.resolve_recv();
            // Turbofish-wrap a generic receiver so `Type<Param>::fn` is not
            // parsed as chained comparisons.
            let recv = if recv.contains('<') {
                format!("<{recv}>")
            } else {
                recv
            };
            return format!("{recv}::{fn_name}{serde_turbofish}({})", self.call_args(0));
        }
        // Free fn: absolute `::<crate>` path; a submodule fn uses its full
        // crate-relative call path.
        let call_target = if self.f.call_path().is_empty() {
            fn_name
        } else {
            self.f.call_path().to_owned()
        };
        format!(
            "::{}::{call_target}{serde_turbofish}({})",
            self.krate,
            self.call_args(0)
        )
    }

    /// Fixed-array params bind fallible conversions to `bN` locals; a length
    /// mismatch early-returns `Err` (no panic).
    fn arr_prelude(&self) -> Vec<String> {
        (0..self.n_params())
            .filter_map(|j| {
                let sk = seq_kind(self.raw_param(j))?;
                let n = match sk.shape {
                    SeqShape::Arr(n) | SeqShape::RefArr(n) => n,
                    SeqShape::Slice | SeqShape::Owned => return None,
                };
                Some(match sk.elem {
                    SeqElem::U8 => format!(
                        "let b{j}: [u8; {n}] = match to_u8_array::<IpeError, {n}>(&arg{j}) \
                         {{ IpeResult::Ok(a) => a, IpeResult::Err(e) => return IpeResult::Err(e), }};"
                    ),
                    SeqElem::General { rust, .. } => format!(
                        "let b{j}: [{rust}; {n}] = match to_array::<IpeError, {rust}, {n}>(&arg{j}) \
                         {{ IpeResult::Ok(a) => a, IpeResult::Err(e) => return IpeResult::Err(e), }};"
                    ),
                })
            })
            .collect()
    }

    /// Serde-bound params (`serde_json::Value` foreign type behind a `String`
    /// wrapper param) deserialise before the call, early-returning `Err` on
    /// malformed JSON. A `Value`-handle param (wrapper already takes `Value`)
    /// gets no prelude and passes through.
    fn serde_prelude(&self) -> Vec<String> {
        (0..self.n_params())
            .filter(|&j| self.raw_param(j) == "serde_json::Value" && self.decl_param(j) == "String")
            .map(|j| {
                format!(
                    "let sv_{j}: serde_json::Value = match \
                     serde_json::from_str::<serde_json::Value>(&arg{j}) \
                     {{ Ok(v) => v, Err(e) => return IpeResult::Err(ipe_error_from_foreign(e)), }};"
                )
            })
            .collect()
    }
}

/// Opaque borrow shape eligible for own-by-value declaration: `&T` where `T`
/// is not `str`/`String`/another borrow/a slice.
fn is_own_ref_ty(t: &str) -> bool {
    if !t.starts_with('&') || t.starts_with("&mut ") {
        return false;
    }
    let rest = t.get(1..).unwrap_or("").trim();
    rest != "str"
        && rest != "String"
        && !rest.starts_with('&')
        && !rest.starts_with('[')
        && !rest.is_empty()
}

/// Methods/statics on a receiver with an unresolved generic type parameter
/// (`DateTime<Tz>`) cannot be wrapped concretely — loose heuristic: `<` then
/// an uppercase char then `,`/`>`/end or a ≤2-char lowercase tail.
fn has_generic_recv_param(recv_rust_type: &str, is_display_bridge: bool) -> bool {
    if is_display_bridge {
        return false;
    }
    let Some(pos) = recv_rust_type.find('<') else {
        return false;
    };
    let after = recv_rust_type.get(pos + 1..).unwrap_or("");
    let mut cs = after.chars();
    let Some(c) = cs.next() else {
        return false;
    };
    let rest = cs.as_str();
    c.is_uppercase()
        && (rest.is_empty()
            || rest.starts_with(',')
            || rest.starts_with('>')
            || (rest.chars().next().is_some_and(char::is_lowercase) && rest.chars().count() <= 2))
}

// ── the emitter ─────────────────────────────────────────────────────────────

/// Emit the `<crate>_bindings.rs` wrapper module for one crate.
#[must_use]
pub fn emit_bindings(pkg: &PkgInfo) -> String {
    let kernel = rust_kernel_name(pkg.pkg_path());
    let krate = pkg_to_crate_import(pkg.pkg_path());
    let mut lines: Vec<String> = vec![
        format!(
            "// Code generated by ipe-ffi-inspector from {}. DO NOT EDIT.",
            pkg.pkg_path()
        ),
        format!("// Re-run `ipe add {}` to regenerate.", pkg.pkg_path()),
        String::new(),
        "#![allow(unused_imports, unused_mut, dead_code)]".to_owned(),
        String::new(),
        "// The catch_unwind boundary converts a foreign panic into a typed Err;".to_owned(),
        "// that conversion is sound only under panic=unwind, so refuse to build".to_owned(),
        "// under any configuration that selects panic=abort.".to_owned(),
        "#[cfg(panic = \"abort\")]".to_owned(),
        "compile_error!(\"ipe_ffi catch_unwind boundary requires panic=unwind\");".to_owned(),
        String::new(),
        "use crate::*;".to_owned(),
        "use std::collections::HashMap;".to_owned(),
        String::new(),
    ];
    for f in pkg.fns() {
        lines.extend(emit_fn_region(&krate, &kernel, f));
    }
    lines.push(String::new());
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The `wrapper_ref_name` set whose wrapper regions [`emit_bindings`] emits.
///
/// The survivor gate the interface emitter keys off. Runs the SAME region
/// emitter, so it cannot drift from the emitted text.
#[must_use]
pub fn surviving_ref_names(pkg: &PkgInfo) -> std::collections::BTreeSet<String> {
    let kernel = rust_kernel_name(pkg.pkg_path());
    let krate = pkg_to_crate_import(pkg.pkg_path());
    pkg.fns()
        .iter()
        .filter(|f| !emit_fn_region(&krate, &kernel, f).is_empty())
        .map(super::pkginfo::FnInfo::wrapper_ref_name)
        .collect()
}

/// The synthesised wrapper for a closed (zero-type-param) generic instance,
/// or empty when the binding is not one / the synthesiser drops it.
fn closed_instance_lines(kernel_name: &str, f: &FnInfo) -> Vec<String> {
    let closed = f.generic().is_some_and(|g| g.params.is_empty());
    if !closed {
        return Vec::new();
    }
    match crate::instance::synthesise_generic_wrapper(kernel_name, f) {
        Some(crate::instance::WrapperResult::Emitted(w)) => {
            w.source.lines().map(str::to_owned).collect()
        }
        // A rejection on a closed instance is an over-drop at add time: the
        // binding is simply absent (no call site exists yet to blame).
        _ => Vec::new(),
    }
}

/// One binding's sentinel-bracketed wrapper region; empty when the binding is
/// dropped (degenerate method / unresolved-generic receiver / trait fn).
fn emit_fn_region(krate: &str, kernel_name: &str, f: &FnInfo) -> Vec<String> {
    let cx = WrapperCx::new(krate, kernel_name, f);
    let body = match f.shape() {
        FnShape::EnumCtor {
            variant,
            kind,
            struct_fields,
        } => enum_ctor_lines(&cx, variant.as_str(), *kind, struct_fields),
        FnShape::EnumTag { arms, wildcard } => enum_tag_lines(&cx, arms, *wildcard),
        FnShape::EnumExtract {
            variant,
            kind,
            selector,
            field_count,
            wildcard,
        } => enum_extract_lines(
            &cx,
            variant.as_str(),
            *kind,
            selector.as_str(),
            *field_count,
            *wildcard,
        ),
        FnShape::FieldGet => field_get_lines(&cx),
        FnShape::FieldSet => field_set_lines(&cx),
        FnShape::ClosureAdapter { sig } => closure_adapter_lines(&cx, sig),
        FnShape::StructCtor { def } => struct_ctor_lines(&cx, def),
        FnShape::EnumDefCtor { def } => enum_def_ctor_lines(&cx, def),
        FnShape::Plain | FnShape::PkgVar => plain_lines(&cx),
    };
    // A trait-qualified / turbofished binding with NO open type params is a
    // CLOSED instance — its one wrapper is fully determined at add time, so
    // it renders through the instance synthesiser (UFCS, serde reduction,
    // async spawn) into the same sentinel region the flat tier would own.
    // Open generics stay demand-driven at the call site.
    let body = if body.is_empty() {
        closed_instance_lines(kernel_name, f)
    } else {
        body
    };
    if body.is_empty() {
        return Vec::new();
    }
    let mut out = vec![crate::naming::wrapper_begin_sentinel(&f.wrapper_ref_name())];
    out.extend(body);
    out.push(crate::naming::WRAPPER_END_SENTINEL.to_owned());
    out
}

/// Render a carrier as a concrete owned Rust type, absolutizing an opaque
/// handle through the crate so it can never collide with a runtime kernel
/// module re-exported at the app crate root.
fn carrier_rust_ty(krate: &str, c: &Carrier) -> String {
    match c {
        Carrier::Opaque(id) => absolutize_crate(krate, id.as_str()),
        _ => c.rust_owned().to_owned(),
    }
}

/// The `[rust.provide.closure]` adapter wrapper.
///
/// The wrapper takes an Ipê function value — already a
/// `Box<dyn Fn(A0, …) -> R + Send + Sync + 'static>` on the app side — and
/// returns a boxed Rust closure of the EXACT author-declared signature. The
/// captured Ipê value is moved into an `Arc` so the returned closure is
/// `Clone` for multi-call `Fn` re-entry.
///
/// Per-call soundness (design §3.3): each call re-enters the Ipê closure inside
/// `std::panic::catch_unwind` (the module-top `panic="abort"` fence makes that
/// sound). A `Total` (scalar-only) return has NO error channel, so a panic
/// `std::process::abort()`s — fabricating a `Default` would launder a real Ipê
/// panic into a silently-consumed wrong value. A `Result`/`Option` return folds
/// the panic in-band to `Err`/`None`.
///
/// Everything renders from the parsed [`ClosureSig`] — no raw manifest string
/// reaches this emitted Rust.
fn closure_adapter_lines(cx: &WrapperCx<'_>, sig: &ClosureSig) -> Vec<String> {
    let krate = cx.krate;
    // The crate-facing closure's parameter list and the Ipê closure's
    // parameter list are the SAME carriers in the sync P2 case — the Ipê fn's
    // arguments ARE the crate's arguments. Bind each `aN` and forward it.
    let params: Vec<(String, String)> = sig
        .params
        .iter()
        .enumerate()
        .map(|(j, c)| (arg_name(j), carrier_rust_ty(krate, c)))
        .collect();
    let crate_param_decls: Vec<String> = params
        .iter()
        .map(|(name, ty)| format!("{name}: {ty}"))
        .collect();
    let forwarded: Vec<String> = params.iter().map(|(name, _)| name.clone()).collect();
    let forwarded_args = forwarded.join(", ");
    // The Ipê function value the wrapper receives has this exact box type — it
    // is what the app-side backend emits for a fn value of this arrow.
    let ipe_fn_ty = format!("Box<{}>", sig.rust_dyn_fn());
    let crate_ret = match &sig.ret {
        ClosureRet::Total(sc) => sc.rust_owned().to_owned(),
        ClosureRet::Result(c) => format!("Result<{}, IpeError>", carrier_rust_ty(krate, c)),
        ClosureRet::Option(c) => format!("Option<{}>", carrier_rust_ty(krate, c)),
    };
    let bounds = sig.bounds.rust_suffix();
    let out_ty = if bounds.is_empty() {
        format!("Box<dyn Fn({}) -> {crate_ret}>", param_ty_list(&params))
    } else {
        format!(
            "Box<dyn Fn({}) -> {crate_ret} + {bounds}>",
            param_ty_list(&params)
        )
    };
    let wrapper = &cx.rust_name;
    // The captured Ipê closure. `Arc` gives the returned closure `Clone` for
    // multi-call re-entry without cloning the Ipê value itself.
    let call = if forwarded_args.is_empty() {
        "(__ipe_fn)()".to_owned()
    } else {
        format!("(__ipe_fn)({forwarded_args})")
    };
    let per_call_body = match &sig.ret {
        // A total (scalar) return has no error channel: a panic ABORTS. Never
        // fabricate a Default — that launders a real Ipê panic into a wrong
        // value the crate silently consumes. catch_unwind stays so the raw
        // unwind never crosses the foreign Fn frame (that is UB).
        ClosureRet::Total(_) => format!(
            "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {call})) \
             {{ Ok(v) => v, Err(_) => {{ \
             eprintln!(\"ipe_ffi: Ipê closure `{wrapper}` panicked; aborting \
             (total return has no error channel)\"); std::process::abort(); }} }}"
        ),
        ClosureRet::Result(_) => format!(
            "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {call})) \
             {{ Ok(inner) => inner, Err(_) => Err(str_err(\"foreign closure panicked\")) }}"
        ),
        ClosureRet::Option(_) => format!(
            "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {call})) \
             {{ Ok(inner) => inner, Err(_) => None }}"
        ),
    };
    let crate_params_joined = crate_param_decls.join(", ");
    vec![
        format!("pub fn {wrapper}(__ipe_fn: {ipe_fn_ty}) -> {out_ty} {{"),
        format!("    let __ipe_fn = std::sync::Arc::new(__ipe_fn);"),
        format!("    Box::new(move |{crate_params_joined}| {{"),
        format!("        let __ipe_fn = std::sync::Arc::clone(&__ipe_fn);"),
        format!("        {per_call_body}"),
        "    })".to_owned(),
        "}".to_owned(),
    ]
}

/// The `[rust.provide.struct]` definition + constructor wrapper.
///
/// Ipê DEFINES a nominal Rust type here: the emitter renders the `#[derive]`ed
/// struct definition, then a constructor wrapper that takes each field's owned
/// carrier value and builds the struct literal. This solves "define a Rust
/// type" with ZERO new trust surface — the body is built from decode-validated
/// carriers only, exactly like `enum_ctor_lines`. Everything renders from the
/// parsed [`StructDef`]; no raw manifest string reaches this emitted Rust.
///
/// An opaque field type absolutizes through the crate so the defined struct can
/// never collide with a runtime kernel module re-exported at the app crate root.
fn struct_ctor_lines(cx: &WrapperCx<'_>, def: &StructDef) -> Vec<String> {
    let krate = cx.krate;
    // The struct definition, with each opaque field absolutized through the
    // crate (a scalar field renders its owned Rust type directly).
    let mut out = def.definition_lines(&|id| absolutize_crate(krate, id.as_str()));
    // The constructor: one owned-carrier parameter per field, in order, folded
    // into the struct literal. Each parameter's inbound coercion is the identity
    // for the closed carrier set (the owned Rust type IS the Ipê value's carrier
    // — no narrowing), mirroring the struct-variant `enum_ctor_lines` path.
    let struct_name = def.name.as_str();
    let params: Vec<String> = def
        .fields
        .iter()
        .enumerate()
        .map(|(j, (_, c))| format!("{}: {}", arg_name(j), carrier_rust_ty(krate, c)))
        .collect();
    let assigns: Vec<String> = def
        .fields
        .iter()
        .enumerate()
        .map(|(j, (fname, _))| format!("{}: {}", fname.as_str(), arg_name(j)))
        .collect();
    out.push(String::new());
    out.push(format!(
        "pub fn {}({}) -> {struct_name} {{",
        cx.rust_name,
        params.join(", ")
    ));
    out.push(format!("    {struct_name} {{ {} }}", assigns.join(", ")));
    out.push("}".to_owned());
    out
}

/// The `[rust.provide.enum]` definition + one constructor per variant.
///
/// Ipê DEFINES a nominal Rust `enum` here: the emitter renders the `#[derive]`ed
/// enum definition once, then a constructor wrapper per variant. A unit variant
/// gets a nullary constructor (`E::V`); a tuple-payload variant gets one owned
/// carrier parameter per payload position, folded into `E::V(a0, …)`. This is
/// the `struct_ctor_lines` path generalised to a sum — the exact `EnumCtor`
/// inbound coercion an inspected enum already uses, applied to an author-defined
/// enum. Everything renders from the parsed [`EnumDef`]; no raw manifest string
/// reaches this emitted Rust.
///
/// An opaque payload absolutizes through the crate so the defined enum can never
/// collide with a runtime kernel module re-exported at the app crate root.
fn enum_def_ctor_lines(cx: &WrapperCx<'_>, def: &EnumDef) -> Vec<String> {
    let krate = cx.krate;
    // The enum definition, with each opaque payload absolutized through the
    // crate (a scalar payload renders its owned Rust type directly).
    let mut out = def.definition_lines(&|id| absolutize_crate(krate, id.as_str()));
    let enum_name = def.name.as_str();
    // One constructor per variant, named `<ctor>_<snake(variant)>` off the
    // manifest ctor name (`cx.rust_name`). Each parameter's inbound coercion is
    // the identity for the closed carrier set (the owned Rust type IS the Ipê
    // value's carrier — no narrowing), mirroring the tuple-variant `EnumCtor`.
    for v in &def.variants {
        let ctor = format!("{}_{}", cx.rust_name, variant_snake(v.name.as_str()));
        out.push(String::new());
        if v.payload.is_empty() {
            out.push(format!("pub fn {ctor}() -> {enum_name} {{"));
            out.push(format!("    {enum_name}::{}", v.name.as_str()));
        } else {
            let params: Vec<String> = v
                .payload
                .iter()
                .enumerate()
                .map(|(j, c)| format!("{}: {}", arg_name(j), carrier_rust_ty(krate, c)))
                .collect();
            let forwarded: Vec<String> = (0..v.payload.len()).map(arg_name).collect();
            out.push(format!(
                "pub fn {ctor}({}) -> {enum_name} {{",
                params.join(", ")
            ));
            out.push(format!(
                "    {enum_name}::{}({})",
                v.name.as_str(),
                forwarded.join(", ")
            ));
        }
        out.push("}".to_owned());
    }
    out
}

/// The `snake_case` of a variant identifier (`SetValue` → `set_value`), for the
/// per-variant constructor suffix. The input is a validated `RustIdent`, so this
/// only lowercases and inserts `_` at internal case boundaries.
fn variant_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The comma-joined declared parameter TYPE list (`i64, bool`) for a boxed
/// `dyn Fn(...)` type position.
fn param_ty_list(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(_, ty)| ty.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Plain fn / method / static / pkg-var wrapper (the fallible-carrier tier).
#[allow(clippy::too_many_lines)] // one linear body-shape cascade (self-return / borrow-thread / effect arms)
fn plain_lines(cx: &WrapperCx<'_>) -> Vec<String> {
    let f = cx.f;
    // Degenerate method: a self-param entry whose concrete receiver the
    // inspector could not determine — the receiver-tagged variant survives.
    let is_degenerate_method =
        f.recv_type().is_empty() && f.params().first().is_some_and(|p| p.name == "self");
    let unresolved_generic_recv = (cx.is_instance || cx.is_static)
        && has_generic_recv_param(f.recv_rust_type(), cx.is_display_bridge);
    // A trait associated fn renders only as UFCS via the generic-instance
    // path; the flat wrapper would be a duplicate or an E0425.
    let is_trait_fn = f.generic().is_some_and(|g| g.call.has_trait_qualifier());
    if is_degenerate_method || unresolved_generic_recv || is_trait_fn {
        return Vec::new();
    }
    let (ret_inner, ret_coerce) = cx.ret_inner_and_coerce();
    // A by-borrow reader threads its receiver (`arg0`) back beside the result:
    // the inner return type gains a trailing receiver component and the body
    // returns `(value, arg0)`. The `&self`/`&mut self` call only borrows `arg0`,
    // so it is still live to return. Async (effectful) readers move the receiver
    // into the spawned task and cannot thread it back — they keep the
    // `IPE-L0130` linearity backstop.
    let thread_recv = f.is_borrow_reader() && f.effect() != Effect::Effectful;
    let recv_rust_ty = cx.param_types.first().cloned().unwrap_or_default();
    let ret_inner = if thread_recv {
        format!("({ret_inner}, {recv_rust_ty})")
    } else {
        ret_inner
    };
    let ret_type = if f.effect() == Effect::Effectful {
        format!("IpeTask<{ret_inner}>")
    } else {
        format!("IpeResult<IpeError, {ret_inner}>")
    };
    let call = cx.call_expr();
    let effect_tag = match f.effect() {
        Effect::Pure => "pure",
        Effect::Fallible => "fallible",
        Effect::Effectful => "effectful",
    };
    let mut preludes = cx.arr_prelude();
    preludes.extend(cx.serde_prelude());
    let body = if f.self_returning() {
        let own_thread_args = cx.call_args(1);
        let method = rust_safe_ident(f.name());
        format!(
            "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {{ \
             arg0.{method}({own_thread_args}); arg0 }})) \
             {{ Ok(r) => ok_res(r), Err(_) => IpeResult::Err(str_err(\"foreign call panicked\")) }}"
        )
    } else if thread_recv {
        // By-borrow reader: the closure owns `arg0`, calls the borrowing method,
        // then hands the receiver back so it flows out beside the result. A
        // Fallible reader folds its `Err` arm normally (the handle is spent on
        // failure — the Ipê `Result Error (R, T)` carries no tuple there).
        match f.effect() {
            Effect::Fallible => format!(
                "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {{ let __r = {call}; (__r, arg0) }})) \
                 {{ Ok((Ok(v), recv)) => ok_res(({}, recv)), Ok((Err(e), _)) => IpeResult::Err(ipe_error_from_foreign(e)), \
                 Err(_) => IpeResult::Err(str_err(\"foreign call panicked\")) }}",
                ret_coerce("v")
            ),
            // Pure (and, defensively, any non-fallible non-effectful shape).
            _ => format!(
                "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {{ let __r = {call}; (__r, arg0) }})) \
                 {{ Ok((v, recv)) => ok_res(({}, recv)), Err(_) => IpeResult::Err(str_err(\"foreign call panicked\")) }}",
                ret_coerce("v")
            ),
        }
    } else {
        match f.effect() {
            Effect::Effectful => {
                // The async panic boundary is the spawned task's JoinError.
                // A fallible async fn's Ok is itself a Result: three arms.
                let is_async_fallible = cx.effective_raw_result().starts_with("Result<");
                let prelude_inline = if preludes.is_empty() {
                    String::new()
                } else {
                    let mut s = preludes.join(" ");
                    s.push(' ');
                    s
                };
                preludes = Vec::new(); // moved inside the async block
                if is_async_fallible {
                    format!(
                        "Box::pin(async move {{ {prelude_inline}let handle = tokio::task::spawn(async move {{ {call}.await }}); \
                         let guard = AbortOnDrop::new(handle.abort_handle()); let joined = handle.await; guard.defuse(); \
                         match joined \
                         {{ Ok(Ok(v)) => ok_res({}), Ok(Err(e)) => IpeResult::Err(ipe_error_from_foreign(e)), \
                         Err(join_err) => IpeResult::Err(ipe_error_from_foreign(join_err)) }} }})",
                        ret_coerce("v")
                    )
                } else {
                    format!(
                        "Box::pin(async move {{ {prelude_inline}let handle = tokio::task::spawn(async move {{ {call}.await }}); \
                         let guard = AbortOnDrop::new(handle.abort_handle()); let joined = handle.await; guard.defuse(); \
                         match joined \
                         {{ Ok(v) => ok_res({}), Err(join_err) => IpeResult::Err(ipe_error_from_foreign(join_err)) }} }})",
                        ret_coerce("v")
                    )
                }
            }
            Effect::Fallible => format!(
                "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {call})) \
                 {{ Ok(Ok(v)) => ok_res({}), Ok(Err(e)) => IpeResult::Err(ipe_error_from_foreign(e)), \
                 Err(_) => IpeResult::Err(str_err(\"foreign call panicked\")) }}",
                ret_coerce("v")
            ),
            Effect::Pure => format!(
                "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {})) \
                 {{ Ok(v) => ok_res(v), Err(_) => IpeResult::Err(str_err(\"foreign call panicked\")) }}",
                ret_coerce(&call)
            ),
        }
    };
    let mut out = vec![
        format!("// [{effect_tag}] {}", cx.label),
        format!(
            "pub fn {}({}) -> {ret_type} {{",
            cx.rust_name,
            cx.param_decl()
        ),
    ];
    out.extend(preludes.into_iter().map(|p| format!("    {p}")));
    out.push(format!("    {body}"));
    out.push("}".to_owned());
    out
}

/// Struct-field getter: infallible by construction — the inspector gated the
/// field type to the closed eligible set, so the projection/`.clone()` is
/// total and the wrapper returns the bare field type.
fn field_get_lines(cx: &WrapperCx<'_>) -> Vec<String> {
    let f = cx.f;
    let recv_rust = cx.resolve_recv();
    let field = f.method_name();
    let raw_ty = f.results().first().map_or("", Param::rust_type_str);
    let well_formed = f.results().len() == 1
        && !raw_ty.trim().is_empty()
        && !field.is_empty()
        && !recv_rust.trim().is_empty();
    if !well_formed {
        return Vec::new();
    }
    let (inner0, co) = translate_rust_ret(raw_ty);
    let inner = absolutize_crate(cx.krate, &inner0);
    let projection = format!("arg0.{}", rust_safe_ident(field));
    let access = if is_copy_rust(raw_ty) {
        projection
    } else {
        format!("{projection}.clone()")
    };
    vec![
        format!("// [field] {}", cx.label),
        format!("pub fn {}(arg0: {recv_rust}) -> {inner} {{", cx.rust_name),
        format!("    {}", co(&access)),
        "}".to_owned(),
    ]
}

/// Struct-field setter: a NEW receiver with one field replaced — Ipê
/// immutable-update value semantics, infallible by construction.
fn field_set_lines(cx: &WrapperCx<'_>) -> Vec<String> {
    let f = cx.f;
    let recv_rust = cx.resolve_recv();
    let field = f.method_name();
    let well_formed = !f.params().is_empty() && !field.is_empty() && !recv_rust.trim().is_empty();
    if !well_formed {
        return Vec::new();
    }
    let set_val_rust = cx
        .param_types
        .first()
        .cloned()
        .unwrap_or_else(|| "String".to_owned());
    // A FALLIBLE setter carries a narrowing integer field: convert with
    // `try_from` and fold an out-of-range value to `Err` (the checked
    // variant), never a silent truncation.
    if f.effect() == Effect::Fallible {
        return checked_field_set_lines(cx, &recv_rust, field, &set_val_rust);
    }
    let expr = owned_value_coercion(cx.raw_param(0), &set_val_rust, "arg0");
    vec![
        format!("// [field-set] {}", cx.label),
        format!(
            "pub fn {}(arg0: {set_val_rust}, arg1: {recv_rust}) -> {recv_rust} {{",
            cx.rust_name
        ),
        "    let mut r = arg1;".to_owned(),
        format!("    r.{} = {expr};", rust_safe_ident(field)),
        "    r".to_owned(),
        "}".to_owned(),
    ]
}

/// CHECKED setter body for a narrowing integer field (bare or `Option<>`):
/// `try_from` the Ipe `i64`, assign on `Ok`, fold out-of-range to a typed
/// `Err`. Any other raw shape fails closed (empty region — the binding drops).
fn checked_field_set_lines(
    cx: &WrapperCx<'_>,
    recv_rust: &str,
    field: &str,
    set_val_rust: &str,
) -> Vec<String> {
    let raw = cx.raw_param(0).trim();
    let is_checked_int = |t: &str| {
        matches!(
            t,
            "i8" | "i16"
                | "i32"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "i128"
                | "usize"
                | "isize"
        )
    };
    let sig = format!(
        "pub fn {}(arg0: {set_val_rust}, arg1: {recv_rust}) -> IpeResult<IpeError, {recv_rust}> {{",
        cx.rust_name
    );
    let assign = |value_expr: &str| {
        vec![
            format!(
                "        Ok(v) => {{ let mut r = arg1; r.{} = {value_expr}; ok_res(r) }}",
                rust_safe_ident(field)
            ),
            "        Err(e) => IpeResult::Err(ipe_error_from_foreign(e)),".to_owned(),
        ]
    };
    if let Some(inner) = strip_generic1("Option", raw).map(str::trim)
        && is_checked_int(inner)
    {
        let mut out = vec![
            format!("// [field-set checked] {}", cx.label),
            sig,
            format!("    match ipe_maybe_to_option(arg0).map({inner}::try_from).transpose() {{"),
        ];
        out.extend(assign("v"));
        out.push("    }".to_owned());
        out.push("}".to_owned());
        return out;
    }
    if is_checked_int(raw) {
        let mut out = vec![
            format!("// [field-set checked] {}", cx.label),
            sig,
            format!("    match {raw}::try_from(arg0) {{"),
        ];
        out.extend(assign("v"));
        out.push("    }".to_owned());
        out.push("}".to_owned());
        return out;
    }
    Vec::new()
}

/// Owned inbound coercion: lift an Ipê-resolved wrapper value into the exact
/// raw foreign type, OWNED (never borrows — field assignments and enum ctors
/// move owned values in). Int narrowing saturates; a Float-source list keeps
/// the already-saturating bare `as`.
fn owned_value_coercion(raw_ty: &str, decl_ty: &str, base: &str) -> String {
    if let Some(sk) = seq_kind(raw_ty) {
        return match (sk.shape, &sk.elem) {
            (SeqShape::Owned, SeqElem::U8) => format!("to_u8_vec(&{base})"),
            (SeqShape::Owned, SeqElem::General { rust, ipe }) => {
                if rust == "i64" || rust == "f64" || !is_numeric_rust(rust) {
                    base.to_owned()
                } else if ipe == "Float" {
                    format!("{base}.into_iter().map(|x| x as {rust}).collect()")
                } else {
                    format!(
                        "{base}.into_iter().map(|x| {}).collect()",
                        num_saturate(rust, "x")
                    )
                }
            }
            // Slice/array field shapes are outside the closed eligible set.
            _ => base.to_owned(),
        };
    }
    if let Some(inner) = strip_generic1("Option", raw_ty) {
        let opt = format!("ipe_maybe_to_option({base})");
        return if is_numeric_rust(inner) {
            format!("{opt}.map(|x| {})", num_saturate(inner, "x"))
        } else {
            opt
        };
    }
    if raw_ty.is_empty() || raw_ty == decl_ty {
        return base.to_owned();
    }
    if is_numeric_rust(raw_ty) && (decl_ty == "i64" || decl_ty == "f64") {
        return num_saturate(raw_ty, base);
    }
    base.to_owned()
}

/// Enum-variant constructor: `E::Variant(args)` — infallible (the inspector
/// suppressed ctors for `non_exhaustive` enums/variants).
fn enum_ctor_lines(
    cx: &WrapperCx<'_>,
    variant: &str,
    kind: EnumVariantKind,
    struct_fields: &[RustIdent],
) -> Vec<String> {
    let recv_rust = cx.resolve_recv();
    if recv_rust.trim().is_empty() || variant.is_empty() {
        return Vec::new();
    }
    let path = format!("{recv_rust}::{}", rust_safe_ident(variant));
    let ctor_args: Vec<String> = (0..cx.n_params())
        .map(|j| owned_value_coercion(cx.raw_param(j), cx.decl_param(j), &arg_name(j)))
        .collect();
    let expr = match kind {
        EnumVariantKind::Unit => path,
        EnumVariantKind::Struct => {
            let assigns: Vec<String> = struct_fields
                .iter()
                .zip(&ctor_args)
                .map(|(field, a)| format!("{}: {a}", rust_safe_ident(field.as_str())))
                .collect();
            format!("{path} {{ {} }}", assigns.join(", "))
        }
        EnumVariantKind::Tuple => format!("{path}({})", ctor_args.join(", ")),
    };
    vec![
        format!("// [enum-ctor] {}", cx.label),
        format!(
            "pub fn {}({}) -> {recv_rust} {{",
            cx.rust_name,
            cx.param_decl()
        ),
        format!("    {expr}"),
        "}".to_owned(),
    ]
}

/// Enum tag accessor: an exhaustive `match` mapping each variant to its name
/// string; the wildcard arm appears only when the inspector flagged it.
fn enum_tag_lines(cx: &WrapperCx<'_>, arms: &[EnumArm], wildcard: bool) -> Vec<String> {
    let recv_rust = cx.resolve_recv();
    if recv_rust.trim().is_empty() || arms.is_empty() {
        return Vec::new();
    }
    let mut out = vec![
        format!("// [enum-tag] {}", cx.label),
        format!("pub fn {}(arg0: {recv_rust}) -> String {{", cx.rust_name),
        "    let t: &str = match arg0 {".to_owned(),
    ];
    for arm in arms {
        // The pattern is `<variant><suffix>`, suffix ∈ {"", "(..)", "{..}"};
        // raw-escape just the leading variant ident.
        let pat = arm.pattern.as_str();
        let split_at = pat.find(['(', '{']).unwrap_or(pat.len());
        let vid = pat.get(..split_at).unwrap_or("");
        let suffix = pat.get(split_at..).unwrap_or("");
        out.push(format!(
            "        {recv_rust}::{}{suffix} => {},",
            rust_safe_ident(vid),
            rust_str_lit(&arm.tag)
        ));
    }
    if wildcard {
        out.push(format!("        _ => {},", rust_str_lit("<unknown>")));
    }
    out.push("    };".to_owned());
    out.push("    t.to_string()".to_owned());
    out.push("}".to_owned());
    out
}

/// Single-field payload extractor: `E -> IpeMaybe<T>`. The by-value receiver
/// moves the selected owned field out; sibling positions bind `_` and drop.
fn enum_extract_lines(
    cx: &WrapperCx<'_>,
    variant: &str,
    kind: EnumVariantKind,
    selector: &str,
    field_count: u64,
    wildcard: bool,
) -> Vec<String> {
    let f = cx.f;
    let recv_rust = cx.resolve_recv();
    let raw_result = f.results().first().map_or("", Param::rust_type_str);
    let inner_raw = strip_generic1("Option", raw_result)
        .unwrap_or(raw_result)
        .trim()
        .to_owned();
    let well_formed = !recv_rust.trim().is_empty()
        && !variant.is_empty()
        && !inner_raw.is_empty()
        && (kind != EnumVariantKind::Struct || !selector.is_empty());
    if !well_formed {
        return Vec::new();
    }
    let (inner0, co) = translate_rust_ret(&inner_raw);
    let inner = absolutize_crate(cx.krate, &inner0);
    let path = format!("{recv_rust}::{}", rust_safe_ident(variant));
    let (pattern, binder) = match kind {
        EnumVariantKind::Struct => {
            let ident = rust_safe_ident(selector);
            (format!("{path} {{ {ident}, .. }}"), ident)
        }
        // Tuple (and the defensive unit fallback): bind only the selected
        // position; every other position is `_` so unmatched siblings drop
        // without an unused_variables warning.
        EnumVariantKind::Tuple | EnumVariantKind::Unit => {
            if selector.is_empty() {
                (format!("{path}(x)"), "x".to_owned())
            } else {
                let n = usize::try_from(field_count).unwrap_or(1).max(1);
                let idx = selector
                    .parse::<usize>()
                    .ok()
                    .filter(|&i| i < n)
                    .unwrap_or(0);
                let ret = format!("f{idx}");
                let binders: Vec<String> = (0..n)
                    .map(|i| {
                        if i == idx {
                            ret.clone()
                        } else {
                            "_".to_owned()
                        }
                    })
                    .collect();
                (format!("{path}({})", binders.join(", ")), ret)
            }
        }
    };
    let mut out = vec![
        format!("// [enum-extract] {}", cx.label),
        format!(
            "pub fn {}(arg0: {recv_rust}) -> IpeMaybe<{inner}> {{",
            cx.rust_name
        ),
        "    match arg0 {".to_owned(),
        format!("        {pattern} => IpeMaybe::Just({}),", co(&binder)),
    ];
    if wildcard {
        out.push("        _ => IpeMaybe::Nothing,".to_owned());
    }
    out.push("    }".to_owned());
    out.push("}".to_owned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(v: &serde_json::Value) -> PkgInfo {
        PkgInfo::decode_json(&v.to_string()).expect("decodes")
    }

    fn semver_pkg(functions: &serde_json::Value) -> PkgInfo {
        decode(&json!({
            "pkg": "semver",
            "name": "semver",
            "version": "1.0.26",
            "functions": functions,
            "errors": []
        }))
    }

    // ── helper units ────────────────────────────────────────────────────

    #[test]
    fn absolutize_rewrites_only_path_starts() {
        assert_eq!(absolutize_crate("csv", "csv::Reader"), "::csv::Reader");
        assert_eq!(
            absolutize_crate("csv", "Vec<csv::Reader>"),
            "Vec<::csv::Reader>"
        );
        assert_eq!(absolutize_crate("csv", "::csv::Reader"), "::csv::Reader");
        assert_eq!(absolutize_crate("csv", "mycsv::Reader"), "mycsv::Reader");
        assert_eq!(absolutize_crate("csv", "String"), "String");
    }

    #[test]
    fn ok_type_of_result_respects_nested_angles() {
        assert_eq!(ok_type_of_result("Result<Version, Error>"), "Version");
        assert_eq!(ok_type_of_result("Result<Vec<T>, E>"), "Vec<T>");
        assert_eq!(ok_type_of_result("String"), "String");
    }

    #[test]
    fn pkg_to_crate_import_takes_the_last_segment_underscored() {
        assert_eq!(pkg_to_crate_import("uuid"), "uuid");
        assert_eq!(pkg_to_crate_import("some/crate-name"), "crate_name");
    }

    #[test]
    fn seq_kind_classifies_shapes_and_rejects_mut_and_huge_arrays() {
        let u8_slice = seq_kind("&[u8]").expect("slice");
        assert_eq!(u8_slice.shape, SeqShape::Slice);
        assert_eq!(u8_slice.elem, SeqElem::U8);
        assert_eq!(seq_kind("[u8; 16]").expect("arr").shape, SeqShape::Arr(16));
        assert_eq!(
            seq_kind("&[u8; 4]").expect("ref arr").shape,
            SeqShape::RefArr(4)
        );
        let general_vec = seq_kind("Vec<f32>").expect("general vec");
        assert_eq!(
            general_vec.elem,
            SeqElem::General {
                rust: "f32".into(),
                ipe: "Float".into()
            }
        );
        assert_eq!(seq_kind("&mut [u8]"), None);
        assert_eq!(seq_kind("[u8; 999999999999]"), None);
        assert_eq!(seq_kind("Vec<str>"), None);
        assert_eq!(seq_kind("Vec<PathBuf>"), None);
    }

    #[test]
    fn translate_ret_covers_the_coercion_table() {
        let (t, co) = translate_rust_ret("u64");
        assert_eq!(t, "i64");
        assert_eq!(co("v"), "(v).min(i64::MAX as u64) as i64");
        let (t, co) = translate_rust_ret("Option<u32>");
        assert_eq!(t, "IpeMaybe<i64>");
        assert_eq!(
            co("e"),
            "match e { Some(v) => IpeMaybe::Just((v) as i64), None => IpeMaybe::Nothing }"
        );
        let (t, co) = translate_rust_ret("&str");
        assert_eq!(t, "String");
        assert_eq!(co("e"), "e.to_string()");
        let (t, co) = translate_rust_ret("Version");
        assert_eq!(t, "Version");
        assert_eq!(co("e"), "e");
        let (t, co) = translate_rust_ret("Vec<u8>");
        assert_eq!(t, "Vec<i64>");
        assert_eq!(co("e"), "from_u8_slice(&e)");
        let (t, co) = translate_rust_ret("Vec<u64>");
        assert_eq!(t, "Vec<i64>");
        assert_eq!(
            co("e"),
            "e.into_iter().map(|x| (x).min(i64::MAX as u64) as i64).collect::<Vec<_>>()"
        );
        let (t, co) = translate_rust_ret("Vec<String>");
        assert_eq!(t, "Vec<String>");
        assert_eq!(co("e"), "e");
        let (t, co) = translate_rust_ret("&[f32]");
        assert_eq!(t, "Vec<f64>");
        assert_eq!(co("e"), "e.iter().map(|&x| (x) as f64).collect::<Vec<_>>()");
        let (t, co) = translate_rust_ret("serde_json::Value");
        assert_eq!(t, "String");
        assert_eq!(co("e"), "serde_json::to_string(&(e)).unwrap_or_default()");
        let (t, co) = translate_rust_ret("&Version");
        assert_eq!(t, "Version");
        assert_eq!(co("e"), "e.to_owned()");
        let (t, co) = translate_rust_ret("");
        assert_eq!(t, "()");
        assert_eq!(co("e"), "e");
    }

    #[test]
    fn panic_profile_precheck_reads_any_quote_style() {
        assert!(cargo_profile_panic_is_unwind(
            "[profile.release]\nlto = true\n"
        ));
        assert!(cargo_profile_panic_is_unwind("panic = \"unwind\"\n"));
        assert!(!cargo_profile_panic_is_unwind("panic = \"abort\"\n"));
        assert!(!cargo_profile_panic_is_unwind("  panic='abort'\n"));
        assert!(!cargo_profile_panic_is_unwind("panic\t= 'abort'\n"));
    }

    // ── whole-file golden ───────────────────────────────────────────────

    #[test]
    fn golden_semver_bindings_file() {
        let pkg = semver_pkg(&json!([
            {
                "name": "parse",
                "params": [{"name": "text", "type": "String", "ipeType": "String", "rustType": "&str"}],
                "results": [{"name": "", "type": "Result Error Version", "rustType": "Result<Version, Error>"}],
                "effect": "fallible"
            },
            {
                "name": "major_field",
                "params": [{"name": "self", "type": "Version", "ipeType": "Version", "rustType": "&Version"}],
                "results": [{"name": "", "type": "Int", "rustType": "u64"}],
                "effect": "pure",
                "recvType": "Version",
                "recvRustType": "semver::Version",
                "methodName": "major",
                "isField": true
            }
        ]));
        let expected = r#"// Code generated by ipe-ffi-inspector from semver. DO NOT EDIT.
// Re-run `ipe add semver` to regenerate.

#![allow(unused_imports, unused_mut, dead_code)]

// The catch_unwind boundary converts a foreign panic into a typed Err;
// that conversion is sound only under panic=unwind, so refuse to build
// under any configuration that selects panic=abort.
#[cfg(panic = "abort")]
compile_error!("ipe_ffi catch_unwind boundary requires panic=unwind");

use crate::*;
use std::collections::HashMap;

// IPE-FFI-WRAPPER BEGIN parse
// [fallible] Rust_Semver_parse
pub fn semver_parse(arg0: String) -> IpeResult<IpeError, Version> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || ::semver::parse(arg0.as_ref()))) { Ok(Ok(v)) => ok_res(v), Ok(Err(e)) => IpeResult::Err(ipe_error_from_foreign(e)), Err(_) => IpeResult::Err(str_err("foreign call panicked")) }
}
// IPE-FFI-WRAPPER END
// IPE-FFI-WRAPPER BEGIN major_field_from_version
// [field] Rust_Semver_major_field_from_version
pub fn semver_major_field_from_version(arg0: ::semver::Version) -> i64 {
    (arg0.major).min(i64::MAX as u64) as i64
}
// IPE-FFI-WRAPPER END

"#;
        assert_eq!(emit_bindings(&pkg), expected);
    }

    // ── wrapper-shape coverage ──────────────────────────────────────────

    fn closure_region(sig: &str) -> String {
        let pkg = semver_pkg(&json!([{
            "name": "update_fn",
            "effect": "pure",
            "isClosureAdapter": true,
            "closureSig": sig
        }]));
        emit_bindings(&pkg)
    }

    #[test]
    fn closure_adapter_total_return_aborts_on_panic_never_fabricates() {
        let out = closure_region("Fn(Int, Bool) -> Int + Send + Sync + 'static");
        // The wrapper receives the Ipê fn value as the exact app-side box type
        // and returns the crate closure of the exact declared signature.
        assert!(
            out.contains(
                "pub fn semver_update_fn(__ipe_fn: Box<dyn Fn(i64, bool) -> i64 + Send + Sync + \
                 'static>) -> Box<dyn Fn(i64, bool) -> i64 + Send + Sync + 'static> {"
            ),
            "{out}"
        );
        // Multi-call Clone: the captured value is behind an Arc.
        assert!(out.contains("std::sync::Arc::new(__ipe_fn)"), "{out}");
        // Panic-isolated per-call re-entry.
        assert!(out.contains("std::panic::catch_unwind"), "{out}");
        // A total return has no error channel: abort, NEVER fabricate a Default.
        assert!(out.contains("std::process::abort()"), "{out}");
        assert!(!out.contains("Default::default"), "{out}");
        // Sentinel bracketing so DCE can drop an unreached adapter by name.
        assert!(out.contains("// IPE-FFI-WRAPPER BEGIN update_fn"), "{out}");
    }

    #[test]
    fn closure_adapter_result_return_folds_the_panic_in_band() {
        let out = closure_region("Fn(Int) -> Result<Int, Error> + Send + Sync + 'static");
        assert!(
            out.contains("-> Box<dyn Fn(i64) -> Result<i64, IpeError> + Send + Sync + 'static> {"),
            "{out}"
        );
        // A fallible return folds a panic to Err — never aborts.
        assert!(
            out.contains("Err(_) => Err(str_err(\"foreign closure panicked\"))"),
            "{out}"
        );
        assert!(!out.contains("std::process::abort()"), "{out}");
    }

    #[test]
    fn closure_adapter_option_return_folds_the_panic_to_none() {
        let out = closure_region("Fn(Int) -> Option<Int> + Send + Sync + 'static");
        assert!(
            out.contains("-> Box<dyn Fn(i64) -> Option<i64> + Send + Sync + 'static> {"),
            "{out}"
        );
        assert!(out.contains("Err(_) => None"), "{out}");
        assert!(!out.contains("std::process::abort()"), "{out}");
    }

    #[test]
    fn struct_ctor_emits_the_derived_definition_and_a_constructor() {
        let out = emit_bindings(&semver_pkg(&json!([{
            "name": "counter_new",
            "effect": "pure",
            "isStructCtor": true,
            "structName": "Counter",
            "structFields": [{ "name": "value", "type": "i64" }],
            "structDerives": ["Default", "Clone"]
        }])));
        // The `#[derive]`ed definition (canonical derive order).
        assert!(out.contains("#[derive(Clone, Default)]"), "{out}");
        assert!(out.contains("pub struct Counter {"), "{out}");
        assert!(out.contains("    pub value: i64,"), "{out}");
        // The constructor wrapper folds each owned-carrier arg into the literal.
        assert!(
            out.contains("pub fn semver_counter_new(arg0: i64) -> Counter {"),
            "{out}"
        );
        assert!(out.contains("Counter { value: arg0 }"), "{out}");
        // Sentinel bracketing so DCE can drop an unreached ctor by name.
        assert!(
            out.contains("// IPE-FFI-WRAPPER BEGIN counter_new"),
            "{out}"
        );
    }

    #[test]
    fn struct_ctor_with_an_opaque_field_over_drops_no_wrapper() {
        // An opaque field needs the crate opaque-map to resolve to a nameable
        // path; that plumbing is a follow-up, so the entry over-drops at decode
        // rather than emit an unresolvable bare handle and break the SEAL.
        let out = emit_bindings(&semver_pkg(&json!([{
            "name": "wrap_new",
            "effect": "pure",
            "isStructCtor": true,
            "structName": "Wrap",
            "structFields": [{ "name": "inner", "type": "Version" }],
            "structDerives": []
        }])));
        assert!(!out.contains("pub struct Wrap"), "{out}");
        assert!(!out.contains("semver_wrap_new"), "{out}");
    }

    #[test]
    fn struct_ctor_derive_free_scalar_struct_emits() {
        // The derive-free case: no `#[derive]` line, a scalar field, a ctor.
        let out = emit_bindings(&semver_pkg(&json!([{
            "name": "point_new",
            "effect": "pure",
            "isStructCtor": true,
            "structName": "Point",
            "structFields": [
                { "name": "x", "type": "i64" },
                { "name": "y", "type": "i64" }
            ],
            "structDerives": []
        }])));
        assert!(!out.contains("#[derive"), "{out}");
        assert!(out.contains("pub struct Point {"), "{out}");
        assert!(out.contains("    pub x: i64,"), "{out}");
        assert!(out.contains("    pub y: i64,"), "{out}");
        assert!(
            out.contains("pub fn semver_point_new(arg0: i64, arg1: i64) -> Point {"),
            "{out}"
        );
        assert!(out.contains("Point { x: arg0, y: arg1 }"), "{out}");
    }

    #[test]
    fn enum_def_emits_the_derived_definition_and_per_variant_constructors() {
        // The Iced/TEA `Message` shape: unit variants + a tuple-payload one.
        let out = emit_bindings(&semver_pkg(&json!([{
            "name": "message",
            "effect": "pure",
            "isEnumDef": true,
            "enumName": "Message",
            "enumVariants": [
                { "name": "Increment", "payload": [] },
                { "name": "Decrement", "payload": [] },
                { "name": "SetValue", "payload": ["i64"] }
            ],
            "enumDerives": ["Clone"]
        }])));
        assert!(out.contains("#[derive(Clone)]"), "{out}");
        assert!(out.contains("pub enum Message {"), "{out}");
        assert!(out.contains("    Increment,"), "{out}");
        assert!(out.contains("    SetValue(i64),"), "{out}");
        // One constructor per variant, `<ctor>_<snake(variant)>`.
        assert!(
            out.contains("pub fn semver_message_increment() -> Message {"),
            "{out}"
        );
        assert!(out.contains("Message::Increment"), "{out}");
        assert!(
            out.contains("pub fn semver_message_set_value(arg0: i64) -> Message {"),
            "{out}"
        );
        assert!(out.contains("Message::SetValue(arg0)"), "{out}");
        // Sentinel bracketing so DCE can drop the region by name.
        assert!(out.contains("// IPE-FFI-WRAPPER BEGIN message"), "{out}");
    }

    #[test]
    fn enum_def_with_an_opaque_payload_over_drops_no_wrapper() {
        // An opaque payload needs the crate opaque-map to resolve to a nameable
        // path; that plumbing is a follow-up, so the entry over-drops at decode
        // rather than emit an unresolvable bare handle and break the SEAL.
        let out = emit_bindings(&semver_pkg(&json!([{
            "name": "wrap",
            "effect": "pure",
            "isEnumDef": true,
            "enumName": "Wrap",
            "enumVariants": [{ "name": "Hold", "payload": ["Version"] }],
            "enumDerives": []
        }])));
        assert!(!out.contains("pub enum Wrap"), "{out}");
        assert!(!out.contains("semver_wrap_hold"), "{out}");
    }

    #[test]
    fn pure_display_bridge_takes_impl_display_and_catches_unwind() {
        let pkg = semver_pkg(&json!([{
            "name": "to_string",
            "params": [{"name": "self", "type": "Version", "ipeType": "Version", "rustType": "&Version"}],
            "results": [{"name": "", "type": "String", "rustType": "String"}],
            "effect": "pure",
            "recvType": "Version",
            "recvRustType": "semver::Version",
            "methodName": "to_string"
        }]));
        let out = emit_bindings(&pkg);
        assert!(
            out.contains(
                "pub fn semver_to_string_from_version(arg0: impl std::fmt::Display) -> IpeResult<IpeError, String> {"
            ),
            "{out}"
        );
        assert!(
            out.contains(
                "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || arg0.to_string())) { Ok(v) => ok_res(v), Err(_) => IpeResult::Err(str_err(\"foreign call panicked\")) }"
            ),
            "{out}"
        );
    }

    #[test]
    fn effectful_method_owns_the_borrowed_opaque_receiver_param_and_spawns() {
        let pkg = decode(&json!({
            "pkg": "db",
            "name": "db",
            "functions": [{
                "name": "get",
                "params": [
                    {"name": "self", "type": "Db", "ipeType": "Db", "rustType": "&Db"},
                    {"name": "key", "type": "String", "ipeType": "String", "rustType": "&str"}
                ],
                "results": [{"name": "", "type": "Result Error String", "rustType": "Result<String, Error>"}],
                "effect": "effectful",
                "recvType": "Db",
                "recvRustType": "db::Db",
                "methodName": "get"
            }],
            "errors": []
        }));
        let out = emit_bindings(&pkg);
        // The `&Db` foreign param is declared OWNED (async 'static escape).
        assert!(
            out.contains("pub fn db_get_from_db(mut arg0: Db, arg1: String) -> IpeTask<String> {"),
            "{out}"
        );
        // Fallible async: abort-on-drop guard, three arms, typed foreign-error
        // fold, JoinError through the same redaction funnel.
        assert!(
            out.contains(
                "Box::pin(async move { let handle = tokio::task::spawn(async move { arg0.get(arg1.as_ref()).await }); let guard = AbortOnDrop::new(handle.abort_handle()); let joined = handle.await; guard.defuse(); match joined { Ok(Ok(v)) => ok_res(v), Ok(Err(e)) => IpeResult::Err(ipe_error_from_foreign(e)), Err(join_err) => IpeResult::Err(ipe_error_from_foreign(join_err)) } })"
            ),
            "{out}"
        );
    }

    #[test]
    fn infallible_async_spawns_with_two_arms() {
        let pkg = decode(&json!({
            "pkg": "svc",
            "name": "svc",
            "functions": [{
                "name": "ping",
                "params": [],
                "results": [{"name": "", "type": "String", "rustType": "String"}],
                "effect": "effectful"
            }],
            "errors": []
        }));
        let out = emit_bindings(&pkg);
        assert!(
            out.contains(
                "Box::pin(async move { let handle = tokio::task::spawn(async move { ::svc::ping().await }); let guard = AbortOnDrop::new(handle.abort_handle()); let joined = handle.await; guard.defuse(); match joined { Ok(v) => ok_res(v), Err(join_err) => IpeResult::Err(ipe_error_from_foreign(join_err)) } })"
            ),
            "{out}"
        );
        assert!(
            out.contains("pub fn svc_ping(_: ()) -> IpeTask<String> {"),
            "{out}"
        );
    }

    #[test]
    fn self_returning_setter_threads_the_owned_receiver() {
        let pkg = decode(&json!({
            "pkg": "chrono",
            "name": "chrono",
            "functions": [{
                "name": "insert",
                "params": [
                    {"name": "self", "type": "WeekdaySet", "ipeType": "WeekdaySet", "rustType": "WeekdaySet"},
                    {"name": "day", "type": "Weekday", "ipeType": "Weekday", "rustType": "Weekday"}
                ],
                "results": [{"name": "", "type": "WeekdaySet", "rustType": "()"}],
                "effect": "pure",
                "recvType": "WeekdaySet",
                "recvRustType": "chrono::WeekdaySet",
                "methodName": "insert",
                "selfReturning": true
            }],
            "errors": []
        }));
        let out = emit_bindings(&pkg);
        assert!(
            out.contains(
                "match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || { arg0.insert(arg1); arg0 })) { Ok(r) => ok_res(r), Err(_) => IpeResult::Err(str_err(\"foreign call panicked\")) }"
            ),
            "{out}"
        );
    }

    #[test]
    fn borrow_reader_threads_the_receiver_back_beside_the_result() {
        // A `&self` reader (receiver `rustType` begins with `&`) hands `arg0`
        // back beside the coerced result: the return type is `(i64, Widget)`
        // and the body pairs the value with the receiver.
        let pkg = decode(&json!({
            "pkg": "handle_demo",
            "name": "handle_demo",
            "functions": [{
                "name": "slot_count",
                "params": [
                    {"name": "self", "type": "Widget", "ipeType": "Widget", "rustType": "&handle_demo::Widget"}
                ],
                "results": [{"name": "", "type": "Int", "rustType": "usize"}],
                "effect": "pure",
                "recvType": "Widget",
                "recvRustType": "handle_demo::Widget",
                "methodName": "slot_count"
            }],
            "errors": []
        }));
        let out = emit_bindings(&pkg);
        assert!(
            out.contains(
                "pub fn handle_demo_slot_count_from_widget(mut arg0: ::handle_demo::Widget) -> IpeResult<IpeError, (i64, ::handle_demo::Widget)> {"
            ),
            "{out}"
        );
        assert!(
            out.contains("let __r = arg0.slot_count(); (__r, arg0)"),
            "{out}"
        );
        assert!(out.contains("Ok((v, recv)) => ok_res(("), "{out}");
    }

    #[test]
    fn tuple_return_coerces_every_component_to_its_carrier() {
        // `(u64, u32)` declares `(i64, i64)` and destructures the raw tuple,
        // widening each component: the wide unsigned saturates, the narrow one
        // is a plain `as i64`.
        let (decl, co) = translate_rust_ret("(u64, u32)");
        assert_eq!(decl, "(i64, i64)");
        assert_eq!(
            co("r"),
            "{ let (t0, t1) = r; ((t0).min(i64::MAX as u64) as i64, (t1) as i64) }"
        );
    }

    #[test]
    fn tuple_predicate_rejects_a_string_component() {
        // A String component is NOT wired → not coercible (stays dropped).
        let mixed = decode(&json!({
            "pkg": "grid", "name": "grid",
            "functions": [{
                "name": "labelled",
                "params": [],
                "results": [{"name": "", "type": "(Int, String)", "rustType": "(u32, String)"}],
                "effect": "pure",
                "methodName": ""
            }],
            "errors": []
        }));
        let mf = mixed.fns().first().expect("one fn");
        assert!(
            !multi_result_tuple_is_coercible(mf),
            "String component must not admit"
        );
    }

    #[test]
    fn plain_multi_result_tuple_binds_with_per_component_coercion() {
        // A plain (non-reader, non-setter) free fn returning `(u64, u32)` now
        // binds: the wrapper declares `(i64, i64)` and coerces each slot.
        let pkg = decode(&json!({
            "pkg": "geo", "name": "geo",
            "functions": [{
                "name": "bounds",
                "params": [],
                "results": [{"name": "", "type": "(Int, Int)", "rustType": "(u64, u32)"}],
                "effect": "pure",
                "methodName": ""
            }],
            "errors": []
        }));
        let f = pkg.fns().first().expect("one fn");
        assert!(
            multi_result_tuple_is_coercible(f),
            "all-numeric tuple must admit"
        );
        let out = emit_bindings(&pkg);
        assert!(
            out.contains("-> IpeResult<IpeError, (i64, i64)> {"),
            "{out}"
        );
        assert!(
            out.contains(
                "{ let (t0, t1) = ::geo::bounds(); ((t0).min(i64::MAX as u64) as i64, (t1) as i64) }"
            ),
            "{out}"
        );
    }

    #[test]
    fn field_setter_replaces_the_field_with_a_saturating_write() {
        let pkg = semver_pkg(&json!([{
            "name": "patch_set_field",
            "params": [
                {"name": "value", "type": "Int", "ipeType": "Int", "rustType": "u32"},
                {"name": "self", "type": "Version", "ipeType": "Version", "rustType": "semver::Version"}
            ],
            "results": [{"name": "", "type": "Version", "rustType": "semver::Version"}],
            "effect": "pure",
            "recvType": "Version",
            "recvRustType": "semver::Version",
            "methodName": "patch",
            "isFieldSet": true
        }]));
        let out = emit_bindings(&pkg);
        assert!(
            out.contains(
                "pub fn semver_patch_set_field_from_version(arg0: i64, arg1: ::semver::Version) -> ::semver::Version {"
            ),
            "{out}"
        );
        assert!(
            out.contains("    r.patch = (arg0).clamp(0, u32::MAX as i64) as u32;"),
            "{out}"
        );
        assert!(out.contains("    let mut r = arg1;"), "{out}");
    }

    #[test]
    fn fallible_field_setter_renders_checked_conversion() {
        // A narrowing integer field's setter is FALLIBLE: `try_from` + typed
        // Err on out-of-range, never a silent truncation. Both the bare and
        // the `Option<>`-wrapped shapes render checked bodies.
        let pkg = semver_pkg(&json!([
            {
                "name": "patch_set_field",
                "params": [
                    {"name": "value", "type": "Int", "ipeType": "Int", "rustType": "u32"},
                    {"name": "self", "type": "Version", "ipeType": "Version", "rustType": "semver::Version"}
                ],
                "results": [{"name": "", "type": "Version", "rustType": "semver::Version"}],
                "effect": "fallible",
                "recvType": "Version",
                "recvRustType": "semver::Version",
                "methodName": "patch",
                "isFieldSet": true
            },
            {
                "name": "build_set_field",
                "params": [
                    {"name": "value", "type": "Maybe Int", "ipeType": "Maybe Int", "rustType": "Option<u64>"},
                    {"name": "self", "type": "Version", "ipeType": "Version", "rustType": "semver::Version"}
                ],
                "results": [{"name": "", "type": "Version", "rustType": "semver::Version"}],
                "effect": "fallible",
                "recvType": "Version",
                "recvRustType": "semver::Version",
                "methodName": "build",
                "isFieldSet": true
            }
        ]));
        let out = emit_bindings(&pkg);
        assert!(
            out.contains(
                "pub fn semver_patch_set_field_from_version(arg0: i64, arg1: ::semver::Version) -> IpeResult<IpeError, ::semver::Version> {"
            ),
            "{out}"
        );
        assert!(out.contains("match u32::try_from(arg0) {"), "{out}");
        assert!(
            out.contains("Ok(v) => { let mut r = arg1; r.patch = v; ok_res(r) }"),
            "{out}"
        );
        assert!(
            out.contains("Err(e) => IpeResult::Err(ipe_error_from_foreign(e)),"),
            "{out}"
        );
        assert!(
            out.contains("match ipe_maybe_to_option(arg0).map(u64::try_from).transpose() {"),
            "{out}"
        );
    }

    #[test]
    fn enum_accessor_trio_renders_total_bodies() {
        let pkg = semver_pkg(&json!([
            {
                "name": "greater_new_variant",
                "params": [{"name": "f0", "type": "Int", "ipeType": "Int", "rustType": "u64"}],
                "results": [{"name": "", "type": "Op", "rustType": "semver::Op"}],
                "effect": "pure",
                "recvType": "Op",
                "recvRustType": "semver::Op",
                "isEnumCtor": true,
                "enumVariant": "Greater",
                "enumKind": "tuple"
            },
            {
                "name": "tag_of_op",
                "params": [{"name": "e", "type": "Op", "ipeType": "Op", "rustType": "semver::Op"}],
                "results": [{"name": "", "type": "String", "rustType": "String"}],
                "effect": "pure",
                "recvRustType": "semver::Op",
                "methodName": "tag_of_op",
                "isEnumTag": true,
                "enumArms": ["Exact\tExact", "Greater(..)\tGreater", "match\tmatch"],
                "enumWildcard": true
            },
            {
                "name": "value_as_greater",
                "params": [{"name": "e", "type": "Op", "ipeType": "Op", "rustType": "semver::Op"}],
                "results": [{"name": "", "type": "Maybe Int", "rustType": "Option<u64>"}],
                "effect": "pure",
                "recvType": "Op",
                "recvRustType": "semver::Op",
                "isEnumExtract": true,
                "enumVariant": "Greater",
                "enumKind": "tuple",
                "enumStructFields": ["1"],
                "enumFieldCount": 2,
                "enumWildcard": true
            }
        ]));
        let out = emit_bindings(&pkg);
        // Ctor: saturating owned arg, absolute enum path.
        assert!(
            out.contains("pub fn semver_greater_new_variant_from_op(arg0: i64) -> ::semver::Op {"),
            "{out}"
        );
        assert!(
            out.contains("    ::semver::Op::Greater((arg0).max(0) as u64)"),
            "{out}"
        );
        // Tag: one &str match with a keyword-safe arm and a gated wildcard.
        assert!(out.contains("    let t: &str = match arg0 {"), "{out}");
        assert!(
            out.contains("        ::semver::Op::Exact => \"Exact\","),
            "{out}"
        );
        assert!(
            out.contains("        ::semver::Op::Greater(..) => \"Greater\","),
            "{out}"
        );
        assert!(
            out.contains("        ::semver::Op::r#match => \"match\","),
            "{out}"
        );
        assert!(out.contains("        _ => \"<unknown>\","), "{out}");
        assert!(out.contains("    t.to_string()"), "{out}");
        // Extract: binds only the selected tuple position, widens the payload.
        assert!(
            out.contains(
                "        ::semver::Op::Greater(_, f1) => IpeMaybe::Just((f1).min(i64::MAX as u64) as i64),"
            ),
            "{out}"
        );
        assert!(out.contains("        _ => IpeMaybe::Nothing,"), "{out}");
        assert!(
            out.contains(
                "pub fn semver_value_as_greater_from_op(arg0: ::semver::Op) -> IpeMaybe<i64> {"
            ),
            "{out}"
        );
    }

    #[test]
    fn struct_variant_ctor_and_extract_use_named_fields() {
        let pkg = semver_pkg(&json!([
            {
                "name": "point_new_variant",
                "params": [
                    {"name": "x", "type": "Int", "ipeType": "Int", "rustType": "i64"},
                    {"name": "type", "type": "String", "ipeType": "String", "rustType": "String"}
                ],
                "results": [{"name": "", "type": "Shape", "rustType": "semver::Shape"}],
                "effect": "pure",
                "recvType": "Shape",
                "recvRustType": "semver::Shape",
                "isEnumCtor": true,
                "enumVariant": "Point",
                "enumKind": "struct",
                "enumStructFields": ["x", "type"]
            },
            {
                "name": "x_as_point",
                "params": [{"name": "e", "type": "Shape", "ipeType": "Shape", "rustType": "semver::Shape"}],
                "results": [{"name": "", "type": "Maybe Int", "rustType": "Option<i64>"}],
                "effect": "pure",
                "recvType": "Shape",
                "recvRustType": "semver::Shape",
                "isEnumExtract": true,
                "enumVariant": "Point",
                "enumKind": "struct",
                "enumStructFields": ["x"],
                "enumFieldCount": 2,
                "enumWildcard": true
            }
        ]));
        let out = emit_bindings(&pkg);
        assert!(
            out.contains("    ::semver::Shape::Point { x: arg0, r#type: arg1 }"),
            "{out}"
        );
        assert!(
            out.contains("        ::semver::Shape::Point { x, .. } => IpeMaybe::Just((x) as i64),"),
            "{out}"
        );
    }

    #[test]
    fn fixed_array_param_binds_a_fallible_prelude_local() {
        let pkg = decode(&json!({
            "pkg": "uuid",
            "name": "uuid",
            "functions": [{
                "name": "from_bytes",
                "params": [{"name": "b", "type": "List Int", "ipeType": "List Int", "rustType": "[u8; 16]"}],
                "results": [{"name": "", "type": "Uuid", "rustType": "Uuid"}],
                "effect": "pure"
            }],
            "errors": []
        }));
        let out = emit_bindings(&pkg);
        assert!(
            out.contains(
                "    let b0: [u8; 16] = match to_u8_array::<IpeError, 16>(&arg0) { IpeResult::Ok(a) => a, IpeResult::Err(e) => return IpeResult::Err(e), };"
            ),
            "{out}"
        );
        assert!(out.contains("::uuid::from_bytes(b0)"), "{out}");
        assert!(out.contains("(arg0: Vec<i64>)"), "{out}");
    }

    #[test]
    fn serde_param_deserialises_in_the_prelude_sync_and_inside_async() {
        let sync_pkg = decode(&json!({
            "pkg": "js",
            "name": "js",
            "functions": [{
                "name": "merge",
                "params": [{"name": "v", "type": "String", "ipeType": "String", "rustType": "serde_json::Value"}],
                "results": [{"name": "", "type": "String", "rustType": "serde_json::Value"}],
                "effect": "pure"
            }],
            "errors": []
        }));
        let out = emit_bindings(&sync_pkg);
        assert!(
            out.contains(
                "    let sv_0: serde_json::Value = match serde_json::from_str::<serde_json::Value>(&arg0) { Ok(v) => v, Err(e) => return IpeResult::Err(ipe_error_from_foreign(e)), };"
            ),
            "{out}"
        );
        // Serde-reduced return: turbofish + JSON-text lift.
        assert!(
            out.contains("::js::merge::<serde_json::Value>(sv_0)"),
            "{out}"
        );
        assert!(
            out.contains("serde_json::to_string(&(v)).unwrap_or_default()")
                || out.contains("serde_json::to_string"),
            "{out}"
        );

        let async_pkg = decode(&json!({
            "pkg": "js",
            "name": "js",
            "functions": [{
                "name": "push",
                "params": [{"name": "v", "type": "String", "ipeType": "String", "rustType": "serde_json::Value"}],
                "results": [{"name": "", "type": "Result Error ()", "rustType": "Result<(), Error>"}],
                "effect": "effectful"
            }],
            "errors": []
        }));
        let out = emit_bindings(&async_pkg);
        // The early-return prelude must live INSIDE the async block so its
        // `return IpeResult::Err` matches the block's type, not the IpeTask.
        assert!(
            out.contains("Box::pin(async move { let sv_0: serde_json::Value ="),
            "{out}"
        );
    }

    // ── drop cases: emit nothing, never a wrapper cargo rejects ─────────

    #[test]
    fn degenerate_and_generic_receivers_and_trait_fns_emit_nothing() {
        let pkg = decode(&json!({
            "pkg": "chrono",
            "name": "chrono",
            "functions": [
                {
                    "name": "orphan",
                    "params": [{"name": "self", "type": "Foo", "ipeType": "Foo", "rustType": "&Foo"}],
                    "results": [{"name": "", "type": "Int", "rustType": "i64"}],
                    "effect": "pure"
                },
                {
                    "name": "num_days",
                    "params": [{"name": "self", "type": "DateTime", "ipeType": "DateTime", "rustType": "&DateTime<Tz>"}],
                    "results": [{"name": "", "type": "Int", "rustType": "i64"}],
                    "effect": "pure",
                    "recvType": "DateTime",
                    "recvRustType": "chrono::DateTime<Tz>",
                    "methodName": "num_days"
                },
                {
                    "name": "keyed",
                    "params": [{"name": "v", "type": "Int", "ipeType": "Int", "rustType": "i64"}],
                    "results": [{"name": "", "type": "Int", "rustType": "i64"}],
                    "effect": "pure",
                    "generic": {
                        "params": ["a"],
                        "call": {
                            "kind": "function",
                            "path": ["::tm", "Circle"],
                            "method": "keyed",
                            "args": [0],
                            "argTypes": [{"param": 0}],
                            "ret": {"prim": "i64"},
                            "traitQualifier": ["::tm::Circle", "::tm::Scale"]
                        }
                    }
                },
                {
                    "name": "timestamp",
                    "params": [{"name": "self", "type": "DateTime", "ipeType": "DateTime", "rustType": "&DateTime<Utc>"}],
                    "results": [{"name": "", "type": "Int", "rustType": "i64"}],
                    "effect": "pure",
                    "recvType": "DateTime",
                    "recvRustType": "chrono::DateTime<Utc>",
                    "methodName": "timestamp"
                }
            ],
            "errors": []
        }));
        let out = emit_bindings(&pkg);
        assert!(!out.contains("orphan"), "{out}");
        assert!(!out.contains("num_days"), "{out}");
        assert!(!out.contains("keyed"), "{out}");
        // The CONCRETE generic receiver (DateTime<Utc>) is kept.
        assert!(out.contains("timestamp_from_dateTime"), "{out}");
    }

    /// A trait-qualified method with NO open type params (the async-trait
    /// surface: every param concrete, serde slots turbofished) is a CLOSED
    /// instance — it renders at add time through the instance synthesiser
    /// into a sentinel region, so the interface + DCE see a real wrapper.
    #[test]
    fn closed_trait_method_synthesises_into_a_sentinel_region() {
        let pkg = decode(&json!({
            "pkg": "firestore",
            "name": "firestore",
            "functions": [{
                "name": "get_obj",
                "params": [
                    {"name": "self", "type": "Db", "ipeType": "Db", "rustType": "&Db"},
                    {"name": "collection", "type": "String", "ipeType": "String", "rustType": "&str"},
                    {"name": "id", "type": "String", "ipeType": "String", "rustType": "&str"}
                ],
                "results": [{"name": "", "type": "String", "rustType": "serde_json::Value"}],
                "effect": "effectful",
                "recvType": "Db",
                "recvRustType": "firestore::Db",
                "methodName": "get_obj",
                "generic": {
                    "params": [],
                    "call": {
                        "kind": "method",
                        "path": ["::firestore", "Db"],
                        "method": "get_obj",
                        "receiver": {"arg": 0, "by": "ref"},
                        "args": [1, 2],
                        "argTypes": [
                            {"ctor": "::firestore::Db"},
                            {"prim": "String"},
                            {"prim": "String"}
                        ],
                        "ret": {"serdeValue": true},
                        "borrowAsRefArgs": [1, 2],
                        "traitQualifier": ["::firestore::Db", "::firestore::GetByIdSupport"],
                        "isAsync": true,
                        "methodTurbofish": [{"serdeValue": true}]
                    }
                }
            }],
            "errors": []
        }));
        let out = emit_bindings(&pkg);
        assert!(
            out.contains("// IPE-FFI-WRAPPER BEGIN get_obj_from_db"),
            "{out}"
        );
        assert!(
            out.contains("<::firestore::Db as ::firestore::GetByIdSupport>::get_obj"),
            "{out}"
        );
        // Async instance body: spawned + abort-guarded.
        assert!(
            out.contains("AbortOnDrop::new(handle.abort_handle())"),
            "{out}"
        );
        assert!(
            surviving_ref_names(&pkg).contains("get_obj_from_db"),
            "the interface gate must see the synthesised region"
        );
    }

    #[test]
    fn static_generic_receiver_call_wraps_in_turbofish_angles() {
        let pkg = decode(&json!({
            "pkg": "chrono",
            "name": "chrono",
            "functions": [{
                "name": "now",
                "params": [{"name": "x", "type": "()", "ipeType": "()", "rustType": "()"}],
                "results": [{"name": "", "type": "DateTime", "rustType": "DateTime<Utc>"}],
                "effect": "pure",
                "recvType": "Utc",
                "recvRustType": "chrono::Utc",
                "methodName": "now"
            }],
            "errors": []
        }));
        let out = emit_bindings(&pkg);
        assert!(out.contains("::chrono::Utc::now("), "{out}");
    }

    // ── tri-artifact agreement ──────────────────────────────────────────

    #[test]
    fn sentinel_names_match_kernel_json_and_ipei_names() {
        let pkg = semver_pkg(&json!([
            {
                "name": "parse",
                "params": [{"name": "text", "type": "String", "ipeType": "String", "rustType": "&str"}],
                "results": [{"name": "", "type": "Result Error Version", "rustType": "Result<Version, Error>"}],
                "effect": "fallible"
            },
            {
                "name": "major_field",
                "params": [{"name": "self", "type": "Version", "ipeType": "Version", "rustType": "&Version"}],
                "results": [{"name": "", "type": "Int", "rustType": "u64"}],
                "effect": "pure",
                "recvType": "Version",
                "recvRustType": "semver::Version",
                "methodName": "major",
                "isField": true
            }
        ]));
        let bindings = emit_bindings(&pkg);
        let kernel: serde_json::Value =
            serde_json::from_str(&crate::emit::emit_kernel_json(&pkg)).expect("valid JSON");
        let ipei = crate::emit::emit_ipei(&pkg);
        let functions = kernel
            .pointer("/functions")
            .and_then(serde_json::Value::as_array)
            .expect("functions");
        for f in functions {
            let name = f
                .pointer("/name")
                .and_then(serde_json::Value::as_str)
                .expect("name");
            assert!(
                bindings.contains(&crate::naming::wrapper_begin_sentinel(name)),
                "{name} must open a wrapper region"
            );
            assert!(
                ipei.contains(&format!("\n{name} : ")),
                "{name} must seed the .ipei"
            );
        }
    }

    #[test]
    fn duplicate_wrapper_ref_names_collapse_at_decode() {
        let pkg = semver_pkg(&json!([
            {
                "name": "to_string",
                "params": [{"name": "self", "type": "Version", "ipeType": "Version", "rustType": "&Version"}],
                "results": [{"name": "", "type": "String", "rustType": "String"}],
                "effect": "pure",
                "recvType": "Version",
                "recvRustType": "semver::Version",
                "methodName": "to_string"
            },
            {
                "name": "to_string",
                "params": [{"name": "self", "type": "Version", "ipeType": "Version", "rustType": "&Version"}],
                "results": [{"name": "", "type": "String", "rustType": "String"}],
                "effect": "pure",
                "recvType": "Version",
                "recvRustType": "semver::Version",
                "methodName": "to_string"
            }
        ]));
        assert_eq!(pkg.fns().len(), 1);
        let out = emit_bindings(&pkg);
        assert_eq!(
            out.matches("// IPE-FFI-WRAPPER BEGIN to_string_from_version")
                .count(),
            1,
            "{out}"
        );
    }

    #[test]
    fn pkg_var_getter_renders_as_a_free_call() {
        let pkg = decode(&json!({
            "pkg": "svc",
            "name": "svc",
            "functions": [{
                "name": "max_depth",
                "params": [],
                "results": [{"name": "", "type": "Int", "rustType": "usize"}],
                "effect": "pure",
                "isPkgVar": true
            }],
            "errors": []
        }));
        let out = emit_bindings(&pkg);
        assert!(
            out.contains("pub fn svc_max_depth(_: ()) -> IpeResult<IpeError, i64> {"),
            "{out}"
        );
        assert!(
            out.contains("(::svc::max_depth()).min(i64::MAX as usize) as i64"),
            "{out}"
        );
    }
}
