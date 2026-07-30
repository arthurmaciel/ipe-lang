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
// The lexical validation algorithm lives once in the sibling `path_core` module
// (shared with the compiler's `path "…"` gate, which `include!`s the SAME
// `path_core.rs` file via the `ipe_path_core` crate); this module drives it with
// the HOST separator regime so the runtime seal stays target-specific. A sibling
// module (not an extern crate) so it resolves both in the workspace AND when the
// runtime is vendored as `mod ipe_runtime` into an emitted app.
use super::path_core::{clean_with, escapes_root, has_disguised_dotdot, has_nul};

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
    if has_nul(&s) {
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

/// Construct an already-validated `Path` from a pre-cleaned string.
///
/// Only the compiler's code generator calls this — exclusively at sites where
/// a `path "…"` literal has already been validated and cleaned at compile time.
/// Never expose this function to user Ipê source or use it outside generated
/// code: it bypasses the parse-don't-validate seal in [`path_from_string`].
///
/// The string MUST have come from [`path_from_string`]'s cleaned output (NUL-
/// free, non-escaping); the compiler enforces this at compile time before
/// emitting a call here, so no runtime re-check is needed.
#[must_use]
#[doc(hidden)]
pub fn path_literal(cleaned: String) -> Path {
    Path(cleaned)
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

/// Lexically clean `path` under the HOST separator regime (Unix `/`, or the
/// Windows `\`/`/` set with volume-prefix parsing on a Windows build). Thin
/// wrapper over the shared [`super::path_core::clean_with`] so the runtime and the
/// compiler's `path "…"` gate clean identically. Used by the pure helpers
/// ([`path_dir`]) that re-clean a derived substring.
fn clean(path: &str) -> String {
    clean_with(path, WINDOWS)
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
    // Exercised only by the Windows volume-prefix tests below.
    use super::super::path_core::volume_name_len;

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
    fn escapes_root_rejects_leading_glued_dot_run() {
        // Defence-in-depth: `escapes_root` rejects a leading all-dots element of
        // length >= 2 DIRECTLY, so a glued `...`/`....` a broken cleaner might
        // ever emit is caught independently of the cleaner. Exact `..` still
        // rejects; a real filename with dots plus other chars (`..foo`) does not.
        for regime in [false, true] {
            for escape in ["..", "...", "....", ".../x", "..../x"] {
                assert!(
                    escapes_root(escape, regime),
                    "leading all-dots element must escape ({escape:?}, windows={regime})"
                );
            }
            for keep in ["..foo", "..foo/bar", "a/b"] {
                assert!(
                    !escapes_root(keep, regime),
                    "dotted filename / in-bounds path must NOT escape ({keep:?}, windows={regime})"
                );
            }
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

    // ── SSOT differential: compile-time gate ⊆ runtime seal on BOTH targets ────
    //    `ipe_path_core::validate` (the all-targets compile-time gate) must NEVER
    //    accept a string that either target's runtime `path_from_string` seal
    //    would reject — otherwise a validated `path "…"` literal could traverse
    //    at runtime on some target. Both seals share this crate's primitives, so
    //    this test is the guard that keeps the compile-time gate at least as
    //    strict as the runtime on every host.

    /// The runtime seal's accept decision for a given target regime — the exact
    /// predicate `path_from_string` applies (NUL + Windows disguise + escape),
    /// with the separator regime fixed by `windows` rather than the host.
    fn runtime_seal_accepts(s: &str, windows: bool) -> bool {
        if has_nul(s) {
            return false;
        }
        if windows && has_disguised_dotdot(s) {
            return false;
        }
        !escapes_root(&clean_with(s, windows), windows)
    }

    /// A corpus over `{a . / \ : NUL C 1 space}` up to length 4 — every byte
    /// that participates in a separator, a `.`/`..` element, a drive prefix, a
    /// NUL truncation, or the disguise scan.
    fn corpus() -> Vec<String> {
        const ALPHABET: [char; 9] = ['a', '.', '/', '\\', ':', '\0', 'C', '1', ' '];
        let mut out = vec![String::new()];
        let mut frontier = vec![String::new()];
        for _ in 0..4 {
            let mut next = Vec::new();
            for prefix in &frontier {
                for c in ALPHABET {
                    let mut s = prefix.clone();
                    s.push(c);
                    next.push(s);
                }
            }
            out.extend(next.iter().cloned());
            frontier = next;
        }
        out
    }

    #[test]
    fn test_mirrors_runtime() {
        for s in corpus() {
            if super::super::path_core::validate(&s).is_ok() {
                assert!(
                    runtime_seal_accepts(&s, false),
                    "compile-time gate accepted {s:?} but the Unix runtime seal rejects it"
                );
                assert!(
                    runtime_seal_accepts(&s, true),
                    "compile-time gate accepted {s:?} but the Windows runtime seal rejects it"
                );
            }
        }
    }

    #[test]
    fn compile_time_gate_rejects_the_windows_traversal_vectors() {
        // The specific vectors from the finding: each is a Unix-clean no-op yet a
        // traversal on a Windows target, so the all-targets compile-time gate
        // must reject every one.
        for vector in ["..\\secret", "C:..\\x", ".. \\x", "...", "a\\..\\..\\b"] {
            assert_eq!(
                super::super::path_core::validate(vector),
                Err("traversal"),
                "compile-time gate must reject the Windows traversal vector {vector:?}"
            );
        }
    }
}
