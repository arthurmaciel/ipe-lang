#![forbid(unsafe_code)]
//! Diagnostics collection over in-memory fixtures — no filesystem anywhere
//! (the same structural proof as `ipe_db`'s own `lsp_seam.rs`).

use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile, SourceRoot};
use ipe_lsp_features::PositionEncoding;
use ipe_lsp_features::code_actions::{DbView, code_actions};
use ipe_lsp_features::diagnostics::{ModuleDiagnostics, collect, to_lsp};
use lsp_types::{CodeActionOrCommand, Range, TextEdit, Url};

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
// `ENTRY_OK` uses `Ipe.System` — a kernel-qualifier module that resolves
// without needing to be in the SourceRoot, so the clean-project test can
// assert exactly 2 module entries (one per user file).
const ENTRY_OK: &str = "module Main exposing (main)\n\nimport A exposing (visible)\nimport Ipe.System as System\n\nmain : Task Error ()\nmain =\n    System.setenv \"KEY\" \"x\"\n";
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

/// Convert an LSP position back to a byte offset (UTF-16 encoding) and apply a
/// single-hunk `TextEdit`, returning the new document text.
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)] // test helper: len_utf16 is 1 or 2
fn apply_edit(text: &str, edit: &TextEdit) -> String {
    let to_byte = |pos: lsp_types::Position| -> usize {
        let mut line_start = 0usize;
        for (i, l) in text.split('\n').enumerate() {
            if i == pos.line as usize {
                // character is a UTF-16 offset within the line.
                let mut utf16 = 0u32;
                for (b, ch) in l.char_indices() {
                    if utf16 >= pos.character {
                        return line_start + b;
                    }
                    utf16 += ch.len_utf16() as u32;
                }
                return line_start + l.len();
            }
            line_start += l.len() + 1;
        }
        text.len()
    };
    let start = to_byte(edit.range.start);
    let end = to_byte(edit.range.end);
    let mut out = String::with_capacity(text.len() + edit.new_text.len());
    out.push_str(&text[..start]);
    out.push_str(&edit.new_text);
    out.push_str(&text[end..]);
    out
}

/// IPE-N0034 quick-fix: a kernel-qualifier stdlib module used without its
/// import yields an "Add import `Ipe.X`" code action whose edit, once applied,
/// clears the diagnostic (SEAL — the program compiles).
#[test]
fn add_import_quick_fix_inserts_the_missing_import_and_clears_the_diagnostic() {
    // `Crypto.sha256` names the `Ipe.Crypto` kernel-qualifier module without
    // an import declaration. `Ipe.Crypto` is a kernel qualifier (present in
    // `STDLIB_MODULE_QUALIFIERS`), so the missing-import gate fires IPE-N0034.
    // Kernel qualifiers resolve without needing to be in the SourceRoot, so
    // the minimal root (only Main) is sufficient both before and after the fix.
    //
    // `System.setenv` wraps `Crypto.sha256 "hello"` to give `main` the
    // required `Task Error ()` type. `Ipe.System` is also a kernel qualifier,
    // so it too resolves without being in the SourceRoot.
    let src = "module Main exposing (main)\n\nimport Ipe.System as System\n\nmain =\n    System.setenv \"KEY\" (Crypto.sha256 \"hello\")\n";
    let mut db = IpeDatabase::new();
    let entry = file(&db, &["Main"], src);
    let root = root_of(&db, &[(&["Main"], entry)]);

    // The real compiler diagnostic must be IPE-N0034.
    let all = collect(&db, root, entry);
    let main = diags_for(&all, &["Main"]);
    assert_eq!(main.diagnostics.len(), 1, "one diagnostic on Main");
    let diag = main.diagnostics.first().expect("one diagnostic");
    assert_eq!(diag.code().as_str(), "IPE-N0034");
    let lsp_diag = to_lsp(diag, src, PositionEncoding::Utf16);

    // The code action offers the correctly-named insert.
    let uri = Url::from_file_path("/fake/Main.ipe").expect("uri");
    let full_range = Range {
        start: lsp_diag.range.start,
        end: lsp_diag.range.end,
    };
    let actions = code_actions(
        DbView {
            db: &db,
            root,
            entry,
        },
        &["Main".to_owned()],
        &uri,
        full_range,
        std::slice::from_ref(&lsp_diag),
        src,
        PositionEncoding::Utf16,
    );
    let action = actions
        .into_iter()
        .find_map(|a| match a {
            CodeActionOrCommand::CodeAction(action) => Some(action),
            CodeActionOrCommand::Command(_) => None,
        })
        .expect("one CodeAction for IPE-N0034");
    assert_eq!(action.title, "Add import Ipe.Crypto");
    let edit = action
        .edit
        .as_ref()
        .and_then(|e| e.changes.as_ref())
        .and_then(|c| c.values().next())
        .and_then(|v| v.first())
        .expect("edit present");
    assert!(
        edit.new_text.contains("import Ipe.Crypto"),
        "inserts the named import, got {:?}",
        edit.new_text
    );

    // SEAL: applying the edit makes the program compile — N0034 is gone.
    let fixed = apply_edit(src, edit);
    assert!(
        fixed.contains("\nimport Ipe.Crypto\n"),
        "the fixed source carries the import: {fixed:?}"
    );
    assert!(ipe_db::set_text_if_changed(&mut db, entry, &fixed));
    let all = collect(&db, root, entry);
    assert!(
        diags_for(&all, &["Main"]).diagnostics.is_empty(),
        "after applying the quick-fix the module is clean, got {:?}",
        diags_for(&all, &["Main"]).diagnostics
    );
}

