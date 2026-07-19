#![forbid(unsafe_code)]
//! Hover, document symbols, document links, and folding over in-memory
//! fixtures — no filesystem anywhere.

use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile, SourceRoot};
use ipe_lsp_features::PositionEncoding;

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

const HELPER: &str = "module Helper exposing (three)\n\nthree : Int\nthree = 3\n";
const MAIN: &str = "module Main exposing (main)\n\n\
    import Helper exposing (three)\n\n\
    type Shade\n    = Light\n    | Dark\n\n\
    double : Int -> Int\n\
    double n =\n    n + n\n\n\
    main = double three\n";

#[test]
fn hover_reports_the_solved_type_of_the_innermost_region() {
    let db = IpeDatabase::new();
    let helper = file(&db, &["Helper"], HELPER);
    let entry = file(&db, &["Main"], MAIN);
    let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);

    // `three` inside `main = double three` (the last occurrence).
    let byte = u32::try_from(MAIN.rfind("three").expect("occurrence")).expect("fits");
    let info =
        ipe_lsp_features::hover::hover(&db, root, entry, entry, byte).expect("hover hit");
    assert_eq!(info.ty, "Int");

    // Hover in the dep module works with the dep's own source file.
    let byte = u32::try_from(HELPER.rfind('3').expect("literal")).expect("fits");
    let info =
        ipe_lsp_features::hover::hover(&db, root, entry, helper, byte).expect("hover hit in dep");
    assert_eq!(info.ty, "Int");
}

#[test]
fn document_symbols_cover_values_unions_and_ctors() {
    let db = IpeDatabase::new();
    let helper = file(&db, &["Helper"], HELPER);
    let entry = file(&db, &["Main"], MAIN);
    let _root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);

    let symbols = ipe_lsp_features::symbols::document_symbols(&db, entry, PositionEncoding::Utf16);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["Shade", "double", "main"]);
    let shade = symbols.first().expect("Shade");
    let ctor_names: Vec<&str> = shade
        .children
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(ctor_names, vec!["Light", "Dark"]);
}

#[test]
fn document_links_point_at_resolved_imports_only() {
    let db = IpeDatabase::new();
    let helper = file(&db, &["Helper"], HELPER);
    let entry = file(&db, &["Main"], MAIN);
    let root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);

    let links = ipe_lsp_features::links::document_links(&db, root, entry);
    assert_eq!(links.len(), 1);
    let link = links.first().expect("one link");
    assert_eq!(link.target_module, vec!["Helper".to_owned()]);
    let lo = link.span.lo as usize;
    let hi = link.span.hi as usize;
    assert_eq!(
        MAIN.get(lo..hi),
        Some("Helper"),
        "span covers the import path"
    );

    // The dep imports nothing → no links.
    assert!(ipe_lsp_features::links::document_links(&db, root, helper).is_empty());
}

#[test]
fn folding_covers_multi_line_decls_and_the_union() {
    let db = IpeDatabase::new();
    let helper = file(&db, &["Helper"], HELPER);
    let entry = file(&db, &["Main"], MAIN);
    let _root = root_of(&db, &[(&["Helper"], helper), (&["Main"], entry)]);

    let ranges = ipe_lsp_features::folding::folding_ranges(&db, entry, PositionEncoding::Utf16);
    // `type Shade` spans lines 4-6; `double` spans lines 8-10 (its
    // annotation line is separate). Single-line decls fold nothing.
    assert!(
        ranges.iter().any(|r| r.start_line == 4 && r.end_line == 6),
        "{ranges:?}"
    );
    assert!(
        ranges
            .iter()
            .any(|r| r.end_line > r.start_line && r.start_line >= 8),
        "{ranges:?}"
    );
}
