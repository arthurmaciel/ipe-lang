//! `IpeStringify` — the total Ipê value stringifier.
//!
//! Backs `Basics.errorToString` (and `Ipe.Test.debugShow`, which is just
//! `errorToString v`). Go's `Basics_errorToString` returns a `String`
//! verbatim, an `error`'s `.Error()` message, and `fmt.Sprintf("%v", v)`
//! for everything else. The Rust backend mirrors `%v` EXACTLY but TOTALLY:
//! every type reachable from a generic `errorToString` call implements this
//! trait (runtime primitives below; every codegen-emitted record/ADT gets a
//! `IpeStringify` impl from `src/Ipê/Generate/Rust/Builder/Emitter.hs`).
//!
//! Why a trait, not `Debug`: `Debug` QUOTES a `String` (`"hi"`), diverging
//! from Go's unquoted `hi`. A `Display` re-bind is not total (no codegen type
//! emits `Display`). `IpeStringify` is the total, Go-faithful middle path.
//!
//! Totality contract: `ipe_show` NEVER panics — no `unwrap`/`expect`/indexing.
//! A type with no meaningful `%v` analogue (function-typed fields) renders a
//! best-effort placeholder rather than failing.
//!
//! Go `%v` reference (verified against the Go toolchain):
//! - `"hi"` -> `hi`            (string: unquoted)
//! - `42` / `true` / `42.5`    -> Display
//! - `[]int{1,2,3}`            -> `[1 2 3]`   (space-separated, NOT comma)
//! - `[][]int{{1,2},{3,4}}`    -> `[[1 2] [3 4]]`
//! - `T2{1,"a"}` (Ipê tuple)   -> `{1 a}`     (space-separated, no field names)
//! - `R{1,"x"}` (Ipê record)   -> `{1 x}`     (fields in _fieldIndex order)
//! - `map[string]int{...}`     -> `map[a:1 b:2]` (keys SORTED, space-separated)

use crate::core::{IpeMaybe, IpeResult};
use std::collections::HashMap;

/// Total Ipê stringifier. One method, infallible, never panics.
pub trait IpeStringify {
    /// Render `self` byte-identically to Go's `Basics_errorToString` / `%v`.
    fn ipe_show(&self) -> String;
}

// ─── Autoref specialization: total field rendering ───────────────────────────
//
// A codegen-emitted `impl IpeStringify for <GeneratedType>` renders each field
// by calling the field's stringifier. If it called `field.ipe_show()` directly,
// a field of a RUNTIME type that doesn't impl `IpeStringify` (e.g.
// `http_stream::ChunkEvent`) would be a `type-checks ⇒ cargo-fails` E0599 — a
// soundness-floor regression, and a whack-a-mole (every unhandled runtime type
// is a latent failure).
//
// The dispatch makes field rendering TOTAL BY CONSTRUCTION via dtolnay's
// autoref-specialization: a field renders via `IpeStringify` IF its type impls
// it, ELSE falls back to `Debug`. EVERY codegen + runtime type derives `Debug`,
// so this can NEVER fail to compile, regardless of field type.
//
// Mechanism: codegen emits `(&Wrap(&value)).dispatch()` at a CONCRETE field
// type. `Wrap<&T>: ViaIpeStringify` (no autoref) is preferred over
// `&Wrap<T>: ViaDebug` (one autoref) when `T: IpeStringify`; otherwise only the
// `Debug` impl applies. The dispatch is concrete-type-only by design — a generic
// `fn<T>` frame can't select either arm (the same method name on both traits is
// ambiguous when T's bounds are unknown), so the dispatch is emitted INLINE at
// each field site (where the type is concrete or a `IpeStringify + Debug`-bounded
// generic), NOT routed through a generic free function.
// (`basics_error_to_string<T: IpeStringify>` keeps its bound: a top-level
// `errorToString aString` must stay unquoted, which the IpeStringify path
// guarantees; the autoref-`Debug` fallback would quote a String at a generic
// frame.)

/// Newtype carrier for the autoref-specialization receiver. Constructed only by
/// the codegen-emitted `(&Wrap(&field)).dispatch()` field-render expression (and
/// this module's own tests); not part of the user-facing surface.
#[doc(hidden)]
pub struct Wrap<T>(pub T);

