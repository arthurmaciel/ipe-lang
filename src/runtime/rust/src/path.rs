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
//! rather than a wrapper over `std::path`, which is OS-tagged and diverges from
//! Go on trailing slashes, repeated separators, and dotfiles. The Rust
//! backend's equivalence target runs on Linux, so Unix `filepath` semantics are
//! implemented exactly there; on Windows the same engine is driven with the
//! Windows separator set (`\` and `/`) and volume-prefix parsing so the
//! traversal check is not `\`-bypassable (see [`clean_with`]).
//!
//! # Trust model — what `Path` does and does NOT guarantee
//!
//! `Path` is a LEXICAL guard, not a jail. It guarantees the string contains no
//! NUL byte and does not `..`-escape *lexically*. It deliberately does NOT:
//! * forbid ABSOLUTE paths — `/etc/passwd` is a valid `Path`. Confining a
//!   program to a subtree is the job of the runtime capability jail (whether a
//!   program may touch the filesystem AT ALL is the `Filesystem` capability),
//!   not of this lexical constructor.
//! * resolve or forbid SYMLINKS — a validated `Path` may still point through a
//!   symlink that leaves any intended root. Symlink containment is an OS/jail
//!   concern (`openat2(RESOLVE_BENEATH)` / a chroot), out of lexical scope.
//!
//! In short: `Path` closes the raw-string traversal/NUL-injection hole at the
//! type boundary; it is not a substitute for the capability jail's authority
//! decision about which paths a program is allowed to reach.

use super::IpeResult;

// The platform separator set the lexical engine treats as element boundaries.
// Unix: `/` alone. Windows: BOTH `\` and `/` — Windows accepts either at the
// syscall boundary, so a validator that honoured only one would let the other
// carry an unchecked `..` traversal (`..\..\x`) straight past the `..`-element
// scan. `WINDOWS` also switches on volume-prefix parsing (drive letters, UNC).
#[cfg(not(windows))]
const WINDOWS: bool = false;
#[cfg(windows)]
const WINDOWS: bool = true;

