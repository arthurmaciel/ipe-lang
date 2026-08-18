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
        if let Some(d) = first_denied(body, interner) {
            return Err(d);
        }
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

/// Build a minimal single-def module whose body is `body_expr`.
#[cfg(test)]
fn single_def_module(interner: &mut Interner, body_expr: Expr_) -> Module {
    use ipe_diagnostics::Located;
    let main = vec![interner.intern("Main").expect("intern")];
    let entry = interner.intern("main").expect("intern");
    let def = Def::Untyped {
        home: main.clone(),
        name: Located::new(Span::DUMMY, entry),
        patterns: Vec::new(),
        body: Located::new(Span::DUMMY, body_expr),
    };
    Module {
        imports_unsafe_submodule: false,
        name: main,
        unions: Vec::new(),
        defs: vec![def],
    }
}

/// Return the first offending kernel/FFI reference in source order, or `None`
/// if the expression is clean. Children are visited in their textual
/// (left-to-right, outer-before-inner) order, matching the documented
/// contract: callee before arguments, scrutinee before arms, each binding
/// before its continuation, if-condition before body before else, and
/// tuple/list/record/update fields in their declaration order.
fn first_denied(e: &Expr, interner: &Interner) -> Option<Diagnostic> {
    match &e.value {
        Expr_::VarKernel { id, module, name } => {
            let allowed = id.is_some_and(|k| k.available_on(Target::WasmClient));
            // `id: None` (a kernel resolved only by the string-match
            // fallback) fails closed — an unaudited kernel is denied.
            if allowed {
                None
            } else {
                let q = interner.resolve(*module).unwrap_or("?");
                let n = interner.resolve(*name).unwrap_or("?");
                Some(deny(e.span, q, n))
            }
        }
        Expr_::ForeignCall { .. } => {
            // FFI bindings are a native-target concept: the client's only
            // host surface is the fixed web-sys allowlist.
            Some(deny(e.span, "Ffi", "binding"))
        }
        Expr_::VarLocal(_)
        | Expr_::VarTopLevel { .. }
        | Expr_::VarCtor { .. }
        | Expr_::Int(_)
        | Expr_::Float(_)
        | Expr_::Str(_)
        | Expr_::PathLit(_)
        | Expr_::Char(_)
        | Expr_::Unit => None,
        Expr_::Call(f, args) => first_denied(f, interner)
            .or_else(|| args.iter().find_map(|a| first_denied(a, interner))),
        Expr_::Case(scrut, branches) => first_denied(scrut, interner).or_else(|| {
            branches
                .iter()
                .find_map(|CaseBranch { body, .. }| first_denied(body, interner))
        }),
        Expr_::Lambda(_, body) => first_denied(body, interner),
        // Binops resolve to `Basics` arithmetic/comparison kernels —
        // pure, always client-representable.
        Expr_::Binop { lhs, rhs, .. } => {
            first_denied(lhs, interner).or_else(|| first_denied(rhs, interner))
        }
        Expr_::Let(bindings, body) => bindings
            .iter()
            .find_map(|LetBinding { body: b, .. }| first_denied(b, interner))
            .or_else(|| first_denied(body, interner)),
        Expr_::If(arms, els) => arms
            .iter()
            .find_map(|(c, b)| first_denied(c, interner).or_else(|| first_denied(b, interner)))
            .or_else(|| first_denied(els, interner)),
        Expr_::Tuple(items) | Expr_::List(items) => {
            items.iter().find_map(|i| first_denied(i, interner))
        }
        Expr_::Cons(h, t) => first_denied(h, interner).or_else(|| first_denied(t, interner)),
        Expr_::Record(fields) => fields.iter().find_map(|(_, v)| first_denied(v, interner)),
        Expr_::Access(base, _) => first_denied(base, interner),
        Expr_::Update(base, fields) => first_denied(base, interner)
            .or_else(|| fields.iter().find_map(|(_, v)| first_denied(v, interner))),
    }
}

