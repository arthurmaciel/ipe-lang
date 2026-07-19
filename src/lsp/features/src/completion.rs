//! Completion: in-scope identifiers at a cursor position.
//!
//! Reads `canonicalize` (for every in-scope name and its resolved kind) and
//! `typecheck` (for rendered type strings in completion details) from the same
//! memoized `ipe_db` queries the compiler runs. A program that does not
//! type-check still yields completion items — the provider degrades gracefully
//! to kind-only items without a type annotation rather than returning nothing.
//!
//! Scope model: all top-level names of the current module plus all names
//! imported into it via `import … exposing (…)` or `import … exposing (..)`.
//! No positional block-scope narrowing — every in-scope top-level name is
//! offered regardless of cursor position. Keywords are appended last and
//! deduplicated against any same-named user identifier.
//!
//! Lock discipline: salsa queries (which acquire the interner internally) are
//! all demanded BEFORE the caller acquires the interner lock — no nested
//! locking.

use std::collections::BTreeMap;

use ipe_db::{Db as _, IpeDatabase, SourceRoot};
use ipe_intern::Symbol;
use ipe_types::Ty;
use lsp_types::{CompletionItem, CompletionItemKind};

/// Represents a collected completion candidate before string resolution.
struct Candidate {
    name: Symbol,
    home: Vec<Symbol>,
    kind: CandidateKind,
}

enum CandidateKind {
    Value,
    Ctor,
    Type,
}

/// All completion candidates visible in `module`.
///
/// Returns an empty list for an unknown module or an unparseable project.
#[must_use]
pub fn completions(
    db: &IpeDatabase,
    root: SourceRoot,
    entry: ipe_db::SourceFile,
    module: &[String],
) -> Vec<CompletionItem> {
    let files = root.files(db);
    let Some(&file) = files.get(module) else {
        return Vec::new();
    };

    // Demand all salsa queries before locking the interner — each query
    // internally acquires + releases the interner lock, and the Mutex is not
    // reentrant. All results are cloned out of their `Arc` wrappers here so
    // the interner is free when we acquire it below.
    let canonical = ipe_db::canonicalize(db, root, file).ok();
    let dep_canonicals = collect_dep_canonicals(db, root, file);
    let solved_env: Option<BTreeMap<(Vec<Symbol>, Symbol), Ty>> =
        ipe_db::typecheck(db, root, entry)
            .ok()
            .map(|solved| solved.env.clone());

    // Acquire the interner once — all subsequent work is resolution-only.
    let mut interner = db.interner().lock();

    let home_syms: Vec<Symbol> = module
        .iter()
        .map(|s| interner.intern(s).ok())
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default();

    let candidates = build_candidates(
        &mut interner,
        canonical.as_ref(),
        &home_syms,
        &dep_canonicals,
    );

    let mut items: Vec<CompletionItem> =
        render_candidates(&candidates, solved_env.as_ref(), &interner);

    drop(interner);

    for kw in KEYWORDS {
        items.push(keyword_item(kw));
    }

    // Deduplicate by label — first occurrence wins (current-module names
    // have priority over imported names; both beat keywords).
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    items.retain(|item| seen.insert(item.label.clone()));

    items
}

/// Fetch the canonical module for each resolved import, before the interner
/// is locked. Returns `(dep_path, canonical_module)` pairs.
fn collect_dep_canonicals(
    db: &IpeDatabase,
    root: SourceRoot,
    file: ipe_db::SourceFile,
) -> Vec<(Vec<String>, ipe_db::CanonicalModule)> {
    let Ok(resolutions) = ipe_db::resolve_imports(db, root, file) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (dep_path, resolution) in resolutions.iter() {
        if let ipe_db::ImportResolution::Resolved(dep_file) = resolution
            && let Ok(dep_canon) = ipe_db::canonicalize(db, root, *dep_file)
        {
            out.push((dep_path.clone(), (*dep_canon).clone()));
        }
    }
    out
}

/// Collect all raw candidates (symbol + home + kind) while the interner is
/// held, so that intern calls for dep paths are batched in one lock window.
fn build_candidates(
    interner: &mut ipe_intern::Interner,
    canonical: Option<&std::sync::Arc<ipe_db::CanonicalModule>>,
    home_syms: &[Symbol],
    dep_canonicals: &[(Vec<String>, ipe_db::CanonicalModule)],
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();

    if let Some(canon) = canonical {
        for def in &canon.module.defs {
            out.push(Candidate {
                name: def.name().value,
                home: home_syms.to_vec(),
                kind: CandidateKind::Value,
            });
        }
        for union in &canon.module.unions {
            out.push(Candidate {
                name: union.name,
                home: home_syms.to_vec(),
                kind: CandidateKind::Type,
            });
            for ctor in &union.ctors {
                out.push(Candidate {
                    name: ctor.name,
                    home: home_syms.to_vec(),
                    kind: CandidateKind::Ctor,
                });
            }
        }
    }

    for (dep_path, dep_canon) in dep_canonicals {
        let dep_home: Vec<Symbol> = dep_path
            .iter()
            .map(|s| interner.intern(s).ok())
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default();
        for &name_sym in &dep_canon.exports.values {
            out.push(Candidate {
                name: name_sym,
                home: dep_home.clone(),
                kind: CandidateKind::Value,
            });
        }
        for &ctor_sym in dep_canon.exports.ctors.keys() {
            out.push(Candidate {
                name: ctor_sym,
                home: dep_home.clone(),
                kind: CandidateKind::Ctor,
            });
        }
        for &type_sym in dep_canon.exports.types.keys() {
            out.push(Candidate {
                name: type_sym,
                home: dep_home.clone(),
                kind: CandidateKind::Type,
            });
        }
    }
    out
}

