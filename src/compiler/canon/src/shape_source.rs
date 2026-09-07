//! Classify a program's rendering shape from the head of `main`, over the raw
//! parse tree — the single compile-time source of truth the delivery grammar
//! cross-checks against (spec § 0, § 1).
//!
//! A program's shape is pinned by what `main` head-calls, never by config: a
//! `main = Web.app …` is a DOM app, `main = Tui.app …` a terminal-cells app,
//! `main = Cli.app …` a terminal-lines app, `main = Server.listen …` a server,
//! and any other `main` (a plain `Task`) a script. This peels the same
//! head-forms the resolver's shape gate peels — application, lambda, and `let` —
//! so the CLI cross-check and the compiler agree on one classification.

use ipe_intern::Interner;
use ipe_syntax::{Exposed, Exposing, Expr, Expr_, Import, Module};

/// The rendering shape a `main` pins, as read from the parse tree.
///
/// Mirrors the delivery grammar's shape axis: the four non-web shapes plus the
/// DOM `Web` shape (the only one with a delivery runtime choice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainShape {
    /// A plain `main : Task Error ()` — renders nothing.
    Script,
    /// `main = Tui.app …` — terminal cells.
    Tui,
    /// `main = Cli.app …` — terminal lines.
    Cli,
    /// `main = Server.listen …` — an HTTP server.
    Server,
    /// `main = Web.app …` / `appRouted` / `appWith` — the DOM shape. A webview is
    /// a delivery *host* of this shape (`web desktop`), not a distinct shape
    /// (spec § 1).
    Web,
}

/// A `main` head that head-calls one of these `(qualifier, name)` pairs pins the
/// paired shape. The head may be written qualified (`Web.app`) or brought into
/// scope unqualified by an `exposing` import of the same shape entry; either way
/// it resolves to one of these pairs. Kept in lockstep with the resolver's
/// `TEA_APP_ENTRIES` and `Server.listen` entry.
const SHAPE_ENTRIES: &[(&str, &str, MainShape)] = &[
    ("Web", "app", MainShape::Web),
    ("Web", "appRouted", MainShape::Web),
    ("Web", "appWith", MainShape::Web),
    ("Tui", "app", MainShape::Tui),
    ("Cli", "app", MainShape::Cli),
    ("Server", "listen", MainShape::Server),
];

/// Classify a parsed module's `main` into its pinned [`MainShape`].
///
/// Returns [`MainShape::Script`] for a module that defines no `main`, or a
/// `main` whose head is not one of the shape-entry kernels — a plain `Task`
/// program renders nothing. A shape-entry head pins the paired shape.
///
/// The head is found by peeling the same forms the resolver's shape gate peels:
/// `entry cfg` (the callee is the head), `\arg -> entry cfg` (the lambda body),
/// and `let … in entry cfg` (the `in` body). A qualified head (`Web.app`) and a
/// bare head brought into scope by `import Ipe.Tea.Web exposing (app)` classify
/// identically. Any other head is a script.
#[must_use]
pub fn classify_main_shape(module: &Module, interner: &Interner) -> MainShape {
    let Some(main_sym) = interner.lookup("main") else {
        return MainShape::Script; // `main` never interned → no entry here.
    };
    let Some(value) = module
        .values
        .iter()
        .find(|v| v.value.name.value == main_sym)
    else {
        return MainShape::Script; // helper module with no `main`.
    };
    // A `main x = …` binding with argument patterns is desugared to a lambda
    // head; classify its body head the same way as a `main = \x -> …`.
    head_shape(&value.value.body, module, interner).unwrap_or(MainShape::Script)
}

/// Peel a `main` body to its head reference and match it against the shape
/// entries. `None` when the head is not a shape-entry reference.
///
/// A head is a shape entry either qualified — `Web.app`, `Server.listen` — or
/// unqualified through an `import Ipe.Tea.Web exposing (app)` that brings the
/// entry into scope under its bare name. An unqualified head is resolved to the
/// same `(shape-qualifier, name)` its exposing import pins, so `app` from
/// `Ipe.Tea.Web` classifies identically to a written-out `Web.app`.
fn head_shape(body: &Expr, module: &Module, interner: &Interner) -> Option<MainShape> {
    let mut node = body;
    loop {
        match &node.value {
            // `entry cfg` — the callee is the head.
            Expr_::Call(callee, _) => node = callee,
            // `\arg -> entry cfg` (lambda body) and `let … in entry cfg` (the
            // `in` body) both peel to the inner expression.
            Expr_::Lambda(_, inner) | Expr_::Let(_, inner) => node = inner,
            // `Web.app` / `Tui.app` / … at the head pins its shape.
            Expr_::VarQual(qual, name) => {
                let (q, n) = (interner.resolve(*qual)?, interner.resolve(*name)?);
                return shape_for(q, n);
            }
            // A bare `app` head: pinned iff exactly one exposing import brings a
            // shape entry of that name into scope (fail-closed on none/ambiguity).
            Expr_::VarLocal(name) => {
                let n = interner.resolve(*name)?;
                return shape_for_exposed(n, module, interner);
            }
            _ => return None,
        }
    }
}

