//! Name resolution: `sky_syntax` source tree → canonical AST. Port of the M0
//! subset of `Sky.Canonicalise.{Module,Expression,Pattern,Type}`.

use std::collections::BTreeSet;

use sky_diagnostics::{Diagnostic, DResult, Located, NameError, Span};
use sky_intern::{Interner, Symbol};
use sky_syntax as src;

use crate::ast as canon;
use crate::env::{CtorHome, Env, VarHome};

/// Canonicalise a parsed module into its name-resolved form.
///
/// # Errors
/// Returns [`Diagnostic::Name`] with [`NameError::Unknown`] for any name that
/// resolves to neither a constructor, a bound variable, a top-level binding,
/// nor a kernel function.
pub fn canonicalise(m: &src::Module, interner: &mut Interner) -> DResult<canon::Module> {
    let home = m.name.value.clone();
    let mut env = Env::initial(home.clone(), interner);

    // Collect the local union names first; the type canonicaliser sets a
    // constructor application's home to this module only for these names.
    let local_union_names: BTreeSet<Symbol> =
        m.unions.iter().map(|u| u.value.name.value).collect();

    // Register unions + their constructors into the environment.
    let mut unions = Vec::with_capacity(m.unions.len());
    for u in &m.unions {
        let union = register_union(&u.value, &home, &mut env);
        unions.push(union);
    }

    // Register every top-level value name so bindings can be referenced before
    // their definition (mutual / forward references).
    for v in &m.values {
        env.vars.insert(v.value.name.value, VarHome::TopLevel(home.clone()));
    }

    // Canonicalise each value declaration.
    let mut defs = Vec::with_capacity(m.values.len());
    for v in &m.values {
        defs.push(canonicalise_value(&v.value, &env, &local_union_names, interner)?);
    }

    Ok(canon::Module { name: home, unions, defs })
}

/// Register a union and its constructors into the environment, returning the
/// canonical union record.
fn register_union(u: &src::Union, home: &[Symbol], env: &mut Env) -> canon::Union {
    let type_name = u.name.value;
    let mut ctors = Vec::with_capacity(u.ctors.len());
    for (index, c) in u.ctors.iter().enumerate() {
        let name = c.value.name;
        let arity = c.value.args.len();
        env.ctors.insert(
            name,
            CtorHome { home: home.to_vec(), type_name, name, index, arity },
        );
        ctors.push(canon::Ctor { name, index, arity });
    }
    canon::Union { name: type_name, ctors }
}

/// Canonicalise a single top-level value declaration.
fn canonicalise_value(
    val: &src::Value,
    env: &Env,
    local_union_names: &BTreeSet<Symbol>,
    interner: &mut Interner,
) -> DResult<canon::Def> {
    // Add parameter-bound names to a body-local environment.
    let mut body_env = env.clone();
    for p in &val.patterns {
        bind_pattern_names(&p.value, &mut body_env);
    }

    let mut patterns = Vec::with_capacity(val.patterns.len());
    for p in &val.patterns {
        patterns.push(canonicalise_pattern(p, env)?);
    }
    let body = canonicalise_expr(&val.body, &body_env, interner)?;

    match &val.type_annotation {
        None => Ok(canon::Def::Untyped { name: val.name, patterns, body }),
        Some(ann) => {
            let mut free_vars = BTreeSet::new();
            let ty = canonicalise_type(&ann.value, env, local_union_names, &mut free_vars);
            Ok(canon::Def::Typed {
                name: val.name,
                free_vars: free_vars.into_iter().collect(),
                patterns,
                body,
                ty,
            })
        }
    }
}

/// Bind every variable a pattern introduces as a local in the environment.
fn bind_pattern_names(p: &src::Pattern_, env: &mut Env) {
    match p {
        src::Pattern_::PAnything => {}
        src::Pattern_::PVar(name) => env.add_local(*name),
        src::Pattern_::PCtor(_, _, args) => {
            for a in args {
                bind_pattern_names(&a.value, env);
            }
        }
    }
}

