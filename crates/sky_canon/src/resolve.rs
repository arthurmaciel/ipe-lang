//! Name resolution: `sky_syntax` source tree → canonical AST. Port of the M0
//! subset of `Sky.Canonicalise.{Module,Expression,Pattern,Type}`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use sky_diagnostics::{DResult, Diagnostic, Feature, Located, LowerError, NameError, Span};
use sky_intern::{Interner, Symbol};
use sky_syntax as src;

use crate::ast as canon;
use crate::env::{CtorHome, Env, VarHome};

/// The maximum number of `did you mean` suggestions attached to an unresolved
/// name. Keeping it small prevents a wall of near-misses drowning the actual
/// error; the list is `(Levenshtein, name)`-sorted so the closest comes first.
const MAX_SUGGESTIONS: usize = 3;

/// The inclusive edit-distance ceiling for a suggestion. Mirrors the Haskell
/// reference (`Sky.Canonicalise.Module.suggestQualifier`): beyond two edits a
/// "did you mean" is more misleading than helpful, so silence wins.
const SUGGESTION_MAX_DISTANCE: usize = 2;

/// Canonicalise a parsed module into its name-resolved form.
///
/// # Errors
/// Returns [`Diagnostic::Name`] for any name that resolves to neither a
/// constructor, a bound variable, a top-level binding, nor a kernel function
/// ([`NameError::ValueNotFound`] / [`NameError::ConstructorNotFound`] /
/// [`NameError::UnknownModule`] / [`NameError::NoSuchMember`], each with a
/// deterministic did-you-mean), or for a duplicated value/constructor/type name
/// ([`NameError::DuplicateValue`] / [`NameError::DuplicateConstructor`] /
/// [`NameError::DuplicateType`]). [`Diagnostic::CompilerBug`] if the interner's
/// symbol table is exhausted or a name symbol is not interned.
pub fn canonicalise(m: &src::Module, interner: &mut Interner) -> DResult<canon::Module> {
    let home = m.name.value.clone();
    let mut env = Env::initial(home.clone(), interner)?;

    // Collect the local union names first; the type canonicaliser sets a
    // constructor application's home to this module only for these names.
    let local_union_names: BTreeSet<Symbol> = m.unions.iter().map(|u| u.value.name.value).collect();

    // Register unions + their constructors into the environment, rejecting any
    // duplicate type or constructor name (closes the silent last-wins that the
    // bare `insert` used to hide).
    let mut unions = Vec::with_capacity(m.unions.len());
    let mut seen_types: BTreeMap<Symbol, Span> = BTreeMap::new();
    let mut seen_ctors: BTreeMap<Symbol, Span> = BTreeMap::new();
    for u in &m.unions {
        let type_name = u.value.name.value;
        let type_span = u.value.name.span;
        if let Some(&first) = seen_types.get(&type_name) {
            return Err(Diagnostic::Name {
                span: type_span,
                msg: NameError::DuplicateType {
                    name: name_str(interner, type_name)?,
                    first,
                },
            });
        }
        seen_types.insert(type_name, type_span);
        let union = register_union(&u.value, &home, &mut env, &mut seen_ctors, interner)?;
        unions.push(union);
    }

    // Collect type aliases. M1 supports the non-parametric form only — a declared
    // type parameter is a not-yet-supported feature (SKY-L0109). An alias name
    // that collides with a union (or another alias) is a duplicate type name. The
    // aliased bodies are kept as source annotations and expanded in-place at every
    // use site by `canonicalise_type`, so no later stage ever sees an alias.
    let mut aliases: BTreeMap<Symbol, src::TypeAnnotation> = BTreeMap::new();
    for a in &m.aliases {
        if let Some(var) = a.value.vars.first() {
            return Err(Diagnostic::Lower {
                span: var.span,
                msg: LowerError::Unsupported(Feature::ParametricAliases),
            });
        }
        let alias_name = a.value.name.value;
        let alias_span = a.value.name.span;
        if let Some(&first) = seen_types.get(&alias_name) {
            return Err(Diagnostic::Name {
                span: alias_span,
                msg: NameError::DuplicateType {
                    name: name_str(interner, alias_name)?,
                    first,
                },
            });
        }
        seen_types.insert(alias_name, alias_span);
        aliases.insert(alias_name, a.value.body.value.clone());
    }

    // Register every top-level value name so bindings can be referenced before
    // their definition (mutual / forward references), rejecting duplicates.
    let mut seen_values: BTreeMap<Symbol, Span> = BTreeMap::new();
    for v in &m.values {
        let name = v.value.name.value;
        let name_span = v.value.name.span;
        if let Some(&first) = seen_values.get(&name) {
            return Err(Diagnostic::Name {
                span: name_span,
                msg: NameError::DuplicateValue {
                    name: name_str(interner, name)?,
                    first,
                },
            });
        }
        seen_values.insert(name, name_span);
        env.vars.insert(name, VarHome::TopLevel(home.clone()));
    }

    // Canonicalise each value declaration.
    let mut defs = Vec::with_capacity(m.values.len());
    for v in &m.values {
        defs.push(canonicalise_value(
            &v.value,
            &env,
            &local_union_names,
            &aliases,
            interner,
        )?);
    }

    Ok(canon::Module {
        name: home,
        unions,
        defs,
    })
}

