//! Production module and include sources that point into test code.
//!
//! Path-based checks skip test code, so a production `#[path = "tests/x.rs"]`
//! module or `include!("tests/x.rs")` would compile a skipped file into the
//! production build unaudited. Each such source is judged by its literal as
//! written, never resolved: a `tests` component or a final `tests.rs` names
//! test code. A source that is not a plain string literal cannot be judged and
//! is reported too, so an unreadable source never reads as a safe one.

use std::fmt;
use std::path::Path;

use proc_macro2::{Delimiter, Literal, TokenStream, TokenTree};

use crate::manifest::names_test_code;
use crate::{attr_gates_test_only, cfg_pred_is_test_only, split_top_level_commas};

/// Attribute key that relocates a module's source file.
const PATH_ATTR: &str = "path";

/// Attribute that applies its trailing attributes under a predicate.
const CFG_ATTR: &str = "cfg_attr";

/// Macro that splices another file's Rust into the current one.
const INCLUDE_MACRO: &str = "include";

/// A production source that may pull test code into the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestPathInclude {
    /// 1-based line of the source literal or macro.
    pub line: usize,
    pub form: IncludeForm,
    pub target: IncludeTarget,
}

/// How a file names another source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncludeForm {
    /// `#[path = "…"]`, directly or inside a `cfg_attr`.
    PathAttr,
    /// `include!("…")`.
    IncludeMacro,
}

/// What an include source names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncludeTarget {
    /// A plain string literal naming test code.
    TestPath(String),
    /// A value that is not a plain string literal, so its target is unknown.
    Opaque(String),
}

impl fmt::Display for IncludeForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PathAttr => "#[path]",
            Self::IncludeMacro => "include!",
        })
    }
}

impl fmt::Display for TestPathInclude {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.target {
            IncludeTarget::TestPath(path) => write!(
                f,
                "line {}: production {} names test code `{path}`, which path-based checks skip",
                self.line, self.form
            ),
            IncludeTarget::Opaque(value) => write!(
                f,
                "line {}: production {} source `{value}` is not a plain string literal, so its target is unknown",
                self.line, self.form
            ),
        }
    }
}

/// Every production `#[path]` or `include!` source in `ts` naming test code.
pub fn test_path_includes(ts: TokenStream, out: &mut Vec<TestPathInclude>) {
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    let mut test_only = false;
    let mut pending: Vec<TestPathInclude> = Vec::new();
    let mut idx = 0;
    while let Some(tok) = toks.get(idx) {
        idx = idx.saturating_add(1);
        match tok {
            TokenTree::Punct(p) if p.as_char() == '#' => {
                let inner = toks.get(idx).is_some_and(|t| is_punct(t, '!'));
                let attr_at = if inner { idx.saturating_add(1) } else { idx };
                if let Some(TokenTree::Group(g)) = toks.get(attr_at) {
                    if g.delimiter() == Delimiter::Bracket {
                        if inner {
                            attr_path_sources(&g.stream(), out);
                        } else {
                            test_only |= attr_gates_test_only(&g.stream());
                            attr_path_sources(&g.stream(), &mut pending);
                        }
                        idx = attr_at.saturating_add(1);
                    }
                }
            }
            TokenTree::Punct(p) if p.as_char() == ';' => {
                end_item(test_only, &mut pending, out);
                test_only = false;
            }
            TokenTree::Ident(id) if id == INCLUDE_MACRO => {
                if let (Some(bang), Some(TokenTree::Group(args))) =
                    (toks.get(idx), toks.get(idx.saturating_add(1)))
                {
                    if is_punct(bang, '!') && !test_only {
                        let line = id.span().start().line;
                        if let Some(target) = judge_source(&args.stream()) {
                            out.push(TestPathInclude {
                                line,
                                form: IncludeForm::IncludeMacro,
                                target,
                            });
                        }
                    }
                }
            }
            TokenTree::Group(g) => {
                let skipped = test_only && g.delimiter() == Delimiter::Brace;
                if !skipped {
                    test_path_includes(g.stream(), out);
                }
                if g.delimiter() == Delimiter::Brace {
                    end_item(test_only, &mut pending, out);
                    test_only = false;
                }
            }
            _ => {}
        }
    }
    end_item(test_only, &mut pending, out);
}

/// Close an item: its `#[path]` sources count unless the item is test-only.
fn end_item(test_only: bool, pending: &mut Vec<TestPathInclude>, out: &mut Vec<TestPathInclude>) {
    if test_only {
        pending.clear();
    } else {
        out.append(pending);
    }
}

/// Collect the `path = …` sources of attribute body `attr` naming test code.
///
/// A `cfg_attr` whose predicate is test-only contributes nothing; any other
/// `cfg_attr` contributes the sources of every attribute it applies.
fn attr_path_sources(attr: &TokenStream, out: &mut Vec<TestPathInclude>) {
    let toks: Vec<TokenTree> = attr.clone().into_iter().collect();
    match toks.as_slice() {
        [TokenTree::Ident(key), TokenTree::Punct(eq), value @ ..]
            if key == PATH_ATTR && eq.as_char() == '=' =>
        {
            let value: TokenStream = value.iter().cloned().collect();
            if let Some(target) = judge_source(&value) {
                out.push(TestPathInclude {
                    line: key.span().start().line,
                    form: IncludeForm::PathAttr,
                    target,
                });
            }
        }
        [TokenTree::Ident(key), TokenTree::Group(args)]
            if key == CFG_ATTR && args.delimiter() == Delimiter::Parenthesis =>
        {
            let operands = split_top_level_commas(&args.stream());
            let Some((pred, applied)) = operands.split_first() else {
                return;
            };
            if !cfg_pred_is_test_only(pred) {
                for nested in applied {
                    attr_path_sources(nested, out);
                }
            }
        }
        _ => {}
    }
}

