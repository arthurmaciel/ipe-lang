//! Document links: each `import Foo.Bar` resolved to its in-project file.
//!
//! Reads `resolve_imports` — the same edge set canonicalisation consumes —
//! so a link can only point where the compiler actually resolves. Kernel /
//! missing imports produce no link (never a guess).

use ipe_db::{ImportResolution, IpeDatabase, SourceRoot};
use ipe_diagnostics::Span;

/// One resolved import link: the import path's source span and the target
/// module's path segments (the server maps those to a URI).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImportLink {
    /// The `Foo.Bar` span in the importing module's source.
    pub span: Span,
    /// The resolved target module's path segments.
    pub target_module: Vec<String>,
}

/// Every resolved import edge of `file`, in declaration order.
#[must_use]
pub fn document_links(
    db: &IpeDatabase,
    root: SourceRoot,
    file: ipe_db::SourceFile,
) -> Vec<ImportLink> {
    let Ok(module) = ipe_db::parse(db, file) else {
        return Vec::new();
    };
    let Ok(resolutions) = ipe_db::resolve_imports(db, root, file) else {
        return Vec::new();
    };
    // `resolve_imports` iterates the AST's import declarations, so the two
    // sequences are index-aligned by construction.
    module
        .imports
        .iter()
        .zip(resolutions.iter())
        .filter_map(|(import, (path, resolution))| match resolution {
            ImportResolution::Resolved(_) => Some(ImportLink {
                span: import.name.span,
                target_module: path.clone(),
            }),
            ImportResolution::Unresolved => None,
        })
        .collect()
}
