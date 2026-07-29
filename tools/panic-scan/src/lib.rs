//! Token-level scanner for Rust abrupt-failure constructs.
//!
//! It lexes with `proc-macro2` rather than matching text, so a construct named
//! inside a string literal or a comment is invisible (a string is one opaque
//! `Literal` token; comments are dropped), and a construct split across lines
//! (`panic!\n(…)`, `obj.\nunwrap()`) is still found. That is what makes the
//! scan free of false positives and — the property that matters for an
//! attestation — free of false negatives.
//!
//! Scope: it finds every *authored, token-detectable* abrupt-failure construct.
//! Indexing (`a[i]`) and arithmetic overflow are deliberately out of scope —
//! they are not distinct tokens and are covered by clippy (`indexing_slicing`,
//! `arithmetic_side_effects`). Standard-library precondition panics
//! (`split_at`, `borrow_mut`, …) are not authored tokens and cannot be found
//! this way; they are the documented "no *authored* panic" boundary.

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use std::str::FromStr;

/// Panic-invoking macros (each may be invoked with `()`, `[]`, or `{}`).
const MACROS: &[&str] = &[
    "panic",
    "unreachable",
    "todo",
    "unimplemented",
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "assert_matches",
    "debug_assert_matches",
];

/// Panicking (or UB) methods: `x.unwrap()`, `x.expect(..)`, …
const METHODS: &[&str] = &[
    "unwrap",
    "expect",
    "unwrap_err",
    "expect_err",
    "unwrap_unchecked",
];

/// Free functions that abort/panic: `panic_any(..)`, `unreachable_unchecked()`.
const FNS: &[&str] = &["panic_any", "unreachable_unchecked"];

/// `process::abort` (panic-free hard abort) and `process::exit` (boundary-only).
const PROCESS_FNS: &[&str] = &["abort", "exit"];

/// The per-site sanction marker. A hit is suppressed when this exact text
/// appears in the contiguous block of source lines ending at the hit — i.e. on
/// the hit line or any line above it up to the nearest blank line. That block is
/// the construct plus its directly-attached annotations (`// …` audit rationale,
/// `#[allow(…)]`, statement continuations), so a marker placed by convention
/// just above the flagged construct sanctions it. The marker lives in a comment;
/// the lexer drops comments, so the suppression is applied against the *raw
/// source lines*, not the token stream. This is deliberately per-site and
/// explicit: an unannotated new construct still fails, so the gate is never
/// weakened.
pub const AUDIT_MARKER: &str = "IPE-RUST-AUDIT:ACCEPTED";

/// One flagged construct: its 1-based source line and a short token label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub line: usize,
    pub tok: String,
}

/// Scan Rust source, returning every production-region abrupt-failure hit that
/// is not sanctioned by an [`AUDIT_MARKER`] comment, or an error string if the
/// input does not lex as Rust tokens.
///
/// `#[cfg(test)]` / `#[test]` item bodies are skipped: this scanner attests the
/// *production* surface. (A `--tests` mode with the inverted rule — allow the
/// assert family, forbid the rest — is a separate entry point.)
pub fn scan_str(src: &str) -> Result<Vec<Hit>, String> {
    let ts = TokenStream::from_str(src).map_err(|e| e.to_string())?;
    let mut hits = Vec::new();
    scan_stream(ts, &mut hits);
    let lines: Vec<&str> = src.lines().collect();
    hits.retain(|h| !is_sanctioned(&lines, h.line));
    hits.sort_by_key(|h| h.line);
    Ok(hits)
}

/// True when the [`AUDIT_MARKER`] appears on the 1-based `line` or on any
/// preceding line back to (and stopping at) the nearest blank line — the
/// annotation block directly attached to the construct. A blank line bounds the
/// block so a marker on an unrelated earlier statement never leaks downward.
fn is_sanctioned(lines: &[&str], line: usize) -> bool {
    let mut idx = line; // 1-based; `lines[idx-1]` is the hit line.
    while idx >= 1 {
        let Some(text) = lines.get(idx - 1) else {
            break;
        };
        if idx != line && text.trim().is_empty() {
            break;
        }
        if text.contains(AUDIT_MARKER) {
            return true;
        }
        idx -= 1;
    }
    false
}

