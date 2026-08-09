//! Ipê → Rust identifier naming rules.
//!
//! Ports the relevant arms of `Ipê/Generate/Rust/Builder/Naming.hs`:
//! `toCamelCase` / `toSnakeCase` and the module-prefixing used to derive Rust
//! type and function names. The conversions are written generally so they
//! match the reference across every module, not just `Main`.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use ipe_ir::KernelFn;

/// The first `snake_case` segment of every runtime-kernel Rust name — the set of
/// identifier prefixes the `ipe_runtime` glob (`pub use ipe_runtime::*;`) owns at
/// the emitted crate root.
///
/// Derived structurally from the authoritative kernel table ([`KernelFn::ALL`] ×
/// [`kernel_name`]) rather than a hand-maintained list, so it can never drift out
/// of sync with the kernels it must reserve: a kernel whose Rust name is
/// `auth_hash_password` contributes `auth`; `string_from_int` contributes
/// `string`. A user top-level function whose default `snake_case` name begins
/// with one of these segments would collide with a kernel at the crate root, so
/// [`module_value`] disambiguates it (see [`disambiguate_user_fn_name`]).
static KERNEL_NAME_PREFIXES: LazyLock<BTreeSet<&'static str>> = LazyLock::new(|| {
    KernelFn::ALL
        .iter()
        .map(|&k| {
            let name = kernel_name(k);
            name.split_once('_').map_or(name, |(head, _)| head)
        })
        .collect()
});

/// If a user top-level function's default `snake_case` name would collide with a
/// runtime-kernel namespace at the emitted crate root, return the disambiguated
/// name; otherwise `None`.
///
/// A user `module Auth` value `hashPassword` folds to `auth_hash_password` —
/// byte-identical to the `Ipe.Auth` kernel's runtime name. Once both the user
/// module's `pub use …::*` and `pub use ipe_runtime::*;` land at the crate root,
/// that name is doubly-defined: the user body's own call to the kernel resolves
/// to the user function (self-recursion), and the two definitions clash. The
/// structural fix is to prefix the user side with `user_`, which no kernel name
/// begins with, making the user function provably disjoint from every kernel
/// while leaving kernel call sites byte-identical. The check keys on the first
/// `snake_case` segment (the kernel-owned namespace), so `auth_mint_token` — a
/// user function with no kernel counterpart but in a kernel-owned namespace —
/// is disambiguated too, keeping the whole `Auth` module on one consistent
/// scheme.
#[must_use]
fn disambiguate_user_fn_name(default_snake: &str) -> Option<String> {
    let head = default_snake
        .split_once('_')
        .map_or(default_snake, |(head, _)| head);
    if KERNEL_NAME_PREFIXES.contains(head) {
        Some(format!("user_{default_snake}"))
    } else {
        None
    }
}

