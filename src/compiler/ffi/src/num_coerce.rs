//! Scalar numeric coercion at the Ipê↔Rust FFI boundary — the DAG leaf.
//!
//! Exactly ONE source of every scalar width cast in generated wrapper code:
//! `num_saturate` narrows an Ipê carrier (`i64`/`f64`) into a foreign
//! parameter width, `num_widen_scalar` widens a foreign return into the
//! carrier. Both are TOTAL and SATURATING — a value outside the target's
//! range clamps to the nearest representable bound, never wraps, never
//! panics, never sign-flips (`u64::MAX as i64 == -1` is the defect class
//! this module exists to kill).
//!
//! Platform correctness by construction: `usize`/`isize` are platform-width,
//! so they route through `try_from` — a bare `as` would truncate on 32-bit,
//! which all-64-bit CI can never catch.
//!
//! Sanctioned divergence from the golden oracle: a value above `i64::MAX`
//! saturates rather than wraps or errors (`oracle_divergence = true`; the
//! clamp is total and documented, satisfying "no silent numeric coercion").

/// Every Rust numeric primitive width.
#[must_use]
pub fn is_numeric_rust(t: &str) -> bool {
    matches!(
        t,
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
    )
}

/// The Ipê CARRIER type a foreign numeric width travels as across the FFI
/// boundary.
///
/// Every integer width is carried as Ipê `Int` (`i64`), every float as Ipê
/// `Float` (`f64`). `None` for a non-numeric type (the caller leaves it
/// alone). The wrapper param for a numeric arg is always the carrier; the
/// call site narrows to the foreign width via [`num_saturate`].
#[must_use]
pub fn num_carrier(width: &str) -> Option<&'static str> {
    match width {
        "f32" | "f64" => Some("f64"),
        w if is_numeric_rust(w) => Some("i64"),
        _ => None,
    }
}

/// Render the expression that coerces an Ipê scalar (`i64` for `Int`, `f64`
/// for `Float`) INTO a foreign numeric param of Rust width `raw`, saturating.
///
/// `expr` must be a side-effect-free expression (a bound local) — both the
/// `isize` and `usize` arms evaluate it more than once in the generated code.
#[must_use]
pub fn num_saturate(raw: &str, expr: &str) -> String {
    let par = format!("({expr})");
    match raw {
        // Precision-lossy but total.
        "f32" => format!("{par} as f32"),
        // Identity: the carrier IS the width.
        "f64" | "i64" => expr.to_owned(),
        // Signed narrowing: clamp into [MIN, MAX] of the target, lossless `as`.
        "i8" | "i16" | "i32" => {
            format!("{par}.clamp({raw}::MIN as i64, {raw}::MAX as i64) as {raw}")
        }
        // Unsigned narrowing: clamp into [0, MAX], then a lossless `as`.
        "u8" | "u16" | "u32" => format!("{par}.clamp(0, {raw}::MAX as i64) as {raw}"),
        // Every non-negative i64 fits u64; negatives saturate to 0.
        "u64" => format!("{par}.max(0) as u64"),
        // Wider than i64: i128 is a pure sign-preserving widen; u128 saturates
        // negatives to 0 (a bare `as u128` would sign-extend -1 to ~3.4e38).
        "i128" => format!("{par} as i128"),
        "u128" => format!("{par}.max(0) as u128"),
        // Platform-width → try_from (32-bit-correct by construction).
        "usize" => format!("usize::try_from({par}.max(0)).unwrap_or(usize::MAX)"),
        "isize" => format!(
            "isize::try_from({expr}).unwrap_or_else(|_| if {par} < 0 {{ isize::MIN }} else {{ isize::MAX }})"
        ),
        // Non-numeric width: total fallback, unreachable behind num_carrier.
        _ => format!("{par} as {raw}"),
    }
}

/// A scalar RETURN widening: the Ipê carrier the value arrives as, plus the
/// rendered coercion expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidenedScalar {
    /// The Ipê carrier type the widened value travels as (`"i64"` / `"f64"`).
    pub carrier: &'static str,
    /// The rendered Rust expression producing the carrier-typed value.
    pub expr: String,
}

