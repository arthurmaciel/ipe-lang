//! `ipe diff <old> <new>` — public-API delta + enforced semver.
//!
//! Compares two package versions by their [`crate::api_surface::PublicApi`],
//! classifies each change as breaking or compatible (Elm's `elm diff` rules,
//! mapped to Ipê's pre-1.0 semver), and derives the required version bump. The
//! gate consumes [`check_semver_bump`] to reject a version that under-bumps.
//!
//! Ipê is pre-1.0, so a major bump is reserved: a **breaking** delta requires a
//! **minor** bump, a **compatible** delta a **patch** bump (matching the
//! release-please config). The classifier is conservative (Security first): a
//! change it cannot prove compatible is breaking — a false-breaking wastes a
//! version number, a false-compatible ships a silent break.

use std::path::Path;

use semver::Version;

use crate::api_surface::{DiffError, ModuleApi, PublicApi, extract_tree};

/// Whether a public-API delta breaks existing users.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compatibility {
    /// No change, or a purely additive one — existing users keep compiling.
    Compatible,
    /// A removal, rename, or type change — existing users may break.
    Breaking,
}

/// The minimum version-component bump a delta requires (pre-1.0 mapping).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequiredBump {
    /// A patch bump (`0.y.Z`) — a compatible delta (or no change; a re-release
    /// still needs a new version).
    Patch,
    /// A minor bump (`0.Y.0`) — a breaking delta (major is reserved pre-1.0).
    Minor,
}

/// One classified difference between two public APIs.
///
/// A closed sum over the finite change kinds — the classifier's `match` over it
/// is exhaustive, so a new change kind is a compile error, never a silently
/// mis-classified magnitude.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiChange {
    /// An exported module present in `old` is gone from `new`.
    ModuleRemoved { module: String },
    /// An exported module present in `new` is new.
    ModuleAdded { module: String },
    /// An exported value was removed (a rename surfaces as a removal + an add).
    ValueRemoved { module: String, name: String },
    /// A new exported value.
    ValueAdded { module: String, name: String },
    /// An exported value's type signature changed.
    ValueChanged {
        module: String,
        name: String,
        old: String,
        new: String,
    },
    /// An exported union type was removed.
    UnionRemoved { module: String, name: String },
    /// A new exported union type.
    UnionAdded { module: String, name: String },
    /// An exported union's type-parameter arity changed.
    UnionArityChanged {
        module: String,
        name: String,
        old: usize,
        new: usize,
    },
    /// A constructor was removed from an exported union.
    ConstructorRemoved {
        module: String,
        union: String,
        ctor: String,
    },
    /// A constructor was added to an exported union.
    ConstructorAdded {
        module: String,
        union: String,
        ctor: String,
    },
    /// A constructor's argument types changed.
    ConstructorChanged {
        module: String,
        union: String,
        ctor: String,
    },
}

impl ApiChange {
    /// Classify this change's compatibility (Elm's rules, conservative).
    ///
    /// Additive changes (a new module / value / union) are compatible; every
    /// removal, rename, or type change is breaking. A new constructor to an
    /// exposed union is breaking — importers may write exhaustive `case`
    /// expressions a new variant would break — matching Elm and Ipê's exhaustive
    /// matching.
    #[must_use]
    pub fn compatibility(&self) -> Compatibility {
        match self {
            Self::ModuleAdded { .. } | Self::ValueAdded { .. } | Self::UnionAdded { .. } => {
                Compatibility::Compatible
            }
            Self::ModuleRemoved { .. }
            | Self::ValueRemoved { .. }
            | Self::ValueChanged { .. }
            | Self::UnionRemoved { .. }
            | Self::UnionArityChanged { .. }
            | Self::ConstructorRemoved { .. }
            | Self::ConstructorAdded { .. }
            | Self::ConstructorChanged { .. } => Compatibility::Breaking,
        }
    }
}

/// The full result of comparing two package versions: the classified changes,
/// the required bump, and whether the proposed new version clears its floor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemverReport {
    /// Every classified change, in a deterministic order.
    pub changes: Vec<ApiChange>,
    /// The overall compatibility (breaking if any change is breaking).
    pub compatibility: Compatibility,
    /// The minimum bump the delta requires.
    pub required: RequiredBump,
    /// The minimum acceptable new version given `old_version` and `required`.
    pub floor: Version,
    /// Whether the proposed new version is at or above the floor.
    pub satisfied: bool,
}

/// Diff `old` against `new`, producing the classified changes in deterministic
/// (module-then-name) order.
#[must_use]
pub fn diff_api(old: &PublicApi, new: &PublicApi) -> Vec<ApiChange> {
    let mut changes = Vec::new();

    for module in old.modules.keys() {
        if !new.modules.contains_key(module) {
            changes.push(ApiChange::ModuleRemoved {
                module: module.join("."),
            });
        }
    }
    for module in new.modules.keys() {
        if !old.modules.contains_key(module) {
            changes.push(ApiChange::ModuleAdded {
                module: module.join("."),
            });
        }
    }

    // Modules present in both: diff their surfaces.
    for (module, old_api) in &old.modules {
        let Some(new_api) = new.modules.get(module) else {
            continue;
        };
        diff_module(&module.join("."), old_api, new_api, &mut changes);
    }

    changes
}