/// The insert is sorted among existing imports, not just appended.
#[test]
fn add_import_quick_fix_sorts_among_existing_imports() {
    // `Ipe.Crypto` should land between `Ipe.App` (A < C) and `Ipe.System`
    // (S > C). All three are kernel qualifiers (present in
    // `STDLIB_MODULE_QUALIFIERS`), so they resolve without needing to be in
    // the SourceRoot. `Crypto` is missing → IPE-N0034 fires; the quick-fix
    // must insert `import Ipe.Crypto` in sorted order.
    //
    // The body uses `System.setenv` (String -> String -> Task Error ()) with
    // `Crypto.sha256 "hello"` (String) as its second argument, so after the
    // fix the module type-checks clean.
    let src = "module Main exposing (main)\n\n\
        import Ipe.App as App\n\
        import Ipe.System as System\n\n\
        main = System.setenv \"KEY\" (Crypto.sha256 \"hello\")\n";
    let mut db = IpeDatabase::new();
    let entry = file(&db, &["Main"], src);
    let root = root_of(&db, &[(&["Main"], entry)]);

    let all = collect(&db, root, entry);
    let main = diags_for(&all, &["Main"]);
    let diag = main
        .diagnostics
        .iter()
        .find(|d| d.code().as_str() == "IPE-N0034")
        .expect("an IPE-N0034 diagnostic");
    let lsp_diag = to_lsp(diag, src, PositionEncoding::Utf16);

    let uri = Url::from_file_path("/fake/Main.ipe").expect("uri");
    let range = Range {
        start: lsp_diag.range.start,
        end: lsp_diag.range.end,
    };
    let actions = code_actions(
        DbView {
            db: &db,
            root,
            entry,
        },
        &["Main".to_owned()],
        &uri,
        range,
        std::slice::from_ref(&lsp_diag),
        src,
        PositionEncoding::Utf16,
    );
    let action = actions
        .into_iter()
        .find_map(|a| match a {
            CodeActionOrCommand::CodeAction(action) => Some(action),
            CodeActionOrCommand::Command(_) => None,
        })
        .expect("one CodeAction for IPE-N0034");
    let edit = action
        .edit
        .as_ref()
        .and_then(|e| e.changes.as_ref())
        .and_then(|c| c.values().next())
        .and_then(|v| v.first())
        .expect("edit present");

    let fixed = apply_edit(src, edit);
    let app_at = fixed.find("import Ipe.App").expect("App present");
    let crypto_at = fixed.find("import Ipe.Crypto").expect("Crypto inserted");
    let system_at = fixed.find("import Ipe.System").expect("System present");
    assert!(
        app_at < crypto_at && crypto_at < system_at,
        "Crypto sorts between App and System: {fixed:?}"
    );

    // SEAL: the sorted insert also compiles.
    assert!(ipe_db::set_text_if_changed(&mut db, entry, &fixed));
    let all = collect(&db, root, entry);
    assert!(
        diags_for(&all, &["Main"]).diagnostics.is_empty(),
        "sorted insert compiles, got {:?}",
        diags_for(&all, &["Main"]).diagnostics
    );
}