/// Register a union and its constructors into the environment, returning the
/// canonical union record.
///
/// `seen_ctors` carries every constructor name already registered across the
/// module so a name reused in the same or a different union is rejected
/// ([`NameError::DuplicateConstructor`]) instead of silently overwriting the
/// earlier `env.ctors` entry.
///
/// # Errors
/// [`NameError::DuplicateConstructor`] on a repeated constructor name;
/// [`Diagnostic::CompilerBug`] if a constructor symbol is not interned.
fn register_union(
    u: &src::Union,
    home: &[Symbol],
    env: &mut Env,
    seen_ctors: &mut BTreeMap<Symbol, Span>,
    interner: &Interner,
) -> DResult<canon::Union> {
    let type_name = u.name.value;
    let mut ctors = Vec::with_capacity(u.ctors.len());
    for (index, c) in u.ctors.iter().enumerate() {
        let name = c.value.name;
        let arity = c.value.args.len();
        if let Some(&first) = seen_ctors.get(&name) {
            return Err(Diagnostic::Name {
                span: c.span,
                msg: NameError::DuplicateConstructor {
                    name: name_str(interner, name)?,
                    first,
                },
            });
        }
        seen_ctors.insert(name, c.span);
        env.ctors.insert(
            name,
            CtorHome {
                home: home.to_vec(),
                type_name,
                name,
                index,
                arity,
            },
        );
        ctors.push(canon::Ctor { name, index, arity });
    }
    Ok(canon::Union {
        name: type_name,
        ctors,
    })
}