/// Diff one module's values and unions, appending to `changes`.
fn diff_module(module: &str, old: &ModuleApi, new: &ModuleApi, changes: &mut Vec<ApiChange>) {
    for (name, old_sig) in &old.values {
        match new.values.get(name) {
            None => changes.push(ApiChange::ValueRemoved {
                module: module.to_owned(),
                name: name.clone(),
            }),
            Some(new_sig) if new_sig != old_sig => changes.push(ApiChange::ValueChanged {
                module: module.to_owned(),
                name: name.clone(),
                old: old_sig.clone(),
                new: new_sig.clone(),
            }),
            Some(_) => {}
        }
    }
    for name in new.values.keys() {
        if !old.values.contains_key(name) {
            changes.push(ApiChange::ValueAdded {
                module: module.to_owned(),
                name: name.clone(),
            });
        }
    }

    for (name, old_union) in &old.unions {
        match new.unions.get(name) {
            None => changes.push(ApiChange::UnionRemoved {
                module: module.to_owned(),
                name: name.clone(),
            }),
            Some(new_union) => {
                if new_union.params != old_union.params {
                    changes.push(ApiChange::UnionArityChanged {
                        module: module.to_owned(),
                        name: name.clone(),
                        old: old_union.params,
                        new: new_union.params,
                    });
                }
                for (ctor, old_args) in &old_union.ctors {
                    match new_union.ctors.get(ctor) {
                        None => changes.push(ApiChange::ConstructorRemoved {
                            module: module.to_owned(),
                            union: name.clone(),
                            ctor: ctor.clone(),
                        }),
                        Some(new_args) if new_args != old_args => {
                            changes.push(ApiChange::ConstructorChanged {
                                module: module.to_owned(),
                                union: name.clone(),
                                ctor: ctor.clone(),
                            });
                        }
                        Some(_) => {}
                    }
                }
                for ctor in new_union.ctors.keys() {
                    if !old_union.ctors.contains_key(ctor) {
                        changes.push(ApiChange::ConstructorAdded {
                            module: module.to_owned(),
                            union: name.clone(),
                            ctor: ctor.clone(),
                        });
                    }
                }
            }
        }
    }
    for name in new.unions.keys() {
        if !old.unions.contains_key(name) {
            changes.push(ApiChange::UnionAdded {
                module: module.to_owned(),
                name: name.clone(),
            });
        }
    }
}

/// The overall compatibility of a set of changes: breaking if any change is
/// breaking, else compatible (the max-magnitude fold, collapsed to two
/// outcomes). An empty delta is compatible.
#[must_use]
pub fn magnitude(changes: &[ApiChange]) -> Compatibility {
    if changes
        .iter()
        .any(|c| c.compatibility() == Compatibility::Breaking)
    {
        Compatibility::Breaking
    } else {
        Compatibility::Compatible
    }
}

/// Map a delta's compatibility to its required bump (pre-1.0).
#[must_use]
pub fn required_bump(compat: Compatibility) -> RequiredBump {
    match compat {
        Compatibility::Compatible => RequiredBump::Patch,
        Compatibility::Breaking => RequiredBump::Minor,
    }
}

/// The minimum acceptable new version given the old version and the required
/// bump (pre-1.0 floors).
///
/// - `Patch`: any strict increase over `old` (`0.y.(z+1)` is the smallest).
/// - `Minor`: at least `0.(y+1).0` — a patch bump does not clear it.
#[must_use]
pub fn bump_floor(old: &Version, required: RequiredBump) -> Version {
    match required {
        RequiredBump::Patch => Version::new(old.major, old.minor, old.patch.saturating_add(1)),
        RequiredBump::Minor => Version::new(old.major, old.minor.saturating_add(1), 0),
    }
}

/// Compare two package trees + their versions and produce the full report.
///
/// # Errors
/// [`DiffError`] when either tree cannot be read, does not typecheck, or exposes
/// an open interface.
pub fn check_semver_bump(
    old_tree: &Path,
    new_tree: &Path,
    old_version: &Version,
    new_version: &Version,
) -> Result<SemverReport, DiffError> {
    let old_api = extract_tree(old_tree)?;
    let new_api = extract_tree(new_tree)?;
    Ok(report(&old_api, &new_api, old_version, new_version))
}

/// Build the [`SemverReport`] for two already-extracted APIs + versions.
#[must_use]
pub fn report(
    old_api: &PublicApi,
    new_api: &PublicApi,
    old_version: &Version,
    new_version: &Version,
) -> SemverReport {
    let changes = diff_api(old_api, new_api);
    let compatibility = magnitude(&changes);
    let required = required_bump(compatibility);
    let floor = bump_floor(old_version, required);
    let satisfied = *new_version >= floor;
    SemverReport {
        changes,
        compatibility,
        required,
        floor,
        satisfied,
    }
}
