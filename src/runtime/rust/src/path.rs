//! `Ipe.Path` — a typed, opaque filesystem path.
//!
//! The ONLY way to obtain a `Path` is through [`path_from_string`] (the
//! parse-don't-validate seal): it normalises the path lexically and REJECTS
//! the two byte-level primitives that make a raw `String` path a traversal /
//! injection surface:
//!
//! * a NUL byte (`\0`) — a C-string terminator that truncates the path at the
//!   syscall boundary, so `"safe.txt\0../../etc/passwd"` reaches the kernel as
//!   `"safe.txt"` on one code path and the full string on another (a classic
//!   poisoned-NUL bypass); and
//! * a traversal escape — a relative path whose `..` elements climb ABOVE the
//!   directory it is resolved against (cleaned form is `..` or begins `../`).
//!   A rooted path cannot escape (`Clean` already stops `..` at `/`), so it is
//!   allowed; a relative path that stays at or below its base is allowed.
//!
//! Because every `Path` is validated at construction, the pure helpers
//! ([`path_base`] / [`path_dir`] / [`path_ext`] / [`path_is_absolute`]) and the
//! `Ipe.File` kernels take a `Path` and never re-validate — the type is the
//! proof. [`path_to_string`] is the single un-parse back to the raw `String`.
//!
//! The lexical engine (`clean`) is a faithful port of Go `path/filepath`
//! (Unix, separator `/`) rather than a wrapper over `std::path`, which is
//! OS-tagged and diverges from Go on trailing slashes, repeated separators, and
//! dotfiles. The Rust backend's equivalence target runs on Linux, so Unix
//! `filepath` semantics are implemented exactly.

use super::IpeResult;

const SEP: u8 = b'/';

/// `Ipe.Path`'s opaque, validated newtype. See the module doc for the
/// construction contract. The wrapped `String` is always the lexically-cleaned,
/// NUL-free, non-escaping form produced by [`path_from_string`].
///
/// `Clone` is derived (a `Path` may be stored and passed to more than one
/// kernel). `Debug` / `PartialEq` / `Eq` are derived and safe: a `Path` is not
/// a secret, so printing or comparing the cleaned string leaks nothing the
/// caller did not already hand in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path(String);

impl super::stringify::IpeStringify for Path {
    /// Backs Ipê's `toString` / interpolation on a `Path`: the cleaned path
    /// string. Identical to [`path_to_string`].
    fn ipe_show(&self) -> String {
        self.0.clone()
    }
}

/// `Ipe.Path.fromString : String -> Result Error Path` — THE seal. The only
/// public constructor: every `Path` value in a Ipê program traces back to one
/// of these calls, so a reviewer can `grep` this one symbol to audit every
/// place a raw string becomes a typed path.
///
/// Fails closed (`Err`) on a NUL byte or a traversal escape; succeeds with the
/// lexically-cleaned form otherwise. The empty string cleans to `"."` (the
/// current directory), matching Go `filepath.Clean("")`.
#[must_use]
pub fn path_from_string<E: From<String>>(s: String) -> IpeResult<E, Path> {
    if s.as_bytes().contains(&0) {
        return IpeResult::Err(
            "Ipe.Path: path contains a NUL byte (a syscall-boundary truncation / traversal risk)"
                .to_string()
                .into(),
        );
    }
    let cleaned = clean(&s);
    if escapes_root(&cleaned) {
        return IpeResult::Err(
            format!(
                "Ipe.Path: path escapes its root via `..` traversal: {s:?} (cleaned: {cleaned:?})"
            )
            .into(),
        );
    }
    IpeResult::Ok(Path(cleaned))
}

/// `Ipe.Path.toString : Path -> String` — THE single un-parse: recover the
/// cleaned path string. Consumes the `Path` (the typed proof is spent when the
/// raw string comes back out).
#[must_use]
pub fn path_to_string(p: Path) -> String {
    p.0
}