/// Higher-priority arm: a `Wrap<&T>` where `T: IpeStringify` renders via the
/// trait (Go-`%v`-faithful — String unquoted, nested generated types via their
/// own impl). Selected with ZERO autoref, so it beats the `Debug` fallback.
#[doc(hidden)]
pub trait ViaIpeStringify {
    fn dispatch(&self) -> String;
}
impl<T: IpeStringify> ViaIpeStringify for Wrap<&T> {
    fn dispatch(&self) -> String {
        self.0.ipe_show()
    }
}

/// Lower-priority arm: ANY `Wrap<T>` where `T: Debug` renders via `Debug`.
/// Reached only by ONE autoref (`&Wrap<T>`), so it loses to `ViaIpeStringify`
/// whenever the field type impls `IpeStringify`. Every type derives `Debug`,
/// so this arm is always available — the dispatch can never E0599.
#[doc(hidden)]
pub trait ViaDebug {
    fn dispatch(&self) -> String;
}
impl<T: core::fmt::Debug> ViaDebug for &Wrap<T> {
    fn dispatch(&self) -> String {
        format!("{:?}", self.0)
    }
}

// ─── Scalars ────────────────────────────────────────────────────────────────

impl IpeStringify for String {
    // Go: a String returns verbatim (UNQUOTED). This is the primary fix.
    fn ipe_show(&self) -> String {
        self.clone()
    }
}

impl IpeStringify for str {
    fn ipe_show(&self) -> String {
        self.to_string()
    }
}

impl IpeStringify for i64 {
    fn ipe_show(&self) -> String {
        self.to_string()
    }
}

impl IpeStringify for f64 {
    // Go's `%v` on a float64 is `strconv.FormatFloat(f, 'g', -1, 64)`: the
    // shortest round-trippable digits, formatted with `%e` when the decimal
    // exponent is < -4 or >= 6 and `%f` otherwise, with `+Inf`/`-Inf`/`NaN`
    // for the non-finite values. Rust's `f64::to_string` matches Go's `%f`
    // branch exactly (42.5 -> "42.5", 1.0 -> "1", 0.0001 -> "0.0001"), but
    // diverges on infinities (`inf`/`-inf`) and never emits exponent form
    // (1e21 -> "1000000000000000000000" instead of Go's "1e+21"). Bridge the
    // gap totally: handle the non-finite cases, then reformat Rust's shortest
    // scientific output to Go's `%g`-`%e` shape only when Go would use it.
    fn ipe_show(&self) -> String {
        let f = *self;
        if f.is_nan() {
            return "NaN".to_string();
        }
        if f.is_infinite() {
            return if f > 0.0 { "+Inf" } else { "-Inf" }.to_string();
        }
        // `{:e}` gives the shortest mantissa + decimal exponent, lowercase `e`,
        // no `+` and no zero-padding on the exponent (e.g. "1e21", "1.5e-5").
        let sci = format!("{f:e}");
        match sci.split_once('e') {
            // Go uses exponent form iff exp < -4 || exp >= 6 (Go `'g'`
            // shortest-mode cut), same threshold as string_from_float (see
            // string.rs). Verified against Go 1.26.2 `fmt %v` ==
            // `strconv.FormatFloat(f,'g',-1,64)`: 1e6 -> "1e+06", 1e15 ->
            // "1e+15", 999999 -> "999999" (see reference-audit.md item 27 for
            // the oracle probe). The `../ipe` reference uses 21 here, which
            // diverges from the Go oracle on every 1e6..1e20 value.
            Some((mantissa, exp_str)) => match exp_str.parse::<i32>() {
                Ok(exp) if !(-4..6).contains(&exp) => {
                    // Go's `%e` exponent: explicit sign, minimum two digits.
                    // i64 widen so `-exp` can't overflow for any i32.
                    let (sign, mag) = if exp < 0 {
                        ('-', -i64::from(exp))
                    } else {
                        ('+', i64::from(exp))
                    };
                    format!("{mantissa}e{sign}{mag:02}")
                }
                _ => f.to_string(),
            },
            None => f.to_string(),
        }
    }
}