/// The canonical separator emitted in a cleaned path (all input separators
/// normalise to this): `/` on Unix, `\` on Windows, matching Go `filepath`.
const SEP: u8 = if WINDOWS { b'\\' } else { b'/' };

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
/// Fails closed (`Err`) on a NUL byte, a Windows trailing-dot/space traversal
/// disguise, or a `..` escape; succeeds with the lexically-cleaned form
/// otherwise. The empty string cleans to `"."` (the current directory),
/// matching Go `filepath.Clean("")`.
#[must_use]
pub fn path_from_string<E: From<String>>(s: String) -> IpeResult<E, Path> {
    if s.as_bytes().contains(&0) {
        return IpeResult::Err(
            "Ipe.Path: path contains a NUL byte (a syscall-boundary truncation / traversal risk)"
                .to_string()
                .into(),
        );
    }
    if WINDOWS && has_disguised_dotdot(&s) {
        // Windows strips trailing dots and spaces from every path element at the
        // syscall, so `".. "` and `"..."` name the parent directory even though
        // the lexical scan sees a literal filename. Reject before `clean` so the
        // disguise can never resolve into a traversal we failed to count.
        return IpeResult::Err(
            format!(
                "Ipe.Path: path element resolves to `..` after Windows trailing dot/space \
                 stripping (a traversal disguise): {s:?}"
            )
            .into(),
        );
    }
    let cleaned = clean(&s);
    if escapes_root(&cleaned, WINDOWS) {
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

/// Is byte `c` an element separator under the active separator set? Unix honours
/// only `/`; Windows ALSO honours `\`, because Windows accepts either at a
/// syscall — so both must count, or the un-honoured one smuggles a `..` past the
/// traversal scan.
fn is_sep(c: u8, windows: bool) -> bool {
    c == b'/' || (windows && c == b'\\')
}

/// Length in bytes of the leading VOLUME name of `path` under Windows rules
/// (`0` on Unix, where no path element is ever consumed as a volume). Ported
/// from Go `path/filepath.volumeNameLen`. Recognised prefixes:
/// * `\\?\…` / `\\.\…` — verbatim / device namespaces (consume up to the next
///   separator after the namespace tag);
/// * `\\server\share` — a UNC root (both the server and the share component);
/// * `C:` — a drive designator (two bytes).
///
/// The volume is copied through `clean` untouched and is the floor the `..`
/// scan can never pop below — so `..` can neither delete a drive letter nor
/// climb out of a UNC share.
fn volume_name_len(path: &str, windows: bool) -> usize {
    if !windows {
        return 0;
    }
    let b = path.as_bytes();
    let n = b.len();
    let at = |i: usize| -> Option<u8> { b.get(i).copied() };
    // `C:` drive designator: an ASCII letter followed by a colon.
    if at(0).is_some_and(|c| c.is_ascii_alphabetic()) && at(1) == Some(b':') {
        return 2;
    }
    // UNC / device / verbatim: `\\` or `//` (any mix) followed by a component.
    if n >= 2
        && at(0).is_some_and(|c| is_sep(c, windows))
        && at(1).is_some_and(|c| is_sep(c, windows))
    {
        // Skip the first component (server, or `?`/`.` namespace tag).
        let mut i = 2usize;
        while i < n && !at(i).is_some_and(|c| is_sep(c, windows)) {
            i += 1;
        }
        if i == n {
            // `\\server` with no trailing separator — whole string is the volume.
            return n;
        }
        // Consume the single separator, then the second component (the share).
        i += 1;
        while i < n && !at(i).is_some_and(|c| is_sep(c, windows)) {
            i += 1;
        }
        return i;
    }
    0
}

/// Could a path element alias to the `..` parent token once Windows applies its
/// filename canonicalisation? Windows strips trailing dots and spaces, so `".. "`
/// and `".. . "` name the parent directory — yet the lexical `..` scan, which
/// matches only the exact `..` token, would treat them as ordinary filenames and
/// miss the climb. Fail closed on any element that is made up SOLELY of dots and
/// spaces and carries at least two dots (`..`, `.. `, `. .`, `...`, ` .. `, …):
/// none is a legitimate filename, and each can canonicalise to `..`. Scanned
/// over the Windows separator set (`\` and `/`).
///
/// The exact `..` token is deliberately EXCLUDED here — the lexical scan already
/// counts it and `escapes_root` rejects any that climb out — so an in-bounds
/// `a\..\b` still resolves instead of being false-rejected.
fn has_disguised_dotdot(path: &str) -> bool {
    let windows = true;
    path.as_bytes().split(|&c| is_sep(c, windows)).any(|elem| {
        if elem == b".." {
            return false;
        }
        let only_dots_and_spaces = elem.iter().all(|&c| c == b'.' || c == b' ');
        let dot_count = elem.iter().filter(|&&c| c == b'.').count();
        only_dots_and_spaces && dot_count >= 2
    })
}

/// Does a CLEANED path climb above its root? Checks the path AFTER its volume
/// prefix (a drive/UNC volume is itself the root and can never be escaped). True
/// when that remainder is the whole `..` element or begins with a `..` element —
/// the two shapes `clean` leaves when leading `..`s could not be resolved away.
/// A rooted remainder (begins with a separator) can never escape: `clean` stops
/// `..` at the root. Separator-aware so a Windows `..\` escape is caught exactly
/// as a Unix `../` is.
fn escapes_root(cleaned: &str, windows: bool) -> bool {
    let vol = volume_name_len(cleaned, windows);
    let rest = cleaned.get(vol..).unwrap_or("");
    let rb = rest.as_bytes();
    if rest == ".." {
        return true;
    }
    // begins with a `..` element: `..` then a separator.
    rb.first().copied() == Some(b'.')
        && rb.get(1).copied() == Some(b'.')
        && rb.get(2).copied().is_some_and(|c| is_sep(c, windows))
}

/// Faithful port of Go `path/filepath.Clean`, driven by the platform separator
/// set. Lexically simplifies a path: collapses repeated separators, resolves
/// `.`/`..` elements, drops a trailing separator (except a root), normalises
/// every input separator to the platform `SEP`, and preserves a leading Windows
/// volume prefix (drive / UNC) that the `..` scan can never pop below. Pure byte
/// work — multi-byte UTF-8 path elements are copied intact (their bytes are
/// never a separator or ASCII `.`), so the result is valid UTF-8.
fn clean(path: &str) -> String {
    clean_with(path, WINDOWS)
}

/// The host-independent cleaner. `windows == true` selects the Windows
/// separator set (`\` and `/`) plus volume-prefix parsing; `false` is Unix
/// (`/` only, no volume). Split out so both branches are unit-testable on any
/// host — the Windows traversal defences are proven on Linux CI, not left to a
/// Windows-only build.
fn clean_with(path: &str, windows: bool) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let b = path.as_bytes();
    let n = b.len();
    // Total byte access (no `[]` indexing — clippy::indexing_slicing / no-panic
    // gate). Out-of-range reads as `None`, never panics.
    let at = |i: usize| -> Option<u8> { b.get(i).copied() };
    let sep = if windows { b'\\' } else { b'/' };

    let vol = volume_name_len(path, windows);
    let mut out: Vec<u8> = Vec::with_capacity(n + 1);
    // Copy the volume prefix through verbatim, normalising its separators (a UNC
    // `//server/share` becomes `\\server\share`). The `..` scan's floor,
    // `dotdot`, is anchored past it, so `..` can never delete or climb out of a
    // drive/UNC root.
    for i in 0..vol {
        match at(i) {
            Some(c) if is_sep(c, windows) => out.push(sep),
            Some(c) => out.push(c),
            None => {}
        }
    }
    // Width of the emitted volume prefix. The relative-part separator decisions
    // floor here (0 for a Unix/relative path), so consecutive leading `..`s stay
    // separated (`../..`, never a glued `....` that `escapes_root` would miss).
    let volw = out.len();
    let mut r = vol;
    // A path is rooted when the byte just after the volume is a separator. A
    // BARE drive (`C:` with no following separator) is drive-RELATIVE, not
    // rooted — so `C:..\x` keeps its leading `..` and is rejected as an escape,
    // never silently resolved against the drive root.
    let rooted = at(vol).is_some_and(|c| is_sep(c, windows));
    // `dotdot` is the index in `out` past which leading `..`s have been written
    // (for a relative path) or past the volume + root separator — popping never
    // crosses it.
    let mut dotdot = out.len();
    if rooted {
        out.push(sep);
        r += 1;
        dotdot = out.len();
    }
    while r < n {
        if at(r).is_some_and(|c| is_sep(c, windows)) {
            // empty path element → skip
            r += 1;
        } else if at(r) == Some(b'.')
            && (r + 1 == n || at(r + 1).is_some_and(|c| is_sep(c, windows)))
        {
            // `.` element → skip
            r += 1;
        } else if at(r) == Some(b'.')
            && at(r + 1) == Some(b'.')
            && (r + 2 == n || at(r + 2).is_some_and(|c| is_sep(c, windows)))
        {
            // `..` element → back up
            r += 2;
            if out.len() > dotdot {
                // pop the last element
                let mut w = out.len() - 1;
                while w > dotdot && !out.get(w).copied().is_some_and(|c| c == sep) {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                // cannot back up → keep the `..`
                if out.len() > volw {
                    out.push(sep);
                }
                out.push(b'.');
                out.push(b'.');
                dotdot = out.len();
            }
        } else {
            // real path element → append a separator (if needed) then the element
            if (rooted && out.len() != dotdot) || (!rooted && out.len() != volw) {
                out.push(sep);
            }
            while r < n && !at(r).is_some_and(|c| is_sep(c, windows)) {
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
        assert!(
            matches!(r, IpeResult::Err(_)),
            "a NUL byte must be rejected"
        );
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

    // ── Windows separator set — proven on Linux via the host-independent
    //    `clean_with(_, true)` / `escapes_root(_, true)` / `volume_name_len`.
    //    Each test names the Windows bypass vector it defends. `would_seal`
    //    mirrors the Windows branch of `path_from_string` (disguise guard +
    //    clean + escape check) so the whole seal is exercised off a real
    //    Windows host. ────────────────────────────────────────────────────────

    /// True when the Windows seal would ACCEPT `s` (mirror of the Windows
    /// `path_from_string` branch, forced on for a Linux-hosted test).
    fn win_seal_accepts(s: &str) -> bool {
        if s.as_bytes().contains(&0) {
            return false;
        }
        if has_disguised_dotdot(s) {
            return false;
        }
        !escapes_root(&clean_with(s, true), true)
    }

    #[test]
    fn unix_clean_is_byte_identical_under_the_unix_separator_set() {
        // Regression guard: the Windows-aware rewrite must not perturb Unix.
        for s in [
            "",
            "a//b///c",
            "a/b/../c",
            "/a/../../b",
            "src/Main.ipe",
            "/",
        ] {
            assert_eq!(
                clean_with(s, false),
                clean(s),
                "unix clean drifted for {s:?}"
            );
        }
    }

    #[test]
    fn unix_seal_rejects_consecutive_leading_dotdot() {
        // Regression: two consecutive leading `..` must stay separated (`../..`),
        // never glue into a `....` run that `escapes_root` misses. Each of these
        // escapes the root, so the Unix seal (clean + escapes_root) must reject it.
        for s in [
            "../..",
            "../../../etc/passwd",
            "a/../../..",
            "../../..",
            "x/../../../../y",
        ] {
            let cleaned = clean_with(s, false);
            assert!(
                escapes_root(&cleaned, false),
                "unix seal must reject escaping path {s:?} (cleaned to {cleaned:?})"
            );
        }
    }

    #[test]
    fn unix_clean_agrees_with_go_path_clean_on_a_dotdot_corpus() {
        // `clean_with(_, false)` must match Go `path.Clean` so a glued-dot
        // regression cannot slip past the escape check. Reference values are Go
        // `path.Clean` outputs.
        for (input, want) in [
            ("../..", "../.."),
            ("../../../etc/passwd", "../../../etc/passwd"),
            ("a/../../..", "../.."),
            ("./../a", "../a"),
            ("a/b/../../../c", "../c"),
        ] {
            assert_eq!(clean_with(input, false), want, "clean drift for {input:?}");
        }
    }

    #[test]
    fn win_backslash_traversal_is_rejected() {
        // Vector: `..\` — a backslash-separated parent climb Unix would miss.
        assert!(!win_seal_accepts("..\\secret"), "`..\\` must be rejected");
    }

    #[test]
    fn win_mixed_separator_traversal_is_rejected() {
        // Vector: `../..\` — separators mixed to slip one style past the scan.
        assert!(
            !win_seal_accepts("a/../..\\etc"),
            "mixed `../..\\` climbing out must be rejected"
        );
    }

    #[test]
    fn win_drive_relative_dotdot_is_rejected() {
        // Vector: `C:..\` — a drive-RELATIVE (not rooted) `..` climb. `C:` is a
        // bare volume, so the remainder is relative and its `..` escapes.
        assert!(
            !win_seal_accepts("C:..\\Windows"),
            "drive-relative `C:..\\` must be rejected"
        );
    }

    #[test]
    fn win_unc_root_is_not_escapable() {
        // Vector: `\\server\share\..\..\x` — `..` must not climb out of the UNC
        // share; it stays pinned at the volume and cleans in-bounds.
        let cleaned = clean_with("\\\\server\\share\\..\\..\\x", true);
        assert_eq!(cleaned, "\\\\server\\share\\x");
        assert!(
            !escapes_root(&cleaned, true),
            "UNC root must not be escapable"
        );
    }

    #[test]
    fn win_drive_absolute_dotdot_stops_at_root() {
        // A ROOTED drive path (`C:\`) stops `..` at the drive root, like Unix.
        let cleaned = clean_with("C:\\a\\..\\..\\b", true);
        assert_eq!(cleaned, "C:\\b");
        assert!(!escapes_root(&cleaned, true));
    }

    #[test]
    fn win_trailing_dot_space_disguised_dotdot_is_rejected() {
        // Vector: `.. ` / `...` — Windows strips trailing dots/spaces, turning a
        // literal element back into the `..` parent token the scan would miss.
        assert!(has_disguised_dotdot("a\\.. \\b"), "`.. ` disguise");
        assert!(has_disguised_dotdot("a\\...\\b"), "`...` disguise");
        assert!(!win_seal_accepts("a\\.. \\secret"));
        assert!(!win_seal_accepts("foo/.../bar"));
    }

    #[test]
    fn win_plain_dotdot_element_is_not_treated_as_a_disguise() {
        // The exact `..` token is handled by the normal scan, not the disguise
        // guard — so an in-bounds `a\..\b` still resolves rather than false-firing.
        assert!(!has_disguised_dotdot("a\\..\\b"));
        assert_eq!(clean_with("a\\..\\b", true), "b");
        assert!(win_seal_accepts("a\\..\\b"));
    }

    #[test]
    fn win_legitimate_path_cleans_and_normalises_separators() {
        // A real Windows path: mixed separators normalise, `.`/dup-sep collapse.
        assert_eq!(
            clean_with("C:\\Users\\me/Documents\\.\\a.ipe", true),
            "C:\\Users\\me\\Documents\\a.ipe"
        );
        assert!(win_seal_accepts("C:\\Users\\me\\Documents\\a.ipe"));
    }

    #[test]
    fn win_volume_name_len_recognises_drive_and_unc() {
        assert_eq!(volume_name_len("C:\\x", true), 2, "drive designator");
        assert_eq!(
            volume_name_len("\\\\srv\\shr\\x", true),
            9,
            "UNC server+share"
        );
        assert_eq!(volume_name_len("relative\\x", true), 0, "no volume");
        assert_eq!(
            volume_name_len("C:\\x", false),
            0,
            "no volume under Unix rules"
        );
    }

    #[test]
    fn win_nul_byte_still_rejected() {
        assert!(!win_seal_accepts("safe.txt\0..\\..\\Windows"));
    }
}
