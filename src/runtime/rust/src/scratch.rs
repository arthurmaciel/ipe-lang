//! Private scratch creation, the one primitive every temporary path goes through.
//!
//! This file is the single source of truth for both sides of the toolchain: the
//! runtime compiles it as `scratch`, and the compiler's `ipe_sandbox` compiles
//! this exact file through `#[path]`. It therefore names no crate-local module:
//! the entropy source and the profile lookup are passed in by each caller.
//!
//! A scratch path is handed to writers that re-resolve it by name, so it is only
//! safe while no other user can read, predict, or replace any component of it.
//! Every constructor therefore establishes:
//!
//! - the base is resolved and proven trusted: on Unix it is canonicalised and
//!   every ancestor is a real directory owned by the effective user or root,
//!   writable by no one else unless sticky; elsewhere, where the standard
//!   library offers no owner-only creation, it must resolve inside the current
//!   user's profile directory, which the OS keeps private to that user;
//! - inside a user namespace an ancestor owned by an unmapped uid shows the
//!   overflow uid, which proves nothing, so a jail never re-derives the proof:
//!   its launcher proves the scratch directory on the host, where owners are
//!   real, and hands it down as an open descriptor ([`ScratchAnchor`]); the
//!   walk stops at that node once the descriptor is shown to hold it, and an
//!   absent or unproven anchor leaves the full walk in force;
//! - entries are created under the resolved base, never the given path, so a
//!   link on the given path re-pointed after the check cannot redirect them;
//! - a created name carries 128 bits of CSPRNG entropy (an unavailable CSPRNG
//!   fails the creation, never weakens the name); it is created exclusively, a
//!   collision retries with a fresh name a bounded number of times, and the
//!   entry is re-verified: a real directory (mode 0700) or regular file (mode
//!   0600), owned by the effective user, no group/other permission bits;
//! - a file is opened with `O_EXCL` + `O_NOFOLLOW`, and its handle is verified
//!   with `fstat`;
//! - an entry inside a private directory is named by a [`ScratchLeaf`], which
//!   is exactly one plain path component.
//!
//! A check that fails refuses with [`io::ErrorKind::PermissionDenied`] carrying
//! a [`ScratchError`] or a [`ScratchRootRefusal`]; nothing is created under an
//! untrusted base and nothing is written through a planted link.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Attempts at a fresh name before creation gives up.
const MAX_ATTEMPTS: usize = 8;

/// Bytes of CSPRNG entropy in every scratch name (128 bits).
const ENTROPY_BYTES: usize = 16;

/// Longest caller label kept in a scratch name.
const MAX_LABEL_CHARS: usize = 64;

/// Longest leaf name, in bytes: the common filesystem limit on one component.
const MAX_LEAF_BYTES: usize = 255;

/// The label used when a caller label confines to nothing.
const FALLBACK_LABEL: &str = "scratch";

/// The variable naming the current user's profile directory on Windows.
pub const PROFILE_VAR: &str = "USERPROFILE";

/// Why a scratch location was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScratchRefusal {
    /// The entry is not a real directory (a symlink, a file, or another type).
    NotADirectory,
    /// The entry is not a regular file.
    NotARegularFile,
    /// The entry is owned by a user other than the effective user (or root, for a base).
    ForeignOwner,
    /// The entry is writable by other users without the sticky bit.
    WritableByOthers,
    /// A private entry grants group or other permission bits.
    NotPrivate,
}

impl fmt::Display for ScratchRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotADirectory => "not a real directory",
            Self::NotARegularFile => "not a regular file",
            Self::ForeignOwner => "owned by another user",
            Self::WritableByOthers => "writable by other users without the sticky bit",
            Self::NotPrivate => "grants group or other permissions",
        })
    }
}

impl std::error::Error for ScratchRefusal {}

/// A refused scratch location: the path and the reason.
#[derive(Debug)]
pub struct ScratchError {
    /// The path that failed verification.
    pub path: PathBuf,
    /// Why it was refused.
    pub refusal: ScratchRefusal,
}

impl fmt::Display for ScratchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "refusing scratch location {}: {}",
            self.path.display(),
            self.refusal
        )
    }
}

impl std::error::Error for ScratchError {}

/// Wrap a refusal of `path` as a `PermissionDenied` I/O error.
fn refused(path: &Path, refusal: ScratchRefusal) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        ScratchError {
            path: path.to_path_buf(),
            refusal,
        },
    )
}

/// Why a scratch root cannot be proven private to the current user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScratchRootRefusal {
    /// No absolute profile directory is set, so no root can be proven private.
    NoProfile,
    /// A path could not be resolved to its canonical form.
    Unresolvable {
        /// The path that failed to resolve.
        path: PathBuf,
        /// The rendered OS error.
        detail: String,
    },
    /// The root resolves outside the profile directory.
    OutsideProfile {
        /// The canonical scratch root.
        root: PathBuf,
        /// The canonical profile directory.
        profile: PathBuf,
    },
}

/// The remedy every root refusal names.
const FIX: &str = "set TEMP and TMP to a directory inside your user profile, \
                   such as %LOCALAPPDATA%\\Temp (the Windows default)";

impl fmt::Display for ScratchRootRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProfile => write!(
                f,
                "refusing to create scratch files: {PROFILE_VAR} does not name an absolute \
                 directory, so the scratch location cannot be proven private to you; {FIX}"
            ),
            Self::Unresolvable { path, detail } => write!(
                f,
                "refusing to create scratch files: cannot resolve {} ({detail}), so it cannot \
                 be proven private to you; {FIX}",
                path.display()
            ),
            Self::OutsideProfile { root, profile } => write!(
                f,
                "refusing to create scratch files under {}: it lies outside your user profile \
                 ({}), so other local users may read or modify what is written there; {FIX}",
                root.display(),
                profile.display()
            ),
        }
    }
}

impl std::error::Error for ScratchRootRefusal {}

impl From<ScratchRootRefusal> for io::Error {
    fn from(refusal: ScratchRootRefusal) -> Self {
        Self::new(io::ErrorKind::PermissionDenied, refusal)
    }
}

/// Every one of the [`MAX_ATTEMPTS`] fresh names already existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamesExhausted;

impl fmt::Display for NamesExhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not create a unique scratch entry after {MAX_ATTEMPTS} attempts"
        )
    }
}

impl std::error::Error for NamesExhausted {}

// ── ScratchLeaf ──────────────────────────────────────────────────────────────

/// Why a name was refused as a [`ScratchLeaf`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafRefusal {
    /// The name is empty.
    Empty,
    /// The name is longer than one filesystem component allows.
    TooLong,
    /// The name holds a path separator (`/`, `\`) or a stream separator (`:`).
    Separator,
    /// The name holds a NUL byte.
    Nul,
    /// The name is `.` or `..`, which name an existing directory.
    DotName,
    /// The name does not parse as exactly one plain path component.
    NotOneComponent,
}

impl fmt::Display for LeafRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "a scratch entry name must not be empty",
            Self::TooLong => "a scratch entry name must be at most 255 bytes",
            Self::Separator => "a scratch entry name must not contain '/', '\\' or ':'",
            Self::Nul => "a scratch entry name must not contain a NUL byte",
            Self::DotName => "a scratch entry name must not be '.' or '..'",
            Self::NotOneComponent => "a scratch entry name must be exactly one path component",
        })
    }
}

impl std::error::Error for LeafRefusal {}

impl From<LeafRefusal> for io::Error {
    fn from(refusal: LeafRefusal) -> Self {
        Self::new(io::ErrorKind::InvalidInput, refusal)
    }
}

/// The name of one entry directly inside a private scratch directory.
///
/// It is exactly one plain path component, so joining it onto a private
/// directory can never leave that directory, name the directory itself, or
/// reach an alternate data stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScratchLeaf(String);