impl IpeStringify for bool {
    fn ipe_show(&self) -> String {
        self.to_string()
    }
}

impl IpeStringify for () {
    // Ipê `()` is Go's empty struct; `%v` renders `{}`. Rare in errorToString,
    // kept total for completeness.
    fn ipe_show(&self) -> String {
        "{}".to_string()
    }
}

// ─── References / boxes (delegate) ───────────────────────────────────────────

impl<T: IpeStringify + ?Sized> IpeStringify for &T {
    fn ipe_show(&self) -> String {
        (**self).ipe_show()
    }
}

impl<T: IpeStringify + ?Sized> IpeStringify for Box<T> {
    fn ipe_show(&self) -> String {
        (**self).ipe_show()
    }
}

// ─── Lists ───────────────────────────────────────────────────────────────────

impl<T: IpeStringify> IpeStringify for Vec<T> {
    // Go slice `%v`: `[a b c]` — space-separated, square brackets, empty -> `[]`.
    fn ipe_show(&self) -> String {
        let parts: Vec<String> = self.iter().map(IpeStringify::ipe_show).collect();
        format!("[{}]", parts.join(" "))
    }
}

impl<T: IpeStringify> IpeStringify for [T] {
    fn ipe_show(&self) -> String {
        let parts: Vec<String> = self.iter().map(IpeStringify::ipe_show).collect();
        format!("[{}]", parts.join(" "))
    }
}

// ─── Maps ────────────────────────────────────────────────────────────────────

impl<K: IpeStringify + Ord, V: IpeStringify> IpeStringify for HashMap<K, V> {
    // Go map `%v`: `map[k1:v1 k2:v2]` with keys SORTED, space-separated.
    fn ipe_show(&self) -> String {
        let mut entries: Vec<(&K, &V)> = self.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let parts: Vec<String> = entries
            .iter()
            .map(|(k, v)| format!("{}:{}", k.ipe_show(), v.ipe_show()))
            .collect();
        format!("map[{}]", parts.join(" "))
    }
}

// ─── Tuples (Ipê tuples render like Go's T2/T3 structs: `{a b ...}`) ─────────

impl<A: IpeStringify, B: IpeStringify> IpeStringify for (A, B) {
    fn ipe_show(&self) -> String {
        format!("{{{} {}}}", self.0.ipe_show(), self.1.ipe_show())
    }
}

impl<A: IpeStringify, B: IpeStringify, C: IpeStringify> IpeStringify for (A, B, C) {
    fn ipe_show(&self) -> String {
        format!(
            "{{{} {} {}}}",
            self.0.ipe_show(),
            self.1.ipe_show(),
            self.2.ipe_show()
        )
    }
}

impl<A, B, C, D> IpeStringify for (A, B, C, D)
where
    A: IpeStringify,
    B: IpeStringify,
    C: IpeStringify,
    D: IpeStringify,
{
    fn ipe_show(&self) -> String {
        format!(
            "{{{} {} {} {}}}",
            self.0.ipe_show(),
            self.1.ipe_show(),
            self.2.ipe_show(),
            self.3.ipe_show()
        )
    }
}

// ─── Ipê core ADTs ───────────────────────────────────────────────────────────

impl<T: IpeStringify> IpeStringify for IpeMaybe<T> {
    // Go renders a Ipê `Maybe` (a flattened-struct ADT) with a leaked layout
    // (`{tag payload}` + zero-init inactive fields) that a Rust enum cannot
    // reproduce. Best-effort, total, and human-useful: `Just <v>` / `Nothing`.
    // Documented residual: NOT byte-identical to Go's ADT `%v` (see module doc).
    fn ipe_show(&self) -> String {
        match self {
            IpeMaybe::Just(v) => format!("Just {}", v.ipe_show()),
            IpeMaybe::Nothing => "Nothing".to_string(),
        }
    }
}

impl<E: IpeStringify, A: IpeStringify> IpeStringify for IpeResult<E, A> {
    // Best-effort (same ADT-layout residual as IpeMaybe): `Ok <a>` / `Err <e>`.
    fn ipe_show(&self) -> String {
        match self {
            IpeResult::Ok(a) => format!("Ok {}", a.ipe_show()),
            IpeResult::Err(e) => format!("Err {}", e.ipe_show()),
        }
    }
}