fn scan_stream(ts: TokenStream, hits: &mut Vec<Hit>) {
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    // When we pass a `#[cfg(test)]` / `#[test]` attribute, the next brace group
    // (the item body) is test code and is not recursed into.
    let mut skip_next_brace = false;

    for i in 0..toks.len() {
        match &toks[i] {
            // Attribute: `#` then a `[ … ]` group naming cfg(test) or test.
            TokenTree::Punct(p) if p.as_char() == '#' => {
                if let Some(TokenTree::Group(g)) = toks.get(i + 1) {
                    if g.delimiter() == Delimiter::Bracket {
                        let inner: String = g
                            .stream()
                            .to_string()
                            .chars()
                            .filter(|c| !c.is_whitespace())
                            .collect();
                        if inner == "test" || inner.contains("cfg(test") {
                            skip_next_brace = true;
                        }
                    }
                }
            }

            // Macro invocation: Ident(name) immediately followed by `!`.
            TokenTree::Ident(id) if MACROS.contains(&id.to_string().as_str()) => {
                if let Some(TokenTree::Punct(bang)) = toks.get(i + 1) {
                    if bang.as_char() == '!' {
                        hits.push(Hit {
                            line: id.span().start().line,
                            tok: format!("{id}!"),
                        });
                    }
                }
            }

            // Free-function call: `panic_any(` / `unreachable_unchecked(`.
            TokenTree::Ident(id) if FNS.contains(&id.to_string().as_str()) => {
                if let Some(TokenTree::Group(g)) = toks.get(i + 1) {
                    if g.delimiter() == Delimiter::Parenthesis {
                        hits.push(Hit {
                            line: id.span().start().line,
                            tok: id.to_string(),
                        });
                    }
                }
            }

            // Path call: `process :: (abort|exit) (`.
            TokenTree::Ident(id) if id == "process" => {
                if let (
                    Some(TokenTree::Punct(c1)),
                    Some(TokenTree::Punct(c2)),
                    Some(TokenTree::Ident(f)),
                ) = (toks.get(i + 1), toks.get(i + 2), toks.get(i + 3))
                {
                    let fname = f.to_string();
                    if c1.as_char() == ':'
                        && c2.as_char() == ':'
                        && PROCESS_FNS.contains(&fname.as_str())
                    {
                        hits.push(Hit {
                            line: f.span().start().line,
                            tok: format!("process::{fname}"),
                        });
                    }
                }
            }

            // Method call/turbofish: `. ident ( … )` or `. ident :: < … >`.
            TokenTree::Punct(dot) if dot.as_char() == '.' => {
                if let Some(TokenTree::Ident(id)) = toks.get(i + 1) {
                    if METHODS.contains(&id.to_string().as_str()) {
                        let is_call = match toks.get(i + 2) {
                            Some(TokenTree::Group(g)) => g.delimiter() == Delimiter::Parenthesis,
                            Some(TokenTree::Punct(p)) => p.as_char() == ':',
                            _ => false,
                        };
                        if is_call {
                            hits.push(Hit {
                                line: id.span().start().line,
                                tok: format!(".{id}()"),
                            });
                        }
                    }
                }
            }

            _ => {}
        }

        // Recurse into groups, skipping a test item body.
        if let TokenTree::Group(g) = &toks[i] {
            if skip_next_brace && g.delimiter() == Delimiter::Brace {
                skip_next_brace = false;
            } else {
                scan_stream(g.stream(), hits);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Lines a fixture marks with a trailing `//@HIT` are the exact set that
    /// must be found. Comment-only lines are ignored so prose that merely
    /// mentions the marker (e.g. this file's header) does not count as a case.
    fn wanted(src: &str) -> BTreeSet<usize> {
        src.lines()
            .enumerate()
            .filter(|(_, l)| l.contains("//@HIT") && !l.trim_start().starts_with("//"))
            .map(|(i, _)| i + 1)
            .collect()
    }

    #[test]
    fn positives_no_false_negatives_and_no_false_positives() {
        let src = include_str!("../fixtures/positives.rs");
        let res = scan_str(src);
        assert!(res.is_ok(), "fixture must lex: {:?}", res.err());
        let got: BTreeSet<usize> = res
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.line)
            .collect();
        let want = wanted(src);
        let missed: Vec<_> = want.difference(&got).collect();
        let extra: Vec<_> = got.difference(&want).collect();
        assert!(missed.is_empty(), "FALSE NEGATIVES at lines {missed:?}");
        assert!(extra.is_empty(), "FALSE POSITIVES at lines {extra:?}");
    }

    #[test]
    fn negatives_produce_no_hits() {
        let src = include_str!("../fixtures/negatives.rs");
        let res = scan_str(src);
        assert!(res.is_ok(), "fixture must lex: {:?}", res.err());
        let hits = res.unwrap_or_default();
        assert!(
            hits.is_empty(),
            "FALSE POSITIVES: {:?}",
            hits.iter()
                .map(|h| (h.line, h.tok.as_str()))
                .collect::<Vec<_>>()
        );
    }
}
