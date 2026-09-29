//! The one builder every LSP provider's [`WorkspaceEdit`] passes through, so
//! an unversioned edit is unrepresentable outside this module.
//!
//! Emits versioned `documentChanges` when the client accepts them
//! (`workspace.workspaceEdit.documentChanges`), carrying each touched
//! document's version. A document this server does not track (not an open
//! overlay) has no buffer that can go stale, so `version_of` returning `None`
//! for it is not a refusal — the LSP spec allows a `null` version on a
//! `documentChanges` entry for exactly this case. The edit falls back to the
//! unversioned flat `changes` map only when the client lacks the capability
//! altogether, since such a client may not understand `documentChanges` at
//! all.

use std::collections::HashMap;

use lsp_types::{
    DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier, TextDocumentEdit, TextEdit,
    Url, WorkspaceEdit,
};

/// Build a `WorkspaceEdit` over one or more documents.
///
/// `document_changes_supported` mirrors the client's `documentChanges`
/// capability for the session; `version_of` is asked once per touched
/// document. Callers with nothing to send should not call this — an empty
/// `edits` yields a `WorkspaceEdit` with an empty map, never `None`, since
/// "no edit" is a decision every call site already makes before reaching
/// here.
#[must_use]
pub fn workspace_edit(
    edits: impl IntoIterator<Item = (Url, Vec<TextEdit>)>,
    document_changes_supported: bool,
    version_of: impl Fn(&Url) -> Option<i32>,
) -> WorkspaceEdit {
    if document_changes_supported {
        let doc_edits: Vec<TextDocumentEdit> = edits
            .into_iter()
            .map(|(uri, text_edits)| TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    version: version_of(&uri),
                    uri,
                },
                edits: text_edits.into_iter().map(OneOf::Left).collect(),
            })
            .collect();
        WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(doc_edits)),
            change_annotations: None,
        }
    } else {
        let changes: HashMap<Url, Vec<TextEdit>> = edits.into_iter().collect();
        WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }
    }
}

/// Build a `WorkspaceEdit` for a single document and a single [`TextEdit`] —
/// the common case for every quick-fix and refactor provider.
///
/// Versioned exactly when `version` is `Some` — equivalent to
/// [`workspace_edit`] with `document_changes_supported = version.is_some()`
/// and one document, so a caller that does not track `uri`'s version (or
/// whose client lacks the capability) transparently gets the unversioned
/// fallback.
#[must_use]
pub fn single_edit(uri: &Url, version: Option<i32>, edit: TextEdit) -> WorkspaceEdit {
    workspace_edit(
        std::iter::once((uri.clone(), vec![edit])),
        version.is_some(),
        move |_| version,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(text: &str) -> TextEdit {
        TextEdit {
            range: lsp_types::Range::default(),
            new_text: text.to_owned(),
        }
    }

    #[test]
    fn single_edit_with_no_version_is_unversioned_changes() {
        let Ok(u) = Url::parse("file:///a.ipe") else {
            return;
        };
        let ws = single_edit(&u, None, edit("x"));
        assert!(ws.document_changes.is_none());
        assert!(matches!(&ws.changes, Some(m) if m.contains_key(&u)));
    }

    #[test]
    fn single_edit_with_a_version_is_versioned_document_changes() {
        let Ok(u) = Url::parse("file:///a.ipe") else {
            return;
        };
        let ws = single_edit(&u, Some(3), edit("x"));
        assert!(ws.changes.is_none());
        assert!(matches!(&ws.document_changes,
            Some(DocumentChanges::Edits(edits))
                if matches!(edits.as_slice(), [e] if e.text_document.version == Some(3))));
    }

    #[test]
    fn multi_document_edit_carries_a_null_version_for_an_untracked_document() {
        let (Ok(tracked), Ok(untracked)) = (
            Url::parse("file:///open.ipe"),
            Url::parse("file:///closed.ipe"),
        ) else {
            return;
        };
        let tracked_clone = tracked.clone();
        let ws = workspace_edit(
            [
                (tracked.clone(), vec![edit("a")]),
                (untracked.clone(), vec![edit("b")]),
            ],
            true,
            move |u| (*u == tracked_clone).then_some(9),
        );
        assert!(ws.changes.is_none());
        let Some(DocumentChanges::Edits(edits)) = &ws.document_changes else {
            return;
        };
        let versioned = edits
            .iter()
            .find(|e| e.text_document.uri == tracked)
            .and_then(|e| e.text_document.version);
        let unversioned = edits
            .iter()
            .find(|e| e.text_document.uri == untracked)
            .and_then(|e| e.text_document.version);
        assert_eq!(versioned, Some(9));
        assert_eq!(unversioned, None);
        assert_eq!(edits.len(), 2, "both documents carry an edit, {edits:?}");
    }

    #[test]
    fn capability_off_always_falls_back_to_flat_changes_even_with_a_known_version() {
        let Ok(u) = Url::parse("file:///a.ipe") else {
            return;
        };
        let u2 = u.clone();
        let ws = workspace_edit([(u.clone(), vec![edit("x")])], false, move |q| {
            (*q == u2).then_some(1)
        });
        assert!(ws.document_changes.is_none());
        assert!(matches!(&ws.changes, Some(m) if m.contains_key(&u)));
    }
}