/// Layer-1 red-team tests: the default-deny allowlist BLOCKS server-only
/// kernels and FFI under `--target wasm`; wasm-available kernels pass.
#[cfg(test)]
mod tests {
    use ipe_diagnostics::NameError;
    use ipe_kernels::StdlibKernel;

    use super::*;

    fn intern(interner: &mut Interner, s: &str) -> ipe_intern::Symbol {
        #[allow(clippy::expect_used)]
        interner.intern(s).expect("intern must succeed in a test")
    }

    /// A server-only kernel (`Db.query`, `id: None` — unregistered) is denied
    /// under `--target wasm`: it has no `WasmClient` denotation and must never
    /// reach the bundle (it would either fail cargo or leak a server secret).
    #[test]
    fn server_only_kernel_is_denied() {
        let mut interner = Interner::new();
        let ipe_db = intern(&mut interner, "Ipe.Db");
        let query = intern(&mut interner, "query");
        let body = Expr_::VarKernel {
            id: None,
            module: ipe_db,
            name: query,
        };
        let module = single_def_module(&mut interner, body);
        let err = check_wasm_client(&module, &interner)
            .expect_err("Ipe.Db.query must be denied under --target wasm");
        let (qualifier, name) = match err {
            Diagnostic::Name {
                msg: NameError::ServerOnlyKernelForWasm { qualifier, name },
                ..
            } => (qualifier, name),
            other => {
                return assert_eq!(
                    format!("{other:?}"),
                    "ServerOnlyKernelForWasm",
                    "expected ServerOnlyKernelForWasm"
                );
            }
        };
        assert_eq!(qualifier.as_ref(), "Ipe.Db");
        assert_eq!(name.as_ref(), "query");
    }

    /// A wasm-available kernel (`CmdNone`) carries a `WasmClient` denotation
    /// and must pass the Layer-1 check without error.
    #[test]
    fn wasm_available_kernel_passes() {
        let mut interner = Interner::new();
        let ipe_cmd = intern(&mut interner, "Ipe.Cmd");
        let none_sym = intern(&mut interner, "none");
        let body = Expr_::VarKernel {
            id: Some(StdlibKernel::CmdNone),
            module: ipe_cmd,
            name: none_sym,
        };
        let module = single_def_module(&mut interner, body);
        check_wasm_client(&module, &interner)
            .expect("Cmd.none is wasm-available and must pass the Layer-1 gate");
    }

    /// A foreign FFI call is always denied under `--target wasm` — the client's
    /// only host surface is the fixed web-sys allowlist, not arbitrary crates.
    #[test]
    fn ffi_call_is_denied() {
        let mut interner = Interner::new();
        let ident = intern(&mut interner, "native_lib_do_something");
        let body = Expr_::ForeignCall {
            ident,
            args: vec![],
            asserted: false,
        };
        let module = single_def_module(&mut interner, body);
        let err = check_wasm_client(&module, &interner)
            .expect_err("FFI calls must be denied under --target wasm");
        assert!(
            matches!(
                err,
                Diagnostic::Name {
                    msg: NameError::ServerOnlyKernelForWasm { .. },
                    ..
                }
            ),
            "expected ServerOnlyKernelForWasm, got {err:?}"
        );
    }

    /// A program with no kernel calls (pure unit body) passes the gate — the
    /// default-deny allowlist only fires on an EXPLICIT kernel reference.
    #[test]
    fn kernel_free_program_passes() {
        let mut interner = Interner::new();
        let module = single_def_module(&mut interner, Expr_::Unit);
        check_wasm_client(&module, &interner)
            .expect("a kernel-free program must pass the Layer-1 gate unconditionally");
    }

    /// Builds a denied `VarKernel` expression at the given byte-offset span.
    fn denied_kernel_at(interner: &mut Interner, lo: u32) -> Expr {
        use ipe_diagnostics::Located;
        let m = intern(interner, "Ipe.Db");
        let n = intern(interner, "query");
        Located::new(
            Span::new(lo, lo + 1),
            Expr_::VarKernel {
                id: None,
                module: m,
                name: n,
            },
        )
    }