/// Judge a source value: `None` when it is a plain string naming production code.
fn judge_source(value: &TokenStream) -> Option<IncludeTarget> {
    let toks: Vec<TokenTree> = value.clone().into_iter().collect();
    let decoded = match toks.as_slice() {
        [TokenTree::Literal(lit)] => plain_string(lit),
        _ => None,
    };
    decoded.map_or_else(
        || Some(IncludeTarget::Opaque(value.to_string())),
        |path| names_test_code(Path::new(&path)).then_some(IncludeTarget::TestPath(path)),
    )
}

/// The contents of a string literal with no escapes, raw or plain.
///
/// An escaped literal is not decoded: its written form could spell a `tests`
/// component the undecoded text hides, so it is left opaque.
fn plain_string(lit: &Literal) -> Option<String> {
    let text = lit.to_string();
    let raw = text.strip_prefix('r').map(|rest| rest.trim_matches('#'));
    let quoted = raw.unwrap_or(&text);
    let body = quoted.strip_prefix('"')?.strip_suffix('"')?;
    let escaped = raw.is_none() && body.contains('\\');
    (!escaped).then(|| body.to_owned())
}

/// Whether `tok` is the punctuation character `ch`.
fn is_punct(tok: &TokenTree, ch: char) -> bool {
    matches!(tok, TokenTree::Punct(p) if p.as_char() == ch)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn includes(src: &str) -> Vec<TestPathInclude> {
        let ts = TokenStream::from_str(src);
        assert!(ts.is_ok(), "{src:?} must lex");
        let mut out = Vec::new();
        test_path_includes(ts.unwrap_or_default(), &mut out);
        out
    }

    #[test]
    fn production_sources_naming_test_code_are_reported() {
        for (src, form) in [
            (
                "#[path = \"tests/prod.rs\"]\npub mod prod;",
                IncludeForm::PathAttr,
            ),
            (
                "#[path = r\"tests/prod.rs\"]\nmod prod;",
                IncludeForm::PathAttr,
            ),
            (
                "#[path = r#\"a/tests.rs\"#]\nmod prod;",
                IncludeForm::PathAttr,
            ),
            (
                "#[cfg_attr(unix, path = \"tests/prod.rs\")]\nmod prod;",
                IncludeForm::PathAttr,
            ),
            (
                "#[cfg_attr(not(test), path = \"tests/prod.rs\")]\nmod prod;",
                IncludeForm::PathAttr,
            ),
            (
                "#[cfg_attr(unix, cfg_attr(linux, path = \"tests/p.rs\"))]\nmod p;",
                IncludeForm::PathAttr,
            ),
            (
                "#[cfg(not(test))]\n#[path = \"tests/p.rs\"]\nmod p;",
                IncludeForm::PathAttr,
            ),
            ("include!(\"tests/prod.rs\");", IncludeForm::IncludeMacro),
            (
                "fn f() { include!(\"../tests/prod.rs\") }",
                IncludeForm::IncludeMacro,
            ),
            (
                "mod m { include!{\"tests.rs\"} }",
                IncludeForm::IncludeMacro,
            ),
            (
                "#[cfg(test)]\nuse x;\ninclude!(\"tests/p.rs\");",
                IncludeForm::IncludeMacro,
            ),
        ] {
            let found = includes(src);
            assert!(
                matches!(found.as_slice(), [TestPathInclude { form: f, target: IncludeTarget::TestPath(_), .. }] if *f == form),
                "{src:?} -> {found:?}"
            );
        }
    }

    #[test]
    fn opaque_sources_are_reported() {
        for src in [
            "include!(concat!(\"tests/\", \"p.rs\"));",
            "include!(\"t\\x65sts/p.rs\");",
            "#[path = \"t\\u{65}sts/p.rs\"]\nmod p;",
            "#[path = SOME_CONST]\nmod p;",
        ] {
            let found = includes(src);
            assert!(
                matches!(
                    found.as_slice(),
                    [TestPathInclude {
                        target: IncludeTarget::Opaque(_),
                        ..
                    }]
                ),
                "{src:?} -> {found:?}"
            );
        }
    }

    #[test]
    fn test_only_and_production_sources_are_not_reported() {
        for src in [
            "#[path = \"imp/unix.rs\"]\nmod imp;",
            "#[path = \"contests.rs\"]\nmod c;",
            "include!(\"generated/table.rs\");",
            "#[cfg(test)]\n#[path = \"tests/p.rs\"]\nmod p;",
            "#[path = \"tests/p.rs\"]\n#[cfg(test)]\nmod p;",
            "#[cfg(all(test, unix))]\ninclude!(\"tests/p.rs\");",
            "#[cfg_attr(test, path = \"tests/p.rs\")]\nmod p;",
            "#[cfg(test)]\nmod t {\n    include!(\"tests/p.rs\");\n}",
            "#[test]\nfn t() { include!(\"tests/p.rs\"); }",
            "const S: &str = \"include!(\\\"tests/p.rs\\\")\";",
            "include_str!(\"tests/data.txt\");",
        ] {
            let found = includes(src);
            assert!(found.is_empty(), "{src:?} -> {found:?}");
        }
    }

    #[test]
    fn a_test_only_item_does_not_shield_its_production_sibling() {
        let found = includes("#[cfg(test)]\nmod t {}\n#[path = \"tests/p.rs\"]\nmod p;");
        assert_eq!(found.len(), 1, "{found:?}");
    }
}
