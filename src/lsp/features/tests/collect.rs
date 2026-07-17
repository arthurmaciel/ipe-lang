#![forbid(unsafe_code)]
//! Diagnostics collection over in-memory fixtures — no filesystem anywhere
//! (the same structural proof as `ipe_db`'s own `lsp_seam.rs`).

use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile, SourceRoot};
use ipe_lsp_features::PositionEncoding;
use ipe_lsp_features::diagnostics::{ModuleDiagnostics, collect, to_lsp};

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

fn diags_for<'a>(all: &'a [ModuleDiagnostics], module: &[&str]) -> &'a ModuleDiagnostics {
    #[allow(clippy::expect_used)] // test helper: a missing module entry is the failure
    all.iter()
        .find(|m| {
            m.module
                .iter()
                .map(String::as_str)
                .eq(module.iter().copied())
        })
        .expect("module entry present")
}

const DEP_A: &str = "module A exposing (visible)\n\nvisible = 1\n";
const ENTRY_OK: &str =
    "module Main exposing (main)\n\nimport A exposing (visible)\n\nmain = visible\n";
const ENTRY_TYPE_ERROR: &str = "module Main exposing (main)\n\nimport A exposing (visible)\n\n\
    main : Int\n\
    main = \"not an int\"\n";
const ENTRY_PARSE_ERROR: &str = "module Main exposing (main)\n\nmain = = 1\n";

#[test]
fn clean_project_yields_empty_lists_for_every_module() {
    let db = IpeDatabase::new();
    let a = file(&db, &["A"], DEP_A);
    let entry = file(&db, &["Main"], ENTRY_OK);
    let root = root_of(&db, &[(&["A"], a), (&["Main"], entry)]);

    let all = collect(&db, root, entry);
    assert_eq!(all.len(), 2, "every module gets an entry");
    assert!(diags_for(&all, &["A"]).diagnostics.is_empty());
    assert!(diags_for(&all, &["Main"]).diagnostics.is_empty());
}

#[test]
fn type_error_is_attributed_to_the_entry_module() {
    let db = IpeDatabase::new();
    let a = file(&db, &["A"], DEP_A);
    let entry = file(&db, &["Main"], ENTRY_TYPE_ERROR);
    let root = root_of(&db, &[(&["A"], a), (&["Main"], entry)]);

    let all = collect(&db, root, entry);
    assert!(diags_for(&all, &["A"]).diagnostics.is_empty());
    let main = diags_for(&all, &["Main"]);
    assert_eq!(main.diagnostics.len(), 1);
    let diag = main.diagnostics.first().expect("one diagnostic");
    assert_eq!(diag.code().as_str(), "IPE-T0001");

    // The LSP mapping points at the failing expression, not 0:0.
    let lsp = to_lsp(diag, ENTRY_TYPE_ERROR, PositionEncoding::Utf16);
    assert_eq!(
        lsp.code,
        Some(lsp_types::NumberOrString::String("IPE-T0001".to_owned()))
    );
    assert_eq!(lsp.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
    assert!(
        lsp.range.start.line > 0,
        "range must be positioned, got {:?}",
        lsp.range
    );
    assert!(lsp.message.contains("type mismatch"), "{}", lsp.message);
}

#[test]
fn dep_parse_error_is_blamed_on_the_dep_not_the_importer() {
    let db = IpeDatabase::new();
    let a = file(
        &db,
        &["A"],
        "module A exposing (visible)\n\nvisible = = 1\n",
    );
    let entry = file(&db, &["Main"], ENTRY_OK);
    let root = root_of(&db, &[(&["A"], a), (&["Main"], entry)]);

    let all = collect(&db, root, entry);
    assert_eq!(diags_for(&all, &["A"]).diagnostics.len(), 1);
    assert!(
        diags_for(&all, &["Main"]).diagnostics.is_empty(),
        "the importer must not replay its dep's diagnostic"
    );
}

#[test]
fn edit_converges_error_then_clean() {
    let mut db = IpeDatabase::new();
    let a = file(&db, &["A"], DEP_A);
    let entry = file(&db, &["Main"], ENTRY_PARSE_ERROR);
    let root = root_of(&db, &[(&["A"], a), (&["Main"], entry)]);

    let all = collect(&db, root, entry);
    assert_eq!(diags_for(&all, &["Main"]).diagnostics.len(), 1);

    assert!(ipe_db::set_text_if_changed(&mut db, entry, ENTRY_OK));
    let all = collect(&db, root, entry);
    assert!(diags_for(&all, &["Main"]).diagnostics.is_empty());
    assert!(diags_for(&all, &["A"]).diagnostics.is_empty());
}
