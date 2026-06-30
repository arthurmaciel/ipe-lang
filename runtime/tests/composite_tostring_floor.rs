//! Floor-lock for the `composite-tostring` divergence (disposition: DOCUMENT_BLOCKED).
//!
//! Spec: runtime-rust/docs/superpowers/specs/2026-06-15-composite-tostring-design.md
//!
//! ## What this fixture is for
//!
//! `Basics.toString` / `Debug.toString` (the `{{interp}}` stringifier) are
//! `Display`-bound in the Rust runtime (`src/sky_runtime/basics.rs`):
//!
//! ```ignore
//! pub fn basics_to_string<T: std::fmt::Display>(v: T) -> String { format!("{}", v) }
//! pub fn debug_to_string<T:  std::fmt::Display>(v: T) -> String { format!("{}", v) }
//! ```
//!
//! For SCALARS, `Display` == Go's `fmt.Sprintf("%v", …)` byte-for-byte, so the
//! Rust backend already matches the Go reference backend exactly. This file PINS
//! that scalar floor so a future refactor (e.g. switching `toString` to `Debug`,
//! which would quote strings and add field names) can't silently regress it.
//!
//! It deliberately does NOT assert that a record/ADT renders like Go — that is
//! precisely the BLOCKED shape (see "Why composite Display is intentionally
//! absent" below). Asserting it would mean fabricating Go's reflected struct
//! layout in Rust; the design decision is to leave composites as a clean
//! compile-time error instead.
//!
//! ## Go `%v` reference table (the parity oracle — empirically captured)
//!
//! Sky values lower to Go as: record → anonymous struct sorted by `_fieldIndex`;
//! ADT → a SINGLE flattened struct (`Tag int` + every variant's payload fields).
//! `Debug_toString` deref's pointers, then `Sprintf("%v", v)`:
//!
//! | Sky shape                        | Go `%v` output | Notes                                                  |
//! |----------------------------------|----------------|--------------------------------------------------------|
//! | scalar Int `5`                   | `5`            | matched here                                           |
//! | scalar Float `42.5`              | `42.5`         | matched here                                           |
//! | scalar Bool `true`               | `true`         | matched here                                           |
//! | scalar String `"hi"`             | `hi`           | UNQUOTED / identity — matched here                     |
//! | record `{ x = 1, y = 2, z = 3 }` | `{1 2 3}`      | brace-wrapped, space-joined VALUES, `_fieldIndex` order, no field names, no type name |
//! | nested record-in-record          | `{{2 z} 9}`    | inner struct recursively `%v`'d                        |
//! | nullary ADT variant              | `{0 0}`        | leaks Go's `Tag` int + zero-valued payload slots of OTHER variants |
//! | payload ADT variant `Foo 42`     | `{1 42}`       | `{tag payload…}` — constructor NAME never appears      |
//! | List `[1, 2, 3]`                 | `[1 2 3]`      | space-joined, square brackets                          |
//! | Dict / map                       | `map[a:1 b:2]` | `map[k:v …]`, Go-sorted keys                            |
//! | tuple `(1, "q")`                 | `{1 q}`        | identical to a 2-field struct                          |
//! | `nil` pointer / Nothing          | `<nil>`        |                                                        |
//! | function-typed field             | `0x<addr>`     | NON-DETERMINISTIC process address — unmatchable        |
//!
//! ## Why composite `Display` is intentionally absent (DOCUMENT_BLOCKED)
//!
//! 1. **No runtime panic — clean compile-time error.** A composite (record/ADT)
//!    has no `Display` impl, so `basics_to_string(record)` fails at COMPILE time
//!    with `E0277` (trait bound not satisfied), never at runtime. That is MORE in
//!    line with "no runtime errors from well-typed Sky code" than Go's runtime
//!    reflection: Rust catches it before a binary exists.
//!
//! 2. **The ADT shape is unmatchable in Rust's type system.** Go's `{1 42}` /
//!    `{0 0}` leaks a flattened-struct memory layout (Tag int + zero-init
//!    inactive-variant fields). Rust enums are sum types — there is no Tag-int +
//!    zero-init-inactive-fields value to render from. Reproducing the string
//!    would mean FABRICATING Go's layout (symptom-masking, not a meaning).
//!
//! 3. **The function-field subset is non-deterministic by construction.** Go
//!    renders a func-typed field as `0x<addr>` — a process address that changes
//!    per run (same class already reclassified out of equiv for
//!    `35-composite-generics`). Rust cannot reproduce a Go func address.
//!
//! 4. **Unverifiable here.** Zero upstream `examples/` interpolate or `toString`
//!    a record/ADT into stdout (every `{{…}}` site is a pre-stringified scalar),
//!    `examples/` is read-only, and the equiv-sweep can't diff a shape no example
//!    exercises. We never ship what we cannot verify.
//!
//! If an upstream example later interpolates a Sky RECORD (not ADT) into stdout,
//! the record-only sub-case becomes a verifiable, in-boundary IMPLEMENT and
//! should be promoted then (see spec §"Disposition rationale"). The ADT shape
//! stays blocked until Go stops leaking its struct layout.

