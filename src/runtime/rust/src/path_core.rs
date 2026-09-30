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
// * `ElementClass` — the one per-element classifier under Windows filename
//   canonicalisation, read by the seal, the compile-time gate and the runtime
//   child-join parse alike.

/// Why a `path "…"` literal was rejected by [`validate`].
///
/// Each variant names one distinct rejection class; an exhaustive `match` on
/// this type forces every consumer to handle every class explicitly — a new
/// variant is a compile-time error at every call site, never a silent catch-all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathRejection {
    /// The string contains a NUL byte — a C-string terminator that truncates a
    /// path at the syscall boundary, enabling a poisoned-NUL bypass.
    Nul,
    /// The path escapes its root via `..` traversal (under Unix or Windows
    /// separators), or carries a Windows trailing-dot/space `..` disguise.
    Traversal,
}

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
/// [`PathRejection`] on failure.
///
/// # Errors
///
/// Returns `Err(PathRejection::Nul)` for a NUL byte, or
/// `Err(PathRejection::Traversal)` for any `..` escape (either separator
/// regime) or Windows dot/space disguise.
///
/// # Examples (illustrative only — `text`, not a compiled doctest)
///
/// ```text
/// validate("src/Main.ipe")   // Ok("src/Main.ipe")
/// validate("../etc/passwd")  // Err(PathRejection::Traversal)
/// validate("..\secret")      // Err(PathRejection::Traversal) — Windows separator
/// validate("a\0b")           // Err(PathRejection::Nul)
/// ```
pub fn validate(s: &str) -> Result<String, PathRejection> {
    if has_nul(s) {
        return Err(PathRejection::Nul);
    }
    if has_disguised_dotdot(s) {
        return Err(PathRejection::Traversal);
    }
    // Reject if the path escapes under EITHER separator regime: a Windows target
    // honours `\` as a separator, so a `..\` climb Unix cleaning would miss must
    // still fail the compile-time gate.
    if escapes_root(&clean_with(s, false), false) || escapes_root(&clean_with(s, true), true) {
        return Err(PathRejection::Traversal);
    }
    Ok(clean_with(s, false))
}

/// Is byte `c` an element separator under the active separator set?
///
/// Unix honours only `/`; Windows ALSO honours `\`, because Windows accepts
/// either at a syscall — so both must count, or the un-honoured one smuggles a
/// `..` past the traversal scan.
pub(crate) const fn is_sep(c: u8, windows: bool) -> bool {
    c == b'/' || (windows && c == b'\\')
}

/// The namespace tag of a `\\?\…` / `\\.\…` prefix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Namespace {
    /// `\\?\…` — verbatim: handed to the object manager without normalisation.
    Verbatim,
    /// `\\.\…` — the Win32 device namespace.
    Device,
}

/// The leading VOLUME of a path under Windows rules, parsed once.
///
/// Every consumer that asks "does this path carry a volume, and how long is
/// it?" reads this one parse, so the drive / UNC / namespace grammar lives in
/// one place. Unix never has a volume ([`Volume::None`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Volume<'a> {
    /// No volume: a Unix path, or a relative / root-relative Windows path.
    None,
    /// `X:` — a drive designator. Win32 reads ANY first UTF-16 unit followed by
    /// `:` as a drive (`é:`, `1:`, `::` included), so any non-separator
    /// character encoded as one UTF-16 unit qualifies — not only ASCII letters.
    /// A character outside the BMP is two UTF-16 units, so Win32 never pairs it
    /// with the `:` and it is not a drive.
    Drive(char),
    /// `\\server\share` — a UNC root; the share is absent for a bare `\\server`.
    Unc {
        server: &'a str,
        share: Option<&'a str>,
    },
    /// `\\?\UNC\server\share` — a verbatim UNC root. The server and share are
    /// part of the volume, so `..` can never climb out of the share and a
    /// root-relative path anchors on the right server.
    VerbatimUnc {
        server: &'a str,
        share: Option<&'a str>,
    },
    /// `\\?\name` / `\\.\name` — a verbatim or device namespace plus its first
    /// component (`\\?\C:`, `\\.\PhysicalDrive0`); `name` is absent for a bare
    /// `\\?` / `\\.`.
    Namespaced {
        tag: Namespace,
        name: Option<&'a str>,
    },
}

/// Split `s` at its first regime separator: the component before it, and the
/// text after it (`None` when `s` holds no separator).
fn split_component(s: &str, windows: bool) -> (&str, Option<&str>) {
    s.bytes()
        .position(|c| is_sep(c, windows))
        .map_or((s, None), |i| {
            // A separator is one ASCII byte, so `i` and `i + 1` are char boundaries.
            (s.get(..i).unwrap_or(""), s.get(i + 1..))
        })
}