/// IPE-N0035 quick-fix: a Terminal app importing the Web shape's `Cmd` yields a
/// "Change import to `Ipe.Tea.Terminal.Cmd`" code action whose edit, once
/// applied, repoints the import to the app's shape and clears the diagnostic
/// (SEAL — the program compiles), leaving the `as Cmd` binding untouched.
#[test]
fn wrong_shape_cmd_quick_fix_repoints_the_import_and_clears_the_diagnostic() {
    let src = "module Main exposing (main)\n\n\
        import Ipe.Tea.Terminal as Terminal\n\
        import Ipe.Tea.Web.Cmd as Cmd\n\
        import Ipe.Tea.Terminal.Sub as Sub\n\n\
        init _u = ( { n = 0 }, Cmd.none )\n\
        update _m model = ( model, Cmd.none )\n\
        view _m = \"ok\"\n\
        subscriptions _m = Sub.none\n\
        onLine l = l\n\
        main =\n    \
            Terminal.appLines { init = init, update = update, view = view, subscriptions = subscriptions, onLine = onLine }\n";
    let mut db = IpeDatabase::new();
    let entry = file(&db, &["Main"], src);
    let root = root_of(&db, &[(&["Main"], entry)]);

    // The real compiler diagnostic must be IPE-N0035.
    let all = collect(&db, root, entry);
    let main = diags_for(&all, &["Main"]);
    let diag = main
        .diagnostics
        .iter()
        .find(|d| d.code().as_str() == "IPE-N0035")
        .expect("an IPE-N0035 diagnostic");
    let lsp_diag = to_lsp(diag, src, PositionEncoding::Utf16);

    let uri = Url::from_file_path("/fake/Main.ipe").expect("uri");
    let range = Range {
        start: lsp_diag.range.start,
        end: lsp_diag.range.end,
    };
    let actions = code_actions(
        DbView {
            db: &db,
            root,
            entry,
        },
        &["Main".to_owned()],
        &uri,
        range,
        std::slice::from_ref(&lsp_diag),
        src,
        PositionEncoding::Utf16,
    );
    let action = actions
        .into_iter()
        .find_map(|a| match a {
            CodeActionOrCommand::CodeAction(action) => Some(action),
            CodeActionOrCommand::Command(_) => None,
        })
        .expect("one CodeAction for IPE-N0035");
    assert_eq!(action.title, "Change import to Ipe.Tea.Terminal.Cmd");
    let edit = action
        .edit
        .as_ref()
        .and_then(|e| e.changes.as_ref())
        .and_then(|c| c.values().next())
        .and_then(|v| v.first())
        .expect("edit present");

    // SEAL: applying the edit repoints the import (keeping `as Cmd`) and the
    // module compiles — N0035 is gone.
    let fixed = apply_edit(src, edit);
    assert!(
        fixed.contains("import Ipe.Tea.Terminal.Cmd as Cmd"),
        "the fixed source repoints to the Terminal shape, keeping the alias: {fixed:?}"
    );
    assert!(
        !fixed.contains("Ipe.Tea.Web.Cmd"),
        "the wrong-shape import is gone: {fixed:?}"
    );
    assert!(ipe_db::set_text_if_changed(&mut db, entry, &fixed));
    let all = collect(&db, root, entry);
    assert!(
        !diags_for(&all, &["Main"])
            .diagnostics
            .iter()
            .any(|d| d.code().as_str() == "IPE-N0035"),
        "after applying the quick-fix the wrong-shape diagnostic is gone, got {:?}",
        diags_for(&all, &["Main"]).diagnostics
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
