//! `source.organizeImports` / `source.fixAll`: whole-document code actions.
//!
//! Both are thin adapters over `ipe_lint`'s own proven rewrites: this crate
//! computes no usage or fixability analysis of its own, only turning
//! [`ipe_lint::organize_imports`]'s block edit, or the [`ipe_lint::minimal_edit`]
//! of [`ipe_lint::fix_all`]'s result, into an LSP [`WorkspaceEdit`].

use ipe_diagnostics::Span;
use ipe_lint::{BlockEdit, LintConfig, SourceModule};
use lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, TextEdit, Url};

use crate::action_kind::offered;
use crate::offset::{PositionEncoding, span_to_range};
use crate::workspace_edit::single_edit;

/// The `source.*` kinds this crate can produce.
///
/// Parallels [`crate::refactor::advertised_kinds`]'s role for the server's
/// capability advertisement.
#[must_use]
pub fn advertised_kinds() -> Vec<CodeActionKind> {
    vec![
        CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
        CodeActionKind::SOURCE_FIX_ALL,
    ]
}

/// Whether `only` names any kind [`source_actions`] can produce.
#[must_use]
pub fn requested(only: Option<&[CodeActionKind]>) -> bool {
    advertised_kinds().iter().any(|kind| offered(only, kind))
}

/// The open document a source action edits.
#[derive(Clone, Copy, Debug)]
pub struct Document<'a> {
    /// The document's URI.
    pub uri: &'a Url,
    /// The document's current text.
    pub text: &'a str,
    /// The document version the edit applies to.
    ///
    /// Set only when the client accepts versioned `documentChanges` and the
    /// document is open; the edit is then refused by a client whose copy moved on.
    pub version: Option<i32>,
}

/// Compute the `source.organizeImports` / `source.fixAll` actions for one document.
///
/// A kind is computed only when `only` names it or a parent kind
/// ([`crate::action_kind::offered`]); a request without `only` gets none. Each
/// action is one minimal [`TextEdit`] over the lines that change, versioned
/// when [`Document::version`] is set. No action is offered when the rewrite is
/// refused or a no-op.
#[must_use]
pub fn source_actions(
    module: &[String],
    doc: Document<'_>,
    config: &LintConfig,
    only: Option<&[CodeActionKind]>,
    encoding: PositionEncoding,
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();
    let wants_organize = offered(only, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS);
    let wants_fix_all = offered(only, &CodeActionKind::SOURCE_FIX_ALL);
    if !wants_organize && !wants_fix_all {
        return actions;
    }
    let source = SourceModule {
        module: module.to_vec(),
        source: doc.text.to_owned(),
    };

    if wants_organize && let Some(edit) = ipe_lint::organize_imports(&source, config) {
        push_edit(
            &mut actions,
            "Organize imports",
            CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
            doc,
            &edit,
            encoding,
        );
    }

    if wants_fix_all
        && let Some(edit) = ipe_lint::fix_all(std::slice::from_ref(&source), module, config)
            .and_then(|after| ipe_lint::minimal_edit(doc.text, &after))
    {
        push_edit(
            &mut actions,
            "Fix all auto-fixable lint findings",
            CodeActionKind::SOURCE_FIX_ALL,
            doc,
            &edit,
            encoding,
        );
    }

    actions
}

fn push_edit(
    actions: &mut Vec<CodeActionOrCommand>,
    title: &str,
    kind: CodeActionKind,
    doc: Document<'_>,
    edit: &BlockEdit,
    encoding: PositionEncoding,
) {
    let (Ok(lo), Ok(hi)) = (u32::try_from(edit.lo), u32::try_from(edit.hi)) else {
        return;
    };
    let text_edit = TextEdit {
        range: span_to_range(doc.text, Span { lo, hi }, encoding),
        new_text: edit.replacement.clone(),
    };
    let workspace_edit = single_edit(doc.uri, doc.version, text_edit);
    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
        title: title.to_owned(),
        kind: Some(kind),
        diagnostics: None,
        edit: Some(workspace_edit),
        command: None,
        is_preferred: None,
        disabled: None,
        data: None,
    }));
}

#[cfg(test)]
mod tests {
    use lsp_types::{DocumentChanges, WorkspaceEdit};

    use super::*;

    const UNSORTED: &str = "module Main exposing (main)\n\nimport Zeta\nimport Alpha\n\nmain =\n    (Zeta.a, Alpha.b)\n";