/// Byte length of an optional `\component` tail.
fn tail_len(component: Option<&str>) -> usize {
    component.map_or(0, |c| 1 + c.len())
}

impl<'a> Volume<'a> {
    /// Parse the leading volume of `path`; always [`Volume::None`] on Unix.
    #[must_use]
    pub fn parse(path: &'a str, windows: bool) -> Self {
        if !windows {
            return Self::None;
        }
        let mut chars = path.chars();
        if let (Some(c), Some(':')) = (chars.next(), chars.next())
            && !u8::try_from(c).is_ok_and(|c| is_sep(c, windows))
            && c.len_utf16() == 1
        {
            return Self::Drive(c);
        }
        let b = path.as_bytes();
        let lead = |i: usize| b.get(i).is_some_and(|&c| is_sep(c, windows));
        if !(lead(0) && lead(1)) {
            return Self::None;
        }
        let (first, after) = split_component(path.get(2..).unwrap_or(""), windows);
        let component = |s: &'a str| split_component(s, windows).0;
        let tag = match first {
            "?" => Namespace::Verbatim,
            "." => Namespace::Device,
            server => {
                return Self::Unc {
                    server,
                    share: after.map(component),
                };
            }
        };
        if tag == Namespace::Verbatim
            && let Some(a) = after
            && let (name, Some(unc)) = split_component(a, windows)
            && name.eq_ignore_ascii_case("UNC")
        {
            let (server, share) = split_component(unc, windows);
            return Self::VerbatimUnc {
                server,
                share: share.map(component),
            };
        }
        Self::Namespaced {
            tag,
            name: after.map(component),
        }
    }

    /// Length in bytes of the volume prefix (`0` for [`Volume::None`]).
    #[must_use]
    pub fn byte_len(self) -> usize {
        match self {
            Self::None => 0,
            Self::Drive(c) => c.len_utf8() + 1,
            Self::Unc { server, share } => 2 + server.len() + tail_len(share),
            // `\\?\UNC\` is eight bytes.
            Self::VerbatimUnc { server, share } => 8 + server.len() + tail_len(share),
            // `\\?` / `\\.` is three bytes.
            Self::Namespaced { name, .. } => 3 + tail_len(name),
        }
    }

    /// Is this a drive designator (`X:`)? A drive alone is drive-RELATIVE,
    /// never rooted.
    #[must_use]
    pub const fn is_drive(self) -> bool {
        matches!(self, Self::Drive(_))
    }

    /// Can this volume anchor a Windows root-relative path (`\x`)?
    ///
    /// Only a volume that names one location completely: a drive, a UNC or
    /// verbatim-UNC root with both a server and a share, or a namespace with a
    /// non-UNC name. A device-namespace `\\.\UNC` (whose server and share this
    /// parse leaves outside the volume) or an incomplete UNC is refused, so a
    /// root-relative path is never anchored on the wrong server.
    #[must_use]
    pub fn anchors(self) -> bool {
        let named = |part: Option<&str>| part.is_some_and(|p| !p.is_empty());
        match self {
            Self::None => false,
            Self::Drive(_) => true,
            Self::Unc { server, share } | Self::VerbatimUnc { server, share } => {
                !server.is_empty() && named(share)
            }
            Self::Namespaced { name, .. } => {
                named(name) && !name.is_some_and(|n| n.eq_ignore_ascii_case("UNC"))
            }
        }
    }
}

/// Length in bytes of the leading VOLUME name of `path` under Windows rules:
/// the byte length of its [`Volume::parse`].
///
/// `0` on Unix, where no path element is ever consumed as a volume. The volume
/// is copied through `clean_with` untouched and is the floor the `..` scan can
/// never pop below — so `..` can neither delete a drive letter nor climb out of
/// a UNC share.
#[must_use]
pub fn volume_name_len(path: &str, windows: bool) -> usize {
    Volume::parse(path, windows).byte_len()
}

