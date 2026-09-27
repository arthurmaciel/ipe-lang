//! The identifiers emitted Rust may use to name an external crate.
//!
//! Text is lexed with `proc-macro2`, the token model
//! [`crate::capability_scan`] uses, so a string literal or a comment is opaque
//! and whitespace, a line break, or a comment between a segment and `::`
//! changes nothing. Every token kind and every predecessor state has an
//! explicit arm: an identifier is left out only where the grammar proves it
//! cannot name a crate, so the set over-approximates by construction.

use std::collections::BTreeSet;
use std::iter::Peekable;
use std::str::FromStr;

use proc_macro2::{Punct, Spacing, TokenStream, TokenTree, token_stream};

/// Rust text that does not lex, so nothing about the crates it names is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unlexable;

/// Every identifier `src` may use to name an external crate.
///
/// Included: an identifier opening a path (`x` in `x::y`, `::x`, `<T>::x`,
/// `..x::y`, `use::x`), one named by a `use` tree or an `extern crate` item,
/// and every identifier inside a macro invocation's input, since a macro can
/// splice any of them into a path. Left out: a keyword, a segment
/// continuing a path after a plain identifier or `crate`/`self`/`super`/`Self`
/// (`y` in `x::y`), a field or method after a lone `.`, and an identifier no
/// `::` touches outside an import. A raw identifier `r#x` counts as `x`.
///
/// # Errors
///
/// [`Unlexable`] when `src` is not a Rust token stream.
pub fn crate_references(src: &str) -> Result<BTreeSet<String>, Unlexable> {
    let stream = TokenStream::from_str(src).map_err(|_| Unlexable)?;
    let mut refs = BTreeSet::new();
    // An explicit stack of open groups keeps the walk iterative, so group
    // nesting depth costs heap bounded by the token count, never call stack.
    let mut levels = vec![Level::new(stream, false, false)];
    while let Some(level) = levels.last_mut() {
        let Some(tree) = level.tokens.next() else {
            levels.pop();
            continue;
        };
        if let Some(inner) = level.read(&tree, &mut refs) {
            levels.push(inner);
        }
    }
    Ok(refs)
}

/// What the previous token of a group makes of the next one.
#[derive(Clone, Copy)]
enum Prev {
    /// A plain identifier or `crate`/`self`/`super`/`Self`: a `::` after it
    /// continues its path.
    Segment,
    /// A `::` continuing a path: the next identifier is not a crate name.
    ContinuingSep,
    /// A `::` opening a path: the next identifier is a crate name.
    OpeningSep,
    /// A lone `.`: the next identifier is a field or method.
    Dot,
    /// A `!`: a group after it is a macro invocation's input.
    Bang,
    /// The `extern` keyword: a `crate` after it opens an import.
    Extern,
    /// Anything else.
    Other,
}

/// One group's token walk.
struct Level {
    tokens: Peekable<token_stream::IntoIter>,
    /// Inside a macro invocation's input, where no position proves an
    /// identifier does not name a crate.
    verbatim: bool,
    /// Inside a `use` tree or an `extern crate` item, where a bare identifier
    /// names a crate.
    import: bool,
    prev: Prev,
    /// The identifier just read, a crate name exactly when a `::` follows it.
    pending: Option<String>,
}

impl Level {
    fn new(stream: TokenStream, verbatim: bool, import: bool) -> Self {
        Self {
            tokens: stream.into_iter().peekable(),
            verbatim,
            import,
            prev: Prev::Other,
            pending: None,
        }
    }

    /// Classify one token, returning the walk of a group it opens.
    fn read(&mut self, tree: &TokenTree, refs: &mut BTreeSet<String>) -> Option<Self> {
        let pending = self.pending.take();
        match tree {
            TokenTree::Ident(ident) => {
                self.read_ident(&ident.to_string(), refs);
                None
            }
            TokenTree::Punct(punct) => {
                self.read_punct(punct, pending, refs);
                None
            }
            TokenTree::Literal(_) => {
                self.prev = Prev::Other;
                None
            }
            TokenTree::Group(group) => {
                let verbatim = self.verbatim || matches!(self.prev, Prev::Bang);
                self.prev = Prev::Other;
                Some(Self::new(group.stream(), verbatim, self.import))
            }
        }
    }

    fn read_ident(&mut self, text: &str, refs: &mut BTreeSet<String>) {
        let (name, raw) = text
            .strip_prefix("r#")
            .map_or((text, false), |name| (name, true));
        if !raw && ipe_intern::is_rust_keyword(name) {
            let next = match name {
                "crate" if matches!(self.prev, Prev::Extern) => {
                    self.import = true;
                    Prev::Segment
                }
                "crate" | "self" | "super" | "Self" => Prev::Segment,
                "use" => {
                    self.import = true;
                    Prev::Other
                }
                "extern" => Prev::Extern,
                _ => Prev::Other,
            };
            self.prev = next;
            return;
        }
        if self.verbatim {
            refs.insert(name.to_owned());
            self.prev = Prev::Other;
            return;
        }
        match self.prev {
            Prev::ContinuingSep | Prev::Dot => {}
            Prev::OpeningSep => {
                refs.insert(name.to_owned());
            }
            Prev::Segment | Prev::Bang | Prev::Extern | Prev::Other => {
                if self.import {
                    refs.insert(name.to_owned());
                } else {
                    self.pending = Some(name.to_owned());
                }
            }
        }
        self.prev = Prev::Segment;
    }

