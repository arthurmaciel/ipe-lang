//! `source.organizeImports` / `source.fixAll`: whole-document code actions.
//!
//! Both are thin adapters over `ipe_lint`'s own rewrites: this crate computes
//! no usage or fixability analysis of its own, only turning the crate's
//! already-typed [`ipe_lint::organize_imports`] / [`ipe_lint::fix_all`]
//! rewrite into an LSP [`WorkspaceEdit`] spanning the whole document.

use std::collections::HashMap;

use ipe_diagnostics::Span;
use ipe_lint::{LintConfig, SourceModule};
use lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, TextEdit, Url, WorkspaceEdit};

use crate::offset::{PositionEncoding, span_to_range};

/// The `source.*` kinds this crate can produce — parallels
/// [`crate::refactor::advertised_kinds`]'s role for the server's capability
/// advertisement.
#[must_use]
pub fn advertised_kinds() -> Vec<CodeActionKind> {
    vec![
        CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
        CodeActionKind::SOURCE_FIX_ALL,
    ]
}

/// Compute the `source.organizeImports` / `source.fixAll` actions for one
/// document, filtered to the kinds `only` allows. `only: None` returns both —
/// the client asking for every kind, per the LSP default when a request
/// carries no `context.only`.
///
/// Each action is a single whole-document [`TextEdit`]: the entire text
/// `ipe_lint`'s own rewrite produced, verbatim. No action is offered when the
/// rewrite is a no-op (nothing to organize, nothing left to fix) — an edit is
/// offered only when there is a real edit to make.
#[must_use]
pub fn source_actions(
    module: &[String],
    uri: &Url,
    text: &str,
    config: &LintConfig,
    only: Option<&[CodeActionKind]>,
    encoding: PositionEncoding,
) -> Vec<CodeActionOrCommand> {
    let source = SourceModule {
        module: module.to_vec(),
        source: text.to_owned(),
    };
    let mut actions = Vec::new();

    if wants(only, CodeActionKind::SOURCE_ORGANIZE_IMPORTS.as_str()) {
        let rewritten = ipe_lint::organize_imports(&source, config);
        push_if_changed(
            &mut actions,
            "Organize imports",
            CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
            uri,
            text,
            &rewritten,
            encoding,
        );
    }

    if wants(only, CodeActionKind::SOURCE_FIX_ALL.as_str()) {
        let rewritten = ipe_lint::fix_all(std::slice::from_ref(&source), module, config);
        push_if_changed(
            &mut actions,
            "Fix all auto-fixable lint findings",
            CodeActionKind::SOURCE_FIX_ALL,
            uri,
            text,
            &rewritten,
            encoding,
        );
    }

    actions
}

/// Whether a client that asked for `only` (or asked for everything, when
/// `only` is `None`) should be offered `kind`.
fn wants(only: Option<&[CodeActionKind]>, kind: &str) -> bool {
    match only {
        None => true,
        Some(kinds) => kinds.iter().any(|k| kind_matches(k.as_str(), kind)),
    }
}

/// Whether `kind` satisfies a client's requested `requested` kind: an exact
/// match, or `kind` nested under `requested` (`source` matches
/// `source.fixAll`), per the LSP code-action-kind hierarchy.
fn kind_matches(requested: &str, kind: &str) -> bool {
    kind == requested
        || kind
            .strip_prefix(requested)
            .is_some_and(|rest| rest.starts_with('.'))
}

fn push_if_changed(
    actions: &mut Vec<CodeActionOrCommand>,
    title: &str,
    kind: CodeActionKind,
    uri: &Url,
    before: &str,
    after: &str,
    encoding: PositionEncoding,
) {
    if before == after {
        return;
    }
    let Ok(hi) = u32::try_from(before.len()) else {
        return;
    };
    let edit = TextEdit {
        range: span_to_range(before, Span { lo: 0, hi }, encoding),
        new_text: after.to_owned(),
    };
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
        title: title.to_owned(),
        kind: Some(kind),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        command: None,
        is_preferred: None,
        disabled: None,
        data: None,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri() -> Url {
        Url::parse("file:///Main.ipe").expect("valid test uri")
    }

    #[test]
    fn organize_imports_kind_is_offered_by_default() {
        let text = "module Main exposing (main)\n\nimport Zeta\nimport Alpha\n\nmain =\n    (Zeta.a, Alpha.b)\n";
        let actions = source_actions(
            &["Main".to_owned()],
            &uri(),
            text,
            &LintConfig::default(),
            None,
            PositionEncoding::Utf16,
        );
        assert!(
            actions.iter().any(|a| matches!(a,
                CodeActionOrCommand::CodeAction(action)
                    if action.kind == Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS)
            )),
            "organizeImports offered when imports are out of order, got {actions:?}"
        );
    }

    #[test]
    fn only_filters_out_the_unrequested_kind() {
        let text = "module Main exposing (main)\n\nimport Zeta\nimport Alpha\n\nmain =\n    (Zeta.a, Alpha.b)\n";
        let only = [CodeActionKind::SOURCE_FIX_ALL];
        let actions = source_actions(
            &["Main".to_owned()],
            &uri(),
            text,
            &LintConfig::default(),
            Some(&only),
            PositionEncoding::Utf16,
        );
        assert!(
            !actions.iter().any(|a| matches!(a,
                CodeActionOrCommand::CodeAction(action)
                    if action.kind == Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS)
            )),
            "only: [source.fixAll] must not also return organizeImports, got {actions:?}"
        );
    }

    #[test]
    fn no_op_rewrite_offers_no_action() {
        // Already sorted, nothing unused — organize_imports is a no-op.
        let text = "module Main exposing (main)\n\nimport Alpha\nimport Zeta\n\nmain =\n    (Zeta.a, Alpha.b)\n";
        let actions = source_actions(
            &["Main".to_owned()],
            &uri(),
            text,
            &LintConfig::default(),
            Some(&[CodeActionKind::SOURCE_ORGANIZE_IMPORTS]),
            PositionEncoding::Utf16,
        );
        assert!(
            actions.is_empty(),
            "a no-op rewrite offers no edit, got {actions:?}"
        );
    }
}
