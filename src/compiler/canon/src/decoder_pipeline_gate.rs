//! Reject the reverse-associated hand-nested form of the decoder-pipeline
//! combinators (`Db.Decode` / `Json.Decode.Pipeline`
//! `required` / `optional` / `requiredAt` / `custom`).
//!
//! These combinators are applicative: each takes the accumulated
//! constructor-decoder as its LAST argument and applies that constructor to the
//! field it just decoded. Written as a hand-nested application
//! (`required "a" da (required "b" db (succeed ctor))`) the INNERMOST
//! combinator binds to the constructor's FIRST parameter, so first-in-source
//! binds to the LAST parameter — a silent field↔parameter reversal that raises
//! no type error whenever adjacent fields share a runtime type. The idiomatic
//! `|>` pipe form (`succeed ctor |> required "a" da |> required "b" db`)
//! threads the accumulator the other way and is correct.
//!
//! This gate rejects the directly hand-nested spelling fail-closed with
//! IPE-N0040, whose message shows the pipe rewrite — the shape the footgun takes
//! in practice, and the one an "inline the decoder" edit produces. It keys on
//! the syntactic nesting of two combinator applications, so an accumulator
//! reached through a `let`/top-level binder is out of its current reach. It is
//! target-independent and runs on the linked module before type-checking.
//!
//! ## Why the pipe form is never flagged
//!
//! A flat call `required "a" da next` canonicalises to `Call(VarKernel(required),
//! [a, da, next])` — the head is directly a [`Expr_::VarKernel`]. The pipe form
//! `succeed ctor |> required "a" da` desugars to `Call(required "a" da,
//! [succeed ctor])`, whose head is itself a `Call` (the partial application
//! `required "a" da`), NOT a bare `VarKernel`, and whose inner partial
//! application is under-applied (fewer args than the combinator's arity). The
//! gate fires ONLY on a FULLY-applied combinator call whose head is a bare
//! kernel and whose accumulator argument is another FULLY-applied bare-kernel
//! call from the same family, so the pipe form cannot match.

use ipe_diagnostics::{DResult, Diagnostic, NameError, Span};
use ipe_intern::{Interner, Symbol};
use ipe_kernels::StdlibKernel;

use crate::ast::{CaseBranch, Def, Expr, Expr_, LetBinding, Module};

/// A fully-applied call to a reversible decoder-pipeline combinator: its
/// resolved kernel, the interned `module`/`name` for the diagnostic, and its
/// applied argument list (whose LAST element is the accumulator).
struct ReversibleCall<'e> {
    module: Symbol,
    name: Symbol,
    args: &'e [Expr],
}

/// The decoder-pipeline combinators whose accumulator (the constructor-decoder
/// / "next decoder") is their LAST argument, so a nested application reverses
/// the field↔parameter binding. Each entry is `(kernel, source-arity)`; a call
/// is "fully applied" when it carries exactly `arity` arguments.
const REVERSIBLE_COMBINATORS: &[(StdlibKernel, usize)] = &[
    (StdlibKernel::DbDecRequired, 3),
    (StdlibKernel::DbDecOptional, 4),
    (StdlibKernel::JsonDecPRequired, 3),
    (StdlibKernel::JsonDecPOptional, 4),
    (StdlibKernel::JsonDecPCustom, 2),
    (StdlibKernel::JsonDecPRequiredAt, 3),
];

/// The source arity of `kernel` when it is one of the reversible combinators,
/// else `None`.
fn reversible_arity(kernel: StdlibKernel) -> Option<usize> {
    REVERSIBLE_COMBINATORS
        .iter()
        .find_map(|&(k, arity)| (k == kernel).then_some(arity))
}