/// The shape an unqualified head name pins by being exposed from a shape module.
///
/// The name is resolved exactly as name resolution would: each `import M
/// exposing (name)` binds `name` to `M`'s member, whose qualified spelling is
/// `<leaf(M)>.name`. Matching that against the shape entries pins the shape.
/// Fail-closed: a name no exposing import brings into scope, or one that two
/// imports expose to different shapes, stays a script (`None`).
fn shape_for_exposed(name: &str, module: &Module, interner: &Interner) -> Option<MainShape> {
    let mut pinned: Option<MainShape> = None;
    for import in &module.imports {
        if !import_exposes_value(import, name, interner) {
            continue;
        }
        let Some(leaf_sym) = import.name.value.last() else {
            continue;
        };
        let Some(leaf) = interner.resolve(*leaf_sym) else {
            continue;
        };
        if let Some(shape) = shape_for(leaf, name) {
            match pinned {
                // A second, differently-shaped exposing import is ambiguous —
                // fail closed rather than guess a shape.
                Some(prior) if prior != shape => return None,
                _ => pinned = Some(shape),
            }
        }
    }
    pinned
}

/// Does this `import M exposing (name)` bring the value `name` into unqualified
/// scope? Only an explicit `exposing (…, name, …)` list counts: an open
/// `exposing (..)` on a stdlib module is a resolver no-op, so it binds no bare
/// name here either.
fn import_exposes_value(import: &Import, name: &str, interner: &Interner) -> bool {
    let Exposing::List(items) = &import.exposing.value else {
        return false;
    };
    items.iter().any(|item| {
        matches!(&item.value, Exposed::Value(sym)
            if interner.resolve(*sym) == Some(name))
    })
}

/// The shape a `(qualifier, name)` head reference pins, or `None` when the pair
/// is not a shape-entry kernel.
fn shape_for(qualifier: &str, name: &str) -> Option<MainShape> {
    SHAPE_ENTRIES
        .iter()
        .find(|(q, n, _)| *q == qualifier && *n == name)
        .map(|(_, _, shape)| *shape)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(src: &str) -> MainShape {
        let mut interner = Interner::new();
        let module = ipe_parse::parse_module(src, &mut interner).expect("parse");
        classify_main_shape(&module, &interner)
    }

    #[test]
    fn plain_task_main_is_script() {
        assert_eq!(
            classify("module Main exposing (..)\n\nmain = Io.println \"hi\"\n"),
            MainShape::Script
        );
    }

    #[test]
    fn no_main_is_script() {
        assert_eq!(
            classify("module Helper exposing (..)\n\nhelper = 1\n"),
            MainShape::Script
        );
    }

    #[test]
    fn web_app_head_is_web() {
        assert_eq!(
            classify("module Main exposing (..)\n\nmain = Web.app cfg\n"),
            MainShape::Web
        );
    }

    #[test]
    fn tui_and_cli_heads() {
        assert_eq!(
            classify("module Main exposing (..)\n\nmain = Tui.app cfg\n"),
            MainShape::Tui
        );
        assert_eq!(
            classify("module Main exposing (..)\n\nmain = Cli.app cfg\n"),
            MainShape::Cli
        );
    }

    #[test]
    fn server_listen_head_is_server() {
        assert_eq!(
            classify("module Main exposing (..)\n\nmain = Server.listen cfg\n"),
            MainShape::Server
        );
    }

    #[test]
    fn let_bound_config_still_classifies() {
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nmain =\n    let cfg = { init = () }\n    in Web.app cfg\n"
            ),
            MainShape::Web
        );
    }

    #[test]
    fn app_with_head_classifies_web() {
        assert_eq!(
            classify("module Main exposing (..)\n\nmain = Web.appWith cfg\n"),
            MainShape::Web
        );
    }

    #[test]
    fn exposed_bare_app_head_classifies_web() {
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.Tea.Web exposing (app)\n\nmain = app cfg\n"
            ),
            MainShape::Web
        );
    }

    #[test]
    fn exposed_bare_app_head_from_tui_classifies_tui() {
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.Tea.Tui exposing (app)\n\nmain = app cfg\n"
            ),
            MainShape::Tui
        );
    }

    #[test]
    fn exposed_bare_app_through_let_classifies_web() {
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.Tea.Web exposing (app)\n\nmain =\n    let cfg = { init = () }\n    in app cfg\n"
            ),
            MainShape::Web
        );
    }

    #[test]
    fn bare_app_with_no_shape_import_stays_script() {
        // Fail-closed: nothing brings `app` into scope from a shape module.
        assert_eq!(
            classify("module Main exposing (..)\n\nmain = app cfg\n"),
            MainShape::Script
        );
    }

    #[test]
    fn bare_app_from_non_shape_import_stays_script() {
        // An `app` exposed by a non-shape module is not a shape entry.
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.Widget exposing (app)\n\nmain = app cfg\n"
            ),
            MainShape::Script
        );
    }

    #[test]
    fn open_exposing_import_does_not_pin_bare_head() {
        // `exposing (..)` on a stdlib module is a resolver no-op, so it binds no
        // bare name here either — the head stays a script.
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.Tea.Web exposing (..)\n\nmain = app cfg\n"
            ),
            MainShape::Script
        );
    }
}
