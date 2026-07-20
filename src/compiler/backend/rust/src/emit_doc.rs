//! The Doc-building emit path (P1): a parallel to
//! [`crate::emit_expr::emit_expr_at`] that returns a [`Doc`] instead of a
//! `String`.
//!
//! Every builder carries EXACTLY the token sequence the string emitter produces,
//! so the whitespace-normalized leaf sequence of `build_doc(e)` equals the
//! whitespace-normalized string of `emit_expr_at(e)` — the SEAL. The one
//! layout-bearing arm this phase structures is the binary-operator chain
//! (`BinOp` Add..Or), flattened into a [`Doc::Chain`] so the renderer lays it
//! out to `rustfmt`-canonical bytes. Every other arm is carried as a single
//! [`Doc::owned`] leaf holding the string emitter's exact bytes.
//!
//! The chain is the only construct the frozen renderer can currently break-or-
//! flatten: a [`Doc::Group`]'s bare [`Doc::Line`]s are unconditional hard breaks
//! (a group never flattens a `{ 1 }` / `(a, b)` body), so structured `if` /
//! block / call / tuple layout needs a renderer extension and is deferred to P2.
//! Carrying those arms as leaves is byte-exact today because the string emitter
//! already produces their single-line pre-`rustfmt` form. The leaf path spans
//! the literals (Int/Float/Str/…), the call-shaped binops (`Append`/`IntDiv`),
//! and the context-heavy arms (kernel-dispatch calls, `let`/destructure binding
//! statements, lambdas, `match`, records, updates, apply, field access).
//!
//! This path is gated behind tests and the `RustFmtConfig.native` flag; the
//! legacy string path in `emit_expr.rs` remains the emit default until the P3
//! cutover, so every intermediate commit stays byte-green against the goldens.

// The Doc emit path is wired into project.rs at the P3 cutover; until then its
// builders are exercised by the golden_doc_render.rs SEAL + byte tests only.
#![allow(dead_code, reason = "consumed at the P3 native-emit cutover")]

use std::borrow::Cow;

use ipe_diagnostics::DResult;
use ipe_ir::{BinOp, Expr};

use crate::EmitCtx;
use crate::doc::{ChainOperand, Doc};
use crate::emit_expr::emit_expr_at;
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

        // Every remaining arm carries the string emitter's exact bytes as one
        // leaf. The frozen renderer's only inline-vs-break mechanism today is
        // the `Chain` node (a `Group`'s bare `Doc::Line`s are unconditional hard
        // breaks, so a `Group` cannot flatten a `{ 1 }` / `(a, b)` body — that
        // needs a renderer extension, which is P2). Until then a leaf carrying
        // the string emitter's bytes is byte-exact for every non-chain arm:
        // leaves (Int/Float/Str/…), the call-shaped binops (Append/IntDiv), the
        // `if`/tuple/cons/list constructs, and the opaque context-heavy arms
        // (Ctor/Call/Let/Destructure/Access/Record/Update/Lambda/Match/TaskSeq).
        // Carrying bytes keeps the SEAL exact by construction.
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

#[cfg(test)]
#[allow(clippy::panic, clippy::expect_used, reason = "test assertions")]
mod tests {
    //! P1 acceptance: the SEAL leaf-sequence property (every builder carries the
    //! string emitter's exact tokens) and per-builder byte-goldens through the
    //! renderer. The goldens are captured from `rustfmt --edition 2024
    //! --style-edition 2024`; the renderer is fixed to match, never the reverse.

    use std::borrow::Cow;

    use ipe_intern::Interner;
    use ipe_ir::{BinOp, EnumDef, Expr, IrType, ModPath, Module, Program, TypeDef, Variant};

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
    }

    fn fixture() -> Fixture {
        let mut interner = Interner::new();
        let main_mod = interner.intern("Main").expect("intern Main");
        let msg_ty = interner.intern("Msg").expect("intern Msg");
        let unit_ctor = interner.intern("Unit").expect("intern Unit");
        let syms = ["a", "b", "c", "x"]
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
                    variants: vec![Variant {
                        name: unit_ctor,
                        fields: vec![],
                    }],
                })],
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
        Fixture {
            interner,
            program,
            syms,
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
            // If (structured).
            Expr::If {
                cond: Box::new(var(fx, 0)),
                then_: Box::new(Expr::Int(1)),
                else_: Box::new(Expr::Int(2)),
            },
            // Tuple (structured).
            Expr::Tuple(vec![var(fx, 0), var(fx, 1)]),
            // Cons (structured, call-shaped).
            Expr::Cons {
                head: Box::new(var(fx, 0)),
                tail: Box::new(Expr::List {
                    elem: IrType::Int,
                    items: vec![],
                }),
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
}