/// How Windows filename canonicalisation reads one raw path element (the bytes
/// between two separators).
///
/// THE element classifier: the runtime seal, the compile-time [`validate`] gate
/// and the runtime child-join parse all read an element through
/// [`ElementClass::of`], so the dot-and-space rule is stated once. Each consumer
/// decides which classes it refuses; the classes themselves never differ.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ElementClass {
    /// An empty element (a doubled or trailing separator).
    Empty,
    /// The exact `.` token.
    Current,
    /// The exact `..` token.
    Parent,
    /// Only dots and spaces with at least two dots, other than the exact `..`
    /// (`.. `, `. .`, `...`, ` .. `): Windows strips trailing dots and spaces,
    /// so it can name the parent directory.
    DisguisedParent,
    /// Only dots and spaces with at most one dot, other than the exact `.`
    /// (` `, `. `, ` . `): Windows strips it to the directory itself.
    DisguisedCurrent,
    /// Holds a `:` — a drive designator (`é:`, `1:`) or an alternate data
    /// stream (`a:b`).
    Colon,
    /// A reserved Win32 DOS device name (see [`is_dos_device`]).
    DosDevice,
    /// Any other element.
    Name,
}

impl ElementClass {
    /// Classify one raw element.
    #[must_use]
    pub fn of(e: &[u8]) -> Self {
        match e {
            b"" => Self::Empty,
            b"." => Self::Current,
            b".." => Self::Parent,
            _ if e.iter().all(|&c| c == b'.' || c == b' ') => {
                // "at least two dots" without a full count (dodges the
                // naive-bytecount lint).
                if e.iter().filter(|&&c| c == b'.').nth(1).is_some() {
                    Self::DisguisedParent
                } else {
                    Self::DisguisedCurrent
                }
            }
            _ if e.contains(&b':') => Self::Colon,
            _ if is_dos_device(e) => Self::DosDevice,
            _ => Self::Name,
        }
    }
}

/// Does the raw element `e` name a reserved Win32 DOS device?
///
/// Win32 opens a device, not a file, for `CON`, `PRN`, `AUX`, `NUL`,
/// `COM0`–`COM9`, `LPT0`–`LPT9`, the superscript-digit `COM¹²³` / `LPT¹²³`, and
/// `CONIN$` / `CONOUT$`, matched case-insensitively on the element's stem: the
/// text before its first `.` or `:`, with trailing spaces dropped. An extension
/// does not escape the device (`nul.txt`, `aux.tar.gz`) on older Windows
/// versions, so the check fails closed on every version.
#[must_use]
pub fn is_dos_device(e: &[u8]) -> bool {
    let stem_end = e
        .iter()
        .position(|&c| c == b'.' || c == b':')
        .unwrap_or(e.len());
    let mut stem = e.get(..stem_end).unwrap_or(e);
    while let [rest @ .., b' '] = stem {
        stem = rest;
    }
    let named = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|n| stem.eq_ignore_ascii_case(n.as_bytes()));
    let numbered = stem.split_at_checked(3).is_some_and(|(head, digit)| {
        (head.eq_ignore_ascii_case(b"COM") || head.eq_ignore_ascii_case(b"LPT"))
            // An ASCII digit, or the UTF-8 encoding of `¹` / `²` / `³`.
            && matches!(digit, [b'0'..=b'9'] | [0xC2, 0xB9 | 0xB2 | 0xB3])
    });
    named || numbered
}

/// Could a path element alias to the `..` parent token once Windows applies its
/// filename canonicalisation?
///
/// True when any element, split over the Windows separator set (`\` and `/`),
/// is an [`ElementClass::DisguisedParent`] (`.. `, `...`, `. .`): the lexical
/// `..` scan matches only the exact `..` token and would miss the climb. None
/// is a legitimate filename.
///
/// The exact `..` token is deliberately EXCLUDED here — the lexical scan already
/// counts it and [`escapes_root`] rejects any that climb out — so an in-bounds
/// `a\..\b` still resolves instead of being false-rejected.
#[must_use]
pub fn has_disguised_dotdot(path: &str) -> bool {
    path.as_bytes()
        .split(|&c| is_sep(c, true))
        .any(|e| ElementClass::of(e) == ElementClass::DisguisedParent)
}

