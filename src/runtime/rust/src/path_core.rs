// The single source of truth for Ipê's lexical path validation.
//
// Both the runtime `Path.fromString` seal (`crate::path`) and the compiler's
// `path "…"` literal gate (`ipe_diagnostics::path_check`) validate the SAME
// way, so the algorithm lives here ONCE and both consumers use this one file.
// Neither keeps its own copy. The module is dependency-free (std only): the
// runtime references it as a sibling module (`crate::path_core::…`), and the
// standalone `ipe_path_core` crate `include!`s this exact file so the compiler
// can validate a literal without pulling in the runtime's heavy optional
// dependencies (tokio, serde, sqlx, …).
//
// Regular (`//`) comments, not inner docs (`//!`): this file is `include!`d
// verbatim into the `ipe_path_core` crate root, where a leading `//!` after the
// `include!` item would be an illegal mid-file inner attribute. The crate-level
// docs live in `ipe_path_core`'s `lib.rs`.
//
// # Two entry points, one algorithm
//
// * `validate` — the COMPILE-TIME gate. The compiler does not know the final
//   target OS, so it rejects a path that would traverse under EITHER separator
//   regime (Unix `/` or Windows `\`/`/`). This is deliberately stricter than
//   the runtime's target-specific check: a compile-time reject can only ever be
//   a superset of what the runtime rejects, so nothing the runtime would refuse
//   is ever emitted as a validated literal.
// * `clean_with` / `escapes_root` / `has_disguised_dotdot` / `has_nul`
//   — the target-specific primitives the runtime seal drives with its own
//   host separator regime (`clean_with(s, cfg!(windows))`), keeping the runtime
//   behaviour byte-identical per platform.

/// Does `s` contain a NUL byte?
///
/// A NUL is a C-string terminator that truncates a path at the syscall boundary
/// (`"safe.txt\0../../etc/passwd"` reaches the kernel as `"safe.txt"` on one code
/// path and the full string on another — a classic poisoned-NUL bypass), so it
/// is rejected under every regime.
#[must_use]
pub fn has_nul(s: &str) -> bool {
    s.as_bytes().contains(&0)
}

/// Compile-time validation for a `path "…"` literal.
///
/// The compiler cannot know the final target OS, so this rejects `s` if it is a
/// traversal or injection surface under EITHER separator regime — a NUL byte, a
/// Windows trailing-dot/space `..` disguise, or a `..` escape under either the
/// Unix (`/`) or the Windows (`\`/`/`) cleaner. Stricter than the runtime's
/// per-target [`escapes_root`] check by construction, so a literal that passes
/// here is accepted by the runtime seal on every target.
///
/// Returns the Unix-cleaned path string on success (the Rust backend's
/// equivalence target is Linux, so the emitted literal is the Unix form), or a
/// `&'static str` reason code on failure:
/// - `"nul"` — the string contains a NUL byte.
/// - `"traversal"` — the path escapes its root via `..` under some regime, or
///   carries a Windows trailing-dot/space `..` disguise.
///
/// # Errors
///
/// Returns `Err("nul")` for a NUL byte, or `Err("traversal")` for any `..`
/// escape (either separator regime) or Windows dot/space disguise.
///
/// # Examples (not runnable — illustrative only)
///
///     validate("src/Main.ipe")   // Ok("src/Main.ipe")
///     validate("../etc/passwd")  // Err("traversal")
///     validate("..\\secret")     // Err("traversal") — Windows separator
///     validate("a\0b")           // Err("nul")
pub fn validate(s: &str) -> Result<String, &'static str> {
    if has_nul(s) {
        return Err("nul");
    }
    if has_disguised_dotdot(s) {
        return Err("traversal");
    }
    // Reject if the path escapes under EITHER separator regime: a Windows target
    // honours `\` as a separator, so a `..\` climb Unix cleaning would miss must
    // still fail the compile-time gate.
    if escapes_root(&clean_with(s, false), false) || escapes_root(&clean_with(s, true), true) {
        return Err("traversal");
    }
    Ok(clean_with(s, false))
}

