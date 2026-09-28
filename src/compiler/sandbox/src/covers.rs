//! Whether a read-only bind would expose the cargo home.
//!
//! The cargo home holds `credentials.toml`; a jail may bind its `bin`,
//! `registry`, and `git` subdirectories but never the home itself nor any
//! directory at or above it.

use std::path::{Path, PathBuf};

use crate::CanonicalPath;

/// The first of `binds` that would make `cargo_home` visible inside the jail.
///
/// Every jail builder refuses a bind set for which this returns `Some`: binding
/// such a path exposes `credentials.toml`.
#[must_use]
pub fn bind_exposing<'a>(
    binds: &'a [CanonicalPath],
    cargo_home: &Path,
) -> Option<&'a CanonicalPath> {
    binds
        .iter()
        .find(|bind| path_covers(bind.as_path(), cargo_home))
}

/// Whether binding `outer` makes `inner` visible.
///
/// `inner` equals or lies under `outer`, judged lexically, with symlinks
/// resolved, and by directory identity. Any judgement finding containment is
/// enough.
#[must_use]
pub fn path_covers(outer: &Path, inner: &Path) -> bool {
    lexical_normal(inner).starts_with(lexical_normal(outer))
        || resolved(inner).starts_with(resolved(outer))
        || covers_by_identity(outer, inner)
}

/// Whether some ancestor of `inner` (itself included) is the directory
/// `outer`, compared by `(dev, ino)`.
///
/// This also catches a bind-mount alias no path comparison sees. `link(2)`
/// refuses directories, so one `(dev, ino)` names exactly one directory and no
/// hardlinked alias exists. An `outer` that does not exist exposes nothing; a
/// missing ancestor of `inner` cannot be `outer`. Any other metadata error
/// counts as covered.
#[cfg(unix)]
fn covers_by_identity(outer: &Path, inner: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let identity = |path: &Path| std::fs::metadata(path).map(|meta| (meta.dev(), meta.ino()));
    let outer_id = match identity(outer) {
        Ok(id) => id,
        Err(e) => return e.kind() != std::io::ErrorKind::NotFound,
    };
    inner
        .ancestors()
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .any(|ancestor| match identity(ancestor) {
            Ok(id) => id == outer_id,
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        })
}

/// Directory identity has no portable form off unix; the lexical and resolved
/// judgements of [`path_covers`] stand alone there.
#[cfg(not(unix))]
const fn covers_by_identity(_outer: &Path, _inner: &Path) -> bool {
    false
}

/// `path` without `.` components, each `..` removing the component before it
/// (never above the root); trailing separators vanish with the components.
fn lexical_normal(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                out.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
        }
    }
    out
}

/// `path` with symlinks resolved as the kernel resolves them.
///
/// A `..` after a symlink climbs from the link's target. A path that does not
/// exist yet resolves through its longest existing ancestor, with the missing
/// tail appended and the whole normalized lexically.
fn resolved(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if let Ok(real) = std::fs::canonicalize(ancestor) {
            return path.strip_prefix(ancestor).map_or_else(
                |_| lexical_normal(path),
                |tail| lexical_normal(&real.join(tail)),
            );
        }
    }
    lexical_normal(path)
}

#[cfg(test)]
#[allow(clippy::expect_used)] // test fixtures: the scratch dirs must exist
mod tests {
    use super::*;
    use crate::test_dir::TestDir;

    fn make_dir(path: &Path) {
        std::fs::create_dir_all(path).expect("create dir");
    }

    #[cfg(unix)]
    #[test]
    fn identity_sees_an_equal_or_enclosing_dir_and_nothing_else() {
        let tmp_dir = TestDir::new("covers-identity");
        let tmp = tmp_dir.path();
        let cargo_home = tmp.join(".cargo");
        let other = tmp.join("other");
        make_dir(&cargo_home.join("bin"));
        make_dir(&other);
        assert!(
            covers_by_identity(&cargo_home, &cargo_home),
            "the directory itself is covered"
        );
        assert!(
            covers_by_identity(tmp, &cargo_home),
            "an enclosing directory covers it"
        );
        assert!(
            !covers_by_identity(&cargo_home.join("bin"), &cargo_home),
            "a directory below does not cover it"
        );
        assert!(
            !covers_by_identity(&other, &cargo_home),
            "a disjoint directory does not cover it"
        );
        assert!(
            !covers_by_identity(&tmp.join("absent"), &cargo_home),
            "a missing bind exposes nothing"
        );
        assert!(
            !covers_by_identity(&other, &cargo_home.join("absent")),
            "a missing tail cannot be the bind"
        );
    }

    #[test]
    fn a_bind_at_or_above_the_cargo_home_exposes_it() {
        let tmp_dir = TestDir::new("covers-exposing");
        let tmp = tmp_dir.path();
        let cargo_home = tmp.join(".cargo");
        make_dir(&cargo_home.join("bin"));
        let canonical = |path: &Path| CanonicalPath::resolve(path).expect("canonical");
        let bin = canonical(&cargo_home.join("bin"));
        for exposing in [canonical(&cargo_home), canonical(tmp)] {
            let binds = [bin.clone(), exposing.clone()];
            assert_eq!(bind_exposing(&binds, &cargo_home), Some(&exposing));
        }
        assert_eq!(bind_exposing(std::slice::from_ref(&bin), &cargo_home), None);
    }

    #[test]
    fn lexical_dot_dot_cannot_hide_the_cargo_home() {
        assert!(path_covers(
            Path::new("/opt/tools/../home"),
            Path::new("/opt/home/.cargo")
        ));
        assert!(!path_covers(
            Path::new("/opt/home/.cargo/bin"),
            Path::new("/opt/home/.cargo")
        ));
    }
}
