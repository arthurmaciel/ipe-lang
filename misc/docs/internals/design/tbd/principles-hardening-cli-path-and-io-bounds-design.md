# CLI & FFI path-and-IO bounds — hardening design

The CLI and the FFI cache accept two kinds of input the runtime already treats
as untrusted but the compile-time surfaces hold to a lower bar: **filesystem
paths taken from a project manifest**, and **file contents read off disk**. Both
enter through bare `std::path::Path::join` and `std::fs::read_to_string`, with no
type that carries the invariant the code relies on. This design replaces those
untyped boundaries with two constructs that make the defect class
unrepresentable, so a fix at one call site cannot leave a sibling open.

## The defect class

Three confirmed findings share one generative cause — an untyped path or an
uncapped read at the boundary:

- A manifest `sourceRoot` is `join`ed onto the project root with no rejection of
  `..` or an absolute prefix, so the resolved source root can point anywhere on
  the filesystem. The only follow-up is an existence check, never a containment
  check.
- The module-discovery walk pushes every `is_dir()` child and reads it, follows
  directory symlinks, and keeps no record of directories already visited and no
  depth ceiling — so a symlink pointing up its own subtree loops until the host
  runs out of memory.
- Every CLI and FFI-cache file read is a bare `read_to_string` with no size
  ceiling, while the runtime's own file reads cap at a declared limit and return
  a typed error past it. A manifest, source file, or cache artifact that is a
  device node or a multi-gigabyte file exhausts memory instead of being turned
  back.

Each is the same shape one layer over: an input dictates a path or a size, and
the code consumes it without a ceiling or a containment proof. Patching one
`join` or one `read_to_string` leaves the next one exposed. The structural
question is: *what type, if it existed, would make an escaping path or an
unbounded read impossible to write?*

## The structural properties

Two properties kill the whole class:

1. **A source root is a contained relative path by construction.** No code can
   hold a source root that escapes the project directory, because the only
   constructor rejects `..` and absolute components and proves containment
   against the canonicalised project root. Downstream traversal never
   re-encounters an unvalidated path.
2. **Every filesystem read is bounded by construction.** No code path reaches
   `read_to_string`; the single capped reader is the only way to turn a path
   into a `String`, and it returns a typed limit error rather than allocating
   without a ceiling. The directory walk carries its own visited-set and depth
   ceiling so it terminates on any tree, cyclic or adversarial.

These mirror the runtime's existing posture (its file reads are already capped;
its crypto roles are already separate types) and extend it to the compile-time
surfaces that were held lower.

## The Rust design

### A contained-relative-path newtype

Introduce a newtype in the CLI project layer whose smart constructor is the sole
entry:

```rust
/// A path proven to resolve strictly inside a given project root: no `..`
/// escape, no absolute reroot. The only constructor is [`ContainedRelPath::parse`],
/// so every value has already been checked against its root.
pub struct ContainedRelPath(PathBuf);

pub enum PathEscape {
    ParentTraversal,   // a `..` component
    Absolute,          // an absolute prefix reroots outside the base
    NotUnderRoot,      // canonicalised result is not a descendant of the root
}

impl ContainedRelPath {
    /// Parse a manifest-supplied relative path against `root`. Rejects any
    /// `..` or absolute component before touching the filesystem, then
    /// canonicalises and asserts the result is a descendant of the
    /// canonicalised `root`.
    pub fn parse(root: &Path, raw: &str) -> Result<Self, PathEscape> { /* … */ }

    /// The resolved absolute path, guaranteed under the root it was parsed with.
    pub fn resolved(&self) -> &Path { &self.0 }
}
```

The manifest readers parse `sourceRoot` into a `ContainedRelPath` at decode time
(the `package.ipe` stage reader and the legacy `ipe.toml` twin both call the same
constructor), so `ProjectManifest::src_root` becomes a `ContainedRelPath` rather
than a bare `PathBuf`. An escaping source root then has no representation past
the manifest boundary — the escape the discovery walk currently disclosed is
gone at the type level, not patched at the join. Rejection carries a typed
`PathEscape` mapped to the CLI's usage diagnostic.

