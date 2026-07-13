//! Backend-internal file-id domain for per-Sky-module Rust emission
//! (Phase-5 continuation — see `docs/architecture/
//! phase5-emit-rust-file-design-2026-07-12.md` §2.1). NOT yet a salsa
//! type — that is Milestone D (§4.1).

use sky_ir::ModPath;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spine_is_not_a_sky_module() {
        assert_ne!(RustFileId::Spine, RustFileId::SkyModule(ModPath(vec![])));
    }
}
