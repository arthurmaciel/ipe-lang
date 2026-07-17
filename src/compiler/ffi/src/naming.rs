//! The single source of truth for every FFI-generated name and sentinel.
//!
//! The three emitters (`.ipei`, `kernel.json`, `_bindings.rs`) and the
//! dead-code-elimination reachability key all derive their names HERE, so the
//! tri-artifact agreement is byte-equal by construction — a three-way name
//! skew (an under-bind that link-fails) is structurally impossible.

use crate::diag::WireDefect;

/// A validated Rust identifier (`^[A-Za-z_][A-Za-z0-9_]*$`).
///
/// The constructor is the only way in, so a crate that names a symbol
/// `"; std::process::Command::new(...)"` can never reach generated source —
/// the injection class dies at the trusted decode surface.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RustIdent(String);

impl RustIdent {
    /// Validate and wrap an identifier.
    ///
    /// # Errors
    ///
    /// [`WireDefect::InvalidIdent`] when the string is empty, starts with a
    /// digit, or contains anything outside `[A-Za-z0-9_]`.
    pub fn parse(s: &str) -> Result<Self, WireDefect> {
        let mut chars = s.chars();
        let head_ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        if head_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            Ok(Self(s.to_owned()))
        } else {
            Err(WireDefect::InvalidIdent { got: s.to_owned() })
        }
    }

    /// The identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RustIdent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A validated `::`-separated path of Rust identifiers (`civil::date`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdentPath(String);

impl IdentPath {
    /// Validate and wrap an identifier path (every segment a [`RustIdent`]).
    ///
    /// # Errors
    ///
    /// [`WireDefect::InvalidModulePath`] when any segment fails identifier
    /// validation or the path is empty.
    pub fn parse(s: &str) -> Result<Self, WireDefect> {
        let invalid = || WireDefect::InvalidModulePath { got: s.to_owned() };
        if s.is_empty() {
            return Err(invalid());
        }
        for seg in s.split("::") {
            RustIdent::parse(seg).map_err(|_| invalid())?;
        }
        Ok(Self(s.to_owned()))
    }

    /// The path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lower-case the first character (`Version` → `version`).
#[must_use]
pub fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_lowercase().chain(chars).collect())
}

