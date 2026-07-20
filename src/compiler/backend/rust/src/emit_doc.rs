//! The Doc-building emit path: a parallel to
//! [`crate::emit_expr::emit_expr_at`] that returns a [`Doc`] instead of a
//! `String`.
//!
//! Every builder carries EXACTLY the token sequence the string emitter produces,
//! so the whitespace-normalized leaf sequence of `build_doc(e)` equals the
//! whitespace-normalized string of `emit_expr_at(e)` — the SEAL.
//!
//! Structured (layout-bearing) arms, each byte-goldened against
//! `rustfmt --edition 2024 --style-edition 2024`:
//!   * the binary-operator chain (`BinOp` Add..Or) → [`Doc::Chain`];
//!   * `if`/`else` → inline or block form by an absolute width threshold;
//!   * the delimited lists — `Tuple`, plain non-empty non-`Ui` `List`, `Cons`,
//!     the plain user-function `Call` (`Callee::Func`, no turbofish pin), the
//!     saturated payload `Ctor` (user-enum, each cyclic-self-field argument boxed,
//!     or runtime-enum `IpeMaybe`/`IpeResult`/…), and the general function-value
//!     `Apply` tail (`({f})(args)`, non-lambda func, non-empty args) — which lay
//!     out flat when they fit and otherwise break one element per line with a
//!     break-conditional trailing comma ([`Doc::IfBroken`]);
//!   * the `let` block (`({ let name = value; body })`) — the one-statement block
//!     always breaks (a `HardLine` per line); its zero-statement multi-use inline
//!     substitution form (`({ inlined_body })`) is a soft group that flattens
//!     when it fits.
//!
//! Every remaining arm is carried as one [`Doc::owned`] leaf holding the string
//! emitter's exact bytes: the literals (Int/Float/Str/…), the call-shaped binops
//! (`Append`/`IntDiv`), the empty/`Ui` list and zero-arg call forms, and the
//! context-heavy arms not yet structured (kernel-dispatch / FFI / pinned calls,
//! the immediately-applied-lambda `Apply` block, the `Destructure` block,
//! lambdas, `match`, records, updates, field access, task sequencing). Carrying
//! bytes keeps the SEAL exact by
//! construction; those arms simply do not yet gain multi-line layout.
//!
//! The legacy string path in `emit_expr.rs` remains the emit default; this path
//! is exercised by the in-module SEAL + byte-golden tests only, so every commit
//! stays byte-green against the goldens until the native-emit cutover wires it in.

// Until the native-emit cutover wires these builders into project.rs, they are
// exercised by the in-module SEAL + byte-golden tests only.
#![allow(dead_code, reason = "consumed at the native-emit cutover")]

use std::borrow::Cow;

use ipe_diagnostics::{DResult, Diagnostic};
use ipe_intern::Symbol;
use ipe_ir::{BinOp, Callee, Expr, IrType, KernelFn, ModPath};

use crate::EmitCtx;
use crate::doc::{ChainOperand, Doc};
use crate::emit_expr::{
    call_has_kernel_special_case, callee_name, clone_targets_in_expr, emit_binding_stmts,
    emit_expr_at, expr_value_is_non_clone, free_vars, kernel_swaps_first_two, record_struct_name,
    scan_free_target, substitute_var,
};
use crate::emit_types::GenericScope;

/// The infix spelling of a chain-eligible operator (never `Append` / `IntDiv`,
/// which are call-shaped). Kept in step with `emit_expr::op_str`.
const fn chain_op_str(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Add => Some("+"),
        BinOp::Sub => Some("-"),
        BinOp::Mul => Some("*"),
        BinOp::Div => Some("/"),
        BinOp::Eq => Some("=="),
        BinOp::Neq => Some("!="),
        BinOp::Lt => Some("<"),
        BinOp::Gt => Some(">"),
        BinOp::Le => Some("<="),
        BinOp::Ge => Some(">="),
        BinOp::And => Some("&&"),
        BinOp::Or => Some("||"),
        // Call-shaped: never an infix chain operator.
        BinOp::Append | BinOp::IntDiv => None,
    }
}

/// Build a [`Doc`] for `expr`. Mirrors [`emit_expr_at`]'s arm structure; the
/// token leaves are byte-identical to the string emitter's output.
///
/// `indent`/`depth`/`generics` are threaded exactly as the string emitter
/// threads them, so a leaf that delegates to `emit_expr_at` sees the same
/// context. `depth` is the IR-nesting level of `expr` (0 at a function body),
/// matching `emit_expr_at`'s own `depth` argument.
pub fn build_doc(
    ctx: &EmitCtx,
    expr: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    let child = depth + 1;
    match expr {
        // A chain-eligible infix operator: flatten the maximal left-nested
        // same-operator run into a `Doc::Chain`, carrying every paren the string
        // emitter emits as a `Text` leaf.
        Expr::BinOp { op, lhs, rhs } if chain_op_str(*op).is_some() => {
            build_binop_chain(ctx, *op, lhs, rhs, indent, child, generics)
        }

        // A parenthesized `if`/`else` expression. rustfmt keeps the whole
        // construct on one line when the `if cond { then } else { else }` text
        // (WITHOUT the outer parens) is at most `single_line_if_else_max_width`
        // (50) columns wide — an absolute, column-independent threshold — and
        // otherwise breaks each branch body onto its own line in block form.
        Expr::If { cond, then_, else_ } => {
            build_if(ctx, cond, then_, else_, indent, child, generics)
        }

        // A tuple constructor `(e0, e1, …)`: a delimited group that breaks one
        // element per line with a trailing comma when it does not fit.
        Expr::Tuple(elems) => build_tuple(ctx, elems, indent, child, generics),

        // A plain non-empty, non-`Ui` list literal `vec![e0, e1, …]`: a delimited
        // group like the tuple. The empty-list forms and the `Ui`-annotated
        // wrapper carry no positional list layout, so they stay leaves.
        Expr::List { elem, items } if !items.is_empty() && !matches!(elem, IrType::Ui { .. }) => {
            build_list(ctx, items, indent, child, generics)
        }

        // `head :: tail`: the runtime cons call, a two-argument delimited group.
        Expr::Cons { head, tail } => build_cons(ctx, head, tail, indent, child, generics),

        // A constructor application `EnumName::Variant(a0, a1, …)`. A nullary
        // constructor (built-in-runtime or user) stays a leaf (`EnumName::Variant`
        // — no positional payload to break); a saturated payload constructor is a
        // delimited group over its argument docs, each cyclic-self-field argument
        // wrapped in `Box::new(..)` exactly as the string emitter wraps it.
        Expr::Ctor {
            home,
            ty,
            variant,
            args,
        } if !args.is_empty() => {
            build_ctor(ctx, home, *ty, *variant, args, indent, child, generics)
        }

        // A function call whose emitted form is the generic fall-through tail
        // `{name}{turbofish}({args})` — the exact shape
        // [`crate::emit_expr::emit_expr_at`] produces once every kernel special
        // case has been ruled out. Structured for any non-empty-arg call
        // (`Callee::Func` / `Ffi` / a kernel with no bespoke wrapping): the plain
        // user call, the FFI wrapper, the turbofish-pinned kernel, and the
        // container-swapping kernel all lower to `{name}{turbofish}(` + a delimited
        // argument list. The `call_has_kernel_special_case` predicate re-runs the
        // ~8 probe helpers + the `Dict.get` clone case; when ANY applies the call
        // stays a byte-carried leaf (its bespoke wrapping is not structured). A
        // zero-arg call also stays a leaf (no positional list to break).
        Expr::Call {
            callee,
            args,
            pin,
            on_form,
        } if !args.is_empty()
            && !call_has_kernel_special_case(
                ctx, callee, args, *on_form, indent, child, generics,
            )? =>
        {
            build_generic_call(ctx, callee, args, *pin, indent, child, generics)
        }

        // A general function-value application `({f})(a0, a1, …)`. Structured ONLY
        // for the non-lambda, non-empty-arg tail: the string emitter's own
        // immediately-applied-lambda branch (`func` is a `Lambda`) rewrites to a
        // `({ let p = a; … body })` BLOCK — that block form is P2 work — so it stays
        // a leaf; a zero-arg apply (`({f})()`) has no positional list, also a leaf.
        // The remaining tail is exactly `({f})(` + a delimited argument list; `f`
        // is built recursively so a structured func operand rides inside its parens.
        Expr::Apply { func, args }
            if !matches!(func.as_ref(), Expr::Lambda { .. }) && !args.is_empty() =>
        {
            let func_doc = build_doc(ctx, func, indent, child, generics)?;
            let docs = build_args(ctx, args, indent, child, generics)?;
            Ok(delimited(
                Doc::concat(vec![Doc::text("("), func_doc, Doc::text(")(")]),
                docs,
                Doc::text(")"),
            ))
        }

        // A `let` expression `({ let name = value; body })`. The one-statement
        // block ALWAYS breaks (rustfmt never inlines a block that holds a `let`):
        // `({`, then the `let` statement and the body each on their own line at
        // one indent step, then `})` dedented back. The multi-use non-clone
        // inline-substitution path (a ZERO-statement block `({ body })`) lays out
        // as a soft group so it flattens when it fits — mirroring
        // [`crate::emit_expr::emit_expr_at`]'s `Expr::Let` arm decision exactly.
        Expr::Let { name, value, body } => {
            build_let(ctx, *name, value, body, indent, child, generics)
        }

        // A `Destructure` block `({ <binding stmts> <body> })`. Like the `let`
        // block, but its binder may expand to MULTIPLE `let` statements
        // (`emit_binding_stmts`: one flat `let <pat> = <value>;` for an alias-free
        // binder, or the clone-split sequence for an aliased one). Each statement
        // and the body go on their own `HardLine`, so the block always breaks —
        // matching `rustfmt`, which never inlines a block that holds a statement.
        Expr::Destructure {
            binder,
            value,
            body,
        } => build_destructure(ctx, binder, value, body, indent, child, generics),

        // A non-empty record literal `StructName { f0: v0, f1: v1 }`: a delimited
        // group over its `field: value` parts that breaks one field per line with a
        // trailing comma when it does not fit, matching rustfmt's struct-literal
        // layout. The struct name is resolved by
        // [`crate::emit_expr::record_struct_name`] (shared with the string
        // emitter). An empty record stays a leaf — rustfmt renders `StructName {}`
        // with no inner break, which the delimited builder is not shaped for.
        Expr::Record(fields) if !fields.is_empty() => {
            build_record(ctx, fields, indent, child, generics)
        }

        // A functional record update `{ let mut __ipe_rec = (record).clone();
        // __ipe_rec.f = v; … __ipe_rec }`: a statement block that ALWAYS breaks
        // (it holds statements), each `let`/assignment/tail on its own `HardLine`
        // inside the sole `Nest(4)`, matching rustfmt. The base and each field
        // value are built recursively.
        Expr::Update { record, fields } => {
            build_update(ctx, record, fields, indent, child, generics)
        }

        // The sync task-sequencing block `{ let _ = task_run(<effect>); <rest> }`.
        // A statement block (it holds the discard `let`), so it ALWAYS breaks: the
        // `let _ = task_run(<effect>);` statement and the `<rest>` tail each on their
        // own `HardLine` inside the block's sole `Nest(4)`, matching rustfmt. The
        // effect gets the identical IR-level clone-capture rewrite the string
        // emitter applies (`clone_targets_in_expr` over `rest`'s `free_vars`); both
        // effect and rest are built recursively.
        Expr::TaskSeqSync { effect, rest } => {
            build_task_seq_sync(ctx, effect, rest, indent, child, generics)
        }

        // Every remaining arm carries the string emitter's exact bytes as one
        // leaf. Its layout is whatever single-line pre-`rustfmt` form the string
        // emitter produces; structuring the rest (call-arg lists, block-form
        // `let`/destructure, match, lambda, record/update) is the remaining P2
        // work. Carrying bytes keeps the SEAL exact by construction.
        _ => leaf(ctx, expr, indent, depth, generics),
    }
}

