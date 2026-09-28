//! `Ipe.Debug` — the development-only escape hatch.
//!
//! `Debug.log label value` prints `"<label>: <value>"` to stderr as a side
//! effect and returns `value` UNCHANGED, so it can be spliced into any
//! expression without altering its result. The value is stringified through the
//! same total `IpeStringify` path `{{expr}}` interpolation uses, so any
//! Ipê-representable value renders (a `String` unquoted, scalars like `%v`,
//! records/ADTs via their codegen-emitted impl).
//!
//! This is the ONE deliberate impure escape hatch in the language — NOT a
//! `Task`. `ipe release` rejects any `Debug.*` use at compile time (IPE-L0140),
//! so this function is only ever reached from a development build.

use crate::stringify::IpeStringify;

/// `Debug.log : String -> a -> a`. Writes `"<label>: <value>"` + a newline to
/// stderr (fallibly, through `system::write_stderr_line`, so a broken pipe
/// never panics), then returns `value`
/// unchanged.
#[must_use]
pub fn debug_log<T: IpeStringify>(label: String, value: T) -> T {
    crate::system::write_stderr_line(&format!("{label}: {}", value.ipe_show()));
    value
}

/// `Debug.todo : String -> a`. Prints `"TODO at <file>:<line>: <note>"` to
/// stderr then exits with a non-zero code.  Returns `!` (the never type),
/// which coerces to any `A` at the call site — no Rust `panic!` is used.
///
/// `location` is a `"<file>:<line>"` string injected by the lowerer at
/// compile time from the call-site source span; it is never computed at
/// runtime.  `note` is the developer-supplied string argument.
pub fn debug_todo<A>(location: String, note: String) -> A {
    crate::system::write_stderr_line(&format!("TODO at {location}: {note}"));
    crate::system::system_exit(1)
}
