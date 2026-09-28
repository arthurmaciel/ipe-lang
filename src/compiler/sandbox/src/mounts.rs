//! The mount plan shared by every bwrap jail that binds `/` read-only.
//!
//! The root bind exposes the whole host filesystem, so the invoker's homes — the
//! user home (`~/.ssh`, shell history) and the cargo home (`credentials.toml`) —
//! are masked with a tmpfs wherever they live, not only under `/home`. The only
//! paths visible below a mask are the explicit binds the caller asks for.
//!
//! bwrap applies mount ops in argv order: a later `--tmpfs` hides every earlier
//! mount below it, and a later bind re-exposes what it covers. [`push_mounts`]
//! orders the ops so that no bind emitted after `--tmpfs M` equals or contains
//! `M`: a bind that would re-expose a whole home is itself masked afterwards.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Directories masked in every jail, whoever the invoker is.
const STATIC_MASKS: [&str; 3] = ["/home", "/root", "/tmp"];

/// A host directory hidden behind a tmpfs in the jail.
///
/// Canonical, so the mask lands on the directory the jailed process resolves
/// the path to, whichever symlink it walks through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskedDir(PathBuf);

impl MaskedDir {
    /// The directory `path` resolves to, or `None` when it resolves to no
    /// directory the invoker can reach: then nothing is there for the jail to
    /// see, so nothing needs hiding.
    #[must_use]
    pub fn resolve(path: &Path) -> Option<Self> {
        let canonical = std::fs::canonicalize(path).ok()?;
        canonical.is_dir().then_some(Self(canonical))
    }

    /// The canonical directory.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// The invoker's homes, masked in every jail that binds `/`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HomeMasks {
    user_home: Option<MaskedDir>,
    cargo_home: Option<MaskedDir>,
}

impl HomeMasks {
    /// Mask `user_home` and `cargo_home` (either may be absent).
    #[must_use]
    pub const fn new(user_home: Option<MaskedDir>, cargo_home: Option<MaskedDir>) -> Self {
        Self {
            user_home,
            cargo_home,
        }
    }

    /// The homes of the invoking process: `HOME` and `CARGO_HOME`.
    ///
    /// An unset `CARGO_HOME` defaults to `$HOME/.cargo`, which the user-home
    /// mask already covers.
    #[must_use]
    pub fn of_invoker() -> Self {
        let user_home = crate::home::home_dir().and_then(|home| MaskedDir::resolve(&home));
        let cargo_home = std::env::var_os("CARGO_HOME")
            .filter(|raw| !raw.is_empty())
            .and_then(|raw| MaskedDir::resolve(Path::new(&raw)));
        Self::new(user_home, cargo_home)
    }

    fn dirs(&self) -> impl Iterator<Item = &Path> {
        self.user_home
            .iter()
            .chain(self.cargo_home.iter())
            .map(MaskedDir::as_path)
    }
}

/// One path a jail re-exposes at the same location inside the jail.
#[derive(Debug, Clone, Copy)]
pub enum Bind<'a> {
    /// `--ro-bind`: visible, never writable.
    ReadOnly(&'a Path),
    /// `--bind`: visible and writable.
    ReadWrite(&'a Path),
}

impl<'a> Bind<'a> {
    const fn path(self) -> &'a Path {
        match self {
            Self::ReadOnly(path) | Self::ReadWrite(path) => path,
        }
    }

    const fn flag(self) -> &'static str {
        match self {
            Self::ReadOnly(_) => "--ro-bind",
            Self::ReadWrite(_) => "--bind",
        }
    }
}