/// Resolve each candidate to a `CompletionItem`, adding type detail for
/// values when available.
fn render_candidates(
    candidates: &[Candidate],
    solved_env: Option<&BTreeMap<(Vec<Symbol>, Symbol), Ty>>,
    interner: &ipe_intern::Interner,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for c in candidates {
        let Some(name_str) = interner.resolve(c.name) else {
            continue;
        };
        let detail = match c.kind {
            CandidateKind::Value => solved_env
                .and_then(|env| env.get(&(c.home.clone(), c.name)))
                .and_then(|ty| {
                    let mut namer = ipe_types::VarNamer::new();
                    ipe_types::ty_to_doc(ty, interner, &mut namer)
                        .ok()
                        .map(|doc| ipe_diagnostics::render_ty(&doc))
                }),
            CandidateKind::Ctor | CandidateKind::Type => None,
        };
        items.push(match c.kind {
            CandidateKind::Value => value_item(name_str.to_owned(), detail),
            CandidateKind::Ctor => ctor_item(name_str.to_owned()),
            CandidateKind::Type => type_item(name_str.to_owned()),
        });
    }
    items
}

fn value_item(label: String, detail: Option<String>) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(CompletionItemKind::FUNCTION),
        detail,
        ..CompletionItem::default()
    }
}

fn ctor_item(label: String) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(CompletionItemKind::ENUM_MEMBER),
        ..CompletionItem::default()
    }
}

fn type_item(label: String) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(CompletionItemKind::CLASS),
        ..CompletionItem::default()
    }
}

fn keyword_item(label: &'static str) -> CompletionItem {
    CompletionItem {
        label: label.to_owned(),
        kind: Some(CompletionItemKind::KEYWORD),
        ..CompletionItem::default()
    }
}

/// Ipê keywords offered at every cursor position.
const KEYWORDS: &[&str] = &[
    "module", "import", "exposing", "type", "alias", "let", "in", "if", "then", "else", "case",
    "of", "as",
];

#[cfg(test)]
mod tests {
    use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile, SourceRoot};

    use super::completions;

    fn file(db: &IpeDatabase, path: &[&str], text: &str) -> SourceFile {
        SourceFile::new(
            db,
            path.iter().map(|s| (*s).to_owned()).collect(),
            text.to_owned(),
            ModuleOrigin::User,
        )
    }

    fn root_of(db: &IpeDatabase, files: &[(&[&str], SourceFile)]) -> SourceRoot {
        SourceRoot::new(
            db,
            files
                .iter()
                .map(|(path, f)| (path.iter().map(|s| (*s).to_owned()).collect(), *f))
                .collect(),
        )
    }

    const HELPER: &str = "module Helper exposing (three, Color(..))\n\nthree : Int\nthree = 3\n\ntype Color = Red | Blue\n";
    const MAIN: &str = "module Main exposing (main)\n\nimport Helper exposing (three, Color(..))\n\nmain : Int\nmain = three\n";

    #[test]
    fn own_module_names_and_imported_names_appear() {
        let db = IpeDatabase::new();
        let helper = file(&db, &["Helper"], HELPER);
        let entry = file(&db, &["Main"], MAIN);
        let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);

        let items = completions(&db, root, entry, &["Main".to_owned()]);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

        assert!(labels.contains(&"main"), "main missing: {labels:?}");
        assert!(labels.contains(&"three"), "three missing: {labels:?}");
        assert!(labels.contains(&"Red"), "Red missing: {labels:?}");
        assert!(labels.contains(&"Blue"), "Blue missing: {labels:?}");
        assert!(labels.contains(&"Color"), "Color missing: {labels:?}");
        assert!(labels.contains(&"let"), "let missing: {labels:?}");
    }

    #[test]
    fn type_annotation_appears_in_detail_when_program_type_checks() {
        let db = IpeDatabase::new();
        let helper = file(&db, &["Helper"], HELPER);
        let entry = file(&db, &["Main"], MAIN);
        let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);

        let items = completions(&db, root, entry, &["Main".to_owned()]);
        let main_item = items
            .iter()
            .find(|i| i.label == "main")
            .expect("main item present");
        // `main : Int` — detail renders as "Int"
        assert_eq!(
            main_item.detail.as_deref(),
            Some("Int"),
            "wrong detail: {:?}",
            main_item.detail
        );
    }

    #[test]
    fn no_duplicates_in_completion_list() {
        let db = IpeDatabase::new();
        let helper = file(&db, &["Helper"], HELPER);
        let entry = file(&db, &["Main"], MAIN);
        let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);

        let items = completions(&db, root, entry, &["Main".to_owned()]);
        let mut seen = std::collections::BTreeSet::new();
        for item in &items {
            assert!(
                seen.insert(item.label.clone()),
                "duplicate label: {}",
                item.label
            );
        }
    }
}