    fn read_punct(&mut self, punct: &Punct, pending: Option<String>, refs: &mut BTreeSet<String>) {
        let joint = punct.spacing() == Spacing::Joint;
        let next = match punct.as_char() {
            ':' if joint && self.next_is(':') => {
                self.tokens.next();
                if let Some(name) = pending {
                    refs.insert(name);
                }
                if matches!(self.prev, Prev::Segment) {
                    Prev::ContinuingSep
                } else {
                    Prev::OpeningSep
                }
            }
            // A run of dots (`..`, `...`, `..=`) is a range, never a receiver.
            '.' if joint && self.next_is('.') => {
                while self.next_is('.') {
                    self.tokens.next();
                }
                Prev::Other
            }
            '.' => Prev::Dot,
            '!' => Prev::Bang,
            ';' => {
                self.import = false;
                Prev::Other
            }
            _ => Prev::Other,
        };
        self.prev = next;
    }

    fn next_is(&mut self, c: char) -> bool {
        matches!(self.tokens.peek(), Some(TokenTree::Punct(p)) if p.as_char() == c)
    }
}

#[cfg(test)]
mod tests {
    use super::{Unlexable, crate_references};
    use std::collections::BTreeSet;

    fn refs(src: &str) -> BTreeSet<String> {
        crate_references(src).unwrap_or_default()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn path_roots_are_references_and_continuations_are_not() {
        assert_eq!(
            refs(
                "use ::syn::parse::Parser; fn f(x: serde_json::Value) -> Vec<a::B> \
                 { x.get::<c::D>(); <T as e::F>::g(); crate::h::i(); self::j::k(); x.len() }"
            ),
            set(&["syn", "serde_json", "a", "c", "e", "g"])
        );
        assert_eq!(refs("fn f() -> k::L { m => n::O }"), set(&["k", "n"]));
    }

    #[test]
    fn a_range_before_a_root_is_not_a_receiver() {
        assert!(refs("S { ..syn::X::default() }").contains("syn"));
        assert!(refs("let r = 0..syn::MAX;").contains("syn"));
        assert!(refs("let r = 0..=syn::MAX;").contains("syn"));
    }

    #[test]
    fn a_keyword_before_a_separator_opens_the_path() {
        for src in [
            "use::syn::X;",
            "fn f(x: &mut::syn::X) {}",
            "let y = x as::syn::T;",
            "for v in::syn::iter() {}",
            "fn f() -> impl::syn::Tr {}",
        ] {
            assert!(refs(src).contains("syn"), "{src}");
        }
    }

    #[test]
    fn a_comparison_before_a_separator_opens_the_path() {
        assert!(refs("let b = a>::syn::C;").contains("syn"));
        assert!(refs("let b = a < c && d>::syn::C;").contains("syn"));
    }

    #[test]
    fn whitespace_and_comments_do_not_split_a_path() {
        assert!(refs("fn f() -> syn /* note */ :: Ident {}").contains("syn"));
        assert!(refs("fn f() -> syn\n    ::Ident {}").contains("syn"));
        assert!(refs(":: syn :: Ident").contains("syn"));
    }

    #[test]
    fn literals_and_comments_are_opaque() {
        assert!(refs("let s = \"syn::X\"; // syn::Y\n/* syn::Z */").is_empty());
    }

    #[test]
    fn imports_name_crates_without_a_separator() {
        assert!(refs("use syn;").contains("syn"));
        assert!(refs("use syn as s;").contains("syn"));
        assert!(refs("use {syn, quote};").is_superset(&set(&["syn", "quote"])));
        assert!(refs("extern crate syn;").contains("syn"));
        // An import ends at its `;`: a later bare binding is not a reference.
        assert!(!refs("use a::b; fn f(syn: u8) {}").contains("syn"));
    }

    #[test]
    fn a_raw_identifier_counts_as_its_name() {
        assert!(refs("r#syn::X").contains("syn"));
        assert!(!refs("let r#type = 1;").contains("type"));
    }

    #[test]
    fn every_identifier_in_macro_input_is_a_reference() {
        assert!(refs("m!(syn)").contains("syn"));
        assert!(refs("m!(a::syn::X)").contains("syn"));
        assert!(refs("m![x.syn::y]").contains("syn"));
        assert!(refs("m! { crate::syn::Z }").contains("syn"));
    }

    #[test]
    fn continuations_fields_and_bindings_are_not_references() {
        for src in [
            "a::syn::X",
            "x.syn::<T>()",
            "self::syn::X",
            "crate::syn::X",
            "super::syn::X",
            "fn f(syn: u8) -> u8 { syn }",
            "x.syn",
        ] {
            assert!(!refs(src).contains("syn"), "{src}");
        }
    }

    #[test]
    fn unlexable_text_is_refused() {
        assert_eq!(crate_references("\"unterminated"), Err(Unlexable));
        assert_eq!(crate_references("fn f() { ( }"), Err(Unlexable));
    }
}