/// The canonical form of `path`, or `path` itself when it does not resolve
/// (bwrap then refuses the bind, so its placement cannot expose anything).
fn canonical_or_given(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn depth(path: &Path) -> usize {
    path.components().count()
}

/// Push the masks and `binds` onto `argv`.
///
/// Masks are emitted shallowest first. Each bind is emitted right after the
/// mask that most closely contains it, strictly; a bind no mask strictly
/// contains is emitted before every mask. A bind that equals or contains a
/// mask therefore always precedes that mask's `--tmpfs`, which hides what the
/// bind would have exposed there. Binds keep their relative order within one
/// mask. Every path is emitted in canonical form, the form bwrap mounts at.
pub fn push_mounts(argv: &mut Vec<OsString>, homes: &HomeMasks, binds: &[Bind<'_>]) {
    let mut masks: Vec<PathBuf> = STATIC_MASKS
        .iter()
        .map(|mask| canonical_or_given(Path::new(mask)))
        .chain(homes.dirs().map(Path::to_path_buf))
        .collect();
    masks.sort_by(|a, b| depth(a).cmp(&depth(b)).then_with(|| a.cmp(b)));
    masks.dedup();
    let placed: Vec<(Option<usize>, &'static str, PathBuf)> = binds
        .iter()
        .map(|bind| {
            let path = canonical_or_given(bind.path());
            let mask = masks
                .iter()
                .enumerate()
                .filter(|(_, mask)| path != **mask && path.starts_with(mask))
                .max_by_key(|(_, mask)| depth(mask))
                .map(|(index, _)| index);
            (mask, bind.flag(), path)
        })
        .collect();
    let push_binds_of = |argv: &mut Vec<OsString>, owner: Option<usize>| {
        for (_, flag, path) in placed.iter().filter(|(mask, _, _)| *mask == owner) {
            argv.push((*flag).into());
            argv.push(path.clone().into());
            argv.push(path.clone().into());
        }
    };
    push_binds_of(argv, None);
    for (index, mask) in masks.iter().enumerate() {
        argv.push("--tmpfs".into());
        argv.push(mask.clone().into());
        push_binds_of(argv, Some(index));
    }
}

/// Test oracle: the first bind that follows a `--tmpfs` it equals or contains
/// (which would re-expose the masked tree), as `(mask, bind)`.
#[cfg(test)]
pub(crate) fn bind_after_covered_mask(argv: &[String]) -> Option<(String, String)> {
    let mut masks: Vec<&str> = Vec::new();
    let mut ops = argv.iter().map(String::as_str);
    while let Some(op) = ops.next() {
        match op {
            "--tmpfs" => masks.extend(ops.next()),
            "--bind" | "--ro-bind" => {
                let Some(dest) = ops.nth(1) else {
                    continue;
                };
                if let Some(mask) = masks.iter().find(|mask| Path::new(mask).starts_with(dest)) {
                    return Some(((*mask).to_owned(), dest.to_owned()));
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::expect_used)] // test fixture: a canonical temp dir must exist
    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ipe-sandbox-mounts-{label}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bin")).expect("create temp dir");
        std::fs::canonicalize(&dir).expect("canonical temp dir")
    }

    #[allow(clippy::expect_used)] // test fixture: the directory must exist
    fn make_dir(path: &Path) {
        std::fs::create_dir_all(path).expect("create dir");
    }

    fn rendered(homes: &HomeMasks, binds: &[Bind<'_>]) -> Vec<String> {
        let mut argv = Vec::new();
        push_mounts(&mut argv, homes, binds);
        argv.into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn position(argv: &[String], window: &[&str]) -> Option<usize> {
        argv.windows(window.len())
            .position(|w| w.iter().zip(window).all(|(a, b)| a == b))
    }

    #[test]
    fn a_missing_home_resolves_to_no_mask() {
        assert_eq!(
            MaskedDir::resolve(Path::new("/nonexistent/ipe-sandbox-home")),
            None
        );
    }

    #[test]
    fn a_cargo_home_outside_home_is_masked_then_only_bin_rebound() {
        let cargo_home = temp_dir("cargo-home-outside");
        let bin = cargo_home.join("bin");
        let homes = HomeMasks::new(None, MaskedDir::resolve(&cargo_home));
        let argv = rendered(&homes, &[Bind::ReadOnly(&bin)]);
        let home = cargo_home.to_string_lossy().into_owned();
        let bin = bin.to_string_lossy().into_owned();
        let mask = position(&argv, &["--tmpfs", &home]).expect("cargo home masked");
        let rebind = position(&argv, &["--ro-bind", &bin, &bin]).expect("bin re-bound");
        assert!(
            rebind > mask,
            "bin must be re-bound after the mask: {argv:?}"
        );
        assert_eq!(bind_after_covered_mask(&argv), None, "{argv:?}");
    }

    #[test]
    fn a_user_home_outside_home_is_masked() {
        let user_home = temp_dir("user-home-outside");
        let homes = HomeMasks::new(MaskedDir::resolve(&user_home), None);
        let argv = rendered(&homes, &[]);
        let home = user_home.to_string_lossy().into_owned();
        assert!(position(&argv, &["--tmpfs", &home]).is_some(), "{argv:?}");
    }

    #[test]
    fn a_bind_equal_to_or_covering_a_home_is_masked_after() {
        let cargo_home = temp_dir("cargo-home-covered");
        let parent = cargo_home.parent().map(Path::to_path_buf);
        let homes = HomeMasks::new(None, MaskedDir::resolve(&cargo_home));
        let mut binds = vec![Bind::ReadOnly(&cargo_home)];
        if let Some(parent) = &parent {
            binds.push(Bind::ReadWrite(parent));
        }
        let argv = rendered(&homes, &binds);
        assert_eq!(bind_after_covered_mask(&argv), None, "{argv:?}");
        let home = cargo_home.to_string_lossy().into_owned();
        let mask = position(&argv, &["--tmpfs", &home]).expect("cargo home masked");
        let bind = position(&argv, &["--ro-bind", &home, &home]).expect("home bind");
        assert!(
            bind < mask,
            "a bind of the home itself must be masked: {argv:?}"
        );
    }

    #[test]
    fn nested_homes_each_get_their_own_mask() {
        let user_home = temp_dir("nested-user");
        let cargo_home = user_home.join("cargo");
        let bin = cargo_home.join("bin");
        make_dir(&bin);
        let homes = HomeMasks::new(
            MaskedDir::resolve(&user_home),
            MaskedDir::resolve(&cargo_home),
        );
        // A whole-user-home bind would re-expose the cargo home without its
        // own mask.
        let argv = rendered(&homes, &[Bind::ReadOnly(&user_home), Bind::ReadOnly(&bin)]);
        assert_eq!(bind_after_covered_mask(&argv), None, "{argv:?}");
        let cargo = cargo_home.to_string_lossy().into_owned();
        let user = user_home.to_string_lossy().into_owned();
        let cargo_mask = position(&argv, &["--tmpfs", &cargo]).expect("cargo mask");
        let user_mask = position(&argv, &["--tmpfs", &user]).expect("user mask");
        assert!(cargo_mask > user_mask, "{argv:?}");
    }

    #[test]
    fn the_oracle_flags_a_bind_that_re_exposes_a_mask() {
        let argv: Vec<String> = ["--tmpfs", "/srv/home", "--ro-bind", "/srv", "/srv"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(
            bind_after_covered_mask(&argv),
            Some(("/srv/home".to_owned(), "/srv".to_owned()))
        );
    }
}
