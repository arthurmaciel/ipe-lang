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

use ipe_diagnostics::{Diagnostic, NameError};
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

/// A `main` head that head-calls one of these `(canonical-module-path, name)`
/// pairs pins the paired shape. The first element is the head's *canonical
/// module* — the full dotted stdlib path the written qualifier resolves to
/// through the import table (`Ipe.App.Tea.Web` for a `Web`/aliased head,
/// `Ipe.Http.Server` for `Server.listen`), NEVER the written qualifier token,
/// which may be an alias (`import … as S`) or accidentally collide with a
/// user module's leaf (`Acme.Server`). Matching the resolved canonical path — not
/// the spelling — is what closes the alias/rename gap and the leaf-collision gap
/// (issue #2142). Kept in lockstep with the resolver's `TEA_APP_ENTRIES` and
/// `Server.listen` entry.
const SHAPE_ENTRIES: &[(&[&str], &str, MainShape)] = &[
    (&["Ipe", "App", "Tea", "Web"], "app", MainShape::Web),
    (&["Ipe", "App", "Tea", "Web"], "appRouted", MainShape::Web),
    (&["Ipe", "App", "Tea", "Web"], "appWith", MainShape::Web),
    (&["Ipe", "App", "Tea", "Tui"], "app", MainShape::Tui),
    (&["Ipe", "App", "Tea", "Cli"], "app", MainShape::Cli),
    (&["Ipe", "Http", "Server"], "listen", MainShape::Server),
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
/// bare head brought into scope by `import Ipe.App.Tea.Web exposing (app)` classify
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

/// A top-level shape view/UI library and the shape whose app entry renders it.
/// These are the shape-agnostic-*looking* but shape-render surfaces a Script may
/// legally import (they are NOT under `Ipe.App.Tea.*`, so IPE-N0033 does not fire),
/// yet a Script has no `view` to hand them to. Each row names the shape and the
/// app entry that WOULD render this UI.
const SHAPE_VIEW_LIBRARIES: &[(&[&str], &str, &str)] = &[
    // `Ipe.Ui` / `Ipe.Html` build the DOM view a `Web.app` renders.
    (&["Ipe", "Ui"], "web", "Web.app"),
    (&["Ipe", "Html"], "web", "Web.app"),
    // `Ipe.Ui.Cells` builds the terminal-cells view a `Tui.app` renders.
    (&["Ipe", "Ui", "Cells"], "terminal", "Tui.app"),
];

/// The Script-hole hint (IPE-N0050): a **Warning** for a Script that imports a
/// shape's view/UI library but, being a Script (a plain-`Task` `main`), renders
/// nothing — so that UI never reaches a screen.
///
/// Returns `None` for any non-Script `main` (an app renders its view; no hole),
/// or a Script that imports no shape view library (nothing built to drop). When
/// it does fire, the returned [`Diagnostic`] is `Severity::Warning`: a Script is
/// a legal program, so this HINTS the likely-intended `<Shape>.app` entry rather
/// than rejecting. Emit it into the compiler's warning channel; it must never
/// fail the build.
///
/// This is deliberately distinct from IPE-N0033: that gate is a hard error for a
/// Script importing the live-loop machinery under `Ipe.App.Tea.*`; this hint is for a
/// Script importing a top-level *view* library (`Ipe.Ui` / `Ipe.Html` /
/// `Ipe.Ui.Cells`), which is legal but almost certainly a mistake.
#[must_use]
pub fn script_view_hole_hint(module: &Module, interner: &Interner) -> Option<Diagnostic> {
    if classify_main_shape(module, interner) != MainShape::Script {
        return None; // an app renders its view — no hole.
    }
    // A Script with no `main` at all cannot mean to render anything.
    let main_sym = interner.lookup("main")?;
    if !module.values.iter().any(|v| v.value.name.value == main_sym) {
        return None;
    }
    for import in &module.imports {
        let Some(path) = module_path_segments(import, interner) else {
            continue;
        };
        if let Some((_, shape, entry)) = SHAPE_VIEW_LIBRARIES
            .iter()
            .find(|(lib_path, _, _)| path_eq(lib_path, &path))
        {
            let shape_ui_module = path.join(".").into_boxed_str();
            return Some(Diagnostic::Name {
                span: import.name.span,
                msg: NameError::ScriptImportsShapeView {
                    shape_ui_module,
                    shape: (*shape).into(),
                    entry: (*entry).into(),
                },
            });
        }
    }
    None
}

/// Peel a `main` body to its head reference and match it against the shape
/// entries. `None` when the head is not a shape-entry reference.
///
/// A head is a shape entry either qualified — `Web.app`, `Server.listen`, or an
/// aliased `S.listen` from `import Ipe.Http.Server as S` — or unqualified through
/// an `import Ipe.App.Tea.Web exposing (app)` that brings the entry into scope under
/// its bare name. A qualified head's written qualifier is resolved through the
/// import table to the *canonical module* it names, then matched — never the
/// written token — so an alias or rename classifies identically to the
/// written-out module (issue #2142). An unqualified head is resolved the same way
/// its exposing import binds it.
fn head_shape(body: &Expr, module: &Module, interner: &Interner) -> Option<MainShape> {
    let mut node = body;
    loop {
        match &node.value {
            // `entry cfg` — the callee is the head.
            Expr_::Call(callee, _) => node = callee,
            // `\arg -> entry cfg` (lambda body) and `let … in entry cfg` (the
            // `in` body) both peel to the inner expression.
            Expr_::Lambda(_, inner) | Expr_::Let(_, inner) => node = inner,
            // `Server.listen` / `S.listen` (alias) / `Web.app` at the head pins
            // its shape — but only after the written qualifier is resolved to the
            // canonical module it imports; a spelling match would break on rename.
            Expr_::VarQual(qual, name) => {
                let (q, n) = (interner.resolve(*qual)?, interner.resolve(*name)?);
                return shape_for_qualified(q, n, module, interner);
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

/// The shape a qualified head `qualifier.name` pins, resolving `qualifier`
/// through the import table to the canonical module it names before matching.
///
/// `qualifier` is the *written* token at the call site — a module leaf
/// (`Server` from `import Ipe.Http.Server`) or an alias (`S` from `… as S`). We
/// resolve it exactly as name resolution would: the import whose `as` alias is
/// `qualifier`, or, absent an alias, whose module-path leaf is `qualifier`, names
/// the canonical module; that module's FULL dotted path keys the shape table.
/// Matching the full canonical path closes both gaps of #2142: an alias
/// (`S.listen` → `Ipe.Http.Server.listen` → Server) and a like-spelled user
/// module (`Acme.Server.listen` does NOT resolve to `Ipe.Http.Server`, so it
/// stays a Script).
///
/// Fail-safe: a qualifier no import in scope names (a bare reference that never
/// resolves) pins no shape (`None`) — the least-capability Script posture, never
/// a panic.
fn shape_for_qualified(
    qualifier: &str,
    name: &str,
    module: &Module,
    interner: &Interner,
) -> Option<MainShape> {
    let canonical_path = resolve_qualifier_to_module_path(qualifier, module, interner)?;
    shape_for_path(&canonical_path, name)
}

/// Resolve a written qualifier token to the dotted segments of the canonical
/// module it names, via the import table.
///
/// An `import M as A` binds the qualifier `A` to module `M`; an `import M` with
/// no alias binds `M`'s own leaf segment. So a written `qualifier` names the
/// canonical module of the first import whose alias equals it, or — when it
/// matches no alias — whose module-path leaf equals it. Returns that module's
/// full dotted path (`["Ipe", "Http", "Server"]`), which keys [`SHAPE_ENTRIES`].
///
/// Returns `None` when no import in scope names the qualifier (fail-safe to
/// Script, never a panic). An alias always shadows a leaf of the same spelling
/// (name resolution's rule), so aliases are scanned before bare-leaf imports.
fn resolve_qualifier_to_module_path(
    qualifier: &str,
    module: &Module,
    interner: &Interner,
) -> Option<Vec<String>> {
    // An `as` alias binds the qualifier directly to its module; it shadows any
    // like-spelled bare-leaf import, so aliases win.
    for import in &module.imports {
        if import
            .alias
            .and_then(|alias_sym| interner.resolve(alias_sym))
            == Some(qualifier)
        {
            return module_path_segments(import, interner);
        }
    }
    // No alias claims the qualifier: it must be a module's own leaf, brought into
    // scope by an un-aliased `import M`.
    for import in &module.imports {
        if import.alias.is_some() {
            continue;
        }
        if import
            .name
            .value
            .last()
            .and_then(|leaf| interner.resolve(*leaf))
            == Some(qualifier)
        {
            return module_path_segments(import, interner);
        }
    }
    None
}

/// The dotted path segments of an import's module name, or `None` if any segment
/// fails to resolve in the interner (fail-safe, never a panic).
fn module_path_segments(import: &Import, interner: &Interner) -> Option<Vec<String>> {
    import
        .name
        .value
        .iter()
        .map(|seg| interner.resolve(*seg).map(str::to_owned))
        .collect()
}

/// The shape a `(canonical-module-path, name)` head reference pins, or `None`
/// when the pair is not a shape-entry kernel. Matches the full dotted path, so a
/// user module that merely shares a leaf segment never masquerades as a shape.
fn shape_for_path(path: &[String], name: &str) -> Option<MainShape> {
    SHAPE_ENTRIES
        .iter()
        .find(|(module_path, n, _)| *n == name && path_eq(module_path, path))
        .map(|(_, _, shape)| *shape)
}

/// Segment-wise string equality between a static shape-module path and a
/// resolved import path.
fn path_eq(shape_path: &[&str], resolved: &[String]) -> bool {
    shape_path.len() == resolved.len()
        && shape_path
            .iter()
            .zip(resolved)
            .all(|(s, r)| *s == r.as_str())
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
        let Some(path) = module_path_segments(import, interner) else {
            continue;
        };
        if let Some(shape) = shape_for_path(&path, name) {
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

/// A **lenient** shape read for scaffolding UX only — NOT a capability gate.
///
/// [`classify_main_shape`] resolves a head's written qualifier through the import
/// table (the strict rule the capability gate keys on: an alias must not smuggle a
/// shape). That strictness is right for gating but wrong for `ipe init`'s
/// scaffold-detection, which runs against a *partially written* `src/Main.ipe`
/// that may spell `main = Tui.app config` before its `import Ipe.App.Tea.Tui` line is
/// typed. There the strict classifier reads Script (no import to resolve), and the
/// re-run guard would stop recognising the project's shape.
///
/// So this reads the shape by the *written* head qualifier's leaf spelling
/// (`Tui.app`/`Web.app`/`Cli.app`/`Server.listen`), matching the shape entries by
/// their module leaf and entry name, without requiring the import to resolve. It
/// exists purely to pick a scaffold template / detect a re-run conflict — a wrong
/// read scaffolds the wrong thing or misses a conflict, it can NEVER escalate a
/// capability. It must never be used where a capability decision is made; use
/// [`classify_main_shape`] there.
#[must_use]
pub fn scaffold_shape_hint(module: &Module, interner: &Interner) -> MainShape {
    let Some(main_sym) = interner.lookup("main") else {
        return MainShape::Script;
    };
    let Some(value) = module
        .values
        .iter()
        .find(|v| v.value.name.value == main_sym)
    else {
        return MainShape::Script;
    };
    lenient_head_shape(&value.value.body, interner).unwrap_or(MainShape::Script)
}

/// Peel a `main` body to its head the same way [`head_shape`] does, but classify
/// a qualified head by the written qualifier's leaf spelling — no import
/// resolution. Scaffolding-only (see [`scaffold_shape_hint`]).
fn lenient_head_shape(body: &Expr, interner: &Interner) -> Option<MainShape> {
    let mut node = body;
    loop {
        match &node.value {
            Expr_::Call(callee, _) => node = callee,
            Expr_::Lambda(_, inner) | Expr_::Let(_, inner) => node = inner,
            Expr_::VarQual(qual, name) => {
                let (q, n) = (interner.resolve(*qual)?, interner.resolve(*name)?);
                return shape_for_written_leaf(q, n);
            }
            _ => return None,
        }
    }
}

/// The shape a `leaf.name` head spells, matching the written qualifier leaf and
/// entry name against the shape entries' module leaf and name. Lenient: it does
/// not confirm the qualifier resolves to the shape module, so it must never gate a
/// capability (see [`scaffold_shape_hint`]).
fn shape_for_written_leaf(qualifier_leaf: &str, name: &str) -> Option<MainShape> {
    SHAPE_ENTRIES
        .iter()
        .find(|(module_path, n, _)| *n == name && module_path.last() == Some(&qualifier_leaf))
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

    fn script_hole(src: &str) -> Option<Diagnostic> {
        let mut interner = Interner::new();
        let module = ipe_parse::parse_module(src, &mut interner).expect("parse");
        script_view_hole_hint(&module, &interner)
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
            classify("module Main exposing (..)\n\nimport Ipe.App.Tea.Web\n\nmain = Web.app cfg\n"),
            MainShape::Web
        );
    }

    #[test]
    fn tui_and_cli_heads() {
        assert_eq!(
            classify("module Main exposing (..)\n\nimport Ipe.App.Tea.Tui\n\nmain = Tui.app cfg\n"),
            MainShape::Tui
        );
        assert_eq!(
            classify("module Main exposing (..)\n\nimport Ipe.App.Tea.Cli\n\nmain = Cli.app cfg\n"),
            MainShape::Cli
        );
    }

    #[test]
    fn server_listen_head_is_server() {
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.Http.Server\n\nmain = Server.listen cfg\n"
            ),
            MainShape::Server
        );
    }

    #[test]
    fn aliased_server_listen_head_is_server() {
        // Issue #2142 (O2): the classifier keys on the RESOLVED module of the
        // head, not the written qualifier. `import Ipe.Http.Server as S` +
        // `main = S.listen …` must classify Server — the alias `S` resolves to
        // `Ipe.Http.Server.listen`. A spelling match on `S` or `Server` is the
        // bug; the capability gate keyed on it would otherwise be forgeable.
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.Http.Server as S\n\nmain = S.listen cfg\n"
            ),
            MainShape::Server
        );
    }

    #[test]
    fn aliased_web_app_head_is_web() {
        // The same resolve-not-spell rule for a renamed Web import: `as W` +
        // `main = W.app …` classifies Web because `W` resolves to `Ipe.App.Tea.Web`.
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.App.Tea.Web as W\n\nmain = W.app cfg\n"
            ),
            MainShape::Web
        );
    }

    #[test]
    fn qualifier_matching_shape_leaf_by_spelling_only_stays_script() {
        // A qualifier that spells `Server` but names a DIFFERENT module (no
        // `Ipe.Http.Server` import in scope) must NOT classify Server — the
        // resolve-not-spell rule fails safe to Script. Here `Server` is the leaf
        // of an unrelated `Acme.Server` import, whose `listen` is not the shape
        // kernel.
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Acme.Server\n\nmain = Server.listen cfg\n"
            ),
            MainShape::Script
        );
    }

    fn scaffold_hint(src: &str) -> MainShape {
        let mut interner = Interner::new();
        let module = ipe_parse::parse_module(src, &mut interner).expect("parse");
        scaffold_shape_hint(&module, &interner)
    }

    #[test]
    fn scaffold_hint_reads_a_written_head_without_its_import() {
        // A partially written entry — `Tui.app` before its `import Ipe.App.Tea.Tui`
        // line is typed — reads its shape leniently for scaffold detection, where
        // the strict gate classifier correctly fails safe to Script. The two must
        // disagree here: the gate stays strict, the UX read is lenient.
        let src = "module Main exposing (main)\n\nmain =\n    Tui.app config\n";
        assert_eq!(scaffold_hint(src), MainShape::Tui);
        assert_eq!(classify(src), MainShape::Script);
    }

    #[test]
    fn scaffold_hint_stays_script_for_a_plain_task_and_a_bare_head() {
        // The lenient read is qualified-head-only: a plain Task or an unqualified
        // head is not confidently a shape.
        assert_eq!(
            scaffold_hint("module Main exposing (main)\n\nmain = Io.println \"hi\"\n"),
            MainShape::Script
        );
        assert_eq!(
            scaffold_hint("module Main exposing (main)\n\nmain =\n    app config\n"),
            MainShape::Script
        );
    }

    #[test]
    fn scaffold_hint_does_not_confuse_a_like_spelled_leaf_across_shapes() {
        // Every shape leaf reads its own shape; a non-shape head stays Script.
        assert_eq!(
            scaffold_hint("module Main exposing (main)\n\nmain = Web.app config\n"),
            MainShape::Web
        );
        assert_eq!(
            scaffold_hint("module Main exposing (main)\n\nmain = Cli.app config\n"),
            MainShape::Cli
        );
        assert_eq!(
            scaffold_hint("module Main exposing (main)\n\nmain = Server.listen config\n"),
            MainShape::Server
        );
        assert_eq!(
            scaffold_hint("module Main exposing (main)\n\nmain = Widget.app config\n"),
            MainShape::Script
        );
    }

    #[test]
    fn let_bound_config_still_classifies() {
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.App.Tea.Web\n\nmain =\n    let cfg = { init = () }\n    in Web.app cfg\n"
            ),
            MainShape::Web
        );
    }

    #[test]
    fn app_with_head_classifies_web() {
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.App.Tea.Web\n\nmain = Web.appWith cfg\n"
            ),
            MainShape::Web
        );
    }

    #[test]
    fn exposed_bare_app_head_classifies_web() {
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.App.Tea.Web exposing (app)\n\nmain = app cfg\n"
            ),
            MainShape::Web
        );
    }

    #[test]
    fn exposed_bare_app_head_from_tui_classifies_tui() {
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.App.Tea.Tui exposing (app)\n\nmain = app cfg\n"
            ),
            MainShape::Tui
        );
    }

    #[test]
    fn exposed_bare_app_through_let_classifies_web() {
        assert_eq!(
            classify(
                "module Main exposing (..)\n\nimport Ipe.App.Tea.Web exposing (app)\n\nmain =\n    let cfg = { init = () }\n    in app cfg\n"
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
                "module Main exposing (..)\n\nimport Ipe.App.Tea.Web exposing (..)\n\nmain = app cfg\n"
            ),
            MainShape::Script
        );
    }

    #[test]
    fn script_importing_web_ui_gets_hole_hint() {
        // A Script (plain-`Task` main) that imports `Ipe.Ui` builds a view it can
        // never render — a warning-level hint (IPE-N0050), not a rejection.
        let hint = script_hole(
            "module Main exposing (..)\n\nimport Ipe.Ui as Ui\n\nmain = Io.println \"hi\"\n",
        )
        .expect("expected a Script-hole hint");
        assert!(
            matches!(
                &hint,
                Diagnostic::Name {
                    msg: NameError::ScriptImportsShapeView { shape_ui_module, shape, entry },
                    ..
                } if &**shape_ui_module == "Ipe.Ui" && &**shape == "web" && &**entry == "Web.app"
            ),
            "wrong diagnostic: {hint:?}"
        );
        // It carries the Script-hole wire code and is warning-severity — it
        // must not fail the build.
        assert_eq!(hint.code(), ipe_diagnostics::IPE_N0050);
        assert_eq!(hint.severity(), ipe_diagnostics::Severity::Warning);
    }

    #[test]
    fn script_importing_cells_ui_hints_terminal() {
        let hint = script_hole(
            "module Main exposing (..)\n\nimport Ipe.Ui.Cells as Cells\n\nmain = Io.println \"hi\"\n",
        )
        .expect("expected a Script-hole hint");
        assert!(
            matches!(
                &hint,
                Diagnostic::Name {
                    msg: NameError::ScriptImportsShapeView { shape, entry, .. },
                    ..
                } if &**shape == "terminal" && &**entry == "Tui.app"
            ),
            "wrong diagnostic: {hint:?}"
        );
    }

    #[test]
    fn web_app_importing_ui_has_no_hole() {
        // A real Web app renders its `Ipe.Ui` view — no hole, no hint.
        assert!(
            script_hole(
                "module Main exposing (..)\n\nimport Ipe.App.Tea.Web\nimport Ipe.Ui as Ui\n\nmain = Web.app cfg\n"
            )
            .is_none()
        );
    }

    #[test]
    fn script_without_view_import_has_no_hole() {
        // A plain Script that imports no view library builds nothing to drop.
        assert!(script_hole("module Main exposing (..)\n\nmain = Io.println \"hi\"\n").is_none());
    }
}