// ─── Runtime opaque value types that flow into errorToString/debugShow ───────
// These are real runtime types (not codegen-emitted), so their IpeStringify
// impls live HERE. A generated ADT can carry them as a payload (e.g.
// `Money(Decimal, …)`, `Claims(Vec<(String, JsonVal)>)`); the codegen's enum
// `ipe_show` calls `.ipe_show()` on the payload, so the type must impl it.

impl IpeStringify for crate::decimal::Decimal {
    // Reuse the canonical Decimal renderer (normalized, no trailing zeros) —
    // matches `Decimal.toString`. Total (no panic).
    fn ipe_show(&self) -> String {
        crate::decimal::decimal_to_string(*self)
    }
}

// `serde_json` is only in the dependency tree under the `json` feature; gate the
// impl so a project that doesn't enable `json` still compiles (the unconditional
// form was an E0433 `unresolved crate serde_json` on default features).
#[cfg(feature = "json")]
impl IpeStringify for serde_json::Value {
    // Best-effort, total: the compact JSON text. Not Go's flattened-struct `%v`
    // layout (a JSON value has no Go-struct analogue), but human-useful and never
    // panics. `to_string` on serde_json::Value is infallible.
    fn ipe_show(&self) -> String {
        self.to_string()
    }
}

// NB: `IpeError` is `type IpeError = String` (see config.rs), so it stringifies
// through the `String` impl above — rendering its message verbatim, exactly
// like Go's `error.Error()` branch in `Basics_errorToString`. No separate impl
// is needed (and a separate one would conflict with the `String` impl).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_unquoted() {
        assert_eq!("hi".to_string().ipe_show(), "hi");
    }
    #[test]
    fn str_unquoted() {
        assert_eq!("hi".ipe_show(), "hi");
    }
    #[test]
    fn empty_string() {
        assert_eq!(String::new().ipe_show(), "");
    }
    #[test]
    fn int_plain() {
        assert_eq!(42i64.ipe_show(), "42");
    }
    #[test]
    fn bool_plain() {
        assert_eq!(true.ipe_show(), "true");
    }
    #[test]
    fn float_plain() {
        assert_eq!(42.5f64.ipe_show(), "42.5");
    }
    #[test]
    fn float_whole() {
        assert_eq!(1.0f64.ipe_show(), "1");
    }

    #[test]
    fn float_go_v_parity() {
        // Byte-for-byte parity with Go `fmt %v` == `strconv.FormatFloat(f,'g',-1,64)`.
        // Oracle: Go 1.26.2 `go run probe.go` (see reference-audit.md item 27). The cut
        // to scientific notation is a FLAT decimal-exponent >= 6 (and < -4), NOT 21.
        // Positional class (exp in [-4, 6)):
        assert_eq!(99999.0f64.ipe_show(), "99999"); // exp 4
        assert_eq!(1e5f64.ipe_show(), "100000"); // exp 5
        assert_eq!(999_999.0_f64.ipe_show(), "999999"); // exp 5 (lower guard)
        assert_eq!(123_456.789_f64.ipe_show(), "123456.789");
        assert_eq!(0.0001f64.ipe_show(), "0.0001"); // exp -4 boundary
        // Scientific class (exp >= 6) — these DISCRIMINATE 6 from 21:
        assert_eq!(1e6f64.ipe_show(), "1e+06"); // exp 6 — 21 would print "1000000"
        assert_eq!(1_000_001.0_f64.ipe_show(), "1.000001e+06"); // not a 1e6 special-case
        assert_eq!(1_234_567.0_f64.ipe_show(), "1.234567e+06");
        assert_eq!(1e15f64.ipe_show(), "1e+15"); // 21 would print 16 zeros
        assert_eq!(1e20f64.ipe_show(), "1e+20"); // 21 would print 21 digits
        assert_eq!(1e21f64.ipe_show(), "1e+21");
        // Scientific class (exp <= -5):
        assert_eq!(1e-5f64.ipe_show(), "1e-05");
        // Negative zero (shared positional branch): Go true -0.0 -> "-0".
        assert_eq!((-0.0f64).ipe_show(), "-0");
        // Non-finite (shared branch):
        assert_eq!(f64::INFINITY.ipe_show(), "+Inf");
        assert_eq!(f64::NEG_INFINITY.ipe_show(), "-Inf");
        assert_eq!(f64::NAN.ipe_show(), "NaN");
        assert_eq!((-1.5f64).ipe_show(), "-1.5");
    }

    #[test]
    fn vec_int_space_separated() {
        assert_eq!(vec![1i64, 2, 3].ipe_show(), "[1 2 3]");
    }
    #[test]
    fn vec_string_unquoted() {
        assert_eq!(vec!["a".to_string(), "b".to_string()].ipe_show(), "[a b]");
    }
    #[test]
    fn vec_empty() {
        let v: Vec<i64> = vec![];
        assert_eq!(v.ipe_show(), "[]");
    }
    #[test]
    fn vec_nested() {
        assert_eq!(vec![vec![1i64, 2], vec![3, 4]].ipe_show(), "[[1 2] [3 4]]");
    }

    #[test]
    fn tuple2() {
        assert_eq!((1i64, "a".to_string()).ipe_show(), "{1 a}");
    }
    #[test]
    fn tuple3() {
        assert_eq!((1i64, "a".to_string(), true).ipe_show(), "{1 a true}");
    }

    #[test]
    fn map_sorted() {
        let mut m: HashMap<String, i64> = HashMap::new();
        m.insert("b".to_string(), 2);
        m.insert("a".to_string(), 1);
        m.insert("c".to_string(), 3);
        assert_eq!(m.ipe_show(), "map[a:1 b:2 c:3]");
    }

    #[test]
    fn maybe_just() {
        assert_eq!(IpeMaybe::Just(5i64).ipe_show(), "Just 5");
    }
    #[test]
    fn maybe_nothing() {
        let n: IpeMaybe<i64> = IpeMaybe::Nothing;
        assert_eq!(n.ipe_show(), "Nothing");
    }
    #[test]
    fn result_ok() {
        let r: IpeResult<String, i64> = IpeResult::Ok(7);
        assert_eq!(r.ipe_show(), "Ok 7");
    }
    #[test]
    fn result_err() {
        let r: IpeResult<String, i64> = IpeResult::Err("boom".to_string());
        assert_eq!(r.ipe_show(), "Err boom");
    }

    // ─── Autoref-specialization dispatch (total field rendering) ─────────────

    // (a) A `String` field renders UNQUOTED via the IpeStringify arm.
    #[test]
    fn dispatch_string_unquoted() {
        let s = "hi".to_string();
        assert_eq!(Wrap(&s).dispatch(), "hi");
    }

    // (b) A type that impls ONLY `Debug` (NOT IpeStringify) renders via the
    // Debug fallback — NO compile error (this is the whole point: total by
    // construction). Mirrors a runtime payload type like `http_stream::ChunkEvent`.
    #[derive(Debug)]
    #[allow(dead_code)] // read only via the derived Debug (the test's whole point)
    struct OnlyDebug {
        x: i64,
    }

    #[test]
    fn dispatch_debug_fallback() {
        let d = OnlyDebug { x: 42 };
        assert_eq!((&Wrap(&d)).dispatch(), "OnlyDebug { x: 42 }");
    }

    // (c) A generated-style struct whose impl renders fields via the dispatch:
    // its String field renders unquoted INSIDE the `{...}` Go-`%v` wrap.
    struct GenStruct {
        name: String,
        debug_only: OnlyDebug,
    }
    impl IpeStringify for GenStruct {
        fn ipe_show(&self) -> String {
            // Exactly what codegen now emits per field.
            format!(
                "{{{} {}}}",
                Wrap(&self.name).dispatch(),
                (&Wrap(&self.debug_only)).dispatch()
            )
        }
    }

    #[test]
    fn dispatch_generated_struct_mixed_fields() {
        let g = GenStruct {
            name: "alice".to_string(),
            debug_only: OnlyDebug { x: 7 },
        };
        // String field unquoted; Debug-only field via fallback — never E0599.
        assert_eq!(g.ipe_show(), "{alice OnlyDebug { x: 7 }}");
    }
}
