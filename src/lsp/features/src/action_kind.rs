//! The single `context.only` filter every code-action provider answers to.
//!
//! A request's `only` list names the kinds the client wants. A kind is offered
//! when it equals a requested kind or nests under one (`source` covers
//! `source.fixAll`). With no `only`, every kind is offered except the
//! whole-document `source.*` family, which a client must name explicitly.

use lsp_types::{CodeActionKind, CodeActionOrCommand};

/// Whether an action of `kind` is offered for a request carrying `only`.
#[must_use]
pub fn offered(only: Option<&[CodeActionKind]>, kind: &CodeActionKind) -> bool {
    only.map_or_else(
        || !nests_under(CodeActionKind::SOURCE.as_str(), kind.as_str()),
        |requested| {
            requested
                .iter()
                .any(|r| nests_under(r.as_str(), kind.as_str()))
        },
    )
}

/// Drop every action `only` does not offer.
///
/// An action without a kind (a bare command, or a kindless action) matches no
/// requested kind, so it survives only a request without `only`.
pub fn retain_offered(actions: &mut Vec<CodeActionOrCommand>, only: Option<&[CodeActionKind]>) {
    actions.retain(|action| match action {
        CodeActionOrCommand::CodeAction(a) => a
            .kind
            .as_ref()
            .map_or_else(|| only.is_none(), |kind| offered(only, kind)),
        CodeActionOrCommand::Command(_) => only.is_none(),
    });
}

/// Whether `kind` equals `requested` or is nested under it.
fn nests_under(requested: &str, kind: &str) -> bool {
    kind == requested
        || kind
            .strip_prefix(requested)
            .is_some_and(|rest| rest.starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_kinds_need_an_explicit_request() {
        assert!(!offered(None, &CodeActionKind::SOURCE_FIX_ALL));
        assert!(!offered(None, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS));
        assert!(offered(None, &CodeActionKind::QUICKFIX));
        assert!(offered(None, &CodeActionKind::REFACTOR_REWRITE));
    }

    #[test]
    fn a_parent_kind_covers_its_children_only() {
        let only = [CodeActionKind::SOURCE];
        assert!(offered(Some(&only), &CodeActionKind::SOURCE_FIX_ALL));
        assert!(!offered(Some(&only), &CodeActionKind::QUICKFIX));
        let prefix_only = [CodeActionKind::new("source.fix")];
        assert!(!offered(
            Some(&prefix_only),
            &CodeActionKind::SOURCE_FIX_ALL
        ));
    }

    #[test]
    fn retain_filters_every_provider_by_only() {
        let action = |kind: Option<CodeActionKind>| {
            CodeActionOrCommand::CodeAction(lsp_types::CodeAction {
                title: String::new(),
                kind,
                ..lsp_types::CodeAction::default()
            })
        };
        let mut actions = vec![
            action(Some(CodeActionKind::QUICKFIX)),
            action(Some(CodeActionKind::REFACTOR_REWRITE)),
            action(None),
        ];
        let only = [CodeActionKind::SOURCE_FIX_ALL];
        retain_offered(&mut actions, Some(&only));
        assert!(actions.is_empty(), "{actions:?}");
    }
}
