//! The frozen Doc IR: a Wadler/Leijen-style document algebra the emitter builds
//! during its owned-IR walk, so a single deterministic renderer ([`crate::render`])
//! lays it out to `rustfmt`-clean bytes without a second parse or a subprocess.
//!
//! Every token the string-emitter would have produced is carried here as a
//! [`Doc::Text`] leaf — including every parenthesis. The leaf sequence is a
//! checkable invariant (the SEAL): `whitespace_normalize(concat(leaves(doc)))`
//! must equal the whitespace-normalized string the legacy `emit_expr_at` emits,
//! so the paren-drop / token-drift class of bug is structurally impossible.
//!
//! The [`Doc::Chain`] variant exists because a generic [`Doc::Group`]
//! (all-flat-or-all-break) cannot render a binop chain's layout — first operator
//! glued to a multiline operand's closing line, the rest broken one-per-line to a
//! single shared indent. That mechanism is proven byte-exact against the golden
//! corpus in `render.rs`.
//!
//! [`Doc::Line`] / [`Doc::Softline`] are SOFT breaks: a space (resp. nothing)
//! when their nearest enclosing [`Doc::Group`] lays out flat, a newline-plus-
//! indent when it breaks. [`Doc::HardLine`] is an UNCONDITIONAL break — always a
//! newline-plus-indent, and its presence forces every enclosing group to break.
//! A statement block (`{ let x = …; x }`) carries a `HardLine` before each
//! statement so it never inlines; an inline structure (`(a, b)`, `if c {1} else
//! {2}`) carries only soft `Line`s so its group flattens when it fits.
//!
//! [`Doc::IfBroken`] is a break-conditional token: it renders only when its
//! enclosing group breaks, and is invisible to the SEAL leaf sequence — it stands
//! for `rustfmt`'s trailing comma on a broken delimited list (`f(a, b, c,)`),
//! which the legacy string emitter never emits.

// The Doc IR and renderer are the P0 deliverable; the emit_expr.rs builders that
// consume them land in P1. Until then the constructors and the SEAL leaf oracle
// are exercised only by the P0 tests, so their non-test uses are pending.
#![allow(dead_code, reason = "consumed by the P1 emit_expr.rs Doc builders")]

use std::borrow::Cow;

