//! A bare `Done` resolves to `Ipe.Task.Step`'s constructor once that type is
//! imported unqualified, never to the ambient `ChunkEvent.Done`.
//!
//! The ambient constructors are installed before any import, and an import of a
//! type's constructors overrides them. Two imports exposing a `Done`
//! unqualified (`Ipe.Task.Step` and `Ipe.Parser.Step`) are ambiguous and fail
//! closed with IPE-N0024.

use std::collections::BTreeMap;

use ipe_canon::ast::{Def, Expr_, Module};
use ipe_canon::{
    ModuleExports, ModuleOrigin, canonicalise_module, canonicalise_module_with_origin,
};
use ipe_diagnostics::{DResult, Diagnostic, NameError};
use ipe_intern::{Interner, Symbol};

/// The `Step` declaration of `Ipe.Task`, as the stdlib module declares it.
const TASK_STUB: &str = "module Ipe.Task exposing (Step(..))\n\n\
                         type Step s a = Continue s | Done a\n";

/// The `Step` declaration of `Ipe.Parser`, whose `Done` shares the name.
const PARSER_STUB: &str = "module Ipe.Parser exposing (Step(..))\n\n\
                           type Step state a = Loop state | Done a\n";

/// Canonicalise the stdlib `stubs` (as embedded stdlib modules) and then the
/// user module `main` against their exports.
fn canonicalise_main(stubs: &[&str], main: &str) -> (DResult<Module>, Interner) {
    let mut interner = Interner::new();
    let mut deps: BTreeMap<Vec<Symbol>, ModuleExports> = BTreeMap::new();
    for stub in stubs {
        let stub_result = ipe_parse::parse_module(stub, &mut interner).and_then(|parsed| {
            let expected = parsed.name.value.clone();
            canonicalise_module_with_origin(
                &parsed,
                &expected,
                &deps,
                ModuleOrigin::EmbeddedStdlib,
                &mut interner,
            )
        });
        match stub_result {
            Ok((_, exports)) => {
                deps.insert(exports.path.clone(), exports);
            }
            Err(err) => return (Err(err), interner),
        }
    }
    let result = ipe_parse::parse_module(main, &mut interner).and_then(|parsed| {
        let expected = parsed.name.value.clone();
        canonicalise_module(&parsed, &expected, &deps, &mut interner).map(|(module, _)| module)
    });
    (result, interner)
}

/// The dot-joined home of the constructor `main`'s body names, if its body is a
/// bare constructor reference.
fn main_ctor_home(module: &Module, interner: &Interner) -> Option<String> {
    let body = module.defs.iter().find_map(|d| match d {
        Def::Untyped { name, body, .. } | Def::Typed { name, body, .. }
            if interner.resolve(name.value) == Some("main") =>
        {
            Some(body)
        }
        Def::Untyped { .. } | Def::Typed { .. } => None,
    })?;
    let Expr_::VarCtor { home, name, .. } = &body.value else {
        return None;
    };
    if interner.resolve(*name) != Some("Done") {
        return None;
    }
    let segments: Option<Vec<&str>> = home.iter().map(|s| interner.resolve(*s)).collect();
    segments.map(|s| s.join("."))
}

#[track_caller]
fn assert_done_resolves_to_task(main: &str) {
    let (result, interner) = canonicalise_main(&[TASK_STUB], main);
    let Ok(module) = &result else {
        assert!(
            result.is_ok(),
            "the importer must canonicalise, got {result:?}"
        );
        return;
    };
    assert_eq!(
        main_ctor_home(module, &interner).as_deref(),
        Some("Ipe.Task"),
        "`Done` must be `Ipe.Task.Step`'s constructor, not the ambient `ChunkEvent.Done`"
    );
}

#[test]
fn an_explicit_step_import_resolves_bare_done_to_ipe_task() {
    assert_done_resolves_to_task(
        "module Main exposing (main)\n\n\
         import Ipe.Task exposing (Step(..))\n\n\
         main = Done\n",
    );
}

#[test]
fn an_open_task_import_resolves_bare_done_to_ipe_task() {
    assert_done_resolves_to_task(
        "module Main exposing (main)\n\n\
         import Ipe.Task exposing (..)\n\n\
         main = Done\n",
    );
}

#[test]
fn a_qualified_task_done_resolves_to_ipe_task() {
    assert_done_resolves_to_task(
        "module Main exposing (main)\n\n\
         import Ipe.Task as Task\n\n\
         main = Task.Done\n",
    );
}

#[test]
fn two_unqualified_step_imports_are_ambiguous() {
    let (result, _) = canonicalise_main(
        &[TASK_STUB, PARSER_STUB],
        "module Main exposing (main)\n\n\
         import Ipe.Task exposing (Step(..))\n\
         import Ipe.Parser exposing (Step(..))\n\n\
         main = Done\n",
    );
    assert!(
        matches!(
            &result,
            Err(Diagnostic::Name {
                msg: NameError::AmbiguousImport { .. },
                ..
            })
        ),
        "two unqualified `Done` constructors must fail closed with IPE-N0024, got {result:?}"
    );
}