/// Does a CLEANED path climb above its root?
///
/// Checks the path AFTER its volume
/// prefix (a drive/UNC volume is itself the root and can never be escaped). True
/// when that remainder's FIRST element is a `..` climb — the shape `clean_with`
/// leaves when a leading `..` could not be resolved away. A rooted remainder
/// (begins with a separator) can never escape: `clean_with` stops `..` at the
/// root. Separator-aware so a Windows `..\` escape is caught exactly as a Unix
/// `../` is.
///
/// # Two-layer defence
///
/// The primary check is the exact `..` token (the token `clean_with` scans and
/// counts). As independent defence-in-depth, the FIRST element is ALSO rejected
/// when it is a non-empty run made SOLELY of dots with length >= 2 (`..`, `...`,
/// `....`, …): so even if a future `clean_with` change ever produced a glued-dot
/// run — a single-point cleaner bug — this escape check would still catch it
/// without relying on the cleaner. The two layers reject independently.
///
/// This over-rejects a legitimate top-level filename made solely of dots
/// (e.g. `...` as a real filename). That is ACCEPTABLE — it fails closed, and
/// matches the Windows `has_disguised_dotdot` behaviour, which already rejects
/// the same all-dots family (Windows canonicalisation would alias it to `..`).
#[must_use]
pub fn escapes_root(cleaned: &str, windows: bool) -> bool {
    let vol = volume_name_len(cleaned, windows);
    let rest = cleaned.get(vol..).unwrap_or("");
    let rb = rest.as_bytes();
    // The FIRST element of the remainder: bytes up to the first separator.
    let first = rb.split(|&c| is_sep(c, windows)).next().unwrap_or(&[]);
    // Layer 1: the exact `..` climb token.
    if first == b".." {
        return true;
    }
    // Layer 2 (defence-in-depth): any leading all-dots run of length >= 2. Even
    // a glued `...`/`....` a broken cleaner might emit is caught here, without
    // depending on the cleaner having split it back into discrete `..` tokens.
    let all_dots = first.iter().all(|&c| c == b'.');
    // Length >= 2: a second byte exists (guarded so a single `.` element, which
    // is not a climb, is never rejected).
    let two_or_more = first.get(1).is_some();
    all_dots && two_or_more
}