/// Emit `expr` through the legacy string emitter and wrap its bytes as one
/// [`Doc::owned`] leaf. Preserves the exact token sequence for any arm not yet
/// structured.
fn leaf(
    ctx: &EmitCtx,
    expr: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    Ok(Doc::owned(emit_expr_at(
        ctx, expr, indent, depth, generics,
    )?))
}

/// Build the `Doc::Chain` for a chain-eligible binop. Walks the left-nested
/// same-operator run (rustfmt groups a left-associative operator run as one
/// chain) into a flat operand list, carrying every wrapping paren.
///
/// The string emitter emits `({l} {op} {r})` recursively, so a run
/// `((a + b) + c)` has the IR shape `Add(Add(a, b), c)` and the string form
/// `((a + b) + c)`. Flattening peels the left spine: the innermost left operand
/// is prefixed by one `(` per spine level, and each right operand is suffixed by
/// one `)`. Operand docs are built recursively, so a higher-precedence sub-expr
/// (or a different operator) stays one atomic operand — its own parens ride
/// along inside its doc.
fn build_binop_chain(
    ctx: &EmitCtx,
    op: BinOp,
    lhs: &Expr,
    rhs: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    let opstr = chain_op_str(op).unwrap_or("");

    // Walk down the left spine while it is the SAME operator, collecting the
    // right operands. `spine` ends up outermost..innermost; reverse to source
    // order. `left` is the innermost left operand.
    let mut left = lhs;
    let mut spine: Vec<&Expr> = vec![rhs];
    while let Expr::BinOp {
        op: inner_op,
        lhs: inner_lhs,
        rhs: inner_rhs,
    } = left
    {
        if *inner_op != op {
            break;
        }
        spine.push(inner_rhs);
        left = inner_lhs;
    }
    spine.reverse();

    let depth_count = spine.len(); // one wrapping `(` per chain level
    let mut operands: Vec<ChainOperand> = Vec::with_capacity(depth_count + 1);

    // First operand: `(` * depth_count, then the innermost left operand's doc.
    let mut first = Vec::with_capacity(depth_count + 1);
    for _ in 0..depth_count {
        first.push(Doc::text("("));
    }
    first.push(build_doc(ctx, left, indent, depth, generics)?);
    operands.push(ChainOperand {
        leading_op: None,
        doc: Doc::concat(first),
    });

    // Each subsequent operand: its own doc followed by ONE `)`.
    for rhs_expr in spine {
        let rhs_doc = build_doc(ctx, rhs_expr, indent, depth, generics)?;
        operands.push(ChainOperand {
            leading_op: Some(Cow::Borrowed(opstr)),
            doc: Doc::concat(vec![rhs_doc, Doc::text(")")]),
        });
    }

    Ok(Doc::Chain { operands })
}

/// `rustfmt`'s `single_line_if_else_max_width` (default 50): the maximum width of
/// an `if cond { then } else { else }` construct — measured WITHOUT the outer
/// parentheses the emitter wraps it in — that `rustfmt` keeps on one line. Wider
/// constructs break each branch body onto its own line. The threshold is absolute
/// (column-independent), so the decision is made here at build time from the flat
/// leaf widths.
const SINGLE_LINE_IF_ELSE_MAX_WIDTH: usize = 50;

/// Build the `Doc` for a parenthesized `if`/`else`. The string emitter produces
/// `(if {cond} {{ {then} }} else {{ {else} }})`; this builder carries the exact
/// same tokens but, when the un-parenthesized construct exceeds
/// [`SINGLE_LINE_IF_ELSE_MAX_WIDTH`], lays the branch bodies out in broken block
/// form (`HardLine`-separated), matching `rustfmt`.
///
/// The branch bodies are built recursively so a nested chain / `if` inside a
/// branch structures too; a branch's own leaves carry its exact tokens.
fn build_if(
    ctx: &EmitCtx,
    cond: &Expr,
    then_: &Expr,
    else_: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    // The branch bodies' visual indentation comes entirely from the renderer's
    // `Nest(4)` wrappers below; the builder `indent` is threaded unchanged so a
    // single-line branch leaf carries no embedded indentation that would
    // double-count against the `Nest`.
    let cond_doc = build_doc(ctx, cond, indent, depth, generics)?;
    let then_doc = build_doc(ctx, then_, indent, depth, generics)?;
    let else_doc = build_doc(ctx, else_, indent, depth, generics)?;

    // The single-line construct width, WITHOUT the outer parens:
    // `if ` + cond + ` { ` + then + ` } else { ` + else + ` }`.
    let cond_flat = cond_doc.normalized_leaves();
    let then_flat = then_doc.normalized_leaves();
    let else_flat = else_doc.normalized_leaves();
    let construct_width = "if ".len()
        + cond_flat.len()
        + " { ".len()
        + then_flat.len()
        + " } else { ".len()
        + else_flat.len()
        + " }".len();

    if construct_width <= SINGLE_LINE_IF_ELSE_MAX_WIDTH {
        // Inline: `(if cond { then } else { else })`. Soft `Line`s so a wider
        // enclosing group could in principle break it, but the width test already
        // guaranteed it fits.
        return Ok(Doc::concat(vec![
            Doc::text("(if "),
            cond_doc,
            Doc::text(" { "),
            then_doc,
            Doc::text(" } else { "),
            else_doc,
            Doc::text(" })"),
        ]));
    }

    // Broken block form. Braces sit at the enclosing block `indent`; each branch
    // body indents to `indent + 4`. `HardLine`s force the layout unconditionally,
    // matching `rustfmt`'s block-form `if` once past the single-line threshold.
    Ok(Doc::concat(vec![
        Doc::text("(if "),
        cond_doc,
        Doc::text(" {"),
        Doc::nest(4, Doc::concat(vec![Doc::HardLine, then_doc])),
        Doc::HardLine,
        Doc::text("} else {"),
        Doc::nest(4, Doc::concat(vec![Doc::HardLine, else_doc])),
        Doc::HardLine,
        Doc::text("})"),
    ]))
}

/// Build a `rustfmt`-canonical delimited list: `{open}{a0}, {a1}, …{close}` when
/// it fits the width, or the broken form with one element per line at one indent
/// step, a break-conditional trailing comma, and the closing delimiter dedented
/// back to the group's start column:
///
/// ```text
/// {open}
///     {a0},
///     {a1},
/// {close}
/// ```
///
/// `open` already includes the opening delimiter (`f(`, `(`, `vec![`); `close` is
/// the matching closer (`)`, `]`). The inter-element separator is a real `,`
/// (which the string emitter emits via `join(", ")`) followed by a soft `Line`
/// (a space when flat, a newline+indent when broken). The TRAILING comma is a
/// [`Doc::IfBroken`] — invisible to the SEAL (the string emitter never emits it)
/// and rendered only in the broken arm, matching `rustfmt`.
///
/// An empty element list is not delimited here (callers handle the zero-arg form
/// as a plain leaf, matching the string emitter's `name()` / `()` output).
///
/// The inner boundary (the break candidate right after `open` and right before
/// `close`) is a zero-width [`Doc::Softline`]: a bracketed list is `(a, b)` /
/// `[a, b]` / `f(a, b)` FLAT — no space hugging the delimiter. For a brace-
/// delimited struct literal, whose flat form hugs the braces WITH a space
/// (`Name { a: 1 }`), use [`delimited_spaced`].
fn delimited(open: Doc, elems: Vec<Doc>, close: Doc) -> Doc {
    delimited_with(open, elems, close, || Doc::Softline)
}

/// Like [`delimited`], but the inner boundary is a soft [`Doc::Line`] (a SPACE
/// when flat, a newline+indent when broken). This is the struct-literal shape:
/// `Name { a: 1, b: 2 }` flat (spaces inside the braces), and one field per line
/// with a trailing comma when broken — exactly `rustfmt`'s struct-literal layout.
fn delimited_spaced(open: Doc, elems: Vec<Doc>, close: Doc) -> Doc {
    delimited_with(open, elems, close, || Doc::Line)
}