/// Canonicalise a single top-level value declaration.
fn canonicalise_value(
    val: &src::Value,
    env: &Env,
    local_union_names: &BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, src::TypeAnnotation>,
    interner: &mut Interner,
) -> DResult<canon::Def> {
    // Add parameter-bound names to a body-local environment.
    let mut body_env = env.clone();
    for p in &val.patterns {
        bind_pattern_names(&p.value, &mut body_env);
    }

    let mut patterns = Vec::with_capacity(val.patterns.len());
    for p in &val.patterns {
        patterns.push(canonicalise_pattern(p, env, interner)?);
    }
    let body = canonicalise_expr(&val.body, &body_env, interner)?;

    match &val.type_annotation {
        None => Ok(canon::Def::Untyped {
            name: val.name,
            patterns,
            body,
        }),
        Some(ann) => {
            let mut free_vars = BTreeSet::new();
            let mut visited = Vec::new();
            let ty = canonicalise_type(
                &ann.value,
                env,
                local_union_names,
                aliases,
                &mut free_vars,
                &mut visited,
            );
            // Order the quantified type variables by their resolved NAME, not by
            // `Symbol` id (intern order is allocation-dependent, hence not a
            // stable wire order). Determinism gate: a multi-tyvar annotation
            // must yield the same `free_vars` regardless of how the interner
            // happened to number the names.
            let mut free_vars: Vec<Symbol> = free_vars.into_iter().collect();
            free_vars.sort_by(|a, b| interner.resolve(*a).cmp(&interner.resolve(*b)));
            Ok(canon::Def::Typed {
                name: val.name,
                free_vars,
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
fn canonicalise_pattern(
    p: &src::Pattern,
    env: &Env,
    interner: &Interner,
) -> DResult<canon::Pattern> {
    let span = p.span;
    let node = match &p.value {
        src::Pattern_::PAnything => canon::Pattern_::PAnything,
        src::Pattern_::PVar(name) => canon::Pattern_::PVar(*name),
        src::Pattern_::PCtor(name, _, args) => {
            let Some(ctor) = env.lookup_ctor(*name) else {
                return Err(Diagnostic::Name {
                    span,
                    msg: NameError::ConstructorNotFound {
                        name: name_str(interner, *name)?,
                        suggestions: suggestions(*name, env.ctors.keys().copied(), interner),
                    },
                });
            };
            let home = ctor.home.clone();
            let type_name = ctor.type_name;
            let index = ctor.index;
            let mut can_args = Vec::with_capacity(args.len());
            for a in args {
                can_args.push(canonicalise_pattern(a, env, interner)?);
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
fn canonicalise_expr(e: &src::Expr, env: &Env, interner: &mut Interner) -> DResult<canon::Expr> {
    let span = e.span;
    let node = match &e.value {
        src::Expr_::Int(n) => canon::Expr_::Int(*n),
        src::Expr_::VarLocal(name) => resolve_var(*name, span, env, interner)?,
        src::Expr_::VarQual(qual, name) => resolve_qual_var(*qual, *name, span, env, interner)?,
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
                let can_pat = canonicalise_pattern(pat, env, interner)?;
                let can_body = canonicalise_expr(body, &arm_env, interner)?;
                branches.push(canon::CaseBranch {
                    pat: can_pat,
                    body: can_body,
                });
            }
            canon::Expr_::Case(Box::new(can_scrut), branches)
        }
        src::Expr_::Binops(pairs, final_) => canonicalise_binops(pairs, final_, env, interner)?,
        src::Expr_::Let(bindings, body) => {
            // Sequential (`let*`) scoping: each binding's value is resolved
            // against the enclosing scope plus the bindings before it, then its
            // name becomes a local for the bindings that follow and for the
            // `in` body. This matches the non-recursive nested-`Let` the lowerer
            // emits, so a self- or forward-reference resolves to an outer name or
            // fails cleanly (`ValueNotFound`) rather than miscompiling.
            let mut let_env = env.clone();
            let mut can_bindings = Vec::with_capacity(bindings.len());
            for b in bindings {
                let can_body = canonicalise_expr(&b.body, &let_env, interner)?;
                let_env.add_local(b.name.value);
                can_bindings.push(canon::LetBinding {
                    name: b.name,
                    body: can_body,
                });
            }
            let can_in = canonicalise_expr(body, &let_env, interner)?;
            canon::Expr_::Let(can_bindings, Box::new(can_in))
        }
        src::Expr_::If(branches, else_expr) => {
            // `if` introduces no bindings: every condition and branch resolves
            // against the same enclosing scope.
            let mut can_branches = Vec::with_capacity(branches.len());
            for (cond, body) in branches {
                let can_cond = canonicalise_expr(cond, env, interner)?;
                let can_body = canonicalise_expr(body, env, interner)?;
                can_branches.push((can_cond, can_body));
            }
            let can_else = canonicalise_expr(else_expr, env, interner)?;
            canon::Expr_::If(can_branches, Box::new(can_else))
        }
        src::Expr_::Tuple(elems) => {
            // A tuple introduces no bindings: every element resolves against the
            // same enclosing scope.
            let mut can_elems = Vec::with_capacity(elems.len());
            for elem in elems {
                can_elems.push(canonicalise_expr(elem, env, interner)?);
            }
            canon::Expr_::Tuple(can_elems)
        }
    };
    Ok(Located::new(span, node))
}

/// Resolve a bare name: constructor first, then variable. Unknown → error.
fn resolve_var(name: Symbol, span: Span, env: &Env, interner: &Interner) -> DResult<canon::Expr_> {
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
        Some(VarHome::TopLevel(module)) => Ok(canon::Expr_::VarTopLevel {
            module: module.clone(),
            name,
        }),
        Some(VarHome::Kernel(m, f)) => Ok(canon::Expr_::VarKernel {
            module: *m,
            name: *f,
        }),
        // A bare value name can resolve to either a value binding or a
        // constructor used as a value, so the suggestion pool spans both
        // namespaces (value bindings first, then constructor names).
        None => Err(Diagnostic::Name {
            span,
            msg: NameError::ValueNotFound {
                name: name_str(interner, name)?,
                suggestions: suggestions(
                    name,
                    env.vars.keys().chain(env.ctors.keys()).copied(),
                    interner,
                ),
            },
        }),
    }
}

/// Resolve a qualified name `Qualifier.name`. Distinguishes an unknown
/// qualifier ([`NameError::UnknownModule`]) from a known qualifier missing the
/// member ([`NameError::NoSuchMember`]).
fn resolve_qual_var(
    qualifier: Symbol,
    name: Symbol,
    span: Span,
    env: &Env,
    interner: &Interner,
) -> DResult<canon::Expr_> {
    let Some(members) = env.qual_members(qualifier) else {
        // The qualifier itself is unknown: suggest from the known qualifiers
        // (kernel modules + import aliases).
        return Err(Diagnostic::Name {
            span,
            msg: NameError::UnknownModule {
                qualifier: name_str(interner, qualifier)?,
                suggestions: suggestions(qualifier, env.qual_vars.keys().copied(), interner),
            },
        });
    };
    match members.get(&name) {
        Some(VarHome::Kernel(m, f)) => Ok(canon::Expr_::VarKernel {
            module: *m,
            name: *f,
        }),
        Some(VarHome::TopLevel(module)) => Ok(canon::Expr_::VarTopLevel {
            module: module.clone(),
            name,
        }),
        Some(VarHome::Local) => Ok(canon::Expr_::VarLocal(name)),
        // The qualifier resolves but the member is absent: suggest from this
        // module's members.
        None => Err(Diagnostic::Name {
            span,
            msg: NameError::NoSuchMember {
                module: name_str(interner, qualifier)?,
                member: name_str(interner, name)?,
                suggestions: suggestions(name, members.keys().copied(), interner),
            },
        }),
    }
}

/// Operator associativity. Mirrors `Sky.Parse.Symbol.Assoc`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Assoc {
    Left,
    Right,
    None,
}

/// The precedence (higher binds tighter) and associativity of `op`.
///
/// Mirror of the Haskell reference `Sky.Parse.Symbol.precedence` for the
/// M1-core operator set; any operator outside the set defaults to `9 L` exactly
/// as the Haskell catch-all does.
const fn op_precedence(op: &str) -> (i32, Assoc) {
    match op.as_bytes() {
        b"*" | b"/" | b"//" | b"%" => (7, Assoc::Left),
        b"+" | b"-" => (6, Assoc::Left),
        b"++" => (5, Assoc::Right),
        b"==" | b"/=" | b"<" | b">" | b"<=" | b">=" => (4, Assoc::None),
        b"&&" => (3, Assoc::Right),
        b"||" => (2, Assoc::Right),
        _ => (9, Assoc::Left),
    }
}

/// Canonicalise a binary-operator chain into a precedence-correct tree.
///
/// The parser records a chain `e0 op0 e1 op1 … opN-1 eN` as a *flat* list of
/// `(operand, operator)` pairs plus a trailing operand, without consulting
/// precedence. Here we re-associate it via precedence climbing (port of
/// `Sky.Canonicalise.Expression.canonicaliseBinops`), reading each operator's
/// precedence + associativity from [`op_precedence`].
///
/// Unlike the Haskell parser — which nests `Src.Binops` pairwise and so needs a
/// flattening pre-pass — the Rust parser already emits one flat chain per
/// syntactic level. A `Binop` *operand* therefore only ever arises from an
/// explicit parenthesised group, which must stay atomic; we never re-flatten
/// it, so the user's grouping is preserved.
fn canonicalise_binops(
    pairs: &[(src::Expr, Located<Symbol>)],
    final_: &src::Expr,
    env: &Env,
    interner: &mut Interner,
) -> DResult<canon::Expr_> {
    // No operators: just the final operand.
    if pairs.is_empty() {
        return Ok(canonicalise_expr(final_, env, interner)?.value);
    }

    let basics = interner.intern("Basics")?;

    // Canonicalise every operand once, left to right, into a front-poppable
    // queue; pair each operator with its precedence + associativity.
    let mut operands: VecDeque<canon::Expr> = VecDeque::with_capacity(pairs.len() + 1);
    let mut ops: VecDeque<(Located<Symbol>, i32, Assoc)> = VecDeque::with_capacity(pairs.len());
    for (operand, op) in pairs {
        operands.push_back(canonicalise_expr(operand, env, interner)?);
        let (prec, assoc) = op_precedence(name_or_empty(interner, op.value));
        ops.push_back((*op, prec, assoc));
    }
    operands.push_back(canonicalise_expr(final_, env, interner)?);

    let left = operands
        .pop_front()
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_canon::canonicalise_binops",
            detail: "binop chain with operators but no operands".to_owned(),
        })?;
    let tree = climb_binops(0, left, &mut operands, &mut ops, basics, interner)?;
    Ok(tree.value)
}

/// Precedence-climbing core. Consumes operators of precedence ≥ `min_prec` from
/// the front of `ops`, each paired with the next operand from `operands`, and
/// folds them around `left`. Direct port of the Haskell `climb` helper.
fn climb_binops(
    min_prec: i32,
    mut left: canon::Expr,
    operands: &mut VecDeque<canon::Expr>,
    ops: &mut VecDeque<(Located<Symbol>, i32, Assoc)>,
    basics: Symbol,
    interner: &mut Interner,
) -> DResult<canon::Expr> {
    while let Some(&(op, prec, assoc)) = ops.front() {
        if prec < min_prec {
            break;
        }
        ops.pop_front();
        let next_operand = operands
            .pop_front()
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_canon::climb_binops",
                detail: "operator without a right operand".to_owned(),
            })?;
        // A left- (or non-) associative operator restricts its right subtree to
        // strictly-higher precedence; a right-associative one admits equal
        // precedence so it nests rightward.
        let next_min = match assoc {
            Assoc::Left | Assoc::None => prec + 1,
            Assoc::Right => prec,
        };
        let right = climb_binops(next_min, next_operand, operands, ops, basics, interner)?;
        left = combine_binop(left, op, right, basics, interner)?;
    }
    Ok(left)
}

/// Resolve an operator symbol to its text, or `""` when (impossibly) un-interned
/// — the empty string falls through [`op_precedence`] to the `9 L` default, so a
/// missing symbol degrades gracefully rather than panicking.
fn name_or_empty(interner: &Interner, sym: Symbol) -> &str {
    interner.resolve(sym).unwrap_or("")
}

/// Build a single resolved binary-operation node.
fn combine_binop(
    lhs: canon::Expr,
    op: Located<Symbol>,
    rhs: canon::Expr,
    basics: Symbol,
    interner: &mut Interner,
) -> DResult<canon::Expr> {
    let func = resolve_op_func(op.value, interner)?;
    let span = Span::new(lhs.span.lo, rhs.span.hi);
    Ok(Located::new(
        span,
        canon::Expr_::Binop {
            op: op.value,
            home: basics,
            func,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
    ))
}

/// Map an operator symbol to its kernel function name. M0 subset of
/// `Expression.resolveOpName`.
fn resolve_op_func(op: Symbol, interner: &mut Interner) -> DResult<Symbol> {
    let func: Option<&'static str> = match interner.resolve(op) {
        Some("+") => Some("add"),
        Some("-") => Some("sub"),
        Some("*") => Some("mul"),
        Some("/") => Some("fdiv"),
        Some("//") => Some("idiv"),
        Some("==") => Some("eq"),
        Some("/=") => Some("neq"),
        Some("<") => Some("lt"),
        Some(">") => Some("gt"),
        Some("<=") => Some("le"),
        Some(">=") => Some("ge"),
        Some("&&") => Some("and"),
        Some("||") => Some("or"),
        Some("++") => Some("append"),
        // Unknown operators map to their own name under Basics, matching the
        // Haskell fall-through (`_ -> Can.VarKernel "Basics" op`).
        _ => None,
    };
    // The immutable borrow above ends here, so interning is now permitted.
    func.map_or(Ok(op), |name| interner.intern(name))
}

/// Canonicalise a type annotation. M0 subset of `Canonicalise.Type`, extended
/// with non-parametric `type alias` expansion: a `TType` whose unqualified name
/// (and no type arguments) names a registered alias is replaced in place by the
/// canonicalised alias body, so no later stage observes the alias name.
///
/// `visited` carries the chain of aliases currently being expanded along this
/// path. A name already in the chain is a recursive alias — expansion stops and
/// the name is left as an opaque constructor rather than recursing forever
/// (soundness over completeness: a cyclic alias is exotic and out of scope for
/// M1, but it must never crash the compiler).
fn canonicalise_type(
    t: &src::TypeAnnotation,
    env: &Env,
    local_union_names: &BTreeSet<Symbol>,
    aliases: &BTreeMap<Symbol, src::TypeAnnotation>,
    free_vars: &mut BTreeSet<Symbol>,
    visited: &mut Vec<Symbol>,
) -> canon::Type {
    match t {
        src::TypeAnnotation::TLambda(a, b) => canon::Type::Lambda(
            Box::new(canonicalise_type(
                a,
                env,
                local_union_names,
                aliases,
                free_vars,
                visited,
            )),
            Box::new(canonicalise_type(
                b,
                env,
                local_union_names,
                aliases,
                free_vars,
                visited,
            )),
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
            // An argument-free reference to a non-parametric alias expands to its
            // body. (The M1 grammar rejects qualified types in annotations, so
            // every `TType` here is unqualified — the qualifier is always empty.)
            // An applied name, or a name already mid-expansion (cycle), skips
            // expansion.
            if args.is_empty()
                && !visited.contains(&name)
                && let Some(body) = aliases.get(&name)
            {
                visited.push(name);
                let expanded =
                    canonicalise_type(body, env, local_union_names, aliases, free_vars, visited);
                visited.pop();
                return expanded;
            }
            let home = if local_union_names.contains(&name) {
                env.home.clone()
            } else {
                Vec::new()
            };
            let can_args = args
                .iter()
                .map(|a| canonicalise_type(a, env, local_union_names, aliases, free_vars, visited))
                .collect();
            canon::Type::Con {
                home,
                name,
                args: can_args,
            }
        }
    }
}

/// Resolve a symbol to an owned name for a diagnostic payload.
///
/// # Errors
/// [`Diagnostic::CompilerBug`] (`SKY-I0010`) when the symbol is not backed by
/// the interner — every name here came from the parser via this interner, so a
/// miss is an impossible-invariant violation, surfaced rather than swallowed as
/// an empty identifier.
fn name_str(interner: &Interner, sym: Symbol) -> DResult<Box<str>> {
    interner
        .resolve(sym)
        .map(Box::<str>::from)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "intern.resolve",
            detail: "canonicaliser: name symbol not backed by the interner".to_owned(),
        })
}

