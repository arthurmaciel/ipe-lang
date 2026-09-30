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
//! The lexical engine (`clean`) implements Unix `filepath` semantics directly
//! rather than wrapping `std::path`, which is OS-tagged and diverges on
//! trailing slashes, repeated separators, and dotfiles. On Windows the same
//! engine is driven with the Windows separator set (`\` and `/`) and
//! volume-prefix parsing so the traversal check is not `\`-bypassable (see
//! [`clean_with`]).
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
//! # Composition — `under` / `absolute`
//!
//! Paths are composed ONLY through [`path_under`] (`root` + relative `child`),
//! never by string concatenation. It refuses an empty, absolute,
//! volume-prefixed, `..`-bearing, or NUL-bearing child, and re-checks that the
//! cleaned join lies component-wise below the root (`/repo2/x` is not under
//! `/repo`). On Windows it also refuses a child element holding a `:`, made
//! only of dots and spaces, or naming a reserved DOS device, and re-scans the
//! joined result for the last two independently of the child parse.
//! [`path_absolute`] resolves a relative path against the working
//! directory through the same join.
//!
//! Symlink decision: both are LEXICAL and do not touch the filesystem. They do
//! not resolve, follow, or forbid symlinks, so a joined path whose ancestor is
//! a link can still reach outside the root when later opened. This fails
//! closed in the only sense a lexical operation can: nothing is ever resolved
//! into a wider path than the text names. Holding the root as a directory
//! handle (`openat`-style resolution) is the `Ipe.File` boundary's concern.
//!
//! In short: `Path` closes the raw-string traversal/NUL-injection hole at the
//! type boundary; it is not a substitute for the capability jail's authority
//! decision about which paths a program is allowed to reach.

use super::{IpeResult, IpeTask, ok_res, str_err};
// The lexical validation algorithm lives once in the sibling `path_core` module
// (shared with the compiler's `path "…"` gate, which `include!`s the SAME
// `path_core.rs` file via the `ipe_path_core` crate); this module drives it with
// the HOST separator regime so the runtime seal stays target-specific. A sibling
// module (not an extern crate) so it resolves both in the workspace AND when the
// runtime is vendored as `mod ipe_runtime` into an emitted app.
use super::path_core::{
    ElementClass, Volume, clean_with, escapes_root, has_disguised_dotdot, has_nul, is_dos_device,
    is_sep, volume_name_len,
};
use std::path::PathBuf;

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
/// normalise to this): `/` on Unix, `\` on Windows,
const SEP: u8 = sep_of(WINDOWS);

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
/// Clean("")`.
#[must_use]
pub fn path_from_string<E: From<String>>(s: String) -> IpeResult<E, Path> {
    match seal_with(&s, WINDOWS) {
        Ok(cleaned) => IpeResult::Ok(Path(cleaned)),
        Err(why) => IpeResult::Err(why.to_string().into()),
    }
}

/// The seal's decision under an explicit separator regime.
///
/// [`path_from_string`] drives it with the host regime; [`absolute_from`]
/// re-seals the working directory through it, so both refuse identically and
/// the Windows refusals are provable on any host. `Ok` is the cleaned form.
fn seal_with(s: &str, windows: bool) -> Result<String, PathRefusal> {
    if has_nul(s) {
        return Err(PathRefusal::Nul);
    }
    if windows && has_disguised_dotdot(s) {
        // Windows strips trailing dots and spaces from every path element at the
        // syscall, so `".. "` and `"..."` name the parent directory even though
        // the lexical scan sees a literal filename. Reject before `clean` so the
        // disguise can never resolve into a traversal we failed to count.
        return Err(PathRefusal::DisguisedParent {
            path: s.to_string(),
        });
    }
    let cleaned = clean_with(s, windows);
    if escapes_root(&cleaned, windows) {
        return Err(PathRefusal::Escape {
            path: s.to_string(),
            cleaned,
        });
    }
    Ok(cleaned)
}

/// Why a `Path` operation refused its input.
///
/// Every refusal of the seal, [`under_with`] and [`absolute_from`] is one of
/// these; it becomes text only at the Ipê-facing boundary, through its one
/// `Display`.
#[derive(Clone, PartialEq, Eq, Debug)]
enum PathRefusal {
    /// The seal met a NUL byte.
    Nul,
    /// The seal met an element Windows strips to `..`.
    DisguisedParent { path: String },
    /// The seal's cleaned form climbs above its root.
    Escape { path: String, cleaned: String },
    /// `under` refused the child itself.
    Child { child: String, why: ChildRefusal },
    /// `under`'s cleaned join does not lie strictly beneath the root.
    NotBeneath { child: String, root: String },
    /// `absolute` met a working directory that is not valid UTF-8.
    CwdNotUtf8,
    /// `absolute` met a working directory with no complete volume to anchor a
    /// Windows root-relative path.
    CwdNoVolume { cwd: String, path: String },
}