    /// Builds a denied `VarKernel` with a distinct name to identify the offender.
    fn denied_kernel_named(interner: &mut Interner, lo: u32, name: &str) -> Expr {
        use ipe_diagnostics::Located;
        let m = intern(interner, "Ipe.Db");
        let n = intern(interner, name);
        Located::new(
            Span::new(lo, lo + 1),
            Expr_::VarKernel {
                id: None,
                module: m,
                name: n,
            },
        )
    }

    fn unit_at(lo: u32) -> Expr {
        use ipe_diagnostics::Located;
        Located::new(Span::new(lo, lo + 1), Expr_::Unit)
    }

    /// Extract the source span from a `Diagnostic::Name` or fail the test.
    fn name_span(d: &Diagnostic, context: &str) -> Span {
        if let Diagnostic::Name { span, .. } = d {
            *span
        } else {
            assert_eq!(
                format!("{d:?}"),
                "Diagnostic::Name { .. }",
                "{context}: expected Name diagnostic"
            );
            Span::DUMMY
        }
    }

    /// Three distinct denied kernels at ascending spans inside a `Call` argument
    /// list. Source order is arg0, arg1, arg2; a LIFO walk returns arg2 first.
    /// The gate must return the offender at arg0's span.
    #[test]
    fn call_reports_first_arg_offender_not_last() {
        use ipe_diagnostics::Located;
        let mut interner = Interner::new();
        let arg0 = denied_kernel_named(&mut interner, 10, "alpha");
        let arg1 = denied_kernel_named(&mut interner, 20, "beta");
        let arg2 = denied_kernel_named(&mut interner, 30, "gamma");
        let f_sym = intern(&mut interner, "f");
        let callee = Located::new(Span::new(0, 1), Expr_::VarLocal(f_sym));
        let body = Expr_::Call(Box::new(callee), vec![arg0, arg1, arg2]);
        let module = single_def_module(&mut interner, body);
        let err = check_wasm_client(&module, &interner)
            .expect_err("at least one denied kernel must be reported");
        let (name, span) = match err {
            Diagnostic::Name {
                span,
                msg: NameError::ServerOnlyKernelForWasm { name, .. },
            } => (name, span),
            other => {
                return assert_eq!(
                    format!("{other:?}"),
                    "ServerOnlyKernelForWasm",
                    "expected ServerOnlyKernelForWasm"
                );
            }
        };
        assert_eq!(
            name.as_ref(),
            "alpha",
            "first source-order arg (alpha) must be reported, not a later one"
        );
        assert_eq!(span.lo, 10, "span must match arg0");
    }

    /// `If` arms: the first condition's offender is reported before any body.
    #[test]
    fn if_reports_first_condition_offender() {
        let mut interner = Interner::new();
        let c0 = denied_kernel_at(&mut interner, 10);
        let b0 = unit_at(20);
        let c1 = denied_kernel_at(&mut interner, 30);
        let b1 = unit_at(40);
        let els = unit_at(50);
        let body = Expr_::If(vec![(c0, b0), (c1, b1)], Box::new(els));
        let module = single_def_module(&mut interner, body);
        let err =
            check_wasm_client(&module, &interner).expect_err("first condition must be denied");
        let span = name_span(&err, "if_reports_first_condition_offender");
        assert_eq!(
            span.lo, 10,
            "must report the first (lo=10) condition, not lo=30"
        );
    }

    /// `Let` bindings: the first binding body offender is reported before
    /// subsequent bindings and before the let-body.
    #[test]
    fn let_reports_first_binding_offender() {
        use crate::ast::Pattern_;
        use ipe_diagnostics::Located;
        let mut interner = Interner::new();
        let v0 = intern(&mut interner, "x");
        let v1 = intern(&mut interner, "y");
        let bind0 = LetBinding {
            pat: Located::new(Span::DUMMY, Pattern_::PVar(v0)),
            body: denied_kernel_at(&mut interner, 10),
        };
        let bind1 = LetBinding {
            pat: Located::new(Span::DUMMY, Pattern_::PVar(v1)),
            body: denied_kernel_at(&mut interner, 20),
        };
        let let_body = unit_at(30);
        let body = Expr_::Let(vec![bind0, bind1], Box::new(let_body));
        let module = single_def_module(&mut interner, body);
        let err = check_wasm_client(&module, &interner).expect_err("first binding must be denied");
        let span = name_span(&err, "let_reports_first_binding_offender");
        assert_eq!(span.lo, 10, "must report first binding (lo=10), not lo=20");
    }

