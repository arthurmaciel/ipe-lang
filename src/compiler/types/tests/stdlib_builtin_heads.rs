//! Pins every stdlib union named like a builtin union to its head identity.
//!
//! An `Ipe`-rooted union shares its head with the empty-home builtin of the
//! same name unless `ipe_types::STDLIB_DISTINCT_UNIONS` lists it. A stdlib
//! union that reuses a builtin union's name is therefore either a mirror (the
//! builtin's exact constructor list) or listed as distinct; anything else would
//! let a value of one type type-check where the other is expected.

use std::path::{Path, PathBuf};

use ipe_canon::builtins::BUILTIN_UNIONS;
use ipe_intern::Interner;

/// One stdlib union whose name is a builtin union's name.
#[derive(Debug)]
struct Collision {
    home: Vec<String>,
    name: String,
    /// `(constructor name, payload arity)` in declaration order.
    ctors: Vec<(String, usize)>,
}

/// Every `.ipe` file under `dir`, recursively, in a stable order.
fn ipe_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let read = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut entries = Vec::new();
    for entry in read {
        entries.push(entry.map_err(|e| format!("{}: {e}", dir.display()))?.path());
    }
    entries.sort();
    for path in entries {
        if path.is_dir() {
            ipe_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "ipe") {
            out.push(path);
        }
    }
    Ok(())
}

/// The stdlib source root.
fn stdlib_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib")
}

/// The interned string behind `sym`.
fn text(interner: &Interner, sym: ipe_intern::Symbol) -> Result<String, String> {
    interner
        .resolve(sym)
        .map(str::to_owned)
        .ok_or_else(|| format!("no backing string for symbol {}", sym.as_raw()))
}

/// Every stdlib union whose name a [`BUILTIN_UNIONS`] entry also declares.
fn collisions() -> Result<Vec<Collision>, String> {
    let root = stdlib_root();
    let mut files = Vec::new();
    ipe_files(&root, &mut files)?;
    if files.is_empty() {
        return Err(format!("no stdlib sources under {}", root.display()));
    }
    let mut found = Vec::new();
    for file in files {
        let src = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let mut interner = Interner::new();
        let module = ipe_parse::parse_module(&src, &mut interner)
            .map_err(|e| format!("{} must parse: {e:?}", file.display()))?;
        let relative = file
            .strip_prefix(&root)
            .map_err(|e| format!("{}: {e}", file.display()))?;
        let home: Vec<String> = relative
            .with_extension("")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        for union in &module.unions {
            let name = text(&interner, union.value.name.value)?;
            if !BUILTIN_UNIONS.iter().any(|b| b.type_name == name) {
                continue;
            }
            let mut ctors = Vec::with_capacity(union.value.ctors.len());
            for c in &union.value.ctors {
                ctors.push((text(&interner, c.value.name)?, c.value.args.len()));
            }
            found.push(Collision {
                home: home.clone(),
                name,
                ctors,
            });
        }
    }
    Ok(found)
}

/// Whether `home.name` is listed in `STDLIB_DISTINCT_UNIONS`.
fn listed_distinct(home: &[String], name: &str) -> bool {
    ipe_types::STDLIB_DISTINCT_UNIONS
        .iter()
        .any(|(entry_home, entry_name)| {
            *entry_name == name
                && entry_home
                    .iter()
                    .copied()
                    .eq(home.iter().map(String::as_str))
        })
}

/// Whether `ctors` is exactly the builtin union `name`'s constructor list.
fn mirrors_builtin(name: &str, ctors: &[(String, usize)]) -> bool {
    BUILTIN_UNIONS
        .iter()
        .filter(|b| b.type_name == name)
        .any(|b| {
            b.ctors.len() == ctors.len()
                && b.ctors
                    .iter()
                    .zip(ctors)
                    .all(|((want, _, arity), (got, got_arity))| {
                        *want == got.as_str() && arity == got_arity
                    })
        })
}

/// A stdlib union reusing a builtin union's name either mirrors it or is listed
/// as a distinct head.
#[test]
fn every_builtin_named_stdlib_union_is_a_mirror_or_listed_distinct() {
    let found = collisions();
    assert!(found.is_ok(), "the stdlib scan must succeed: {found:?}");
    let Ok(found) = found else {
        return;
    };
    let unclassified: Vec<String> = found
        .into_iter()
        .filter(|c| !mirrors_builtin(&c.name, &c.ctors) && !listed_distinct(&c.home, &c.name))
        .map(|c| format!("{}.{}", c.home.join("."), c.name))
        .collect();
    assert!(
        unclassified.is_empty(),
        "these stdlib unions reuse a builtin union name with other constructors and \
         would merge with the builtin head; list them in `STDLIB_DISTINCT_UNIONS`: \
         {unclassified:?}"
    );
}

/// Every `STDLIB_DISTINCT_UNIONS` entry names a stdlib union that exists and
/// does not mirror the builtin, so the table never carries a stale entry.
#[test]
fn every_listed_distinct_union_is_a_real_non_mirror() {
    let found = collisions();
    assert!(found.is_ok(), "the stdlib scan must succeed: {found:?}");
    let Ok(found) = found else {
        return;
    };
    for (home, name) in ipe_types::STDLIB_DISTINCT_UNIONS {
        let entry = found.iter().find(|c| {
            c.name == *name && home.iter().copied().eq(c.home.iter().map(String::as_str))
        });
        assert!(
            entry.is_some_and(|c| !mirrors_builtin(&c.name, &c.ctors)),
            "`{}.{name}` is listed distinct but no stdlib union of that home and \
             name declares other constructors than the builtin",
            home.join(".")
        );
    }
}
