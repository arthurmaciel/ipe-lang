//! An exported type alias resolves in its defining module's scope.
//!
//! Each importer receives the alias body already canonical, so it never needs
//! (and can never capture) the definer's private imports, while a name the
//! importer itself writes is still resolved against the importer's own imports.

use std::collections::BTreeMap;

use ipe_canon::ast::{Def, Module, Type};
use ipe_canon::{ModuleExports, canonicalise_module};
use ipe_diagnostics::{DResult, Diagnostic, NameError};
use ipe_intern::{Interner, Symbol};

const HIGHLIGHT: &str = "module Lib.Highlight exposing (..)\n\n\
                         type Lang = Ipe | Rust\n\n\
                         type alias Code = String\n";

const INDEX: &str = "module Lib.Index exposing (..)\n\n\
                     import Lib.Highlight exposing (Code)\n\n\
                     type alias Unit = { name : String, lang : Highlight.Lang, code : Code }\n\n\
                     type alias Tagged a = { value : a, lang : Highlight.Lang }\n\n\
                     defaultLang : Highlight.Lang\n\
                     defaultLang = Highlight.Ipe\n";

/// Canonicalise `sources` in order, each against the exports of every module
/// before it; returns the last module and the interner its symbols live in.
fn canonicalise_chain(sources: &[&str]) -> (DResult<(Module, ModuleExports)>, Interner) {
    let mut interner = Interner::new();
    let mut deps: BTreeMap<Vec<Symbol>, ModuleExports> = BTreeMap::new();
    let mut last = None;
    for src in sources {
        let result = ipe_parse::parse_module(src, &mut interner).and_then(|parsed| {
            let expected = parsed.name.value.clone();
            canonicalise_module(&parsed, &expected, &deps, &mut interner)
        });
        if let Ok((_, exports)) = &result {
            deps.insert(exports.path.clone(), exports.clone());
        }
        last = Some(result);
        if last.as_ref().is_some_and(Result::is_err) {
            break;
        }
    }
    let result = last.unwrap_or_else(|| {
        Err(Diagnostic::CompilerBug {
            where_: "alias_defining_scope::canonicalise_chain",
            detail: "no module source given".into(),
        })
    });
    (result, interner)
}

/// The canonical annotation of the top-level binding `name`.
fn annotation<'m>(module: &'m Module, interner: &Interner, name: &str) -> Option<&'m Type> {
    module.defs.iter().find_map(|d| match d {
        Def::Typed { name: n, ty, .. } if interner.resolve(n.value) == Some(name) => Some(ty),
        _ => None,
    })
}

/// The type of record field `field` in `ty` (a record, or the argument of a
/// function whose parameter is one).
fn field<'t>(ty: &'t Type, interner: &Interner, field: &str) -> Option<&'t Type> {
    let record = match ty {
        Type::Lambda(arg, _) => arg.as_ref(),
        other => other,
    };
    let Type::Record(fields) = record else {
        return None;
    };
    fields
        .iter()
        .find(|(n, _)| interner.resolve(*n) == Some(field))
        .map(|(_, t)| t)
}

/// The dot-joined home and name of a constructor type, e.g. `Lib.Highlight.Lang`.
fn con_name(ty: &Type, interner: &Interner) -> Option<String> {
    let Type::Con { home, name, .. } = ty else {
        return None;
    };
    let mut parts: Vec<&str> = home.iter().filter_map(|s| interner.resolve(*s)).collect();
    parts.push(interner.resolve(*name)?);
    Some(parts.join("."))
}

#[test]
fn an_imported_alias_naming_a_module_only_its_definer_imports_compiles() {
    let (result, i) = canonicalise_chain(&[
        HIGHLIGHT,
        INDEX,
        "module Lib.Queue exposing (..)\n\n\
         import Lib.Index exposing (Unit)\n\n\
         label : Unit -> String\n\
         label u = u.name\n",
    ]);
    let Ok((queue, _)) = result else {
        assert!(result.is_ok(), "Lib.Queue must canonicalise: {result:?}");
        return;
    };
    let lang = annotation(&queue, &i, "label").and_then(|t| field(t, &i, "lang"));
    assert_eq!(
        lang.and_then(|t| con_name(t, &i)).as_deref(),
        Some("Lib.Highlight.Lang"),
        "the alias field resolves to the definer's `Highlight.Lang`"
    );
}