impl ScratchLeaf {
    /// Parse `name` as a single plain path component.
    ///
    /// # Errors
    /// The [`LeafRefusal`] naming the first violated condition.
    pub fn new(name: &str) -> Result<Self, LeafRefusal> {
        if name.is_empty() {
            return Err(LeafRefusal::Empty);
        }
        if name.len() > MAX_LEAF_BYTES {
            return Err(LeafRefusal::TooLong);
        }
        if name.contains(['/', '\\', ':']) {
            return Err(LeafRefusal::Separator);
        }
        if name.contains('\0') {
            return Err(LeafRefusal::Nul);
        }
        if name == "." || name == ".." {
            return Err(LeafRefusal::DotName);
        }
        let mut components = Path::new(name).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(one)), None) if one == OsStr::new(name) => {
                Ok(Self(name.to_owned()))
            }
            _ => Err(LeafRefusal::NotOneComponent),
        }
    }

    /// The leaf name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<Path> for ScratchLeaf {
    fn as_ref(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl fmt::Display for ScratchLeaf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ── Verdicts ─────────────────────────────────────────────────────────────────

/// The ownership and permission facts a verdict needs about one filesystem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryFacts {
    /// The entry's file type, as seen without following a final symlink.
    pub kind: EntryKind,
    /// Owning uid.
    pub owner: u32,
    /// Owning gid.
    pub group: u32,
    /// Full mode bits (permission, sticky, setuid/setgid).
    pub mode: u32,
}

/// The file type of an entry, as seen without following a final symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A real directory.
    Directory,
    /// A regular file.
    File,
    /// Anything else, a symlink included.
    Other,
}

/// The effective identity scratch entries must belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// Effective uid.
    pub euid: u32,
    /// Effective gid.
    pub egid: u32,
}

const ROOT_UID: u32 = 0;
const STICKY: u32 = 0o1000;
const GROUP_WRITE: u32 = 0o020;
const OTHER_WRITE: u32 = 0o002;
const GROUP_OTHER_BITS: u32 = 0o077;

/// Whether `entry` may be an ancestor of (or be) the base a private entry is created under.
///
/// The base must be a directory owned by the effective user or root. A world-
/// writable base must be sticky; a group-writable one must be sticky or be the
/// effective user's own directory with the effective group (a user-private group).
///
/// # Errors
/// The [`ScratchRefusal`] naming the first violated condition.
pub const fn base_verdict(entry: EntryFacts, who: Identity) -> Result<(), ScratchRefusal> {
    if !matches!(entry.kind, EntryKind::Directory) {
        return Err(ScratchRefusal::NotADirectory);
    }
    if entry.owner != who.euid && entry.owner != ROOT_UID {
        return Err(ScratchRefusal::ForeignOwner);
    }
    let sticky = entry.mode & STICKY != 0;
    if entry.mode & OTHER_WRITE != 0 && !sticky {
        return Err(ScratchRefusal::WritableByOthers);
    }
    let own_private_group = entry.owner == who.euid && entry.group == who.egid;
    if entry.mode & GROUP_WRITE != 0 && !sticky && !own_private_group {
        return Err(ScratchRefusal::WritableByOthers);
    }
    Ok(())
}

/// Whether `entry` is a private scratch entry of the expected `kind`.
///
/// It must be exactly that kind (never a symlink), owned by the effective user,
/// and grant no group or other permission bits.
///
/// # Errors
/// The [`ScratchRefusal`] naming the first violated condition.
pub const fn private_verdict(
    entry: EntryFacts,
    kind: EntryKind,
    who: Identity,
) -> Result<(), ScratchRefusal> {
    match (entry.kind, kind) {
        (EntryKind::Directory, EntryKind::Directory) | (EntryKind::File, EntryKind::File) => {}
        (EntryKind::Directory | EntryKind::Other, EntryKind::File) => {
            return Err(ScratchRefusal::NotARegularFile);
        }
        (
            EntryKind::Directory | EntryKind::File | EntryKind::Other,
            EntryKind::Directory | EntryKind::Other,
        ) => return Err(ScratchRefusal::NotADirectory),
    }
    if entry.owner != who.euid {
        return Err(ScratchRefusal::ForeignOwner);
    }
    if entry.mode & GROUP_OTHER_BITS != 0 {
        return Err(ScratchRefusal::NotPrivate);
    }
    Ok(())
}

// ── Inherited anchor ─────────────────────────────────────────────────────────

/// The variable a jail launcher sets to hand its jailed process a host-proven scratch anchor.
///
/// Its value is `<fd>:<dev>:<ino>`, all decimal; see [`ScratchAnchor`].
pub const ANCHOR_VAR: &str = "IPE_SCRATCH_ANCHOR";

/// The identity of one filesystem node: its device and inode numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeId {
    /// Device number of the filesystem holding the node.
    pub dev: u64,
    /// Inode number within that filesystem.
    pub ino: u64,
}

impl NodeId {
    /// Whether `self` and `other` name the same node.
    #[must_use]
    pub const fn same(self, other: Self) -> bool {
        self.dev == other.dev && self.ino == other.ino
    }
}

/// A claimed scratch anchor: an inherited descriptor and the node it must hold.
///
/// A launcher that proved a directory trusted where owners are observable (on
/// the host, with real uids) opens it, leaves the descriptor open across the
/// exec into the jail, and names both the descriptor and the node in
/// [`ANCHOR_VAR`]. Inside a user namespace an ancestor owned by an unmapped
/// uid reads as the overflow uid, which aliases every unmapped host user, so
/// ownership cannot be re-derived there; the ancestor walk instead stops at
/// the anchor, whose proof it inherits. The claim is only a claim until
/// [`anchor_verdict`] matches it against the node the descriptor really holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScratchAnchor {
    fd: u32,
    node: NodeId,
}

impl ScratchAnchor {
    /// The anchor for the inherited descriptor `fd` holding `node`.
    #[must_use]
    pub const fn new(fd: u32, node: NodeId) -> Self {
        Self { fd, node }
    }

    /// Parse an [`ANCHOR_VAR`] value of exactly three decimal fields.
    ///
    /// The shape is `<fd>:<dev>:<ino>`, each a non-empty run of ASCII digits.
    /// Anything else (a sign, a space, a missing or extra field, a value out of
    /// range, non-UTF-8 bytes) is no anchor.
    #[must_use]
    pub fn parse(raw: &OsStr) -> Option<Self> {
        let mut fields = raw.to_str()?.split(':');
        let fd = decimal(fields.next()?)?;
        let dev = decimal(fields.next()?)?;
        let ino = decimal(fields.next()?)?;
        if fields.next().is_some() {
            return None;
        }
        Some(Self::new(fd, NodeId { dev, ino }))
    }

    /// The [`ANCHOR_VAR`] value [`ScratchAnchor::parse`] reads back as `self`.
    #[must_use]
    pub fn encode(self) -> OsString {
        OsString::from(format!("{}:{}:{}", self.fd, self.node.dev, self.node.ino))
    }

    /// The inherited descriptor number.
    #[must_use]
    pub const fn fd(self) -> u32 {
        self.fd
    }

    /// The node the descriptor must hold.
    #[must_use]
    pub const fn node(self) -> NodeId {
        self.node
    }
}

/// A decimal field of an anchor value: ASCII digits only, in range.
fn decimal<T: std::str::FromStr>(field: &str) -> Option<T> {
    if field.is_empty() || !field.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    field.parse().ok()
}

/// Why a claimed anchor was not honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorRefusal {
    /// The descriptor holds a node other than the one claimed.
    OtherNode,
    /// The held node is not a private directory of the effective user.
    NotPrivate(ScratchRefusal),
}

impl fmt::Display for AnchorRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OtherNode => f.write_str("the anchor descriptor holds another node"),
            Self::NotPrivate(refusal) => write!(f, "the anchor directory is {refusal}"),
        }
    }
}

impl std::error::Error for AnchorRefusal {}

/// A launcher's refusal to hand down the directory at `path` as a scratch anchor.
#[derive(Debug)]
pub struct AnchorError {
    /// The directory that was to be the anchor.
    pub path: PathBuf,
    /// Why it was refused.
    pub refusal: AnchorRefusal,
}

impl fmt::Display for AnchorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "refusing scratch anchor {}: {}",
            self.path.display(),
            self.refusal
        )
    }
}

