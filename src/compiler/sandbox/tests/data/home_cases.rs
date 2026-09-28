//! Shared HOME-parsing agreement table.
//!
//! One `(raw, expected)` row list, `include!`d verbatim by both
//! `ipe_sandbox::home`'s tests and `ipe_runtime_rust::system`'s tests, so the
//! runtime's and the sandbox's independent `home_dir_from` parsers are driven
//! through the identical rows. A row on which they disagree fails wherever
//! this file is included, instead of drifting unnoticed as two hand-copied
//! lists. `raw`/`expected` are UTF-8 text; each crate additionally pins its
//! own non-UTF-8 refusal alongside this table (a raw value neither parser's
//! `String`/UTF-8-checked type can represent).
#[cfg(not(windows))]
const HOME_PARSE_ABSOLUTE_RAW: &str = "/home/u";
#[cfg(windows)]
const HOME_PARSE_ABSOLUTE_RAW: &str = r"C:\Users\u";

/// `(raw env value, expected resolved home)`; `None` on the left is unset,
/// `None` on the right is "names no directory".
const HOME_PARSE_CASES: &[(Option<&str>, Option<&str>)] = &[
    (None, None),
    (Some(""), None),
    (Some("."), None),
    (Some("~"), None),
    (Some("relative/x"), None),
    (Some("home/u"), None),
    (Some("./home"), None),
    (Some("../home"), None),
    (Some(HOME_PARSE_ABSOLUTE_RAW), Some(HOME_PARSE_ABSOLUTE_RAW)),
];
