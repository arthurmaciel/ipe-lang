//! Floor-lock for `Basics.toString` / `Debug.toString` — the whole `%v` surface.
//!
//! `Basics.toString` and `Debug.toString` (the `{{interp}}` stringifier) route
//! through the total `IpeStringify` trait, the SAME path as
//! `Basics.errorToString`:
//!
//! ```ignore
//! pub fn basics_to_string<T: IpeStringify>(v: T) -> String { v.ipe_show() }
//! pub fn debug_to_string<T:  IpeStringify>(v: T) -> String { v.ipe_show() }
//! ```
//!
//! `IpeStringify` renders every value totally — every scalar and
//! every composite (record / ADT / list / map). This file pins both halves:
//!
//! 1. SCALARS keep their exact bytes (a refactor to `Debug` would quote
//!    strings and rename this a regression).
//! 2. COMPOSITES stringify correctly — no `Display` bound, so there is no
//!    exit-0-then-cargo-fail hole (a composite has no `Display` impl; it DOES
//!    have an `IpeStringify` impl, runtime-provided here and codegen-provided for
//!    every emitted record/ADT).
//!
//! ## String rendering reference (empirically captured)
//!
//! | value                            | rendered      | Notes                                            |
//! |----------------------------------|----------------|--------------------------------------------------|
//! | scalar Int `5`                   | `5`            |                                                  |
//! | scalar Float `42.5`              | `42.5`         |                                                  |
//! | scalar Bool `true`               | `true`         |                                                  |
//! | scalar String `"hi"`             | `hi`           | UNQUOTED / identity                              |
//! | record `{ x = 1, y = 2 }`        | `{1 2}`        | brace-wrapped, space-joined, `_fieldIndex` order |
//! | tuple `(1, "q")`                 | `{1 q}`        | identical to a 2-field struct                    |
//! | List `[1, 2, 3]`                 | `[1 2 3]`      | space-joined, square brackets                    |
//! | map `{ a: 1, b: 2 }`             | `map[a:1 b:2]` | alphabetically sorted                                   |
//!
//! A codegen-emitted ADT renders `Vname f0 f1 …` (variant name, space-joined
//! fields) — the `../ipe` Rust backend's `IpeStringify` enum shape — verified by
//! the `m_tostring_composite` golden's end-to-end output (`Circle 5` / `Empty`),
//! not here (this file tests the runtime primitives, not codegen).
//!
//! The one residual: a bare function-typed value has no meaningful string repr (
//! prints a non-deterministic address); `toString` on a function is rejected at
//! ipe type-check (the Stringify obligation's `Fun` head-rejection — see
//! `m_tostring_fn_rejected`), so it never reaches this runtime path.

use ipe_runtime_rust::basics::{basics_to_string, debug_to_string};
use std::collections::HashMap;

// --- Scalars ---

#[test]
fn to_string_int_matches_go_percent_v() {
    fmt.Sprintf("%v", int64(5)) == "5"
    assert_eq!(basics_to_string(5i64), "5");
}

#[test]
fn to_string_float_matches_go_percent_v() {
    "42.5"
    assert_eq!(basics_to_string(42.5f64), "42.5");
    // Go `%v` == strconv.FormatFloat(f,'g',-1,64): cuts to scientific at exp >= 6
    // and for infinities/NaN. The `IpeStringify` f64 impl reproduces this; the
    // former `Display` path did NOT (it printed "1000000" / "inf").
    assert_eq!(basics_to_string(1e6f64), "1e+06");
    assert_eq!(basics_to_string(f64::INFINITY), "+Inf");
}

#[test]
fn to_string_bool_true_matches_go_percent_v() {
    "true"
    assert_eq!(basics_to_string(true), "true");
    assert_eq!(basics_to_string(false), "false");
}

#[test]
fn to_string_string_renders_unquoted_identity() {
    String returns verbatim (no surrounding quotes) — NOT Debug (which
    // would yield "\"hi\"").
    assert_eq!(basics_to_string("hi".to_string()), "hi");
    assert_eq!(basics_to_string("hi"), "hi");
}

// --- debug_to_string (the `{{expr}}` interpolation entry) shares the path. ---

#[test]
fn debug_to_string_scalars_match_go_percent_v() {
    assert_eq!(debug_to_string(5i64), "5");
    assert_eq!(debug_to_string(42.5f64), "42.5");
    assert_eq!(debug_to_string(true), "true");
    // String interpolates as itself — the load-bearing identity property: a
    // `{{name}}` site must splice the raw String, never a quoted form.
    assert_eq!(debug_to_string("hi".to_string()), "hi");
}

// --- Composites now stringify correctly (no exit-0-then-cargo-fail hole). ---

#[test]
fn to_string_list_matches_go_percent_v() {
    "[1 2 3]"
    assert_eq!(basics_to_string(vec![1i64, 2, 3]), "[1 2 3]");
}

#[test]
fn to_string_tuple_matches_go_percent_v() {
    Ipê tuple lowers to a struct — `%v` is `{a b}`.
    assert_eq!(basics_to_string((1i64, "q".to_string())), "{1 q}");
}

#[test]
fn to_string_map_matches_go_percent_v() {
    "map[a:1 b:2]" (keys sorted).
    let mut m: HashMap<String, i64> = HashMap::new();
    m.insert("b".to_string(), 2);
    m.insert("a".to_string(), 1);
    assert_eq!(basics_to_string(m), "map[a:1 b:2]");
}