/// A layout document. Rendered by [`crate::render::render`].
///
/// Downstream builders compose these variants. A soft break candidate is
/// [`Doc::Line`] (a space when flat, a newline plus indent when broken) or
/// [`Doc::Softline`] (empty when flat); [`Doc::HardLine`] is an unconditional
/// break that also forces its enclosing group to break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Doc {
    /// A leaf token, carried verbatim — including every parenthesis the emitter
    /// emits. Never a break point.
    Text(Cow<'static, str>),
    /// A sequence laid out left to right with no break points of its own.
    Concat(Vec<Self>),
    /// A soft break candidate: a single space when its enclosing group is flat, a
    /// newline followed by the current indent when the group is broken.
    Line,
    /// A zero-width soft break candidate: empty when flat, a newline plus indent
    /// when broken. Used where flat layout wants no space (e.g. before a closing
    /// delimiter on a call arg list).
    Softline,
    /// An unconditional break: always a newline plus the current indent, whether
    /// or not any enclosing group is flat. Its presence anywhere inside a group
    /// forces that group to lay out broken. Used for statement separators in a
    /// block body (a block with any statement never inlines).
    HardLine,
    /// A break-conditional token: renders as its text when the nearest enclosing
    /// [`Doc::Group`] breaks, and as nothing when the group lays out flat. It is
    /// INVISIBLE to the SEAL leaf sequence (contributes nothing to
    /// [`Doc::collect_leaves`]), because the token it stands for — `rustfmt`'s
    /// trailing comma on a broken `f(a, b, c,)` / `(a, b,)` / `vec![a, b,]` — is
    /// NOT a token the legacy string emitter produces. A `Softline`-guarded `,`
    /// cannot express this: it would show the comma flat too. Never a break point
    /// itself (it introduces no newline), so it never forces a group broken.
    IfBroken(Cow<'static, str>),
    /// Indent the inner document by `n` columns relative to the current indent.
    /// Used non-accumulating (`Nest(4, ...)`) for block bodies and arg lists.
    Nest(usize, Box<Self>),
    /// A group: rendered flat if the whole group fits the remaining width,
    /// otherwise every [`Doc::Line`] / [`Doc::Softline`] in it breaks. Used for
    /// block bodies, call arg lists, and if-branch bodies.
    Group(Box<Self>),
    /// A `rustfmt` token-level brace pair whose braces appear ONLY when the body
    /// does not fit flat. It models the "braces iff the body is a block / does not
    /// fit" decision `rustfmt` applies to a closure body (`move |_| { rest }` →
    /// `move |_| rest` when it fits, `move |_| { <break> rest <break> }` when it
    /// does not) and to a wide non-block `match` arm body (`Pat => body,` inline,
    /// `Pat => { <break> body <break> }` when it breaks — comma then dropped).
    ///
    /// Rendered flat when `body` fits the remaining width: JUST the body, no
    /// braces. Rendered broken otherwise: `{`, the body on its own line at one
    /// indent step, then `}` dedented back. Its own break decision is independent
    /// of any enclosing group (like [`Doc::Group`]), because `rustfmt` re-tests the
    /// closure body against the width on its own line even when the enclosing call
    /// broke. A `body` carrying a [`Doc::HardLine`] can never fit, so it always
    /// braces (a statement-block closure body is always braced).
    ///
    /// UNLIKE [`Doc::IfBroken`], its braces ARE part of the SEAL leaf sequence: the
    /// legacy string emitter always writes the braces (`move |_| {{ {rest} }}`), so
    /// [`Doc::collect_leaves`] carries `{` + the body's leaves + `}` to keep the
    /// leaf sequence equal to the string emitter's tokens in BOTH the flat and
    /// broken cases. The flat RENDER drops the braces (matching `rustfmt`), so the
    /// rendered bytes and the leaf sequence diverge on the brace tokens exactly as
    /// they diverge on the trailing comma for [`Doc::IfBroken`] — the byte golden
    /// checks the render against `rustfmt`, the SEAL checks the leaves against the
    /// string emitter, and both hold.
    BraceBody(Box<Self>),
    /// A `match` arm tail: the arm body plus its trailing comma, laid out per
    /// `rustfmt`'s arm brace/comma rule. When the body fits on the arm's line it is
    /// rendered inline followed by a comma (`Pat => body,`). When it does not fit,
    /// the layout depends on the body's head kind, carried in `control`:
    ///
    ///   * a DELIMITED-TAIL body (`control == false`: a call / tuple / list / cons
    ///     / struct-literal / constructor / `task_and_then` — anything whose own
    ///     group breaks as a bracketed argument list) breaks INSIDE its own
    ///     delimiters, and `rustfmt` keeps the trailing comma: `Pat => call(\n …\n),`.
    ///   * a CONTROL body (`control == true`: an `if` / binary-operator chain /
    ///     block / parenthesized statement block) is wrapped by `rustfmt` in a
    ///     SYNTHESIZED brace block, and the trailing comma is dropped:
    ///     `Pat => {\n body\n}`.
    ///
    /// SEAL accounting matches the string emitter, which always writes the body
    /// followed by a comma and NEVER writes the synthesized braces: the trailing
    /// comma IS a leaf (so it appears in the SEAL) and the synthesized braces are
    /// INVISIBLE (like [`Doc::IfBroken`]). The render drops the comma and adds the
    /// braces only in the broken control case, matching `rustfmt`, so rendered bytes
    /// and leaves diverge there exactly as they do for the trailing comma on a
    /// broken delimited list. A body carrying a [`Doc::HardLine`] (a prelude block
    /// arm) never fits, so it always takes the broken path.
    MatchArmTail {
        /// The arm body's document.
        body: Box<Self>,
        /// Whether the body is a control/paren-wrapped expression (`true`) that
        /// `rustfmt` wraps in synthesized braces when it breaks, rather than a
        /// delimited-tail expression (`false`) that breaks inside its own brackets.
        control: bool,
    },
    /// A left-associative same-precedence binary-operator run. The renderer lays
    /// this out with rustfmt's chain mechanism: line-1 packs the maximal
    /// left-nested prefix that fits the width, then every remaining operator
    /// breaks one-per-line to a single shared indent (chain-begin-line indent +
    /// 4), with the sole exception of an operator glued to a multiline operand's
    /// closing line when it still fits.
    Chain {
        /// The operands and their leading operators, in source order. The first
        /// operand's `leading_op` is `None`.
        operands: Vec<ChainOperand>,
    },
}

/// One operand of a [`Doc::Chain`], with the operator that precedes it (if any).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainOperand {
    /// The infix operator immediately before this operand, e.g. `"+"`. `None`
    /// for the first operand.
    pub leading_op: Option<Cow<'static, str>>,
    /// The operand's own document. May itself render multiline (a block, a
    /// forced-break call), which drives the chain's last-line-glue decision.
    pub doc: Doc,
}