#[test]
fn a_qualified_imported_alias_compiles_without_the_definers_imports() {
    let (result, _) = canonicalise_chain(&[
        HIGHLIGHT,
        INDEX,
        "module Lib.Queue exposing (..)\n\n\
         import Lib.Index as Index\n\n\
         label : Index.Unit -> String\n\
         label u = u.name\n",
    ]);
    assert!(
        result.is_ok(),
        "qualified `Index.Unit` must canonicalise: {result:?}"
    );
}

#[test]
fn naming_the_definers_private_import_directly_is_still_an_unknown_module() {
    let (result, _) = canonicalise_chain(&[
        HIGHLIGHT,
        INDEX,
        "module Lib.Queue exposing (..)\n\n\
         import Lib.Index exposing (Unit)\n\n\
         langOf : Unit -> Highlight.Lang\n\
         langOf u = u.lang\n",
    ]);
    assert!(
        matches!(
            &result,
            Err(Diagnostic::Name {
                msg: NameError::UnknownModule { qualifier, .. },
                ..
            }) if &**qualifier == "Highlight"
        ),
        "a module the importer never imported stays IPE-N0004, got {result:?}"
    );
}

#[test]
fn an_importers_same_named_alias_never_captures_the_definers_body() {
    // `Code` is `String` where `Unit` is declared; the importer's own `Code`
    // (an `Int`) must not leak into `Unit`'s expansion.
    let (result, i) = canonicalise_chain(&[
        HIGHLIGHT,
        INDEX,
        "module Lib.Queue exposing (..)\n\n\
         import Lib.Index exposing (Unit)\n\n\
         type alias Code = Int\n\n\
         codeOf : Unit -> String\n\
         codeOf u = u.code\n",
    ]);
    let Ok((queue, _)) = result else {
        assert!(result.is_ok(), "Lib.Queue must canonicalise: {result:?}");
        return;
    };
    let code = annotation(&queue, &i, "codeOf").and_then(|t| field(t, &i, "code"));
    assert_eq!(
        code.and_then(|t| con_name(t, &i)).as_deref(),
        Some("String"),
        "`Unit.code` keeps the definer's `Code = String`"
    );
}

#[test]
fn an_alias_of_an_imported_parametric_alias_resolves_across_three_modules() {
    let (result, i) = canonicalise_chain(&[
        HIGHLIGHT,
        INDEX,
        "module Lib.Mid exposing (..)\n\n\
         import Lib.Index exposing (Tagged)\n\n\
         type alias Named = Tagged String\n",
        "module Lib.Top exposing (..)\n\n\
         import Lib.Mid exposing (Named)\n\n\
         valueOf : Named -> String\n\
         valueOf n = n.value\n",
    ]);
    let Ok((top, _)) = result else {
        assert!(result.is_ok(), "Lib.Top must canonicalise: {result:?}");
        return;
    };
    let ann = annotation(&top, &i, "valueOf");
    assert_eq!(
        ann.and_then(|t| field(t, &i, "value"))
            .and_then(|t| con_name(t, &i))
            .as_deref(),
        Some("String"),
        "the parameter slot is substituted with the argument"
    );
    assert_eq!(
        ann.and_then(|t| field(t, &i, "lang"))
            .and_then(|t| con_name(t, &i))
            .as_deref(),
        Some("Lib.Highlight.Lang"),
        "the transitive field keeps its defining module's home"
    );
}

#[test]
fn an_imported_record_alias_constructor_needs_no_definer_imports() {
    let (result, _) = canonicalise_chain(&[
        HIGHLIGHT,
        INDEX,
        "module Lib.Queue exposing (..)\n\n\
         import Lib.Index exposing (Unit, defaultLang)\n\n\
         fresh : Unit\n\
         fresh = Unit \"main\" defaultLang \"\"\n",
    ]);
    assert!(
        result.is_ok(),
        "the record-alias constructor must canonicalise: {result:?}"
    );
}

#[test]
fn an_alias_body_naming_a_module_its_definer_never_imports_is_refused_at_the_declaration() {
    let (result, _) = canonicalise_chain(&[
        HIGHLIGHT,
        "module Lib.Index exposing (..)\n\n\
         type alias Unit = { lang : Highlight.Lang }\n",
    ]);
    assert!(
        matches!(
            &result,
            Err(Diagnostic::Name {
                msg: NameError::UnknownModule { qualifier, .. },
                ..
            }) if &**qualifier == "Highlight"
        ),
        "the definer's own unresolvable alias body is IPE-N0004, got {result:?}"
    );
}
