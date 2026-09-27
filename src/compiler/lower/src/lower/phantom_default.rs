//! The one default a phantom type variable takes, per position.
//!
//! A phantom is a type variable the solver left free that is not a generic of
//! the enclosing definition: no value of it is ever built or observed, so any
//! inhabited `Clone` type is sound there. Every site that must name one — the
//! ownership classifier, a kernel turbofish pin, a type annotation lowered from
//! a solved type, a producer pin on a phantom-born value — reads it from here,
//! so the type a binder is classified at is the type it is emitted at.

use ipe_ir::{CallPin, IrType};

/// Where a phantom type variable sits, which fixes the type it defaults to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PhantomPosition {
    /// Any value position: a list or set element, a `Maybe` or `Ok` payload,
    /// a dict key or value, a user type's parameter.
    Value,
    /// The error slot `e` of `Result e a`.
    ///
    /// Pinned to the runtime's `IpeError`, the error type the main crate's
    /// `ok_res` and `task_fail` wrappers already fix, so a binder annotated
    /// from the same free variable agrees with the value those wrappers build.
    ResultError,
}

impl PhantomPosition {
    /// The IR type a phantom in this position lowers to.
    pub(super) const fn ir_type(self) -> IrType {
        match self {
            Self::Value => IrType::Str,
            Self::ResultError => IrType::Error,
        }
    }

    /// The Ipê builtin type name the solver-side carrier is built from.
    pub(super) const fn builtin_name(self) -> &'static str {
        match self {
            Self::Value => "String",
            Self::ResultError => "Error",
        }
    }

    /// The Rust type name a turbofish pin spells in the emitted main crate.
    pub(super) const fn rust_name(self) -> &'static str {
        match self {
            Self::Value => "String",
            Self::ResultError => "IpeError",
        }
    }
}

/// Whether `whole` is exactly the concatenation of `parts`.
const fn is_concatenation(whole: &[u8], parts: &[&[u8]]) -> bool {
    let mut rest = whole;
    let mut remaining = parts;
    while let Some((part, tail)) = remaining.split_first() {
        let mut bytes: &[u8] = part;
        while let Some((b, more)) = bytes.split_first() {
            let Some((w, rest_tail)) = rest.split_first() else {
                return false;
            };
            if *w != *b {
                return false;
            }
            rest = rest_tail;
            bytes = more;
        }
        remaining = tail;
    }
    rest.is_empty()
}

const VALUE: &[u8] = PhantomPosition::Value.rust_name().as_bytes();
const ERROR: &[u8] = PhantomPosition::ResultError.rust_name().as_bytes();

/// Whether every kernel turbofish pin spells the phantom default this module fixes.
const TURBOFISH_PINS_AGREE: bool = is_concatenation(
    CallPin::DefaultValue.turbofish().as_bytes(),
    &[b"::<", VALUE, b">"],
) && is_concatenation(
    CallPin::DefaultDict.turbofish().as_bytes(),
    &[b"::<", VALUE, b", ", VALUE, b">"],
) && is_concatenation(
    CallPin::DefaultResultMapErr.turbofish().as_bytes(),
    &[b"::<_, _, ", VALUE, b">"],
) && is_concatenation(
    CallPin::ErrIpeError.turbofish().as_bytes(),
    &[b"::<", ERROR, b">"],
) && is_concatenation(CallPin::None.turbofish().as_bytes(), &[]);

// IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — compile-time `const` assertion (not a runtime panic); fails the BUILD if a `CallPin` turbofish drifts from the phantom-default SSOT [ledger #boundary]
const _: () = assert!(TURBOFISH_PINS_AGREE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_default_is_string() {
        assert_eq!(PhantomPosition::Value.ir_type(), IrType::Str);
        assert_eq!(PhantomPosition::Value.builtin_name(), "String");
    }

    #[test]
    fn result_error_default_is_ipe_error() {
        assert_eq!(PhantomPosition::ResultError.ir_type(), IrType::Error);
        assert_eq!(PhantomPosition::ResultError.builtin_name(), "Error");
    }

    #[test]
    fn concatenation_refuses_a_near_miss() {
        assert!(is_concatenation(b"::<String>", &[b"::<", b"String", b">"]));
        assert!(!is_concatenation(b"::<i64>", &[b"::<", b"String", b">"]));
        assert!(!is_concatenation(
            b"::<String>>",
            &[b"::<", b"String", b">"]
        ));
        assert!(!is_concatenation(b"::<String", &[b"::<", b"String", b">"]));
    }
}
