//! Sky → Rust identifier naming rules (M0 subset).
//!
//! Ports the relevant arms of `Sky/Generate/Rust/Builder/Naming.hs`:
//! `toCamelCase` / `toSnakeCase` and the module-prefixing used to derive Rust
//! type and function names. M0 only needs the `Main` module's enum + functions,
//! but the conversions are written generally so they match the reference.

use sky_ir::KernelFn;

/// Convert a Sky module-prefixed name to `UpperCamelCase` (used for type names).
///
/// `Sky_Core_Error_Error` → `SkyCoreErrorError`, `Main_Msg` → `MainMsg`.
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

/// Convert a Sky module-prefixed name to `snake_case` (used for function names).
///
/// `Sky_Core_List_map` → `sky_core_list_map`, `Main_update` → `main_update`.
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

/// The dotted module prefix rendered with `_` separators (`["Sky","Core","Io"]`
/// → `Sky_Core_Io`). This matches `moduleNameToRust` (dots → underscores) when
/// the path is supplied as already-split segments.
fn module_prefix(module: &[&str]) -> String {
    module.join("_")
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
/// (`type` → `type_`, `Self` → `Self_`); every other name passes through
/// unchanged, so emission for the M0 golden stays byte-identical.
///
/// The trailing-underscore form is chosen over raw identifiers (`r#name`)
/// because it is valid for *every* keyword — `r#crate` / `r#self` / `r#Self` /
/// `r#super` are themselves rejected by the Rust grammar — so one rule covers
/// the whole set without special cases.
#[must_use]
pub fn mangle_reserved(name: String) -> String {
    if is_reserved_rust_name(&name) {
        let mut mangled = name;
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
/// The program entry `main` in module `Main` is special-cased to `sky_main`
/// (the fixed `fn main` entry-point in the epilogue calls `sky_main()`), matching
/// the Haskell `rustName` rule in `ModuleEmitter.hs`.
#[must_use]
pub fn module_value(module: &[&str], name: &str) -> String {
    if name == "main" && module == ["Main"] {
        return "sky_main".to_owned();
    }
    mangle_reserved(to_snake_case(&format!(
        "{}_{}",
        module_prefix(module),
        name
    )))
}

/// The Rust runtime function name for a kernel built-in (M0 subset). Mirrors
/// `Kernel.kernelToRust`.
#[must_use]
pub const fn kernel_name(k: KernelFn) -> &'static str {
    match k {
        KernelFn::StringFromInt => "string_from_int",
        KernelFn::LogPrintln => "log_println",
    }
}

#[cfg(test)]
mod tests {
    use super::{enum_name, kernel_name, module_value, to_camel_case, to_snake_case};
    use sky_ir::KernelFn;

    #[test]
    fn camel_case_module_prefixed() {
        assert_eq!(to_camel_case("Main_Msg"), "MainMsg");
        assert_eq!(to_camel_case("Sky_Core_Error_Error"), "SkyCoreErrorError");
    }

    #[test]
    fn snake_case_module_prefixed() {
        assert_eq!(to_snake_case("Main_update"), "main_update");
        assert_eq!(to_snake_case("Sky_Core_List_map"), "sky_core_list_map");
    }

    #[test]
    fn enum_and_value_names() {
        assert_eq!(enum_name(&["Main"], "Msg"), "MainMsg");
        assert_eq!(module_value(&["Main"], "update"), "main_update");
    }

    #[test]
    fn entry_main_is_sky_main() {
        assert_eq!(module_value(&["Main"], "main"), "sky_main");
        // `main` outside the `Main` module is NOT the entry.
        assert_eq!(module_value(&["Other"], "main"), "other_main");
    }

    #[test]
    fn kernel_names() {
        assert_eq!(kernel_name(KernelFn::StringFromInt), "string_from_int");
        assert_eq!(kernel_name(KernelFn::LogPrintln), "log_println");
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
        // A bare module value whose snake form *is* a keyword gets mangled.
        assert_eq!(module_value(&["Type"], ""), "type_");
        // The enum-name path routes its camel result through `mangle_reserved`:
        // a camel output that lands on the keyword `Self` is mangled to `Self_`.
        assert_eq!(super::mangle_reserved(to_camel_case("Self")), "Self_");
    }
}