/// Convert a Ipê module-prefixed name to `UpperCamelCase` (used for type names).
///
/// `Ipe_Core_Error_Error` → `IpeCoreErrorError`, `Main_Msg` → `MainMsg`.
/// An underscore is dropped and the following character upper-cased; a trailing
/// underscore with no successor is kept verbatim (mirrors the Haskell pattern
/// `go ('_':c:cs)` falling through to `go (c:cs)` when there is no `c`).
#[must_use]
pub fn to_camel_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = if let Some(&c) = chars.first() {
        out.extend(c.to_uppercase());
        1usize
    } else {
        0
    };
    while let Some(&c) = chars.get(i) {
        if c == '_' {
            if let Some(&n) = chars.get(i + 1) {
                out.extend(n.to_uppercase());
                i += 2;
                continue;
            }
            out.push('_');
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Convert a Ipê module-prefixed name to `snake_case` (used for function names).
///
/// `Ipe_Core_List_map` → `ipe_core_list_map`, `Main_update` → `main_update`.
/// Mirrors the Haskell `toSnakeCase`: the leading character is lower-cased; an
/// underscore followed by a character emits `_` plus the lower-cased successor;
/// an interior upper-case character emits `_` plus its lower-case form.
#[must_use]
pub fn to_snake_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = if let Some(&c) = chars.first() {
        out.extend(c.to_lowercase());
        1usize
    } else {
        0
    };
    while let Some(&c) = chars.get(i) {
        if c == '_' {
            if let Some(&n) = chars.get(i + 1) {
                out.push('_');
                out.extend(n.to_lowercase());
                i += 2;
                continue;
            }
            out.push('_');
        } else if c.is_uppercase() {
            out.push('_');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// The dotted module prefix rendered with `_` separators (`["Ipê","Core","Io"]`
/// → `Ipe_Io`). This matches `moduleNameToRust` (dots → underscores) when
/// the path is supplied as already-split segments.
///
/// The fold is INJECTIVE on the segment list: a literal `_` inside a segment is
/// escaped to `__` before the single-`_` join, so the segment separator and an
/// in-segment underscore are never conflated. Without this, `["Std", "Ui"]` and
/// the single segment `["Std_Ui"]` would both fold to `"Std_Ui"` — two distinct
/// module homes producing one Rust `mod`/type/value name (E0428 + a silent
/// file-overwrite at the split boundary). Ipê module segments cannot contain
/// `.` (the parser splits paths on `.`), so an in-segment `_` is the ONLY
/// ambiguity, and escaping it restores injectivity. The escape is a NO-OP for
/// every segment that contains no `_`, so single-`.`-segment module names (every
/// real program's) fold byte-identically to the previous `join("_")`.
///
/// `pub` (rather than private) because `rust_file::mod_ident` reuses this exact
/// fold for the `ModPath -> Rust mod identifier` namespace. `naming` stays a
/// private module, so `pub` here is crate-scoped in practice
/// (`clippy::redundant_pub_crate` — a `pub(crate)` item inside a private module
/// is equivalent to `pub`).
pub fn module_prefix(module: &[&str]) -> String {
    module
        .iter()
        .map(|seg| seg.replace('_', "__"))
        .collect::<Vec<_>>()
        .join("_")
}

/// Every Rust keyword that cannot appear as a bare identifier in emitted code:
/// the strict keywords (2015 + 2018), the reserved-for-future keywords, and the
/// 2024-reserved `gen`. Mirrors the Go backend's `reservedGoNames` audit. A name
/// in this set is mangled by [`mangle_reserved`] before it reaches the output.
///
/// `union`/`dyn`-style weak/contextual keywords are intentionally excluded: they
/// are legal in identifier position. The four keywords that additionally cannot
/// be written as raw identifiers (`crate`/`self`/`Self`/`super`) are covered
/// here too, which is why [`mangle_reserved`] uses the universally-valid
/// trailing-underscore form rather than `r#name`.
fn is_reserved_rust_name(s: &str) -> bool {
    matches!(
        s,
        // Strict keywords (Rust 2015).
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            // Strict keywords added in the 2018 edition.
            | "async"
            | "await"
            | "dyn"
            // Reserved for future use.
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            // Reserved in the 2024 edition.
            | "gen"
    )
}

/// Rewrite an emitted identifier that collides with a Rust keyword so the
/// generated code compiles. A reserved name gains a trailing underscore
/// (`type` → `type_`, `Self` → `Self_`).
///
/// The trailing-underscore form is chosen over raw identifiers (`r#name`)
/// because it is valid for *every* keyword — `r#crate` / `r#self` / `r#Self` /
/// `r#super` are themselves rejected by the Rust grammar — so one rule covers
/// the whole set without special cases.
///
/// The rule is INJECTIVE: a bare `+_` on reserved names alone would map the
/// keyword `match` to `match_`, colliding with a user identifier literally
/// spelled `match_` (which would pass through unchanged) — two distinct source
/// names folding to one Rust name, an E0428 / silent-shadow at cargo time. To
/// keep the fold one-to-one, any identifier whose stem (after stripping trailing
/// underscores) is reserved is mangled by appending ONE MORE underscore than it
/// already carries: `match` → `match_`, `match_` → `match__`, `match__` →
/// `match___`. The reserved-mangle image (an odd count of trailing underscores
/// over a keyword stem is never required — each preimage adds exactly one) and
/// the pass-through image (non-reserved stem, kept verbatim) are provably
/// disjoint, so no two distinct inputs share an output.
#[must_use]
pub fn mangle_reserved(name: String) -> String {
    let trailing = name.len() - name.trim_end_matches('_').len();
    let stem = &name[..name.len() - trailing];
    if is_reserved_rust_name(stem) {
        let mut mangled = String::with_capacity(name.len() + 1);
        mangled.push_str(&name);
        mangled.push('_');
        mangled
    } else {
        name
    }
}

/// The Rust enum/type name for a user type: `enum_name(["Main"], "Msg")` →
/// `MainMsg`. Mirrors `unionToRustTypeDef`'s `codegenName`. A name colliding
/// with a Rust keyword (e.g. a bare `type Self`) is mangled by
/// [`mangle_reserved`].
#[must_use]
pub fn enum_name(module: &[&str], ty: &str) -> String {
    mangle_reserved(to_camel_case(&format!("{}_{}", module_prefix(module), ty)))
}

/// The Rust function name for a top-level value: `module_value(["Main"],
/// "update")` → `main_update`.
///
/// The program entry `main` in module `Main` is special-cased to `ipe_main`
/// (the fixed `fn main` entry-point in the epilogue calls `ipe_main()`), matching
/// the Haskell `rustName` rule in `ModuleEmitter.hs`.
#[must_use]
pub fn module_value(module: &[&str], name: &str) -> String {
    if name == "main" && module == ["Main"] {
        return "ipe_main".to_owned();
    }
    // A record type-alias auto-constructor is the ONLY top-level value
    // whose Ipê name begins with an uppercase letter (the parser forces every
    // other value / function name lowercase). Snake-casing it would fold the
    // case and could COLLIDE with a same-spelled lowercase value in the same
    // module — e.g. `type alias Row = { … }` yields the constructor `Row`, and a
    // value `row` in the same module would both mangle to `main_row` (a
    // duplicate-definition / wrong-arity miscompile). Emit the constructor's
    // name VERBATIM (case-preserved) so the ident keeps an uppercase letter and
    // is therefore provably disjoint from every snake-cased (all-lowercase)
    // value name, while remaining injective across constructors (Ipê names are
    // unique per module) and across modules (the snake-cased module prefix). The
    // module-level `#![allow(non_snake_case)]` suppresses the style lint.
    if name.chars().next().is_some_and(char::is_uppercase) {
        let prefix = to_snake_case(&module_prefix(module));
        return mangle_reserved(format!("{prefix}_{name}"));
    }
    let default_snake = to_snake_case(&format!("{}_{}", module_prefix(module), name));
    // A user module named after a kernel namespace (e.g. `module Auth`, whose
    // functions fold to `auth_*`) collides with the `ipe_runtime` glob at the
    // crate root. Prefix the user side with `user_` so it is provably disjoint
    // from every kernel; kernel call sites are untouched. See
    // [`disambiguate_user_fn_name`].
    let disambiguated = disambiguate_user_fn_name(&default_snake).unwrap_or(default_snake);
    mangle_reserved(disambiguated)
}

/// The base Rust struct name for a synthesised record shape, derived from its
/// sorted field names: `record_struct_name(["x", "y"])` → `RecXY`.
///
/// Mirrors the Haskell Rust backend's `anonStructName` strategy (a name built
/// from the field set) but with a `Rec_` stem. The result is a *base* name; the
/// caller deduplicates across the program and appends a numeric suffix on the
/// rare event that two distinct field sets camel-case to the same string (e.g.
/// `["a_b"]` and `["a", "b"]`), keeping every synthesised struct collision-free.
/// A base that lands on a Rust keyword is mangled by [`mangle_reserved`].
#[must_use]
pub fn record_struct_name(field_names: &[String]) -> String {
    let joined = field_names.join("_");
    mangle_reserved(to_camel_case(&format!("Rec_{joined}")))
}

/// The field-witness trait name for a record field — field `name` →
/// `IpeHasName`. One trait per field name; the `IpeHas` prefix keeps the
/// namespace disjoint from the `Rec…` struct namespace and from every runtime
/// type. A row generic bounded by this trait can read the field through the
/// trait's getter, so rustc monomorphises the call per concrete record shape.
#[must_use]
pub fn field_witness_trait_name(field_name: &str) -> String {
    to_camel_case(&format!("Ipe_has_{field_name}"))
}

/// The field-witness getter method name — field `name` → `ipe_name`. The
/// `ipe_` prefix cannot collide with a Rust struct field or an inherent method
/// on a registry struct, so an impl of the witness trait can always name it.
#[must_use]
pub fn field_witness_getter_name(field_name: &str) -> String {
    format!("ipe_{}", mangle_reserved(field_name.to_owned()))
}

/// The field-witness associated-type name — field `name` → `Name`. Baking the
/// field type into an associated type (rather than the trait's type parameters)
/// lets one trait serve every field type: the impls stay type-agnostic and the
/// row bound `R: IpeHasName<Name = String>` does the type checking.
#[must_use]
pub fn field_witness_assoc_type_name(field_name: &str) -> String {
    to_camel_case(field_name)
}

/// The Rust runtime function name a kernel built-in emits.
///
/// This table is the emit ground truth `emit_expr` reads. It is pinned equal to
/// the `KernelDef.runtime_fn` descriptor in `ipe_kernels` by the
/// `kernel_name_equals_descriptor_runtime_fn` tripwire, so the two statements of
/// a kernel's emitted symbol can never drift.
#[must_use]
#[allow(clippy::too_many_lines)]
pub const fn kernel_name(k: KernelFn) -> &'static str {
    match k {
        KernelFn::BasicsCompare => "basics_compare",
        KernelFn::StringFromInt => "string_from_int",
        KernelFn::StringFromFloat => "string_from_float",
        // ── String arity-1 ──────────────────────────────────────────────────
        KernelFn::StringLength => "string_length",
        KernelFn::StringIsEmpty => "string_is_empty",
        KernelFn::StringReverse => "string_reverse",
        KernelFn::StringToUpper => "string_to_upper",
        KernelFn::StringToLower => "string_to_lower",
        KernelFn::StringCasefold => "string_casefold",
        KernelFn::StringTrim => "string_trim",
        KernelFn::StringTrimStart => "string_trim_start",
        KernelFn::StringTrimEnd => "string_trim_end",
        KernelFn::StringToInt => "string_to_int",
        KernelFn::StringToFloat => "string_to_float",
        KernelFn::StringFromChar => "string_from_char",
        KernelFn::StringFromList => "string_from_list",
        KernelFn::StringConcat => "string_concat",
        KernelFn::StringWords => "string_words",
        KernelFn::StringLines => "string_lines",
        KernelFn::StringToList => "string_to_list",
        KernelFn::StringIsEmail => "string_is_email",
        KernelFn::StringIsUrl => "string_is_url",
        // ── String arity-2 ──────────────────────────────────────────────────
        KernelFn::StringAppend => "string_append",
        KernelFn::StringContains => "string_contains",
        KernelFn::StringStartsWith => "string_starts_with",
        KernelFn::StringEndsWith => "string_ends_with",
        KernelFn::StringEqualFold => "string_equal_fold",
        KernelFn::StringJoin => "string_join",
        KernelFn::StringSplit => "string_split",
        KernelFn::StringRepeat => "string_repeat",
        KernelFn::StringDropLeft => "string_drop_left",
        KernelFn::StringDropRight => "string_drop_right",
        // ── String arity-3 ──────────────────────────────────────────────────
        KernelFn::StringReplace => "string_replace",
        KernelFn::StringSlice => "string_slice",
        KernelFn::StringPadLeft => "string_pad_left",
        KernelFn::StringPadRight => "string_pad_right",
        KernelFn::StringContainsIn => "string_contains_in",
        KernelFn::StringStartsWithIn => "string_starts_with_in",
        KernelFn::StringEndsWithIn => "string_ends_with_in",
        KernelFn::StringLeft => "string_left",
        KernelFn::StringRight => "string_right",
        KernelFn::StringCons => "string_cons",
        KernelFn::StringUncons => "string_uncons",
        KernelFn::StringPad => "string_pad",
        KernelFn::StringIndexes => "string_indexes",
        KernelFn::StringMap => "string_map",
        KernelFn::StringFilter => "string_filter",
        KernelFn::StringFoldl => "string_foldl",
        KernelFn::StringFoldr => "string_foldr",
        KernelFn::StringAny => "string_any",
        KernelFn::StringAll => "string_all",
        // ── Char arity-1 ────────────────────────────────────────────────────
        KernelFn::CharIsAlpha => "char_is_alpha",
        KernelFn::CharIsDigit => "char_is_digit",
        KernelFn::CharIsLower => "char_is_lower",
        KernelFn::CharIsUpper => "char_is_upper",
        KernelFn::CharToLower => "char_to_lower",
        KernelFn::CharToUpper => "char_to_upper",
        KernelFn::CharToCode => "char_to_code",
        KernelFn::CharFromCode => "char_from_code",
        KernelFn::CharIsAlphaNum => "char_is_alpha_num",
        KernelFn::CharIsHexDigit => "char_is_hex_digit",
        KernelFn::CharIsOctDigit => "char_is_oct_digit",
        KernelFn::LogInfo => "log_info",
        KernelFn::LogDebug => "log_debug",
        KernelFn::LogWarn => "log_warn",
        KernelFn::LogError => "log_error",
        KernelFn::LogInfoWith => "log_info_with",
        KernelFn::LogDebugWith => "log_debug_with",
        KernelFn::LogWarnWith => "log_warn_with",
        KernelFn::LogErrorWith => "log_error_with",
        KernelFn::ListMap => "list_map_consume",
        KernelFn::ListFilter => "list_filter",
        KernelFn::ListFoldl => "list_foldl",
        KernelFn::ListFoldr => "list_foldr",
        KernelFn::ListLength => "list_length",
        KernelFn::ListHead => "list_head",
        KernelFn::ListTail => "list_tail",
        KernelFn::ListMember => "list_member",
        KernelFn::ListRange => "list_range",
        KernelFn::ListReverse => "list_reverse",
        KernelFn::ListAppend => "list_append",
        KernelFn::ListConcat => "list_concat",
        KernelFn::ListTake => "list_take",
        KernelFn::ListDrop => "list_drop",
        KernelFn::ListZip => "list_zip",
        KernelFn::ListCons => "ipe_list_cons",
        KernelFn::ListIsEmpty => "list_is_empty",
        KernelFn::ListConcatMap => "list_concat_map",
        KernelFn::ListIndexedMap => "list_indexed_map",
        KernelFn::ListAny => "list_any",
        KernelFn::ListAll => "list_all",
        KernelFn::ListFind => "list_find",
        // ── List batch ────────────────────────────────────────────────
        KernelFn::ListFilterMap => "list_filter_map",
        KernelFn::ListSortBy => "list_sort_by",
        KernelFn::ListSort => "list_sort",
        KernelFn::ListSortWith => "list_sort_with_order",
        KernelFn::ListSingleton => "list_singleton",
        KernelFn::ListRepeat => "list_repeat",
        KernelFn::ListSum => "list_sum",
        KernelFn::ListProduct => "list_product",
        KernelFn::ListMaximum => "list_maximum",
        KernelFn::ListMinimum => "list_minimum",
        KernelFn::ListUnique => "list_unique",
        KernelFn::ListIntersperse => "list_intersperse",
        KernelFn::ListPartition => "list_partition",
        KernelFn::ListUnzip => "list_unzip",
        KernelFn::ListMap2 => "list_map2",
        KernelFn::ListMap3 => "list_map3",
        KernelFn::ListMap4 => "list_map4",
        KernelFn::ListMap5 => "list_map5",
        KernelFn::BasicsNot => "basics_not",
        KernelFn::BasicsIdentity => "basics_identity",
        KernelFn::BasicsAlways => "basics_always",
        KernelFn::BasicsFst => "basics_fst",
        KernelFn::BasicsSnd => "basics_snd",
        KernelFn::BasicsModBy => "basics_mod_by",
        KernelFn::BasicsClamp => "basics_clamp",
        KernelFn::BasicsToString => "basics_to_string",
        // ── Basics numerics ──────────────────────────────────────────
        KernelFn::BasicsNegate => "basics_negate",
        KernelFn::BasicsAbs => "basics_abs",
        // BasicsSqrt / BasicsMin / BasicsMax reuse the existing Math runtime
        // helpers: `math_sqrt(f64->f64)`, `math_min<T:PartialOrd>`,
        // `math_max<T:PartialOrd>`. No new runtime symbol needed.
        // These arms are merged with their Math.* counterparts below.
        // ── end Basics numerics ──────────────────────────────────────
        // ── Error kernels (Ipe.Error — real Error/ErrorKind ADT) ──
        // Each message constructor classifies its own `ErrorKind` at
        // construction (`ipe_runtime::error::IpeError`, not a shared
        // string-identity). `toString` reuses the existing `errorToString`
        // runtime (`basics_error_to_string`).
        KernelFn::ErrorUnexpected => "ipe_error_unexpected",
        KernelFn::ErrorInvalidInput => "ipe_error_invalid_input",
        KernelFn::ErrorIo => "ipe_error_io",
        KernelFn::ErrorNetwork => "ipe_error_network",
        KernelFn::ErrorFfi => "ipe_error_ffi",
        KernelFn::ErrorDecode => "ipe_error_decode",
        KernelFn::ErrorConflict => "ipe_error_conflict",
        KernelFn::ErrorUnavailable => "ipe_error_unavailable",
        // ── CssSafety (Ipe.CssSafety — Ipe.Css leaf kernels) ──────
        // The bare runtime fn names, re-exported at the `ipe_runtime` root via
        // `pub use css::*`. `safe_value`/`safe_prop_name`/`safe_selector` return
        // `IpeMaybe<String>`; `strip_style_close_kernel` returns `String`.
        KernelFn::CssSafetySafeValue => "safe_value",
        KernelFn::CssSafetySafePropName => "safe_prop_name",
        KernelFn::CssSafetySafeSelector => "safe_selector",
        KernelFn::CssSafetyStripStyleClose => "strip_style_close_kernel",
        KernelFn::ErrorTimeout => "ipe_error_timeout",
        KernelFn::ErrorNotFound => "ipe_error_not_found",
        KernelFn::ErrorPermissionDenied => "ipe_error_permission_denied",
        KernelFn::ErrorToString => "basics_error_to_string",
        KernelFn::ErrorWithMessage => "ipe_error_with_message",
        KernelFn::ErrorIsRetryable => "ipe_error_is_retryable",
        KernelFn::ErrorWithDetails => "ipe_error_with_details",
        KernelFn::ErrorKind => "ipe_error_kind",
        KernelFn::ErrorMessage => "ipe_error_message",
        KernelFn::ErrorKindName => "ipe_error_kind_name",
        KernelFn::MaybeWithDefault => "maybe_with_default",
        KernelFn::MaybeMap => "ipe_maybe_map",
        KernelFn::MaybeAndThen => "ipe_maybe_and_then",
        KernelFn::MaybeMap2 => "maybe_map2",
        KernelFn::MaybeMap3 => "maybe_map3",
        KernelFn::MaybeMap4 => "maybe_map4",
        KernelFn::MaybeMap5 => "maybe_map5",
        KernelFn::MaybeAndMap => "maybe_and_map",
        KernelFn::MaybeCombine => "maybe_combine",
        KernelFn::ResultWithDefault => "result_with_default",
        KernelFn::ResultMap => "ipe_result_map",
        KernelFn::ResultAndThen => "ipe_result_and_then",
        KernelFn::ResultMapError => "ipe_result_map_error",
        KernelFn::ResultMap2 => "result_map2",
        KernelFn::ResultMap3 => "result_map3",
        KernelFn::ResultMap4 => "result_map4",
        KernelFn::ResultMap5 => "result_map5",
        KernelFn::ResultAndMap => "result_and_map",
        KernelFn::ResultCombine => "result_combine",
        KernelFn::ResultTraverse => "result_traverse",
        KernelFn::ResultToMaybe => "ipe_result_to_maybe",
        KernelFn::ResultFromMaybe => "ipe_result_from_maybe",
        // `Math.min` / `Math.max` map to the runtime's generic
        // `math_min<T: PartialOrd>` / `math_max<T: PartialOrd>`: a real
        // polymorphic compare at the argument's actual type — NO `Int`
        // coercion, NO float truncation. Divergence from Ipê:
        // Ipê routes args through AsInt; Ipê-Rust follows Elm's polymorphic
        // comparable (a -> a -> a). Rationale: Elm-conformance.
        // ── Basics numerics: BasicsSqrt / BasicsMin / BasicsMax merged here ──
        KernelFn::BasicsSqrt | KernelFn::MathSqrt => "math_sqrt",
        KernelFn::BasicsMin | KernelFn::MathMin => "math_min",
        KernelFn::BasicsMax | KernelFn::MathMax => "math_max",
        // ── Math constants ───────────────────────────────────────────────────
        KernelFn::MathPi => "math_pi",
        KernelFn::MathE => "math_e",
        KernelFn::MathPhi => "math_phi",
        KernelFn::MathSqrt2 => "math_sqrt2",
        KernelFn::MathInf => "math_inf",
        KernelFn::MathNan => "math_nan",
        KernelFn::MathIsNaN => "math_is_nan",
        // ── Math arity-1 (Int → Int) ─────────────────────────────────────────
        KernelFn::MathAbs => "math_abs",
        // ── Math arity-1 (Float → Float) ────────────────────────────────────
        // MathSqrt merged with BasicsSqrt above.
        KernelFn::MathCbrt => "math_cbrt",
        KernelFn::MathExp => "math_exp",
        KernelFn::MathExp2 => "math_exp2",
        KernelFn::MathLog => "math_log",
        KernelFn::MathLog2 => "math_log2",
        KernelFn::MathLog10 => "math_log10",
        KernelFn::MathSin => "math_sin",
        KernelFn::MathCos => "math_cos",
        KernelFn::MathTan => "math_tan",
        KernelFn::MathAsin => "math_asin",
        KernelFn::MathAcos => "math_acos",
        KernelFn::MathAtan => "math_atan",
        KernelFn::MathSinh => "math_sinh",
        KernelFn::MathCosh => "math_cosh",
        KernelFn::MathTanh => "math_tanh",
        KernelFn::MathAsinh => "math_asinh",
        KernelFn::MathAcosh => "math_acosh",
        KernelFn::MathAtanh => "math_atanh",
        // ── Math arity-1 (Float → Int) ───────────────────────────────────────
        KernelFn::MathFloor => "math_floor",
        KernelFn::MathCeil => "math_ceil",
        KernelFn::MathRound => "math_round",
        KernelFn::MathTrunc => "math_trunc",
        // ── Math arity-2 (Float → Float → Float) ────────────────────────────
        KernelFn::MathPow => "math_pow",
        KernelFn::MathHypot => "math_hypot",
        KernelFn::MathAtan2 => "math_atan2",
        KernelFn::MathMod => "math_mod",
        KernelFn::MathRemainder => "math_remainder",
        // ── Bitwise kernels ─────────────────────────────────────────────────
        KernelFn::BitwiseAnd => "bitwise_and",
        KernelFn::BitwiseOr => "bitwise_or",
        KernelFn::BitwiseXor => "bitwise_xor",
        KernelFn::BitwiseComplement => "bitwise_complement",
        KernelFn::BitwiseShiftLeftBy => "bitwise_shift_left_by",
        KernelFn::BitwiseShiftRightBy => "bitwise_shift_right_by",
        KernelFn::BitwiseShiftRightZfBy => "bitwise_shift_right_zf_by",
        // ── Random seeded (Generator primitives) ────────────────────────────
        KernelFn::RandomSeededInt => "random_seeded_int",
        KernelFn::RandomSeededFloat => "random_seeded_float",
        KernelFn::RandomSeededChoice => "random_seeded_choice",
        KernelFn::ResultOkDefault => "ok_res",
        // ── Dict kernels ────────────────────────────────────────────────────
        KernelFn::DictEmpty => "dict_empty",
        KernelFn::DictIsEmpty => "dict_is_empty",
        KernelFn::DictSize => "dict_size",
        KernelFn::DictKeys => "dict_keys",
        KernelFn::DictValues => "dict_values",
        KernelFn::DictToList => "dict_to_list",
        KernelFn::DictFromList => "dict_from_list",
        KernelFn::DictGet => "dict_get",
        KernelFn::DictMember => "dict_member",
        KernelFn::DictRemove => "dict_remove",
        KernelFn::DictUnion => "dict_union",
        KernelFn::DictMap => "dict_map",
        KernelFn::DictInsert => "dict_insert",
        KernelFn::DictFoldl => "dict_foldl",
        KernelFn::DictSingleton => "dict_singleton",
        KernelFn::DictFoldr => "dict_foldr",
        KernelFn::DictFilter => "dict_filter",
        KernelFn::DictPartition => "dict_partition",
        KernelFn::DictIntersect => "dict_intersect",
        KernelFn::DictDiff => "dict_diff",
        KernelFn::DictUpdate => "dict_update",
        // ── Set kernels ─────────────────────────────────────────────────────
        KernelFn::SetEmpty => "set_empty",
        KernelFn::SetSize => "set_size",
        KernelFn::SetToList => "set_to_list",
        KernelFn::SetFromList => "set_from_list",
        KernelFn::SetMember => "set_member",
        KernelFn::SetInsert => "set_insert",
        KernelFn::SetRemove => "set_remove",
        KernelFn::SetUnion => "set_union",
        KernelFn::SetIntersect => "set_intersect",
        KernelFn::SetDiff => "set_diff",
        KernelFn::SetIsEmpty => "set_is_empty",
        KernelFn::SetSingleton => "set_singleton",
        KernelFn::SetFoldl => "set_foldl",
        KernelFn::SetFoldr => "set_foldr",
        KernelFn::SetMap => "set_map",
        KernelFn::SetFilter => "set_filter",
        KernelFn::SetPartition => "set_partition",
        // ── Bytes kernels ─────────────────────────────────────────────
        KernelFn::BytesEmpty => "bytes_empty",
        KernelFn::BytesLength => "bytes_length",
        KernelFn::BytesIsEmpty => "bytes_is_empty",
        KernelFn::BytesFromString => "bytes_from_string",
        KernelFn::BytesToString => "bytes_to_string",
        KernelFn::BytesFromHex => "bytes_from_hex",
        KernelFn::BytesToHex => "bytes_to_hex",
        KernelFn::BytesFromBase64 => "bytes_from_base64",
        KernelFn::BytesToBase64 => "bytes_to_base64",
        KernelFn::BytesAppend => "bytes_append",
        KernelFn::BytesSlice => "bytes_slice",
        // ── Encoding kernels ──────────────────────────────────────────
        // Encoders are total (infallible) — no type-inference issue.
        KernelFn::EncodingBase64Encode => "base64_encode",
        KernelFn::EncodingUrlEncode => "url_encode",
        KernelFn::EncodingHexEncode => "encoding_hex_encode",
        // Decoders return `Result Error String`. The upstream runtime uses a
        // generic `E: From<String>` bound for flexibility, but generated Ipê code
        // always sets `IpeError = String`. Rust cannot infer `E` when the error
        // arm is `Err _ ->` (discarded), so we route to concrete aliases that pin
        // `E = String`, eliminating the ambiguity without changing semantics.
        KernelFn::EncodingBase64Decode => "ipe_base64_decode",
        KernelFn::EncodingUrlDecode => "ipe_url_decode",
        KernelFn::EncodingHexDecode => "ipe_encoding_hex_decode",
        // ── JsonEnc kernels ────────────────────────────────────────────
        KernelFn::JsonEncString => "json_enc_string",
        KernelFn::JsonEncInt => "json_enc_int",
        KernelFn::JsonEncFloat => "json_enc_float",
        KernelFn::JsonEncBool => "json_enc_bool",
        KernelFn::JsonEncNull => "json_enc_null",
        KernelFn::JsonEncList => "json_enc_list",
        KernelFn::JsonEncObject => "json_enc_object",
        KernelFn::JsonEncEncode => "json_enc_encode",
        // ── JsonDec kernels (shared by Ipe.Config over the same carrier) ─
        KernelFn::JsonDecString | KernelFn::ConfigString => "json_decode_string",
        KernelFn::JsonDecInt | KernelFn::ConfigInt => "json_decode_int",
        KernelFn::JsonDecFloat | KernelFn::ConfigFloat => "json_decode_float",
        KernelFn::JsonDecBool | KernelFn::ConfigBool => "json_decode_bool",
        KernelFn::JsonDecDecodeString => "decode_from_json_string",
        KernelFn::JsonDecField | KernelFn::ConfigField => "decode_field",
        KernelFn::JsonDecAt | KernelFn::ConfigAt => "decode_at",
        KernelFn::JsonDecIndex | KernelFn::ConfigIndex => "decode_index",
        KernelFn::ConfigKeyValuePairs => "decode_key_value_pairs",
        KernelFn::ConfigMaybe => "config_maybe",
        KernelFn::ConfigDict => "config_dict",
        KernelFn::JsonDecList | KernelFn::ConfigList => "decode_list",
        KernelFn::JsonDecMap | KernelFn::DbDecMap | KernelFn::ConfigMap => "decode_map",
        KernelFn::JsonDecAndThen | KernelFn::DbDecAndThen | KernelFn::ConfigAndThen => {
            "decode_and_then"
        }
        KernelFn::JsonDecSucceed | KernelFn::DbDecSucceed | KernelFn::ConfigSucceed => {
            "decode_succeed"
        }
        KernelFn::JsonDecFail | KernelFn::DbDecFail | KernelFn::ConfigFail => "decode_fail",
        KernelFn::JsonDecOneOf | KernelFn::ConfigOneOf => "decode_one_of",
        KernelFn::JsonDecMap2 | KernelFn::DbDecMap2 | KernelFn::ConfigMap2 => "decode_map2",
        KernelFn::JsonDecMap3 | KernelFn::DbDecMap3 | KernelFn::ConfigMap3 => "decode_map3",
        KernelFn::JsonDecMap4 | KernelFn::DbDecMap4 | KernelFn::ConfigMap4 => "decode_map4",
        KernelFn::ConfigMap5 => "decode_map5",
        KernelFn::ConfigMap6 => "decode_map6",
        KernelFn::ConfigMap7 => "decode_map7",
        KernelFn::ConfigMap8 => "decode_map8",
        // ── JsonDecP pipeline kernels ──────────────────────────────────
        KernelFn::JsonDecPRequired => "decode_pipeline_required",
        KernelFn::JsonDecPOptional => "decode_pipeline_optional",
        KernelFn::JsonDecPCustom => "decode_pipeline_custom",
        KernelFn::JsonDecPRequiredAt => "decode_pipeline_required_at",
        // ── Crypto kernels ─────────────────────────────────────────────
        KernelFn::CryptoSha256 => "crypto_sha256",
        KernelFn::CryptoSha512 => "crypto_sha512",
        KernelFn::CryptoSha1 => "crypto_sha1",
        KernelFn::CryptoMd5 => "crypto_md5",
        KernelFn::CryptoHmacSha256 => "crypto_hmac_sha256",
        KernelFn::CryptoHmacSha512 => "crypto_hmac_sha512",
        // Concrete alias pins E=String — avoids type-inference ambiguity when
        // the Err arm is discarded in generated Ipê code.
        KernelFn::CryptoRsaSha256Sign => "ipe_crypto_rsa_sha256_sign",
        KernelFn::CryptoRsaSha256Verify => "crypto_rsa_sha256_verify",
        KernelFn::CryptoConstantTimeEqual => "crypto_constant_time_equal",
        // Concrete aliases pin E=String for all AEAD Result-returning functions.
        KernelFn::CryptoAesGcmEncrypt => "ipe_aes_gcm_encrypt",
        KernelFn::CryptoAesGcmDecrypt => "ipe_aes_gcm_decrypt",
        KernelFn::CryptoChacha20Encrypt => "ipe_chacha20_encrypt",
        KernelFn::CryptoChacha20Decrypt => "ipe_chacha20_decrypt",
        KernelFn::CryptoAesKeyFromPassword => "crypto_aes_key_from_password",
        KernelFn::CryptoChachaKeyFromPassword => "crypto_chacha_key_from_password",
        KernelFn::CryptoRandomBytes => "crypto_random_bytes",
        KernelFn::CryptoRandomToken => "crypto_random_token",
        // ── Uuid kernels ──────────────────────────────────────────────
        // uuid_v4 / uuid_v7 are `() -> Task Error String`: entropy is
        // an effect, so they return `IpeTask<E, String>` (E inferred from the
        // enclosing Task chain, like `crypto_random_token`).
        KernelFn::UuidV4 => "uuid_v4",
        KernelFn::UuidV7 => "uuid_v7",
        // uuid_parse returns IpeMaybe<String> — no E type, no concretisation.
        KernelFn::UuidParse => "uuid_parse",
        // ── Jwt kernels ───────────────────────────────────────────────
        // Concrete aliases pin E=String so Rust can infer the error type at
        // call sites where the Err arm is discarded (matches the Crypto pattern).
        KernelFn::JwtEncodeHs256 => "ipe_jwt_encode_hs256",
        KernelFn::JwtDecodeHs256 => "ipe_jwt_decode_hs256",
        KernelFn::JwtEncodeRs256 => "ipe_jwt_encode_rs256",
        KernelFn::JwtDecodeRs256 => "ipe_jwt_decode_rs256",
        // ── Jwt builder API ────────────────────────────────────
        KernelFn::JwtClaims => "ipe_jwt_claims",
        KernelFn::JwtHs256 => "ipe_jwt_hs256",
        KernelFn::JwtRs256 => "ipe_jwt_rs256",
        KernelFn::JwtSubject => "ipe_jwt_subject",
        KernelFn::JwtIssuer => "ipe_jwt_issuer",
        KernelFn::JwtAudience => "ipe_jwt_audience",
        KernelFn::JwtExpiresAt => "ipe_jwt_expires_at",
        KernelFn::JwtNotBefore => "ipe_jwt_not_before",
        KernelFn::JwtIssuedAt => "ipe_jwt_issued_at",
        KernelFn::JwtJwtId => "ipe_jwt_jwt_id",
        KernelFn::JwtWithClaim => "ipe_jwt_with_claim",
        KernelFn::JwtEncode => "ipe_jwt_encode",
        KernelFn::JwtDecode => "ipe_jwt_decode",
        // ── Task combinators ────────────────────────────────────────────
        KernelFn::TaskSucceed => "task_succeed",
        KernelFn::TaskFail => "task_fail",
        KernelFn::TaskMap => "task_map",
        KernelFn::TaskMap2 => "task_map2",
        KernelFn::TaskMap3 => "task_map3",
        KernelFn::TaskMap4 => "task_map4",
        KernelFn::TaskMap5 => "task_map5",
        KernelFn::TaskAndThen => "task_and_then",
        KernelFn::TaskMapError => "task_map_error",
        KernelFn::TaskOnError => "task_on_error",
        KernelFn::TaskFromResult => "task_from_result",
        KernelFn::TaskAndThenResult => "task_and_then_result",
        KernelFn::TaskSequence => "task_sequence",
        KernelFn::TaskParallel => "task_parallel",
        // `Task.perform` is a 1-arg legacy alias for `Task.run`; both lower to the
        // same runtime function.
        KernelFn::TaskRun | KernelFn::TaskPerform => "task_run",
        KernelFn::TaskLazy => "task_lazy",
        // ── Task retry surface — special-case emitter intercepts; names here
        // are only reached when the special-case path falls through (never in
        // practice, but needed for exhaustiveness).
        KernelFn::TaskRetryWith => "task_retry_with",
        KernelFn::TaskLinearBackoff => "task_linear_backoff",
        KernelFn::TaskExponentialBackoff => "task_exponential_backoff",
        KernelFn::TaskWithJitter => "task_with_jitter",
        KernelFn::TaskRetryOn => "task_retry_on",
        KernelFn::TaskWithRetryOn => "task_with_retry_on",
        KernelFn::TaskDefaultRetryPolicy => "task_default_retry_policy",
        KernelFn::TaskWithMaxAttempts => "task_with_max_attempts",
        KernelFn::TaskWithBaseMs => "task_with_base_ms",
        KernelFn::TaskWithKind => "task_with_kind",
        // ── Io kernels ──────────────────────────────────────────────────
        KernelFn::IoReadLine => "io_read_line",
        KernelFn::IoReadSecret => "io_read_secret",
        KernelFn::IoWriteStdout => "io_write_stdout",
        KernelFn::IoWriteStderr => "io_write_stderr",
        KernelFn::IoPrintln => "io_println",
        KernelFn::IoEprintln => "io_eprintln",
        // ── Debug kernel (dev-only) ─────────────────────────────────────
        KernelFn::DebugLog => "debug_log",
        // ── Time kernels ────────────────────────────────────────────────
        KernelFn::TimeNow => "time_now",
        KernelFn::TimeSleep => "time_sleep",
        KernelFn::TimeUnixMillis => "time_unix_millis",
        KernelFn::TimeTimeString => "time_time_string",
        KernelFn::TimeIsLeapYear => "time_is_leap_year",
        KernelFn::TimeDaysInMonth => "time_days_in_month",
        // ── System kernels ──────────────────────────────────────────────
        KernelFn::SystemArgs => "system_args",
        KernelFn::SystemGetenv => "system_getenv",
        KernelFn::SystemGetenvOr => "system_getenv_or",
        KernelFn::SystemGetArg => "system_get_arg",
        KernelFn::SystemGetenvInt => "system_getenv_int",
        KernelFn::SystemGetenvBool => "system_getenv_bool",
        KernelFn::SystemSetenv => "system_setenv",
        KernelFn::SystemUnsetenv => "system_unsetenv",
        KernelFn::SystemCwd => "system_cwd",
        KernelFn::SystemLoadEnv => "system_load_env",
        KernelFn::SystemExit => "system_exit",
        // ── Random kernels ──────────────────────────────────────────────
        KernelFn::RandomInt => "random_int",
        KernelFn::RandomFloat => "random_float",
        KernelFn::RandomChoice => "random_choice",
        KernelFn::RandomChoiceMaybe => "random_choice_maybe",
        KernelFn::RandomShuffle => "random_shuffle",
        KernelFn::RandomWeighted => "random_weighted",
        // ── File kernels ────────────────────────────────────────────────
        KernelFn::FileReadFile => "file_read_file",
        KernelFn::FileWriteFile => "file_write_file",
        KernelFn::FileExists => "file_exists",
        KernelFn::FileRemove => "file_remove",
        KernelFn::FileMkdirAll => "file_mkdir_all",
        KernelFn::FileReadFileLimit => "file_read_file_limit",
        KernelFn::FileReadFileBytes => "file_read_file_bytes",
        KernelFn::FileAppend => "file_append",
        KernelFn::FileReadDir => "file_read_dir",
        KernelFn::FileIsDir => "file_is_dir",
        KernelFn::FileTempFile => "file_temp_file",
        KernelFn::FileTempDir => "file_temp_dir",
        KernelFn::FileCopy => "file_copy",
        KernelFn::FileRename => "file_rename",
        KernelFn::FileDelete => "file_delete",
        // ── Process kernels ───────────────────────────────────────────
        KernelFn::ProcessRun => "process_run",
        // ── Http kernels ──────────────────────────────────────────────
        // `HttpParseQuery` maps to `http_parse_query` (pure, no E type).
        // `HttpGet` / `HttpPost` / `HttpRequest` emit through `emit_http_call`
        // (not the standard callee_name path) because they need a `task_map`
        // conversion closure; these names are still registered here so
        // `kernel_name` is total over all KernelFn variants (no _ catch-all).
        // The record-update builder kernels (`HttpWithMethod` / `HttpWith*`)
        // emit through `emit_http_builder_call` rather than the standard
        // `callee_name` path; their entries here keep the function total. The
        // typed-target builders (`HttpDefaultRequest` /
        // `HttpDefaultRequestFromString` / `HttpWithUrl`) instead go through the
        // standard call path to their runtime fns, which perform the fail-closed
        // scheme narrowing.
        KernelFn::HttpGet => "http_get",
        KernelFn::HttpPost => "http_post",
        KernelFn::HttpRequest => "http_request",
        KernelFn::HttpParseQuery => "http_parse_query",
        KernelFn::HttpDefaultRequest => "http_default_request",
        KernelFn::HttpDefaultRequestFromString => "http_default_request_from_string",
        KernelFn::HttpWithMethod => "http_with_method",
        KernelFn::HttpWithTimeout => "http_with_timeout",
        KernelFn::HttpWithBody => "http_with_body",
        KernelFn::HttpWithHeader => "http_with_header",
        KernelFn::HttpWithUrl => "http_with_url",
        KernelFn::HttpWithFollowRedirects => "http_with_follow_redirects",
        KernelFn::HttpWithMaxRedirects => "http_with_max_redirects",
        KernelFn::HttpMethodFromString => "http_method_from_string",
        KernelFn::HttpMethodToString => "http_method_to_string",
        // ── Db kernels ─────────────────────────────────────────────
        // `DbExec`/`DbQuery`/`DbQueryDecode` → `db_exec_params` / `db_query_params` /
        // `db_query_decode_params`.  The Ipê surface type is polymorphic
        // (`exec : Db -> String -> List a -> Task Error Int`) so `a` may be
        // `String`, `Int`, `Float`, `Bool`, or `SqlValue`.  The emitter projects
        // the params list via `ipe_runtime::db::SqlParam::from` which is
        // implemented for all of those types (runtime primitives + generated
        // `StdDbSqlValue`).  The untyped `db_exec(Vec<String>)` variant in the
        // runtime is retained for direct Rust test use but is not emitted
        // for Ipê source.
        KernelFn::DbConnect => "db_connect",
        KernelFn::DbOpen => "db_open",
        KernelFn::DbClose => "db_close",
        KernelFn::DbExecRaw => "db_exec_raw",
        KernelFn::DbExec => "db_exec_params",
        KernelFn::DbQuery => "db_query_params",
        KernelFn::DbQueryDecode => "db_query_decode_params",
        KernelFn::DbGetString => "db_get_string",
        KernelFn::DbGetInt => "db_get_int",
        KernelFn::DbGetBool => "db_get_bool",
        KernelFn::DbGetField => "db_get_field",
        KernelFn::DbInsertRow => "db_insert_row",
        KernelFn::DbGetById => "db_get_by_id",
        KernelFn::DbUpdateById => "db_update_by_id",
        KernelFn::DbDeleteById => "db_delete_by_id",
        KernelFn::DbFindOneByField => "db_find_one_by_field",
        KernelFn::DbFindManyByField => "db_find_many_by_field",
        KernelFn::DbFindByConditions => "db_find_by_conditions",
        KernelFn::DbFindWhere => "db_find_where",
        KernelFn::DbDeleteWhere => "db_delete_where",
        KernelFn::DbInsertFields => "db_insert_fields",
        KernelFn::DbUpdateFields => "db_update_fields",
        KernelFn::DbInsertFieldsReturning => "db_insert_fields_returning",
        KernelFn::DbWithTransaction => "db_with_transaction",
        KernelFn::DbMigrate => "db_migrate_apply",
        // Never emitted as a call — `emit_expr` intercepts it into an inline
        // `Migration` struct literal. Name kept for completeness/`ipe doc`.
        KernelFn::DbDefaultMigration => "db_default_migration",
        // ── Db.Decode kernels ───────────────────────────────────────
        KernelFn::DbDecString => "db_decode_string",
        KernelFn::DbDecInt => "db_decode_int",
        KernelFn::DbDecFloat => "db_decode_float",
        KernelFn::DbDecBool => "db_decode_bool",
        KernelFn::DbDecNullable => "db_decode_nullable",
        // DbDecMap / DbDecAndThen / DbDecFail / DbDecMap2/3/4 share the same
        // runtime function as their JsonDec* counterparts; the arms are merged
        // into the JsonDec* section above to satisfy clippy::match_same_arms.
        KernelFn::DbDecRequired => "db_decode_required",
        KernelFn::DbDecOptional => "db_decode_optional",
        KernelFn::DbDecMoney => "db_decode_money",
        KernelFn::DbDecBytes => "db_decode_bytes",
        // ── Ipe.Db.Sql — SqlFragment builder ───────────────────
        // `int`/`string`/`float`/`bool` share `sql_param`'s runtime symbol —
        // each is a Ipê-level type narrowing of the same generic
        // `sql_param::<T: Into<SqlParam>>`, so no separate runtime fn exists.
        KernelFn::SqlColumn => "sql_column",
        KernelFn::SqlUnsafeFragment => "sql_unsafe_fragment",
        KernelFn::SqlParam
        | KernelFn::SqlInt
        | KernelFn::SqlString
        | KernelFn::SqlFloat
        | KernelFn::SqlBool => "sql_param",
        KernelFn::SqlEq => "sql_eq",
        KernelFn::SqlNe => "sql_ne",
        KernelFn::SqlGt => "sql_gt",
        KernelFn::SqlLt => "sql_lt",
        KernelFn::SqlGte => "sql_gte",
        KernelFn::SqlLte => "sql_lte",
        KernelFn::SqlAnd => "sql_and",
        KernelFn::SqlOr => "sql_or",
        KernelFn::SqlNot => "sql_not",
        KernelFn::SqlIsNull => "sql_is_null",
        KernelFn::SqlIsNotNull => "sql_is_not_null",
        KernelFn::SqlInList => "sql_in_list",
        KernelFn::SqlLike => "sql_like",
        // ── Ipe.Secret — opaque secret-string wrapper ─────
        KernelFn::SecretFromString => "secret_from_string",
        KernelFn::SecretReveal => "secret_reveal",
        KernelFn::SecretUse => "secret_use",
        KernelFn::SecretRedacted => "secret_redacted",
        // ── Ipe.Regex kernels (pure; ungated runtime re-export) ────
        // Names MUST match `ipe_runtime::regex_kernel::*` exactly.
        KernelFn::RegexCompile => "regex_compile",
        KernelFn::RegexMatch => "regex_match",
        KernelFn::RegexFind => "regex_find",
        KernelFn::RegexFindAll => "regex_find_all",
        KernelFn::RegexReplace => "regex_replace",
        KernelFn::RegexSplit => "regex_split",
        // ── Ipe.Path kernels (pure; ungated runtime re-export) ─────
        // Names MUST match `ipe_runtime::path::*` exactly.
        KernelFn::PathFromString => "path_from_string",
        KernelFn::PathToString => "path_to_string",
        KernelFn::PathBase => "path_base",
        KernelFn::PathDir => "path_dir",
        KernelFn::PathExt => "path_ext",
        KernelFn::PathIsAbsolute => "path_is_absolute",
        // ── Ipe.Trace kernels (Task; runtime `ipe_runtime::trace::*`) ───
        KernelFn::TraceSpan => "trace_span",
        KernelFn::TraceEvent => "trace_event",
        KernelFn::TraceAttr => "trace_attr",
        // ── Ipe.Compression kernels (`ipe_runtime::compression::*`) ─────
        KernelFn::CompressionGzip => "compression_gzip",
        KernelFn::CompressionGunzip => "compression_gunzip",
        KernelFn::CompressionZstdCompress => "compression_zstd_compress",
        KernelFn::CompressionZstdDecompress => "compression_zstd_decompress",
        // ── Ipe.Csv kernels (`ipe_runtime::csv::*`) ────────────────────
        KernelFn::CsvParse => "csv_parse",
        KernelFn::CsvParseWithDelimiter => "csv_parse_with_delimiter",
        KernelFn::CsvEncode => "csv_encode",
        KernelFn::CsvEncodeWithDelimiter => "csv_encode_with_delimiter",
        KernelFn::CsvParseStreamFromFile => "csv_parse_stream_from_file",
        // ── Ipe.Cache kernels (`ipe_runtime::cache::*`) ────────────────
        // Names MUST match the runtime fns exactly (`cache_new_raw`).
        KernelFn::CacheNewRaw => "cache_new_raw",
        KernelFn::CacheGet => "cache_get",
        KernelFn::CachePut => "cache_put",
        KernelFn::CacheRemove => "cache_remove",
        KernelFn::CacheClear => "cache_clear",
        KernelFn::CacheSize => "cache_size",
        KernelFn::CacheStats => "cache_stats",
        // ── Ipe.Config format/nullable/load kernels (Config-own fns) ────
        // The 11 combinator/primitive `Config_*` kernels reuse the shared JSON
        // `decode_*` / `json_decode_*` fns (merged into the `JsonDec*` arms
        // above); only these five have Config-specific runtime fns.
        KernelFn::ConfigNullable => "config_nullable",
        KernelFn::ConfigDecodeToml => "config_decode_toml",
        KernelFn::ConfigDecodeYaml => "config_decode_yaml",
        KernelFn::ConfigDecodeJson => "config_decode_json",
        KernelFn::ConfigLoadFromFile => "config_load_from_file",
        // ── Ipe.Email ───────────────────────────────────────────────────
        KernelFn::EmailSend => "email_send",
        // ── Ipe.Crypto typed-key newtypes ───────────────────────────────
        KernelFn::CryptoKeyFromString => "crypto_key_from_string",
        KernelFn::CryptoKeyFromBytes => "crypto_key_from_bytes",
        KernelFn::CryptoMacToHex => "crypto_mac_to_hex",
        KernelFn::CryptoHmacSha256WithKey => "crypto_hmac_sha256_key",
        KernelFn::CryptoHmacSha512WithKey => "crypto_hmac_sha512_key",
        KernelFn::CryptoAesKeyFromPasswordKey => "crypto_aes_key_from_password_key",
        KernelFn::CryptoChachaKeyFromPasswordKey => "crypto_chacha_key_from_password_key",
        KernelFn::CryptoAesGcmEncryptKey => "crypto_aes_gcm_encrypt_key",
        KernelFn::CryptoAesGcmDecryptKey => "crypto_aes_gcm_decrypt_key",
        KernelFn::CryptoChacha20EncryptKey => "crypto_chacha20_encrypt_key",
        KernelFn::CryptoChacha20DecryptKey => "crypto_chacha20_decrypt_key",
        // ── Ipe.Email.EmailAddress ───────────────────────────────────────
        KernelFn::EmailAddressParse => "email_address_parse",
        KernelFn::EmailAddressToString => "email_address_to_string",
        // ── Ipe.Url ──────────────────────────────────────────────────────
        KernelFn::UrlFromString => "url_from_string",
        KernelFn::UrlToString => "url_to_string",
        KernelFn::UrlScheme => "url_scheme",
        KernelFn::UrlHost => "url_host",
        KernelFn::UrlPort => "url_port",
        KernelFn::UrlPath => "url_path",
        KernelFn::UrlQuery => "url_query",
        KernelFn::UrlFragment => "url_fragment",
        KernelFn::UrlBuildQuery => "url_build_query",
        // ── Ipe.Locale ───────────────────────────────────────────────────
        KernelFn::LocaleFromTag => "locale_from_tag",
        KernelFn::LocaleToTag => "locale_to_tag",
        KernelFn::StringToUpperIn => "string_to_upper_in",
        KernelFn::StringToLowerIn => "string_to_lower_in",
        // ── TEA Cmd / Sub / Time kernels (wired) ────────────────────────
        KernelFn::CmdNone => "cmd_none",
        KernelFn::CmdBatch => "cmd_batch",
        // `Task.attempt` shares `cmd_perform` (the Task→Cmd bridge), emitted
        // with args swapped in `emit_tea_call`.
        KernelFn::CmdPerform | KernelFn::TaskAttempt => "cmd_perform",
        KernelFn::CmdMap => "cmd_map",
        KernelFn::SubNone => "sub_none",
        KernelFn::SubBatch => "sub_batch",
        KernelFn::SubEvery => "sub_every",
        KernelFn::SubMap => "sub_map",
        KernelFn::TimeEvery => "time_every",
        // ── Reserved TEA kernels (NOT emittable; emit path returns CompilerBug) ─────
        // kernel_name is still required for any exhaustive match on KernelFn.
        KernelFn::CmdPublish => "cmd_publish",
        KernelFn::CmdPublishNoEcho => "cmd_publish_no_echo",
        KernelFn::SubSubscribeTopic => "sub_subscribe_topic",
        KernelFn::PubSubPublish => "pubsub_publish",
        KernelFn::PubSubPublishNoEcho => "pubsub_publish_no_echo",
        KernelFn::PubSubTopic => "pubsub_topic",
        // ── Ipe.Http.Server kernels (wired) ─────────────────────────────────
        KernelFn::ServerGet => "server_get",
        KernelFn::ServerPost => "server_post",
        KernelFn::ServerPut => "server_put",
        KernelFn::ServerDelete => "server_delete",
        KernelFn::ServerAny => "server_any",
        KernelFn::ServerApi => "server_api",
        KernelFn::ServerStatic => "server_static",
        KernelFn::ServerListen => "server_listen",
        KernelFn::ServerText => "server_text",
        KernelFn::ServerJson => "server_json",
        KernelFn::ServerHtml => "server_html",
        KernelFn::ServerWithStatus => "server_with_status",
        KernelFn::ServerWithHeader => "server_with_header",
        KernelFn::ServerRedirect => "server_redirect",
        KernelFn::ServerParam => "server_param",
        KernelFn::ServerQueryParam => "server_query_param",
        KernelFn::ServerHeader => "server_header",
        KernelFn::ServerGetCookie => "server_get_cookie",
        KernelFn::ServerBody => "server_body",
        KernelFn::ServerPath => "server_path",
        KernelFn::ServerMethod => "server_method",
        KernelFn::ServerCookieNew => "server_cookie",
        KernelFn::ServerWithCookie => "server_with_cookie",
        KernelFn::MiddlewareWithCors => "middleware_with_cors",
        KernelFn::MiddlewareWithLogging => "middleware_with_logging",
        KernelFn::MiddlewareWithBasicAuth => "middleware_with_basic_auth",
        KernelFn::MiddlewareWithRateLimit => "middleware_with_rate_limit",
        KernelFn::MiddlewareWithCsrf => "middleware_with_csrf",
        KernelFn::RateLimitAllow => "rate_limit_allow",
        // ── Ipe.Ui / Ipe.Html render kernels (fully wired) ──────────────
        KernelFn::UiLayout => "ui_layout",
        KernelFn::UiLayoutWith => "ui_layout_with",
        // `Html.toString` is a distinct kernel sharing `HtmlRender`'s runtime fn.
        KernelFn::HtmlRender | KernelFn::HtmlToString => "html_render_",
        KernelFn::HtmlEscapeText => "html_escape_text_",
        KernelFn::HtmlEscapeAttr => "html_escape_attr_",
        KernelFn::HtmlAttrToString => "html_attr_to_string_",
        // ── Ipe.Web app-entry kernels ───────────────────────────────────
        KernelFn::WebApp => "web_app",
        KernelFn::WebAppRouted => "web_app_routed",
        KernelFn::WebRoute => "web_route",
        KernelFn::WebRenderStatic => "web_render_static",
        // ── Ipe.Terminal app-entry kernels ──────────────────────────────
        KernelFn::TerminalAppScreen => "tui_app_ui",
        // ── Ipe.WebView app-entry kernel ────────────────────────────────
        KernelFn::WebViewApp => "webview_app",
        // ── Ipe.Ui element builders ──────────────────────────────────────
        KernelFn::UiNone => "ui_none_",
        KernelFn::UiText => "ui_text_",
        KernelFn::UiHtml => "ui_html_",
        KernelFn::UiCells => "ui_cells_",
        KernelFn::UiNode => "ui_node_",
        KernelFn::UiTaggedNode => "ui_tagged_node_",
        KernelFn::UiButton => "ui_button_",
        KernelFn::UiLink => "ui_link_",
        KernelFn::UiImage => "ui_image_",
        KernelFn::UiAbove => "ui_above_",
        KernelFn::UiBelow => "ui_below_",
        KernelFn::UiOnLeft => "ui_on_left_",
        KernelFn::UiOnRight => "ui_on_right_",
        KernelFn::UiInFront => "ui_in_front_",
        KernelFn::UiBehind => "ui_behind_",
        // ── Ipe.Ui attribute builders ────────────────────────────────────
        KernelFn::UiSpacing => "ui_spacing_",
        KernelFn::UiPadding => "ui_padding_",
        KernelFn::UiPaddingXY => "ui_padding_xy_",
        KernelFn::UiPaddingEach => "ui_padding_each_",
        KernelFn::UiWidth => "ui_width_",
        KernelFn::UiHeight => "ui_height_",
        KernelFn::UiCenterX => "ui_center_x_",
        KernelFn::UiCenterY => "ui_center_y_",
        KernelFn::UiAlignLeft => "ui_align_left_",
        KernelFn::UiAlignRight => "ui_align_right_",
        KernelFn::UiAlignTop => "ui_align_top_",
        KernelFn::UiAlignBottom => "ui_align_bottom_",
        KernelFn::UiPointer => "ui_pointer_",
        KernelFn::UiClip => "ui_clip_",
        KernelFn::UiClipX => "ui_clip_x_",
        KernelFn::UiClipY => "ui_clip_y_",
        KernelFn::UiScrollbars => "ui_scrollbars_",
        KernelFn::UiScrollbarX => "ui_scrollbar_x_",
        KernelFn::UiScrollbarY => "ui_scrollbar_y_",
        KernelFn::UiGridColumns => "ui_grid_columns_",
        // ── Ipe.Ui Length builders ───────────────────────────────────────
        KernelFn::UiPx => "ui_px_",
        KernelFn::UiFill => "ui_fill_",
        KernelFn::UiContent => "ui_content_",
        KernelFn::UiShrink => "ui_shrink_",
        KernelFn::UiFillPortion => "ui_fill_portion_",
        KernelFn::UiVh => "ui_vh_",
        KernelFn::UiVw => "ui_vw_",
        KernelFn::UiMinimum => "ui_minimum_",
        KernelFn::UiMaximum => "ui_maximum_",
        // ── Ipe.Ui Color builders ────────────────────────────────────────
        KernelFn::UiRgb => "ui_rgb_",
        KernelFn::UiRgba => "ui_rgba_",
        KernelFn::UiWhite => "ui_white_",
        KernelFn::UiBlack => "ui_black_",
        KernelFn::UiTransparent => "ui_transparent_",
        KernelFn::UiColorCss => "ui_color_css_",
        // ── Background / Border / Font sub-modules ───────────────────────
        KernelFn::BackgroundColor => "ui_background_color_",
        KernelFn::BackgroundImage => "ui_background_image_",
        KernelFn::BackgroundLinearGradient => "ui_background_linear_gradient_",
        KernelFn::BorderWidth => "ui_border_width_",
        KernelFn::BorderRounded => "ui_border_rounded_",
        KernelFn::BorderColor => "ui_border_color_",
        KernelFn::BorderWidthEach => "ui_border_width_each_",
        KernelFn::BorderShadow => "ui_border_shadow_",
        KernelFn::BorderGlow => "ui_border_glow_",
        KernelFn::BorderInnerShadow => "ui_border_inner_shadow_",
        KernelFn::FontSize => "ui_font_size_",
        KernelFn::FontColor => "ui_font_color_",
        KernelFn::FontFamily => "ui_font_family_",
        KernelFn::FontBold => "ui_font_bold_",
        KernelFn::FontItalic => "ui_font_italic_",
        // ── extended Ipe.Ui / Font / Background / Border builders ──
        KernelFn::UiSquare => "ui_square_",
        KernelFn::UiWidescreen => "ui_widescreen_",
        KernelFn::UiCinemascope => "ui_cinemascope_",
        KernelFn::UiAspectRatio => "ui_aspect_ratio_",
        KernelFn::UiAspectRatioWH => "ui_aspect_ratio_wh_",
        KernelFn::UiHtmlAttribute => "ui_html_attribute_",
        KernelFn::UiName => "ui_name_",
        KernelFn::UiStyle => "ui_style_",
        KernelFn::UiTransitionRaw => "ui_transition_raw_",
        KernelFn::UiGridTracksRaw => "ui_grid_tracks_raw_",
        KernelFn::UiAnimateRaw => "ui_animate_raw_",
        // Breakpoint constants + wrapper
        KernelFn::UiBreakpoint => "ui_breakpoint_",
        KernelFn::UiMediaQuery => "ui_media_query_",
        KernelFn::UiMobile => "ui_mobile_",
        KernelFn::UiTablet => "ui_tablet_",
        KernelFn::UiDesktop => "ui_desktop_",
        KernelFn::UiDarkMode => "ui_dark_mode_",
        KernelFn::UiLightMode => "ui_light_mode_",
        KernelFn::UiReducedMotion => "ui_reduced_motion_",
        KernelFn::UiOnPseudo => "ui_on_pseudo_",
        KernelFn::UiHover => "ui_hover_",
        KernelFn::UiFocus => "ui_focus_",
        KernelFn::UiFocusVisible => "ui_focus_visible_",
        KernelFn::UiActive => "ui_active_",
        KernelFn::UiDisabled => "ui_disabled_",
        KernelFn::BackgroundHoverColor => "ui_bg_hover_color_",
        KernelFn::BackgroundFocusColor => "ui_bg_focus_color_",
        KernelFn::BackgroundActiveColor => "ui_bg_active_color_",
        KernelFn::BackgroundDisabledColor => "ui_bg_disabled_color_",
        KernelFn::BorderSolid => "ui_border_solid_",
        KernelFn::BorderDashed => "ui_border_dashed_",
        KernelFn::BorderDotted => "ui_border_dotted_",
        KernelFn::BorderHoverColor => "ui_border_hover_color_",
        KernelFn::BorderFocusColor => "ui_border_focus_color_",
        KernelFn::BorderActiveColor => "ui_border_active_color_",
        KernelFn::BorderHoverWidth => "ui_border_hover_width_",
        KernelFn::BorderHoverRounded => "ui_border_hover_rounded_",
        KernelFn::FontWeight => "ui_font_weight_",
        KernelFn::FontSemiBold => "ui_font_semi_bold_",
        KernelFn::FontRegular => "ui_font_regular_",
        KernelFn::FontLight => "ui_font_light_",
        KernelFn::FontExtraBold => "ui_font_extra_bold_",
        KernelFn::FontBlack => "ui_font_black_",
        KernelFn::FontUnderline => "ui_font_underline_",
        KernelFn::FontNoDecoration => "ui_font_no_decoration_",
        KernelFn::FontLineThrough => "ui_font_line_through_",
        KernelFn::FontLetterSpacing => "ui_font_letter_spacing_",
        KernelFn::FontWordSpacing => "ui_font_word_spacing_",
        KernelFn::FontAlignLeft => "ui_font_align_left_",
        KernelFn::FontAlignRight => "ui_font_align_right_",
        KernelFn::FontAlignCenter => "ui_font_align_center_",
        KernelFn::FontCenter => "ui_font_center_",
        KernelFn::FontJustify => "ui_font_justify_",
        KernelFn::FontSansSerif => "ui_font_sans_serif_",
        KernelFn::FontSerif => "ui_font_serif_",
        KernelFn::FontMonospace => "ui_font_monospace_",
        KernelFn::FontHoverColor => "ui_font_hover_color_",
        KernelFn::FontFocusColor => "ui_font_focus_color_",
        KernelFn::FontActiveColor => "ui_font_active_color_",
        KernelFn::FontDisabledColor => "ui_font_disabled_color_",
        KernelFn::FontHoverSize => "ui_font_hover_size_",
        // ── Ipe.Ui.Region ──────────────────────────────────────────────
        KernelFn::RegionMainContent => "ui_region_main_content_",
        KernelFn::RegionNavigation => "ui_region_navigation_",
        KernelFn::RegionFooter => "ui_region_footer_",
        KernelFn::RegionAside => "ui_region_aside_",
        KernelFn::RegionHeading => "ui_region_heading_",
        KernelFn::RegionLabel => "ui_region_label_",
        KernelFn::RegionAnnounce => "ui_region_announce_",
        KernelFn::RegionAnnounceUrgently => "ui_region_announce_urgently_",
        // ── Ui.input + Ui.describe + desc* constructors ───────────────────────
        KernelFn::UiDescribe => "ui_describe_",
        KernelFn::UiDescNone => "ui_desc_none_",
        KernelFn::UiDescParagraph => "ui_desc_paragraph_",
        KernelFn::UiDescMain => "ui_desc_main_",
        KernelFn::UiDescNavigation => "ui_desc_navigation_",
        KernelFn::UiDescContentInfo => "ui_desc_content_info_",
        KernelFn::UiDescComplementary => "ui_desc_complementary_",
        KernelFn::UiDescLivePolite => "ui_desc_live_polite_",
        KernelFn::UiDescLiveAssertive => "ui_desc_live_assertive_",
        KernelFn::UiDescHeading => "ui_desc_heading_",
        KernelFn::UiDescLabel => "ui_desc_label_",
        // ── Ipe.Ui.Input ───────────────────────────────────────────────
        KernelFn::InputLabelAbove => "input_label_above_",
        KernelFn::InputLabelBelow => "input_label_below_",
        KernelFn::InputLabelLeft => "input_label_left_",
        KernelFn::InputLabelRight => "input_label_right_",
        KernelFn::InputLabelHidden => "input_label_hidden_",
        KernelFn::InputPlaceholder => "input_placeholder_",
        KernelFn::InputText => "input_text_",
        KernelFn::InputMultiline => "input_multiline_",
        KernelFn::InputEmail => "input_email_",
        KernelFn::InputUsername => "input_username_",
        KernelFn::InputSearch => "input_search_",
        KernelFn::InputCurrentPassword => "input_current_password_",
        KernelFn::InputNewPassword => "input_new_password_",
        KernelFn::InputCheckbox => "input_checkbox_",
        KernelFn::InputSlider => "input_slider_",
        KernelFn::InputOption => "input_option_",
        KernelFn::InputRadio => "input_radio_",
        KernelFn::InputRadioRow => "input_radio_row_",
        // ── Html element builders ────────────────────────────────────────
        KernelFn::HtmlTextNode => "html_text_node_",
        KernelFn::HtmlRawNode => "html_raw_node_",
        KernelFn::HtmlDoctype => "html_doctype_",
        KernelFn::HtmlTitleNode => "html_title_node_",
        KernelFn::HtmlStyleNode => "html_style_node_",
        // `Html.node` / `Html.voidNode` share the generic `html_node_` sink;
        // the wire tag is a real runtime arg, and `voidNode` bakes an empty
        // children vec at the emit site.
        KernelFn::HtmlNode | KernelFn::HtmlVoidNode => "html_node_",
        // Ipe.Html.Attributes retained primitives. The full call (including the
        // key argument) is produced by `emit_ui_call`; these names are the bare
        // runtime helpers.
        KernelFn::HtmlAttribute => "html_named_attr_",
        KernelFn::HtmlBoolAttribute => "html_bool_named_attr_",
        KernelFn::HtmlNoAttr => "html_no_attr_",
        // Event-attribute builders
        KernelFn::UiOnClick => "ui_on_click_",
        KernelFn::UiOnFocus => "ui_on_focus_",
        KernelFn::UiOnBlur => "ui_on_blur_",
        KernelFn::UiOnMouseOver => "ui_on_mouse_over_",
        KernelFn::UiOnMouseOut => "ui_on_mouse_out_",
        KernelFn::UiOnInput => "ui_on_input_",
        KernelFn::UiOnChange => "ui_on_change_",
        KernelFn::UiOnKeyDown => "ui_on_key_down_",
        KernelFn::UiOnKeyUp => "ui_on_key_up_",
        KernelFn::UiOnBool => "ui_on_bool_",
        KernelFn::UiOnSubmit => "ui_on_submit_",
        KernelFn::UiOnFile => "ui_on_file_",
        // Ipe.Html.Events builders — emitted via the dedicated
        // `emit_ui_call` arm (`html_event_shape().is_some()`), so this generic
        // name map is not consulted at emit time; the arms are here to keep the
        // match exhaustive and the name faithful to each runtime constructor.
        KernelFn::HtmlOnClick
        | KernelFn::HtmlOnFocus
        | KernelFn::HtmlOnBlur
        | KernelFn::HtmlOnMouseOver
        | KernelFn::HtmlOnMouseOut => "html_on_msg_",
        KernelFn::HtmlOnInput
        | KernelFn::HtmlOnChange
        | KernelFn::HtmlOnKeyDown
        | KernelFn::HtmlOnKeyUp => "html_on_string_",
        KernelFn::HtmlOnBool => "html_on_bool_",
        KernelFn::HtmlOnSubmit => "html_on_raw_",
        // ── Terminal line app-entry ─────────────────────────────────────
        // TerminalAppLines is emitted via the dedicated emit_console_call path;
        // kernel_name is kept for match exhaustiveness.
        KernelFn::TerminalAppLines => "ipe_console_app_",
        // ── Ipe.Auth runtime function names (auth.rs) ──────────────────
        KernelFn::AuthHashPassword => "auth_hash_password",
        KernelFn::AuthHashPasswordCost => "auth_hash_password_cost",
        KernelFn::AuthVerifyPassword => "auth_verify_password",
        KernelFn::AuthPasswordStrength => "auth_password_strength",
        KernelFn::AuthSignToken => "auth_sign_token",
        KernelFn::AuthVerifyToken => "auth_verify_token",
        KernelFn::AuthRegister => "auth_register",
        KernelFn::AuthLogin => "auth_login",
        KernelFn::AuthSetRole => "auth_set_role",
        // ── Ipe.Http.Server.Stream runtime function names (server_stream.rs)
        KernelFn::StreamStream => "server_stream_stream",
        KernelFn::StreamEmit => "server_stream_emit",
        KernelFn::StreamFinish => "server_stream_finish",
        KernelFn::StreamWithContentType => "server_stream_with_content_type",
        // ── Ipe.Http.Stream runtime function names (http_stream.rs) ─
        KernelFn::HttpStreamOpen => "http_stream_open",
        KernelFn::HttpStreamForEachChunk => "http_stream_for_each_chunk",
        KernelFn::HttpStreamClose => "http_stream_close",
        KernelFn::HttpStreamChunks => "sub_subscribe_stream",
        // ── Ipe.Http.Server.WebSocket runtime function names (server.rs) ─
        KernelFn::WsDefaultCfg => "ws_server_default_cfg",
        KernelFn::WsWithOnConnect => "ws_server_with_on_connect",
        KernelFn::WsWithOnMessage => "ws_server_with_on_message",
        KernelFn::WsWithOnClose => "ws_server_with_on_close",
        KernelFn::WsWithOnError => "ws_server_with_on_error",
        KernelFn::WsWithMaxMessageBytes => "ws_server_with_max_message_bytes",
        KernelFn::WsWithOriginPatterns => "ws_server_with_origin_patterns",
        KernelFn::WsUpgrade => "server_web_socket_upgrade",
        KernelFn::WsSendToClient => "ws_server_send_to_client",
        KernelFn::WsSendBinaryToClient => "ws_server_send_binary_to_client",
        KernelFn::WsBroadcast => "ws_server_broadcast",
        KernelFn::WsCloseClient => "ws_server_close_client",
        // ── Ipe.WebSocket outbound-client runtime fn names (ws_client.rs) ─
        KernelFn::WebSocketConnect => "web_socket_connect",
        KernelFn::WebSocketConnectWith => "web_socket_connect_with",
        KernelFn::WebSocketSend => "web_socket_send",
        KernelFn::WebSocketSendBinary => "web_socket_send_binary",
        KernelFn::WebSocketClose => "web_socket_close",
        KernelFn::WebSocketCloseWithCode => "web_socket_close_with_code",
        // The peephole in `emit_tea_call` rewrites `SubSubscribeWebSocket` to one
        // of the four typed `sub_subscribe_ws_*` fns; this name is the fallback
        // (the `message` kind) and is never emitted via the standard N-arg path.
        KernelFn::SubSubscribeWebSocket => "sub_subscribe_ws_message",
        // ── Ipe.Env — build-time-embedded public config ──────────────────
        KernelFn::EnvPublic => "env_public",
        // ── Ipe.Ui.Lazy ────────────────────────────────────────────────
        KernelFn::LazyLazy => "lazy_lazy_",
        KernelFn::LazyLazy2 => "lazy_lazy2_",
        KernelFn::LazyLazy3 => "lazy_lazy3_",
        KernelFn::LazyLazy4 => "lazy_lazy4_",
        KernelFn::LazyLazy5 => "lazy_lazy5_",
        // ── Ipe.Ui.Keyed ──────────────────────────────────────────────────────
        KernelFn::KeyedColumn => "keyed_column_",
        KernelFn::KeyedRow => "keyed_row_",
        // ── Ipe.Decimal ───────────────────────────────────────────────────────
        KernelFn::DecZero => "decimal_zero",
        KernelFn::DecOne => "decimal_one",
        KernelFn::DecOneHundred => "decimal_one_hundred",
        KernelFn::DecFromString => "decimal_from_string",
        KernelFn::DecFromInt => "decimal_from_int",
        KernelFn::DecFromFloat => "decimal_from_float",
        KernelFn::DecFromMinor => "decimal_from_minor",
        KernelFn::DecToString => "decimal_to_string",
        KernelFn::DecToStringFixed => "decimal_to_string_fixed",
        KernelFn::DecToFloat => "decimal_to_float",
        KernelFn::DecToInt => "decimal_to_int",
        KernelFn::DecToMinor => "decimal_to_minor",
        KernelFn::DecAdd => "decimal_add",
        KernelFn::DecSub => "decimal_sub",
        KernelFn::DecMul => "decimal_mul",
        KernelFn::DecDiv => "decimal_div",
        KernelFn::DecMod => "decimal_mod",
        KernelFn::DecNeg => "decimal_neg",
        KernelFn::DecAbs => "decimal_abs",
        KernelFn::DecFloor => "decimal_floor",
        KernelFn::DecCeil => "decimal_ceil",
        KernelFn::DecRound => "decimal_round",
        KernelFn::DecRoundHalfUp => "decimal_round_half_up",
        KernelFn::DecTruncate => "decimal_truncate",
        KernelFn::DecCompare => "decimal_compare",
        KernelFn::DecEq => "decimal_eq",
        KernelFn::DecNeq => "decimal_neq",
        KernelFn::DecLt => "decimal_lt",
        KernelFn::DecLte => "decimal_lte",
        KernelFn::DecGt => "decimal_gt",
        KernelFn::DecGte => "decimal_gte",
        KernelFn::DecMin => "decimal_min",
        KernelFn::DecMax => "decimal_max",
        KernelFn::DecIsZero => "decimal_is_zero",
        KernelFn::DecIsPositive => "decimal_is_positive",
        KernelFn::DecIsNegative => "decimal_is_negative",
        KernelFn::DecPercentOf => "decimal_percent_of",
        KernelFn::DecAddPercent => "decimal_add_percent",
        KernelFn::DecSubPercent => "decimal_sub_percent",
        KernelFn::DecFormatWith => "decimal_format_with",
        // ── Ipe.Money ───────────────────────────────────────────────────────────
        KernelFn::MoneyMinorUnits => "money_minor_units",
        KernelFn::MoneySymbol => "money_symbol",
        KernelFn::MoneyCurrencyName => "money_currency_name",
        KernelFn::MoneyIsKnownCurrency => "money_is_known_currency",
        KernelFn::MoneyFormat => "money_format",
        KernelFn::MoneyFormatWithCode => "money_format_with_code",
        KernelFn::MoneyAllocate => "money_allocate",
        KernelFn::MoneySetRate => "money_set_rate",
        KernelFn::MoneyGetRate => "money_get_rate",
        KernelFn::MoneyHasRate => "money_has_rate",
        KernelFn::MoneyClearRates => "money_clear_rates",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        enum_name, field_witness_assoc_type_name, field_witness_getter_name,
        field_witness_trait_name, kernel_name, module_value, to_camel_case, to_snake_case,
    };
    use ipe_ir::KernelFn;

    #[test]
    fn field_witness_names_derive_from_field() {
        assert_eq!(field_witness_trait_name("name"), "IpeHasName");
        assert_eq!(field_witness_getter_name("name"), "ipe_name");
        assert_eq!(field_witness_assoc_type_name("name"), "Name");
    }

    #[test]
    fn field_witness_names_camel_case_multiword_fields() {
        // A snake_case Ipê field folds to one CamelCase token in the trait and
        // associated-type names, and stays snake in the prefixed getter.
        assert_eq!(field_witness_trait_name("first_name"), "IpeHasFirstName");
        assert_eq!(field_witness_assoc_type_name("first_name"), "FirstName");
        assert_eq!(field_witness_getter_name("first_name"), "ipe_first_name");
    }

    /// The hazard the row-witness disjointness gate exists to catch: two DISTINCT
    /// surface field names that camel-case to the SAME witness-trait name. Both
    /// spellings are valid lowercase-initial identifiers the parser admits, and
    /// `to_camel_case` erases the distinction, so `field_witness_trait_name` is
    /// NOT injective — the backend must not assume witness names are unique per
    /// field. `EmitCtx::assert_row_witness_names_disjoint` fails such a program
    /// closed rather than emit two `IpeHasFirstName` traits (E0428).
    #[test]
    fn witness_trait_name_is_not_injective_over_field_names() {
        assert_eq!(
            field_witness_trait_name("first_name"),
            field_witness_trait_name("firstName"),
            "snake and camel spellings of the same field collide to one \
             witness-trait name — the gate must reject their coexistence"
        );
    }

    #[test]
    fn field_witness_getter_mangles_reserved_field() {
        // A field whose name is a Rust keyword is keyword-mangled inside the
        // getter suffix, so the emitted method identifier is always valid — and
        // the `ipe_` prefix already keeps it clear of the reserved namespace.
        assert_eq!(field_witness_getter_name("type"), "ipe_type_");
    }

    /// A runtime `false` the optimiser cannot fold, so `assert!(false_marker(),
    /// …)` reads as a deliberate unconditional failure rather than a suspicious
    /// constant condition — keeps this file free of the `clippy::panic` deny.
    const fn false_marker() -> bool {
        std::hint::black_box(false)
    }

    #[test]
    fn camel_case_module_prefixed() {
        assert_eq!(to_camel_case("Main_Msg"), "MainMsg");
        assert_eq!(to_camel_case("Ipe_Core_Error_Error"), "IpeCoreErrorError");
    }

    #[test]
    fn snake_case_module_prefixed() {
        assert_eq!(to_snake_case("Main_update"), "main_update");
        assert_eq!(to_snake_case("Ipe_Core_List_map"), "ipe_core_list_map");
    }

    #[test]
    fn enum_and_value_names() {
        assert_eq!(enum_name(&["Main"], "Msg"), "MainMsg");
        assert_eq!(module_value(&["Main"], "update"), "main_update");
    }

    #[test]
    fn entry_main_is_ipe_main() {
        assert_eq!(module_value(&["Main"], "main"), "ipe_main");
        // `main` outside the `Main` module is NOT the entry.
        assert_eq!(module_value(&["Other"], "main"), "other_main");
    }

    #[test]
    fn record_ctor_name_is_case_preserved_and_collision_free() {
        // a record-alias auto-constructor's uppercase Ipê name must NOT
        // snake-case-fold into a same-spelled lowercase value's ident.
        assert_eq!(module_value(&["Main"], "Row"), "main_Row");
        assert_eq!(module_value(&["Main"], "row"), "main_row");
        assert_ne!(
            module_value(&["Main"], "Row"),
            module_value(&["Main"], "row"),
            "ctor `Row` and value `row` must be distinct idents"
        );
        // A multi-word ctor stays verbatim (never `main_user_profile`, which a
        // value `userProfile` could also produce).
        assert_eq!(module_value(&["Main"], "UserProfile"), "main_UserProfile");
        assert_ne!(
            module_value(&["Main"], "UserProfile"),
            module_value(&["Main"], "userProfile"),
        );
        // Multi-segment module prefix is still snake-cased; only the ctor name
        // is preserved.
        assert_eq!(
            module_value(&["Lib", "State"], "Widget"),
            "lib_state_Widget"
        );
    }

    #[test]
    fn user_module_named_after_kernel_namespace_is_disambiguated() {
        // A user `module Auth` value `hashPassword` folds to `auth_hash_password`,
        // byte-identical to the `Ipe.Auth` kernel runtime name — self-recursion +
        // duplicate-definition once both glob re-exports land at the crate root.
        // The `user_` prefix makes the user side provably disjoint from every
        // kernel while leaving the kernel name untouched.
        assert_eq!(
            kernel_name(KernelFn::AuthHashPassword),
            "auth_hash_password"
        );
        assert_eq!(
            module_value(&["Auth"], "hashPassword"),
            "user_auth_hash_password"
        );
        assert_ne!(
            module_value(&["Auth"], "hashPassword"),
            kernel_name(KernelFn::AuthHashPassword),
            "user Auth.hashPassword must not collide with the Ipe.Auth kernel"
        );
        // Every function in a kernel-owned namespace is disambiguated uniformly,
        // even one with no kernel counterpart (`mintToken`), so the whole module
        // stays on one consistent scheme.
        assert_eq!(module_value(&["Auth"], "mintToken"), "user_auth_mint_token");
        // A user module whose prefix is NOT a kernel namespace is untouched: the
        // composite-server's `Routes.Auth` folds to `routes_auth_*` (first segment
        // `routes`), so it keeps its default name.
        assert_eq!(
            module_value(&["Routes", "Auth"], "handleLogin"),
            "routes_auth_handle_login"
        );
        // The reserved set is derived from the live kernel table, so a `String`
        // module (matching the `String` kernel namespace) is reserved too.
        assert_eq!(
            module_value(&["String"], "myHelper"),
            "user_string_my_helper"
        );
    }

    /// A kernel's emitted runtime symbol is stated in two places: the greppable
    /// `kernel_name` table here (the emit ground truth `emit_expr` reads) and the
    /// `KernelDef.runtime_fn` descriptor field in `ipe_kernels`. This tripwire
    /// pins them equal for every kernel, so the two statements can never drift: a
    /// commit that changes one without the other fails to merge. Emitted output is
    /// therefore identical whichever source a future reader trusts.
    #[test]
    fn kernel_name_equals_descriptor_runtime_fn() {
        let mut drifted: Vec<String> = Vec::new();
        for &k in ipe_kernels::StdlibKernel::ALL {
            let emitted = kernel_name(k);
            let descriptor = k.def().runtime_fn;
            if emitted != descriptor {
                drifted.push(format!(
                    "{k:?}: kernel_name = `{emitted}` but KernelDef.runtime_fn = `{descriptor}`"
                ));
            }
        }
        assert!(
            drifted.is_empty(),
            "emit-symbol drift between kernel_name and KernelDef.runtime_fn:\n{}",
            drifted.join("\n")
        );
    }

    #[test]
    fn kernel_names() {
        assert_eq!(kernel_name(KernelFn::StringFromInt), "string_from_int");
        assert_eq!(kernel_name(KernelFn::StringFromFloat), "string_from_float");
        assert_eq!(kernel_name(KernelFn::LogInfo), "log_info");
        assert_eq!(kernel_name(KernelFn::IoPrintln), "io_println");
        assert_eq!(kernel_name(KernelFn::IoEprintln), "io_eprintln");
        assert_eq!(kernel_name(KernelFn::DebugLog), "debug_log");
        // ── Http kernels ──────────────────────────────────────────────
        assert_eq!(kernel_name(KernelFn::HttpGet), "http_get");
        assert_eq!(kernel_name(KernelFn::HttpPost), "http_post");
        assert_eq!(kernel_name(KernelFn::HttpRequest), "http_request");
        assert_eq!(kernel_name(KernelFn::HttpParseQuery), "http_parse_query");
        assert_eq!(
            kernel_name(KernelFn::HttpDefaultRequest),
            "http_default_request"
        );
        assert_eq!(kernel_name(KernelFn::HttpWithMethod), "http_with_method");
        assert_eq!(kernel_name(KernelFn::HttpWithTimeout), "http_with_timeout");
        assert_eq!(kernel_name(KernelFn::HttpWithBody), "http_with_body");
        assert_eq!(kernel_name(KernelFn::HttpWithHeader), "http_with_header");
        assert_eq!(kernel_name(KernelFn::HttpWithUrl), "http_with_url");
        assert_eq!(
            kernel_name(KernelFn::HttpWithFollowRedirects),
            "http_with_follow_redirects"
        );
        assert_eq!(
            kernel_name(KernelFn::HttpWithMaxRedirects),
            "http_with_max_redirects"
        );
        assert_eq!(
            kernel_name(KernelFn::HttpMethodFromString),
            "http_method_from_string"
        );
        assert_eq!(
            kernel_name(KernelFn::HttpMethodToString),
            "http_method_to_string"
        );
    }

    #[test]
    fn record_struct_names_from_field_sets() {
        use super::record_struct_name;
        assert_eq!(
            record_struct_name(&["x".to_owned(), "y".to_owned()]),
            "RecXY"
        );
        assert_eq!(
            record_struct_name(&["name".to_owned(), "age".to_owned()]),
            "RecNameAge"
        );
        // A single field.
        assert_eq!(record_struct_name(&["count".to_owned()]), "RecCount");
        // Different field sets that camel-case to the SAME base name — the
        // caller disambiguates with a numeric suffix; the base collision is a
        // documented possibility, not a panic.
        assert_eq!(record_struct_name(&["a_b".to_owned()]), "RecAB");
        assert_eq!(
            record_struct_name(&["a".to_owned(), "b".to_owned()]),
            "RecAB"
        );
    }

    #[test]
    fn non_reserved_names_pass_through() {
        assert_eq!(super::mangle_reserved("count".to_owned()), "count");
        assert_eq!(super::mangle_reserved("MainMsg".to_owned()), "MainMsg");
        assert_eq!(
            super::mangle_reserved("main_update".to_owned()),
            "main_update"
        );
    }

    #[test]
    fn reserved_names_get_a_trailing_underscore() {
        for kw in [
            "type", "fn", "match", "self", "Self", "crate", "super", "become", "priv", "typeof",
            "unsized", "virtual", "macro", "gen", "async", "await", "dyn", "true", "false",
        ] {
            assert_eq!(super::mangle_reserved(kw.to_owned()), format!("{kw}_"));
        }
    }

    #[test]
    fn reserved_collisions_mangle_at_emit_helpers() {
        // `main_loop` is not itself reserved, so the keyword segment passes
        // through unchanged once module-prefixed.
        assert_eq!(module_value(&["Main"], "loop"), "main_loop");
        // A bare module value whose snake form is the keyword `type` PLUS the
        // empty-name separator (`type_`) has the reserved stem `type`, so the
        // injective mangle appends one more underscore (`type__`) — keeping it
        // provably disjoint from the mangle of the bare keyword `type`
        // (`type_`), which a different fold could produce.
        assert_eq!(module_value(&["Type"], ""), "type__");
        // The enum-name path routes its camel result through `mangle_reserved`:
        // a camel output that lands on the keyword `Self` is mangled to `Self_`.
        assert_eq!(super::mangle_reserved(to_camel_case("Self")), "Self_");
    }

    /// `mangle_reserved` is INJECTIVE over the reserved set ∪ its `<kw>_` shadow
    /// set: no two distinct inputs share an output. The historical hole was the
    /// keyword `match` mangling to `match_` while a user identifier literally
    /// spelled `match_` passed through unchanged — both `match_`, a silent
    /// collision. The `+_`-per-reserved-stem rule sends `match_` to `match__`.
    #[test]
    fn mangle_reserved_is_injective_over_reserved_and_shadows() {
        use std::collections::BTreeMap;

        let mut images: BTreeMap<String, String> = BTreeMap::new();
        let mut inputs: Vec<String> = Vec::new();
        for kw in [
            "type", "fn", "match", "self", "Self", "crate", "super", "become", "priv", "gen",
            "async", "await", "dyn", "true", "false", "loop", "move",
        ] {
            // The keyword, plus its 0..=3-underscore shadows.
            inputs.push(kw.to_owned());
            inputs.push(format!("{kw}_"));
            inputs.push(format!("{kw}__"));
            inputs.push(format!("{kw}___"));
        }
        // Non-reserved control inputs that must pass through untouched.
        inputs.push("count".to_owned());
        inputs.push("match_x".to_owned());
        inputs.push("matchx_".to_owned());

        for input in inputs {
            let out = super::mangle_reserved(input.clone());
            if let Some(prev) = images.insert(out.clone(), input.clone()) {
                assert!(
                    false_marker(),
                    "mangle_reserved not injective: {prev:?} and {input:?} both fold to {out:?}"
                );
            }
        }
    }
}
