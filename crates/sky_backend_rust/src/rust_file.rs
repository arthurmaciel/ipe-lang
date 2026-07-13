//! Backend-internal file-id domain for per-Sky-module Rust emission
//! (Phase-5 continuation — see `docs/architecture/
//! phase5-emit-rust-file-design-2026-07-12.md` §2.1). NOT yet a salsa
//! type — that is Milestone D (§4.1).

use std::collections::BTreeMap;

use sky_diagnostics::{DResult, Diagnostic, NameError, Span};
use sky_intern::Interner;
use sky_ir::{EnumDef, Func, ModPath, Program, TypeDef};

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

/// Partition every [`EnumDef`] and [`Func`] in `program` by the
/// [`RustFileId`] it is declared in.
///
/// Proven TOTAL (Task 4): every item in `program.modules[..].types /
/// .funcs` appears in EXACTLY ONE output bucket — no drop, no duplicate.
///
/// **`SqlValue`/`SqlField` Spine special case (design doc §2.2).** These two
/// synthetic enums (`sky_lower::lower`'s `synthetic_sqlvalue_enum` /
/// `synthetic_sqlfield_enum`) carry the empty canonical `home` — the SAME
/// documented Prelude-built-in home every OTHER hand-built-IR item with an
/// empty `home` carries. Left unpatched, the generic empty-home fallback
/// below would route them into whichever `SkyModule` bucket the program's
/// entry-point module happens to own — contradicting §2.2's decision that
/// they are fixed to `Spine`, unconditionally, alongside the DB-projection
/// impl blocks that reference them. So they are detected BY NAME, before
/// the generic empty-home fallback runs, reusing the exact detection idiom
/// `EmitCtx::build`'s `uses_db` scan already applies
/// (`crate::lib`'s `uses_db` / `sqlvalue_rust_name` / `sqlfield_rust_name`).
pub fn partition_items<'p>(
    program: &'p Program,
    interner: &Interner,
) -> BTreeMap<RustFileId, (Vec<&'p EnumDef>, Vec<&'p Func>)> {
    let mut out: BTreeMap<RustFileId, (Vec<&'p EnumDef>, Vec<&'p Func>)> = BTreeMap::new();
    for module in &program.modules {
        for ty in &module.types {
            let TypeDef::Enum(def) = ty;
            // §2.2 fix: SqlValue/SqlField are Prelude built-ins (empty
            // canon home, see lower.rs's own doc comment on
            // synthetic_sqlvalue_enum) that the DB-projection impl blocks
            // (ALWAYS Spine, per §2.2) reference. Force them into Spine
            // BY NAME, before the generic empty-home fallback below would
            // otherwise route them into whichever module happens to be
            // `Module.name` — reuses the exact detection idiom `uses_db`
            // already applies.
            let resolved = interner.resolve(def.name);
            if matches!(resolved, Some("SqlValue" | "SqlField")) {
                out.entry(RustFileId::Spine).or_default().0.push(def);
                continue;
            }
            let home = if def.home.0.is_empty() {
                module.name.clone()
            } else {
                def.home.clone()
            };
            out.entry(RustFileId::SkyModule(home)).or_default().0.push(def);
        }
        for func in &module.funcs {
            let home = if func.home.0.is_empty() {
                module.name.clone()
            } else {
                func.home.clone()
            };
            out.entry(RustFileId::SkyModule(home)).or_default().1.push(func);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use sky_diagnostics::DResult;
    use sky_intern::Interner;
    use sky_ir::{EnumDef, Func, FuncId, IrType, Module, Variant};

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

    /// Shared no-op module builder for the `partition_items` fixtures below
    /// — every flag `false`, no types/funcs/records, `entry: None`. Callers
    /// override `types`/`funcs`/`records`.
    fn empty_module(name: ModPath) -> Module {
        Module {
            name,
            types: vec![],
            funcs: vec![],
            entry: None,
            records: vec![],
            uses_tea: false,
            uses_server: false,
            uses_ui: false,
            uses_live: false,
            uses_tui: false,
            uses_webview: false,
            uses_css: false,
            uses_auth: false,
        }
    }

    #[test]
    fn partition_items_is_total_across_two_homes() -> DResult<()> {
        let mut interner = Interner::new();
        let lib_mod = interner.intern("Lib")?;
        let main_mod = interner.intern("Main")?;
        let lib_ty = interner.intern("Color")?;
        let main_ty = interner.intern("Msg")?;
        let red = interner.intern("Red")?;
        let increment = interner.intern("Increment")?;
        let lib_fn = interner.intern("helper")?;
        let main_fn = interner.intern("update")?;

        let mut module = empty_module(ModPath(vec![main_mod]));
        module.types = vec![
            TypeDef::Enum(EnumDef {
                name: lib_ty,
                home: ModPath(vec![lib_mod]),
                type_params: vec![],
                variants: vec![Variant {
                    name: red,
                    fields: vec![],
                }],
            }),
            TypeDef::Enum(EnumDef {
                name: main_ty,
                home: ModPath(vec![main_mod]),
                type_params: vec![],
                variants: vec![Variant {
                    name: increment,
                    fields: vec![],
                }],
            }),
        ];
        module.funcs = vec![
            Func {
                id: FuncId::from_raw(0),
                name: lib_fn,
                home: ModPath(vec![lib_mod]),
                type_params: vec![],
                params: vec![],
                ret: IrType::Int,
                body: sky_ir::Expr::Int(0),
            },
            Func {
                id: FuncId::from_raw(1),
                name: main_fn,
                home: ModPath(vec![main_mod]),
                type_params: vec![],
                params: vec![],
                ret: IrType::Int,
                body: sky_ir::Expr::Int(0),
            },
        ];

        let program = Program {
            modules: vec![module],
        };
        let buckets = partition_items(&program, &interner);

        let total_enums: usize = buckets.values().map(|(e, _)| e.len()).sum();
        let total_funcs: usize = buckets.values().map(|(_, f)| f.len()).sum();
        assert_eq!(total_enums, 2, "every EnumDef must land in exactly one bucket");
        assert_eq!(total_funcs, 2, "every Func must land in exactly one bucket");
        assert_eq!(
            buckets.len(),
            2,
            "two distinct non-empty homes must produce exactly two SkyModule buckets"
        );
        assert!(!buckets.contains_key(&RustFileId::Spine));
        Ok(())
    }

    #[test]
    fn partition_items_is_total_for_a_single_module_fixture() -> DResult<()> {
        // Mirrors `tests/golden.rs`'s `build_m0` shape: one module, every
        // item's `home` matches the module's own name — the case every
        // existing (pre-Milestone-C) golden fixture is in.
        let mut interner = Interner::new();
        let main_mod = interner.intern("Main")?;
        let msg_ty = interner.intern("Msg")?;
        let increment = interner.intern("Increment")?;
        let update = interner.intern("update")?;

        let mut module = empty_module(ModPath(vec![main_mod]));
        module.types = vec![TypeDef::Enum(EnumDef {
            name: msg_ty,
            home: ModPath(vec![main_mod]),
            type_params: vec![],
            variants: vec![Variant {
                name: increment,
                fields: vec![],
            }],
        })];
        module.funcs = vec![Func {
            id: FuncId::from_raw(0),
            name: update,
            home: ModPath(vec![main_mod]),
            type_params: vec![],
            params: vec![],
            ret: IrType::Int,
            body: sky_ir::Expr::Int(0),
        }];

        let program = Program {
            modules: vec![module],
        };
        let buckets = partition_items(&program, &interner);

        let total_enums: usize = buckets.values().map(|(e, _)| e.len()).sum();
        let total_funcs: usize = buckets.values().map(|(_, f)| f.len()).sum();
        assert_eq!(total_enums, 1);
        assert_eq!(total_funcs, 1);
        assert_eq!(buckets.len(), 1, "a single-home program must collapse to one bucket");
        Ok(())
    }

    #[test]
    fn partition_items_routes_empty_home_to_module_name_fallback() -> DResult<()> {
        let mut interner = Interner::new();
        let main_mod = interner.intern("Main")?;
        let msg_ty = interner.intern("Msg")?;
        let increment = interner.intern("Increment")?;

        let mut module = empty_module(ModPath(vec![main_mod]));
        // `home` empty simulates hand-built test IR (never the real driver
        // path, which always sets `home` — see `EnumDef::home`'s own doc
        // comment). The existing naming-layer fallback (module name) must
        // still apply for anything that is NOT SqlValue/SqlField.
        module.types = vec![TypeDef::Enum(EnumDef {
            name: msg_ty,
            home: ModPath(vec![]),
            type_params: vec![],
            variants: vec![Variant {
                name: increment,
                fields: vec![],
            }],
        })];

        let program = Program {
            modules: vec![module],
        };
        let buckets = partition_items(&program, &interner);

        let key = RustFileId::SkyModule(ModPath(vec![main_mod]));
        let (enums, _) = buckets.get(&key).expect("expected the Main-name fallback bucket");
        assert_eq!(enums.len(), 1);
        assert!(!buckets.contains_key(&RustFileId::Spine));
        Ok(())
    }

    #[test]
    fn partition_items_forces_sqlvalue_and_sqlfield_into_spine() -> DResult<()> {
        let mut interner = Interner::new();
        let main_mod = interner.intern("Main")?;
        let sqlvalue = interner.intern("SqlValue")?;
        let sqlfield = interner.intern("SqlField")?;
        let sql_string = interner.intern("SqlString")?;

        let mut module = empty_module(ModPath(vec![main_mod]));
        // `SqlValue`/`SqlField` carry the empty canonical Prelude-built-in
        // home (`sky_lower::lower`'s `synthetic_sqlvalue_enum` /
        // `synthetic_sqlfield_enum`) — matching the real driver path exactly,
        // not merely simulating hand-built IR (design doc §2.2).
        module.types = vec![
            TypeDef::Enum(EnumDef {
                name: sqlvalue,
                home: ModPath(vec![]),
                type_params: vec![],
                variants: vec![Variant {
                    name: sql_string,
                    fields: vec![],
                }],
            }),
            TypeDef::Enum(EnumDef {
                name: sqlfield,
                home: ModPath(vec![]),
                type_params: vec![],
                variants: vec![Variant {
                    name: sql_string,
                    fields: vec![],
                }],
            }),
        ];

        let program = Program {
            modules: vec![module],
        };
        let buckets = partition_items(&program, &interner);

        let (spine_enums, _) = buckets.get(&RustFileId::Spine).expect("expected a Spine bucket");
        assert_eq!(spine_enums.len(), 2, "both SqlValue and SqlField must route to Spine");
        assert!(
            !buckets.contains_key(&RustFileId::SkyModule(ModPath(vec![main_mod]))),
            "SqlValue/SqlField must NEVER fall into the generic empty-home module fallback"
        );
        Ok(())
    }
}