/// Borrow the cleaned path string. For the `Ipe.File` kernel boundary, which
/// needs the `&str` to hand to `std::fs`.
impl Path {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume into the owned cleaned string (for kernels that need `String`).
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

/// Does a CLEANED, relative path climb above its base? True when the whole
/// path is `..` or it begins with `../` — the two shapes `clean` leaves when a
/// relative path's leading `..`s could not be resolved away. A rooted path
/// (begins `/`) can never escape: `clean` stops `..` at the root.
fn escapes_root(cleaned: &str) -> bool {
    cleaned == ".." || cleaned.starts_with("../")
}

/// Faithful port of Go `path/filepath.Clean` (Unix). Lexically simplifies a
/// path: collapses repeated `/`, resolves `.`/`..` elements, drops a trailing
/// `/` (except root). Pure byte work — multi-byte UTF-8 path elements are copied
/// intact (their bytes are never `/` or ASCII `.`), so the result is valid
/// UTF-8.
fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let b = path.as_bytes();
    let n = b.len();
    // Total byte access (no `[]` indexing — clippy::indexing_slicing / no-panic
    // gate). Out-of-range reads as `None`, never panics.
    let at = |i: usize| -> Option<u8> { b.get(i).copied() };
    let rooted = at(0) == Some(SEP);
    let mut out: Vec<u8> = Vec::with_capacity(n + 1);
    let mut r = 0usize;
    // `dotdot` is the index in `out` past which leading `..`s have been written
    // (for a relative path) or past the root `/` — popping never crosses it.
    let mut dotdot = 0usize;
    if rooted {
        out.push(SEP);
        r = 1;
        dotdot = 1;
    }
    while r < n {
        if at(r) == Some(SEP) {
            // empty path element → skip
            r += 1;
        } else if at(r) == Some(b'.') && (r + 1 == n || at(r + 1) == Some(SEP)) {
            // `.` element → skip
            r += 1;
        } else if at(r) == Some(b'.')
            && at(r + 1) == Some(b'.')
            && (r + 2 == n || at(r + 2) == Some(SEP))
        {
            // `..` element → back up
            r += 2;
            if out.len() > dotdot {
                // pop the last element
                let mut w = out.len() - 1;
                while w > dotdot && out.get(w).copied() != Some(SEP) {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                // cannot back up → keep the `..`
                if !out.is_empty() {
                    out.push(SEP);
                }
                out.push(b'.');
                out.push(b'.');
                dotdot = out.len();
            }
        } else {
            // real path element → append a separator (if needed) then the element
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(SEP);
            }
            while r < n && at(r) != Some(SEP) {
                if let Some(c) = at(r) {
                    out.push(c);
                }
                r += 1;
            }
        }
    }
    if out.is_empty() {
        return ".".to_string();
    }
    String::from_utf8(out).unwrap_or_else(|_| ".".to_string())
}

/// `Ipe.Path.base : Path -> String` — Go `filepath.Base` (Unix).
/// "" → "."; all-slashes → "/"; else the final element with trailing slashes
/// stripped.
#[must_use]
pub fn path_base(p: Path) -> String {
    let path = p.0;
    if path.is_empty() {
        return ".".to_string();
    }
    // strip trailing separators
    let b = path.as_bytes();
    let mut end = b.len();
    while end > 0 && b.get(end - 1).copied() == Some(SEP) {
        end -= 1;
    }
    if end == 0 {
        // path was all separators
        return "/".to_string();
    }
    let stripped = path.get(..end).unwrap_or(&path);
    let sb = stripped.as_bytes();
    // find the last separator
    let mut i = sb.len();
    while i > 0 && sb.get(i - 1).copied() != Some(SEP) {
        i -= 1;
    }
    stripped.get(i..).unwrap_or("").to_string()
}

/// `Ipe.Path.dir : Path -> String` — Go `filepath.Dir` (Unix).
/// All but the last element, then `Clean`ed: "" / "foo" → "."; "/" → "/";
/// "/foo/bar" → "/foo"; "/foo/" → "/foo"; "a//b" → "a".
#[must_use]
pub fn path_dir(p: Path) -> String {
    let path = p.0;
    let b = path.as_bytes();
    let mut i = b.len();
    while i > 0 && b.get(i - 1).copied() != Some(SEP) {
        i -= 1;
    }
    // path[..i] is everything up to and including the last separator (or "" when
    // there is none). Clean("") = ".".
    clean(path.get(..i).unwrap_or(""))
}

