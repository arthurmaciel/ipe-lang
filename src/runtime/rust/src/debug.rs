//! `Ipe.Debug` — the development-only escape hatch.
//!
//! `Debug.log label value` prints `"<label>: <value>"` to stderr as a side
//! effect and returns `value` UNCHANGED, so it can be spliced into any
//! expression without altering its result. The value is stringified through the
//! same total `IpeStringify` path `Basics.toString` / `{{expr}}` interpolation
//! uses, so any Ipê-representable value renders (a `String` unquoted, scalars
//! like Go's `%v`, records/ADTs via their codegen-emitted impl).
//!
//! This is the ONE deliberate impure escape hatch in the language — NOT a
//! `Task`. A PRODUCTION build (`ipe build --optimize`) rejects any `Debug.*`
//! use at compile time (IPE-L0140), so this function is only ever reached from
//! a development build.

use crate::stringify::IpeStringify;

/// `Debug.log : String -> a -> a`. Writes `"<label>: <value>"` + a newline to
/// stderr (fallibly, dropping a broken-pipe error rather than panicking — the
/// same discipline `log.rs`'s line writers use), then returns `value`
/// unchanged.
#[must_use]
pub fn debug_log<T: IpeStringify>(label: String, value: T) -> T {
    use std::io::Write as _;
    let line = format!("{label}: {}", value.ipe_show());
    // A closed downstream pipe surfaces as an `EPIPE` write error (Rust ignores
    // SIGPIPE by default); drop it so a well-typed `Debug.log` never aborts.
    let _ = writeln!(std::io::stderr().lock(), "{line}");
    value
}