/// Canonicalise a pattern. M0 supports wildcard, var, and constructor patterns.
fn canonicalise_pattern(p: &src::Pattern, env: &Env) -> DResult<canon::Pattern> {
    let span = p.span;
    let node = match &p.value {
        src::Pattern_::PAnything => canon::Pattern_::PAnything,
        src::Pattern_::PVar(name) => canon::Pattern_::PVar(*name),
        src::Pattern_::PCtor(name, _, args) => {
            let ctor = env.lookup_ctor(*name).ok_or(Diagnostic::Name {
                span,
                msg: NameError::Unknown,
            })?;
            let home = ctor.home.clone();
            let type_name = ctor.type_name;
            let index = ctor.index;
            let mut can_args = Vec::with_capacity(args.len());
            for a in args {
                can_args.push(canonicalise_pattern(a, env)?);
            }
            canon::Pattern_::PCtor {
                home,
                type_name,
                name: *name,
                index,
                args: can_args,
            }
        }
    };
    Ok(Located::new(span, node))
}

/// Canonicalise an expression, resolving every name.
fn canonicalise_expr(
    e: &src::Expr,
    env: &Env,
    interner: &mut Interner,
) -> DResult<canon::Expr> {
    let span = e.span;
    let node = match &e.value {
        src::Expr_::Int(n) => canon::Expr_::Int(*n),
        src::Expr_::VarLocal(name) => resolve_var(*name, span, env)?,
        src::Expr_::VarQual(qual, name) => resolve_qual_var(*qual, *name, span, env)?,
        src::Expr_::Call(f, args) => {
            let callee = canonicalise_expr(f, env, interner)?;
            let mut can_args = Vec::with_capacity(args.len());
            for a in args {
                can_args.push(canonicalise_expr(a, env, interner)?);
            }
            canon::Expr_::Call(Box::new(callee), can_args)
        }
        src::Expr_::Case(scrut, arms) => {
            let can_scrut = canonicalise_expr(scrut, env, interner)?;
            let mut branches = Vec::with_capacity(arms.len());
            for (pat, body) in arms {
                // Pattern-bound names are local in the arm body.
                let mut arm_env = env.clone();
                bind_pattern_names(&pat.value, &mut arm_env);
                let can_pat = canonicalise_pattern(pat, env)?;
                let can_body = canonicalise_expr(body, &arm_env, interner)?;
                branches.push(canon::CaseBranch { pat: can_pat, body: can_body });
            }
            canon::Expr_::Case(Box::new(can_scrut), branches)
        }
        src::Expr_::Binops(pairs, final_) => {
            canonicalise_binops(pairs, final_, env, interner)?
        }
    };
    Ok(Located::new(span, node))
}

/// Resolve a bare name: constructor first, then variable. Unknown → error.
fn resolve_var(name: Symbol, span: Span, env: &Env) -> DResult<canon::Expr_> {
    if let Some(ctor) = env.lookup_ctor(name) {
        return Ok(canon::Expr_::VarCtor {
            home: ctor.home.clone(),
            type_name: ctor.type_name,
            name: ctor.name,
            index: ctor.index,
        });
    }
    match env.lookup_var(name) {
        Some(VarHome::Local) => Ok(canon::Expr_::VarLocal(name)),
        Some(VarHome::TopLevel(module)) => {
            Ok(canon::Expr_::VarTopLevel { module: module.clone(), name })
        }
        Some(VarHome::Kernel(m, f)) => {
            Ok(canon::Expr_::VarKernel { module: *m, name: *f })
        }
        None => Err(Diagnostic::Name { span, msg: NameError::Unknown }),
    }
}

/// Resolve a qualified name `Qualifier.name`. Unknown → error.
fn resolve_qual_var(
    qualifier: Symbol,
    name: Symbol,
    span: Span,
    env: &Env,
) -> DResult<canon::Expr_> {
    match env.lookup_qual_var(qualifier, name) {
        Some(VarHome::Kernel(m, f)) => Ok(canon::Expr_::VarKernel { module: *m, name: *f }),
        Some(VarHome::TopLevel(module)) => {
            Ok(canon::Expr_::VarTopLevel { module: module.clone(), name })
        }
        Some(VarHome::Local) => Ok(canon::Expr_::VarLocal(name)),
        None => Err(Diagnostic::Name { span, msg: NameError::Unknown }),
    }
}