    /// `Tuple` and `List`: first element in declaration order is reported.
    #[test]
    fn tuple_and_list_report_first_field_offender() {
        let mut interner = Interner::new();

        let t0 = denied_kernel_at(&mut interner, 10);
        let t1 = denied_kernel_at(&mut interner, 20);
        let body = Expr_::Tuple(vec![t0, t1]);
        let module = single_def_module(&mut interner, body);
        let err =
            check_wasm_client(&module, &interner).expect_err("first tuple field must be denied");
        let span = name_span(&err, "tuple");
        assert_eq!(span.lo, 10, "tuple: first field (lo=10) must be reported");

        let l0 = denied_kernel_at(&mut interner, 10);
        let l1 = denied_kernel_at(&mut interner, 20);
        let body2 = Expr_::List(vec![l0, l1]);
        let module2 = single_def_module(&mut interner, body2);
        let err2 =
            check_wasm_client(&module2, &interner).expect_err("first list element must be denied");
        let span2 = name_span(&err2, "list");
        assert_eq!(span2.lo, 10, "list: first element (lo=10) must be reported");
    }

    /// `Record`: first field value offender is reported.
    #[test]
    fn record_reports_first_field_offender() {
        let mut interner = Interner::new();
        let f0 = intern(&mut interner, "foo");
        let f1 = intern(&mut interner, "bar");
        let v0 = denied_kernel_at(&mut interner, 10);
        let v1 = denied_kernel_at(&mut interner, 20);
        let body = Expr_::Record(vec![(f0, v0), (f1, v1)]);
        let module = single_def_module(&mut interner, body);
        let err =
            check_wasm_client(&module, &interner).expect_err("first record field must be denied");
        let span = name_span(&err, "record_reports_first_field_offender");
        assert_eq!(span.lo, 10, "record: first field (lo=10) must be reported");
    }

    /// `Update`: base is checked before update fields.
    #[test]
    fn update_reports_base_before_fields() {
        let mut interner = Interner::new();
        let f0 = intern(&mut interner, "foo");
        let base = denied_kernel_at(&mut interner, 10);
        let fv = denied_kernel_at(&mut interner, 20);
        let body = Expr_::Update(Box::new(base), vec![(f0, fv)]);
        let module = single_def_module(&mut interner, body);
        let err = check_wasm_client(&module, &interner).expect_err("base must be denied first");
        let span = name_span(&err, "update_reports_base_before_fields");
        assert_eq!(
            span.lo, 10,
            "update: base (lo=10) must precede field (lo=20)"
        );
    }

    /// Nested `Call` inside an `If` arm body: the recursive walk correctly
    /// descends in source order and finds the earliest offender in a mixed tree.
    #[test]
    fn nested_deep_body_still_finds_first() {
        use ipe_diagnostics::Located;
        let mut interner = Interner::new();

        // If [(clean_cond, Call(f, [denied@lo5, denied@lo15]))] Unit
        let clean_cond = unit_at(0);
        let f_sym = intern(&mut interner, "f");
        let callee = Located::new(Span::new(2, 3), Expr_::VarLocal(f_sym));
        let arg0 = denied_kernel_at(&mut interner, 5);
        let arg1 = denied_kernel_at(&mut interner, 15);
        let call_body = Located::new(
            Span::new(2, 16),
            Expr_::Call(Box::new(callee), vec![arg0, arg1]),
        );
        let body = Expr_::If(vec![(clean_cond, call_body)], Box::new(unit_at(50)));
        let module = single_def_module(&mut interner, body);
        let err = check_wasm_client(&module, &interner)
            .expect_err("denied kernel inside nested call must be found");
        let span = name_span(&err, "nested_deep_body_still_finds_first");
        assert_eq!(span.lo, 5, "first offender is the call arg at lo=5");
    }
}
