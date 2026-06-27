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

/// The Rust enum/type name for a user type: `enum_name(["Main"], "Msg")` →
/// `MainMsg`. Mirrors `unionToRustTypeDef`'s `codegenName`.
#[must_use]
pub fn enum_name(module: &[&str], ty: &str) -> String {
    to_camel_case(&format!("{}_{}", module_prefix(module), ty))
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
    to_snake_case(&format!("{}_{}", module_prefix(module), name))
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
}