/// The shared delimited-group core. `boundary` supplies the break candidate that
/// hugs the open and close delimiters — [`Doc::Softline`] for bracketed lists
/// (no flat space), [`Doc::Line`] for brace-delimited struct literals (a flat
/// space). The inter-element separator is always a real `,` then a soft `Line`;
/// the trailing comma is always a SEAL-invisible [`Doc::IfBroken`].
fn delimited_with(open: Doc, elems: Vec<Doc>, close: Doc, boundary: impl Fn() -> Doc) -> Doc {
    let mut inner = vec![open];
    let mut nested = Vec::with_capacity(elems.len() * 2);
    let last = elems.len().saturating_sub(1);
    for (i, e) in elems.into_iter().enumerate() {
        if i == 0 {
            nested.push(boundary());
        } else {
            nested.push(Doc::text(","));
            nested.push(Doc::Line);
        }
        nested.push(e);
        if i == last {
            // Trailing comma only when the group breaks; absent (and SEAL-
            // invisible) when flat.
            nested.push(Doc::if_broken(","));
        }
    }
    inner.push(Doc::nest(4, Doc::concat(nested)));
    inner.push(boundary());
    inner.push(close);
    Doc::group(Doc::concat(inner))
}

/// Build the arg docs for a positional argument list, each at the child depth the
/// string emitter uses. Mirrors the `for arg in args { emit_expr_at(child) }`
/// loops in the string emitter's call / ctor / tuple / list arms.
fn build_args(
    ctx: &EmitCtx,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Vec<Doc>> {
    let mut docs = Vec::with_capacity(args.len());
    for arg in args {
        docs.push(build_doc(ctx, arg, indent, child, generics)?);
    }
    Ok(docs)
}

/// Build the `Doc` for a `Tuple`. The string emitter renders `({parts.join(", ")})`
/// with each part at the child depth; this builder carries the identical tokens in
/// a delimited group so a wide tuple breaks one element per line with a trailing
/// comma, matching `rustfmt`.
fn build_tuple(
    ctx: &EmitCtx,
    elems: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    let docs = build_args(ctx, elems, indent, child, generics)?;
    Ok(delimited(Doc::text("("), docs, Doc::text(")")))
}

/// Build the `Doc` for a plain non-empty list literal `vec![e0, e1, …]`. Only the
/// plain path is structured here; the empty-list forms (`Vec::<T>::new()` /
/// `Vec::new()`) and the `Ui`-annotated `{ let __ipe_m: … = vec![…]; __ipe_m }`
/// wrapper carry no positional delimited layout, so those cases stay leaves (the
/// caller checks and delegates). Elements are built at the child depth.
fn build_list(
    ctx: &EmitCtx,
    items: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    let docs = build_args(ctx, items, indent, child, generics)?;
    Ok(delimited(Doc::text("vec!["), docs, Doc::text("]")))
}

/// Build the `Doc` for `head :: tail`. The string emitter renders the runtime
/// call `ipe_runtime::list::ipe_list_cons({h}, {t})`; this builder carries the
/// identical tokens in a two-element delimited group so a wide cons breaks its two
/// arguments one per line with a trailing comma, matching `rustfmt`.
fn build_cons(
    ctx: &EmitCtx,
    head: &Expr,
    tail: &Expr,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    let head_doc = build_doc(ctx, head, indent, child, generics)?;
    let tail_doc = build_doc(ctx, tail, indent, child, generics)?;
    Ok(delimited(
        Doc::text("ipe_runtime::list::ipe_list_cons("),
        vec![head_doc, tail_doc],
        Doc::text(")"),
    ))
}

/// Build the `Doc` for the generic call tail `{name}{turbofish}({args})`,
/// mirroring the fall-through of [`crate::emit_expr::emit_expr_at`]'s `Expr::Call`
/// arm token-for-token. The caller has already proved no kernel special case
/// applies (via [`crate::emit_expr::call_has_kernel_special_case`]) and that the
/// argument list is non-empty, so the emitted form is exactly the callee name,
/// then the pin's turbofish (with the `Ipe.Csv` parse `::<IpeError>` override the
/// string emitter applies), then a delimited argument list — with the two
/// arguments pre-swapped for the container-first `Maybe`/`Result`/… kernels that
/// [`crate::emit_expr::kernel_swaps_first_two`] flags, exactly as the string
/// emitter reverses `parts` before joining.
fn build_generic_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    pin: ipe_ir::CallPin,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    let name = callee_name(ctx, callee)?;
    // The pin's turbofish, with the CsvParse error-channel anchor the string
    // emitter substitutes when the pin is empty (`csv_parse::<IpeError>(…)`).
    let pin_turbofish = pin.turbofish();
    let turbofish: &str = if pin_turbofish.is_empty()
        && matches!(
            callee,
            Callee::Kernel(KernelFn::CsvParse | KernelFn::CsvParseWithDelimiter)
        ) {
        "::<IpeError>"
    } else {
        pin_turbofish
    };
    let mut docs = build_args(ctx, args, indent, child, generics)?;
    // Container-first kernels take their two arguments in the opposite order to
    // the Ipê call; the string emitter reverses the rendered `parts`, so the Doc
    // builder reverses the built arg docs to carry the identical token sequence.
    if matches!(callee, Callee::Kernel(k) if kernel_swaps_first_two(*k)) {
        docs.reverse();
    }
    Ok(delimited(
        Doc::owned(format!("{name}{turbofish}(")),
        docs,
        Doc::text(")"),
    ))
}

/// Build the `Doc` for a saturated payload constructor. Mirrors
/// [`crate::emit_expr::emit_ctor`] token-for-token: the runtime-enum branch
/// (`IpeMaybe::Just(..)`, `IpeResult::Err(..)`, …) and the user-enum branch
/// (`EnumName::Variant(..)`) both build the prefix path, then lay the payload out
/// as a delimited group. Each user-enum argument on a type-size cycle back to its
/// own enum is wrapped in `Box::new(..)` — the exact wrap the string emitter
/// applies (runtime-enum payloads are never self-recursive, so that branch never
/// boxes). The nullary forms never reach here (the caller keeps them leaves).
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors emit_ctor's (home, ty) split"
)]
fn build_ctor(
    ctx: &EmitCtx,
    home: &ModPath,
    ty: Symbol,
    variant: Symbol,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    // A built-in `Maybe` / `Result` / `Order` / `ChunkEvent` constructor routes to
    // the runtime enum; its payload is never a self-recursive user field, so no
    // field-boxing lookup applies (matching the string emitter).
    if let Some(runtime) = ctx.builtin_runtime_enum(home, ty) {
        let path = format!("{runtime}::{}", ctx.emit_ident(variant)?);
        let docs = build_args(ctx, args, indent, child, generics)?;
        return Ok(delimited(
            Doc::owned(format!("{path}(")),
            docs,
            Doc::text(")"),
        ));
    }

    let path = format!("{}::{}", ctx.enum_name(home, ty)?, ctx.emit_ident(variant)?);
    let fields = ctx.variant_fields(home, ty, variant)?;
    if fields.len() != args.len() {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::build_ctor",
            detail: format!(
                "constructor {} of enum {} applied to {} args but declares {} fields; \
                 a constructor application must be saturated",
                variant.as_raw(),
                ty.as_raw(),
                args.len(),
                fields.len()
            ),
        });
    }
    let mut docs = Vec::with_capacity(args.len());
    for (arg, field_ty) in args.iter().zip(fields.iter()) {
        let arg_doc = build_doc(ctx, arg, indent, child, generics)?;
        // A cyclic self-edge field is boxed in the enum, so its construction
        // argument is boxed too: `Box::new(<arg>)`. The `Box::new(` prefix and the
        // matching `)` are carried as leaves so the SEAL sees the exact tokens.
        if ctx.is_cyclic_self_field(field_ty, home, ty) {
            docs.push(Doc::concat(vec![
                Doc::text("Box::new("),
                arg_doc,
                Doc::text(")"),
            ]));
        } else {
            docs.push(arg_doc);
        }
    }
    Ok(delimited(
        Doc::owned(format!("{path}(")),
        docs,
        Doc::text(")"),
    ))
}

/// Build the `Doc` for a `let` expression, mirroring
/// [`crate::emit_expr::emit_expr_at`]'s `Expr::Let` arm token-for-token.
///
/// Two shapes, chosen by the identical `needs_inline` predicate the string
/// emitter uses (multi-use of a non-`Clone` value, no `CloneVar` capture site):
///
///   * inline-substitution path — a ZERO-statement block `({ inlined_body })`.
///     Laid out as a soft group (`Line`s) so it flattens when it fits and breaks
///     the body onto its own line otherwise, matching rustfmt's zero-statement
///     block.
///   * normal path — a ONE-statement block `({ let name = value; body })`. rustfmt
///     never inlines a block that holds a statement, so this ALWAYS breaks: `({`,
///     the `let` statement and the body each on their own line at one indent step
///     (the renderer's sole `Nest(4)`), then `})` dedented back. `HardLine`s force
///     the break unconditionally.
///
/// `value` and `body` are built recursively; their leaves carry the string
/// emitter's single-line bytes, so no leaf embeds a newline+indent that would
/// double-count against the block's `Nest(4)` (the sole indent source).
fn build_let(
    ctx: &EmitCtx,
    name: Symbol,
    value: &Expr,
    body: &Expr,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    let (occurrences, has_clonevar) = scan_free_target(body, name);
    let needs_inline = occurrences > 1 && expr_value_is_non_clone(value) && !has_clonevar;
    if needs_inline {
        // Zero-statement block `({ inlined_body })`: the body with `name`
        // substituted by `value`, laid out as a soft group.
        let inlined_body = substitute_var(body.clone(), name, value);
        let body_doc = build_doc(ctx, &inlined_body, indent, child, generics)?;
        return Ok(Doc::group(Doc::concat(vec![
            Doc::text("({"),
            Doc::nest(4, Doc::concat(vec![Doc::Line, body_doc])),
            Doc::Line,
            Doc::text("})"),
        ])));
    }
    let name_s = ctx.emit_ident(name)?;
    let value_doc = build_doc(ctx, value, indent, child, generics)?;
    let body_doc = build_doc(ctx, body, indent, child, generics)?;
    // One-statement block: `let name = value;` then `body`, each on its own line.
    Ok(Doc::concat(vec![
        Doc::text("({"),
        Doc::nest(
            4,
            Doc::concat(vec![
                Doc::HardLine,
                Doc::owned(format!("let {name_s} = ")),
                value_doc,
                Doc::text(";"),
                Doc::HardLine,
                body_doc,
            ]),
        ),
        Doc::HardLine,
        Doc::text("})"),
    ]))
}