impl std::fmt::Display for PathRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nul => f.write_str(
                "Ipe.Path: path contains a NUL byte (a syscall-boundary truncation / traversal risk)",
            ),
            Self::DisguisedParent { path } => write!(
                f,
                "Ipe.Path: path element resolves to `..` after Windows trailing dot/space \
                 stripping (a traversal disguise): {path:?}"
            ),
            Self::Escape { path, cleaned } => write!(
                f,
                "Ipe.Path: path escapes its root via `..` traversal: {path:?} (cleaned: {cleaned:?})"
            ),
            Self::Child { child, why } => {
                write!(f, "Ipe.Path.under: child path {child:?} {}", why.reason())
            }
            Self::NotBeneath { child, root } => write!(
                f,
                "Ipe.Path.under: {child:?} does not resolve beneath the root {root:?}"
            ),
            Self::CwdNotUtf8 => {
                f.write_str("Ipe.Path.absolute: the working directory is not valid UTF-8")
            }
            Self::CwdNoVolume { cwd, path } => write!(
                f,
                "Ipe.Path.absolute: the working directory {cwd:?} names no complete volume to \
                 anchor {path:?}"
            ),
        }
    }
}

/// Why `under` refused a child on its own, before any containment check.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ChildRefusal {
    /// The root or the child holds a NUL byte.
    Nul,
    /// The child names the root itself (empty or `.`).
    Empty,
    /// The child is rooted or volume-prefixed, so it would replace the root.
    Absolute,
    /// The root is a bare drive (`C:`), which is drive-relative.
    BareDriveRoot,
    /// One raw element of the child may never be joined.
    Element(ElementRefusal),
}

impl ChildRefusal {
    /// The reason text [`PathRefusal`]'s `Display` reports after the child.
    const fn reason(self) -> &'static str {
        match self {
            Self::Nul => "contains a NUL byte",
            Self::Empty => "is empty (it names the root itself)",
            Self::Absolute => "is absolute or volume-prefixed (it would replace the root)",
            Self::BareDriveRoot => "cannot be joined to a bare drive root (drive-relative)",
            Self::Element(e) => e.reason(),
        }
    }
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

/// `Ipe.Path.base : Path -> String` (Unix semantics).
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

/// `Ipe.Path.dir : Path -> String` (Unix semantics).
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

/// `Ipe.Path.ext : Path -> String` (Unix semantics).
/// The suffix from the LAST `.` in the final path element (including the dot),
/// or "" when the final element has no dot. `".bashrc"` → `".bashrc"`.
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

/// `Ipe.Path.isAbsolute : Path -> Bool`: an absolute path begins with `/`.
#[must_use]
pub fn path_is_absolute(p: Path) -> bool {
    p.0.as_bytes().first() == Some(&SEP)
}

/// The element separator the lexical engine emits under a regime.
const fn sep_of(windows: bool) -> u8 {
    if windows { b'\\' } else { b'/' }
}

/// Is `p` rooted (any regime separator right after its volume prefix)?
///
/// Every separator the regime honours counts (`/` AND `\` on Windows), so a
/// raw `/x` is rooted on both. A bare Windows drive (`C:x`) is drive-relative,
/// not rooted; a lone UNC or verbatim prefix is rooted.
fn is_rooted(p: &str, windows: bool) -> bool {
    let vol = Volume::parse(p, windows);
    let len = vol.byte_len();
    p.as_bytes().get(len).is_some_and(|&c| is_sep(c, windows))
        || (!vol.is_drive() && len > 2 && len == p.len())
}

/// Does `p` end in any separator the regime honours?
fn ends_with_sep(p: &str, windows: bool) -> bool {
    p.as_bytes().last().is_some_and(|&c| is_sep(c, windows))
}

/// Does the cleaned `candidate` lie at or below the cleaned `root`?
///
/// Component-wise, never a bare string prefix: `/repo2/x` is NOT under
/// `/repo`. A root that already ends in a separator (`/`, `C:\`) prefixes its
/// children directly.
fn is_within(root: &str, candidate: &str, windows: bool) -> bool {
    if root == "." {
        return !candidate
            .as_bytes()
            .first()
            .is_some_and(|&c| is_sep(c, windows))
            && !is_rooted(candidate, windows)
            && volume_name_len(candidate, windows) == 0
            && !(windows && first_element_has_colon(candidate))
            && !escapes_root(candidate, windows);
    }
    if candidate == root {
        return true;
    }
    candidate.strip_prefix(root).is_some_and(|rest| {
        ends_with_sep(root, windows) || rest.as_bytes().first().is_some_and(|&c| is_sep(c, windows))
    })
}

/// Does the cleaned `joined` lie STRICTLY below the cleaned `root`?
///
/// The independent post-join check: a join that names the root itself, or
/// anything outside it, is refused whatever the pre-join scans concluded.
fn strictly_beneath(root: &str, joined: &str, windows: bool) -> bool {
    joined != root
        && is_within(root, joined, windows)
        && !(windows && (has_stripped_element(root, joined) || has_device_element(root, joined)))
}