impl std::error::Error for AnchorError {}

/// Whether the node read through the anchor descriptor honours the anchor `claim`.
///
/// `held` and `held_node` are the facts and identity of what the descriptor
/// really holds. It must be exactly the claimed node, and that node must pass
/// [`private_verdict`] as a directory: an anchor is a private directory of the
/// effective user, never a shared or foreign one.
///
/// # Errors
/// The [`AnchorRefusal`] naming the first violated condition.
pub const fn anchor_verdict(
    held: EntryFacts,
    held_node: NodeId,
    claim: ScratchAnchor,
    who: Identity,
) -> Result<NodeId, AnchorRefusal> {
    if !held_node.same(claim.node) {
        return Err(AnchorRefusal::OtherNode);
    }
    match private_verdict(held, EntryKind::Directory, who) {
        Ok(()) => Ok(held_node),
        Err(refusal) => Err(AnchorRefusal::NotPrivate(refusal)),
    }
}

/// What the ancestor walk does after one ancestor passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Prove the next ancestor up.
    Continue,
    /// The ancestor is the honoured anchor: everything above it is already proven.
    Anchored,
}

/// The walk's verdict on one ancestor, given the honoured anchor, if any.
///
/// Every ancestor up to and including the anchor must pass [`base_verdict`];
/// the walk stops only at the anchor node itself, never at any other node.
///
/// # Errors
/// The [`ScratchRefusal`] of [`base_verdict`].
pub const fn ancestor_step(
    entry: EntryFacts,
    node: NodeId,
    who: Identity,
    anchor: Option<NodeId>,
) -> Result<Step, ScratchRefusal> {
    if let Err(refusal) = base_verdict(entry, who) {
        return Err(refusal);
    }
    match anchor {
        Some(anchor) if anchor.same(node) => Ok(Step::Anchored),
        Some(_) | None => Ok(Step::Continue),
    }
}

/// Decide whether a canonical scratch root lies inside a canonical profile.
///
/// Both paths are expected canonical (absolute, links resolved). Anything that
/// is not provably inside is refused: a profile that is relative, holds a `..`
/// segment, or names a bare filesystem root (which would make every path
/// "private"); a root that is relative, holds a `..` segment, or lies outside
/// the profile, compared component-wise so `/home/al` never contains
/// `/home/alice`.
///
/// # Errors
/// Returns the [`ScratchRootRefusal`] that the root fails.
pub fn root_within_profile(root: &Path, profile: &Path) -> Result<(), ScratchRootRefusal> {
    let has_parent_dir = |p: &Path| p.components().any(|c| matches!(c, Component::ParentDir));
    let names_a_directory = profile
        .components()
        .any(|c| matches!(c, Component::Normal(_)));
    if !profile.is_absolute() || has_parent_dir(profile) || !names_a_directory {
        return Err(ScratchRootRefusal::NoProfile);
    }
    if root.is_absolute() && !has_parent_dir(root) && root.starts_with(profile) {
        Ok(())
    } else {
        Err(ScratchRootRefusal::OutsideProfile {
            root: root.to_path_buf(),
            profile: profile.to_path_buf(),
        })
    }
}

/// Resolve `root` and `profile`, require the root inside the profile, and return the resolved root.
///
/// `profile` is the raw value of [`PROFILE_VAR`]; an unset, empty, or relative
/// value proves nothing and is refused. Resolution follows every link, so a
/// link inside the profile that points outside it is refused too. Entries must
/// be created under the returned path, not under `root`, so that a link on
/// `root` re-pointed after the check cannot redirect the creation.
///
/// # Errors
/// Returns a [`ScratchRootRefusal`] when either path cannot be resolved or the
/// resolved root lies outside the resolved profile.
pub fn verify_root_within_profile(
    root: &Path,
    profile: Option<&OsStr>,
) -> Result<PathBuf, ScratchRootRefusal> {
    let profile = profile
        .map(Path::new)
        .filter(|p| p.is_absolute())
        .ok_or(ScratchRootRefusal::NoProfile)?;
    let root = canonical(root)?;
    root_within_profile(&root, &canonical(profile)?)?;
    Ok(root)
}

/// The canonical form of `path`, or the refusal naming why it has none.
fn canonical(path: &Path) -> Result<PathBuf, ScratchRootRefusal> {
    std::fs::canonicalize(path).map_err(|e| ScratchRootRefusal::Unresolvable {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })
}

#[cfg(unix)]
mod platform {
    use super::{EntryFacts, EntryKind, Identity, NodeId};
    use std::os::unix::fs::MetadataExt as _;

    /// The effective uid and gid of this process.
    #[must_use]
    pub fn identity() -> Identity {
        Identity {
            euid: rustix::process::geteuid().as_raw(),
            egid: rustix::process::getegid().as_raw(),
        }
    }

    /// The verdict facts of `meta`.
    #[must_use]
    pub fn facts(meta: &std::fs::Metadata) -> EntryFacts {
        let ft = meta.file_type();
        let kind = if ft.is_dir() {
            EntryKind::Directory
        } else if ft.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        EntryFacts {
            kind,
            owner: meta.uid(),
            group: meta.gid(),
            mode: meta.mode(),
        }
    }

    /// The node identity of `meta`.
    #[must_use]
    pub fn node(meta: &std::fs::Metadata) -> NodeId {
        NodeId {
            dev: meta.dev(),
            ino: meta.ino(),
        }
    }
}

/// Resolve `base` to the trusted canonical directory private entries are created under.
///
/// Creates `base` when absent, canonicalises it (so no ancestor is a symlink), and
/// requires [`base_verdict`] of every ancestor up to the root, or up to and
/// including the honoured inherited anchor ([`inherited_anchor`] of what
/// `anchor` yields), whose ancestors its launcher already proved. Ownership
/// decides trust here, so the profile lookup is never consulted.
#[cfg(unix)]
fn trusted_base(
    base: &Path,
    _profile: impl FnOnce() -> Option<OsString>,
    anchor: impl FnOnce() -> Option<OsString>,
) -> io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt as _;
    // Components this call creates are private, so they pass `base_verdict`
    // without the owner-group exception.
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(base)?;
    let canonical = std::fs::canonicalize(base)?;
    let who = platform::identity();
    walk_ancestors(&canonical, who, inherited_anchor(anchor().as_deref(), who))?;
    Ok(canonical)
}

/// Require [`ancestor_step`] of every ancestor of the canonical `dir`, stopping at `anchor`.
///
/// # Errors
/// `PermissionDenied` with a [`ScratchError`] naming the first refused
/// ancestor; the lookup error otherwise.
#[cfg(unix)]
fn walk_ancestors(dir: &Path, who: Identity, anchor: Option<NodeId>) -> io::Result<()> {
    for ancestor in dir.ancestors() {
        let meta = std::fs::symlink_metadata(ancestor)?;
        match ancestor_step(platform::facts(&meta), platform::node(&meta), who, anchor) {
            Ok(Step::Continue) => {}
            Ok(Step::Anchored) => return Ok(()),
            Err(refusal) => return Err(refused(ancestor, refusal)),
        }
    }
    Ok(())
}

/// The node of the honoured inherited anchor named by the [`ANCHOR_VAR`] value `raw`, if any.
///
/// The claimed descriptor is examined through `/proc/self/fd`, so what is
/// judged is the node the descriptor really holds, not the claim. A value
/// that does not parse, a descriptor that is not open, or a held node that
/// fails [`anchor_verdict`] honours nothing: the walk then proves every
/// ancestor, the conservative branch.
#[cfg(unix)]
#[must_use]
pub fn inherited_anchor(raw: Option<&OsStr>, who: Identity) -> Option<NodeId> {
    let claim = ScratchAnchor::parse(raw?)?;
    let meta = std::fs::metadata(format!("/proc/self/fd/{}", claim.fd())).ok()?;
    anchor_verdict(platform::facts(&meta), platform::node(&meta), claim, who).ok()
}

