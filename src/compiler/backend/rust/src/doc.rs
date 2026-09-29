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
    // Dead only in the production build: the P0 render tests construct it, so the
    // expectation is scoped to `not(test)` where it genuinely never appears.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reserved for zero-width break sites not yet emitted"
        )
    )]
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
    #[expect(
        dead_code,
        reason = "reserved for trailing-comma-on-break sites not yet emitted"
    )]
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
    /// An assignment whose right-hand side breaks to its own line when the
    /// `prefix RHS` overflows — `rustfmt`'s dedicated `let name: TYPE = RHS`
    /// layout axis, distinct from breaking into the RHS's own delimiters. The
    /// `prefix` carries the `let name: TYPE = ` tokens (up to and including the
    /// `= `); `rhs` is the value document. Three layouts, in `rustfmt`'s order:
    ///
    ///   * FLAT — `prefix RHS` plus the `trailer` (the following `;` this
    ///     assignment is a statement of, in columns) fits the remaining width:
    ///     rendered on one line, `prefix` then the flat `rhs`.
    ///   * RHS-BREAK — the same-line form overflows, but the flat `rhs` plus the
    ///     `trailer` fits at one indent step past the block indent: `prefix` on its
    ///     line, then a newline and the flat `rhs` at `indent + 4`. The break is
    ///     pure whitespace, so it is INVISIBLE to the SEAL (like [`Doc::HardLine`])
    ///     — the string emitter writes `prefix RHS` with a single separating space,
    ///     which normalizes equal.
    ///   * DELIMITER-BREAK — even at `indent + 4` the flat `rhs` plus the `trailer`
    ///     overflows: the `rhs` stays glued to `prefix` and breaks into its own
    ///     delimiters (rendered non-flat at the block indent), `rustfmt`'s fallback.
    ///
    /// `trailer` is the width `rustfmt` reserves after the `rhs` on whichever line
    /// it lands — the trailing `;` when this assignment is a block statement (the
    /// only shape the emitter builds), so `trailer == 1`. It is carried explicitly
    /// rather than assumed so the fit decision is a function of the node alone.
    ///
    /// The whole assignment decides its layout independently of any enclosing
    /// group (like [`Doc::Group`] / [`Doc::BraceBody`]) — `rustfmt` re-tests the
    /// RHS against the width on its own line.
    Assign {
        /// The `let name: TYPE = ` tokens, up to and including the `= `.
        prefix: Box<Self>,
        /// The right-hand-side value document.
        rhs: Box<Self>,
        /// Columns reserved after the `rhs` on its line (the trailing `;`).
        trailer: usize,
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
    /// A bracket-delimited argument list (`f(a, b)`, `(a, b)`, `vec![a, b]`,
    /// `Ctor(a, b)`) laid out with `rustfmt`'s call-argument COMBINING rule — the
    /// "combining openings and closings" behavior. Three layouts, in `rustfmt`'s
    /// order:
    ///
    ///   * FLAT — the whole `open a, b close` fits single-line: rendered inline,
    ///     no trailing comma.
    ///   * COMBINED (head-glue) — the list has EXACTLY ONE element and that element
    ///     renders multiline as a combinable construct (its own bracketed/braced
    ///     break: a nested call / macro / block / tuple / list / ctor). `rustfmt`
    ///     glues `open` to the element's own head and lets the element break IN
    ///     PLACE at the CURRENT indent (not one step deeper), then glues `close`
    ///     onto the element's closing line — `f(g(\n    x,\n))`. No trailing comma,
    ///     no per-element break. This nests: a chain of single-argument calls all
    ///     glue their heads (`io_println(string_from_int(list_length(\n …))`).
    ///   * ONE-PER-LINE — otherwise (more than one element, or the sole element is
    ///     not a combinable multiline construct): each element on its own line at
    ///     one indent step, a break-conditional trailing comma (unless
    ///     `trailing_comma == false` for a macro), the close dedented back — the
    ///     plain broken delimited form.
    ///
    /// The break decision is independent of any enclosing group (like [`Doc::Group`]
    /// / [`Doc::BraceBody`]): `rustfmt` re-tests the argument list against the width
    /// on its own line. SEAL accounting matches the plain delimited group: the
    /// trailing comma is SEAL-invisible (the string emitter never writes it), every
    /// other token is a leaf.
    CallArgs {
        /// The opening delimiter and any callee/name prefix (`f(`, `(`, `vec![`).
        open: Box<Self>,
        /// The argument documents, in source order.
        elems: Vec<Self>,
        /// The closing delimiter (`)`, `]`).
        close: Box<Self>,
        /// Whether the broken one-per-line form carries a trailing comma. `false`
        /// for a macro argument list (`format!` / `vec!`-macro), which `rustfmt`
        /// breaks without one.
        trailing_comma: bool,
    },
    /// A struct literal `Name { f0: v0, f1: v1 }`, laid out with `rustfmt`'s
    /// `struct_lit_width` (default 18) rule. Unlike a [`Doc::CallArgs`] (gated by
    /// `fn_call_width` = 60), a struct literal stays on one line ONLY when its FIELD
    /// TEXT — the span between the braces, trimmed of the hugging spaces — fits 18
    /// columns AND the whole line fits `max_width`. Otherwise it breaks one field
    /// per line with a trailing comma, `close` dedented back to `open`'s column.
    ///
    /// Flat form hugs the braces WITH a space (`Name { a: 1, b: 2 }`); the break
    /// decision is independent of any enclosing group (like [`Doc::CallArgs`]), so
    /// a struct literal nested in a broken outer construct re-tests its own field
    /// width. SEAL accounting matches [`Doc::CallArgs`]: `open`, each `field: value`
    /// joined by `, `, and `close` are leaves; the trailing comma is SEAL-invisible.
    StructLit {
        /// The `Name {` opening (the struct name and the brace).
        open: Box<Self>,
        /// The `field: value` documents, in source order.
        fields: Vec<Self>,
        /// The closing `}`.
        close: Box<Self>,
    },
    /// An angle-bracketed generic type carrying a `+`-separated trait-bound list —
    /// `Ptr<Head + T1 + T2 + …>` where `Ptr` is a pointer path (`Box`,
    /// `::std::sync::Arc`), `Head` is the first bound (`dyn Fn(…) -> R`), and each
    /// `Ti` is a marker trait (`Send`, `Sync`, `'static`). `rustfmt` lays this out
    /// in one of three forms, in order:
    ///
    ///   * FLAT — `Ptr<Head + T1 + …>` fits: rendered inline.
    ///   * ANGLE-BREAK — the flat form overflows: `Ptr<` on the opening line, the
    ///     whole bound list at one indent step (`Head + T1 + …,` with a trailing
    ///     comma), and `>` dedented back to `Ptr`'s column.
    ///   * BOUND-BREAK — even at the indent step the bound list overflows: `Ptr<`,
    ///     then `Head` on its own line, each `+ Ti` on its own line at a further
    ///     indent step, a trailing comma after the last, and `>` dedented back.
    ///
    /// Used only for the `let __ipe_fn: <TypedFn> = ` annotation prefix of a boxed /
    /// shared closure binding, whose wide `Box<dyn Fn(…) -> R + Send + Sync +
    /// 'static>` type `rustfmt` breaks when the binding sits at a deep indent. The
    /// break decision is independent of any enclosing group. SEAL accounting: `ptr`,
    /// `<`, `head`, each `+ Ti`, and `>` are all leaves (the string emitter writes
    /// the same tokens); the trailing comma of a broken bound list is SEAL-invisible.
    TypeBound {
        /// The pointer path and opening angle bracket, e.g. `Box<` / `::std::sync::Arc<`.
        ptr_open: Box<Self>,
        /// The first bound (`dyn Fn(…) -> R`), never itself broken here.
        head: Box<Self>,
        /// The marker traits after `head`, each a `+`-prefixed leaf token WITHOUT
        /// the leading `+ ` (e.g. `Send`, `Sync`, `'static`), in source order.
        traits: Vec<Self>,
        /// The closing angle bracket `>`.
        close: Box<Self>,
    },
    /// A REDUNDANT wrapping paren pair `( inner )` that `rustfmt` elides when `inner`
    /// is itself already parenthesized — a doubled `(( … ))` collapses to `( … )`.
    /// The string emitter's `({f})(args)` application form writes both pairs when `f`
    /// already renders as `({ … })`, so the outer pair is redundant; `rustfmt` drops
    /// it. Rendered as JUST `inner` when `inner`'s rendered form begins with `(` (a
    /// self-parenthesizing block / paren-expr), else as `( inner )`.
    ///
    /// SEAL accounting: the parens ARE leaves (the string emitter writes them), so
    /// they appear in the leaf sequence whether or not the render drops them — the
    /// same rendered-vs-leaves divergence as [`Doc::IfBroken`]'s trailing comma and
    /// [`Doc::BraceBody`]'s braces. The byte golden checks the render (parens
    /// dropped), the SEAL checks the leaves (parens present), and both hold.
    ElidableParen {
        /// The inner document whose leading `(` makes the wrapping parens redundant.
        inner: Box<Self>,
    },
    /// A match-arm OR-PATTERN `P0 | P1 | … | Pn`, laid out with `rustfmt`'s
    /// or-pattern list rule. The alternatives stay on one line joined ` | ` when
    /// the flat run fits the width `rustfmt` gives an arm pattern — `max_width`
    /// less the pattern's start column less the 5 columns it reserves for
    /// ` => {` before rewriting the pattern. Otherwise EVERY alternative goes on
    /// its own line at the pattern's start column, the first bare and each
    /// subsequent one led by `| ` (`rustfmt`'s front-placed vertical-bar
    /// separator):
    ///
    /// ```text
    /// Currency::USD
    /// | Currency::EUR
    /// | Currency::GBP => true,
    /// ```
    ///
    /// Not modeled: `rustfmt`'s mixed FILL layout, taken only when every
    /// alternative is "short" (≤ 20 columns AND literal/binding-shaped). The
    /// emitter's or-pattern alternatives are constructor paths
    /// (`Type::Variant(…)`), which `rustfmt` never classifies short, so the
    /// horizontal/vertical rule here is exact for every emitted shape.
    ///
    /// SEAL accounting: the alternatives joined by ` | ` are leaves — exactly
    /// the bytes the string emitter writes — in both layouts (the vertical
    /// form's newlines are pure whitespace, and its line-leading `| `
    /// separators normalize to the flat form's ` | `).
    OrPattern {
        /// The alternatives, in source order, each a single-line pattern.
        alts: Vec<Cow<'static, str>>,
    },
    /// A trailing `.method(…)` applied to a receiver, laid out with `rustfmt`'s
    /// method-chain rule keyed off whether the receiver is BLOCK-SHAPED. When the
    /// `receiver` renders as a plain single-line non-brace expression, the method
    /// glues inline (`get_or_init(|| (a / b)).clone()`). When the `receiver` is
    /// block-shaped — its rendered form spans multiple lines OR carries a brace block
    /// `{ … }` (a closure body / `if` / `match` / statement block) — `rustfmt` drops
    /// the method onto its OWN line at the receiver's begin-line indent
    /// (`recv…\n})\n.method()` or `recv…{…}\n.method()`), even when the whole line
    /// would fit. The break decision is a function of the receiver's own layout,
    /// independent of any enclosing group.
    ///
    /// SEAL accounting: `receiver` and `method` are both leaves (the string emitter
    /// writes `recv.method()` with no break); the newline is pure whitespace and
    /// normalizes away, so the leaf sequence is identical in both layouts.
    MethodChain {
        /// The receiver document whose closing-line kind drives the method's break.
        receiver: Box<Self>,
        /// The trailing `.method(…)` text, including the leading dot.
        method: Box<Self>,
    },
    /// A parenthesized `if`/`else` expression `(if cond { then } else { else })`,
    /// laid out with `rustfmt`'s `single_line_if_else_max_width` rule. The whole
    /// construct stays on one line when the un-parenthesized `if cond { then } else
    /// { else }` text is at most `single_line_if_else_max_width` (default 50)
    /// columns wide — an ABSOLUTE, column-independent threshold — and otherwise
    /// breaks each branch body onto its own line in block form:
    ///
    /// ```text
    /// (if cond {
    ///     then
    /// } else {
    ///     else
    /// })
    /// ```
    ///
    /// Unlike a [`Doc::Group`], the threshold is absolute (measured from the flat
    /// leaf widths, not the remaining column budget), so this cannot be a soft
    /// group. The break decision is independent of any enclosing group. SEAL
    /// accounting: `(if `, `cond`, ` { `, `then`, ` } else { `, `else`, and ` })`
    /// are all leaves (the string emitter writes those exact tokens); the block
    /// form's newlines are pure whitespace and normalize to the flat separators.
    IfElse {
        /// The condition document (a single-line leaf run).
        cond: Box<Self>,
        /// The then-branch body document.
        then_: Box<Self>,
        /// The else-branch body document.
        else_: Box<Self>,
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
    #[expect(
        dead_code,
        reason = "reserved for trailing-comma-on-break sites not yet emitted"
    )]
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

    /// An assignment whose `rhs` breaks to its own line when `prefix rhs`
    /// overflows. `trailer` is the width reserved after the `rhs` (a trailing
    /// `;` for a block-statement assignment is `1`). See [`Doc::Assign`].
    pub fn assign(prefix: Self, rhs: Self, trailer: usize) -> Self {
        Self::Assign {
            prefix: Box::new(prefix),
            rhs: Box::new(rhs),
            trailer,
        }
    }

    /// A bracket-delimited argument list laid out with `rustfmt`'s combining rule.
    /// See [`Doc::CallArgs`].
    pub fn call_args(open: Self, elems: Vec<Self>, close: Self, trailing_comma: bool) -> Self {
        Self::CallArgs {
            open: Box::new(open),
            elems,
            close: Box::new(close),
            trailing_comma,
        }
    }

    /// A struct literal laid out with `rustfmt`'s `struct_lit_width` rule.
    /// See [`Doc::StructLit`].
    pub fn struct_lit(open: Self, fields: Vec<Self>, close: Self) -> Self {
        Self::StructLit {
            open: Box::new(open),
            fields,
            close: Box::new(close),
        }
    }

    /// An angle-bracketed generic bound `Ptr<Head + T1 + …>` that breaks the angle
    /// brackets (and, if needed, the `+`-list) when it overflows. See
    /// [`Doc::TypeBound`].
    pub fn type_bound(ptr_open: Self, head: Self, traits: Vec<Self>, close: Self) -> Self {
        Self::TypeBound {
            ptr_open: Box::new(ptr_open),
            head: Box::new(head),
            traits,
            close: Box::new(close),
        }
    }

    /// A redundant wrapping paren pair around `inner`, elided at render when `inner`
    /// is already parenthesized. See [`Doc::ElidableParen`].
    pub fn elidable_paren(inner: Self) -> Self {
        Self::ElidableParen {
            inner: Box::new(inner),
        }
    }

    /// A match-arm or-pattern `P0 | P1 | …` that breaks one alternative per line
    /// when the flat run overflows the arm-pattern width. See [`Doc::OrPattern`].
    pub const fn or_pattern(alts: Vec<Cow<'static, str>>) -> Self {
        Self::OrPattern { alts }
    }

    /// A trailing `.method(…)` on `receiver`, dropped to its own line when the
    /// receiver renders multiline. See [`Doc::MethodChain`].
    pub fn method_chain(receiver: Self, method: Self) -> Self {
        Self::MethodChain {
            receiver: Box::new(receiver),
            method: Box::new(method),
        }
    }

    /// A parenthesized `if`/`else` that breaks its branches to block form when the
    /// construct exceeds `single_line_if_else_max_width`. See [`Doc::IfElse`].
    pub fn if_else(cond: Self, then_: Self, else_: Self) -> Self {
        Self::IfElse {
            cond: Box::new(cond),
            then_: Box::new(then_),
            else_: Box::new(else_),
        }
    }

    /// Append every text leaf of this document, in order, to `out`.
    ///
    /// This is the SEAL oracle: `concat(leaves(doc))` whitespace-normalizes to the
    /// legacy emitter's string. Break candidates ([`Doc::Line`] / [`Doc::HardLine`])
    /// contribute a single space so adjacency is preserved under normalization
    /// ([`Doc::Softline`] contributes nothing, as it never separates tokens).
    pub fn collect_leaves(&self, out: &mut String) {
        self.write_leaves(&mut LeafSink::new(LeafMode::Seal, out));
    }

    /// The document with no layout decision made, as valid Rust.
    ///
    /// Its leaves in order, exactly as [`Doc::collect_leaves`] writes them, with
    /// two differences that are pure whitespace: a [`Doc::HardLine`] is a
    /// newline, and a newline closes any line comment a leaf left open before
    /// the next layout-owned byte (a break, a joiner, a delimiter) is written.
    /// It is the token sequence the SEAL certifies, and no byte the layout owns
    /// can land inside a comment, so it is valid Rust. One walk over the tree,
    /// linear in the document's bytes: the renderer's layout of last resort
    /// when the fit search runs out of fuel.
    pub fn plain_layout(&self) -> String {
        let mut out = String::new();
        self.write_leaves(&mut LeafSink::new(LeafMode::Plain, &mut out));
        out
    }

    /// Write every leaf in order through `sink`, which owns the break bytes.
    fn write_leaves(&self, sink: &mut LeafSink<'_>) {
        sink.steps = sink.steps.saturating_add(1);
        match self {
            Self::Text(s) => sink.leaf(s),
            Self::Line => sink.layout(" "),
            Self::HardLine => sink.hard_line(),
            // Invisible to the SEAL: the trailing comma it stands for is not a
            // token the legacy string emitter produces, so it must not appear in
            // the leaf sequence the SEAL compares.
            Self::Softline | Self::IfBroken(_) => sink.layout(""),
            Self::Concat(docs) => {
                for d in docs {
                    d.write_leaves(sink);
                }
            }
            Self::Nest(_, inner) | Self::Group(inner) => inner.write_leaves(sink),
            // The braces ARE part of the leaf sequence: the string emitter always
            // writes them, so they must appear in the SEAL comparison (unlike the
            // trailing comma above, which the string emitter never writes). A space
            // pads each brace so token adjacency survives normalization.
            Self::BraceBody(inner) => {
                sink.layout("{ ");
                inner.write_leaves(sink);
                sink.layout(" }");
            }
            // The trailing comma IS a leaf (the string emitter writes it after every
            // arm body); the synthesized braces are NOT (the string emitter never
            // writes them, so they stay invisible like `IfBroken`).
            Self::MatchArmTail { body, .. } => {
                body.write_leaves(sink);
                sink.layout(",");
            }
            // The break after `= ` is pure whitespace (SEAL-invisible, like a
            // `HardLine`): the string emitter writes `prefix rhs` with a single
            // separating space, which normalizes equal to either broken layout.
            Self::Assign { prefix, rhs, .. } => {
                prefix.write_leaves(sink);
                rhs.write_leaves(sink);
            }
            Self::Chain { operands } => {
                for (i, op) in operands.iter().enumerate() {
                    if let Some(o) = &op.leading_op {
                        if i > 0 {
                            sink.layout(" ");
                        }
                        sink.layout(o);
                        sink.layout(" ");
                    }
                    op.doc.write_leaves(sink);
                }
            }
            // The open/close delimiters and each element ARE leaves; elements are
            // separated by `, ` (the string emitter's `join(", ")`). The trailing
            // comma of the broken one-per-line form is SEAL-invisible (the string
            // emitter never writes it), exactly like the plain delimited group's.
            Self::CallArgs {
                open, elems, close, ..
            } => write_delimited(sink, open, elems, "", close),
            // Same accounting as `CallArgs`: `open`, each field joined by `, `, and
            // `close` are leaves; the trailing comma is SEAL-invisible. A space pads
            // the braces so `Name { a: 1 }` normalizes with the hugging spaces.
            Self::StructLit {
                open,
                fields,
                close,
            } => write_delimited(sink, open, fields, " ", close),
            // `Ptr<Head + T1 + T2 + …>` — the same token sequence the string emitter
            // writes for the flat annotation. The angle-break's trailing comma is
            // SEAL-invisible (the string emitter never writes it).
            Self::TypeBound {
                ptr_open,
                head,
                traits,
                close,
            } => {
                ptr_open.write_leaves(sink);
                head.write_leaves(sink);
                for t in traits {
                    sink.layout(" + ");
                    t.write_leaves(sink);
                }
                close.write_leaves(sink);
            }
            // The parens ARE leaves (the string emitter writes `({f})`), whether or
            // not the render drops them — the same rendered-vs-leaves divergence as
            // `IfBroken` / `BraceBody`.
            Self::ElidableParen { inner } => {
                sink.layout("(");
                inner.write_leaves(sink);
                sink.layout(")");
            }
            // The alternatives joined ` | ` — the string emitter's exact bytes.
            // The vertical layout's newlines are pure whitespace and its `| `
            // line leads normalize to the same ` | ` separators.
            Self::OrPattern { alts } => {
                for (i, alt) in alts.iter().enumerate() {
                    if i > 0 {
                        sink.layout(" | ");
                    }
                    sink.leaf(alt);
                }
            }
            // `receiver.method(…)` with no break — the string emitter writes them
            // adjacent, and the method-on-its-own-line layout is pure whitespace
            // (SEAL-invisible), so both layouts normalize to the same leaves.
            Self::MethodChain { receiver, method } => {
                receiver.write_leaves(sink);
                method.write_leaves(sink);
            }
            // The `(if … { … } else { … })` tokens the string emitter writes
            // adjacently. The block form's newlines are pure whitespace and its
            // brace tokens are identical, so both layouts normalize to the same
            // leaves — the branch bodies carry their own leaves in between.
            Self::IfElse { cond, then_, else_ } => {
                sink.layout("(if ");
                cond.write_leaves(sink);
                sink.layout(" { ");
                then_.write_leaves(sink);
                sink.layout(" } else { ");
                else_.write_leaves(sink);
                sink.layout(" })");
            }
        }
    }

    /// The whitespace-normalized leaf string: runs of whitespace collapsed to a
    /// single space, trimmed. Two documents with the same token sequence
    /// (ignoring layout) normalize equal — this is the SEAL comparison key.
    #[cfg(test)]
    pub fn normalized_leaves(&self) -> String {
        self.normalized_leaves_with_work().0
    }

    /// [`Doc::normalized_leaves`] with the work computing it took.
    ///
    /// The work is the nodes walked plus the raw bytes written, so a subtree of
    /// zero-width nodes is charged for its size, not only for its text.
    pub fn normalized_leaves_with_work(&self) -> (String, usize) {
        let mut raw = String::new();
        let mut sink = LeafSink::new(LeafMode::Seal, &mut raw);
        self.write_leaves(&mut sink);
        let steps = sink.steps;
        let work = steps.saturating_add(raw.len());
        (whitespace_normalize(&raw), work)
    }
}