/// The part of `joined` below `root` (all of it for the root `.`).
fn below_root<'a>(root: &str, joined: &'a str) -> &'a str {
    if root == "." {
        joined
    } else {
        joined.strip_prefix(root).unwrap_or(joined)
    }
}

/// Does `p`'s first Windows element (up to a `\` or `/`) carry a `:`?
///
/// A raw byte scan that neither cleans nor parses a [`Volume`], so the root-`.`
/// containment check refuses a drive-designated join (`é:\x`, `1:x`) even if
/// the volume grammar ever misses a drive form Win32 honours.
fn first_element_has_colon(p: &str) -> bool {
    p.as_bytes()
        .split(|&b| is_sep(b, true))
        .next()
        .is_some_and(|e| e.contains(&b':'))
}

/// Does the part of the Windows `joined` below `root` hold an element made only
/// of dots and spaces?
///
/// Windows strips trailing dots and spaces from every element, so such an
/// element names `.` or `..` rather than a child of its own — a join ending in
/// `\ ` resolves to the root itself. A raw byte scan, independent of the
/// child-element parser that runs before the join.
fn has_stripped_element(root: &str, joined: &str) -> bool {
    below_root(root, joined)
        .as_bytes()
        .split(|&b| is_sep(b, true))
        .any(|e| !e.is_empty() && e.iter().all(|&c| c == b'.' || c == b' '))
}

/// Does the part of the Windows `joined` below `root` hold a reserved DOS
/// device name?
///
/// Win32 opens the device (`CON`, `nul.txt`) rather than a file beneath the
/// root. Scans the joined result, independent of the child-element parser.
fn has_device_element(root: &str, joined: &str) -> bool {
    below_root(root, joined)
        .as_bytes()
        .split(|&b| is_sep(b, true))
        .any(is_dos_device)
}

/// Why a Windows child element can never be joined beneath a root.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ElementRefusal {
    /// The `..` parent token.
    Parent,
    /// A `:` — a drive designator (`é:`, `1:`) or an alternate data stream
    /// (`a:b`), either of which re-anchors or aliases the element.
    Colon,
    /// Only dots and spaces, other than the exact `.`/`..`: Windows strips
    /// trailing dots and spaces, so it names `.` or `..` (`" "`, `". "`,
    /// `".. "`, `"..."`).
    DotSpaceRun,
    /// A reserved DOS device name (`CON`, `nul.txt`, `COM1`): Win32 opens the
    /// device, not a file beneath the root.
    DosDevice,
}

impl ElementRefusal {
    /// The refusal reason [`under_with`] reports.
    const fn reason(self) -> &'static str {
        match self {
            Self::Parent => "contains a `..` element",
            Self::Colon => "contains a `:` (a drive designator or an alternate data stream)",
            Self::DotSpaceRun => {
                "contains an element made only of dots and spaces (Windows strips it to `.` or `..`)"
            }
            Self::DosDevice => {
                "contains a reserved Windows device name (`CON`, `NUL`, `COM1`, ...) that opens a device"
            }
        }
    }
}

/// One element of a Windows child path, parsed once by [`ChildElement::parse`].
///
/// The child side of [`under_with`] reads every element through this parse,
/// which refuses the forms Win32 resolves to something other than an entry of
/// that name beneath the root: `..`, a `:` (a drive or a stream), a dot/space
/// run (`.` or `..` once stripped), and a reserved DOS device. An accepted name
/// may still alias a sibling name in the same directory (a stripped trailing
/// `.` or space, an 8.3 short name); that alias stays beneath the root.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ChildElement {
    /// An empty element (a doubled separator), dropped by cleaning.
    Empty,
    /// The exact `.` token, dropped by cleaning.
    Current,
    /// A plain name that stays a name after Windows canonicalisation.
    Name,
}

impl ChildElement {
    /// Classify one raw element, or say why it may never be joined.
    fn parse(e: &[u8]) -> Result<Self, ElementRefusal> {
        match ElementClass::of(e) {
            ElementClass::Empty => Ok(Self::Empty),
            ElementClass::Current => Ok(Self::Current),
            ElementClass::Name => Ok(Self::Name),
            ElementClass::Parent => Err(ElementRefusal::Parent),
            ElementClass::Colon => Err(ElementRefusal::Colon),
            ElementClass::DisguisedParent | ElementClass::DisguisedCurrent => {
                Err(ElementRefusal::DotSpaceRun)
            }
            ElementClass::DosDevice => Err(ElementRefusal::DosDevice),
        }
    }
}

/// Why `child` may not be joined beneath any root, judged on its RAW text.
///
/// Runs before any cleaning, so a `..` element or a Windows dot/space disguise
/// is refused even when cleaning would have folded it into an in-bounds form.
fn raw_child_refusal(c: &str, windows: bool) -> Option<ChildRefusal> {
    let mut elements = c.as_bytes().split(|&b| is_sep(b, windows));
    let element_refusal = if windows {
        elements.find_map(|e| ChildElement::parse(e).err())
    } else {
        elements
            .any(|e| ElementClass::of(e) == ElementClass::Parent)
            .then_some(ElementRefusal::Parent)
    };
    element_refusal.map(ChildRefusal::Element).or_else(|| {
        (is_rooted(c, windows) || volume_name_len(c, windows) > 0).then_some(ChildRefusal::Absolute)
    })
}