/// Prove the existing directory `dir` a trusted private directory of the effective user, with no anchor.
///
/// This is the launcher-side proof an inherited anchor stands for: `dir` is
/// canonicalised, every ancestor passes [`base_verdict`], and `dir` itself
/// passes [`private_verdict`] as a directory. Returns the canonical path and
/// the node it resolved to.
///
/// # Errors
/// `PermissionDenied` with a [`ScratchError`] naming the refused entry; the
/// lookup error otherwise.
#[cfg(unix)]
pub fn prove_anchor_dir(dir: &Path) -> io::Result<(PathBuf, NodeId)> {
    let canonical = std::fs::canonicalize(dir)?;
    walk_ancestors(&canonical, platform::identity(), None)?;
    let meta = std::fs::symlink_metadata(&canonical)?;
    verify_private(&canonical, &meta, EntryKind::Directory)?;
    Ok((canonical, platform::node(&meta)))
}

/// Judge the open directory `held`, just opened at the proven `canonical` path, as the anchor for `proven`.
///
/// The handle must hold exactly the node the proof resolved to, and that node
/// must still be a private directory of the effective user.
///
/// # Errors
/// `PermissionDenied` when the handle holds another node or the node is no
/// longer private; the `fstat` error otherwise.
#[cfg(unix)]
pub fn verify_anchor_handle(
    canonical: &Path,
    held: &File,
    fd: u32,
    proven: NodeId,
) -> io::Result<ScratchAnchor> {
    let meta = held.metadata()?;
    let claim = ScratchAnchor::new(fd, proven);
    anchor_verdict(
        platform::facts(&meta),
        platform::node(&meta),
        claim,
        platform::identity(),
    )
    .map(|_| claim)
    .map_err(|refusal| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            AnchorError {
                path: canonical.to_path_buf(),
                refusal,
            },
        )
    })
}

/// Resolve `base` to the directory private entries are created under, proven inside the user's profile.
///
/// Non-unix hosts carry no POSIX owner or mode bits, so an entry inherits its
/// parent's access control. The nearest existing ancestor of `base` is proven
/// inside the profile `profile` returns before any missing component is
/// created, and `base` itself is proven again once it exists; the resolved
/// path is returned.
#[cfg(not(unix))]
fn trusted_base(
    base: &Path,
    profile: impl FnOnce() -> Option<OsString>,
    _anchor: impl FnOnce() -> Option<OsString>,
) -> io::Result<PathBuf> {
    let profile = profile();
    let profile = profile.as_deref();
    let existing = base
        .ancestors()
        .find(|a| !a.as_os_str().is_empty() && a.exists())
        .unwrap_or(base);
    verify_root_within_profile(existing, profile)?;
    std::fs::create_dir_all(base)?;
    verify_root_within_profile(base, profile).map_err(io::Error::from)
}

/// Verify that `path` is a private directory of the effective user.
///
/// # Errors
/// A `PermissionDenied` [`ScratchError`] when it is a symlink, not a directory,
/// foreign-owned, or grants group/other bits; the lookup error otherwise.
pub fn verify_private_dir(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    verify_private(path, &meta, EntryKind::Directory)
}

/// Verify that the open handle `file`, created at `path`, is a private regular file of the effective user.
///
/// # Errors
/// A `PermissionDenied` [`ScratchError`] when the handle is not a regular file,
/// is foreign-owned, or grants group/other bits; the `fstat` error otherwise.
pub fn verify_private_file(path: &Path, file: &File) -> io::Result<()> {
    verify_private(path, &file.metadata()?, EntryKind::File)
}

#[cfg(unix)]
fn verify_private(path: &Path, meta: &std::fs::Metadata, kind: EntryKind) -> io::Result<()> {
    private_verdict(platform::facts(meta), kind, platform::identity()).map_err(|r| refused(path, r))
}

#[cfg(not(unix))]
fn verify_private(path: &Path, meta: &std::fs::Metadata, kind: EntryKind) -> io::Result<()> {
    let ft = meta.file_type();
    let ok = match kind {
        EntryKind::Directory => ft.is_dir() && !ft.is_symlink(),
        EntryKind::File => ft.is_file() && !ft.is_symlink(),
        EntryKind::Other => false,
    };
    if ok {
        return Ok(());
    }
    Err(refused(
        path,
        if matches!(kind, EntryKind::File) {
            ScratchRefusal::NotARegularFile
        } else {
            ScratchRefusal::NotADirectory
        },
    ))
}

/// Confine a caller label to one bounded, non-empty path component.
///
/// ASCII alphanumerics, `-` and `_` are kept, every other character becomes `_`,
/// and at most [`MAX_LABEL_CHARS`] characters survive, so a label can never add
/// a path component, name `.`/`..`, or reach an alternate data stream.
#[must_use]
pub fn confined_label(label: &str) -> String {
    let confined: String = label
        .chars()
        .take(MAX_LABEL_CHARS)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if confined.is_empty() {
        FALLBACK_LABEL.to_owned()
    } else {
        confined
    }
}

/// The longest name [`create_private_dir`] or [`create_exclusive_file`] can give an entry labelled `label`.
///
/// A caller bounded by a total path length (a Unix socket path) checks the
/// base against this before creating anything.
#[must_use]
pub fn max_scratch_name_len(label: &str) -> usize {
    // `-<pid>-<hex>`: a u32 pid is at most 10 decimal digits.
    confined_label(label).len() + 1 + 10 + 1 + 2 * ENTROPY_BYTES
}

/// `<label>-<pid>-<32 hex chars>`, the hex drawn from `fill`.
fn scratch_name(
    label: &str,
    fill: &mut impl FnMut(&mut [u8]) -> io::Result<()>,
) -> io::Result<String> {
    use std::fmt::Write as _;
    let mut entropy = [0u8; ENTROPY_BYTES];
    fill(&mut entropy)?;
    let mut name = confined_label(label);
    let _ = write!(name, "-{}-", std::process::id());
    for byte in entropy {
        let _ = write!(name, "{byte:02x}");
    }
    Ok(name)
}

/// Create an entry under the trusted `base` with `create`, retrying a collision with a fresh name.
///
/// At most [`MAX_ATTEMPTS`] names are tried; `fill` supplies each name's entropy
/// and its failure fails the creation before any entry is attempted.
fn create_unique<T>(
    base: &Path,
    label: &str,
    mut fill: impl FnMut(&mut [u8]) -> io::Result<()>,
    mut create: impl FnMut(&Path) -> io::Result<T>,
) -> io::Result<(PathBuf, T)> {
    for _ in 0..MAX_ATTEMPTS {
        let path = base.join(scratch_name(label, &mut fill)?);
        match create(&path) {
            Ok(entry) => return Ok((path, entry)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, NamesExhausted))
}

/// Create the directory `path` exclusively with mode 0700; only the final component is created.
#[cfg(unix)]
fn exclusive_mkdir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(false)
        .create(path)
}

