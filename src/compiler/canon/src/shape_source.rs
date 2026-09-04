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
use ipe_syntax::{Expr, Expr_, Module};

/// The rendering shape a `main` pins, as read from the parse tree. Mirrors the
/// delivery grammar's shape axis: the four non-web shapes plus the DOM `Web`
/// shape (the only one with a delivery runtime choice).
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
    /// `main = Web.app …` / `appRouted` / `appWith` / `WebView.app …` — the DOM
    /// shape.
    Web,
}

/// A `main` head that head-calls one of these `(qualifier, name)` pairs pins the
/// paired shape. `WebView.app` folds onto [`MainShape::Web`] — a webview is a
/// Web *host*, not a distinct shape (spec § 1). Kept in lockstep with the
/// resolver's `TEA_APP_ENTRIES` and `Server.listen` entry.
const SHAPE_ENTRIES: &[(&str, &str, MainShape)] = &[
    ("Web", "app", MainShape::Web),
    ("Web", "appRouted", MainShape::Web),
    ("Web", "appWith", MainShape::Web),
    ("WebView", "app", MainShape::Web),
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
/// and `let … in entry cfg` (the `in` body). Any other head is a script.
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
    head_shape(&value.value.body, interner).unwrap_or(MainShape::Script)
}

/// Peel a `main` body to its head reference and match it against the shape
/// entries. `None` when the head is not a qualified shape-entry reference.
fn head_shape(body: &Expr, interner: &Interner) -> Option<MainShape> {
    let mut node = body;
    loop {
        match &node.value {
            // `entry cfg` — the callee is the head.
            Expr_::Call(callee, _) => node = callee,
            // `\arg -> entry cfg` — the lambda body is the head.
            Expr_::Lambda(_, inner) => node = inner,
            // `let … in entry cfg` — the `in` body is the head.
            Expr_::Let(_, inner) => node = inner,
            // `Web.app` / `Tui.app` / … at the head pins its shape.
            Expr_::VarQual(qual, name) => {
                let (q, n) = (interner.resolve(*qual)?, interner.resolve(*name)?);
                return shape_for(q, n);
            }
            _ => return None,
        }
    }
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
    fn webview_app_folds_onto_web() {
        assert_eq!(
            classify("module Main exposing (..)\n\nmain = WebView.app cfg\n"),
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
}