/// Faithful port of , driven by the chosen separator set.
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
        assert_eq!(validate("safe\0bad"), Err(PathRejection::Nul));
    }

    #[test]
    fn leading_dotdot_rejected() {
        assert_eq!(validate("../secret"), Err(PathRejection::Traversal));
    }

    #[test]
    fn bare_dotdot_rejected() {
        assert_eq!(validate(".."), Err(PathRejection::Traversal));
    }

    #[test]
    fn dotdot_that_resolves_to_escape_rejected() {
        // "a/../../etc" cleans to "../etc"
        assert_eq!(validate("a/../../etc"), Err(PathRejection::Traversal));
    }

    // ── validate: rejected under the Windows regime (the all-targets guarantee) ─
    //    Each of these is a Unix-clean no-op (a `\` is a plain filename byte on
    //    Unix) yet a traversal on Windows. The all-targets gate must reject them
    //    at compile time so no such literal is ever emitted for a Windows build.

    #[test]
    fn win_backslash_traversal_rejected() {
        assert_eq!(validate("..\\secret"), Err(PathRejection::Traversal));
    }

    #[test]
    fn win_drive_relative_dotdot_rejected() {
        assert_eq!(validate("C:..\\x"), Err(PathRejection::Traversal));
    }

    #[test]
    fn win_trailing_dot_space_disguise_rejected() {
        assert_eq!(validate(".. \\x"), Err(PathRejection::Traversal));
    }

    #[test]
    fn win_triple_dot_disguise_rejected() {
        assert_eq!(validate("..."), Err(PathRejection::Traversal));
    }

    #[test]
    fn win_mixed_separator_traversal_rejected() {
        assert_eq!(validate("a\\..\\..\\b"), Err(PathRejection::Traversal));
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

    #[test]
    fn volume_parse_follows_the_win32_drive_and_verbatim_unc_grammar() {
        // Any single-UTF-16-unit character before `:` is a drive, as in Win32.
        assert_eq!(volume_name_len("é:\\x", true), 3);
        assert_eq!(volume_name_len("1:x", true), 2);
        assert!(Volume::parse("é:", true).is_drive());
        // A non-BMP character is two UTF-16 units: never a drive.
        assert_eq!(volume_name_len("𝒳:x", true), 0);
        // A verbatim UNC root keeps its server and share inside the volume.
        assert_eq!(volume_name_len("\\\\?\\UNC\\srv\\shr\\x", true), 15);
        assert!(Volume::parse("\\\\?\\UNC\\srv\\shr\\x", true).anchors());
        assert_eq!(
            clean_with("\\\\?\\UNC\\srv\\shr\\..\\..", true),
            "\\\\?\\UNC\\srv\\shr\\"
        );
        assert_eq!(volume_name_len("\\\\?\\C:\\x", true), 6);
        // A device-namespace UNC and an incomplete UNC name no anchoring volume.
        assert!(!Volume::parse("\\\\.\\UNC\\srv\\shr\\x", true).anchors());
        assert!(!Volume::parse("\\\\srv", true).anchors());
        assert!(!Volume::parse("\\x", true).anchors());
    }

    // ── ElementClass: the one element classifier ──────────────────────────────

    #[test]
    fn element_class_names_the_dot_space_aliases() {
        for (e, want) in [
            ("", ElementClass::Empty),
            (".", ElementClass::Current),
            ("..", ElementClass::Parent),
            (".. ", ElementClass::DisguisedParent),
            ("...", ElementClass::DisguisedParent),
            (". .", ElementClass::DisguisedParent),
            (" ", ElementClass::DisguisedCurrent),
            (". ", ElementClass::DisguisedCurrent),
            (" . ", ElementClass::DisguisedCurrent),
            ("a:b", ElementClass::Colon),
            ("CON", ElementClass::DosDevice),
            ("a.b", ElementClass::Name),
            ("..foo", ElementClass::Name),
        ] {
            assert_eq!(ElementClass::of(e.as_bytes()), want, "{e:?}");
        }
    }

    #[test]
    fn dos_device_names_are_recognised_on_their_stem() {
        for e in [
            "CON",
            "con",
            "con.txt",
            "NUL ",
            "nul.txt",
            "COM1",
            "lpt9",
            "COM0",
            "COM\u{b9}",
            "LPT\u{b3}",
            "CONOUT$",
            "conin$",
            "aux.tar.gz",
            "PRN:",
            "AUX :x",
        ] {
            assert!(is_dos_device(e.as_bytes()), "{e:?} is a device");
        }
        for e in [
            "CONSOLE",
            "COM10",
            "nulx",
            "xCON",
            "COM",
            "LPT",
            "COM\u{b4}",
            "CO",
            "",
            "COMa",
        ] {
            assert!(!is_dos_device(e.as_bytes()), "{e:?} is a plain name");
        }
    }

    #[test]
    fn disguised_dotdot_is_exactly_the_disguised_parent_class() {
        // The seal and gate rule, stated independently of the classifier: an
        // all-dots-and-spaces element with at least two dots, other than `..`.
        let rule = |e: &[u8]| {
            e != b".."
                && e.iter().all(|&c| c == b'.' || c == b' ')
                && e.iter().filter(|&&c| c == b'.').nth(1).is_some()
        };
        for e in [
            "", ".", "..", "...", ".. ", " ", ". ", " .. ", "a", "a.", ". . .",
        ] {
            assert_eq!(has_disguised_dotdot(e), rule(e.as_bytes()), "{e:?}");
        }
    }

    // ── escapes_root: two-layer defence against a leading all-dots element ─────

    #[test]
    fn escapes_root_rejects_exact_leading_dotdot() {
        // Layer 1: the exact `..` token, whole or as a leading element.
        for (regime, s) in [
            (false, ".."),
            (false, "../x"),
            (true, ".."),
            (true, "..\\x"),
        ] {
            assert!(
                escapes_root(s, regime),
                "leading `..` must escape ({s:?}, windows={regime})"
            );
        }
    }

    #[test]
    fn escapes_root_rejects_leading_glued_dots() {
        // Layer 2 (defence-in-depth): a leading all-dots run of length >= 2 is
        // rejected DIRECTLY, without the cleaner having to split it into `..`
        // tokens. These are the shapes a broken cleaner might glue together.
        for (regime, s) in [
            (false, "..."),
            (false, "...."),
            (false, ".../x"),
            (false, "..../x"),
            (true, "..."),
            (true, "....\\x"),
        ] {
            assert!(
                escapes_root(s, regime),
                "leading glued-dot run must escape ({s:?}, windows={regime})"
            );
        }
    }

    #[test]
    fn escapes_root_allows_in_bounds_and_dotted_names() {
        // A single `.` is not a climb; a legitimate in-bounds cleaned path does
        // not escape; and a name that has dots PLUS other chars (`..foo`) is a
        // real filename, not an all-dots run, so it is NOT rejected.
        for (regime, s) in [
            (false, "a/b"),
            (false, "."),
            (false, "..foo"),
            (false, "..foo/bar"),
            (false, "foo.."),
            (true, "..foo\\bar"),
        ] {
            assert!(
                !escapes_root(s, regime),
                "in-bounds / dotted-name path must NOT escape ({s:?}, windows={regime})"
            );
        }
        // `a/../b` resolves in-bounds and does not escape after cleaning.
        assert_eq!(clean_with("a/../b", false), "b");
        assert!(!escapes_root(&clean_with("a/../b", false), false));
    }
}
