//! The one type every phantom type variable defaults to.
//!
//! A phantom is a type variable the solver left free that is not a generic of
//! the enclosing definition: no value of it is ever built or observed, so any
//! inhabited `Clone` type is sound there. Every site that must name one — the
//! ownership classifier, a kernel turbofish pin, a type annotation lowered from
//! a solved type, a producer pin on a phantom-born value — reads it from here.
//!
//! The default is ONE carrier, whatever slot the variable occupies (a `Maybe`
//! payload, a `Result` error slot, a dict key). A variable reaching several
//! slots — `Maybe a` in one argument and `Result a Int` in another — is then
//! pinned to the same type at every one, so no two producers of the same free
//! variable can disagree. `String` is `Clone + Ord + Hash + Send`, so it also
//! satisfies every bound a generic slot may impose on the variable.

use ipe_ir::{CallPin, IrType};

/// The IR type a phantom lowers to.
pub(super) const PHANTOM_IR_TYPE: IrType = IrType::Str;

/// The Ipê builtin type name the solver-side carrier is built from.
pub(super) const PHANTOM_BUILTIN_NAME: &str = "String";

/// The Rust type name a turbofish pin spells in the emitted main crate.
const PHANTOM_RUST_NAME: &[u8] = b"String";

/// Whether a phantom in a slot takes the default or refuses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PhantomSlot {
    /// Any value position: a list or set element, a `Maybe`/`Result` payload
    /// or error, a dict key or value, a user type's parameter.
    Defaulted,
    /// A position whose type fixes the emitted carrier.
    ///
    /// A function arrow's operands and a `Program` shape tag: a phantom here
    /// is not defaulted, so type lowering sees it and refuses (IPE-L0102).
    CarrierFixing,
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

const VALUE: &[u8] = PHANTOM_RUST_NAME;

/// Whether every phantom turbofish pin spells the default this module fixes.
const TURBOFISH_PINS_AGREE: bool = is_concatenation(
    CallPin::DefaultValue.turbofish().as_bytes(),
    &[b"::<", VALUE, b">"],
) && is_concatenation(
    CallPin::DefaultDict.turbofish().as_bytes(),
    &[b"::<", VALUE, b", ", VALUE, b">"],
) && is_concatenation(
    CallPin::DefaultResultMapErr.turbofish().as_bytes(),
    &[b"::<_, _, ", VALUE, b">"],
) && is_concatenation(CallPin::None.turbofish().as_bytes(), &[]);

// IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — compile-time `const` assertion (not a runtime panic); fails the BUILD if a `CallPin` turbofish drifts from the phantom-default SSOT [ledger #boundary]
const _: () = assert!(TURBOFISH_PINS_AGREE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phantom_default_is_string() {
        assert_eq!(PHANTOM_IR_TYPE, IrType::Str);
        assert_eq!(PHANTOM_BUILTIN_NAME, "String");
        assert_eq!(PHANTOM_RUST_NAME, b"String");
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