/// Is byte `c` an element separator under the active separator set?
///
/// Unix honours only `/`; Windows ALSO honours `\`, because Windows accepts
/// either at a syscall — so both must count, or the un-honoured one smuggles a
/// `..` past the traversal scan.
const fn is_sep(c: u8, windows: bool) -> bool {
    c == b'/' || (windows && c == b'\\')
}

/// Length in bytes of the leading VOLUME name of `path` under Windows rules.
///
/// `0` on Unix, where no path element is ever consumed as a volume. Ported
/// from Go `path/filepath.volumeNameLen`. Recognised prefixes:
/// * `\\?\…` / `\\.\…` — verbatim / device namespaces (consume up to the next
///   separator after the namespace tag);
/// * `\\server\share` — a UNC root (both the server and the share component);
/// * `C:` — a drive designator (two bytes).
///
/// The volume is copied through `clean_with` untouched and is the floor the `..`
/// scan can never pop below — so `..` can neither delete a drive letter nor
/// climb out of a UNC share.
#[must_use]
pub fn volume_name_len(path: &str, windows: bool) -> usize {
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
/// filename canonicalisation?
///
/// Windows strips trailing dots and spaces, so `".. "`
/// and `".. . "` name the parent directory — yet the lexical `..` scan, which
/// matches only the exact `..` token, would treat them as ordinary filenames and
/// miss the climb. Fail closed on any element that is made up SOLELY of dots and
/// spaces and carries at least two dots (`..`, `.. `, `. .`, `...`, ` .. `, …):
/// none is a legitimate filename, and each can canonicalise to `..`. Scanned
/// over the Windows separator set (`\` and `/`).
///
/// The exact `..` token is deliberately EXCLUDED here — the lexical scan already
/// counts it and [`escapes_root`] rejects any that climb out — so an in-bounds
/// `a\..\b` still resolves instead of being false-rejected.
#[must_use]
pub fn has_disguised_dotdot(path: &str) -> bool {
    let windows = true;
    path.as_bytes().split(|&c| is_sep(c, windows)).any(|elem| {
        if elem == b".." {
            return false;
        }
        let only_dots_and_spaces = elem.iter().all(|&c| c == b'.' || c == b' ');
        // "at least two dots" without a full count (dodges the naive-bytecount lint).
        let has_two_dots = elem.iter().filter(|&&c| c == b'.').nth(1).is_some();
        only_dots_and_spaces && has_two_dots
    })
}

/// Does a CLEANED path climb above its root?
///
/// Checks the path AFTER its volume
/// prefix (a drive/UNC volume is itself the root and can never be escaped). True
/// when that remainder is the whole `..` element or begins with a `..` element —
/// the two shapes `clean_with` leaves when leading `..`s could not be resolved
/// away. A rooted remainder (begins with a separator) can never escape:
/// `clean_with` stops `..` at the root. Separator-aware so a Windows `..\`
/// escape is caught exactly as a Unix `../` is.
#[must_use]
pub fn escapes_root(cleaned: &str, windows: bool) -> bool {
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

/// Faithful port of Go `path/filepath.Clean`, driven by the chosen separator set.
///
/// `windows == true` selects the Windows separator set (`\` and `/`) plus
/// volume-prefix parsing; `false` is Unix (`/` only, no volume). Split so both
/// branches are unit-testable on any host — the Windows traversal defences are
/// proven on Linux CI, not left to a Windows-only build.
///
/// Lexically simplifies a path: collapses repeated separators, resolves `.`/`..`
/// elements, drops a trailing separator (except a root), normalises every input
/// separator to the platform separator, and preserves a leading Windows volume
/// prefix (drive / UNC) that the `..` scan can never pop below. Pure byte work —
/// multi-byte UTF-8 path elements are copied intact (their bytes are never a
/// separator or ASCII `.`), so the result is valid UTF-8.
#[must_use]
pub fn clean_with(path: &str, windows: bool) -> String {
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
    if rooted {
        out.push(sep);
        r += 1;
    }
    // `dotdot` is the index in `out` past which leading `..`s have been written
    // (for a relative path) or past the volume + root separator — popping never
    // crosses it. Anchored AFTER the root separator (if any) is written.
    let mut dotdot = out.len();
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
                while w > dotdot && out.get(w).copied().is_none_or(|c| c != sep) {
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate: accepted paths ─────────────────────────────────────────────

    #[test]
    fn plain_relative_accepted() {
        assert_eq!(validate("src/Main.ipe"), Ok("src/Main.ipe".to_string()));
    }

    #[test]
    fn absolute_accepted() {
        assert_eq!(
            validate("/usr/share/data"),
            Ok("/usr/share/data".to_string())
        );
    }

    #[test]
    fn interior_dotdot_that_stays_in_bounds_accepted() {
        assert_eq!(validate("a/b/../c"), Ok("a/c".to_string()));
    }

    #[test]
    fn rooted_dotdot_cannot_escape_accepted() {
        assert_eq!(validate("/a/../../b"), Ok("/b".to_string()));
    }

    #[test]
    fn empty_cleans_to_dot() {
        assert_eq!(validate(""), Ok(".".to_string()));
    }

    // ── validate: rejected under the Unix regime ─────────────────────────────

    #[test]
    fn nul_byte_rejected() {
        assert_eq!(validate("safe\0bad"), Err("nul"));
    }

    #[test]
    fn leading_dotdot_rejected() {
        assert_eq!(validate("../secret"), Err("traversal"));
    }

    #[test]
    fn bare_dotdot_rejected() {
        assert_eq!(validate(".."), Err("traversal"));
    }

    #[test]
    fn dotdot_that_resolves_to_escape_rejected() {
        // "a/../../etc" cleans to "../etc"
        assert_eq!(validate("a/../../etc"), Err("traversal"));
    }

    // ── validate: rejected under the Windows regime (the all-targets guarantee) ─
    //    Each of these is a Unix-clean no-op (a `\` is a plain filename byte on
    //    Unix) yet a traversal on Windows. The all-targets gate must reject them
    //    at compile time so no such literal is ever emitted for a Windows build.

    #[test]
    fn win_backslash_traversal_rejected() {
        assert_eq!(validate("..\\secret"), Err("traversal"));
    }

    #[test]
    fn win_drive_relative_dotdot_rejected() {
        assert_eq!(validate("C:..\\x"), Err("traversal"));
    }

    #[test]
    fn win_trailing_dot_space_disguise_rejected() {
        assert_eq!(validate(".. \\x"), Err("traversal"));
    }

    #[test]
    fn win_triple_dot_disguise_rejected() {
        assert_eq!(validate("..."), Err("traversal"));
    }

    #[test]
    fn win_mixed_separator_traversal_rejected() {
        assert_eq!(validate("a\\..\\..\\b"), Err("traversal"));
    }

    #[test]
    fn win_in_bounds_backslash_dotdot_accepted() {
        // `a\..\b` resolves in-bounds on Windows; on Unix `\` is a filename byte,
        // so the whole thing is a single element. Neither regime escapes, so the
        // all-targets gate accepts it. Cleaned form is the Unix reading.
        assert_eq!(validate("a\\..\\b"), Ok("a\\..\\b".to_string()));
    }

    // ── clean_with: Unix / Windows byte-for-byte spot checks ──────────────────

    #[test]
    fn clean_collapses_repeated_separators() {
        assert_eq!(clean_with("a//b///c", false), "a/b/c");
    }

    #[test]
    fn clean_empty_gives_dot() {
        assert_eq!(clean_with("", false), ".");
    }

    #[test]
    fn win_unc_root_not_escapable() {
        let cleaned = clean_with("\\\\server\\share\\..\\..\\x", true);
        assert_eq!(cleaned, "\\\\server\\share\\x");
        assert!(!escapes_root(&cleaned, true));
    }

    #[test]
    fn volume_name_len_recognises_drive_and_unc() {
        assert_eq!(volume_name_len("C:\\x", true), 2);
        assert_eq!(volume_name_len("\\\\srv\\shr\\x", true), 9);
        assert_eq!(volume_name_len("relative\\x", true), 0);
        assert_eq!(volume_name_len("C:\\x", false), 0);
    }
}