### A single capped reader

Introduce one reader in a shared CLI IO module, mirroring the runtime's
`file.rs` ceiling:

```rust
/// Read a file to a `String`, refusing past `max` bytes with a typed limit
/// error instead of allocating without a ceiling. Reads `max + 1` in one pass
/// and checks the actual byte count, so it never buffers more than the cap.
pub fn read_to_string_capped(path: &Path, max: u64) -> Result<String, CliError> {
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    std::io::Read::take(f, max.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Err(CliError::FileTooLarge { path: path.into(), max });
    }
    String::from_utf8(buf).map_err(/* typed non-utf8 error */)
}
```

Every CLI and FFI-cache `read_to_string` site routes through this reader with a
declared cap (a manifest cap, a source-file cap, an FFI-artifact cap — each a
named `const`, single source of truth). The `String::with_capacity(src.len() +
…)` pre-reserve is dropped in favour of the now-bounded length. A grep gate or a
module-privacy boundary (the raw `read_to_string` is not re-exported from the CLI
crate) keeps a new call site from reappearing.

### A bounded directory walk

The discovery walk gains a canonicalised `visited: HashSet<PathBuf>` and a
`MAX_DISCOVERY_DEPTH` ceiling threaded with each queued directory:

```rust
let mut visited = HashSet::new();
while let Some((dir, depth)) = stack.pop_front() {
    if depth > MAX_DISCOVERY_DEPTH { return Err(CliError::DiscoveryTooDeep { max }); }
    let real = std::fs::canonicalize(&dir).unwrap_or(dir.clone());
    if !visited.insert(real) { continue; }   // a symlink cycle re-enters here
    // … read_dir; push children as (path, depth + 1) …
}
```

Canonicalising before the visited check collapses a `src/a/b -> src/a` cycle to
one already-seen entry; the depth ceiling bounds a legitimately deep but acyclic
tree. Both refusals are typed `CliError` variants, not a hang.

## What stays mechanical

The remaining confirmed findings are one-line swaps that need no new structure —
they are tracked in their own issues and fixed in place:

- The FFI `opaqueTypeIds` decode is made to `?`-propagate the same `malformed`
  error its sibling `opaqueTypes` already does (a fail-open turned fail-closed).
- The legacy migration scanner warns on an unrecognised key under a known
  security section instead of dropping it silently.
- The OAuth form body percent-encodes each field through an encoder rather than
  trusting a comment's URL-safe assertion.
- Foreign rustc stderr is stripped of control characters before any
  Display-formatted diagnostic (or kept on Debug formatting).

None of these introduces a type; each is a local correction, so inventing a
shared abstraction for them would trade Readability for nothing.

## Phased plan

1. **Capped reader.** Add `read_to_string_capped` and the per-surface cap
   consts; route every CLI/FFI-cache read through it; delete the
   `with_capacity` pre-reserve. Pin a rejection test per surface (a file one
   byte past the cap is refused). Close the runtime-vs-CLI consistency gap
   first, since it is self-contained.
2. **Contained source root.** Add `ContainedRelPath` + `PathEscape`; parse
   `sourceRoot` through it in both manifest readers; make
   `ProjectManifest::src_root` the newtype. Pin the rejection tests: a `..`
   root, an absolute root, and a symlink-through root are each turned away.
3. **Bounded walk.** Add the visited-set and depth ceiling to
   `discover_modules`; pin a cyclic-symlink fixture that must terminate with a
   typed error and a deep-tree fixture that must refuse past the ceiling.
4. **Mechanical corrections.** Land the four in-place fixes above with their
   rejection tests.

Each phase is independently landable and independently testable; the newtype and
the capped reader are the two that remove a defect class rather than an instance,
so they carry the rejection suites that keep the class closed.
