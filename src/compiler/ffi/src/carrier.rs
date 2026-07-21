//! The closed carrier set for Ipê-defined Rust types (the `provide` surface).
//!
//! When Ipê DEFINES a Rust type — a struct field type, or a closure parameter
//! / result type — the type it names must be one the wrapper can lift an owned,
//! immutable Ipê value into and out of *totally*. That set is closed and small:
//! the scalar carriers plus a nominal opaque handle already vouched by the
//! crate's own inspection. Anything outside it is refused at the decode
//! boundary (over-drop the whole `provide` entry) rather than emitted as Rust
//! the wrapper cannot soundly coerce — the same parse-don't-validate discipline
//! the `PkgInfo` and `Call` boundaries hold.
//!
//! This module is a pure decode LEAF: it renders no Rust and touches no
//! sandbox path. It is the parse boundary the later `provide` emitters render
//! from, so no raw manifest string ever reaches generated source.

use crate::diag::WireDefect;
use crate::naming::RustIdent;

/// A type an Ipê-defined Rust struct field or closure component may carry.
///
/// Every variant maps to exactly one owned Rust type the existing
/// `owned_value_coercion` path can lift an Ipê value into; `Opaque` is a
/// nominal handle the crate's inspection already validated (its `RustIdent`
/// spelling, never a path — the path resolves through the crate's opaque map).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Carrier {
    /// The Ipê `Int` carrier (`i64`).
    Int,
    /// The Ipê `Float` carrier (`f64`).
    Float,
    /// The Ipê `Bool` carrier (`bool`).
    Bool,
    /// The Ipê `Char` carrier (`char`).
    Char,
    /// The Ipê `String` carrier (owned `String`).
    Str,
    /// The Ipê `Bytes` carrier (`Vec<u8>`).
    Bytes,
    /// A nominal opaque handle named by the crate — its type identifier, whose
    /// absolute path resolves through the crate's opaque-type map at emission.
    Opaque(RustIdent),
}

impl Carrier {
    /// Parse one carrier spelling as it appears in a `provide` manifest entry.
    ///
    /// The scalar spellings are the Ipê-facing carrier names AND their Rust
    /// spellings (both `i64` and `Int` name the integer carrier), so an author
    /// may write either. Any other capitalised identifier is taken as an opaque
    /// handle name and validated as a `RustIdent`.
    ///
    /// # Errors
    ///
    /// [`WireDefect::InvalidType`] when the spelling is empty, is a bare
    /// lowercase word outside the scalar set (a would-be Rust primitive Ipê has
    /// no carrier for, e.g. `u128`/`str`), or is not a legal identifier.
    pub fn parse(s: &str) -> Result<Self, WireDefect> {
        let t = s.trim();
        let invalid = || WireDefect::InvalidType { got: s.to_owned() };
        match t {
            "i64" | "Int" => return Ok(Self::Int),
            "f64" | "Float" => return Ok(Self::Float),
            "bool" | "Bool" => return Ok(Self::Bool),
            "char" | "Char" => return Ok(Self::Char),
            "String" | "Str" => return Ok(Self::Str),
            "Bytes" => return Ok(Self::Bytes),
            _ => {}
        }
        // A lowercase-led word that was not a known scalar is a Rust primitive
        // or borrow Ipê cannot carry (`u32`, `usize`, `str`, `&T`) — refuse it
        // rather than misread it as an opaque handle.
        if !t.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return Err(invalid());
        }
        RustIdent::parse(t).map(Self::Opaque).map_err(|_| invalid())
    }

    /// The owned Rust type this carrier lowers to, for a scalar carrier. An
    /// [`Carrier::Opaque`] returns its bare handle name; the emitter absolutizes
    /// it through the crate's opaque map (this leaf never renders a path).
    #[must_use]
    pub fn rust_owned(&self) -> &str {
        match self {
            Self::Int => "i64",
            Self::Float => "f64",
            Self::Bool => "bool",
            Self::Char => "char",
            Self::Str => "String",
            Self::Bytes => "Vec<u8>",
            Self::Opaque(id) => id.as_str(),
        }
    }

    /// The Ipê surface type this carrier presents to a consumer signature.
    #[must_use]
    pub fn ipe_surface(&self) -> &str {
        match self {
            Self::Int => "Int",
            Self::Float => "Float",
            Self::Bool => "Bool",
            Self::Char => "Char",
            Self::Str => "String",
            Self::Bytes => "Bytes",
            Self::Opaque(id) => id.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_spellings_parse_by_either_name() {
        for (rust, ipe, carrier) in [
            ("i64", "Int", Carrier::Int),
            ("f64", "Float", Carrier::Float),
            ("bool", "Bool", Carrier::Bool),
            ("char", "Char", Carrier::Char),
            ("String", "Str", Carrier::Str),
        ] {
            assert_eq!(Carrier::parse(rust), Ok(carrier.clone()), "{rust}");
            assert_eq!(Carrier::parse(ipe), Ok(carrier.clone()), "{ipe}");
        }
        assert_eq!(Carrier::parse("Bytes"), Ok(Carrier::Bytes));
        // Whitespace is trimmed.
        assert_eq!(Carrier::parse("  Int  "), Ok(Carrier::Int));
    }

    #[test]
    fn a_capitalised_word_is_an_opaque_handle() {
        let c = Carrier::parse("Counter").expect("opaque");
        assert_eq!(c, Carrier::Opaque(RustIdent::parse("Counter").unwrap()));
        assert_eq!(c.rust_owned(), "Counter");
        assert_eq!(c.ipe_surface(), "Counter");
    }

    #[test]
    fn rust_primitives_without_an_ipe_carrier_are_refused() {
        // Widths Ipê collapses to Int/Float on the READ side have no carrier on
        // the DEFINE side (Ipê only offers i64/f64), so a struct field cannot
        // name them — refuse rather than silently widen and mis-coerce.
        for bad in ["u8", "u32", "u64", "usize", "i32", "f32", "str", "isize"] {
            assert!(
                matches!(Carrier::parse(bad), Err(WireDefect::InvalidType { .. })),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn injection_and_borrow_shapes_die_at_the_boundary() {
        for bad in [
            "",
            "   ",
            "&Counter",
            "Vec<u8>",
            "Box<dyn Fn()>",
            "String; std::process::exit(1)",
            "A B",
            "9lives",
        ] {
            assert!(
                matches!(Carrier::parse(bad), Err(WireDefect::InvalidType { .. })),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn owned_rust_and_ipe_surface_agree_with_the_existing_coercion_table() {
        // These are exactly the owned types `ipe_type_to_rust` /
        // `owned_value_coercion` already lift, so a struct built from them uses
        // the existing inbound path unchanged.
        assert_eq!(Carrier::Int.rust_owned(), "i64");
        assert_eq!(Carrier::Float.rust_owned(), "f64");
        assert_eq!(Carrier::Str.rust_owned(), "String");
        assert_eq!(Carrier::Bytes.rust_owned(), "Vec<u8>");
        assert_eq!(Carrier::Bool.ipe_surface(), "Bool");
    }
}
