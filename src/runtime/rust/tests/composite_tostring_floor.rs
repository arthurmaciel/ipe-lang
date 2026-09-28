//! Floor-lock for the `{{expr}}` interpolation renderer: the closed scalar set.
//!
//! `interpolate_to_string` is bounded by the sealed `IpeInterpolate`, implemented
//! for exactly `String` / `Int` / `Float` / `Bool` / `Char`. Each scalar renders
//! as its `String.from*` conversion, and a `String` splices verbatim (never a
//! quoted `Debug` form). A record, custom type, `List`, `Maybe`, tuple, `Dict`
//! or opaque runtime value has no impl, so the type checker refuses it
//! (IPE-T0014) and it never reaches this renderer.

use ipe_runtime_rust::basics::interpolate_to_string;
use ipe_runtime_rust::string::{string_from_char, string_from_float};

#[test]
fn interpolate_int() {
    assert_eq!(interpolate_to_string(5i64), "5");
    assert_eq!(interpolate_to_string(-12i64), "-12");
}

#[test]
fn interpolate_float_is_string_from_float() {
    for f in [42.5f64, 1e6, 1e21, -0.0, 0.0001, f64::INFINITY, f64::NAN] {
        assert_eq!(interpolate_to_string(f), string_from_float(f));
    }
    assert_eq!(interpolate_to_string(42.5f64), "42.5");
}

#[test]
fn interpolate_bool_is_lowercase() {
    assert_eq!(interpolate_to_string(true), "true");
    assert_eq!(interpolate_to_string(false), "false");
}

#[test]
fn interpolate_string_is_identity() {
    assert_eq!(interpolate_to_string("hi".to_string()), "hi");
    assert_eq!(
        interpolate_to_string("say \"hi\"".to_string()),
        "say \"hi\""
    );
}

#[test]
fn interpolate_char_is_string_from_char() {
    assert_eq!(interpolate_to_string('x'), string_from_char('x'));
    assert_eq!(interpolate_to_string('x'), "x");
}