    fn actions_for(
        text: &str,
        version: Option<i32>,
        only: Option<&[CodeActionKind]>,
    ) -> Vec<CodeActionOrCommand> {
        let Ok(uri) = Url::parse("file:///Main.ipe") else {
            return Vec::new();
        };
        source_actions(
            &["Main".to_owned()],
            Document {
                uri: &uri,
                text,
                version,
            },
            &LintConfig::default(),
            only,
            PositionEncoding::Utf16,
        )
    }

    fn has_kind(actions: &[CodeActionOrCommand], kind: &CodeActionKind) -> bool {
        actions.iter().any(|a| {
            matches!(a, CodeActionOrCommand::CodeAction(action) if action.kind.as_ref() == Some(kind))
        })
    }

    #[test]
    fn organize_imports_is_offered_when_requested() {
        let only = [CodeActionKind::SOURCE_ORGANIZE_IMPORTS];
        let actions = actions_for(UNSORTED, None, Some(&only));
        assert!(
            has_kind(&actions, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
            "organizeImports offered when imports are out of order, got {actions:?}"
        );
    }

    #[test]
    fn no_source_action_without_only() {
        let actions = actions_for(UNSORTED, None, None);
        assert!(actions.is_empty(), "{actions:?}");
        assert!(!requested(None));
        assert!(!requested(Some(&[CodeActionKind::QUICKFIX])));
        assert!(requested(Some(&[CodeActionKind::SOURCE])));
    }

    #[test]
    fn only_filters_out_the_unrequested_kind() {
        let only = [CodeActionKind::SOURCE_FIX_ALL];
        let actions = actions_for(UNSORTED, None, Some(&only));
        assert!(
            !has_kind(&actions, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
            "only: [source.fixAll] must not also return organizeImports, got {actions:?}"
        );
    }

    #[test]
    fn no_op_rewrite_offers_no_action() {
        let text = "module Main exposing (main)\n\nimport Alpha\nimport Zeta\n\nmain =\n    (Zeta.a, Alpha.b)\n";
        let only = [CodeActionKind::SOURCE_ORGANIZE_IMPORTS];
        let actions = actions_for(text, None, Some(&only));
        assert!(
            actions.is_empty(),
            "a no-op rewrite offers no edit, got {actions:?}"
        );
    }

    #[test]
    fn refused_rewrite_offers_no_action() {
        let text = "module Main exposing (main)\n\nimport Zeta\n-- note\nimport Alpha\n\nmain =\n    (Zeta.a, Alpha.b)\n";
        let only = [CodeActionKind::SOURCE];
        let actions = actions_for(text, None, Some(&only));
        assert!(
            !has_kind(&actions, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
            "{actions:?}"
        );
    }

    #[test]
    fn the_edit_covers_only_the_import_lines() {
        let only = [CodeActionKind::SOURCE_ORGANIZE_IMPORTS];
        let actions = actions_for(UNSORTED, None, Some(&only));
        let edits: Vec<&TextEdit> = actions
            .iter()
            .filter_map(|a| match a {
                CodeActionOrCommand::CodeAction(action) => action.edit.as_ref(),
                CodeActionOrCommand::Command(_) => None,
            })
            .filter_map(|e| e.changes.as_ref())
            .flat_map(|c| c.values().flatten())
            .collect();
        assert!(
            matches!(edits.as_slice(), [e] if e.range.start.line == 2
                && e.range.start.character == 0
                && e.range.end.line == 4
                && e.range.end.character == 0
                && e.new_text == "import Alpha\nimport Zeta\n"),
            "{edits:?}"
        );
    }

    #[test]
    fn a_known_version_yields_versioned_document_changes() {
        let only = [CodeActionKind::SOURCE_ORGANIZE_IMPORTS];
        let actions = actions_for(UNSORTED, Some(7), Some(&only));
        let versioned = |e: &WorkspaceEdit| {
            e.changes.is_none()
                && matches!(&e.document_changes, Some(DocumentChanges::Edits(edits))
                    if matches!(edits.as_slice(), [edit] if edit.text_document.version == Some(7)))
        };
        assert!(
            matches!(actions.as_slice(), [CodeActionOrCommand::CodeAction(action)]
                if action.edit.as_ref().is_some_and(versioned)),
            "{actions:?}"
        );
    }
}