/// Canonicalise a binary-operator chain.
///
/// M0 limitation: operators are folded left-associatively. The M0 grammar only
/// exposes `+` and `-` (both left-associative, equal precedence), for which a
/// left fold is exactly correct. Full precedence climbing (per
/// `Sky.Parse.Symbol.precedence`) arrives with the operator table in a later
/// milestone.
fn canonicalise_binops(
    pairs: &[(src::Expr, Located<Symbol>)],
    final_: &src::Expr,
    env: &Env,
    interner: &mut Interner,
) -> DResult<canon::Expr_> {
    let basics = interner.intern("Basics");

    // Fold left: (((operand0 op0 operand1) op1 operand2) ...) opN final.
    let mut iter = pairs.iter();
    let Some((first_src, first_op)) = iter.next() else {
        // No operators: just the final operand.
        return Ok(canonicalise_expr(final_, env, interner)?.value);
    };

    let mut acc_expr = canonicalise_expr(first_src, env, interner)?;
    let mut pending_op = *first_op;

    for (operand, op) in iter {
        let rhs = canonicalise_expr(operand, env, interner)?;
        acc_expr = combine_binop(acc_expr, pending_op, rhs, basics, interner);
        pending_op = *op;
    }

    let rhs = canonicalise_expr(final_, env, interner)?;
    Ok(combine_binop(acc_expr, pending_op, rhs, basics, interner).value)
}

/// Build a single resolved binary-operation node.
fn combine_binop(
    lhs: canon::Expr,
    op: Located<Symbol>,
    rhs: canon::Expr,
    basics: Symbol,
    interner: &mut Interner,
) -> canon::Expr {
    let func = resolve_op_func(op.value, interner);
    let span = Span::new(lhs.span.lo, rhs.span.hi);
    Located::new(
        span,
        canon::Expr_::Binop {
            op: op.value,
            home: basics,
            func,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
    )
}

/// Map an operator symbol to its kernel function name. M0 subset of
/// `Expression.resolveOpName`.
fn resolve_op_func(op: Symbol, interner: &mut Interner) -> Symbol {
    let func: Option<&'static str> = match interner.resolve(op) {
        "+" => Some("add"),
        "-" => Some("sub"),
        "*" => Some("mul"),
        "/" => Some("fdiv"),
        "//" => Some("idiv"),
        "==" => Some("eq"),
        "/=" => Some("neq"),
        "<" => Some("lt"),
        ">" => Some("gt"),
        "<=" => Some("le"),
        ">=" => Some("ge"),
        "&&" => Some("and"),
        "||" => Some("or"),
        "++" => Some("append"),
        // Unknown operators map to their own name under Basics, matching the
        // Haskell fall-through (`_ -> Can.VarKernel "Basics" op`).
        _ => None,
    };
    // The immutable borrow above ends here, so interning is now permitted.
    func.map_or(op, |name| interner.intern(name))
}

/// Canonicalise a type annotation. M0 subset of `Canonicalise.Type`.
fn canonicalise_type(
    t: &src::TypeAnnotation,
    env: &Env,
    local_union_names: &BTreeSet<Symbol>,
    free_vars: &mut BTreeSet<Symbol>,
) -> canon::Type {
    match t {
        src::TypeAnnotation::TLambda(a, b) => canon::Type::Lambda(
            Box::new(canonicalise_type(a, env, local_union_names, free_vars)),
            Box::new(canonicalise_type(b, env, local_union_names, free_vars)),
        ),
        src::TypeAnnotation::TVar(v) => {
            free_vars.insert(*v);
            canon::Type::Var(*v)
        }
        src::TypeAnnotation::TType(_, segments, args) => {
            let name = segments.last().copied().unwrap_or_else(|| {
                // An unnamed type cannot occur in the M0 grammar; fall back to
                // the home module's name so the node is still well-formed.
                env.home.last().copied().unwrap_or_else(name_zero)
            });
            let home = if local_union_names.contains(&name) {
                env.home.clone()
            } else {
                Vec::new()
            };
            let can_args = args
                .iter()
                .map(|a| canonicalise_type(a, env, local_union_names, free_vars))
                .collect();
            canon::Type::Con { home, name, args: can_args }
        }
    }
}

/// The interned symbol for the empty string (symbol id 0 is never guaranteed,
/// so we cannot hardcode it). Used only on the unreachable unnamed-type path.
const fn name_zero() -> Symbol {
    Symbol::from_raw(0)
}