use sky_runtime_rust::sky_runtime::basics::{basics_to_string, debug_to_string};

// --- Positive floor: scalars match Go `%v` byte-for-byte via the Display path. ---

#[test]
fn to_string_int_matches_go_percent_v() {
    // Go: fmt.Sprintf("%v", int64(5)) == "5"
    assert_eq!(basics_to_string(5i64), "5");
}

#[test]
fn to_string_float_matches_go_percent_v() {
    // Go: fmt.Sprintf("%v", 42.5) == "42.5"
    assert_eq!(basics_to_string(42.5f64), "42.5");
}

#[test]
fn to_string_bool_true_matches_go_percent_v() {
    // Go: fmt.Sprintf("%v", true) == "true"
    assert_eq!(basics_to_string(true), "true");
    assert_eq!(basics_to_string(false), "false");
}

#[test]
fn to_string_string_renders_unquoted_identity() {
    // Go: Debug_toString returns the String verbatim (no surrounding quotes),
    // and so must Rust's Display path — NOT Debug (which would yield "\"hi\"").
    assert_eq!(basics_to_string("hi".to_string()), "hi");
    assert_eq!(basics_to_string("hi"), "hi");
}

// --- debug_to_string (the `{{expr}}` interpolation entry) shares the floor. ---

#[test]
fn debug_to_string_scalars_match_go_percent_v() {
    assert_eq!(debug_to_string(5i64), "5");
    assert_eq!(debug_to_string(42.5f64), "42.5");
    assert_eq!(debug_to_string(true), "true");
    // String interpolates as itself — the load-bearing identity property: a
    // `{{name}}` site must splice the raw String, never a quoted form.
    assert_eq!(debug_to_string("hi".to_string()), "hi");
}

// --- Documentation-as-code: composites are intentionally a compile error. ---
//
// The following, if uncommented, MUST fail to compile with E0277 (no `Display`
// impl for a record/ADT/Vec composite) — never produce a binary, never panic.
// This is the BLOCKED shape; it is left as a comment because a `#[test]` cannot
// assert "does not compile" without a separate trybuild harness, and the design
// decision is precisely that no composite path exists to test.
//
//   #[derive(Clone)]
//   struct Point_R { x: i64, y: i64 }   // a lowered Sky record
//   let _ = basics_to_string(Point_R { x: 1, y: 2 });  // E0277: Point_R: !Display
//   let _ = basics_to_string(vec![1i64, 2, 3]);        // E0277: Vec<i64>: !Display
//
// Go would reflect these at runtime to `{1 2}` / `[1 2 3]`; Rust refuses them at
// compile time. That refusal IS the principled floor this fixture locks.