/// Upper-case the first character (`uuid` → `Uuid`).
#[must_use]
pub fn capitalise_first(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// Mangle an Ipê type-variable name to its emitted Rust type-param form
/// (`a` → `A`, `msg` → `Msg`).
#[must_use]
pub fn mangle_tvar(s: &str) -> String {
    capitalise_first(s)
}

/// THE disambiguated wrapper-reference name for an FFI function — consumed by
/// the `kernel.json` emitter, the `.ipei` emitter, the `_bindings.rs`
/// BEGIN/END sentinels, and the DCE reachability filter.
///
/// Shape: `lower_first(name)` plus a `_from_<lower_first(recv)>` suffix for an
/// accessor with a receiver. Kind-specific discriminators (`_field`,
/// `_set_field`, `tag_of_`, …) are already baked into `name` by the inspector.
#[must_use]
pub fn wrapper_ref_name(fn_name: &str, recv_type: &str) -> String {
    if recv_type.is_empty() {
        lower_first(fn_name)
    } else {
        format!("{}_from_{}", lower_first(fn_name), lower_first(recv_type))
    }
}

/// Ipê-side module name for a bound crate: `uuid` → `Rust.Uuid`.
#[must_use]
pub fn rust_module_name(pkg_path: &str) -> String {
    let clean: String = pkg_path
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    format!("Rust.{}", capitalise_first(&clean))
}

/// Kernel-name prefix for a bound crate: `Rust_` + the capitalised crate base
/// name, version-segment aware (`stripe-go/v82` → `Rust_Stripe_goV82`).
#[must_use]
pub fn rust_kernel_name(pkg_path: &str) -> String {
    let cap_of = |s: &str| -> String {
        let clean: String = s
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        capitalise_first(&clean)
    };
    let is_version = |s: &str| -> bool {
        s.strip_prefix('v')
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
    };
    let segs: Vec<&str> = pkg_path.split('/').filter(|s| !s.is_empty()).collect();
    let base = match segs.as_slice() {
        [] => "Ffi".to_owned(),
        [.., prev, last] if is_version(last) => format!("{}{}", cap_of(prev), cap_of(last)),
        [.., last] => cap_of(last),
    };
    format!("Rust_{base}")
}

/// The literal prefix that opens a per-fn wrapper region in the emitted
/// `_bindings.rs`.
///
/// The DCE filter splits on these so it can drop an unreached wrapper
/// without parsing Rust; anything outside a BEGIN/END pair is preamble-class
/// and kept unconditionally.
pub const WRAPPER_SENTINEL_PREFIX: &str = "// IPE-FFI-WRAPPER BEGIN ";

/// The END sentinel line (closes the most-recently-opened wrapper region).
pub const WRAPPER_END_SENTINEL: &str = "// IPE-FFI-WRAPPER END";

/// The full BEGIN sentinel line for a wrapper of the given reference name.
#[must_use]
pub fn wrapper_begin_sentinel(ref_name: &str) -> String {
    format!("{WRAPPER_SENTINEL_PREFIX}{ref_name}")
}

/// The wrapper value-arg identifier for index `j` (`arg0`, `arg1`, …).
#[must_use]
pub fn arg_name(j: usize) -> String {
    format!("arg{j}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_helpers() {
        assert_eq!(lower_first("Version"), "version");
        assert_eq!(lower_first(""), "");
        assert_eq!(capitalise_first("uuid"), "Uuid");
        assert_eq!(mangle_tvar("a"), "A");
        assert_eq!(mangle_tvar("msg"), "Msg");
    }

    #[test]
    fn wrapper_ref_name_disambiguates_accessors_by_receiver() {
        assert_eq!(wrapper_ref_name("Parse", ""), "parse");
        assert_eq!(
            wrapper_ref_name("major_field", "Version"),
            "major_field_from_version"
        );
    }

    #[test]
    fn module_and_kernel_names() {
        assert_eq!(rust_module_name("uuid"), "Rust.Uuid");
        assert_eq!(rust_module_name("serde-json"), "Rust.Serde_json");
        assert_eq!(rust_kernel_name("uuid"), "Rust_Uuid");
        assert_eq!(rust_kernel_name("stripe-go/v82"), "Rust_Stripe_goV82");
        assert_eq!(rust_kernel_name(""), "Rust_Ffi");
    }

    #[test]
    fn sentinels_key_off_the_wrapper_ref_name() {
        assert_eq!(
            wrapper_begin_sentinel("parse"),
            "// IPE-FFI-WRAPPER BEGIN parse"
        );
        assert!(wrapper_begin_sentinel("x").starts_with(WRAPPER_SENTINEL_PREFIX));
        assert_eq!(WRAPPER_END_SENTINEL, "// IPE-FFI-WRAPPER END");
    }

    #[test]
    fn arg_names_are_positional() {
        assert_eq!(arg_name(0), "arg0");
        assert_eq!(arg_name(12), "arg12");
    }

    #[test]
    fn rust_ident_accepts_identifiers_and_kills_injection_shapes() {
        assert!(RustIdent::parse("parse").is_ok());
        assert!(RustIdent::parse("_private2").is_ok());
        assert!(RustIdent::parse("Version").is_ok());
        for bad in [
            "",
            "2fast",
            "a-b",
            "a b",
            "a::b",
            "; std::process::Command::new(\"sh\")",
            "名前",
        ] {
            assert!(RustIdent::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn ident_path_validates_every_segment() {
        assert!(IdentPath::parse("civil").is_ok());
        assert!(IdentPath::parse("civil::date").is_ok());
        for bad in ["", "::", "a::", "::b", "a::b-c", "a; rm -rf /"] {
            assert!(IdentPath::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }
}
