// The shared HOME-parsing agreement table.
//
// One `(raw, expected)` row list, `include!`d verbatim by the tests of both
// `ipe_runtime_rust::system` and `ipe_sandbox::home`, so both home readers are
// driven through identical rows on top of the one shared parser in
// `src/home_core.rs`. `raw` and `expected` are UTF-8 text; each crate pins its
// own non-UTF-8 refusal beside this table.
//
// Regular (`//`) comments, not inner docs (`//!`): this file is `include!`d
// after other items, where an inner doc is an illegal mid-file attribute.

/// Platform-independent `(raw env value, expected home)` rows.
///
/// `None` on the left is unset; `None` on the right names no directory.
const HOME_PARSE_CASES: &[(Option<&str>, Option<&str>)] = &[
    (None, None),
    (Some(""), None),
    (Some("."), None),
    (Some("~"), None),
    (Some("relative/x"), None),
    (Some("home/u"), None),
    (Some("./home"), None),
    (Some("../home"), None),
];

/// Unix `(raw env value, expected home)` rows.
///
/// An absolute value is kept verbatim, `..` and trailing separators included.
#[cfg(not(windows))]
const HOME_PARSE_PLATFORM_CASES: &[(Option<&str>, Option<&str>)] = &[
    (Some("/home/u"), Some("/home/u")),
    (Some("/home/../home"), Some("/home/../home")),
    (Some("/home/u/"), Some("/home/u/")),
    (Some(" /home/u"), None),
    (Some(r"C:\Users\u"), None),
];

/// Windows `(raw env value, expected home)` rows.
///
/// Root-relative and drive-relative values are not absolute; a UNC path is.
#[cfg(windows)]
const HOME_PARSE_PLATFORM_CASES: &[(Option<&str>, Option<&str>)] = &[
    (Some(r"C:\Users\u"), Some(r"C:\Users\u")),
    (Some(r"\\srv\share\u"), Some(r"\\srv\share\u")),
    (Some(r"\Users\u"), None),
    (Some(r"C:Users\u"), None),
    (Some("/home/u"), None),
    // Verbatim / device-namespace prefixes: `Path::is_absolute` accepts them,
    // but they name a raw device or an unparsed literal path, not a directory.
    (Some(r"\\?\C:\Users\u"), None),
    (Some(r"\\.\pipe\x"), None),
    (Some(r"\\?\UNC\srv\s"), None),
];