/// `Ipe.Path.ext : Path -> String` — Go `filepath.Ext` (Unix).
/// The suffix from the LAST `.` in the final path element (including the dot),
/// or "" when the final element has no dot. `filepath.Ext(".bashrc")` → ".bashrc".
#[must_use]
pub fn path_ext(p: Path) -> String {
    let path = p.0;
    let b = path.as_bytes();
    let mut i = b.len();
    while i > 0 {
        match b.get(i - 1).copied() {
            Some(c) if c == SEP => break,
            Some(b'.') => return path.get(i - 1..).unwrap_or("").to_string(),
            _ => {}
        }
        i -= 1;
    }
    String::new()
}

/// `Ipe.Path.isAbsolute : Path -> Bool` — Go `filepath.IsAbs` (Unix):
/// an absolute path begins with `/`.
#[must_use]
pub fn path_is_absolute(p: Path) -> bool {
    p.0.as_bytes().first() == Some(&SEP)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(s: &str) -> Path {
        match path_from_string::<String>(s.to_string()) {
            IpeResult::Ok(p) => p,
            IpeResult::Err(e) => panic!("expected {s:?} to be a valid Path, got Err: {e}"),
        }
    }

    // ── construction: the seal validates ────────────────────────────────────

    #[test]
    fn empty_cleans_to_dot() {
        assert_eq!(path_to_string(mk("")), ".");
    }

    #[test]
    fn plain_relative_is_accepted() {
        assert_eq!(path_to_string(mk("src/Main.ipe")), "src/Main.ipe");
    }

    #[test]
    fn repeated_separators_collapse() {
        assert_eq!(path_to_string(mk("a//b///c")), "a/b/c");
    }

    #[test]
    fn interior_dotdot_that_stays_in_bounds_is_accepted() {
        // "a/b/../c" resolves to "a/c" — never climbs above the base.
        assert_eq!(path_to_string(mk("a/b/../c")), "a/c");
    }

    #[test]
    fn rooted_dotdot_cannot_escape_and_is_accepted() {
        // Go `Clean` stops `..` at the root, so a rooted path is always safe.
        assert_eq!(path_to_string(mk("/a/../../b")), "/b");
    }

    // ── construction: the seal rejects ──────────────────────────────────────

    #[test]
    fn nul_byte_is_rejected() {
        let r: IpeResult<String, Path> = path_from_string("safe.txt\0../../etc/passwd".to_string());
        assert!(matches!(r, IpeResult::Err(_)), "a NUL byte must be rejected");
    }

    #[test]
    fn leading_dotdot_escape_is_rejected() {
        let r: IpeResult<String, Path> = path_from_string("../secret".to_string());
        assert!(
            matches!(r, IpeResult::Err(_)),
            "a relative path that climbs above its base must be rejected"
        );
    }

    #[test]
    fn dotdot_that_resolves_to_escape_is_rejected() {
        // "a/../../etc" cleans to "../etc" — escapes the base.
        let r: IpeResult<String, Path> = path_from_string("a/../../etc".to_string());
        assert!(
            matches!(r, IpeResult::Err(_)),
            "a path whose cleaned form escapes the base must be rejected"
        );
    }

    #[test]
    fn bare_dotdot_is_rejected() {
        let r: IpeResult<String, Path> = path_from_string("..".to_string());
        assert!(matches!(r, IpeResult::Err(_)), "bare `..` escapes the base");
    }

    // ── pure helpers over a validated Path ──────────────────────────────────

    #[test]
    fn base_filename() {
        assert_eq!(path_base(mk("/foo/bar.txt")), "bar.txt");
    }

    #[test]
    fn base_root() {
        assert_eq!(path_base(mk("/")), "/");
    }

    #[test]
    fn dir_with_parent() {
        assert_eq!(path_dir(mk("/foo/bar.txt")), "/foo");
    }

    #[test]
    fn dir_bare_name() {
        assert_eq!(path_dir(mk("hello.ipe")), ".");
    }

    #[test]
    fn ext_present() {
        assert_eq!(path_ext(mk("/foo/bar.txt")), ".txt");
    }

    #[test]
    fn ext_dotfile() {
        assert_eq!(path_ext(mk(".bashrc")), ".bashrc");
    }

    #[test]
    fn ext_multiple_dots() {
        assert_eq!(path_ext(mk("a.b.c")), ".c");
    }

    #[test]
    fn is_absolute_true() {
        assert!(path_is_absolute(mk("/usr/bin")));
    }

    #[test]
    fn is_absolute_false() {
        assert!(!path_is_absolute(mk("relative/path")));
    }
}