/// Build the deterministic `did you mean` suggestion list for an unresolved
/// `typo`, drawn from `candidates`.
///
/// Candidates within [`SUGGESTION_MAX_DISTANCE`] edits (and not identical) are
/// kept, sorted by `(Levenshtein distance, name)` — a total, allocation-order-
/// independent ordering — and truncated to [`MAX_SUGGESTIONS`]. A candidate the
/// interner cannot resolve is skipped (it cannot be rendered) rather than
/// faulting the whole diagnostic.
fn suggestions(
    typo: Symbol,
    candidates: impl Iterator<Item = Symbol>,
    interner: &Interner,
) -> Box<[Box<str>]> {
    let Some(typo_str) = interner.resolve(typo) else {
        return Box::new([]);
    };
    let mut scored: Vec<(usize, Box<str>)> = candidates
        .filter_map(|c| interner.resolve(c))
        .map(|name| (levenshtein(typo_str, name), Box::<str>::from(name)))
        .filter(|(d, _)| *d > 0 && *d <= SUGGESTION_MAX_DISTANCE)
        .collect();
    // (distance, name) is a total order; `Box<str>` compares lexicographically.
    scored.sort();
    scored.dedup();
    scored
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, name)| name)
        .collect()
}

/// Iterative Levenshtein edit distance over Unicode scalar values, computed
/// with two rolling rows and no indexing (the `indexing_slicing` lint is
/// denied workspace-wide). Sky identifiers are short ASCII names, so the
/// O(n·m) cost is negligible. Mirrors the Haskell reference's `levenshtein`.
fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    // Row 0: cost of deleting every prefix of `b` (i.e. inserting into empty a).
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        // `curr[0]` = cost of deleting the first `i + 1` chars of `a`.
        let mut curr: Vec<usize> = Vec::with_capacity(b_chars.len() + 1);
        curr.push(i + 1);
        // `diag` tracks `prev[j - 1]`; for the first column that is `prev[0]`,
        // which always equals the row index `i`.
        let mut diag = i;
        for (cb, &up) in b_chars.iter().zip(prev.iter().skip(1)) {
            let cost = usize::from(ca != *cb);
            let left = curr.last().copied().unwrap_or(i + 1);
            let cell = (up + 1).min(left + 1).min(diag + cost);
            curr.push(cell);
            diag = up;
        }
        prev = curr;
    }
    prev.last().copied().unwrap_or(0)
}

/// The interned symbol for the empty string (symbol id 0 is never guaranteed,
/// so we cannot hardcode it). Used only on the unreachable unnamed-type path.
const fn name_zero() -> Symbol {
    Symbol::from_raw(0)
}