/// Widen a foreign scalar return of Rust width `raw` to its Ipê carrier,
/// total + saturating for widths that exceed `i64`. `None` for a non-numeric
/// type (the caller leaves the return unchanged).
///
/// `expr` may be any Rust expression, including a side-effecting foreign call.
/// Every generated form evaluates `expr` exactly once.
#[must_use]
pub fn num_widen_scalar(raw: &str, expr: &str) -> Option<WidenedScalar> {
    match raw {
        // Lossless: every value fits in i64 after widening (isize ≤ i64).
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "isize" => Some(WidenedScalar {
            carrier: "i64",
            expr: format!("({expr}) as i64"),
        }),
        // Unsigned ≥ i64 range: a bare `as i64` would sign-flip
        // (`u64::MAX as i64 == -1`). Saturate via min into i64::MAX.
        "u64" | "usize" | "u128" => Some(WidenedScalar {
            carrier: "i64",
            expr: format!("({expr}).min(i64::MAX as {raw}) as i64"),
        }),
        // Signed wide: bind once to avoid evaluating a potentially
        // side-effecting expression twice in the unwrap_or branch.
        "i128" => Some(WidenedScalar {
            carrier: "i64",
            expr: format!(
                "{{ let __w = {expr}; i64::try_from(__w).unwrap_or(if __w < 0 {{ i64::MIN }} else {{ i64::MAX }}) }}"
            ),
        }),
        "f32" | "f64" => Some(WidenedScalar {
            carrier: "f64",
            expr: format!("({expr}) as f64"),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_numeric_width_has_a_carrier_and_nothing_else() {
        for w in [
            "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64", "u128", "isize", "usize",
        ] {
            assert!(is_numeric_rust(w), "{w} must be numeric");
            assert_eq!(num_carrier(w), Some("i64"), "{w} carries as i64");
        }
        for w in ["f32", "f64"] {
            assert!(is_numeric_rust(w), "{w} must be numeric");
            assert_eq!(num_carrier(w), Some("f64"), "{w} carries as f64");
        }
        for w in ["String", "bool", "char", "Vec<u8>", ""] {
            assert!(!is_numeric_rust(w), "{w} must not be numeric");
            assert_eq!(num_carrier(w), None, "{w} has no carrier");
        }
    }

    #[test]
    fn saturate_identity_on_the_carriers() {
        assert_eq!(num_saturate("i64", "x"), "x");
        assert_eq!(num_saturate("f64", "x"), "x");
    }

    #[test]
    fn saturate_signed_and_unsigned_narrowing_clamps() {
        assert_eq!(
            num_saturate("i8", "x"),
            "(x).clamp(i8::MIN as i64, i8::MAX as i64) as i8"
        );
        assert_eq!(
            num_saturate("u16", "x"),
            "(x).clamp(0, u16::MAX as i64) as u16"
        );
    }

    #[test]
    fn saturate_u64_floors_negatives_instead_of_sign_extending() {
        assert_eq!(num_saturate("u64", "x"), "(x).max(0) as u64");
        assert_eq!(num_saturate("u128", "x"), "(x).max(0) as u128");
    }

    #[test]
    fn saturate_platform_widths_route_through_try_from() {
        assert_eq!(
            num_saturate("usize", "x"),
            "usize::try_from((x).max(0)).unwrap_or(usize::MAX)"
        );
        assert_eq!(
            num_saturate("isize", "x"),
            "isize::try_from(x).unwrap_or_else(|_| if (x) < 0 { isize::MIN } else { isize::MAX })"
        );
    }

    #[test]
    fn widen_lossless_widths_are_a_plain_as() {
        for w in ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "isize"] {
            let ws = num_widen_scalar(w, "r").expect("numeric");
            assert_eq!(ws.carrier, "i64");
            assert_eq!(ws.expr, "(r) as i64");
        }
    }

    #[test]
    fn widen_unsigned_wide_saturates_instead_of_sign_flipping() {
        for w in ["u64", "usize", "u128"] {
            let ws = num_widen_scalar(w, "r").expect("numeric");
            assert_eq!(ws.carrier, "i64");
            assert_eq!(ws.expr, format!("(r).min(i64::MAX as {w}) as i64"));
        }
    }

    #[test]
    fn widen_i128_saturates_both_directions() {
        let ws = num_widen_scalar("i128", "r").expect("numeric");
        assert_eq!(ws.carrier, "i64");
        assert_eq!(
            ws.expr,
            "{ let __w = r; i64::try_from(__w).unwrap_or(if __w < 0 { i64::MIN } else { i64::MAX }) }"
        );
    }

    #[test]
    fn widen_i128_expr_appears_once_in_emitted_form() {
        // A side-effecting foreign call passed as expr must not be duplicated
        // in the generated code — each evaluation would invoke the foreign fn
        // again, producing a different value in the overflow branch.
        let call = "some_foreign_fn()";
        let ws = num_widen_scalar("i128", call).expect("numeric");
        let occurrences = ws.expr.matches(call).count();
        assert_eq!(
            occurrences, 1,
            "expr `{call}` appears {occurrences} times in emitted i128 widen — must be exactly once: {}",
            ws.expr
        );
    }

    #[test]
    fn widen_floats_go_to_f64_and_non_numerics_pass_through() {
        let ws = num_widen_scalar("f32", "r").expect("numeric");
        assert_eq!(ws.carrier, "f64");
        assert_eq!(ws.expr, "(r) as f64");
        assert_eq!(num_widen_scalar("String", "r"), None);
    }

    /// The emitted saturation forms must themselves be sound Rust: evaluate
    /// each shape at the extremes here so the generated-code behaviour is
    /// pinned by a real computation, not just by string equality.
    #[test]
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        clippy::cast_lossless,
        clippy::cast_possible_wrap
    )] // evaluates the exact emitted cast forms at the extremes — the casts ARE the subject
    fn saturation_semantics_at_the_extremes() {
        let x: i64 = -1;
        assert_eq!((x).max(0) as u64, 0);
        assert_eq!((x).clamp(0, u16::MAX as i64) as u16, 0);
        assert_eq!((x).clamp(i8::MIN as i64, i8::MAX as i64) as i8, -1);
        let big: i64 = i64::MAX;
        assert_eq!((big).clamp(i8::MIN as i64, i8::MAX as i64) as i8, i8::MAX);
        assert_eq!(usize::try_from((x).max(0)).unwrap_or(usize::MAX), 0);
        let r: u64 = u64::MAX;
        assert_eq!((r).min(i64::MAX as u64) as i64, i64::MAX);
        let r128: i128 = i128::MIN;
        let __w = r128;
        assert_eq!(
            i64::try_from(__w).unwrap_or(if __w < 0 { i64::MIN } else { i64::MAX }),
            i64::MIN
        );
    }
}
