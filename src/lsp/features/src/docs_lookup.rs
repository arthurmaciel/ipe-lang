//! Documentation lookup: bridge the `ipe_docs` index into LSP payloads.
//!
//! The handlers here take an already-built [`ipe_docs::Index`] (the server
//! builds it once and caches it) and turn a `(module, name)` reference, a bare
//! module path, or a diagnostic code into the LSP `documentation`/`Hover`
//! Markdown a client renders under the cursor.
//!
//! **Fail-closed by construction:** every entry point returns `None` when the
//! key does not resolve or the resolved entry carries no prose. A missing doc
//! is never surfaced as an empty box, and a wrong key never resolves to a
//! different symbol's doc — the index is keyed on the exact qualified name.

use ipe_docs::{EntryKind, Index};
use lsp_types::{Documentation, MarkupContent, MarkupKind};

/// The doc-entry text for a qualified symbol reference, if the index carries
/// one.
///
/// `module` is the symbol's home module in dotted-segment form (e.g.
/// `["Ipe", "List"]`); `name` is the bare identifier. Both the fully-qualified
/// key (`Ipe.List.map`) and the short key (`List.map`) are tried, so a
/// reference written either way resolves. Returns `None` when the name belongs
/// to no documented module (a user binding) or the entry has no body.
#[must_use]
pub fn symbol_doc(index: &Index, module: &[String], name: &str) -> Option<String> {
    for key in symbol_keys(module, name) {
        if let Some(entry) = index.resolve(&key)
            && matches!(entry.kind, EntryKind::Symbol)
            && !entry.text.trim().is_empty()
        {
            return Some(entry.text.trim().to_owned());
        }
    }
    None
}

/// The doc-entry text for a module, if the index carries a module-level doc.
///
/// Tries the dotted module path as given and its `Ipe.`-stripped short form.
/// Returns `None` for an undocumented or user module.
#[must_use]
pub fn module_doc(index: &Index, module: &[String]) -> Option<String> {
    let full = module.join(".");
    let short = full.strip_prefix("Ipe.").unwrap_or(&full).to_owned();
    for key in [full.as_str(), short.as_str()] {
        if let Some(entry) = index.resolve(key)
            && matches!(entry.kind, EntryKind::Module)
            && !entry.text.trim().is_empty()
        {
            return Some(entry.text.trim().to_owned());
        }
    }
    None
}

/// The doc-entry text for a diagnostic code (e.g. `IPE-L0107`), if the index
/// carries its explain page. Returns `None` for an unknown code.
#[must_use]
pub fn diagnostic_doc(index: &Index, code: &str) -> Option<String> {
    let entry = index.resolve(code)?;
    if matches!(entry.kind, EntryKind::Diagnostic) && !entry.text.trim().is_empty() {
        Some(entry.text.trim().to_owned())
    } else {
        None
    }
}

/// Wrap doc text as an LSP `Documentation` (Markdown) value for a completion
/// item or a signature.
#[must_use]
pub fn as_documentation(text: String) -> Documentation {
    Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: text,
    })
}

/// The candidate index keys for a qualified symbol, most-specific first: the
/// fully-qualified key, then the `Ipe.`-stripped short key.
fn symbol_keys(module: &[String], name: &str) -> Vec<String> {
    let full = module.join(".");
    let mut keys = Vec::with_capacity(2);
    if full.is_empty() {
        keys.push(name.to_owned());
        return keys;
    }
    keys.push(format!("{full}.{name}"));
    let short = full.strip_prefix("Ipe.").unwrap_or(&full);
    if short != full {
        keys.push(format!("{short}.{name}"));
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Index {
        Index::build_embedded().expect("embedded docs index builds")
    }

    /// A stdlib symbol reference resolves its doc under both its full and short
    /// home — proving the qualified key is what the enrichment surfaces.
    #[test]
    fn symbol_doc_resolves_stdlib_symbol() {
        let idx = index();
        let doc = symbol_doc(&idx, &["Ipe".to_owned(), "Maybe".to_owned()], "withDefault")
            .expect("Maybe.withDefault must resolve a doc via the full home");
        assert!(
            doc.contains("withDefault"),
            "doc body must mention the symbol; got {doc:?}"
        );
        let via_short = symbol_doc(&idx, &["Maybe".to_owned()], "withDefault")
            .expect("Maybe.withDefault must resolve via the short home too");
        assert_eq!(doc, via_short, "full and short home must resolve one doc");
    }

    /// A diagnostic code resolves its explain page.
    #[test]
    fn diagnostic_doc_resolves_explain_page() {
        let idx = index();
        let doc = diagnostic_doc(&idx, "IPE-L0107").expect("IPE-L0107 explain page must resolve");
        assert!(!doc.is_empty(), "explain page text must be present");
    }

    /// The refusals: a user binding, an unknown module, and an unknown code
    /// resolve to no doc — the enrichment is fail-closed and never guesses.
    #[test]
    fn unknown_keys_resolve_to_no_doc() {
        let idx = index();
        assert!(
            symbol_doc(&idx, &["MyApp".to_owned()], "handler").is_none(),
            "a user module symbol has no stdlib doc"
        );
        assert!(
            module_doc(&idx, &["MyApp".to_owned()]).is_none(),
            "a user module has no stdlib doc"
        );
        assert!(
            diagnostic_doc(&idx, "IPE-Z9999").is_none(),
            "an unknown diagnostic code resolves to no doc"
        );
        // A real diagnostic code must not be mistaken for a symbol.
        assert!(
            symbol_doc(&idx, &["IPE-L0107".to_owned()], "x").is_none(),
            "a diagnostic-shaped key is not a symbol doc"
        );
    }
}