/// Why the CLEANED `child` may not be joined, re-checking the raw verdict.
fn clean_child_refusal(c: &str, windows: bool) -> Option<ChildRefusal> {
    if c == "." {
        Some(ChildRefusal::Empty)
    } else if is_rooted(c, windows) || volume_name_len(c, windows) > 0 {
        Some(ChildRefusal::Absolute)
    } else if escapes_root(c, windows) {
        Some(ChildRefusal::Element(ElementRefusal::Parent))
    } else {
        None
    }
}

/// Join `child` beneath `root` under a separator regime.
///
/// Split from [`path_under`] so the Windows refusals are proven on any host.
/// Neither input is trusted to be clean: the raw child is scanned first, then
/// both are cleaned under the regime (a raw root `""` becomes `.`), and the
/// join is always cleaned. `Err` carries the reason the join was refused.
fn under_with(r: &str, c: &str, windows: bool) -> Result<String, PathRefusal> {
    let refused = |why: ChildRefusal| {
        Err(PathRefusal::Child {
            child: c.to_string(),
            why,
        })
    };
    if has_nul(r) || has_nul(c) {
        return refused(ChildRefusal::Nul);
    }
    if c.is_empty() {
        return refused(ChildRefusal::Empty);
    }
    if let Some(why) = raw_child_refusal(c, windows) {
        return refused(why);
    }
    let cc = clean_with(c, windows);
    if let Some(why) = clean_child_refusal(&cc, windows) {
        return refused(why);
    }
    let rr = clean_with(r, windows);
    let root_vol = volume_name_len(&rr, windows);
    if root_vol > 0 && root_vol == rr.len() && !is_rooted(&rr, windows) {
        return refused(ChildRefusal::BareDriveRoot);
    }
    let joined = if rr == "." {
        cc
    } else if ends_with_sep(&rr, windows) {
        clean_with(&format!("{rr}{cc}"), windows)
    } else {
        clean_with(&format!("{rr}{}{cc}", char::from(sep_of(windows))), windows)
    };
    if !strictly_beneath(&rr, &joined, windows) {
        return Err(PathRefusal::NotBeneath {
            child: c.to_string(),
            root: rr,
        });
    }
    Ok(joined)
}

/// `Ipe.Path.under : Path -> Path -> Result Error Path` — join `child` beneath `root`.
///
/// THE typed path-composition operation: it replaces every `root ++ "/" ++ x`
/// string concatenation. Fails closed (`Err`) when `child` is empty (`.`),
/// rooted or volume-prefixed (an absolute child would replace the root), holds
/// any `..` element or Windows dot/space disguise, or carries a NUL byte; and,
/// as an independent second check on the joined result, when the cleaned join
/// does not lie component-wise strictly below `root`. Also refuses a bare
/// Windows drive root (`C:`), whose join would silently re-anchor a
/// drive-relative root at the drive root.
///
/// Containment is LEXICAL: a symlink below `root` may still point outside it
/// (see the module's trust model).
#[must_use]
pub fn path_under<E: From<String>>(root: Path, child: Path) -> IpeResult<E, Path> {
    match under_with(root.as_str(), child.as_str(), WINDOWS) {
        Ok(joined) => IpeResult::Ok(Path(joined)),
        Err(why) => IpeResult::Err(why.to_string().into()),
    }
}

/// Is `p` anchored on its own, needing no working directory to resolve?
///
/// Unix: any rooted path. Windows: a rooted path that ALSO names its volume
/// (`C:\x`, `\\srv\shr\x`); a root-relative `\x` still depends on the current
/// drive.
fn is_self_anchored(p: &str, windows: bool) -> bool {
    is_rooted(p, windows) && (!windows || volume_name_len(p, windows) > 0)
}

/// Resolve `p` against the working directory `cwd` under a separator regime.
///
/// Split from [`path_absolute`] so the refusals (a non-UTF-8 `cwd`, a
/// drive-relative `p`) and the Windows root-relative anchoring are proven on
/// any host. A self-anchored path is returned sealed; a Windows root-relative
/// `\x` is anchored on the working directory's volume; a relative path is
/// joined beneath `cwd` through [`under_with`], inheriting every refusal.
fn absolute_from(cwd: PathBuf, p: &str, windows: bool) -> Result<String, PathRefusal> {
    if is_self_anchored(p, windows) {
        return seal_with(p, windows);
    }
    let Ok(cwd) = cwd.into_os_string().into_string() else {
        return Err(PathRefusal::CwdNotUtf8);
    };
    let cwd = seal_with(&cwd, windows)?;
    if is_rooted(p, windows) {
        // Windows root-relative (`\x`): rooted on the CURRENT drive, so anchor
        // it on the working directory's volume instead of returning it
        // drive-ambiguous.
        let vol = Volume::parse(&cwd, windows);
        if !vol.anchors() {
            return Err(PathRefusal::CwdNoVolume {
                cwd,
                path: p.to_string(),
            });
        }
        let prefix = cwd.get(..vol.byte_len()).unwrap_or("");
        return seal_with(&format!("{prefix}{p}"), windows);
    }
    if clean_with(p, windows) == "." {
        return Ok(cwd);
    }
    under_with(&cwd, p, windows)
}