/// Build the `Doc` for a `Destructure` block, mirroring
/// [`crate::emit_expr::emit_expr_at`]'s `Expr::Destructure` arm token-for-token.
///
/// The string emitter renders `({ <binding stmts> <body> })` where the binding
/// statements come from [`crate::emit_expr::emit_binding_stmts`]: a SINGLE flat
/// `let <pat> = <value>;` for an alias-free binder, or the clone-split
/// `let name = <value>; let (a, b) = name.clone();` sequence for an aliased one —
/// one or MORE `let` statements. Each statement is carried as its own leaf on its
/// own `HardLine`, and the body (built recursively) follows on a further
/// `HardLine`, all inside the block's sole `Nest(4)`. Like the one-statement
/// `let` block, this always breaks — `rustfmt` never inlines a block that holds a
/// statement.
///
/// The value is rendered through the same `emit_expr_at` call the string emitter
/// uses (`emit_binding_stmts` interpolates the value string into each statement),
/// so the statement leaves carry the string emitter's exact tokens and the SEAL
/// holds by construction. The body is the only structured child.
fn build_destructure(
    ctx: &EmitCtx,
    binder: &ipe_ir::Pat,
    value: &Expr,
    body: &Expr,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    let value_s = emit_expr_at(ctx, value, indent, child, generics)?;
    let stmts = emit_binding_stmts(ctx, binder, &value_s)?;
    let body_doc = build_doc(ctx, body, indent, child, generics)?;
    // Each binding statement on its own `HardLine`, then the body on a further
    // `HardLine`, all at one indent step (the renderer's sole `Nest(4)`).
    let mut inner = Vec::with_capacity(stmts.len() * 2 + 2);
    for stmt in stmts {
        inner.push(Doc::HardLine);
        inner.push(Doc::owned(stmt));
    }
    inner.push(Doc::HardLine);
    inner.push(body_doc);
    Ok(Doc::concat(vec![
        Doc::text("({"),
        Doc::nest(4, Doc::concat(inner)),
        Doc::HardLine,
        Doc::text("})"),
    ]))
}

/// Build the `Doc` for a non-empty record literal, mirroring
/// [`crate::emit_expr::emit_record`] token-for-token. The struct name comes from
/// the shared [`crate::emit_expr::record_struct_name`] resolver; each field
/// renders `{field_ident}: {value}` with the value built recursively, laid out as
/// a delimited group so a wide record breaks one field per line with a trailing
/// comma (rustfmt's struct-literal layout). A `ServerResponse`-shaped literal
/// gains the trailing `cookies: Vec::new()` field the string emitter appends, so
/// the two produce the identical field sequence.
fn build_record(
    ctx: &EmitCtx,
    fields: &[(Symbol, Expr)],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    let (struct_name, is_server_response) = record_struct_name(ctx, fields)?;
    let mut field_docs = Vec::with_capacity(fields.len() + usize::from(is_server_response));
    for (sym, value) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        let value_doc = build_doc(ctx, value, indent, child, generics)?;
        field_docs.push(Doc::concat(vec![
            Doc::owned(format!("{field_ident}: ")),
            value_doc,
        ]));
    }
    if is_server_response {
        // The runtime struct's multi-`Set-Cookie` field is not part of the Ipê
        // record alias; the string emitter defaults it, so carry the same leaf.
        field_docs.push(Doc::text("cookies: Vec::new()"));
    }
    Ok(delimited_spaced(
        Doc::owned(format!("{struct_name} {{")),
        field_docs,
        Doc::text("}"),
    ))
}

/// Build the `Doc` for a functional record update, mirroring
/// [`crate::emit_expr::emit_update`] token-for-token. The string emitter renders
/// `{ let mut __ipe_rec = (<record>).clone(); __ipe_rec.f = v; … __ipe_rec }` — a
/// clone-and-reassign statement block. Since it holds statements, it ALWAYS
/// breaks: the `let mut` binding, each field assignment, and the `__ipe_rec` tail
/// each land on their own `HardLine` inside the block's sole `Nest(4)`, matching
/// rustfmt. The base record and each field value are built recursively; their
/// leaves carry the string emitter's exact tokens, so the SEAL holds.
fn build_update(
    ctx: &EmitCtx,
    record: &Expr,
    fields: &[(Symbol, Expr)],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    let base_doc = build_doc(ctx, record, indent, child, generics)?;
    let mut inner = vec![
        Doc::HardLine,
        Doc::text("let mut __ipe_rec = ("),
        base_doc,
        Doc::text(").clone();"),
    ];
    for (sym, value) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        let value_doc = build_doc(ctx, value, indent, child, generics)?;
        inner.push(Doc::HardLine);
        inner.push(Doc::owned(format!("__ipe_rec.{field_ident} = ")));
        inner.push(value_doc);
        inner.push(Doc::text(";"));
    }
    inner.push(Doc::HardLine);
    inner.push(Doc::text("__ipe_rec"));
    Ok(Doc::concat(vec![
        Doc::text("{"),
        Doc::nest(4, Doc::concat(inner)),
        Doc::HardLine,
        Doc::text("}"),
    ]))
}