/// Create the directory `path` exclusively, inheriting the proven-private parent's access control.
#[cfg(not(unix))]
fn exclusive_mkdir(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

/// Open a new file at `path`: exclusive, never through a final symlink, mode 0600.
#[cfg(unix)]
fn exclusive_open(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

/// Open a new file at `path` exclusively, inheriting the proven-private parent's access control.
#[cfg(not(unix))]
fn exclusive_open(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
}

// ── Creation ─────────────────────────────────────────────────────────────────

/// Create a private directory directly under the resolved `base`, creating `base` when absent.
///
/// The name is `<label>-<pid>-<32 hex chars>`, the hex drawn from `fill` (the
/// caller's OS CSPRNG); `label` is a diagnostic tag only, confined by
/// [`confined_label`]. `profile` yields the raw [`PROFILE_VAR`] value on hosts
/// where trust is proven by profile containment; `anchor` yields the raw
/// [`ANCHOR_VAR`] value a jail launcher hands down (see [`ScratchAnchor`]). The caller owns the returned
/// directory and removes it.
///
/// # Errors
/// `PermissionDenied` with a [`ScratchError`] when `base` (or an ancestor) or
/// the created directory fails verification, or with a [`ScratchRootRefusal`]
/// when a non-unix `base` cannot be proven inside the user's profile; the
/// `fill` error when the CSPRNG is unavailable; `AlreadyExists` carrying
/// [`NamesExhausted`] when every fresh name collided; any other I/O error.
pub fn create_private_dir(
    base: &Path,
    label: &str,
    fill: impl FnMut(&mut [u8]) -> io::Result<()>,
    profile: impl FnOnce() -> Option<OsString>,
    anchor: impl FnOnce() -> Option<OsString>,
) -> io::Result<PathBuf> {
    let base = trusted_base(base, profile, anchor)?;
    let (path, ()) = create_unique(&base, label, fill, exclusive_mkdir)?;
    // A refused entry is not ours to remove.
    verify_private_dir(&path)?;
    Ok(path)
}

/// Create the file `leaf` inside the private directory `dir` and return its path and open handle.
///
/// # Errors
/// Any open error (a planted symlink fails `O_NOFOLLOW`, an existing entry
/// fails `O_EXCL`); `PermissionDenied` with a [`ScratchError`] when the opened
/// handle is not a private regular file of the effective user or `dir` is no
/// longer a private directory.
pub fn create_private_file(dir: &Path, leaf: &ScratchLeaf) -> io::Result<(PathBuf, File)> {
    let path = dir.join(leaf);
    let file = exclusive_open(&path)?;
    verify_private_file(&path, &file)?;
    verify_private_dir(dir)?;
    Ok((path, file))
}

/// Create a private, unpredictably named file directly under the resolved `base`.
///
/// The base is proven trusted exactly as for [`create_private_dir`], so a
/// shared base is sticky and no other user can rename or replace the file; the
/// name is drawn as there, and the file is opened `O_EXCL` + `O_NOFOLLOW` with
/// mode 0600 and verified through its handle. The caller owns the file and
/// removes it.
///
/// # Errors
/// As [`create_private_dir`], with the file verified in place of the directory.
pub fn create_exclusive_file(
    base: &Path,
    label: &str,
    fill: impl FnMut(&mut [u8]) -> io::Result<()>,
    profile: impl FnOnce() -> Option<OsString>,
    anchor: impl FnOnce() -> Option<OsString>,
) -> io::Result<(PathBuf, File)> {
    let base = trusted_base(base, profile, anchor)?;
    let (path, file) = create_unique(&base, label, fill, exclusive_open)?;
    verify_private_file(&path, &file)?;
    Ok((path, file))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Identity = Identity {
        euid: 1000,
        egid: 1000,
    };

    const fn dir(owner: u32, group: u32, mode: u32) -> EntryFacts {
        EntryFacts {
            kind: EntryKind::Directory,
            owner,
            group,
            mode,
        }
    }

    /// A fresh directory tree unique to this test, removed on drop.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> io::Result<Self> {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let root = std::env::temp_dir().join(format!(
                "ipe-private-scratch-{tag}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(root.join("profile").join("temp"))?;
            std::fs::create_dir_all(root.join("shared"))?;
            Ok(Self(root))
        }

        fn profile(&self) -> PathBuf {
            self.0.join("profile")
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn base_verdict_accepts_own_root_and_sticky_bases() {
        assert_eq!(base_verdict(dir(1000, 1000, 0o700), ME), Ok(()));
        assert_eq!(base_verdict(dir(0, 0, 0o755), ME), Ok(()));
        assert_eq!(base_verdict(dir(0, 0, 0o1777), ME), Ok(()));
        assert_eq!(base_verdict(dir(1000, 1000, 0o775), ME), Ok(()));
    }

    /// A base owned by another user is refused whatever its mode, sticky included.
    #[test]
    fn base_verdict_refuses_foreign_owner() {
        for mode in [0o700, 0o755, 0o1777] {
            assert_eq!(
                base_verdict(dir(1001, 1001, mode), ME),
                Err(ScratchRefusal::ForeignOwner),
                "mode {mode:o}"
            );
        }
    }

    #[test]
    fn base_verdict_refuses_non_sticky_world_or_group_writable() {
        assert_eq!(
            base_verdict(dir(0, 0, 0o777), ME),
            Err(ScratchRefusal::WritableByOthers)
        );
        assert_eq!(
            base_verdict(dir(1000, 1000, 0o777), ME),
            Err(ScratchRefusal::WritableByOthers)
        );
        assert_eq!(
            base_verdict(dir(0, 100, 0o775), ME),
            Err(ScratchRefusal::WritableByOthers)
        );
        assert_eq!(
            base_verdict(dir(1000, 100, 0o775), ME),
            Err(ScratchRefusal::WritableByOthers)
        );
    }

    #[test]
    fn base_verdict_refuses_a_symlink() {
        let link = EntryFacts {
            kind: EntryKind::Other,
            ..dir(1000, 1000, 0o777)
        };
        assert_eq!(base_verdict(link, ME), Err(ScratchRefusal::NotADirectory));
    }

    const NODE: NodeId = NodeId { dev: 7, ino: 42 };
    const OTHER_NODE: NodeId = NodeId { dev: 7, ino: 43 };

    /// An ancestor owned by the overflow uid (an unmapped owner seen from a
    /// user namespace) is refused like any foreign owner: it proves nothing.
    #[test]
    fn overflow_uid_ancestor_is_refused_without_an_anchor() {
        assert_eq!(
            ancestor_step(dir(65534, 65534, 0o755), NODE, ME, None),
            Err(ScratchRefusal::ForeignOwner)
        );
    }

    /// The walk stops only at the anchor node itself, and only once that node passed `base_verdict`.
    #[test]
    fn ancestor_step_stops_only_at_the_anchor_node() {
        let private = dir(1000, 1000, 0o700);
        assert_eq!(
            ancestor_step(private, NODE, ME, Some(NODE)),
            Ok(Step::Anchored)
        );
        assert_eq!(
            ancestor_step(private, NODE, ME, Some(OTHER_NODE)),
            Ok(Step::Continue)
        );
        assert_eq!(ancestor_step(private, NODE, ME, None), Ok(Step::Continue));
        assert_eq!(
            ancestor_step(dir(1000, 1000, 0o777), NODE, ME, Some(NODE)),
            Err(ScratchRefusal::WritableByOthers)
        );
        assert_eq!(
            ancestor_step(dir(65534, 65534, 0o700), NODE, ME, Some(NODE)),
            Err(ScratchRefusal::ForeignOwner)
        );
    }

    /// A claim is honoured only for exactly the held node, and only when that node is a private directory.
    #[test]
    fn anchor_verdict_refuses_another_node_or_a_non_private_directory() {
        let claim = ScratchAnchor::new(3, NODE);
        let private = dir(1000, 1000, 0o700);
        assert_eq!(anchor_verdict(private, NODE, claim, ME), Ok(NODE));
        assert_eq!(
            anchor_verdict(private, OTHER_NODE, claim, ME),
            Err(AnchorRefusal::OtherNode)
        );
        assert_eq!(
            anchor_verdict(private, NodeId { dev: 8, ino: 42 }, claim, ME),
            Err(AnchorRefusal::OtherNode)
        );
        for (facts, refusal) in [
            (dir(1000, 1000, 0o755), ScratchRefusal::NotPrivate),
            (dir(1000, 1000, 0o1777), ScratchRefusal::NotPrivate),
            (dir(0, 0, 0o700), ScratchRefusal::ForeignOwner),
            (dir(65534, 65534, 0o700), ScratchRefusal::ForeignOwner),
            (
                EntryFacts {
                    kind: EntryKind::File,
                    ..private
                },
                ScratchRefusal::NotADirectory,
            ),
        ] {
            assert_eq!(
                anchor_verdict(facts, NODE, claim, ME),
                Err(AnchorRefusal::NotPrivate(refusal)),
                "{facts:?}"
            );
        }
    }

    #[test]
    fn anchor_value_round_trips() {
        let anchor = ScratchAnchor::new(
            9,
            NodeId {
                dev: u64::MAX,
                ino: 0,
            },
        );
        assert_eq!(ScratchAnchor::parse(&anchor.encode()), Some(anchor));
        assert_eq!(
            ScratchAnchor::parse(OsStr::new("3:7:42")),
            Some(ScratchAnchor::new(3, NODE))
        );
    }

    /// Only exactly three non-empty ASCII-decimal fields in range parse.
    #[test]
    fn malformed_anchor_values_are_no_anchor() {
        for raw in [
            "",
            "3",
            "3:7",
            "3:7:42:1",
            "3:7:42:",
            ":7:42",
            "3::42",
            "+3:7:42",
            "-3:7:42",
            " 3:7:42",
            "3:7:42 ",
            "3:7:0x2a",
            "3:7:٤٢",
            "4294967296:7:42",
            "3:18446744073709551616:42",
        ] {
            assert_eq!(ScratchAnchor::parse(OsStr::new(raw)), None, "{raw:?}");
        }
    }

    #[test]
    fn private_verdict_accepts_only_owned_owner_only_entries() {
        assert_eq!(
            private_verdict(dir(1000, 1000, 0o700), EntryKind::Directory, ME),
            Ok(())
        );
        assert_eq!(
            private_verdict(dir(1001, 1000, 0o700), EntryKind::Directory, ME),
            Err(ScratchRefusal::ForeignOwner)
        );
        assert_eq!(
            private_verdict(dir(0, 0, 0o700), EntryKind::Directory, ME),
            Err(ScratchRefusal::ForeignOwner)
        );
        assert_eq!(
            private_verdict(dir(1000, 1000, 0o750), EntryKind::Directory, ME),
            Err(ScratchRefusal::NotPrivate)
        );
        assert_eq!(
            private_verdict(dir(1000, 1000, 0o700), EntryKind::File, ME),
            Err(ScratchRefusal::NotARegularFile)
        );
        let link = EntryFacts {
            kind: EntryKind::Other,
            ..dir(1000, 1000, 0o700)
        };
        assert_eq!(
            private_verdict(link, EntryKind::Directory, ME),
            Err(ScratchRefusal::NotADirectory)
        );
    }

    #[test]
    fn leaf_accepts_one_plain_component() {
        for name in ["console.db", "index", "a-b_c", ".hidden", "x..y"] {
            assert_eq!(
                ScratchLeaf::new(name).map(|l| l.as_str().to_owned()),
                Ok(name.to_owned())
            );
        }
    }

    #[test]
    fn leaf_refuses_every_name_that_is_not_one_plain_component() {
        let cases = [
            ("", LeafRefusal::Empty),
            ("a/b", LeafRefusal::Separator),
            ("/abs", LeafRefusal::Separator),
            ("../x", LeafRefusal::Separator),
            ("a\\b", LeafRefusal::Separator),
            ("file:stream", LeafRefusal::Separator),
            ("C:", LeafRefusal::Separator),
            ("a\0b", LeafRefusal::Nul),
            (".", LeafRefusal::DotName),
            ("..", LeafRefusal::DotName),
        ];
        for (name, refusal) in cases {
            assert_eq!(ScratchLeaf::new(name), Err(refusal), "{name:?}");
        }
        let long = "a".repeat(MAX_LEAF_BYTES + 1);
        assert_eq!(ScratchLeaf::new(&long), Err(LeafRefusal::TooLong));
        assert!(ScratchLeaf::new(&"a".repeat(MAX_LEAF_BYTES)).is_ok());
        assert_eq!(
            io::Error::from(LeafRefusal::Empty).kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn every_confined_label_is_a_valid_leaf() {
        for label in ["", ".", "..", "a/b", "../../etc", "x:y", "\0", "ok-label"] {
            assert!(
                ScratchLeaf::new(&confined_label(label)).is_ok(),
                "{label:?}"
            );
        }
    }

    #[test]
    fn root_inside_or_equal_to_profile_is_accepted() {
        let profile = Path::new("/home/alice");
        assert_eq!(
            root_within_profile(Path::new("/home/alice/AppData/Local/Temp"), profile),
            Ok(())
        );
        assert_eq!(root_within_profile(profile, profile), Ok(()));
    }

    #[test]
    fn root_outside_profile_is_refused() {
        let refused = root_within_profile(Path::new("/tmp"), Path::new("/home/alice"));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
    }

    #[test]
    fn sibling_sharing_a_name_prefix_is_refused() {
        let refused =
            root_within_profile(Path::new("/home/alice-shared"), Path::new("/home/alice"));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
    }

    #[test]
    fn parent_dir_escape_is_refused() {
        let refused =
            root_within_profile(Path::new("/home/alice/../bob"), Path::new("/home/alice"));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
    }

    #[test]
    fn relative_root_is_refused() {
        let refused = root_within_profile(Path::new("alice/temp"), Path::new("/home/alice"));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
    }

    #[test]
    fn relative_profile_proves_nothing() {
        assert_eq!(
            root_within_profile(Path::new("/home/alice/temp"), Path::new("alice")),
            Err(ScratchRootRefusal::NoProfile)
        );
    }

    #[test]
    fn filesystem_root_profile_proves_nothing() {
        assert_eq!(
            root_within_profile(Path::new("/tmp"), Path::new("/")),
            Err(ScratchRootRefusal::NoProfile)
        );
    }

    #[test]
    fn unset_empty_or_relative_profile_variable_is_refused() -> io::Result<()> {
        let tree = Tree::new("noprofile")?;
        let root = tree.profile().join("temp");
        for profile in [
            None,
            Some(OsStr::new("")),
            Some(OsStr::new("relative/profile")),
        ] {
            assert_eq!(
                verify_root_within_profile(&root, profile),
                Err(ScratchRootRefusal::NoProfile)
            );
        }
        Ok(())
    }

    #[test]
    fn resolved_root_inside_profile_is_accepted() -> io::Result<()> {
        let tree = Tree::new("inside")?;
        let profile = tree.profile();
        let resolved = std::fs::canonicalize(profile.join("temp"))?;
        assert_eq!(
            verify_root_within_profile(&profile.join("temp"), Some(profile.as_os_str())),
            Ok(resolved)
        );
        Ok(())
    }

    #[test]
    fn shared_root_outside_profile_is_refused() -> io::Result<()> {
        let tree = Tree::new("shared")?;
        let profile = tree.profile();
        let refused = verify_root_within_profile(&tree.0.join("shared"), Some(profile.as_os_str()));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
        Ok(())
    }

    #[test]
    fn dot_dot_path_leaving_profile_is_refused() -> io::Result<()> {
        let tree = Tree::new("dotdot")?;
        let profile = tree.profile();
        let escaping = profile.join("temp").join("..").join("..").join("shared");
        let refused = verify_root_within_profile(&escaping, Some(profile.as_os_str()));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::OutsideProfile { .. })
        ));
        Ok(())
    }

    #[test]
    fn missing_root_is_refused() -> io::Result<()> {
        let tree = Tree::new("missing")?;
        let profile = tree.profile();
        let refused =
            verify_root_within_profile(&profile.join("absent"), Some(profile.as_os_str()));
        assert!(matches!(
            refused,
            Err(ScratchRootRefusal::Unresolvable { .. })
        ));
        Ok(())
    }

    #[test]
    fn every_root_refusal_names_the_fix_and_denies_permission() {
        let refusals = [
            ScratchRootRefusal::NoProfile,
            ScratchRootRefusal::Unresolvable {
                path: PathBuf::from("/x"),
                detail: "not found".to_owned(),
            },
            ScratchRootRefusal::OutsideProfile {
                root: PathBuf::from("/tmp"),
                profile: PathBuf::from("/home/alice"),
            },
        ];
        for refusal in refusals {
            assert!(
                refusal.to_string().contains("set TEMP and TMP"),
                "{refusal}"
            );
            assert_eq!(
                io::Error::from(refusal).kind(),
                io::ErrorKind::PermissionDenied
            );
        }
    }

    /// An unavailable CSPRNG fails the creation before any entry is attempted;
    /// no weaker name is ever produced.
    #[test]
    fn entropy_failure_fails_closed() -> io::Result<()> {
        let tree = Tree::new("entropy")?;
        let mut attempts = 0usize;
        let result = create_unique(
            &tree.0,
            "ipe-test",
            |_: &mut [u8]| Err(io::Error::other("entropy unavailable")),
            |p: &Path| {
                attempts += 1;
                exclusive_mkdir(p)
            },
        );
        assert!(result.is_err());
        assert_eq!(attempts, 0);
        assert_eq!(
            std::fs::read_dir(&tree.0)?.count(),
            2,
            "only the fixture tree"
        );
        Ok(())
    }

    /// Fixed entropy collides on every attempt after the first entry, and the
    /// retry loop stops at its bound with `AlreadyExists`.
    #[test]
    fn collisions_stop_at_the_attempt_bound() -> io::Result<()> {
        let tree = Tree::new("collide")?;
        let zeros = |buf: &mut [u8]| {
            buf.fill(0);
            Ok(())
        };
        create_unique(&tree.0, "ipe-test", zeros, exclusive_mkdir)?;
        let mut attempts = 0usize;
        let second = create_unique(&tree.0, "ipe-test", zeros, |p: &Path| {
            attempts += 1;
            exclusive_mkdir(p)
        });
        let Err(err) = second else {
            assert!(second.is_err(), "every name collides");
            return Ok(());
        };
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("unique scratch entry"), "{err}");
        assert_eq!(attempts, MAX_ATTEMPTS);
        Ok(())
    }

    /// A hostile label cannot add a path component, reach an alternate data
    /// stream, or grow the name past its declared bound.
    #[test]
    fn label_is_confined_to_one_bounded_component() -> io::Result<()> {
        let mut fill = |buf: &mut [u8]| {
            buf.fill(0xab);
            Ok(())
        };
        let hostile = format!("../../etc/x:y\\z{}", "a".repeat(500));
        let name = scratch_name(&hostile, &mut fill)?;
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{name}"
        );
        assert!(name.starts_with("______etc_x_y_z"), "{name}");
        assert!(name.ends_with(&"ab".repeat(ENTROPY_BYTES)), "{name}");
        assert!(name.len() <= max_scratch_name_len(&hostile));
        assert!(max_scratch_name_len(&hostile) <= MAX_LABEL_CHARS + 2 + 10 + 2 * ENTROPY_BYTES);
        Ok(())
    }

    #[cfg(unix)]
    mod unix {
        use super::super::*;
        use super::Tree;
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        use std::sync::atomic::{AtomicU64, Ordering};

        /// Distinct test entropy: a process-wide counter mixed with the clock.
        fn distinct(buf: &mut [u8]) -> io::Result<()> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let seed = u128::from(n) ^ (nanos << 64);
            for (byte, source) in buf.iter_mut().zip(seed.to_le_bytes()) {
                *byte = source;
            }
            Ok(())
        }

        const fn no_profile() -> Option<OsString> {
            None
        }

        fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        }

        fn private_dir(base: &Path) -> io::Result<PathBuf> {
            create_private_dir(base, "ipe-test", distinct, no_profile, no_profile)
        }

        fn leaf(name: &str) -> io::Result<ScratchLeaf> {
            ScratchLeaf::new(name).map_err(io::Error::from)
        }

        #[test]
        fn created_dir_is_0700_owned_and_directly_under_the_resolved_base() -> io::Result<()> {
            let tree = Tree::new("mkdir")?;
            let base = std::fs::canonicalize(&tree.0)?;
            let a = private_dir(&tree.0)?;
            let b = private_dir(&tree.0)?;
            assert_ne!(a, b);
            for entry in [&a, &b] {
                assert_eq!(entry.parent(), Some(base.as_path()));
                let meta = std::fs::symlink_metadata(entry)?;
                assert!(meta.file_type().is_dir());
                assert_eq!(meta.mode() & 0o777, 0o700);
                assert_eq!(meta.uid(), rustix::process::geteuid().as_raw());
            }
            Ok(())
        }

        #[test]
        fn created_files_are_0600_and_exclusive() -> io::Result<()> {
            let tree = Tree::new("mkfile")?;
            let dir = private_dir(&tree.0)?;
            let (inner, _handle) = create_private_file(&dir, &leaf("f")?)?;
            let (loose, _loose_handle) =
                create_exclusive_file(&tree.0, "ipe-test", distinct, no_profile, no_profile)?;
            for file in [&inner, &loose] {
                let meta = std::fs::symlink_metadata(file)?;
                assert!(meta.file_type().is_file());
                assert_eq!(meta.mode() & 0o777, 0o600);
            }
            assert_eq!(
                create_private_file(&dir, &leaf("f")?)
                    .map(drop)
                    .map_err(|e| e.kind()),
                Err(io::ErrorKind::AlreadyExists)
            );
            assert_eq!(
                exclusive_mkdir(&dir).map_err(|e| e.kind()),
                Err(io::ErrorKind::AlreadyExists)
            );
            Ok(())
        }

        #[test]
        fn symlinked_private_dir_is_refused_and_target_untouched() -> io::Result<()> {
            let tree = Tree::new("symdir")?;
            let target = tree.0.join("target");
            exclusive_mkdir(&target)?;
            std::fs::write(target.join("canary"), b"canary")?;
            let link = tree.0.join("link");
            std::os::unix::fs::symlink(&target, &link)?;

            let err = verify_private_dir(&link).err();
            assert_eq!(
                err.as_ref().map(io::Error::kind),
                Some(io::ErrorKind::PermissionDenied)
            );
            assert_eq!(std::fs::read(target.join("canary"))?, b"canary");
            Ok(())
        }

        #[test]
        fn group_or_world_accessible_private_dir_is_refused() -> io::Result<()> {
            let tree = Tree::new("open")?;
            let open = tree.0.join("open");
            exclusive_mkdir(&open)?;
            for mode in [0o777, 0o750, 0o705] {
                set_mode(&open, mode)?;
                let err = verify_private_dir(&open).err();
                assert_eq!(
                    err.as_ref().map(io::Error::kind),
                    Some(io::ErrorKind::PermissionDenied),
                    "mode {mode:o} must be refused"
                );
            }
            Ok(())
        }

        #[test]
        fn non_sticky_world_writable_base_is_refused_and_left_empty() -> io::Result<()> {
            let tree = Tree::new("wbase")?;
            let base = tree.0.join("base");
            exclusive_mkdir(&base)?;
            set_mode(&base, 0o777)?;
            let dir = private_dir(&base).err();
            let file =
                create_exclusive_file(&base, "ipe-test", distinct, no_profile, no_profile).err();
            for err in [dir, file] {
                assert_eq!(
                    err.as_ref().map(io::Error::kind),
                    Some(io::ErrorKind::PermissionDenied)
                );
            }
            assert_eq!(std::fs::read_dir(&base)?.count(), 0);
            Ok(())
        }

        #[test]
        fn sticky_world_writable_base_is_accepted() -> io::Result<()> {
            let tree = Tree::new("sbase")?;
            let base = tree.0.join("base");
            exclusive_mkdir(&base)?;
            set_mode(&base, 0o1777)?;
            verify_private_dir(&private_dir(&base)?)?;
            Ok(())
        }

        #[test]
        fn missing_base_components_are_created_private() -> io::Result<()> {
            let tree = Tree::new("mkbase")?;
            let outer = tree.0.join("outer");
            let base = outer.join("inner");
            verify_private_dir(&private_dir(&base)?)?;
            for created in [&outer, &base] {
                let mode = std::fs::symlink_metadata(created)?.permissions().mode();
                assert_eq!(
                    mode & 0o7777,
                    0o700,
                    "{} must be created 0700",
                    created.display()
                );
            }
            Ok(())
        }

        #[test]
        fn base_reached_through_a_symlink_is_canonicalised() -> io::Result<()> {
            let tree = Tree::new("lbase")?;
            let real = tree.0.join("real");
            exclusive_mkdir(&real)?;
            let link = tree.0.join("link");
            std::os::unix::fs::symlink(&real, &link)?;
            let dir = private_dir(&link)?;
            assert_eq!(dir.parent(), Some(std::fs::canonicalize(&real)?.as_path()));
            Ok(())
        }

        /// The accepted root is returned resolved, so creation lands under the
        /// checked location even when the given path runs through a link.
        #[test]
        fn accepted_root_is_returned_resolved() -> io::Result<()> {
            let tree = Tree::new("resolved")?;
            let profile = tree.profile();
            let link = profile.join("temp-link");
            std::os::unix::fs::symlink(profile.join("temp"), &link)?;
            let resolved = verify_root_within_profile(&link, Some(profile.as_os_str()));
            assert_eq!(resolved, Ok(std::fs::canonicalize(profile.join("temp"))?));
            Ok(())
        }

        #[test]
        fn link_inside_profile_pointing_outside_is_refused() -> io::Result<()> {
            let tree = Tree::new("link")?;
            let profile = tree.profile();
            let link = profile.join("temp-link");
            std::os::unix::fs::symlink(tree.0.join("shared"), &link)?;
            let refused = verify_root_within_profile(&link, Some(profile.as_os_str()));
            assert!(matches!(
                refused,
                Err(ScratchRootRefusal::OutsideProfile { .. })
            ));
            Ok(())
        }

        #[test]
        fn planted_symlink_at_the_leaf_is_refused_and_target_untouched() -> io::Result<()> {
            let tree = Tree::new("symfile")?;
            let canary = tree.0.join("canary");
            std::fs::write(&canary, b"canary")?;
            let dir = private_dir(&tree.0)?;
            std::os::unix::fs::symlink(&canary, dir.join("tag"))?;

            let err = create_private_file(&dir, &leaf("tag")?).err();
            assert!(err.is_some(), "a planted symlink must not be opened");
            assert_eq!(std::fs::read(&canary)?, b"canary");
            Ok(())
        }

        #[test]
        fn dangling_symlink_at_the_leaf_creates_nothing() -> io::Result<()> {
            let tree = Tree::new("dangle")?;
            let victim = tree.0.join("victim");
            let dir = private_dir(&tree.0)?;
            std::os::unix::fs::symlink(&victim, dir.join("tag"))?;

            assert!(create_private_file(&dir, &leaf("tag")?).is_err());
            assert!(std::fs::symlink_metadata(&victim).is_err());
            Ok(())
        }

        /// The anchor value naming the open directory `held` as the node at `path`.
        #[cfg(target_os = "linux")]
        fn anchor_value(held: &std::fs::File, path: &Path) -> io::Result<OsString> {
            use std::os::fd::AsRawFd as _;
            let fd = u32::try_from(held.as_raw_fd()).map_err(io::Error::other)?;
            let meta = std::fs::symlink_metadata(path)?;
            let node = NodeId {
                dev: meta.dev(),
                ino: meta.ino(),
            };
            Ok(ScratchAnchor::new(fd, node).encode())
        }

        /// An ancestor the walk cannot prove sits above a private directory, as
        /// the overflow-uid root does above a jail's scoped scratch.
        #[cfg(target_os = "linux")]
        fn anchored_tree(tag: &str) -> io::Result<(Tree, PathBuf)> {
            let tree = Tree::new(tag)?;
            let open = tree.0.join("open");
            exclusive_mkdir(&open)?;
            set_mode(&open, 0o777)?;
            let anchor = open.join("anchor");
            exclusive_mkdir(&anchor)?;
            Ok((tree, std::fs::canonicalize(anchor)?))
        }

        /// A held, private anchor stops the walk, so a base under it is trusted
        /// even though an ancestor above it is not.
        #[cfg(target_os = "linux")]
        #[test]
        fn held_private_anchor_stops_the_walk() -> io::Result<()> {
            let (_tree, anchor) = anchored_tree("anchor")?;
            let held = std::fs::File::open(&anchor)?;
            let raw = anchor_value(&held, &anchor)?;
            let who = platform::identity();
            let node = inherited_anchor(Some(raw.as_os_str()), who);
            assert!(node.is_some(), "a held private anchor is honoured");
            walk_ancestors(&anchor, who, node)?;
            let dir = create_private_dir(&anchor, "ipe-test", distinct, no_profile, || {
                Some(raw.clone())
            })?;
            assert_eq!(dir.parent(), Some(anchor.as_path()));
            verify_private_dir(&dir)
        }

        /// Without an anchor the same base is refused at the unprovable ancestor.
        #[cfg(target_os = "linux")]
        #[test]
        fn base_under_an_unprovable_ancestor_is_refused_without_an_anchor() -> io::Result<()> {
            let (_tree, anchor) = anchored_tree("noanchor")?;
            let err = private_dir(&anchor).err();
            assert_eq!(
                err.as_ref().map(io::Error::kind),
                Some(io::ErrorKind::PermissionDenied)
            );
            assert_eq!(std::fs::read_dir(&anchor)?.count(), 0);
            Ok(())
        }

        /// A claim naming another node, a descriptor holding another directory,
        /// a closed descriptor, or a non-private anchor honours nothing, and the
        /// full walk refuses.
        #[cfg(target_os = "linux")]
        #[test]
        fn unproven_anchor_claims_leave_the_full_walk_in_force() -> io::Result<()> {
            let (tree, anchor) = anchored_tree("badanchor")?;
            let who = platform::identity();
            let held = std::fs::File::open(&anchor)?;
            let decoy = std::fs::File::open(&tree.0)?;
            let wrong_node = anchor_value(&held, &tree.0)?;
            let wrong_fd = anchor_value(&decoy, &anchor)?;
            let closed = {
                let gone = std::fs::File::open(&anchor)?;
                anchor_value(&gone, &anchor)?
            };
            for raw in [wrong_node, wrong_fd, closed] {
                assert_eq!(
                    inherited_anchor(Some(raw.as_os_str()), who),
                    None,
                    "{raw:?}"
                );
                let err = create_private_dir(&anchor, "ipe-test", distinct, no_profile, || {
                    Some(raw.clone())
                })
                .err();
                assert_eq!(
                    err.as_ref().map(io::Error::kind),
                    Some(io::ErrorKind::PermissionDenied),
                    "{raw:?}"
                );
            }
            set_mode(&anchor, 0o755)?;
            let shared = anchor_value(&held, &anchor)?;
            assert_eq!(inherited_anchor(Some(shared.as_os_str()), who), None);
            assert_eq!(std::fs::read_dir(&anchor)?.count(), 0);
            Ok(())
        }

        /// The launcher-side proof accepts a private directory under trusted
        /// ancestors and refuses one under an unprovable ancestor.
        #[test]
        fn anchor_dir_is_proven_by_the_full_walk() -> io::Result<()> {
            let tree = Tree::new("prove")?;
            let good = private_dir(&tree.0)?;
            let (canonical, node) = prove_anchor_dir(&good)?;
            let held = std::fs::File::open(&canonical)?;
            let anchor = verify_anchor_handle(&canonical, &held, 3, node)?;
            assert_eq!(anchor.node(), node);

            let open = tree.0.join("open");
            exclusive_mkdir(&open)?;
            let bad = open.join("scoped");
            exclusive_mkdir(&bad)?;
            set_mode(&open, 0o777)?;
            let err = prove_anchor_dir(&bad).err();
            assert_eq!(
                err.as_ref().map(io::Error::kind),
                Some(io::ErrorKind::PermissionDenied)
            );
            let other = std::fs::File::open(&tree.0)?;
            let moved = verify_anchor_handle(&canonical, &other, 3, node).err();
            assert_eq!(
                moved.as_ref().map(io::Error::kind),
                Some(io::ErrorKind::PermissionDenied)
            );
            Ok(())
        }

        /// A private file is refused once its directory is no longer private.
        #[test]
        fn file_in_a_directory_opened_to_others_is_refused() -> io::Result<()> {
            let tree = Tree::new("opendir")?;
            let dir = private_dir(&tree.0)?;
            set_mode(&dir, 0o755)?;
            let err = create_private_file(&dir, &leaf("f")?).err();
            assert_eq!(
                err.as_ref().map(io::Error::kind),
                Some(io::ErrorKind::PermissionDenied)
            );
            Ok(())
        }
    }
}