/// If `e` is a fully-applied call to a reversible combinator, return its
/// resolved form; else `None`. Only an `id`-resolved kernel head matches — an
/// unresolved (`id: None`) head is a decoder the registry never backed, and is
/// left to the existing fail-closed paths.
fn as_reversible_call(e: &Expr) -> Option<ReversibleCall<'_>> {
    let Expr_::Call(head, args) = &e.value else {
        return None;
    };
    let Expr_::VarKernel {
        id: Some(k),
        module,
        name,
    } = &head.value
    else {
        return None;
    };
    let arity = reversible_arity(*k)?;
    (args.len() == arity).then_some(ReversibleCall {
        module: *module,
        name: *name,
        args: args.as_slice(),
    })
}

/// Reject every reverse-associated hand-nested decoder pipeline in `module`.
///
/// # Errors
/// [`Diagnostic::Name`] (IPE-N0040) at the OUTERMOST offending combinator call,
/// in source order within each def.
pub fn check_decoder_pipelines(module: &Module, interner: &Interner) -> DResult<()> {
    for def in &module.defs {
        let body = match def {
            Def::Untyped { body, .. } | Def::Typed { body, .. } => body,
        };
        check_expr(body, interner)?;
    }
    Ok(())
}

fn reject(span: Span, qualifier: &str, name: &str) -> Diagnostic {
    Diagnostic::Name {
        span,
        msg: NameError::ReverseNestedDecoderPipeline {
            qualifier: qualifier.into(),
            name: name.into(),
        },
    }
}