/// Build the `Doc` for the sync task-sequencing block, mirroring
/// [`crate::emit_expr::emit_expr_at`]'s `Expr::TaskSeqSync` arm token-for-token.
///
/// The string emitter renders `{ let _ = task_run(<effect>); <rest> }` — a
/// statement block that discards the blocking task result, then evaluates `rest`
/// in the same sync scope. Since it holds the discard `let`, it ALWAYS breaks:
/// the `let _ = task_run(` prefix + the effect doc + `);` land on one `HardLine`,
/// and `rest` on a further `HardLine`, all inside the block's sole `Nest(4)` —
/// matching rustfmt, which never inlines a block that holds a statement.
///
/// Before emitting, the effect gets the identical IR-level clone-capture rewrite
/// the string emitter applies: any identifier `rest` reads next but `effect`'s
/// own left-to-right evaluation would move is rewritten to a `CloneVar`
/// (`clone_targets_in_expr` over `rest`'s `free_vars`). Both effect and rest are
/// built recursively; their leaves carry the string emitter's exact tokens, so
/// the SEAL holds.
fn build_task_seq_sync(
    ctx: &EmitCtx,
    effect: &Expr,
    rest: &Expr,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Doc> {
    let rest_captures = free_vars(rest);
    let effect_rw = clone_targets_in_expr(effect.clone(), &rest_captures);
    let effect_doc = build_doc(ctx, &effect_rw, indent, child, generics)?;
    let rest_doc = build_doc(ctx, rest, indent, child, generics)?;
    Ok(Doc::concat(vec![
        Doc::text("{"),
        Doc::nest(
            4,
            Doc::concat(vec![
                Doc::HardLine,
                Doc::text("let _ = task_run("),
                effect_doc,
                Doc::text(");"),
                Doc::HardLine,
                rest_doc,
            ]),
        ),
        Doc::HardLine,
        Doc::text("}"),
    ]))
}

#[cfg(test)]
#[allow(clippy::panic, clippy::expect_used, reason = "test assertions")]
mod tests {
    //! P1 acceptance: the SEAL leaf-sequence property (every builder carries the
    //! string emitter's exact tokens) and per-builder byte-goldens through the
    //! renderer. The goldens are captured from `rustfmt --edition 2024
    //! --style-edition 2024`; the renderer is fixed to match, never the reverse.

    use std::borrow::Cow;

    use ipe_intern::Interner;
    use ipe_ir::{
        BinOp, CallPin, Callee, EnumDef, Expr, Func, FuncId, IrType, KernelFn, ModPath, Module,
        OnFormKind, Pat, Program, TypeDef, Variant,
    };

    use super::build_doc;
    use crate::doc::{ChainOperand, Doc, whitespace_normalize};
    use crate::emit_expr::emit_expr_at;
    use crate::emit_types::GenericScope;
    use crate::render::{RenderConfig, render};
    use crate::{DbDriver, EmitCtx};

    /// A minimal one-module `Program` whose interner carries a handful of value
    /// identifiers (`a`, `b`, `c`, `x`) so `Expr::Var` fixtures resolve. The
    /// module declares one nullary enum so `EmitCtx::build` has a type to key on.
    struct Fixture {
        interner: Interner,
        program: Program,
        syms: Vec<ipe_intern::Symbol>,
        /// `Main` module path — the `home` of the fixture enum, for `Ctor` fixtures.
        main_mod: ipe_intern::Symbol,
        /// The `Msg` enum type symbol, for `Ctor` fixtures.
        msg_ty: ipe_intern::Symbol,
        /// The `Wrap` payload-variant symbol (`Wrap(Int)`), for `Ctor` fixtures.
        wrap_ctor: ipe_intern::Symbol,
        /// The `Triple` three-field-variant symbol (`Triple(Int, Int, Int)`), for
        /// the wide (breaking) `Ctor` byte-golden.
        triple_ctor: ipe_intern::Symbol,
        /// The `Unit` nullary-variant symbol, for the nullary `Ctor` fixture.
        unit_ctor: ipe_intern::Symbol,
        /// The built-in `Maybe` type symbol (no `EnumDef`), for the runtime-enum
        /// `Ctor` fixture.
        maybe_ty: ipe_intern::Symbol,
        /// The built-in `Just` variant symbol, for the runtime-enum `Ctor` fixture.
        just_ctor: ipe_intern::Symbol,
    }

    fn fixture() -> Fixture {
        let mut interner = Interner::new();
        let main_mod = interner.intern("Main").expect("intern Main");
        let msg_ty = interner.intern("Msg").expect("intern Msg");
        let unit_ctor = interner.intern("Unit").expect("intern Unit");
        // A one-field variant so a saturated payload-`Ctor` fixture resolves its
        // field types (`Msg::Wrap(Int)`).
        let wrap_ctor = interner.intern("Wrap").expect("intern Wrap");
        // A three-field variant for the wide (breaking) `Ctor` byte-golden
        // (`Msg::Triple(Int, Int, Int)`).
        let triple_ctor = interner.intern("Triple").expect("intern Triple");
        // Built-in `Maybe`/`Just` symbols: NO `EnumDef` is injected for them, so
        // `builtin_runtime_enum` routes their constructor to `IpeMaybe`.
        let maybe_ty = interner.intern("Maybe").expect("intern Maybe");
        let just_ctor = interner.intern("Just").expect("intern Just");
        // A zero-arg helper function so a `Callee::Func(FuncId 0)` call fixture
        // resolves a Rust name; its body is never emitted by these builder tests.
        let helper_fn = interner.intern("helper").expect("intern helper");
        // The two record field names (`a`, `b`) — interning is idempotent, so these
        // are the same symbols as `syms[0]`/`syms[1]` a `Record` fixture uses.
        let rec_field_a = interner.intern("a").expect("intern a");
        let rec_field_b = interner.intern("b").expect("intern b");
        let syms = [
            "a",
            "b",
            "c",
            "x",
            // Wide identifiers for the broken-`if` byte-golden: their combined
            // width pushes the `if cond { then } else { else }` construct past the
            // single-line threshold so the builder emits block form.
            "some_condition_variable",
            "first_branch_value",
            "second_branch_value_here",
            // Wide identifiers for the broken tuple / list / cons byte-goldens:
            // three of these in a delimited list overflow width 100.
            "argument_that_is_quite_long_enough_to_matter_x",
            "argument_that_is_quite_long_enough_to_matter_y",
            "argument_that_is_quite_long_enough_to_matter_z",
        ]
        .iter()
        .map(|n| interner.intern(n).expect("intern var"))
        .collect::<Vec<_>>();
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    name: msg_ty,
                    home: ModPath(vec![main_mod]),
                    type_params: vec![],
                    variants: vec![
                        Variant {
                            name: unit_ctor,
                            fields: vec![],
                        },
                        Variant {
                            name: wrap_ctor,
                            fields: vec![IrType::Int],
                        },
                        Variant {
                            name: triple_ctor,
                            fields: vec![IrType::Int, IrType::Int, IrType::Int],
                        },
                    ],
                })],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: helper_fn,
                    home: ModPath(vec![main_mod]),
                    type_params: vec![],
                    params: vec![],
                    ret: IrType::Int,
                    body: Expr::Int(0),
                }],
                entry: None,
                // A two-field record `{ a : Int, b : Int }` so a `Record` literal
                // fixture over the `a`/`b` field names resolves a synthesised
                // struct name via `record_struct_name`.
                records: vec![IrType::Record(std::collections::BTreeMap::from([
                    (rec_field_a, IrType::Int),
                    (rec_field_b, IrType::Int),
                ]))],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        };
        Fixture {
            interner,
            program,
            syms,
            main_mod,
            msg_ty,
            wrap_ctor,
            triple_ctor,
            unit_ctor,
            maybe_ty,
            just_ctor,
        }
    }

    fn with_ctx<R>(fx: &Fixture, f: impl FnOnce(&EmitCtx) -> R) -> R {
        let ctx = EmitCtx::build(
            &fx.interner,
            &fx.program,
            DbDriver::Sqlite,
            None,
            ipe_ir::Target::Native,
            Vec::new(),
            false,
        )
        .expect("EmitCtx::build");
        f(&ctx)
    }

    fn sym(fx: &Fixture, i: usize) -> ipe_intern::Symbol {
        fx.syms.get(i).copied().expect("fixture var symbol")
    }

    fn var(fx: &Fixture, i: usize) -> Expr {
        Expr::Var(sym(fx, i))
    }

    fn binop(op: BinOp, l: Expr, r: Expr) -> Expr {
        Expr::BinOp {
            op,
            lhs: Box::new(l),
            rhs: Box::new(r),
        }
    }

    /// The all-variant-shape fixture matrix. Every fixture that `build_doc`
    /// structures (or explicitly delegates as a leaf) appears here so a missing
    /// or drifted builder fails the SEAL property, not a runtime `unreachable!`.
    #[allow(clippy::too_many_lines, reason = "one entry per expr shape under test")]
    fn seal_fixtures(fx: &Fixture) -> Vec<Expr> {
        vec![
            // Leaves.
            Expr::Int(42),
            Expr::Float(3.0),
            Expr::Str("hi".to_owned()),
            Expr::Char("z".to_owned()),
            Expr::Bool(true),
            Expr::Unit,
            var(fx, 0),
            Expr::CloneVar(sym(fx, 0)),
            // Chain-eligible binops (structured).
            binop(BinOp::Add, var(fx, 0), var(fx, 1)),
            binop(
                BinOp::Add,
                binop(BinOp::Add, var(fx, 0), var(fx, 1)),
                var(fx, 2),
            ),
            binop(
                BinOp::Add,
                binop(BinOp::Mul, var(fx, 0), var(fx, 1)),
                var(fx, 2),
            ),
            binop(BinOp::Eq, var(fx, 0), var(fx, 1)),
            binop(BinOp::And, var(fx, 0), var(fx, 1)),
            // Call-shaped binops (leaf).
            binop(BinOp::Append, var(fx, 0), var(fx, 1)),
            binop(BinOp::IntDiv, var(fx, 0), var(fx, 1)),
            // If (structured) — narrow, stays inline.
            Expr::If {
                cond: Box::new(var(fx, 0)),
                then_: Box::new(Expr::Int(1)),
                else_: Box::new(Expr::Int(2)),
            },
            // If (structured) — wide, breaks to block form; the SEAL must hold
            // across the break/flat boundary (same tokens, different layout).
            Expr::If {
                cond: Box::new(var(fx, 4)),
                then_: Box::new(var(fx, 5)),
                else_: Box::new(var(fx, 6)),
            },
            // Composite: a chain-eligible binop whose operands are themselves
            // `if` expressions — exercises the structured `if` builder nested
            // inside the structured `Chain` builder.
            binop(
                BinOp::Add,
                Expr::If {
                    cond: Box::new(var(fx, 0)),
                    then_: Box::new(Expr::Int(1)),
                    else_: Box::new(Expr::Int(2)),
                },
                Expr::If {
                    cond: Box::new(var(fx, 1)),
                    then_: Box::new(Expr::Int(3)),
                    else_: Box::new(Expr::Int(4)),
                },
            ),
            // Composite: an `if` whose branches are chain-eligible binops —
            // exercises the structured `Chain` builder nested inside the `if`.
            Expr::If {
                cond: Box::new(var(fx, 0)),
                then_: Box::new(binop(BinOp::Add, var(fx, 1), var(fx, 2))),
                else_: Box::new(binop(BinOp::Mul, var(fx, 1), var(fx, 2))),
            },
            // Composite: a chain whose first operand is a WIDE (block-form) `if`,
            // so that operand renders multiline inside the chain — the glue-after-
            // multiline-operand path. The SEAL must hold across the break.
            binop(
                BinOp::Add,
                Expr::If {
                    cond: Box::new(var(fx, 4)),
                    then_: Box::new(var(fx, 5)),
                    else_: Box::new(var(fx, 6)),
                },
                var(fx, 3),
            ),
            // Tuple (structured, inline).
            Expr::Tuple(vec![var(fx, 0), var(fx, 1)]),
            // Tuple (structured, wide → breaks). SEAL holds across the boundary.
            Expr::Tuple(vec![var(fx, 7), var(fx, 8), var(fx, 9)]),
            // Cons onto the empty list (structured; tail is a leaf empty-list).
            Expr::Cons {
                head: Box::new(var(fx, 0)),
                tail: Box::new(Expr::List {
                    elem: IrType::Int,
                    items: vec![],
                }),
            },
            // Cons (structured, wide → breaks both arguments).
            Expr::Cons {
                head: Box::new(var(fx, 7)),
                tail: Box::new(var(fx, 8)),
            },
            // Non-empty list (structured, inline).
            Expr::List {
                elem: IrType::Int,
                items: vec![var(fx, 0), var(fx, 1)],
            },
            // Non-empty list (structured, wide → breaks).
            Expr::List {
                elem: IrType::Int,
                items: vec![var(fx, 7), var(fx, 8), var(fx, 9)],
            },
            // Empty list (leaf: `Vec::<i64>::new()`, no positional layout).
            Expr::List {
                elem: IrType::Int,
                items: vec![],
            },
            // Nullary user constructor (leaf: `Msg::Unit`, no positional payload).
            Expr::Ctor {
                home: ModPath(vec![fx.main_mod]),
                ty: fx.msg_ty,
                variant: fx.unit_ctor,
                args: vec![],
            },
            // Saturated payload user constructor (structured): `Msg::Wrap(a)`.
            Expr::Ctor {
                home: ModPath(vec![fx.main_mod]),
                ty: fx.msg_ty,
                variant: fx.wrap_ctor,
                args: vec![var(fx, 0)],
            },
            // Built-in runtime-enum constructor (structured): `IpeMaybe::Just(a)`.
            Expr::Ctor {
                home: ModPath(vec![]),
                ty: fx.maybe_ty,
                variant: fx.just_ctor,
                args: vec![var(fx, 0)],
            },
            // Plain user-function call (structured, inline).
            Expr::Call {
                callee: Callee::Func(FuncId::from_raw(0)),
                args: vec![var(fx, 0), var(fx, 1)],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            },
            // Plain user-function call (structured, wide → breaks args).
            Expr::Call {
                callee: Callee::Func(FuncId::from_raw(0)),
                args: vec![var(fx, 7), var(fx, 8), var(fx, 9)],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            },
            // Zero-arg call (leaf: `crate::…()`, no positional layout).
            Expr::Call {
                callee: Callee::Func(FuncId::from_raw(0)),
                args: vec![],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            },
            // Container-swapping kernel call (structured generic tail): `Maybe.map`
            // reverses its two arguments to the runtime's container-first order,
            // so the Doc builder must reverse the arg docs to match the string
            // emitter's `parts.reverse()`. No probe matches `MaybeMap`.
            Expr::Call {
                callee: Callee::Kernel(KernelFn::MaybeMap),
                args: vec![var(fx, 0), var(fx, 1)],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            },
            // Csv parse kernel call (structured generic tail): the empty pin gets
            // the `::<IpeError>` error-channel anchor between the name and `(`.
            Expr::Call {
                callee: Callee::Kernel(KernelFn::CsvParse),
                args: vec![var(fx, 0)],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            },
            // General function-value application (structured, inline): `(a)(b, c)`.
            Expr::Apply {
                func: Box::new(var(fx, 0)),
                args: vec![var(fx, 1), var(fx, 2)],
            },
            // General application (structured, wide → breaks args).
            Expr::Apply {
                func: Box::new(var(fx, 0)),
                args: vec![var(fx, 7), var(fx, 8), var(fx, 9)],
            },
            // Immediately-applied lambda (leaf: rewrites to a `({ let … })` block —
            // the block form is P2 work, so the string emitter's bytes are carried).
            Expr::Apply {
                func: Box::new(Expr::Lambda {
                    params: vec![(sym(fx, 3), IrType::Int)],
                    ret: IrType::Int,
                    body: Box::new(var(fx, 3)),
                }),
                args: vec![Expr::Int(1)],
            },
            // Zero-arg application (leaf: `(f)()`, no positional layout).
            Expr::Apply {
                func: Box::new(var(fx, 0)),
                args: vec![],
            },
            // `let` normal path (structured block): `({ let a = b; c })`. The body
            // references `a` zero times, so `needs_inline` is false — the plain
            // one-statement block form.
            Expr::Let {
                name: sym(fx, 0),
                value: Box::new(var(fx, 1)),
                body: Box::new(var(fx, 2)),
            },
            // `let` with a body that uses the binding (still normal path — the
            // value `b` is a plain Clone `Var`, so `needs_inline` is false).
            Expr::Let {
                name: sym(fx, 0),
                value: Box::new(var(fx, 1)),
                body: Box::new(binop(BinOp::Add, var(fx, 0), var(fx, 2))),
            },
            // `Destructure` block, alias-free tuple binder (structured): a SINGLE
            // flat `let (a, b) = c;` statement, then the body.
            Expr::Destructure {
                binder: Pat::Tuple(vec![Pat::Var(sym(fx, 0)), Pat::Var(sym(fx, 1))]),
                value: Box::new(var(fx, 2)),
                body: Box::new(var(fx, 0)),
            },
            // `Destructure` block, ALIASED tuple binder (structured): the clone-
            // split MULTI-statement sequence (`let x = c; let (a, b) = x.clone();`),
            // each statement on its own line, then the body. Exercises the
            // multiple-`let` path the plain `let` block never hits.
            Expr::Destructure {
                binder: Pat::Alias(
                    Box::new(Pat::Tuple(vec![Pat::Var(sym(fx, 0)), Pat::Var(sym(fx, 1))])),
                    sym(fx, 3),
                ),
                value: Box::new(var(fx, 2)),
                body: Box::new(var(fx, 0)),
            },
            // Record literal (structured, inline): `RecXY { a: 1, b: 2 }` over the
            // fixture's registered two-field struct.
            Expr::Record(vec![(sym(fx, 0), Expr::Int(1)), (sym(fx, 1), Expr::Int(2))]),
            // Record literal (structured, wide → breaks fields one per line).
            Expr::Record(vec![(sym(fx, 0), var(fx, 7)), (sym(fx, 1), var(fx, 8))]),
            // Record update (structured statement block): clone the base, reassign
            // one field, return the temp. Always breaks (holds statements).
            Expr::Update {
                record: Box::new(var(fx, 2)),
                fields: vec![(sym(fx, 0), Expr::Int(9))],
            },
            // Record update with TWO reassigned fields — multiple assignment lines.
            Expr::Update {
                record: Box::new(var(fx, 2)),
                fields: vec![(sym(fx, 0), Expr::Int(9)), (sym(fx, 1), Expr::Int(8))],
            },
            // Sync task-sequencing block (structured statement block): block on the
            // effect (discarding its result), then evaluate `rest`. Always breaks.
            // The effect `a` and `rest` `b` are distinct vars, so no clone-capture
            // rewrite fires — the SEAL leaves stay the plain `a`/`b` tokens.
            Expr::TaskSeqSync {
                effect: Box::new(var(fx, 0)),
                rest: Box::new(var(fx, 1)),
            },
            // Sync task-seq whose `rest` re-reads the same var the `effect` moves:
            // the IR-level clone-capture rewrite turns the effect's `a` into a
            // `CloneVar` (`a.clone()`), so the effect and body carry DIFFERENT
            // leaves — the SEAL must still match the string emitter's rewritten
            // tokens (both apply the identical `clone_targets_in_expr` pass).
            Expr::TaskSeqSync {
                effect: Box::new(Expr::Call {
                    callee: Callee::Func(FuncId::from_raw(0)),
                    args: vec![var(fx, 0)],
                    pin: CallPin::None,
                    on_form: OnFormKind::NotForm,
                }),
                rest: Box::new(var(fx, 0)),
            },
        ]
    }

    #[test]
    fn seal_leaf_sequence_matches_emit_expr_at_over_all_variants() {
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            for expr in seal_fixtures(&fx) {
                let scope = GenericScope::new(&[]);
                let doc = build_doc(ctx, &expr, 0, 0, scope).expect("build_doc");
                let string = emit_expr_at(ctx, &expr, 0, 0, scope).expect("emit_expr_at");
                assert_eq!(
                    doc.normalized_leaves(),
                    whitespace_normalize(&string),
                    "\nSEAL mismatch for {expr:?}\n  doc leaves : {}\n  emit string: {}",
                    doc.normalized_leaves(),
                    whitespace_normalize(&string),
                );
            }
        });
    }

    #[test]
    fn chain_builder_carries_every_paren() {
        // `((a + b) + c)` — the string emitter wraps each level; the chain
        // builder must carry both `(` and both `)` as leaves so the SEAL holds.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = binop(
                BinOp::Add,
                binop(BinOp::Add, var(&fx, 0), var(&fx, 1)),
                var(&fx, 2),
            );
            let scope = GenericScope::new(&[]);
            let doc = build_doc(ctx, &expr, 0, 0, scope).expect("build_doc");
            let mut leaves = String::new();
            doc.collect_leaves(&mut leaves);
            let opens = leaves.matches('(').count();
            let closes = leaves.matches(')').count();
            assert_eq!(opens, 2, "two wrapping open-parens: {leaves}");
            assert_eq!(closes, 2, "two wrapping close-parens: {leaves}");
            // And it renders inline (fits width): `((a + b) + c)`.
            let rendered = render(&doc, RenderConfig::default());
            assert_eq!(rendered, "((a + b) + c)");
        });
    }

    #[test]
    fn chain_breaks_tail_to_shared_indent_when_too_wide() {
        // A chain wider than 100 cols in a `let z = ` statement at block indent 4
        // breaks its tail operators one-per-line to col 8 (block indent 4 + chain
        // step 4), the param_patterns golden's chain shape. The statement is
        // wrapped in `nest(4)` so the renderer's block indent is 4 (matching the
        // golden's `    let z = ` origin) while the mid-line `let z = ` prefix
        // stays on the statement's first column. Driven through the public
        // `render` entry — no private renderer hook needed.
        let operands = vec![
            ChainOperand {
                leading_op: None,
                doc: Doc::owned(
                    "((((longfunctioncallnamehere_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_owned(),
                ),
            },
            ChainOperand {
                leading_op: Some(Cow::Borrowed("+")),
                doc: Doc::owned("bbbbbbbbbbb)".to_owned()),
            },
            ChainOperand {
                leading_op: Some(Cow::Borrowed("+")),
                doc: Doc::owned("cc)".to_owned()),
            },
            ChainOperand {
                leading_op: Some(Cow::Borrowed("+")),
                doc: Doc::owned("dd)".to_owned()),
            },
            ChainOperand {
                leading_op: Some(Cow::Borrowed("+")),
                doc: Doc::owned("ee)".to_owned()),
            },
        ];
        let doc = Doc::nest(
            4,
            Doc::concat(vec![Doc::text("let z = "), Doc::Chain { operands }]),
        );
        let out = render(&doc, RenderConfig::default());
        // Line 1 packs the maximal prefix that fits width 100: the long first
        // operand, `+ bbbbbbbbbbb)`, and the tiny `+ cc)` all fit (~99 cols);
        // `+ dd` overflows and breaks, and from there `+ ee` breaks too — every
        // post-boundary operator to the shared col-8 indent (block 4 + step 4).
        let expected = "let z = ((((longfunctioncallnamehere_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa + bbbbbbbbbbb) + cc)\n        + dd)\n        + ee)";
        assert_eq!(
            out, expected,
            "\n--- got ---\n{out}\n--- want ---\n{expected}"
        );
    }

    #[test]
    fn if_expr_fits_inline() {
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = Expr::If {
                cond: Box::new(var(&fx, 0)),
                then_: Box::new(Expr::Int(1)),
                else_: Box::new(Expr::Int(2)),
            };
            let scope = GenericScope::new(&[]);
            let doc = build_doc(ctx, &expr, 0, 0, scope).expect("build_doc");
            let rendered = render(&doc, RenderConfig::default());
            assert_eq!(rendered, "(if a { 1 } else { 2 })");
        });
    }

    #[test]
    fn if_expr_at_threshold_boundary_stays_inline() {
        // An `if cond { then } else { else }` construct exactly 50 columns wide
        // (the threshold) stays inline; 51 breaks. `cond1234567890` + `thenvalab`
        // + `elsevalab` is the pinned 50-wide construct captured from rustfmt.
        let mut interner = Interner::new();
        let main_mod = interner.intern("Main").expect("intern Main");
        let cond = interner.intern("cond1234567890").expect("intern");
        let thenv = interner.intern("thenvalab").expect("intern");
        let elsev = interner.intern("elsevalab").expect("intern");
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
                uses_websocket: false,
                uses_email: false,
                uses_env_public: false,
                uses_ffi: false,
            }],
        };
        let ctx = EmitCtx::build(
            &interner,
            &program,
            DbDriver::Sqlite,
            None,
            ipe_ir::Target::Native,
            Vec::new(),
            false,
        )
        .expect("EmitCtx::build");
        let scope = GenericScope::new(&[]);
        let expr = Expr::If {
            cond: Box::new(Expr::Var(cond)),
            then_: Box::new(Expr::Var(thenv)),
            else_: Box::new(Expr::Var(elsev)),
        };
        let doc = build_doc(&ctx, &expr, 4, 0, scope).expect("build_doc");
        assert_eq!(
            render(&doc, RenderConfig::default()),
            "(if cond1234567890 { thenvalab } else { elsevalab })",
            "a 50-wide construct must stay inline"
        );
    }

    #[test]
    fn if_expr_breaks_to_block_form_when_over_threshold() {
        // `(if some_condition_variable { first_branch_value } else {
        // second_branch_value_here })` — the construct is 83 cols (> 50), so
        // rustfmt breaks each branch onto its own line. Golden captured from
        // `rustfmt --edition 2024 --style-edition 2024` at block indent 4.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let expr = Expr::If {
                cond: Box::new(var(&fx, 4)),  // some_condition_variable
                then_: Box::new(var(&fx, 5)), // first_branch_value
                else_: Box::new(var(&fx, 6)), // second_branch_value_here
            };
            let doc = build_doc(ctx, &expr, 4, 0, scope).expect("build_doc");
            // Rendered inside a `let z = ` statement at block indent 4, matching the
            // captured golden's origin column.
            let stmt = Doc::nest(4, Doc::concat(vec![Doc::text("let z = "), doc]));
            let got = render(&stmt, RenderConfig::default());
            let expected = "let z = (if some_condition_variable {\n        first_branch_value\n    } else {\n        second_branch_value_here\n    })";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn tuple_and_cons_render_flat() {
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let tup = Expr::Tuple(vec![var(&fx, 0), var(&fx, 1)]);
            let doc = build_doc(ctx, &tup, 0, 0, scope).expect("build_doc");
            assert_eq!(render(&doc, RenderConfig::default()), "(a, b)");

            let cons = Expr::Cons {
                head: Box::new(var(&fx, 0)),
                tail: Box::new(Expr::List {
                    elem: IrType::Int,
                    items: vec![],
                }),
            };
            let doc = build_doc(ctx, &cons, 0, 0, scope).expect("build_doc");
            assert_eq!(
                render(&doc, RenderConfig::default()),
                "ipe_runtime::list::ipe_list_cons(a, Vec::<i64>::new())"
            );
        });
    }

    /// Render `expr` as the value of a `let z = ` statement at block indent 4,
    /// returning the whole statement line(s) so a mid-line delimited group starts
    /// at the same column `rustfmt` captured its golden from.
    fn render_let_stmt(ctx: &EmitCtx, expr: &Expr) -> String {
        let scope = GenericScope::new(&[]);
        let doc = build_doc(ctx, expr, 4, 0, scope).expect("build_doc");
        let stmt = Doc::nest(4, Doc::concat(vec![Doc::text("let z = "), doc]));
        render(&stmt, RenderConfig::default())
    }

    #[test]
    fn tuple_breaks_one_element_per_line_with_trailing_comma() {
        // `(x, y, z)` with three wide elements overflows width 100, so rustfmt
        // breaks each element onto its own line at nest+4 with a trailing comma and
        // dedents the closing paren to the statement indent. Golden captured from
        // `rustfmt --edition 2024 --style-edition 2024`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = Expr::Tuple(vec![var(&fx, 7), var(&fx, 8), var(&fx, 9)]);
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = (\n        argument_that_is_quite_long_enough_to_matter_x,\n        argument_that_is_quite_long_enough_to_matter_y,\n        argument_that_is_quite_long_enough_to_matter_z,\n    )";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn list_breaks_one_element_per_line_with_trailing_comma() {
        // `vec![x, y, z]` with three wide elements overflows; rustfmt breaks each
        // onto its own line with a trailing comma and dedents `]` to the statement
        // indent. Golden captured from rustfmt.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = Expr::List {
                elem: IrType::Int,
                items: vec![var(&fx, 7), var(&fx, 8), var(&fx, 9)],
            };
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = vec![\n        argument_that_is_quite_long_enough_to_matter_x,\n        argument_that_is_quite_long_enough_to_matter_y,\n        argument_that_is_quite_long_enough_to_matter_z,\n    ]";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn cons_breaks_both_arguments_with_trailing_comma() {
        // A cons whose two arguments together overflow width 100 breaks each onto
        // its own line with a trailing comma; `)` dedents to the statement indent.
        // Golden captured from rustfmt.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = Expr::Cons {
                head: Box::new(var(&fx, 7)),
                tail: Box::new(var(&fx, 8)),
            };
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = ipe_runtime::list::ipe_list_cons(\n        argument_that_is_quite_long_enough_to_matter_x,\n        argument_that_is_quite_long_enough_to_matter_y,\n    )";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn user_call_breaks_args_one_per_line_with_trailing_comma() {
        // A plain user-function call whose three wide args overflow width 100
        // breaks each arg onto its own line at nest+4 with a trailing comma, `)`
        // dedented to the statement indent — rustfmt's call-arg layout. The prefix
        // is `callee_name(callee)(` (the exact Rust name the emitter uses); the
        // layout (breaks, trailing comma, indent) is what this golden pins.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let callee = Callee::Func(FuncId::from_raw(0));
            let name = super::callee_name(ctx, &callee).expect("callee_name");
            let expr = Expr::Call {
                callee,
                args: vec![var(&fx, 7), var(&fx, 8), var(&fx, 9)],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            };
            let got = render_let_stmt(ctx, &expr);
            let expected = format!(
                "let z = {name}(\n        argument_that_is_quite_long_enough_to_matter_x,\n        argument_that_is_quite_long_enough_to_matter_y,\n        argument_that_is_quite_long_enough_to_matter_z,\n    )"
            );
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn user_call_fits_inline() {
        // A short user-function call stays inline: `name(a, b)`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let callee = Callee::Func(FuncId::from_raw(0));
            let name = super::callee_name(ctx, &callee).expect("callee_name");
            let expr = Expr::Call {
                callee,
                args: vec![var(&fx, 0), var(&fx, 1)],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            };
            let doc = build_doc(ctx, &expr, 0, 0, scope).expect("build_doc");
            assert_eq!(
                render(&doc, RenderConfig::default()),
                format!("{name}(a, b)")
            );
        });
    }

    #[test]
    fn swapping_kernel_call_reverses_args_inline() {
        // `Maybe.map f m` lowers container-first: the runtime call reverses the two
        // arguments. The Doc builder's inline render must equal the string
        // emitter's byte-for-byte (same reversed order).
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let expr = Expr::Call {
                callee: Callee::Kernel(KernelFn::MaybeMap),
                args: vec![var(&fx, 0), var(&fx, 1)],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            };
            let doc = build_doc(ctx, &expr, 0, 0, scope).expect("build_doc");
            let string = emit_expr_at(ctx, &expr, 0, 0, scope).expect("emit_expr_at");
            assert_eq!(render(&doc, RenderConfig::default()), string);
            // The runtime container comes first: `b` (the container arg) precedes
            // `a` (the mapped function) in the emitted call.
            assert!(
                string.contains("(b, a)"),
                "swapped arg order expected, got {string}"
            );
        });
    }

    #[test]
    fn csv_parse_kernel_call_carries_turbofish_inline() {
        // `Csv.parse` gets the `::<IpeError>` error-channel anchor between the name
        // and its `(` argument list. The Doc builder must carry it identically.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let expr = Expr::Call {
                callee: Callee::Kernel(KernelFn::CsvParse),
                args: vec![var(&fx, 0)],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            };
            let doc = build_doc(ctx, &expr, 0, 0, scope).expect("build_doc");
            let string = emit_expr_at(ctx, &expr, 0, 0, scope).expect("emit_expr_at");
            assert_eq!(render(&doc, RenderConfig::default()), string);
            assert!(
                string.contains("::<IpeError>("),
                "turbofish anchor expected, got {string}"
            );
        });
    }

    #[test]
    fn chain_operand_that_breaks_multiline_glues_following_operator() {
        // A chain `(broadIf + tail)` whose FIRST operand is a block-form (wide)
        // `if` renders the `if` multiline; the single `+ tail)` operator glues to
        // the `if`'s closing-line column (the chain has not broken, and the glued
        // operand fits). This exercises `render_chain`'s multiline-operand glue
        // path with a structured operand — the previously-untested composite.
        // Golden captured from `rustfmt --edition 2024 --style-edition 2024`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let wide_if = Expr::If {
                cond: Box::new(var(&fx, 4)),  // some_condition_variable
                then_: Box::new(var(&fx, 5)), // first_branch_value
                else_: Box::new(var(&fx, 6)), // second_branch_value_here
            };
            let expr = binop(BinOp::Add, wide_if, var(&fx, 3)); // + x (a placeholder tail)
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = ((if some_condition_variable {\n        first_branch_value\n    } else {\n        second_branch_value_here\n    }) + x)";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn delimited_trailing_comma_is_seal_invisible() {
        // The break-conditional trailing comma is NOT part of the SEAL leaf
        // sequence (the string emitter never emits it), so a broken tuple's
        // normalized leaves must still equal the string emitter's normalized bytes.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let expr = Expr::Tuple(vec![var(&fx, 7), var(&fx, 8), var(&fx, 9)]);
            let doc = build_doc(ctx, &expr, 4, 0, scope).expect("build_doc");
            let string = emit_expr_at(ctx, &expr, 4, 0, scope).expect("emit_expr_at");
            assert_eq!(doc.normalized_leaves(), whitespace_normalize(&string));
        });
    }

    #[test]
    fn ctor_fits_inline() {
        // A short saturated user constructor stays inline: `<EnumName>::Wrap(a)`.
        // A nullary constructor is a bare path leaf: `<EnumName>::Unit`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let payload = Expr::Ctor {
                home: ModPath(vec![fx.main_mod]),
                ty: fx.msg_ty,
                variant: fx.wrap_ctor,
                args: vec![var(&fx, 0)],
            };
            // The exact Rust enum name the emitter chooses for `Main.Msg`.
            let string = emit_expr_at(ctx, &payload, 0, 0, scope).expect("emit_expr_at");
            let doc = build_doc(ctx, &payload, 0, 0, scope).expect("build_doc");
            assert_eq!(render(&doc, RenderConfig::default()), string);
            assert!(string.ends_with("::Wrap(a)"), "got {string}");

            let nullary = Expr::Ctor {
                home: ModPath(vec![fx.main_mod]),
                ty: fx.msg_ty,
                variant: fx.unit_ctor,
                args: vec![],
            };
            let ns = emit_expr_at(ctx, &nullary, 0, 0, scope).expect("emit_expr_at");
            let nd = build_doc(ctx, &nullary, 0, 0, scope).expect("build_doc");
            assert_eq!(render(&nd, RenderConfig::default()), ns);
            assert!(ns.ends_with("::Unit"), "got {ns}");
        });
    }

    #[test]
    fn ctor_breaks_fields_one_per_line_with_trailing_comma() {
        // A saturated three-field constructor whose args overflow width 100 breaks
        // each field onto its own line at nest+4 with a trailing comma, `)` dedented
        // to the statement indent — rustfmt's tuple-variant-call layout. The prefix
        // is `<EnumName>::Triple(` (the exact Rust name the emitter chooses); the
        // layout is what this golden pins. Golden captured from
        // `rustfmt --edition 2024 --style-edition 2024`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let expr = Expr::Ctor {
                home: ModPath(vec![fx.main_mod]),
                ty: fx.msg_ty,
                variant: fx.triple_ctor,
                args: vec![var(&fx, 7), var(&fx, 8), var(&fx, 9)],
            };
            // Recover the emitter's chosen prefix from the flat string form
            // (`<EnumName>::Triple(` up to the first `(`).
            let flat = emit_expr_at(ctx, &expr, 0, 0, scope).expect("emit_expr_at");
            let prefix = flat.split_once('(').expect("ctor has an open paren").0;
            let got = render_let_stmt(ctx, &expr);
            let expected = format!(
                "let z = {prefix}(\n        argument_that_is_quite_long_enough_to_matter_x,\n        argument_that_is_quite_long_enough_to_matter_y,\n        argument_that_is_quite_long_enough_to_matter_z,\n    )"
            );
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn apply_fits_inline() {
        // A short general application stays inline: `(a)(b, c)`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let expr = Expr::Apply {
                func: Box::new(var(&fx, 0)),
                args: vec![var(&fx, 1), var(&fx, 2)],
            };
            let doc = build_doc(ctx, &expr, 0, 0, scope).expect("build_doc");
            assert_eq!(render(&doc, RenderConfig::default()), "(a)(b, c)");
        });
    }

    #[test]
    fn apply_breaks_args_one_per_line_with_trailing_comma() {
        // A general application whose three wide args overflow width 100 breaks each
        // arg onto its own line at nest+4 with a trailing comma, `)` dedented to the
        // statement indent — rustfmt's call-arg layout with an `(f)(` prefix. Golden
        // captured from `rustfmt --edition 2024 --style-edition 2024`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = Expr::Apply {
                func: Box::new(var(&fx, 0)),
                args: vec![var(&fx, 7), var(&fx, 8), var(&fx, 9)],
            };
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = (a)(\n        argument_that_is_quite_long_enough_to_matter_x,\n        argument_that_is_quite_long_enough_to_matter_y,\n        argument_that_is_quite_long_enough_to_matter_z,\n    )";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn let_block_always_breaks_to_statements() {
        // A `let` block ALWAYS breaks (rustfmt never inlines a block that holds a
        // statement), even when short: `({`, the `let` statement and body each on
        // their own line at block+4 (col 8), `})` dedented to the statement indent.
        // Golden captured from `rustfmt --edition 2024 --style-edition 2024`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = Expr::Let {
                name: sym(&fx, 0),            // a
                value: Box::new(var(&fx, 1)), // b
                body: Box::new(var(&fx, 2)),  // c
            };
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = ({\n        let a = b;\n        c\n    })";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn nested_let_blocks_indent_by_the_renderer_nest_only() {
        // A `let` whose body is another `let` block: the inner block indents one
        // further step (col 12 = block 4 + outer Nest 4 + inner Nest 4), the inner
        // `})` sits at col 8. This pins that the renderer's `Nest(4)` is the SOLE
        // indent source — no leaf embeds indentation that would double-count.
        // Golden captured from `rustfmt --edition 2024 --style-edition 2024`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let inner = Expr::Let {
                name: sym(&fx, 0),
                value: Box::new(var(&fx, 1)),
                body: Box::new(var(&fx, 2)),
            };
            let expr = Expr::Let {
                name: sym(&fx, 0),
                value: Box::new(var(&fx, 1)),
                body: Box::new(inner),
            };
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = ({\n        let a = b;\n        ({\n            let a = b;\n            c\n        })\n    })";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn destructure_block_single_statement_always_breaks() {
        // A `Destructure` with an alias-free tuple binder is a ONE-statement block:
        // `({`, the flat `let (a, b) = c;`, the body `a`, `})` — each on its own
        // line, `})` dedented to the statement indent. Always breaks, like the
        // `let` block. Golden captured from `rustfmt --edition 2024`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = Expr::Destructure {
                binder: Pat::Tuple(vec![Pat::Var(sym(&fx, 0)), Pat::Var(sym(&fx, 1))]),
                value: Box::new(var(&fx, 2)), // c
                body: Box::new(var(&fx, 0)),  // a
            };
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = ({\n        let (a, b) = c;\n        a\n    })";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn destructure_block_aliased_binder_emits_multiple_statements() {
        // An ALIASED tuple binder expands to the clone-split MULTI-statement
        // sequence: `let x = c;` then `let (a, b) = x.clone();`, each on its own
        // `HardLine`, then the body — the path the plain `let` block never takes.
        // Golden captured from `rustfmt --edition 2024`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = Expr::Destructure {
                binder: Pat::Alias(
                    Box::new(Pat::Tuple(vec![
                        Pat::Var(sym(&fx, 0)),
                        Pat::Var(sym(&fx, 1)),
                    ])),
                    sym(&fx, 3), // x
                ),
                value: Box::new(var(&fx, 2)), // c
                body: Box::new(var(&fx, 0)),  // a
            };
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = ({\n        let x = c;\n        let (a, b) = x.clone();\n        a\n    })";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn record_fits_inline_with_spaces_inside_braces() {
        // A short record literal stays inline with a space hugging each brace:
        // `<StructName> { a: 1, b: 2 }`. This is the brace-delimited flat shape
        // (`delimited_spaced`), distinct from the space-free bracketed lists.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let expr = Expr::Record(vec![
                (sym(&fx, 0), Expr::Int(1)),
                (sym(&fx, 1), Expr::Int(2)),
            ]);
            let doc = build_doc(ctx, &expr, 0, 0, scope).expect("build_doc");
            let string = emit_expr_at(ctx, &expr, 0, 0, scope).expect("emit_expr_at");
            // The string emitter's flat form is already the rustfmt-canonical
            // inline shape (`Name { a: 1, b: 2 }`), so the render matches it.
            assert_eq!(render(&doc, RenderConfig::default()), string);
            assert!(string.ends_with(" { a: 1, b: 2 }"), "got {string}");
        });
    }

    #[test]
    fn record_breaks_fields_one_per_line_with_trailing_comma() {
        // A record whose two wide fields overflow width 100 breaks each field onto
        // its own line at nest+4 with a trailing comma, `}` dedented to the
        // statement indent — rustfmt's struct-literal layout. Golden captured from
        // `rustfmt --edition 2024 --style-edition 2024`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let expr = Expr::Record(vec![(sym(&fx, 0), var(&fx, 7)), (sym(&fx, 1), var(&fx, 8))]);
            // Recover the emitter's chosen struct name from the flat form
            // (everything up to the first ` {`).
            let flat = emit_expr_at(ctx, &expr, 0, 0, scope).expect("emit_expr_at");
            let name = flat.split_once(" {").expect("record has a brace").0;
            let got = render_let_stmt(ctx, &expr);
            let expected = format!(
                "let z = {name} {{\n        a: argument_that_is_quite_long_enough_to_matter_x,\n        b: argument_that_is_quite_long_enough_to_matter_y,\n    }}"
            );
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn update_block_single_field_always_breaks() {
        // A record update is a clone-and-reassign statement block: `{`, the
        // `let mut __ipe_rec = (c).clone();`, the `__ipe_rec.a = 9;` assignment, the
        // `__ipe_rec` tail, `}` — each on its own line. Always breaks. Golden
        // captured from `rustfmt --edition 2024 --style-edition 2024`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = Expr::Update {
                record: Box::new(var(&fx, 2)), // c
                fields: vec![(sym(&fx, 0), Expr::Int(9))],
            };
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = {\n        let mut __ipe_rec = (c).clone();\n        __ipe_rec.a = 9;\n        __ipe_rec\n    }";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn update_block_multiple_fields_emit_one_assignment_per_line() {
        // Two reassigned fields → two assignment lines between the `let mut` binding
        // and the `__ipe_rec` tail. Golden captured from rustfmt.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = Expr::Update {
                record: Box::new(var(&fx, 2)), // c
                fields: vec![(sym(&fx, 0), Expr::Int(9)), (sym(&fx, 1), Expr::Int(8))],
            };
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = {\n        let mut __ipe_rec = (c).clone();\n        __ipe_rec.a = 9;\n        __ipe_rec.b = 8;\n        __ipe_rec\n    }";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn task_seq_sync_block_always_breaks_to_statements() {
        // A sync task-seq is a statement block `{ let _ = task_run(<effect>); <rest> }`
        // that ALWAYS breaks (rustfmt never inlines a block that holds a statement):
        // `{`, the `let _ = task_run(a);` discard and the `b` tail each on their own
        // line at block+4 (col 8), `}` dedented to the statement indent. Golden
        // captured from `rustfmt --edition 2024 --style-edition 2024`.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let expr = Expr::TaskSeqSync {
                effect: Box::new(var(&fx, 0)), // a
                rest: Box::new(var(&fx, 1)),   // b
            };
            let got = render_let_stmt(ctx, &expr);
            let expected = "let z = {\n        let _ = task_run(a);\n        b\n    }";
            assert_eq!(
                got, expected,
                "\n--- got ---\n{got}\n--- want ---\n{expected}"
            );
        });
    }

    #[test]
    fn ctor_runtime_enum_just_renders_inline() {
        // A built-in `Maybe` constructor routes to the runtime enum
        // `IpeMaybe::Just(a)` — same delimited group, no field-boxing.
        let fx = fixture();
        with_ctx(&fx, |ctx| {
            let scope = GenericScope::new(&[]);
            let expr = Expr::Ctor {
                home: ModPath(vec![]),
                ty: fx.maybe_ty,
                variant: fx.just_ctor,
                args: vec![var(&fx, 0)],
            };
            let doc = build_doc(ctx, &expr, 0, 0, scope).expect("build_doc");
            assert_eq!(render(&doc, RenderConfig::default()), "IpeMaybe::Just(a)");
        });
    }
}