impl Doc {
    /// A text leaf from a static string.
    pub const fn text(s: &'static str) -> Self {
        Self::Text(Cow::Borrowed(s))
    }

    /// A text leaf from an owned string.
    pub const fn owned(s: String) -> Self {
        Self::Text(Cow::Owned(s))
    }

    /// A concatenation of documents.
    pub const fn concat(docs: Vec<Self>) -> Self {
        Self::Concat(docs)
    }

    /// A group (flat-if-fits-else-break).
    pub fn group(inner: Self) -> Self {
        Self::Group(Box::new(inner))
    }

    /// Indent `inner` by `n` columns.
    pub fn nest(n: usize, inner: Self) -> Self {
        Self::Nest(n, Box::new(inner))
    }

    /// A break-conditional static token (renders only when its group breaks).
    pub const fn if_broken(s: &'static str) -> Self {
        Self::IfBroken(Cow::Borrowed(s))
    }

    /// A brace-body token: `body` inline (no braces) when it fits flat, `{ body }`
    /// as a broken block otherwise. See [`Doc::BraceBody`].
    pub fn brace_body(body: Self) -> Self {
        Self::BraceBody(Box::new(body))
    }

    /// A `match` arm tail: the body plus its trailing comma, laid out per
    /// `rustfmt`'s arm brace/comma rule. See [`Doc::MatchArmTail`].
    pub fn match_arm_tail(body: Self, control: bool) -> Self {
        Self::MatchArmTail {
            body: Box::new(body),
            control,
        }
    }

    /// Append every text leaf of this document, in order, to `out`. This is the
    /// SEAL oracle: `concat(leaves(doc))` whitespace-normalizes to the legacy
    /// emitter's string. Break candidates ([`Doc::Line`] / [`Doc::HardLine`])
    /// contribute a single space so adjacency is preserved under normalization
    /// ([`Doc::Softline`] contributes nothing, as it never separates tokens).
    pub fn collect_leaves(&self, out: &mut String) {
        match self {
            Self::Text(s) => out.push_str(s),
            Self::Line | Self::HardLine => out.push(' '),
            // Invisible to the SEAL: the trailing comma it stands for is not a
            // token the legacy string emitter produces, so it must not appear in
            // the leaf sequence the SEAL compares.
            Self::Softline | Self::IfBroken(_) => {}
            Self::Concat(docs) => {
                for d in docs {
                    d.collect_leaves(out);
                }
            }
            Self::Nest(_, inner) | Self::Group(inner) => inner.collect_leaves(out),
            // The braces ARE part of the leaf sequence: the string emitter always
            // writes them, so they must appear in the SEAL comparison (unlike the
            // trailing comma above, which the string emitter never writes). A space
            // pads each brace so token adjacency survives normalization.
            Self::BraceBody(inner) => {
                out.push_str("{ ");
                inner.collect_leaves(out);
                out.push_str(" }");
            }
            // The trailing comma IS a leaf (the string emitter writes it after every
            // arm body); the synthesized braces are NOT (the string emitter never
            // writes them, so they stay invisible like `IfBroken`).
            Self::MatchArmTail { body, .. } => {
                body.collect_leaves(out);
                out.push(',');
            }
            Self::Chain { operands } => {
                for (i, op) in operands.iter().enumerate() {
                    if let Some(o) = &op.leading_op {
                        if i > 0 {
                            out.push(' ');
                        }
                        out.push_str(o);
                        out.push(' ');
                    }
                    op.doc.collect_leaves(out);
                }
            }
        }
    }

    /// The whitespace-normalized leaf string: runs of whitespace collapsed to a
    /// single space, trimmed. Two documents with the same token sequence
    /// (ignoring layout) normalize equal — this is the SEAL comparison key.
    pub fn normalized_leaves(&self) -> String {
        let mut raw = String::new();
        self.collect_leaves(&mut raw);
        whitespace_normalize(&raw)
    }
}

/// Collapse every run of ASCII whitespace to a single space and trim the ends.
/// Token adjacency (not layout) is what the SEAL checks, so this is the
/// canonical form both the Doc leaves and the legacy emitter output reduce to.
pub fn whitespace_normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if c.is_ascii_whitespace() {
            in_ws = true;
        } else {
            if in_ws && !out.is_empty() {
                out.push(' ');
            }
            in_ws = false;
            out.push(c);
        }
    }
    out
}