/// Walk one expression with an explicit heap work-stack (a decoder body can
/// nest arbitrarily deep; native recursion would risk the thread stack).
fn check_expr<'e>(root: &'e Expr, interner: &Interner) -> DResult<()> {
    let mut stack: Vec<&'e Expr> = vec![root];
    while let Some(e) = stack.pop() {
        // A fully-applied reversible combinator whose accumulator (LAST) arg is
        // itself a fully-applied reversible combinator is the reverse-nested
        // form. Blame the OUTERMOST call — the one the author reads first.
        if let Some(outer) = as_reversible_call(e)
            && let Some(accumulator) = outer.args.last()
            && as_reversible_call(accumulator).is_some()
        {
            let q = interner.resolve(outer.module).unwrap_or("?");
            let n = interner.resolve(outer.name).unwrap_or("?");
            return Err(reject(e.span, q, n));
        }
        match &e.value {
            Expr_::VarLocal(_)
            | Expr_::VarTopLevel { .. }
            | Expr_::VarKernel { .. }
            | Expr_::VarCtor { .. }
            | Expr_::Int(_)
            | Expr_::Float(_)
            | Expr_::Str(_)
            | Expr_::PathLit(_)
            | Expr_::Char(_)
            | Expr_::Unit => {}
            Expr_::Call(f, args) => {
                stack.push(f);
                stack.extend(args.iter());
            }
            Expr_::ForeignCall { args, .. } => stack.extend(args.iter()),
            Expr_::Case(scrut, branches) => {
                stack.push(scrut);
                for CaseBranch { body, .. } in branches {
                    stack.push(body);
                }
            }
            Expr_::Lambda(_, body) => stack.push(body),
            Expr_::Binop { lhs, rhs, .. } => {
                stack.push(lhs);
                stack.push(rhs);
            }
            Expr_::Let(bindings, body) => {
                for LetBinding { body: b, .. } in bindings {
                    stack.push(b);
                }
                stack.push(body);
            }
            Expr_::If(arms, els) => {
                for (c, b) in arms {
                    stack.push(c);
                    stack.push(b);
                }
                stack.push(els);
            }
            Expr_::Tuple(items) | Expr_::List(items) => stack.extend(items.iter()),
            Expr_::Cons(h, t) => {
                stack.push(h);
                stack.push(t);
            }
            Expr_::Record(fields) => stack.extend(fields.iter().map(|(_, v)| v)),
            Expr_::Access(base, _) => stack.push(base),
            Expr_::Update(base, fields) => {
                stack.push(base);
                stack.extend(fields.iter().map(|(_, v)| v));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ipe_diagnostics::Located;

    use super::*;

    fn intern(interner: &mut Interner, s: &str) -> ipe_intern::Symbol {
        #[allow(clippy::expect_used)]
        interner.intern(s).expect("intern must succeed in a test")
    }

    fn single_def_module(interner: &mut Interner, body_expr: Expr_) -> Module {
        let main = vec![intern(interner, "Main")];
        let entry = intern(interner, "decoder");
        let def = Def::Untyped {
            home: main.clone(),
            name: Located::new(Span::DUMMY, entry),
            patterns: Vec::new(),
            body: Located::new(Span::DUMMY, body_expr),
        };
        Module {
            name: main,
            unions: Vec::new(),
            defs: vec![def],
        }
    }

    fn kernel(interner: &mut Interner, module: &str, name: &str, k: StdlibKernel) -> Expr {
        let m = intern(interner, module);
        let n = intern(interner, name);
        Located::new(
            Span::DUMMY,
            Expr_::VarKernel {
                id: Some(k),
                module: m,
                name: n,
            },
        )
    }

    fn str_lit(s: &str) -> Expr {
        Located::new(Span::DUMMY, Expr_::Str(s.to_owned()))
    }

    fn local(interner: &mut Interner, name: &str) -> Expr {
        let n = intern(interner, name);
        Located::new(Span::DUMMY, Expr_::VarLocal(n))
    }

    fn call(head: Expr, args: Vec<Expr>) -> Expr {
        Located::new(Span::DUMMY, Expr_::Call(Box::new(head), args))
    }

    /// A `succeed ctor` fully applied — the accumulator base of any pipeline.
    fn db_succeed(interner: &mut Interner) -> Expr {
        let succeed = kernel(interner, "Db.Decode", "succeed", StdlibKernel::DbDecSucceed);
        let ctor = local(interner, "ctor");
        call(succeed, vec![ctor])
    }

    /// The reverse-nested form is rejected with IPE-N0040 naming the outermost
    /// combinator: `required "a" da (required "b" db (succeed ctor))`.
    #[test]
    fn nested_db_required_is_rejected() {
        let mut i = Interner::new();
        let da = local(&mut i, "da");
        let db = local(&mut i, "db");
        let inner_req = kernel(&mut i, "Db.Decode", "required", StdlibKernel::DbDecRequired);
        let inner = call(inner_req, vec![str_lit("b"), db, db_succeed(&mut i)]);
        let outer_req = kernel(&mut i, "Db.Decode", "required", StdlibKernel::DbDecRequired);
        let body = call(outer_req, vec![str_lit("a"), da, inner]).value;
        let module = single_def_module(&mut i, body);
        let err = check_decoder_pipelines(&module, &i)
            .expect_err("nested Db.Decode.required must be rejected");
        let Diagnostic::Name {
            msg: NameError::ReverseNestedDecoderPipeline { qualifier, name },
            ..
        } = err
        else {
            return assert_eq!(
                format!("{err:?}"),
                "ReverseNestedDecoderPipeline",
                "expected ReverseNestedDecoderPipeline"
            );
        };
        assert_eq!(qualifier.as_ref(), "Db.Decode");
        assert_eq!(name.as_ref(), "required");
    }

    /// The nested `JsonDecP.required` form reverses identically and is rejected.
    #[test]
    fn nested_json_pipeline_required_is_rejected() {
        let mut i = Interner::new();
        let succeed = kernel(
            &mut i,
            "Json.Decode",
            "succeed",
            StdlibKernel::JsonDecSucceed,
        );
        let mk = local(&mut i, "mk");
        let base = call(succeed, vec![mk]);
        let db = local(&mut i, "str");
        let inner_req = kernel(
            &mut i,
            "JsonDecP",
            "required",
            StdlibKernel::JsonDecPRequired,
        );
        let inner = call(inner_req, vec![str_lit("b"), db, base]);
        let da = local(&mut i, "str");
        let outer_req = kernel(
            &mut i,
            "JsonDecP",
            "required",
            StdlibKernel::JsonDecPRequired,
        );
        let body = call(outer_req, vec![str_lit("a"), da, inner]).value;
        let module = single_def_module(&mut i, body);
        let err = check_decoder_pipelines(&module, &i)
            .expect_err("nested JsonDecP.required must be rejected");
        assert!(matches!(
            err,
            Diagnostic::Name {
                msg: NameError::ReverseNestedDecoderPipeline { .. },
                ..
            }
        ));
    }

    /// The nested `JsonDecP.optional` form (arity 4) also reverses.
    #[test]
    fn nested_json_pipeline_optional_is_rejected() {
        let mut i = Interner::new();
        let succeed = kernel(
            &mut i,
            "Json.Decode",
            "succeed",
            StdlibKernel::JsonDecSucceed,
        );
        let mk = local(&mut i, "mk");
        let base = call(succeed, vec![mk]);
        let inner = call(
            kernel(
                &mut i,
                "JsonDecP",
                "optional",
                StdlibKernel::JsonDecPOptional,
            ),
            vec![str_lit("b"), local(&mut i, "str"), str_lit("defB"), base],
        );
        let outer = call(
            kernel(
                &mut i,
                "JsonDecP",
                "optional",
                StdlibKernel::JsonDecPOptional,
            ),
            vec![str_lit("a"), local(&mut i, "str"), str_lit("defA"), inner],
        );
        let module = single_def_module(&mut i, outer.value);
        let err = check_decoder_pipelines(&module, &i)
            .expect_err("nested JsonDecP.optional must be rejected");
        assert!(matches!(
            err,
            Diagnostic::Name {
                msg: NameError::ReverseNestedDecoderPipeline { .. },
                ..
            }
        ));
    }

    /// The idiomatic pipe form desugars to `Call(required "a" da, [accumulator])`
    /// — the head is a partial-application `Call`, not a bare kernel — so it is
    /// accepted. `succeed ctor |> required "a" da |> required "b" db`.
    #[test]
    fn piped_form_is_accepted() {
        let mut i = Interner::new();
        // succeed ctor |> required "a" da  ==>  Call(required "a" da, [succeed ctor])
        let req_a = call(
            kernel(&mut i, "Db.Decode", "required", StdlibKernel::DbDecRequired),
            vec![str_lit("a"), local(&mut i, "da")],
        );
        let step1 = call(req_a, vec![db_succeed(&mut i)]);
        // ... |> required "b" db  ==>  Call(required "b" db, [step1])
        let req_b = call(
            kernel(&mut i, "Db.Decode", "required", StdlibKernel::DbDecRequired),
            vec![str_lit("b"), local(&mut i, "db")],
        );
        let step2 = call(req_b, vec![step1]);
        let module = single_def_module(&mut i, step2.value);
        check_decoder_pipelines(&module, &i)
            .expect("the idiomatic pipe form must be accepted unchanged");
    }

    /// A single `required` over a `succeed` base (one field, or the innermost of
    /// a correct pipe) is NOT nested: its accumulator is `succeed`, not another
    /// reversible combinator. Accepted.
    #[test]
    fn single_required_over_succeed_is_accepted() {
        let mut i = Interner::new();
        let body = call(
            kernel(&mut i, "Db.Decode", "required", StdlibKernel::DbDecRequired),
            vec![str_lit("a"), local(&mut i, "da"), db_succeed(&mut i)],
        )
        .value;
        let module = single_def_module(&mut i, body);
        check_decoder_pipelines(&module, &i)
            .expect("a single required over succeed is not the reverse-nested form");
    }

    /// A decoder-free program passes unconditionally.
    #[test]
    fn decoder_free_program_passes() {
        let mut i = Interner::new();
        let module = single_def_module(&mut i, Expr_::Unit);
        check_decoder_pipelines(&module, &i).expect("a decoder-free program must pass");
    }
}