/// `Ipe.Path.absolute : Path -> Task Error Path` — resolve a path against the working directory.
///
/// A volume-rooted path is returned unchanged; a Windows root-relative `\x`
/// is anchored on the working directory's volume; a relative one is joined
/// beneath the process working directory through [`path_under`]'s join, so it
/// inherits every refusal. Fails closed on a working directory that is not
/// valid UTF-8 (never a lossy rewrite that would name a different directory)
/// or that the seal rejects. Lexical only: symlinks are not resolved.
#[must_use]
pub fn path_absolute<E: Send + From<String> + 'static>(p: Path) -> IpeTask<E, Path> {
    Box::pin(async move {
        let cwd = if is_self_anchored(p.as_str(), WINDOWS) {
            PathBuf::new()
        } else {
            match std::env::current_dir() {
                Ok(d) => d,
                Err(e) => return IpeResult::Err(str_err(&format!("Ipe.Path.absolute: {e}"))),
            }
        };
        match absolute_from(cwd, p.as_str(), WINDOWS) {
            Ok(abs) => ok_res(Path(abs)),
            Err(why) => IpeResult::Err(str_err(&why.to_string())),
        }
    })
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
        // `Clean` stops `..` at the root, so a rooted path is always safe.
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
    fn unix_clean_dotdot_corpus() {
        // `clean_with(_, false)` correctness for dotdot traversal paths.
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
                Err(super::super::path_core::PathRejection::Traversal),
                "compile-time gate must reject the Windows traversal vector {vector:?}"
            );
        }
    }

    // ── composition: `under` joins beneath a root, refusals fail closed ─────

    /// Seal `s` into a `Path`, or `None` when the seal refuses it.
    fn seal(s: &str) -> Option<Path> {
        match path_from_string::<String>(s.to_string()) {
            IpeResult::Ok(p) => Some(p),
            IpeResult::Err(_) => None,
        }
    }

    /// `under root child` over sealed strings; `None` when a seal or the join refuses.
    fn join(root: &str, child: &str) -> Option<String> {
        let (r, c) = (seal(root)?, seal(child)?);
        match path_under::<String>(r, c) {
            IpeResult::Ok(p) => Some(path_to_string(p)),
            IpeResult::Err(_) => None,
        }
    }

    /// The Unix-regime join over UNSEALED strings, proving the join's own
    /// refusals hold even for a value that never passed the seal.
    fn unix_raw(root: &str, child: &str) -> Option<String> {
        under_with(root, child, false).ok()
    }

    /// The Windows-regime join over UNSEALED strings (proven on any host).
    fn win_raw(root: &str, child: &str) -> Option<String> {
        under_with(root, child, true).ok()
    }

    #[cfg(not(windows))]
    #[test]
    fn under_joins_a_relative_child_beneath_the_root() {
        assert_eq!(
            join("/repo", "src/Main.ipe").as_deref(),
            Some("/repo/src/Main.ipe")
        );
        assert_eq!(join("repo", "a/b").as_deref(), Some("repo/a/b"));
        assert_eq!(join(".", "a/b").as_deref(), Some("a/b"));
        assert_eq!(join("/repo", "a/./b//c").as_deref(), Some("/repo/a/b/c"));
    }

    #[cfg(not(windows))]
    #[test]
    fn under_handles_trailing_separators_on_root_and_child() {
        assert_eq!(join("/repo/", "a/").as_deref(), Some("/repo/a"));
        assert_eq!(join("/", "a").as_deref(), Some("/a"));
        assert_eq!(unix_raw("/repo/", "a").as_deref(), Some("/repo/a"));
    }

    #[test]
    fn under_refuses_a_dotdot_escape() {
        // The seal already refuses a leading-`..` child ...
        assert_eq!(join("/repo", "../etc/passwd"), None);
        // ... and the join refuses it again on its own, under both regimes.
        for child in ["../etc/passwd", "a/../../x", "..", "a/.."] {
            assert_eq!(unix_raw("/repo", child), None, "{child:?}");
            assert_eq!(win_raw("C:\\repo", child), None, "{child:?}");
        }
        assert_eq!(win_raw("C:\\repo", "a\\..\\..\\x"), None);
    }

    #[test]
    fn under_refuses_an_absolute_child() {
        assert_eq!(join("/repo", "/etc/passwd"), None);
        assert_eq!(join("/repo", "/"), None);
        assert_eq!(unix_raw("/repo", "/etc"), None);
        assert_eq!(win_raw("C:\\repo", "\\etc"), None, "rooted");
        assert_eq!(win_raw("C:\\repo", "D:\\etc"), None, "drive-absolute");
        assert_eq!(win_raw("C:\\repo", "D:etc"), None, "drive-relative");
        assert_eq!(win_raw("C:\\repo", "\\\\srv\\shr\\x"), None, "UNC");
    }

    #[test]
    fn under_refuses_an_empty_child() {
        assert_eq!(join("/repo", ""), None);
        assert_eq!(join("/repo", "."), None);
        assert_eq!(join("/repo", "a/.."), None);
        assert_eq!(unix_raw("/repo", ""), None);
        assert_eq!(win_raw("C:\\repo", "."), None);
    }

    #[test]
    fn under_refuses_a_nul_byte() {
        assert_eq!(unix_raw("/repo", "a\0b"), None);
        assert_eq!(unix_raw("/re\0po", "a"), None);
        assert_eq!(win_raw("C:\\repo", "a\0b"), None);
    }

    #[test]
    fn containment_is_component_wise_not_a_string_prefix() {
        assert!(!is_within("/repo", "/repo2/x", false), "prefix confusion");
        assert!(!is_within("/repo", "/repox", false), "prefix confusion");
        assert!(is_within("/repo", "/repo/x", false));
        assert!(is_within("/", "/x", false));
        assert!(!is_within(".", "/x", false));
        assert!(!is_within(".", "../x", false));
        assert!(
            !is_within("C:\\repo", "C:\\repo2\\x", true),
            "prefix confusion"
        );
        assert!(is_within("C:\\", "C:\\x", true));
        // A join never yields a sibling that merely shares the root's prefix.
        assert_eq!(unix_raw("/repo", "2/x").as_deref(), Some("/repo/2/x"));
    }

    #[test]
    fn windows_regime_joins_and_refuses_a_bare_drive_root() {
        assert_eq!(
            win_raw("C:\\repo", "a\\b").as_deref(),
            Some("C:\\repo\\a\\b")
        );
        assert_eq!(
            win_raw("C:\\repo", "a/b").as_deref(),
            Some("C:\\repo\\a\\b")
        );
        assert_eq!(win_raw("C:\\", "a").as_deref(), Some("C:\\a"));
        assert_eq!(
            win_raw("\\\\srv\\shr", "a").as_deref(),
            Some("\\\\srv\\shr\\a")
        );
        // `C:` + `a` must not silently become the drive-rooted `C:\a`.
        assert_eq!(win_raw("C:", "a"), None);
    }

    #[test]
    fn under_refusal_arrives_on_the_error_channel() {
        let r = path_under::<String>(Path("/repo".to_string()), Path("/etc".to_string()));
        assert!(
            matches!(&r, IpeResult::Err(e) if e.contains("absolute")),
            "{r:?}"
        );
    }

    #[test]
    fn under_treats_every_windows_separator_as_rooting() {
        // A `/`-rooted child replaces the root on Windows too: never the drive root.
        assert_eq!(win_raw(".", "/x"), None, "escape to the drive root");
        assert_eq!(win_raw(".", "\\x"), None);
        assert_eq!(win_raw("C:\\repo", "/etc"), None, "`/` roots on Windows");
        assert_eq!(win_raw("C:\\repo", "//srv/shr/x"), None, "`/`-spelled UNC");
        assert!(is_rooted("/x", true) && is_rooted("\\x", true));
        assert!(ends_with_sep("C:/", true) && ends_with_sep("C:\\", true));
        assert!(
            !ends_with_sep("a\\", false),
            "`\\` is a filename byte on Unix"
        );
    }

    #[test]
    fn under_cleans_a_raw_root_before_joining() {
        // A raw empty root is `.`, never re-anchored at `/`.
        assert_eq!(unix_raw("", "a").as_deref(), Some("a"));
        assert_eq!(win_raw("", "a").as_deref(), Some("a"));
        assert_eq!(unix_raw(".", "./a/").as_deref(), Some("a"));
        assert_eq!(win_raw(".", "a/b").as_deref(), Some("a\\b"));
    }

    #[test]
    fn under_accepts_valid_joins_beneath_an_unclean_root() {
        assert_eq!(win_raw("C:/repo", "a").as_deref(), Some("C:\\repo\\a"));
        assert_eq!(
            win_raw("C:/repo/", "a/b").as_deref(),
            Some("C:\\repo\\a\\b")
        );
        assert_eq!(win_raw("src/a", "b").as_deref(), Some("src\\a\\b"));
        assert_eq!(unix_raw("./a", "b").as_deref(), Some("a/b"));
        assert_eq!(unix_raw("/repo//x/", "b").as_deref(), Some("/repo/x/b"));
        assert_eq!(unix_raw("src/a", "b").as_deref(), Some("src/a/b"));
    }

    #[test]
    fn under_refuses_a_windows_dot_space_disguise() {
        for child in [".. \\x", "a\\...\\x", "a/. ./x", "...", "a\\.. "] {
            assert_eq!(win_raw("C:\\repo", child), None, "{child:?}");
        }
    }

    #[test]
    fn under_refuses_verbatim_and_device_children() {
        for child in [
            "\\\\?\\C:\\x",
            "\\\\.\\x",
            "//?/C:/x",
            "\\\\.\\PhysicalDrive0",
        ] {
            assert_eq!(win_raw("C:\\repo", child), None, "{child:?}");
        }
    }

    #[test]
    fn under_refuses_a_join_that_names_the_root_itself() {
        assert_eq!(unix_raw("/repo", "./"), None);
        assert_eq!(win_raw("C:\\repo", ".\\"), None);
        // The post-join check refuses the root itself and any sibling on its own.
        assert!(!strictly_beneath("/repo", "/repo", false));
        assert!(!strictly_beneath("/repo", "/repo2", false));
        assert!(!strictly_beneath("C:\\repo", "C:\\repo", true));
        assert!(strictly_beneath("/repo", "/repo/a", false));
    }

    // ── `absolute` resolves against the working directory ──────────────────

    /// Poll a ready-at-first-poll `IpeTask` once, without an executor.
    fn run_now<A>(mut t: IpeTask<String, A>) -> Option<IpeResult<String, A>> {
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match t.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(r) => Some(r),
            std::task::Poll::Pending => None,
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn absolute_keeps_a_rooted_path_and_roots_a_relative_one() {
        let rooted = seal("/etc/x").map(|p| run_now(path_absolute::<String>(p)));
        assert!(
            matches!(&rooted, Some(Some(IpeResult::Ok(p))) if p.as_str() == "/etc/x"),
            "{rooted:?}"
        );
        let rel = seal("a/b").map(|p| run_now(path_absolute::<String>(p)));
        assert!(
            matches!(&rel, Some(Some(IpeResult::Ok(p)))
                if is_rooted(p.as_str(), WINDOWS) && p.as_str().ends_with("/a/b")),
            "{rel:?}"
        );
        let dot = seal(".").map(|p| run_now(path_absolute::<String>(p)));
        assert!(
            matches!(&dot, Some(Some(IpeResult::Ok(p))) if is_rooted(p.as_str(), WINDOWS)),
            "{dot:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn absolute_refuses_a_non_utf8_working_directory() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let cwd = PathBuf::from(OsString::from_vec(vec![0xff]));
        let r = absolute_from(cwd, "a", false);
        assert_eq!(r, Err(PathRefusal::CwdNotUtf8));
    }

    #[test]
    fn absolute_resolves_under_both_regimes() {
        let unix = |p: &str| absolute_from(PathBuf::from("/work"), p, false);
        assert_eq!(unix("/etc/x"), Ok("/etc/x".to_string()));
        assert_eq!(unix("a/b"), Ok("/work/a/b".to_string()));
        assert_eq!(unix("."), Ok("/work".to_string()));
        let win = |p: &str| absolute_from(PathBuf::from("C:\\work"), p, true);
        assert_eq!(win("D:\\x"), Ok("D:\\x".to_string()));
        assert_eq!(win("a/b"), Ok("C:\\work\\a\\b".to_string()));
        // A `/`-rooted path is rooted on Windows, never joined as relative.
        assert_eq!(win("/x"), Ok("C:\\x".to_string()));
    }

    #[test]
    fn absolute_anchors_a_windows_root_relative_path_on_the_cwd_volume() {
        let at = |cwd: &str, p: &str| absolute_from(PathBuf::from(cwd), p, true);
        assert_eq!(at("C:\\work", "\\x"), Ok("C:\\x".to_string()));
        assert_eq!(
            at("\\\\srv\\shr\\work", "\\x"),
            Ok("\\\\srv\\shr\\x".to_string())
        );
        // A cwd with no volume cannot anchor it: refused, never left ambiguous.
        assert!(at("\\work", "\\x").is_err());
    }

    #[test]
    fn absolute_refuses_a_drive_relative_or_escaping_path() {
        let win = |p: &str| absolute_from(PathBuf::from("C:\\work"), p, true);
        assert!(win("D:x").is_err(), "drive-relative");
        assert!(win("..\\x").is_err());
        assert!(win(".. \\x").is_err(), "disguise");
        let unix = absolute_from(PathBuf::from("/work"), "../x", false);
        assert!(unix.is_err(), "{unix:?}");
    }

    #[test]
    fn absolute_refuses_an_escaping_unsealed_child() {
        let r = run_now(path_absolute::<String>(Path("../x".to_string())));
        assert!(matches!(r, Some(IpeResult::Err(_))), "{r:?}");
    }

    #[test]
    fn win_under_refuses_a_colon_or_dot_space_run_child() {
        for child in [
            "é:\\x", "1:x", "a:b", "x\\a:b", " ", ". ", "a\\. ", ".. ", "...",
        ] {
            assert_eq!(win_raw(".", child), None, "root `.` joined {child:?}");
            assert_eq!(
                win_raw("C:\\repo", child),
                None,
                "`C:\\repo` joined {child:?}"
            );
        }
        // A plain name with an inner dot or space is still a child.
        assert_eq!(
            win_raw("C:\\repo", "a b\\c.d").as_deref(),
            Some("C:\\repo\\a b\\c.d")
        );
    }

    #[test]
    fn win_containment_post_checks_hold_without_the_child_parse() {
        // The root-`.` check refuses a drive-designated candidate on its own.
        assert!(!is_within(".", "é:\\x", true));
        assert!(!is_within(".", "1:x", true));
        // Drive designators the volume parser never names: a non-BMP letter
        // and a multi-letter prefix (an alternate data stream).
        assert!(!is_within(".", "𝒳:x", true));
        assert!(!is_within(".", "ab:c", true));
        // A leading separator is refused by its raw first byte alone.
        assert!(!is_within(".", "/x", false));
        assert!(!is_within(".", "\\x", true));
        assert!(!is_within(".", "/x", true));
        assert!(is_within(".", "x", true));
        // A reserved device element below the root is refused on its own.
        assert!(!strictly_beneath("C:\\repo", "C:\\repo\\a\\CON", true));
        assert!(!strictly_beneath(".", "nul.txt", true));
        assert!(strictly_beneath("C:\\repo", "C:\\repo\\CONSOLE", true));
        // A trailing dot/space-only element names the root, never beneath it.
        assert!(!strictly_beneath("C:\\repo", "C:\\repo\\ ", true));
        assert!(!strictly_beneath(".", ". ", true));
        // A lone non-ASCII drive is drive-relative, never a rooted root.
        assert!(!is_rooted("é:", true));
    }

    #[test]
    fn win_under_refuses_a_dos_device_child() {
        for child in [
            "CON",
            "con.txt",
            "NUL ",
            "COM1",
            "COM\u{b9}",
            "CONOUT$",
            "aux.tar.gz",
            "a\\CON",
        ] {
            assert_eq!(win_raw(".", child), None, "root `.` joined {child:?}");
            assert_eq!(
                win_raw("C:\\uploads", child),
                None,
                "`C:\\uploads` joined {child:?}"
            );
        }
        for child in ["CONSOLE", "COM10", "nulx"] {
            assert_eq!(
                win_raw("C:\\uploads", child),
                Some(format!("C:\\uploads\\{child}")),
                "{child:?}"
            );
        }
        // Device names are a Win32 namespace; the Unix regime joins them.
        assert_eq!(unix_raw("/repo", "CON").as_deref(), Some("/repo/CON"));
    }

    #[test]
    fn child_parse_and_device_scan_agree_with_the_shared_classifier() {
        for e in [
            "",
            ".",
            "..",
            "...",
            ". ",
            " ",
            "a",
            "a.",
            "a:b",
            "CON",
            "nul.txt",
            "COM\u{b9}",
            "COM10",
        ] {
            let class = ElementClass::of(e.as_bytes());
            assert_eq!(
                ChildElement::parse(e.as_bytes()) == Err(ElementRefusal::DosDevice),
                class == ElementClass::DosDevice,
                "{e:?}"
            );
            assert_eq!(
                is_dos_device(e.as_bytes()),
                class == ElementClass::DosDevice,
                "{e:?}"
            );
        }
    }

    #[test]
    fn refusals_render_their_boundary_text() {
        assert_eq!(
            PathRefusal::Nul.to_string(),
            "Ipe.Path: path contains a NUL byte (a syscall-boundary truncation / traversal risk)"
        );
        assert_eq!(
            under_with("C:\\uploads", "CON", true)
                .err()
                .map(|e| e.to_string())
                .as_deref(),
            Some(
                "Ipe.Path.under: child path \"CON\" contains a reserved Windows device name \
                 (`CON`, `NUL`, `COM1`, ...) that opens a device"
            )
        );
        assert_eq!(
            under_with("/repo", "../x", false)
                .err()
                .map(|e| e.to_string())
                .as_deref(),
            Some("Ipe.Path.under: child path \"../x\" contains a `..` element")
        );
        assert_eq!(
            seal_with("../x", false)
                .err()
                .map(|e| e.to_string())
                .as_deref(),
            Some(
                "Ipe.Path: path escapes its root via `..` traversal: \"../x\" (cleaned: \"../x\")"
            )
        );
    }

    #[test]
    fn absolute_anchors_on_a_verbatim_unc_share_and_refuses_a_device_unc() {
        let at = |cwd: &str, p: &str| absolute_from(PathBuf::from(cwd), p, true);
        assert_eq!(
            at("\\\\?\\UNC\\srv\\shr\\work", "\\x"),
            Ok("\\\\?\\UNC\\srv\\shr\\x".to_string())
        );
        assert!(at("\\\\.\\UNC\\srv\\shr\\work", "\\x").is_err());
        assert!(at("\\\\srv", "\\x").is_err(), "UNC with no share");
    }
}