/// Write `open`, each of `elems` joined by `, `, and `close` through `sink`, with
/// `pad` inside each delimiter.
fn write_delimited(sink: &mut LeafSink<'_>, open: &Doc, elems: &[Doc], pad: &str, close: &Doc) {
    open.write_leaves(sink);
    if !pad.is_empty() {
        sink.layout(pad);
    }
    for (i, e) in elems.iter().enumerate() {
        if i > 0 {
            sink.layout(", ");
        }
        e.write_leaves(sink);
    }
    if !pad.is_empty() {
        sink.layout(pad);
    }
    close.write_leaves(sink);
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

/// Which leaf stream a [`LeafSink`] writes.
#[derive(Clone, Copy)]
enum LeafMode {
    /// The SEAL oracle: every break is one space.
    Seal,
    /// The fallback layout: unconditional breaks are newlines, and no
    /// layout-owned byte is written inside a line comment.
    Plain,
}

/// The writer [`Doc::write_leaves`] streams through.
///
/// Leaf bytes pass through verbatim in both modes, so the token sequence never
/// depends on the mode. Every other byte is layout-owned and always sits on a
/// token boundary, so in [`LeafMode::Plain`] a newline may precede it: that is
/// how a line comment a leaf opened is closed before the layout writes code the
/// comment would swallow.
struct LeafSink<'a> {
    out: &'a mut String,
    mode: LeafMode,
    /// Whether the current output line may sit inside a `//` comment.
    ///
    /// Tracked only in [`LeafMode::Plain`]. Over-approximate (a `//` inside a
    /// string literal counts), which costs at most a harmless newline.
    comment_open: bool,
    /// The nodes walked so far.
    steps: usize,
}

impl<'a> LeafSink<'a> {
    const fn new(mode: LeafMode, out: &'a mut String) -> Self {
        Self {
            out,
            mode,
            comment_open: false,
            steps: 0,
        }
    }

    /// Write a leaf's bytes verbatim.
    fn leaf(&mut self, s: &str) {
        let spliced = self.out.ends_with('/') && s.starts_with('/');
        self.out.push_str(s);
        if matches!(self.mode, LeafMode::Plain) {
            self.comment_open = match s.rfind('\n') {
                Some(at) => s.get(at..).is_some_and(|tail| tail.contains("//")),
                None => self.comment_open || spliced || s.contains("//"),
            };
        }
    }

    /// Write a layout-owned separator, first ending any open line comment.
    fn layout(&mut self, s: &str) {
        if self.comment_open {
            self.out.push('\n');
            self.out.push_str(s.trim_start_matches(' '));
            self.comment_open = false;
        } else {
            self.out.push_str(s);
        }
    }

    /// Write an unconditional break.
    fn hard_line(&mut self) {
        match self.mode {
            LeafMode::Seal => self.out.push(' '),
            LeafMode::Plain => {
                self.out.push('\n');
                self.comment_open = false;
            }
        }
    }
}
