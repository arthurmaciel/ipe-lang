//! Layer-1 wasm security gate: server-only kernels have no denotation in a
//! `--target wasm` build.
//!
//! Walks the (linked) canonical module and rejects every kernel reference
//! that is not on the `WasmClient` allowlist
//! (`ipe_kernels::StdlibKernel::available_on` — default-deny). Emitted code
//! contains every linked def, so the check is naming-based over the whole
//! module: a kernel outside the allowlist has no runtime symbol in the wasm
//! crate, and letting it through would trade this compile error for a cargo
//! failure (THE SEAL) — or, worse, a secret consumer in a public bundle.
//!
//! Foreign-crate FFI calls are equally unrepresentable client-side: the wasm
//! crate's only host surface is the fixed web-sys allowlist.

use ipe_diagnostics::{DResult, Diagnostic, NameError, Span};
use ipe_intern::Interner;
use ipe_kernels::Target;

use crate::ast::{CaseBranch, Def, Expr, Expr_, LetBinding, Module};

/// Reject every kernel/FFI reference in `module` that has no `WasmClient`
/// denotation.
///
/// # Errors
/// [`Diagnostic::Name`] (IPE-N0029) at the first offending reference, in
/// source order within each def.
pub fn check_wasm_client(module: &Module, interner: &Interner) -> DResult<()> {
    for def in &module.defs {
        let body = match def {
            Def::Untyped { body, .. } | Def::Typed { body, .. } => body,
        };
        check_expr(body, interner)?;
    }
    Ok(())
}

fn deny(span: Span, qualifier: &str, name: &str) -> Diagnostic {
    Diagnostic::Name {
        span,
        msg: NameError::ServerOnlyKernelForWasm {
            qualifier: qualifier.into(),
            name: name.into(),
        },
    }
}

/// Walk one expression with an explicit heap work-stack (a view/update body
/// can nest arbitrarily deep; native recursion would risk the thread stack).
fn check_expr<'e>(root: &'e Expr, interner: &Interner) -> DResult<()> {
    let mut stack: Vec<&'e Expr> = vec![root];
    while let Some(e) = stack.pop() {
        match &e.value {
            Expr_::VarKernel { id, module, name } => {
                let allowed = id.is_some_and(|k| k.available_on(Target::WasmClient));
                // `id: None` (a kernel resolved only by the string-match
                // fallback) fails closed — an unaudited kernel is denied.
                if !allowed {
                    let q = interner.resolve(*module).unwrap_or("?");
                    let n = interner.resolve(*name).unwrap_or("?");
                    return Err(deny(e.span, q, n));
                }
            }
            Expr_::ForeignCall { .. } => {
                // FFI bindings are a native-target concept (spec Q5): the
                // client's only host surface is the fixed web-sys allowlist.
                return Err(deny(e.span, "Ffi", "binding"));
            }
            Expr_::VarLocal(_)
            | Expr_::VarTopLevel { .. }
            | Expr_::VarCtor { .. }
            | Expr_::Int(_)
            | Expr_::Float(_)
            | Expr_::Str(_)
            | Expr_::Char(_)
            | Expr_::Unit => {}
            Expr_::Call(f, args) => {
                stack.push(f);
                stack.extend(args.iter());
            }
            Expr_::Case(scrut, branches) => {
                stack.push(scrut);
                for CaseBranch { body, .. } in branches {
                    stack.push(body);
                }
            }
            Expr_::Lambda(_, body) => stack.push(body),
            // Binops resolve to `Basics` arithmetic/comparison kernels —
            // pure, always client-representable.
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
