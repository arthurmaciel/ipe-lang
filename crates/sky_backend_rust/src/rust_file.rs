//! Backend-internal file-id domain for per-Sky-module Rust emission
//! (Phase-5 continuation — see `docs/architecture/
//! phase5-emit-rust-file-design-2026-07-12.md` §2.1). NOT yet a salsa
//! type — that is Milestone D (§4.1).

use sky_diagnostics::{DResult, Diagnostic, NameError, Span};
use sky_intern::Interner;
use sky_ir::ModPath;

use crate::naming;

/// Which Rust file a program's item (an [`sky_ir::EnumDef`] or
/// [`sky_ir::Func`]) is declared in. `Spine` is the always-present entry
/// file (`src/main.rs`); a `SkyModule` is one Sky module's OWN file
/// (`src/sky_mods/<ident>.rs`), materialised only when 2+ distinct homes are
/// present (the Spine-collapse invariant — see the design doc §3.3).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum RustFileId {
    Spine,
    SkyModule(ModPath),
}

/// The Rust module identifier for Sky module path `home` — e.g.
/// `["Std", "Palette"]` → `sky_mod_std_palette`, `["Lib"]` → `sky_mod_lib`.
///
/// Reuses the same base fold [`naming::module_prefix`] already applies for
/// the value/type case, snake-cased and prefixed with `sky_mod_` to keep it
/// visually distinct from the vendored `sky_runtime` module tree. This is a
/// NEW namespace (design doc §2.1.1) — nothing before this task needed a
/// `ModPath -> Rust identifier` folding for a `mod` declaration, because
/// there was only ever one file.
#[must_use]
pub fn mod_ident(home: &[&str]) -> String {
    format!(
        "sky_mod_{}",
        naming::to_snake_case(&naming::module_prefix(home))
    )
}

/// Resolve `home`'s [`mod_ident`] through `interner`. A symbol that fails to
/// resolve is never reachable on the real driver path (only malformed
/// hand-built test IR) — surfaced as a [`Diagnostic::CompilerBug`], never a
/// panic.
fn resolve_mod_ident(home: &ModPath, interner: &Interner) -> DResult<String> {
    let segs = home
        .0
        .iter()
        .map(|sym| {
            interner
                .resolve(*sym)
                .ok_or_else(|| Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::rust_file::resolve_mod_ident",
                    detail: format!("symbol {} not present in interner", sym.as_raw()),
                })
        })
        .collect::<DResult<Vec<&str>>>()?;
    Ok(mod_ident(&segs))
}

/// Fail closed if two DISTINCT `home`s among `ids`' [`RustFileId::SkyModule`]
/// entries fold to the same [`mod_ident`]. Mirrors the EXISTING
/// `func_names.values().any(...)` / `enum_names.values().any(...)`
/// fail-closed pattern (`crate::lib`) — same `Diagnostic::Name::
/// DuplicateValue` shape, applied to the one genuinely new namespace this
/// design introduces (§2.1.1). `RustFileId::Spine` carries no `mod_ident`
/// and is skipped.
pub fn assert_mod_idents_unique(ids: &[RustFileId], interner: &Interner) -> DResult<()> {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for id in ids {
        let RustFileId::SkyModule(home) = id else {
            continue;
        };
        let ident = resolve_mod_ident(home, interner)?;
        if !seen.insert(ident.clone()) {
            return Err(Diagnostic::Name {
                span: Span::DUMMY,
                msg: NameError::DuplicateValue {
                    name: ident.into_boxed_str(),
                    first: Span::DUMMY,
                },
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sky_diagnostics::DResult;
    use sky_intern::Interner;

    use super::*;

    #[test]
    fn spine_is_not_a_sky_module() {
        assert_ne!(RustFileId::Spine, RustFileId::SkyModule(ModPath(vec![])));
    }

    #[test]
    fn mod_ident_is_stable_and_distinct_for_distinct_homes() {
        let a = mod_ident(&["Std", "Palette"]);
        let b = mod_ident(&["Lib"]);
        assert_ne!(a, b);
        assert_eq!(a, "sky_mod_std_palette");
        assert_eq!(b, "sky_mod_lib");
    }

    #[test]
    fn duplicate_mod_idents_fail_closed() -> DResult<()> {
        // Two DISTINCT `ModPath`s that fold to the SAME `mod_ident` under
        // the current `module_prefix` fold ("_"-joined segments, then
        // snake-cased): a single segment spelled "Std_Ui" and a two-segment
        // path ["Std", "Ui"] both join to "Std_Ui" before snake-casing —
        // a real, constructible collision, not a hedged placeholder (mirrors
        // the AUD-08 collision class `crate::lib.rs` already documents for
        // func/enum names, e.g. `["Std","Ui"]/borderRounded` vs
        // `["Std","Ui","Border"]/rounded`).
        let mut interner = Interner::new();
        let combined = interner.intern("Std_Ui")?;
        let std_seg = interner.intern("Std")?;
        let ui_seg = interner.intern("Ui")?;
        let ids = vec![
            RustFileId::SkyModule(ModPath(vec![combined])),
            RustFileId::SkyModule(ModPath(vec![std_seg, ui_seg])),
        ];
        let result = assert_mod_idents_unique(&ids, &interner);
        assert!(
            result.is_err(),
            "expected a fail-closed collision, got {result:?}"
        );
        Ok(())
    }

    #[test]
    fn distinct_mod_idents_do_not_fail_closed() -> DResult<()> {
        let mut interner = Interner::new();
        let lib = interner.intern("Lib")?;
        let main = interner.intern("Main")?;
        let ids = vec![
            RustFileId::SkyModule(ModPath(vec![lib])),
            RustFileId::SkyModule(ModPath(vec![main])),
        ];
        assert_mod_idents_unique(&ids, &interner)
    }
}
