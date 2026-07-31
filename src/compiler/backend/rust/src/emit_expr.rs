//! Expression and function emission.
//!
//! Ports the relevant arms of `Ipê/Generate/Rust/Builder/ExprEmitter.hs` and
//! the function-item shape from `ModuleEmitter.hs`. The byte target is golden
//! `main.rs` lines 129–137 (`main_update` / `ipe_main`).

use core::fmt::Write as _;

use ipe_diagnostics::{DResult, Diagnostic, LowerError, Span};
use ipe_intern::Symbol;
use ipe_ir::{
    Arm, BinOp, BoundSet, Callee, Expr, Func, IrType, KernelFn, MAX_IR_RENDER_DEPTH, Match,
    ModPath, Pat,
};

use crate::EmitCtx;
use crate::doc::Doc;
use crate::emit_types::{GenericScope, render_type};
use crate::naming::kernel_name;
use crate::render::{RenderConfig, render_seeded};

/// The deepest expression nesting the backend will descend before failing fast.
///
/// `emit_expr` recurses one Rust stack frame per IR-expression level (`BinOp`
/// operands, call arguments, match scrutinee/arm bodies). An adversarially or
/// buggily deep IR spine would otherwise overflow the native stack with no
/// diagnostic. The parser already caps *source* nesting at 256 (IPE-P0003);
/// this matching bound is defence-in-depth against an IR produced past that —
/// well below the native stack ceiling (≤ 2 MB default thread stack), so the
/// guard fires first. Sized conservatively to leave headroom for the frame size
/// of `emit_expr_at` in debug builds.
///
/// Shares [`ipe_ir::MAX_IR_RENDER_DEPTH`] rather than a separately-declared
/// copy of the same value — a `--emit-ir` dev-flag dump and the real emitter
/// must refuse a program at the identical depth, not two independently
/// tuned bounds that can drift apart.
const MAX_EMIT_DEPTH: u16 = MAX_IR_RENDER_DEPTH;

/// One indentation level: four spaces, matching the golden's formatting.
fn indent_of(level: usize) -> String {
    "    ".repeat(level)
}

/// Returns `true` if the expression produces a value that Rust will MOVE on
/// first use (i.e., a non-`Clone` type), making a multi-use `let` binding
/// cause E0382 "use of moved value".
///
/// The primary case is `Vec<IpeTask<A>>`: a list whose element type is or
/// contains a task.  `IpeTask<A>` is a `Pin<Box<dyn Future …>>` — it has no
/// `Clone` impl because polling a future to completion consumes it.  Ipê's
/// pure semantics guarantee re-evaluation is always correct, so the emitter
/// can safely inline the value expression at every use site.
///
/// Plain `Clone`/`Copy` values (integers, booleans, strings, records, enums)
/// do NOT trigger this path — their `let` bindings are preserved so the
/// compiler can share the computation.
///
/// Recurses into `Tuple`/`Record` LITERALS so a directly-constructed
/// `(tasks, n)` or `{ tasks = ..., n = ... }` whose task-list is nested one
/// level down is also caught — a purely structural widening of the AUD-04
/// audit's narrower `Expr::List`-only check (#B4). A `let`-bound value that
/// is task-typed only through its declared TYPE (e.g. a `Call` to a
/// Task-returning helper, or a `Var` reference to an already-task-typed
/// binding) is NOT detected here — that needs a real type-of-expression
/// recovery pass this backend does not have; filed as a residual gap rather
/// than guessed at (see AUD-04 follow-up in backlog.md).
pub fn expr_value_is_non_clone(expr: &Expr) -> bool {
    match expr {
        // A list whose element is a task (or contains one) — Vec<IpeTask<A>>
        // is move-only.
        Expr::List { elem, .. } => ir_type_contains_task(elem),
        Expr::Tuple(items) => items.iter().any(expr_value_is_non_clone),
        Expr::Record(fields) => fields.iter().any(|(_, e)| expr_value_is_non_clone(e)),
        _ => false,
    }
}

/// Is `ty` a Rust type that is UNCONDITIONALLY `Copy` in every emission this
/// backend produces? Mirrors `ipe_lower::lower::clone_class`'s `CopyLeaf`
/// classification exactly (kept duplicated across the crate boundary per this
/// file's established convention — see [`pat_bound_symbols`]'s doc comment).
///
/// Deliberately conservative: a `Generic(_)` type parameter is bounded only by
/// `Clone` (`emit_func` injects `Clone`, never `Copy`), so it must return
/// `false` even though a caller might monomorphize it to a Copy type at some
/// call site — the backend has no per-call-site visibility here. A user
/// `Enum`/`Record` also returns `false`: synthesized enums/structs derive
/// `Clone`, not `Copy`. `StreamWriter`/`WebSocketServer` are
/// `#[derive(Clone, Copy)]` i64 id wrappers (`server_stream.rs` / websocket
/// server), matching `clone_class`'s own `CopyLeaf` arm for them.
///
/// Used by the `Expr::Access` emission arm for AUD-09's type-directed
/// Copy elision — see
/// `docs/adr/0011-emitter-clone-borrow-discipline.md` §3.
const fn ir_type_is_definitely_copy(ty: &IrType) -> bool {
    matches!(
        ty,
        IrType::Int
            | IrType::Float
            | IrType::Bool
            | IrType::Char
            | IrType::Unit
            | IrType::Order
            | IrType::HttpMethod
            | IrType::Decimal
            | IrType::ErrorKind
            | IrType::StreamWriter
            | IrType::WebSocketServer
    )
}

/// Collect every symbol a pattern binds (recursively) into `out`. Mirrors
/// `ipe_lower::pat_binds_symbol`'s traversal shape but gathers the full bound
/// set in one pass rather than testing membership of one target.
fn pat_bound_symbols(pat: &Pat, out: &mut std::collections::BTreeSet<Symbol>) {
    match pat {
        Pat::Var(s) => {
            out.insert(*s);
        }
        Pat::Wildcard | Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) | Pat::Str(_) => {}
        Pat::Alias(inner, s) => {
            out.insert(*s);
            pat_bound_symbols(inner, out);
        }
        Pat::Ctor { args, .. } => {
            for p in args {
                pat_bound_symbols(p, out);
            }
        }
        Pat::Tuple(elems) => {
            for p in elems {
                pat_bound_symbols(p, out);
            }
        }
        Pat::Record(fields) => {
            for (_, p) in fields {
                pat_bound_symbols(p, out);
            }
        }
        Pat::Slice { prefix, rest } => {
            for p in prefix {
                pat_bound_symbols(p, out);
            }
            if let Some(p) = rest {
                pat_bound_symbols(p, out);
            }
        }
        // Every alternative of an or-pattern binds the same names, so the first
        // alternative's binders are the whole set.
        Pat::Or(alts) => {
            if let Some(first) = alts.first() {
                pat_bound_symbols(first, out);
            }
        }
    }
}

/// Exhaustive, binder-aware free-variable collector over the typed IR
/// `Expr` tree — replaces the AUD-04 textual `clone_captured_vars` /
/// `replace_word_all` passes, which operated on ALREADY-EMITTED Rust source
/// text with no string-literal-state tracking (a captured-variable word
/// appearing mid string literal, or matching a record field name, would get
/// corrupted). Operating on the IR instead means a string literal is an
/// opaque `Expr::Str` leaf and a record field name is a `Symbol` in a
/// `(Symbol, Expr)` pair — neither is ever mistaken for a `Var` occurrence.
///
/// Every binder (`Let`/`Destructure`/`Lambda`/`TailLoop`/`Match` arm
/// patterns) removes its bound name(s) from the free set of the scope it
/// introduces, exactly mirroring `ipe_lower::rewrite_var_to_apply`'s
/// shadow-aware recursion shape.
pub fn free_vars(expr: &Expr) -> std::collections::BTreeSet<Symbol> {
    let mut out = std::collections::BTreeSet::new();
    collect_free_vars(expr, &mut out);
    out
}

#[allow(clippy::too_many_lines)] // A recursive tree-walk over a large enum — necessarily long.
fn collect_free_vars(expr: &Expr, out: &mut std::collections::BTreeSet<Symbol>) {
    match expr {
        Expr::Int(_)
        | Expr::Bool(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::PathLit(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => {}
        Expr::Var(s) | Expr::CloneVar(s) => {
            out.insert(*s);
        }
        Expr::Ctor { args, .. } | Expr::Call { args, .. } | Expr::TailRecur { args } => {
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::BinOp { lhs, rhs, .. } => {
            collect_free_vars(lhs, out);
            collect_free_vars(rhs, out);
        }
        Expr::Let { name, value, body } => {
            collect_free_vars(value, out);
            let mut body_free = std::collections::BTreeSet::new();
            collect_free_vars(body, &mut body_free);
            body_free.remove(name);
            out.extend(body_free);
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            collect_free_vars(value, out);
            let mut bound = std::collections::BTreeSet::new();
            pat_bound_symbols(binder, &mut bound);
            let mut body_free = std::collections::BTreeSet::new();
            collect_free_vars(body, &mut body_free);
            for b in &bound {
                body_free.remove(b);
            }
            out.extend(body_free);
        }
        Expr::If { cond, then_, else_ } => {
            collect_free_vars(cond, out);
            collect_free_vars(then_, out);
            collect_free_vars(else_, out);
        }
        Expr::Match(m) => {
            collect_free_vars(m.scrutinee(), out);
            for arm in m.arms() {
                let mut bound = std::collections::BTreeSet::new();
                pat_bound_symbols(&arm.pat, &mut bound);
                let mut body_free = std::collections::BTreeSet::new();
                collect_free_vars(&arm.body, &mut body_free);
                for b in &bound {
                    body_free.remove(b);
                }
                out.extend(body_free);
            }
        }
        Expr::Tuple(items) | Expr::List { items, .. } => {
            for e in items {
                collect_free_vars(e, out);
            }
        }
        Expr::Cons { head, tail } => {
            collect_free_vars(head, out);
            collect_free_vars(tail, out);
        }
        Expr::ListIndexClone { list, .. } | Expr::ListLenCheck { list, .. } => {
            collect_free_vars(list, out);
        }
        Expr::Record(fields) => {
            for (_, e) in fields {
                collect_free_vars(e, out);
            }
        }
        Expr::Access { record, .. } => collect_free_vars(record, out),
        Expr::Update { record, fields } => {
            collect_free_vars(record, out);
            for (_, e) in fields {
                collect_free_vars(e, out);
            }
        }
        Expr::Lambda { params, body, .. }
        | Expr::SharedLambda { params, body, .. }
        | Expr::TailLoop { params, body } => {
            let mut body_free = std::collections::BTreeSet::new();
            collect_free_vars(body, &mut body_free);
            for (s, _) in params {
                body_free.remove(s);
            }
            out.extend(body_free);
        }
        Expr::Apply { func, args } => {
            collect_free_vars(func, out);
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::TaskSeq { effect, rest } | Expr::TaskSeqSync { effect, rest } => {
            collect_free_vars(effect, out);
            collect_free_vars(rest, out);
        }
    }
}

/// Shadow-aware IR rewrite: replace every FREE occurrence of `Expr::Var(target)`
/// in `expr` with `Expr::CloneVar(target)`, stopping recursion into any subtree
/// where a binder rebinds `target` (that occurrence is a different binding, not
/// the captured one). Structurally identical shadow-skip shape to
/// `ipe_lower::rewrite_var_to_apply` — the one existing precedent for a
/// single-target IR substitution in this codebase — with a `CloneVar` leaf
/// action instead of an `Apply` wrap.
///
/// Cloning a `Copy` value (Int/Bool/…) compiles to a bitwise copy — harmless —
/// so this never needs a Copy/non-Copy type check to stay sound; it only ever
/// clones a variable that a caller determined is genuinely captured (see
/// `clone_targets_in_expr`).
#[allow(clippy::too_many_lines)] // A recursive tree-walk over a large enum — necessarily long.
fn clone_free_target(expr: Expr, target: Symbol) -> Expr {
    match expr {
        Expr::Var(s) if s == target => Expr::CloneVar(s),
        Expr::Var(_)
        | Expr::CloneVar(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::PathLit(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => expr,
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(clone_free_target(*lhs, target)),
            rhs: Box::new(clone_free_target(*rhs, target)),
        },
        Expr::Let { name, value, body } => {
            let new_value = Box::new(clone_free_target(*value, target));
            let new_body = if name == target {
                body
            } else {
                Box::new(clone_free_target(*body, target))
            };
            Expr::Let {
                name,
                value: new_value,
                body: new_body,
            }
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            let new_value = Box::new(clone_free_target(*value, target));
            let new_body = if pat_binds_target(&binder, target) {
                body
            } else {
                Box::new(clone_free_target(*body, target))
            };
            Expr::Destructure {
                binder,
                value: new_value,
                body: new_body,
            }
        }
        Expr::If { cond, then_, else_ } => Expr::If {
            cond: Box::new(clone_free_target(*cond, target)),
            then_: Box::new(clone_free_target(*then_, target)),
            else_: Box::new(clone_free_target(*else_, target)),
        },
        Expr::Match(m) => Expr::Match(m.map_bodies(
            |scrutinee| clone_free_target(scrutinee, target),
            |pat, body, guard| {
                let binds = pat_binds_target(pat, target);
                let new_body = if binds {
                    body
                } else {
                    clone_free_target(body, target)
                };
                // Preserve the list-length guard, rewriting it too when the arm
                // pattern does not bind `target`.
                let new_guard = guard.map(|g| {
                    if binds {
                        g
                    } else {
                        clone_free_target(g, target)
                    }
                });
                (new_body, new_guard)
            },
        )),
        Expr::Call {
            callee,
            args,
            pin,
            on_form,
        } => Expr::Call {
            callee,
            args: args
                .into_iter()
                .map(|a| clone_free_target(a, target))
                .collect(),
            pin,
            on_form,
        },
        Expr::Tuple(items) => Expr::Tuple(
            items
                .into_iter()
                .map(|e| clone_free_target(e, target))
                .collect(),
        ),
        Expr::List { elem, items } => Expr::List {
            elem,
            items: items
                .into_iter()
                .map(|e| clone_free_target(e, target))
                .collect(),
        },
        Expr::Cons { head, tail } => Expr::Cons {
            head: Box::new(clone_free_target(*head, target)),
            tail: Box::new(clone_free_target(*tail, target)),
        },
        Expr::ListIndexClone { list, index } => Expr::ListIndexClone {
            list: Box::new(clone_free_target(*list, target)),
            index,
        },
        Expr::ListLenCheck { list, len, exact } => Expr::ListLenCheck {
            list: Box::new(clone_free_target(*list, target)),
            len,
            exact,
        },
        Expr::Record(fields) => Expr::Record(
            fields
                .into_iter()
                .map(|(s, e)| (s, clone_free_target(e, target)))
                .collect(),
        ),
        Expr::Access {
            record,
            field,
            field_ty,
        } => Expr::Access {
            record: Box::new(clone_free_target(*record, target)),
            field,
            field_ty,
        },
        Expr::Update { record, fields } => Expr::Update {
            record: Box::new(clone_free_target(*record, target)),
            fields: fields
                .into_iter()
                .map(|(s, e)| (s, clone_free_target(e, target)))
                .collect(),
        },
        Expr::Lambda { params, ret, body } => {
            let new_body = if params.iter().any(|(s, _)| *s == target) {
                body
            } else {
                Box::new(clone_free_target(*body, target))
            };
            Expr::Lambda {
                params,
                ret,
                body: new_body,
            }
        }
        Expr::SharedLambda { params, ret, body } => {
            let new_body = if params.iter().any(|(s, _)| *s == target) {
                body
            } else {
                Box::new(clone_free_target(*body, target))
            };
            Expr::SharedLambda {
                params,
                ret,
                body: new_body,
            }
        }
        Expr::Apply { func, args } => Expr::Apply {
            func: Box::new(clone_free_target(*func, target)),
            args: args
                .into_iter()
                .map(|a| clone_free_target(a, target))
                .collect(),
        },
        Expr::TaskSeq { effect, rest } => Expr::TaskSeq {
            effect: Box::new(clone_free_target(*effect, target)),
            rest: Box::new(clone_free_target(*rest, target)),
        },
        Expr::TaskSeqSync { effect, rest } => Expr::TaskSeqSync {
            effect: Box::new(clone_free_target(*effect, target)),
            rest: Box::new(clone_free_target(*rest, target)),
        },
        Expr::Ctor {
            home,
            ty,
            variant,
            args,
        } => Expr::Ctor {
            home,
            ty,
            variant,
            args: args
                .into_iter()
                .map(|a| clone_free_target(a, target))
                .collect(),
        },
        Expr::TailLoop { params, body } => {
            let new_body = if params.iter().any(|(s, _)| *s == target) {
                body
            } else {
                Box::new(clone_free_target(*body, target))
            };
            Expr::TailLoop {
                params,
                body: new_body,
            }
        }
        Expr::TailRecur { args } => Expr::TailRecur {
            args: args
                .into_iter()
                .map(|a| clone_free_target(a, target))
                .collect(),
        },
    }
}

/// `Pat` version of the shadow check used by [`clone_free_target`] /
/// [`substitute_var`]: does this irrefutable/refutable binder pattern bind
/// `target`? Local twin of `ipe_lower::pat_binds_symbol` (same shape) — kept
/// in this crate rather than shared because `ipe_backend_rust` does not
/// depend on `ipe_lower` (IR flows one way: lower produces it, backends
/// consume it).
fn pat_binds_target(pat: &Pat, target: Symbol) -> bool {
    match pat {
        Pat::Var(s) => *s == target,
        Pat::Wildcard | Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) | Pat::Str(_) => false,
        Pat::Alias(inner, s) => *s == target || pat_binds_target(inner, target),
        Pat::Ctor { args, .. } => args.iter().any(|p| pat_binds_target(p, target)),
        Pat::Tuple(elems) => elems.iter().any(|p| pat_binds_target(p, target)),
        Pat::Record(fields) => fields.iter().any(|(_, p)| pat_binds_target(p, target)),
        Pat::Slice { prefix, rest } => {
            prefix.iter().any(|p| pat_binds_target(p, target))
                || rest.as_deref().is_some_and(|p| pat_binds_target(p, target))
        }
        // Every alternative binds the same names, so it binds `target` iff any
        // (equivalently the first) alternative does.
        Pat::Or(alts) => alts.iter().any(|p| pat_binds_target(p, target)),
    }
}

/// Fold [`clone_free_target`] over every symbol in `targets`. Each fold step
/// only ever rewrites bare `Var` occurrences into `CloneVar` — the passes
/// don't interfere with each other regardless of order (a `CloneVar` leaf is
/// never re-matched by a later target's pass).
pub fn clone_targets_in_expr(expr: Expr, targets: &std::collections::BTreeSet<Symbol>) -> Expr {
    targets.iter().fold(expr, |e, &t| clone_free_target(e, t))
}

/// Shadow-aware scan of `expr` for a `let`-bound `target`'s free occurrences,
/// used to gate [`Expr::Let`]'s multi-use inline decision. Returns
/// `(var_count, has_clonevar)`:
///
/// * `var_count` — number of free `Expr::Var(target)` reads. Replaces the
///   AUD-04 textual `count_word_occurrences(&body_s, &name_s)`, which counted
///   matches inside ALREADY-RENDERED text (so a match inside a string literal
///   or a record field name inflated the count and could trigger a corrupting
///   inline). Counting over the IR instead only ever sees genuine `Var` reads.
/// * `has_clonevar` — `true` if a free `Expr::CloneVar(target)` occurs (a
///   lambda capture-clone site the lowerer already emitted for this same
///   binding). [`Expr`] has no node for "clone of an arbitrary expression",
///   so [`substitute_var`] cannot cleanly substitute through a `CloneVar`
///   leaf; when this is `true`, `Expr::Let`'s emitter skips inlining and
///   keeps the plain `let` form — always correct, just not move-optimized
///   for that one binding.
pub fn scan_free_target(expr: &Expr, target: Symbol) -> (usize, bool) {
    let mut count = 0usize;
    let mut has_clonevar = false;
    scan_free_target_into(expr, target, &mut count, &mut has_clonevar);
    (count, has_clonevar)
}

#[allow(clippy::too_many_lines)] // A recursive tree-walk over a large enum — necessarily long.
fn scan_free_target_into(expr: &Expr, target: Symbol, count: &mut usize, has_clonevar: &mut bool) {
    match expr {
        Expr::Var(s) => {
            if *s == target {
                *count += 1;
            }
        }
        Expr::CloneVar(s) => {
            if *s == target {
                *has_clonevar = true;
            }
        }
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::PathLit(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => {}
        Expr::Ctor { args, .. } | Expr::Call { args, .. } | Expr::TailRecur { args } => {
            for a in args {
                scan_free_target_into(a, target, count, has_clonevar);
            }
        }
        Expr::BinOp { lhs, rhs, .. } => {
            scan_free_target_into(lhs, target, count, has_clonevar);
            scan_free_target_into(rhs, target, count, has_clonevar);
        }
        Expr::Let { name, value, body } => {
            scan_free_target_into(value, target, count, has_clonevar);
            if *name != target {
                scan_free_target_into(body, target, count, has_clonevar);
            }
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            scan_free_target_into(value, target, count, has_clonevar);
            if !pat_binds_target(binder, target) {
                scan_free_target_into(body, target, count, has_clonevar);
            }
        }
        Expr::If { cond, then_, else_ } => {
            scan_free_target_into(cond, target, count, has_clonevar);
            scan_free_target_into(then_, target, count, has_clonevar);
            scan_free_target_into(else_, target, count, has_clonevar);
        }
        Expr::Match(m) => {
            scan_free_target_into(m.scrutinee(), target, count, has_clonevar);
            for arm in m.arms() {
                if !pat_binds_target(&arm.pat, target) {
                    scan_free_target_into(&arm.body, target, count, has_clonevar);
                }
            }
        }
        Expr::Tuple(items) | Expr::List { items, .. } => {
            for e in items {
                scan_free_target_into(e, target, count, has_clonevar);
            }
        }
        Expr::Cons { head, tail } => {
            scan_free_target_into(head, target, count, has_clonevar);
            scan_free_target_into(tail, target, count, has_clonevar);
        }
        Expr::ListIndexClone { list, .. } | Expr::ListLenCheck { list, .. } => {
            scan_free_target_into(list, target, count, has_clonevar);
        }
        Expr::Record(fields) => {
            for (_, e) in fields {
                scan_free_target_into(e, target, count, has_clonevar);
            }
        }
        Expr::Access { record, .. } => scan_free_target_into(record, target, count, has_clonevar),
        Expr::Update { record, fields } => {
            scan_free_target_into(record, target, count, has_clonevar);
            for (_, e) in fields {
                scan_free_target_into(e, target, count, has_clonevar);
            }
        }
        Expr::Lambda { params, body, .. }
        | Expr::SharedLambda { params, body, .. }
        | Expr::TailLoop { params, body } => {
            if !params.iter().any(|(s, _)| *s == target) {
                scan_free_target_into(body, target, count, has_clonevar);
            }
        }
        Expr::Apply { func, args } => {
            scan_free_target_into(func, target, count, has_clonevar);
            for a in args {
                scan_free_target_into(a, target, count, has_clonevar);
            }
        }
        Expr::TaskSeq { effect, rest } | Expr::TaskSeqSync { effect, rest } => {
            scan_free_target_into(effect, target, count, has_clonevar);
            scan_free_target_into(rest, target, count, has_clonevar);
        }
    }
}

/// Shadow-aware IR substitution: replace every FREE occurrence of
/// `Expr::Var(target)` in `body` with a clone of `replacement`, stopping
/// recursion into any subtree where a binder rebinds `target`. Replaces the
/// AUD-04 textual `replace_word_all(&body_s, &name_s, &replacement)`, which
/// pattern-matched the RENDERED Rust source by word-boundary only — so a
/// captured-variable word appearing mid string literal (`"the count is"` →
/// `"the count.clone() is"`) or matching a record field name
/// (`RecCount { count: n }` → `RecCount { count.clone(): n }`, invalid Rust)
/// got corrupted. Operating on the IR instead only ever touches genuine
/// `Var` leaf nodes — a string literal is an opaque `Expr::Str`, a record
/// field name is a `Symbol` key never matched against `Expr::Var`.
#[allow(clippy::too_many_lines)] // A recursive tree-walk over a large enum — necessarily long.
pub fn substitute_var(expr: Expr, target: Symbol, replacement: &Expr) -> Expr {
    match expr {
        Expr::Var(s) if s == target => replacement.clone(),
        Expr::Var(_)
        | Expr::CloneVar(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Str(_)
        | Expr::PathLit(_)
        | Expr::Char(_)
        | Expr::Unit
        | Expr::FuncValue { .. } => expr,
        Expr::BinOp { op, lhs, rhs } => Expr::BinOp {
            op,
            lhs: Box::new(substitute_var(*lhs, target, replacement)),
            rhs: Box::new(substitute_var(*rhs, target, replacement)),
        },
        Expr::Let { name, value, body } => {
            let new_value = Box::new(substitute_var(*value, target, replacement));
            let new_body = if name == target {
                body
            } else {
                Box::new(substitute_var(*body, target, replacement))
            };
            Expr::Let {
                name,
                value: new_value,
                body: new_body,
            }
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            let new_value = Box::new(substitute_var(*value, target, replacement));
            let new_body = if pat_binds_target(&binder, target) {
                body
            } else {
                Box::new(substitute_var(*body, target, replacement))
            };
            Expr::Destructure {
                binder,
                value: new_value,
                body: new_body,
            }
        }
        Expr::If { cond, then_, else_ } => Expr::If {
            cond: Box::new(substitute_var(*cond, target, replacement)),
            then_: Box::new(substitute_var(*then_, target, replacement)),
            else_: Box::new(substitute_var(*else_, target, replacement)),
        },
        Expr::Match(m) => Expr::Match(m.map_bodies(
            |scrutinee| substitute_var(scrutinee, target, replacement),
            |pat, body, guard| {
                let binds = pat_binds_target(pat, target);
                let new_body = if binds {
                    body
                } else {
                    substitute_var(body, target, replacement)
                };
                let new_guard = guard.map(|g| {
                    if binds {
                        g
                    } else {
                        substitute_var(g, target, replacement)
                    }
                });
                (new_body, new_guard)
            },
        )),
        Expr::Call {
            callee,
            args,
            pin,
            on_form,
        } => Expr::Call {
            callee,
            args: args
                .into_iter()
                .map(|a| substitute_var(a, target, replacement))
                .collect(),
            pin,
            on_form,
        },
        Expr::Tuple(items) => Expr::Tuple(
            items
                .into_iter()
                .map(|e| substitute_var(e, target, replacement))
                .collect(),
        ),
        Expr::List { elem, items } => Expr::List {
            elem,
            items: items
                .into_iter()
                .map(|e| substitute_var(e, target, replacement))
                .collect(),
        },
        Expr::Cons { head, tail } => Expr::Cons {
            head: Box::new(substitute_var(*head, target, replacement)),
            tail: Box::new(substitute_var(*tail, target, replacement)),
        },
        Expr::ListIndexClone { list, index } => Expr::ListIndexClone {
            list: Box::new(substitute_var(*list, target, replacement)),
            index,
        },
        Expr::ListLenCheck { list, len, exact } => Expr::ListLenCheck {
            list: Box::new(substitute_var(*list, target, replacement)),
            len,
            exact,
        },
        Expr::Record(fields) => Expr::Record(
            fields
                .into_iter()
                .map(|(s, e)| (s, substitute_var(e, target, replacement)))
                .collect(),
        ),
        Expr::Access {
            record,
            field,
            field_ty,
        } => Expr::Access {
            record: Box::new(substitute_var(*record, target, replacement)),
            field,
            field_ty,
        },
        Expr::Update { record, fields } => Expr::Update {
            record: Box::new(substitute_var(*record, target, replacement)),
            fields: fields
                .into_iter()
                .map(|(s, e)| (s, substitute_var(e, target, replacement)))
                .collect(),
        },
        Expr::Lambda { params, ret, body } => {
            let new_body = if params.iter().any(|(s, _)| *s == target) {
                body
            } else {
                Box::new(substitute_var(*body, target, replacement))
            };
            Expr::Lambda {
                params,
                ret,
                body: new_body,
            }
        }
        Expr::SharedLambda { params, ret, body } => {
            let new_body = if params.iter().any(|(s, _)| *s == target) {
                body
            } else {
                Box::new(substitute_var(*body, target, replacement))
            };
            Expr::SharedLambda {
                params,
                ret,
                body: new_body,
            }
        }
        Expr::Apply { func, args } => Expr::Apply {
            func: Box::new(substitute_var(*func, target, replacement)),
            args: args
                .into_iter()
                .map(|a| substitute_var(a, target, replacement))
                .collect(),
        },
        Expr::TaskSeq { effect, rest } => Expr::TaskSeq {
            effect: Box::new(substitute_var(*effect, target, replacement)),
            rest: Box::new(substitute_var(*rest, target, replacement)),
        },
        Expr::TaskSeqSync { effect, rest } => Expr::TaskSeqSync {
            effect: Box::new(substitute_var(*effect, target, replacement)),
            rest: Box::new(substitute_var(*rest, target, replacement)),
        },
        Expr::Ctor {
            home,
            ty,
            variant,
            args,
        } => Expr::Ctor {
            home,
            ty,
            variant,
            args: args
                .into_iter()
                .map(|a| substitute_var(a, target, replacement))
                .collect(),
        },
        Expr::TailLoop { params, body } => {
            let new_body = if params.iter().any(|(s, _)| *s == target) {
                body
            } else {
                Box::new(substitute_var(*body, target, replacement))
            };
            Expr::TailLoop {
                params,
                body: new_body,
            }
        }
        Expr::TailRecur { args } => Expr::TailRecur {
            args: args
                .into_iter()
                .map(|a| substitute_var(a, target, replacement))
                .collect(),
        },
    }
}

/// Returns `true` if `ty` is or structurally contains `IrType::Task`.
fn ir_type_contains_task(ty: &IrType) -> bool {
    match ty {
        IrType::Task(_) => true,
        IrType::Maybe(inner) | IrType::List(inner) => ir_type_contains_task(inner),
        IrType::Result(e, a) => ir_type_contains_task(e) || ir_type_contains_task(a),
        _ => false,
    }
}

/// The Rust spelling of a binary operator for use in infix emission.
///
/// Every Ipê core arithmetic/comparison/boolean operator maps to the
/// identically-spelled Rust operator except `/=` (Ipê inequality → Rust `!=`).
///
/// `IntDiv` and `Append` are listed here only to keep the match exhaustive
/// (a compiler requirement when a new `BinOp` variant is added); they MUST
/// NOT reach the infix branch:
/// - `BinOp::IntDiv` emits as a helper call, never as infix — `//` is a Rust
///   line comment, so reaching this arm is a codegen bug, caught at compile time
///   by the exhaustive `match op` in `Expr::BinOp`.
/// - `BinOp::Append` emits as `format!`, similarly intercepted before this arm.
const fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Eq => "==",
        BinOp::Neq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        // `IntDiv` is routed through ipe_runtime::math::ipe_int_div in the
        // Expr::BinOp handler — it must never reach the infix `op_str` path.
        // `//` here is a Rust line comment, making silent corruption impossible:
        // any accidental infix emit would comment out the rest of the expression.
        // Listed for exhaustiveness so adding a future BinOp variant is a
        // compile error here, not a silent gap.
        BinOp::IntDiv => "//",
        // `Append` has no infix Rust form; the `BinOp` arm routes it to
        // `format!` before reaching here. Listed for exhaustiveness.
        BinOp::Append => "++",
    }
}

/// Render an `f64` as a Rust literal that is guaranteed to TYPE as `f64`.
///
/// Rust's default `f64` Display drops the decimal point for a whole number
/// (`3.0` prints as `3`), and a bare `3` types as an integer — so a whole-number
/// float literal must keep (or regain) a decimal point. The shortest round-trip
/// Display is used (so the emitted literal parses back to the same bit pattern),
/// and `.0` is appended only when the rendering carries no `.`/`e` exponent
/// marker. A non-finite value (an over-range lexeme reads back as `inf`) can have
/// no decimal literal, so it renders through the `f64` associated constants,
/// keeping the emission total and valid Rust.
fn float_literal(f: f64) -> String {
    if f.is_nan() {
        return "f64::NAN".to_owned();
    }
    if f.is_infinite() {
        return if f < 0.0 {
            "f64::NEG_INFINITY"
        } else {
            "f64::INFINITY"
        }
        .to_owned();
    }
    let s = format!("{f}");
    if s.bytes().any(|b| b == b'.' || b == b'e' || b == b'E') {
        s
    } else {
        format!("{s}.0")
    }
}

/// The Rust name of a call target.
pub fn callee_name(ctx: &EmitCtx, callee: &Callee) -> DResult<String> {
    match callee {
        // Absolute `crate::` path so the call ALWAYS binds to the top-level
        // `fn`, never to a local `let` binder of the same folded name. A local
        // cannot shadow an absolute path, so a local spelled like a top-level
        // fn's Rust name (`let main_update = …` vs `fn main_update`) can no
        // longer intercept the call — closing the E0618 / silent-wrong-call
        // shadow class for every name at once. The `ipe_main` entry point and
        // FFI wrappers are already crate-root, so this is uniform.
        Callee::Func(id) => Ok(format!("crate::{}", ctx.func_name(*id)?)),
        Callee::Kernel(k) => Ok(kernel_name(*k).to_owned()),
        // A foreign wrapper lives in the emitted `src/ffi.rs` module; the
        // absolute `crate::ffi::` path keeps it unambiguous from every
        // emitted file. The identifier was validated at canonicalisation.
        Callee::Ffi { ident } => Ok(format!("crate::ffi::{}", ctx.resolve_ident(*ident)?)),
    }
}

/// Does this call target an FFI wrapper with transparent conversion glue?
/// The doc builder keeps such calls as byte-carried leaves so the string
/// emitter's glued rendering is the single source of the emitted text.
pub fn ffi_call_has_glue(ctx: &EmitCtx, callee: &Callee) -> DResult<bool> {
    if let Callee::Ffi { ident } = callee {
        Ok(ctx.ffi_wrapper_glue(*ident)?.is_some())
    } else {
        Ok(false)
    }
}

/// Emit a [`Callee::Ffi`] call through its transparent conversion glue.
///
/// Marked arguments convert Ipê→foreign inline; a glued result converts
/// foreign→Ipê around the call — under the `IpeResult` Ok arm for a fallible
/// wrapper, or over the bare value for an infallible accessor. Unmarked
/// positions render exactly as the generic tail would.
fn emit_ffi_glued_call(
    ctx: &EmitCtx,
    wrapper: Symbol,
    glue: &crate::FfiWrapperGlue,
    args: &[Expr],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let name = format!("crate::ffi::{}", ctx.resolve_ident(wrapper)?);
    let mut parts = Vec::with_capacity(args.len());
    for (i, arg) in args.iter().enumerate() {
        let rendered = emit_expr_at(ctx, arg, indent, depth, generics)?;
        match glue.params.get(i).and_then(Option::as_ref) {
            None => parts.push(rendered),
            Some(t) => parts.push(ffi_to_foreign(ctx, t, &rendered)?),
        }
    }
    let call = format!("{name}({})", parts.join(", "));
    let Some(result) = &glue.result else {
        return Ok(call);
    };
    let conv = ffi_from_foreign(ctx, &result.ty, "__ipe_ffi_v")?;
    if result.in_result {
        Ok(format!(
            "match {call} {{ IpeResult::Ok(__ipe_ffi_v) => IpeResult::Ok({conv}), \
             IpeResult::Err(__ipe_ffi_e) => IpeResult::Err(__ipe_ffi_e) }}"
        ))
    } else {
        Ok(format!("{{ let __ipe_ffi_v = {call}; {conv} }}"))
    }
}

/// Render the Ipê→foreign conversion of `value` (a rendered expression) for
/// one transparent type: a record moves field-for-field into the foreign
/// struct literal; a union matches the app enum into the foreign enum.
fn ffi_to_foreign(ctx: &EmitCtx, ty: &crate::FfiGlueType, value: &str) -> DResult<String> {
    match ty {
        crate::FfiGlueType::Record { rust_path, fields } => {
            let moves: Vec<String> = fields
                .iter()
                .map(|f| format!("{f}: __ipe_ffi_r.{f}"))
                .collect();
            Ok(format!(
                "{{ let __ipe_ffi_r = {value}; {rust_path} {{ {} }} }}",
                moves.join(", ")
            ))
        }
        crate::FfiGlueType::Union {
            module,
            name,
            rust_path,
            variants,
        } => {
            let app = ffi_union_app_name(ctx, module, name)?;
            let arms: Vec<String> = variants
                .iter()
                .map(|v| ffi_union_arm(&app, rust_path, v, Direction::ToForeign))
                .collect();
            Ok(format!("match ({value}) {{ {} }}", arms.join(", ")))
        }
    }
}

/// Render the foreign→Ipê conversion of the bound variable `value` for one
/// transparent type: a struct moves field-for-field into the synthesised
/// record struct; an enum matches the foreign enum into the app enum.
fn ffi_from_foreign(ctx: &EmitCtx, ty: &crate::FfiGlueType, value: &str) -> DResult<String> {
    match ty {
        crate::FfiGlueType::Record { fields, .. } => {
            let rec = ctx.record_name_for_literal(fields)?;
            let moves: Vec<String> = fields.iter().map(|f| format!("{f}: {value}.{f}")).collect();
            Ok(format!("{rec} {{ {} }}", moves.join(", ")))
        }
        crate::FfiGlueType::Union {
            module,
            name,
            rust_path,
            variants,
        } => {
            let app = ffi_union_app_name(ctx, module, name)?;
            let arms: Vec<String> = variants
                .iter()
                .map(|v| ffi_union_arm(&app, rust_path, v, Direction::FromForeign))
                .collect();
            Ok(format!("match {value} {{ {} }}", arms.join(", ")))
        }
    }
}

/// Which way a transparent-union match arm converts.
#[derive(Clone, Copy)]
enum Direction {
    ToForeign,
    FromForeign,
}

/// One `match` arm converting a transparent enum variant between the app
/// enum (always tuple-shaped — the positional Ipê constructor surface) and
/// the foreign enum (its declared unit/tuple/struct shape).
fn ffi_union_arm(
    app: &str,
    rust_path: &str,
    v: &crate::FfiGlueVariant,
    direction: Direction,
) -> String {
    let vn = &v.name;
    let binders: Vec<String> = match &v.payload {
        crate::FfiGluePayload::Unit => Vec::new(),
        crate::FfiGluePayload::Tuple(n) => (0..*n).map(|i| format!("__ipe_ffi_p{i}")).collect(),
        crate::FfiGluePayload::Struct(members) => (0..members.len())
            .map(|i| format!("__ipe_ffi_p{i}"))
            .collect(),
    };
    // The app side is positional; the foreign side re-attaches struct-variant
    // member names.
    let app_side = if binders.is_empty() {
        format!("{app}::{vn}")
    } else {
        format!("{app}::{vn}({})", binders.join(", "))
    };
    let foreign_side = match &v.payload {
        crate::FfiGluePayload::Unit => format!("{rust_path}::{vn}"),
        crate::FfiGluePayload::Tuple(_) => format!("{rust_path}::{vn}({})", binders.join(", ")),
        crate::FfiGluePayload::Struct(members) => {
            let named: Vec<String> = members
                .iter()
                .zip(&binders)
                .map(|(m, b)| format!("{m}: {b}"))
                .collect();
            format!("{rust_path}::{vn} {{ {} }}", named.join(", "))
        }
    };
    match direction {
        Direction::ToForeign => format!("{app_side} => {foreign_side}"),
        Direction::FromForeign => format!("{foreign_side} => {app_side}"),
    }
}

/// The app-side Rust enum name for a transparent union, resolved through the
/// registered `EnumDef` exactly as every other reference to it.
fn ffi_union_app_name(ctx: &EmitCtx, module: &[String], name: &str) -> DResult<String> {
    let mut segs = Vec::with_capacity(module.len());
    for m in module {
        segs.push(ctx.lookup_symbol(m)?);
    }
    let name_sym = ctx.lookup_symbol(name)?;
    Ok(ctx.enum_name(&ipe_ir::ModPath(segs), name_sym)?.to_owned())
}

/// Whether a kernel's runtime function takes its two arguments in the OPPOSITE
/// order to the Ipê call. The `Maybe` / `Result` mapping combinators are
/// container-first in the runtime (`ipe_maybe_map(m, f)`) but function-first in
/// Ipê (`Maybe.map f m`); every other wired kernel matches the Ipê order. Used by
/// the [`Expr::Call`] emitter to reverse the rendered argument list.
pub const fn kernel_swaps_first_two(k: ipe_ir::KernelFn) -> bool {
    matches!(
        k,
        KernelFn::MaybeMap
            | KernelFn::MaybeAndThen
            | KernelFn::ResultMap
            // `Result.andThen f r` / `Result.mapError f r` — Ipê passes the
            // fn first; the runtime `ipe_result_and_then(r, f)` /
            // `ipe_result_map_error(r, f)` take the container first.
            | KernelFn::ResultAndThen
            | KernelFn::ResultMapError
            // `JsonDec.andThen f decoder` — Ipê passes fn first; Rust runtime
            // `decode_and_then(decoder, f)` expects decoder first. `Config.andThen`
            // shares `decode_and_then`, so it needs the same reorder.
            | KernelFn::JsonDecAndThen
            | KernelFn::ConfigAndThen
            // `Task.andThen f task` — Ipê passes continuation first; Rust runtime
            // `task_and_then(task, f)` expects effect first so Rust evaluates the
            // effect expression BEFORE the continuation closure captures shared Db
            // pool values, preventing E0507 / E0382 move conflicts at connect-use
            // sites (see `Expr::TaskSeq` below for the auto-force counterpart).
            | KernelFn::TaskAndThen
    )
}

/// Whether a `Call` node hits one of the bespoke kernel special cases the
/// generic `{name}{turbofish}({args})` tail below does NOT cover — the JSON /
/// Http / Http-builder / Task-retry / Db / TEA / Server / UI probe helpers, or
/// the `Dict.get` clone-arg case. Every one of those probes gates on
/// `Callee::Kernel`, so a non-kernel callee is trivially `false`.
///
/// This is the p'does any special case apply?' predicate the native Doc emitter
/// ([`crate::emit_doc`]) consults to decide whether a `Call` can be structured as
/// the generic delimited tail (special case absent) or must stay a byte-carried
/// leaf (special case present). It re-runs the probes rather than duplicating
/// their per-kernel `KernelFn` matches, so it can never drift from them; the
/// probes take `&EmitCtx` immutably and have no side effects, so re-running them
/// is safe. The rendered strings they return are discarded here.
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the emit_expr_at Call arm's probe-chain arguments verbatim"
)]
pub fn call_has_kernel_special_case(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    on_form: ipe_ir::OnFormKind,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<bool> {
    // Only kernels have special cases; every probe would gate out immediately.
    if !matches!(callee, Callee::Kernel(_)) {
        return Ok(false);
    }
    if emit_json_decoder_call(ctx, callee, args, indent, child, generics)?.is_some()
        || emit_http_call(ctx, callee, args, indent, child, generics)?.is_some()
        || emit_http_builder_call(ctx, callee, args, indent, child, generics)?.is_some()
        || emit_task_retry_call(ctx, callee, args, indent, child, generics)?.is_some()
        || emit_db_call(ctx, callee, args, indent, child, generics)?.is_some()
        || emit_tea_call(ctx, callee, args, indent, child, generics)?.is_some()
        || emit_server_call(ctx, callee, args, indent, child, generics)?.is_some()
        || emit_ui_call(ctx, callee, args, on_form, indent, child, generics)?.is_some()
    {
        return Ok(true);
    }
    // `Dict.get` clones its dict arg — the generic tail would drop the `.clone()`.
    if matches!(callee, Callee::Kernel(KernelFn::DictGet)) {
        return Ok(true);
    }
    Ok(false)
}

/// Handle Http kernel calls that require custom argument wrapping.
///
/// Returns `Some(emitted)` for the three network-effect kernels
/// (`HttpGet` / `HttpPost` / `HttpRequest`), which need a `task_map`
/// closure that converts `ipe_runtime::HttpResponse` into the synthesised
/// Ipê record struct for `{body, headers, status}`.
///
/// `HttpParseQuery` returns `HashMap<String,String>` which is exactly
/// `Dict String String` — the standard `Expr::Call` emitter is correct
/// and this function returns `None` for it.
///
/// The conversion is a PURE FIELD-FOR-FIELD MOVE — no validation, no
/// second parse boundary. All guards (SSRF, body cap, timeout, error
/// redaction) live inside the runtime entry points; the emitter only
/// wraps the response record.
///
/// All three network kernels emit explicit `::<IpeError>` turbofish so
/// Rust can infer the error channel even when the `Err` arm is discarded.
/// The closure parameter is typed `|r: ipe_runtime::HttpResponse|` so
/// the closure's input type is never ambiguous.
///
/// Factored out of `emit_expr_at` to keep that function's stack frame
/// small (matching the `emit_json_decoder_call` pattern).
/// Emit the typed-target request builders — `Http.defaultRequest` (Url) /
/// `Http.defaultRequestFromString` (String) / `Http.withUrl` (Url, req). Each
/// returns `Result Error HttpRequest`; the error channel appears only in the
/// result, so an explicit `::<IpeError>` turbofish anchors `E`. The
/// fail-closed http/https scheme narrowing lives in the runtime fns these call.
/// Returns `None` for any other callee.
#[inline(never)]
fn emit_http_typed_target_builder(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    match callee {
        Callee::Kernel(
            k @ (KernelFn::HttpDefaultRequest | KernelFn::HttpDefaultRequestFromString),
        ) => {
            let arg = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_typed_target_builder",
                detail: "typed-target builder expects exactly 1 argument".to_owned(),
            })?;
            let arg_str = emit_expr_at(ctx, arg, indent, child, generics)?;
            let name = kernel_name(*k);
            Ok(Some(format!(
                "ipe_runtime::http_client::{name}::<IpeError>({arg_str})"
            )))
        }
        Callee::Kernel(KernelFn::HttpWithUrl) => {
            let url = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_typed_target_builder",
                detail: "HttpWithUrl expects 2 arguments (url, req)".to_owned(),
            })?;
            let req = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_typed_target_builder",
                detail: "HttpWithUrl expects 2 arguments (url, req)".to_owned(),
            })?;
            let url_str = emit_expr_at(ctx, url, indent, child, generics)?;
            let req_str = emit_expr_at(ctx, req, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::http_client::http_with_url::<IpeError>({url_str}, {req_str})"
            )))
        }
        _ => Ok(None),
    }
}

#[inline(never)]
fn emit_http_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    // The three network kernels plus the three typed-target builders need
    // special treatment: all carry an error channel that appears only in the
    // `Result Error _` / `Task Error _` result, so Rust cannot infer the `E`
    // type parameter when the `Err` arm is discarded. Each is emitted with an
    // explicit `::<IpeError>` turbofish. The typed-target builders return a
    // `Result Error HttpRequest` directly (no `task_map` wrapping), so they are
    // handled by `emit_http_typed_target_builder` before the response-shaping
    // network kernels below.
    if let Some(emitted) =
        emit_http_typed_target_builder(ctx, callee, args, indent, child, generics)?
    {
        return Ok(Some(emitted));
    }
    let Callee::Kernel(k @ (KernelFn::HttpGet | KernelFn::HttpPost | KernelFn::HttpRequest)) =
        callee
    else {
        return Ok(None);
    };

    // Resolve the synthesised struct name for the HttpResponse field set
    // {body, headers, status}. The field set is sorted alphabetically;
    // these three names are already in alphabetical order.
    let resp_key: Vec<String> = vec!["body".to_owned(), "headers".to_owned(), "status".to_owned()];
    let resp_struct = ctx
        .record_struct_by_key(&resp_key)
        .map_err(|_| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_http_call",
            detail: "no synthesised struct for HttpResponse fieldset {body, headers, status}; \
                     the lowerer must surface the HttpResponse record type before emission"
                .to_owned(),
        })?;
    let resp_name = &resp_struct.name;

    // Build the task_map conversion closure shared by all three variants.
    // The closure is a pure field-for-field move — soundness note: all
    // fields are owned (String / i64 / HashMap), no borrows, no boxing.
    let conv = format!(
        "|r: ipe_runtime::HttpResponse| {resp_name} {{ \
         body: r.body, headers: r.headers, status: r.status }}"
    );

    match k {
        KernelFn::HttpGet => {
            // Http.get : String -> Task Error HttpResponse
            // args[0] = url : String
            let url = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_call",
                detail: "HttpGet expects exactly 1 argument (url)".to_owned(),
            })?;
            let url_s = emit_expr_at(ctx, url, indent, child, generics)?;
            Ok(Some(format!(
                "task_map(Box::new({conv}), \
                 ipe_runtime::http_client::http_get::<IpeError>({url_s}))"
            )))
        }
        KernelFn::HttpPost => {
            // Http.post : String -> String -> Task Error HttpResponse
            // args[0] = url, args[1] = body
            let url = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_call",
                detail: "HttpPost expects 2 arguments (url, body)".to_owned(),
            })?;
            let body_arg = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_call",
                detail: "HttpPost expects 2 arguments (url, body)".to_owned(),
            })?;
            let url_s = emit_expr_at(ctx, url, indent, child, generics)?;
            let body_s = emit_expr_at(ctx, body_arg, indent, child, generics)?;
            Ok(Some(format!(
                "task_map(Box::new({conv}), \
                 ipe_runtime::http_client::http_post::<IpeError>({url_s}, {body_s}))"
            )))
        }
        KernelFn::HttpRequest => {
            // Http.request : HttpRequest -> Task Error HttpResponse
            // args[0] = req : HttpRequest
            //
            // `HttpRequest` is the opaque nominal type `ir_type_from_ty`
            // folds any solved record shape matching the canonical
            // {body, followRedirects, headers, maxRedirects, method, timeout,
            // url} field set into (`ipe_lower::lower::ir_type_from_ty`'s
            // HTTP_REQUEST_FIELDS special case) — it is ALWAYS backed by
            // `ipe_runtime::HttpRequest`, never a backend-synthesised
            // `record_by_fieldset` struct (which is only populated for
            // shapes the special case does NOT intercept). So `req_expr`'s
            // emitted Rust value already has the runtime's field names —
            // no `record_struct_by_key` lookup needed here.
            let req_expr = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_call",
                detail: "HttpRequest expects exactly 1 argument (req record)".to_owned(),
            })?;
            let req_s = emit_expr_at(ctx, req_expr, indent, child, generics)?;
            // Bind the synthesised request struct once (`__req`) and move each
            // field exactly once into `ipe_runtime::HttpRequest`. The runtime
            // struct uses `#[allow(non_snake_case)]` camelCase field names
            // verbatim — `followRedirects`, `maxRedirects` — so they must match
            // here exactly. The Ipê names emit via `emit_ident` as-is (none are
            // Rust keywords); the runtime names are string literals.
            Ok(Some(format!(
                "({{ let __req = {req_s}; task_map(Box::new({conv}), \
                 ipe_runtime::http_client::http_request::<IpeError>(\
                 ipe_runtime::HttpRequest {{ \
                 method: __req.method, url: __req.url, body: __req.body, \
                 headers: __req.headers, timeout: __req.timeout, \
                 followRedirects: __req.followRedirects, \
                 maxRedirects: __req.maxRedirects }}))\
                 }})"
            )))
        }
        // The non-network Http kernels (HttpParseQuery) fall through to
        // `None` — handled above by the `match k` guard.
        _ => Ok(None),
    }
}

/// Handle Http builder kernel calls that emit inline struct construction or
/// clone-and-reassign record updates.
///
/// Returns `Some(emitted)` for the eight pure builder kernels:
///
/// The typed-target builders (`HttpDefaultRequest` /
/// `HttpDefaultRequestFromString` / `HttpWithUrl`) are NOT handled here: they go
/// through the standard call path to runtime fns that perform the fail-closed
/// http/https scheme narrowing and return `Result Error HttpRequest`.
///
/// * **`HttpWithMethod m req`**, **`HttpWithTimeout t req`**,
///   **`HttpWithBody b req`**,
///   **`HttpWithFollowRedirects f req`**, **`HttpWithMaxRedirects n req`**
///   — each emits a clone-and-reassign
///   block
///   (`{ let mut __ipe_rec = (req).clone(); __ipe_rec.field = val; __ipe_rec }`)
///   matching the `emit_update` pattern so the source record is moved once.
///
/// * **`HttpWithHeader k v req`** — emits a prepend:
///   `{ let mut __ipe_rec = (req).clone(); __ipe_rec.headers.insert(0, (k, v)); __ipe_rec }`.
///   PREPEND (cons-prepend) matches the Go reference implementation in `Http.ipe`
///   (`{ req | headers = (k, v) :: req.headers }`), so `withHeader "B" "2"` after
///   `withHeader "A" "1"` yields `B:2,A:1` in iteration order.
///
/// Returns `None` for any other callee — the caller falls through to the
/// standard call path. Factored out of `emit_expr_at` to keep its stack frame
/// small (same rationale as `emit_http_call`).
#[inline(never)]
#[allow(clippy::too_many_lines)] // 8 match arms × ~20 lines = inherently verbose but linear
fn emit_http_builder_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let Callee::Kernel(
        k @ (KernelFn::HttpWithMethod
        | KernelFn::HttpWithTimeout
        | KernelFn::HttpWithBody
        | KernelFn::HttpWithHeader
        | KernelFn::HttpWithFollowRedirects
        | KernelFn::HttpWithMaxRedirects),
    ) = callee
    else {
        return Ok(None);
    };

    // `HttpRequest` is the opaque nominal type `ir_type_from_ty` folds any
    // solved record shape matching the canonical {body, followRedirects,
    // headers, maxRedirects, method, timeout, url} field set into
    // (`ipe_lower::lower::ir_type_from_ty`'s HTTP_REQUEST_FIELDS special
    // case) — it is ALWAYS backed by `ipe_runtime::HttpRequest`, never a
    // backend-synthesised `record_by_fieldset` struct. So `HttpDefaultRequest`
    // emits the fixed runtime type name directly rather than looking up a
    // synthesised struct: that struct only exists incidentally (when some OTHER
    // signature in the program happens to also carry the same 7-field shape as
    // a plain, non-opaque record — e.g. an explicitly-annotated function
    // parameter). A program whose only `HttpRequest` consumer reads a field or
    // calls `Http.request`/`HttpStream.open` — never spelling the fieldset out
    // in an annotation — synthesises no such struct, and a lookup would hit
    // IPE-I0001. Emitting the fixed runtime type name removes the dependency
    // entirely.
    match k {
        KernelFn::HttpWithMethod => {
            // withMethod : HttpMethod -> HttpRequest -> HttpRequest
            let m = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithMethod expects 2 arguments (method, req)".to_owned(),
            })?;
            let req = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithMethod expects 2 arguments (method, req)".to_owned(),
            })?;
            let m_s = emit_expr_at(ctx, m, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __ipe_rec = ({req_s}).clone(); \
                 __ipe_rec.method = {m_s}; __ipe_rec }}"
            )))
        }
        KernelFn::HttpWithTimeout => {
            // withTimeout : Int -> HttpRequest -> HttpRequest
            let t = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithTimeout expects 2 arguments (timeout, req)".to_owned(),
            })?;
            let req = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithTimeout expects 2 arguments (timeout, req)".to_owned(),
            })?;
            let t_s = emit_expr_at(ctx, t, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __ipe_rec = ({req_s}).clone(); \
                 __ipe_rec.timeout = {t_s}; __ipe_rec }}"
            )))
        }
        KernelFn::HttpWithBody => {
            // withBody : String -> HttpRequest -> HttpRequest
            let b = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithBody expects 2 arguments (body, req)".to_owned(),
            })?;
            let req = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithBody expects 2 arguments (body, req)".to_owned(),
            })?;
            let b_s = emit_expr_at(ctx, b, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __ipe_rec = ({req_s}).clone(); \
                 __ipe_rec.body = {b_s}; __ipe_rec }}"
            )))
        }
        KernelFn::HttpWithFollowRedirects => {
            // withFollowRedirects : Bool -> HttpRequest -> HttpRequest
            let f = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithFollowRedirects expects 2 arguments (flag, req)".to_owned(),
            })?;
            let req = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithFollowRedirects expects 2 arguments (flag, req)".to_owned(),
            })?;
            let f_s = emit_expr_at(ctx, f, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __ipe_rec = ({req_s}).clone(); \
                 __ipe_rec.followRedirects = {f_s}; __ipe_rec }}"
            )))
        }
        KernelFn::HttpWithMaxRedirects => {
            // withMaxRedirects : Int -> HttpRequest -> HttpRequest
            let n = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithMaxRedirects expects 2 arguments (n, req)".to_owned(),
            })?;
            let req = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithMaxRedirects expects 2 arguments (n, req)".to_owned(),
            })?;
            let n_s = emit_expr_at(ctx, n, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __ipe_rec = ({req_s}).clone(); \
                 __ipe_rec.maxRedirects = {n_s}; __ipe_rec }}"
            )))
        }
        KernelFn::HttpWithHeader => {
            // withHeader : String -> String -> HttpRequest -> HttpRequest
            // PREPENDS (key, value) — matches Go reference `(k,v) :: req.headers`.
            let k_arg = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithHeader expects 3 arguments (key, value, req)".to_owned(),
            })?;
            let v_arg = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithHeader expects 3 arguments (key, value, req)".to_owned(),
            })?;
            let req = args.get(2).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_http_builder_call",
                detail: "HttpWithHeader expects 3 arguments (key, value, req)".to_owned(),
            })?;
            let k_s = emit_expr_at(ctx, k_arg, indent, child, generics)?;
            let v_s = emit_expr_at(ctx, v_arg, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __ipe_rec = ({req_s}).clone(); \
                 __ipe_rec.headers.insert(0, ({k_s}, {v_s})); __ipe_rec }}"
            )))
        }
        // Unreachable: the guard at the top of this function constrains `k` to the
        // record-update builder variants matched above. The `_ =>` arm keeps Rust's
        // exhaustiveness checker satisfied without introducing a catch-all over the
        // full `KernelFn` set (which would violate the no-catch-all principle for
        // the logic above).
        _ => Ok(None),
    }
}

/// Handle Db kernel calls that require `SqlValue` / `SqlField` boundary
/// projection.
///
/// The Ipê surface for parameterised Db calls (`Db.exec`, `Db.query`,
/// `Db.queryDecode`, `Db.insertFields`, `Db.updateFields`,
/// `Db.insertFieldsReturning`) passes a `List SqlValue` or
/// `List (String, SqlField)` as a plain Ipê argument. The runtime's typed-param
/// functions (`db_exec_params`, `db_query_params`, …) expect `Vec<SqlParam>` /
/// `Vec<(String, Option<SqlParam>)>`. The projection is emitted INLINE at the
/// call site — the Ipê list is converted with a short `.into_iter().map(…)`
/// chain so the compiler never needs separate IR types for the two.
///
/// Kernels that accept only `Db` / `String` / `Int` / plain Dict arguments (no
/// `SqlValue` / `SqlField` in the parameter list) return `None` here and fall
/// through to the standard `name(args)` path.
///
/// Factored out of `emit_expr_at` to keep that function's stack frame small
/// (same rationale as `emit_http_call`).
#[inline(never)]
#[allow(clippy::too_many_lines)]
// linear dispatch over many projection cases
/// Emit `Task.retryWith` and all `RetryPolicy` builder kernels.
///
/// Design rationale:
/// - `RetryPolicy e` is a closed record with a function field `shouldRetry : e ->
///   Bool`.  Because `Box<dyn Fn>` is not `Clone`, builders use MOVE semantics
///   (`let mut __ipe_rec = (rec); __ipe_rec.field = val; __ipe_rec`) rather than
///   `.clone()`.
/// - `Task.retryWith` decomposes the policy and calls the runtime function
///   `ipe_runtime::task::task_retry_with`, adapting the `Box<dyn Fn(E) -> bool>`
///   field to the `impl Fn(&E) -> bool` expected by the runtime via a cloning
///   adapter closure.
fn emit_task_retry_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let Callee::Kernel(
        k @ (KernelFn::TaskRetryWith
        | KernelFn::TaskLinearBackoff
        | KernelFn::TaskExponentialBackoff
        | KernelFn::TaskWithJitter
        | KernelFn::TaskRetryOn
        | KernelFn::TaskWithRetryOn
        | KernelFn::TaskDefaultRetryPolicy
        | KernelFn::TaskWithMaxAttempts
        | KernelFn::TaskWithBaseMs
        | KernelFn::TaskWithKind),
    ) = callee
    else {
        return Ok(None);
    };

    // `RetryPolicy e = { baseMs, jitter, kind, maxAttempts, shouldRetry }` —
    // alphabetical BTreeMap order matches the emitted struct name.
    let rp_key: Vec<String> = vec![
        "baseMs".to_owned(),
        "jitter".to_owned(),
        "kind".to_owned(),
        "maxAttempts".to_owned(),
        "shouldRetry".to_owned(),
    ];
    // Only builders that construct a new struct need the struct name.
    // For the pure move-update builders (TaskWithJitter etc.) we look it up too
    // so the pattern is consistent; a missing struct signals a lowering bug.
    let rp_name = ctx
        .record_struct_by_key(&rp_key)
        .map_err(|_| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_task_retry_call",
            detail: "no synthesised struct for RetryPolicy fieldset \
                 {baseMs, jitter, kind, maxAttempts, shouldRetry}; \
                 the lowerer must surface the RetryPolicy record type before emission"
                .to_owned(),
        })?
        .name
        .clone();

    match k {
        KernelFn::TaskDefaultRetryPolicy => {
            // `defaultRetryPolicy : RetryPolicy e` — 0-arg, emit inline literal.
            // 3 attempts, 500 ms, exponential (kind=1), no jitter, always-retry.
            Ok(Some(format!(
                "{rp_name} {{ baseMs: 500i64, jitter: false, kind: 1i64, \
                 maxAttempts: 3i64, shouldRetry: Box::new(|_: IpeError| true) }}"
            )))
        }
        KernelFn::TaskLinearBackoff => {
            // `linearBackoff maxAttempts delayMs` — constant delay, kind=0.
            let n = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskLinearBackoff expects 2 arguments (maxAttempts, delayMs)".to_owned(),
            })?;
            let ms = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskLinearBackoff expects 2 arguments (maxAttempts, delayMs)".to_owned(),
            })?;
            let n_s = emit_expr_at(ctx, n, indent, child, generics)?;
            let ms_s = emit_expr_at(ctx, ms, indent, child, generics)?;
            Ok(Some(format!(
                "{rp_name} {{ baseMs: {ms_s}, jitter: false, kind: 0i64, \
                 maxAttempts: {n_s}, shouldRetry: Box::new(|_: IpeError| true) }}"
            )))
        }
        KernelFn::TaskExponentialBackoff => {
            // `exponentialBackoff maxAttempts baseMs` — exponential, kind=1.
            let n = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskExponentialBackoff expects 2 arguments (maxAttempts, baseMs)"
                    .to_owned(),
            })?;
            let ms = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskExponentialBackoff expects 2 arguments (maxAttempts, baseMs)"
                    .to_owned(),
            })?;
            let n_s = emit_expr_at(ctx, n, indent, child, generics)?;
            let ms_s = emit_expr_at(ctx, ms, indent, child, generics)?;
            Ok(Some(format!(
                "{rp_name} {{ baseMs: {ms_s}, jitter: false, kind: 1i64, \
                 maxAttempts: {n_s}, shouldRetry: Box::new(|_: IpeError| true) }}"
            )))
        }
        KernelFn::TaskWithJitter => {
            // `withJitter policy` — set jitter=true, MOVE the record.
            let policy = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskWithJitter expects 1 argument (policy)".to_owned(),
            })?;
            let policy_s = emit_expr_at(ctx, policy, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __ipe_rec = ({policy_s}); __ipe_rec.jitter = true; __ipe_rec }}"
            )))
        }
        KernelFn::TaskWithMaxAttempts => {
            // `withMaxAttempts n policy` — move-update maxAttempts.
            let n = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskWithMaxAttempts expects 2 arguments (n, policy)".to_owned(),
            })?;
            let policy = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskWithMaxAttempts expects 2 arguments (n, policy)".to_owned(),
            })?;
            let n_s = emit_expr_at(ctx, n, indent, child, generics)?;
            let policy_s = emit_expr_at(ctx, policy, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __ipe_rec = ({policy_s}); __ipe_rec.maxAttempts = {n_s}; __ipe_rec }}"
            )))
        }
        KernelFn::TaskWithBaseMs => {
            // `withBaseMs ms policy` — move-update baseMs.
            let ms = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskWithBaseMs expects 2 arguments (ms, policy)".to_owned(),
            })?;
            let policy = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskWithBaseMs expects 2 arguments (ms, policy)".to_owned(),
            })?;
            let ms_s = emit_expr_at(ctx, ms, indent, child, generics)?;
            let policy_s = emit_expr_at(ctx, policy, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __ipe_rec = ({policy_s}); __ipe_rec.baseMs = {ms_s}; __ipe_rec }}"
            )))
        }
        KernelFn::TaskWithKind => {
            // `withKind k policy` — move-update kind.
            let k_arg = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskWithKind expects 2 arguments (k, policy)".to_owned(),
            })?;
            let policy = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskWithKind expects 2 arguments (k, policy)".to_owned(),
            })?;
            let k_s = emit_expr_at(ctx, k_arg, indent, child, generics)?;
            let policy_s = emit_expr_at(ctx, policy, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __ipe_rec = ({policy_s}); __ipe_rec.kind = {k_s}; __ipe_rec }}"
            )))
        }
        KernelFn::TaskRetryOn | KernelFn::TaskWithRetryOn => {
            // `retryOn pred policy` / `withRetryOn pred policy` — move-update shouldRetry.
            let pred = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskRetryOn/TaskWithRetryOn expects 2 arguments (pred, policy)".to_owned(),
            })?;
            let policy = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskRetryOn/TaskWithRetryOn expects 2 arguments (pred, policy)".to_owned(),
            })?;
            let pred_s = emit_expr_at(ctx, pred, indent, child, generics)?;
            let policy_s = emit_expr_at(ctx, policy, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __ipe_rec = ({policy_s}); \
                 __ipe_rec.shouldRetry = {pred_s}; __ipe_rec }}"
            )))
        }
        KernelFn::TaskRetryWith => {
            // `retryWith policy task` — decompose policy, call runtime.
            // The `shouldRetry` field is `Box<dyn Fn(IpeError) -> bool>` but
            // `task_retry_with` expects `impl Fn(&IpeError) -> bool`.  The adapter
            // closure bridges the gap by cloning the (cheap String) error ref.
            let policy = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskRetryWith expects 2 arguments (policy, task)".to_owned(),
            })?;
            let task = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_task_retry_call",
                detail: "TaskRetryWith expects 2 arguments (policy, task)".to_owned(),
            })?;
            let policy_s = emit_expr_at(ctx, policy, indent, child, generics)?;
            let task_s = emit_expr_at(ctx, task, indent, child, generics)?;
            Ok(Some(format!(
                "{{ \
                 let __ipe_p = {policy_s}; \
                 let __ipe_sr = __ipe_p.shouldRetry; \
                 ipe_runtime::task::task_retry_with(\
                 __ipe_p.maxAttempts, \
                 __ipe_p.baseMs, \
                 __ipe_p.jitter, \
                 __ipe_p.kind, \
                 move |__ipe_e: &IpeError| (__ipe_sr)(__ipe_e.clone()), \
                 move || {{ {task_s} }}\
                 ) }}"
            )))
        }
        _ => Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_task_retry_call",
            detail: "non-retry kernel reached retry dispatch arm — guard should have excluded it"
                .to_owned(),
        }),
    }
}

// The match below lists standard-path Db kernels explicitly (same Ok(None) body
// as the wildcard) so that any future param-taking Db kernel added to `KernelFn`
// that NEEDS a custom arm causes a *compile error* here — not a silent
// exit-0-then-cargo-fail when `_ => Ok(None)` swallows it.
// `match_same_arms` fires because both the list and `_` return `Ok(None)`; the
// documentation value justifies the suppression.  `too_many_lines` fires because
// the function explicitly enumerates every Db kernel arm for compile-time
// completeness; extracting sub-helpers would hide the intentional coverage.
#[allow(clippy::match_same_arms, clippy::too_many_lines)]
fn emit_db_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    // Fast path: not a Db kernel at all.
    let Callee::Kernel(k) = callee else {
        return Ok(None);
    };

    // Helper: emit a single arg by index, returning a CompilerBug on miss.
    macro_rules! arg {
        ($idx:expr, $name:literal) => {
            args.get($idx).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_db_call",
                detail: format!("Db kernel {:?} missing arg[{}] ({})", k, $idx, $name),
            })
        };
    }

    // Projection snippets.
    //
    // `project_params(s)` — `List a` → `Vec<SqlParam>`
    //
    // Maps via `Into::into` (NOT `SqlParam::from`) and collects into the
    // EXPLICIT `Vec<ipe_runtime::db::SqlParam>` (not `Vec<_>`), so the
    // projection compiles both for a concrete element type AND for a still-
    // generic one:
    //   • `String` / `i64` / `f64` / `bool` / `StdDbSqlValue` — each has a
    //     `From<T> for SqlParam` impl in the runtime (the generated one is
    //     emitted by `ipe_backend_rust::project::emit_db_projection_impls`);
    //     std's blanket `impl<T, U: From<T>> Into<U> for T` makes `.into()`
    //     resolve identically to the old `SqlParam::from(x)` call for every
    //     one of them — no behaviour change for a concrete element type.
    //   • A still-generic `T{n}` (a Ipê wrapper function forwarding its own
    //     `List a` parameter into `Db.exec` / `Db.query` / `Db.queryDecode`,
    //     e.g. `Database.exec label sql args` in `examples/17-ipemon`) can
    //     only be bounded via the STANDARD `<T{n}: Trait>` generic-parameter
    //     list — a `where SqlParam: From<T{n}>` clause bounds the WRONG type
    //     (`SqlParam`, not `T{n}`) and cannot be expressed that way. The
    //     lowerer's `BoundSet::SQL_PARAM` instead emits `T{n}: …
    //     Into<ipe_runtime::db::SqlParam>` (see `render_bounds`), which
    //     `.into()` — but NOT `SqlParam::from` — can actually call inside a
    //     still-generic function body.
    // This mirrors `exec : Db -> String -> List a -> Task Error Int` (polymorphic
    // `List a`, not fixed to `List SqlValue`).
    let project_params = |s: &str| {
        // Empty-list fast path: `Vec::new()` has no elements, so Rust cannot
        // infer which `Into<SqlParam>` impl to use — the turbofish form names
        // the element type explicitly and skips the map/collect entirely.
        // Kept as defence-in-depth (the type-checker's defaulting normally
        // gives an empty Ipê `[]` literal a concrete `SqlValue` element type
        // before it ever reaches this closure — see the `sql_param` arm of
        // the numeric-defaulting loop in `ipe_types::lib` — but a bare
        // `Vec::new()` remains a possible input from any other empty-list
        // source, e.g. a Ipê-level `List.filter (always False) xs`).
        if s == "Vec::new()" {
            return "Vec::<ipe_runtime::db::SqlParam>::new()".to_string();
        }
        format!(
            "({s}).into_iter().map(::core::convert::Into::into)\
             .collect::<Vec<ipe_runtime::db::SqlParam>>()"
        )
    };
    // `project_fields(s)` — `List (String, SqlField)` → `Vec<(String, Option<SqlParam>)>`
    let project_fields = |s: &str| {
        format!(
            "({s}).into_iter().map(|(__k, __v)| (__k, __v.into_field_param()))\
             .collect::<Vec<_>>()"
        )
    };
    // `project_where(s)` — `List (String, SqlValue)` → `Vec<(String, SqlParam)>`
    // `SqlValue` elements here are always the concrete generated type (not
    // polymorphic), so we keep the explicit `into_sql_param()` call.
    let project_where = |s: &str| {
        format!(
            "({s}).into_iter().map(|(__k, __v)| (__k, __v.into_sql_param()))\
             .collect::<Vec<_>>()"
        )
    };

    match k {
        // ── DbExecRaw: (conn, sql) — DDL / no-param statements ──────────────
        //
        // The connection is cloned here (and in every other task-returning Db
        // kernel below) because the emitter wraps sequential Db calls in nested
        // `task_and_then(effect, move |_| { … })` continuations.  Rust evaluates
        // function arguments left-to-right: the EFFECT is built first (arg 0),
        // which would MOVE the `conn` binding, leaving the continuation closure
        // unable to capture it.  Cloning at each call site is the idiomatic fix
        // for `Arc`-backed handles; the `Db` type wraps an `Arc<Pool<…>>` so
        // cloning is cheap (pointer increment only).
        KernelFn::DbExecRaw => {
            let conn_e = arg!(0, "conn")?;
            let sql_e = arg!(1, "sql")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let sql_s = emit_expr_at(ctx, sql_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!("{fn_name}({conn_s}.clone(), {sql_s})")))
        }
        // ── DbExec / DbQuery: (conn, sql, List SqlValue) ────────────────────
        KernelFn::DbExec | KernelFn::DbQuery => {
            let conn_e = arg!(0, "conn")?;
            let sql_e = arg!(1, "sql")?;
            let params_e = arg!(2, "params")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let sql_s = emit_expr_at(ctx, sql_e, indent, child, generics)?;
            let params_s = emit_expr_at(ctx, params_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {sql_s}, {})",
                project_params(&params_s)
            )))
        }
        // ── DbQueryDecode: (conn, sql, List SqlValue, decoder) ──────────────
        KernelFn::DbQueryDecode => {
            let conn_e = arg!(0, "conn")?;
            let sql_e = arg!(1, "sql")?;
            let params_e = arg!(2, "params")?;
            let dec_e = arg!(3, "decoder")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let sql_s = emit_expr_at(ctx, sql_e, indent, child, generics)?;
            let params_s = emit_expr_at(ctx, params_e, indent, child, generics)?;
            let dec_s = emit_expr_at(ctx, dec_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {sql_s}, {}, {dec_s})",
                project_params(&params_s)
            )))
        }
        // ── DbInsertFields: (conn, table, List (String, SqlField)) ───────────
        KernelFn::DbInsertFields => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let fields_e = arg!(2, "fields")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let fields_s = emit_expr_at(ctx, fields_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {})",
                project_fields(&fields_s)
            )))
        }
        // ── DbUpdateFields: (conn, table, List (String,SqlValue), List (String,SqlField)) ─
        KernelFn::DbUpdateFields => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let where_e = arg!(2, "where_cols")?;
            let set_e = arg!(3, "set_fields")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let where_s = emit_expr_at(ctx, where_e, indent, child, generics)?;
            let set_s = emit_expr_at(ctx, set_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {}, {})",
                project_where(&where_s),
                project_fields(&set_s)
            )))
        }
        // ── DbInsertFieldsReturning: (conn, table, List (String, SqlField), projection, decoder) ─
        KernelFn::DbInsertFieldsReturning => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let fields_e = arg!(2, "fields")?;
            let proj_e = arg!(3, "projection")?;
            let dec_e = arg!(4, "decoder")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let fields_s = emit_expr_at(ctx, fields_e, indent, child, generics)?;
            let proj_s = emit_expr_at(ctx, proj_e, indent, child, generics)?;
            let dec_s = emit_expr_at(ctx, dec_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {}, {proj_s}, {dec_s})",
                project_fields(&fields_s)
            )))
        }
        // ── DbInsertRow: (conn, table, row: Dict String String) ────────────────
        // The Ipe surface is upstream-parity `Dict String String` (bdbc572);
        // `Dict String String` already lowers to `HashMap<String, String>`
        // (the runtime function's own parameter type), so `row_s` passes
        // straight through with no conversion.
        KernelFn::DbInsertRow => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let row_e = arg!(2, "row")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let row_s = emit_expr_at(ctx, row_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {row_s})"
            )))
        }
        // ── DbUpdateById: (conn, table, id, row: Dict String String) ───────────
        // Same no-conversion-needed rationale as DbInsertRow above.
        KernelFn::DbUpdateById => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let id_e = arg!(2, "id")?;
            let row_e = arg!(3, "row")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let id_s = emit_expr_at(ctx, id_e, indent, child, generics)?;
            let row_s = emit_expr_at(ctx, row_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {id_s}, {row_s})"
            )))
        }
        // ── DbWithTransaction: (conn, body: Db -> Task e a) → Task e a ────────
        // Clone ensures the pool handle remains usable for any Db calls that
        // follow the `withTransaction` in the same continuation chain.  The body
        // closure itself receives its own pool copy through the task-local routing
        // (see `db_with_transaction` in the runtime), so the clone never causes an
        // extra SQLite connection.
        KernelFn::DbWithTransaction => {
            let conn_e = arg!(0, "conn")?;
            let body_e = arg!(1, "body")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let body_s = emit_expr_at(ctx, body_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!("{fn_name}({conn_s}.clone(), {body_s})")))
        }
        // ── DbMigrate: (conn, List Migration) → Task e (List String) ──
        // `Migration` is the record alias `{ name : String, sql : String }`
        // (reference `Std/Db.ipe:237`), lowered to the synthesised struct with
        // those two fields. The runtime `db_migrate_apply` takes `Vec<(String,
        // String)>`, so map each record to a `(name, sql)` tuple — the exact
        // shape the reference's pure-Ipê `migrate` produces via `List.map (\m ->
        // (m.name, m.sql))`.
        KernelFn::DbMigrate => {
            let conn_e = arg!(0, "conn")?;
            let migrations_e = arg!(1, "migrations")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let migrations_s = emit_expr_at(ctx, migrations_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {migrations_s}.into_iter()\
                 .map(|__m| (__m.name, __m.sql)).collect::<Vec<(String, String)>>())"
            )))
        }
        // ── DbDefaultMigration: String -> Migration ──────────────────────────
        // Pure record builder — a `Migration` named with an empty SQL body
        // (reference `Std/Db.ipe:246`). Emitted inline as the synthesised
        // `{ name, sql }` struct literal so no runtime kernel is required.
        KernelFn::DbDefaultMigration => {
            let name_e = arg!(0, "name")?;
            let name_s = emit_expr_at(ctx, name_e, indent, child, generics)?;
            let key = vec!["name".to_owned(), "sql".to_owned()];
            let struct_name = ctx.record_name_for_literal(&key)?.to_owned();
            Ok(Some(format!(
                "{struct_name} {{ name: {name_s}, sql: String::new() }}"
            )))
        }
        // ── DbGetById: (conn, table, id) ────────────────────────────────────
        // Conn must be cloned so subsequent Db calls in the same continuation
        // chain can still capture it (Pool<Sqlite> is not Copy).
        KernelFn::DbGetById => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let id_e = arg!(2, "id")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let id_s = emit_expr_at(ctx, id_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {id_s})"
            )))
        }
        // ── DbDeleteById: (conn, table, id) ─────────────────────────────────
        KernelFn::DbDeleteById => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let id_e = arg!(2, "id")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let id_s = emit_expr_at(ctx, id_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {id_s})"
            )))
        }
        // ── DbFindOneByField: (conn, table, field, value) ────────────────────
        KernelFn::DbFindOneByField => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let field_e = arg!(2, "field")?;
            let value_e = arg!(3, "value")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let field_s = emit_expr_at(ctx, field_e, indent, child, generics)?;
            let value_s = emit_expr_at(ctx, value_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {field_s}, {value_s})"
            )))
        }
        // ── DbFindManyByField: (conn, table, field, value) ───────────────────
        KernelFn::DbFindManyByField => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let field_e = arg!(2, "field")?;
            let value_e = arg!(3, "value")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let field_s = emit_expr_at(ctx, field_e, indent, child, generics)?;
            let value_s = emit_expr_at(ctx, value_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {field_s}, {value_s})"
            )))
        }
        // ── DbGet*: (field, row) — row is passed by reference so the same row
        // binding can be used in multiple consecutive accessor calls within a
        // single expression (e.g. inside a `list_map_consume` lambda that reads
        // several columns). The runtime functions take `row: &R where R: IpeRow`.
        KernelFn::DbGetString | KernelFn::DbGetInt | KernelFn::DbGetBool | KernelFn::DbGetField => {
            let field_e = arg!(0, "field")?;
            let row_e = arg!(1, "row")?;
            let field_s = emit_expr_at(ctx, field_e, indent, child, generics)?;
            let row_s = emit_expr_at(ctx, row_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!("{fn_name}({field_s}, &({row_s}))")))
        }
        // ── DbFindByConditions: (conn, table, conditions: Dict String String) ──
        //
        // The runtime `db_find_by_conditions` takes `HashMap<String, String>` —
        // identical to the IR's `Dict String String` representation — so no
        // conversion is needed beyond passing the value through.
        KernelFn::DbFindByConditions => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let conditions_e = arg!(2, "conditions")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let conditions_s = emit_expr_at(ctx, conditions_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {conditions_s})"
            )))
        }
        // ── Db.findWhere / Db.deleteWhere: (conn, table, frag: SqlFragment) ──
        //
        // The `SqlFragment`-typed replacement for the removed `unsafeFindWhere`
        // `frag` is a bare struct value (no `List` projection
        // needed) — only the `conn.clone()` treatment (shared by every other
        // Task-returning Db kernel here) is special-cased.
        KernelFn::DbFindWhere | KernelFn::DbDeleteWhere => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let frag_e = arg!(2, "frag")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let frag_s = emit_expr_at(ctx, frag_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {frag_s})"
            )))
        }
        // ── Sql.inList: (frag: SqlFragment, values: List SqlValue) ───────────
        //
        // `values` needs the same `List SqlValue` → `Vec<SqlParam>` projection
        // as `DbExec`/`DbQuery`'s params argument.
        KernelFn::SqlInList => {
            let frag_e = arg!(0, "frag")?;
            let values_e = arg!(1, "values")?;
            let frag_s = emit_expr_at(ctx, frag_e, indent, child, generics)?;
            let values_s = emit_expr_at(ctx, values_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({frag_s}, {})",
                project_params(&values_s)
            )))
        }
        // ── Standard-path Db kernels ────────────────────────────────────────────
        //
        // The kernels below route through `emit_call`'s standard path — their
        // argument types emit correctly without special-case projection.  List
        // them explicitly so any future param-taking Db kernel that needs a custom
        // arm is a compile error here, not a silent exit-0-then-cargo-fail.
        KernelFn::DbConnect
        | KernelFn::DbOpen
        | KernelFn::DbClose
        | KernelFn::DbDecString
        | KernelFn::DbDecInt
        | KernelFn::DbDecFloat
        | KernelFn::DbDecBool
        | KernelFn::DbDecNullable
        | KernelFn::DbDecMap
        | KernelFn::DbDecAndThen
        | KernelFn::DbDecSucceed
        | KernelFn::DbDecFail
        | KernelFn::DbDecMap2
        | KernelFn::DbDecMap3
        | KernelFn::DbDecMap4
        | KernelFn::DbDecRequired
        | KernelFn::DbDecOptional
        | KernelFn::DbDecMoney
        // `Sql.column`/`param`/`int`/`string`/`float`/`bool`/`eq`/`ne`/`gt`/`lt`/
        // `gte`/`lte`/`and`/`or`/`not`/`isNull`/`isNotNull`/`like` take plain
        // scalar or `SqlFragment` args — no `Db` handle, no List projection.
        | KernelFn::SqlColumn
        | KernelFn::SqlParam
        | KernelFn::SqlInt
        | KernelFn::SqlString
        | KernelFn::SqlFloat
        | KernelFn::SqlBool
        | KernelFn::SqlEq
        | KernelFn::SqlNe
        | KernelFn::SqlGt
        | KernelFn::SqlLt
        | KernelFn::SqlGte
        | KernelFn::SqlLte
        | KernelFn::SqlAnd
        | KernelFn::SqlOr
        | KernelFn::SqlNot
        | KernelFn::SqlIsNull
        | KernelFn::SqlIsNotNull
        | KernelFn::SqlLike => Ok(None),
        // A Db kernel that reached this arm is a compiler bug: either add a
        // custom projection arm above, or add it to the standard-path list.
        // This arm is unreachable for any KernelFn variant listed above, so
        // its only way to fire is a newly-added Db* variant that was not wired
        // into either list — making the miss a compile-time-hard error rather
        // than a silent exit-0-then-cargo-fail.
        _ if k.is_db() => Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_db_call",
            detail: format!(
                "unprojected Db kernel {k:?}: add a custom projection arm \
                 or a standard-path entry to emit_db_call"
            ),
        }),
        // Non-Db kernel: let the standard call path handle it.
        _ => Ok(None),
    }
}

/// Handle TEA (`Cmd` / `Sub` / `Time.every`) kernel calls that require custom
/// argument wiring.
///
/// Returns `Some(emitted)` for:
///
/// * **`CmdNone` / `SubNone`** — zero-arg constructors; the runtime functions
///   take no arguments, so we emit `cmd_none()` / `sub_none()` rather than
///   going through the default N-arg path.
///
/// * **`CmdBatch` / `SubBatch`** — `List (Cmd msg) -> Cmd msg`; the list
///   argument is passed directly to the runtime (its `IrType::List` renders as
///   a Rust `Vec`), so we emit `cmd_batch(<list_expr>)` /
///   `sub_batch(<list_expr>)`.  (A previous version of this doc stated that the
///   argument was materialised via `vec_from_ipe_list`; that was never the
///   actual code path — the emitted list expression already has `Vec` type.)
///
/// * **`CmdPerform`** — `Task Error a -> (Result Error a -> msg) -> Cmd msg`;
///   the callback must be boxed as a `Box<dyn Fn(IpeResult<A>) -> M + Send + 'static>`.
///   Emits `cmd_perform(<task>, Box::new(<f>))`.
///
/// * **`SubEvery` / `TimeEvery`** — `Int -> msg -> Sub msg`; these pass
///   through the standard N-arg path (no custom boxing needed), returning
///   `Ok(None)` so the standard emitter handles them.
///
/// Returns `Err(CompilerBug)` for any `k.is_tea()` variant that is:
///
/// * **reserved, not emittable here** (`CmdPublish`, `CmdPublishNoEcho`,
///   `SubSubscribeTopic`) — guard fires if a program somehow reaches one (e.g.
///   if `lower_callee` mis-routes it); not user-reachable.
///
/// Returns `Ok(None)` for non-TEA callees so the standard path handles them.
#[allow(clippy::match_same_arms, clippy::too_many_lines)]
fn emit_tea_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let Callee::Kernel(k) = callee else {
        return Ok(None);
    };
    if !k.is_tea() {
        return Ok(None);
    }

    macro_rules! arg {
        ($idx:expr, $name:literal) => {
            args.get($idx).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_tea_call",
                detail: format!("TEA kernel {:?} missing arg[{}] ({})", k, $idx, $name),
            })
        };
    }

    match k {
        // ── Arity-0: nullary TEA constructors ──────────────────────────────────
        // `Cmd.none : Cmd msg`  →  `cmd_none()`
        KernelFn::CmdNone => Ok(Some("cmd_none()".to_owned())),
        // `Sub.none : Sub msg`  →  `sub_none()`
        KernelFn::SubNone => Ok(Some("sub_none()".to_owned())),
        // ── Arity-1: list-of-cmds / list-of-subs ────────────────────────────────
        // `Cmd.batch : List (Cmd msg) -> Cmd msg`
        KernelFn::CmdBatch => {
            let list_e = arg!(0, "list")?;
            let list_s = emit_expr_at(ctx, list_e, indent, child, generics)?;
            Ok(Some(format!("cmd_batch({list_s})")))
        }
        // `Sub.batch : List (Sub msg) -> Sub msg`
        KernelFn::SubBatch => {
            let list_e = arg!(0, "list")?;
            let list_s = emit_expr_at(ctx, list_e, indent, child, generics)?;
            Ok(Some(format!("sub_batch({list_s})")))
        }
        // ── Arity-2: Cmd.perform (requires boxing the callback) ─────────────────
        // `Cmd.perform : Task Error a -> (Result Error a -> msg) -> Cmd msg`
        // Emits: `cmd_perform(<task>, <f>)`
        // The runtime's `cmd_perform` signature already boxes the callback,
        // so we can pass the emitted closure expression directly.
        KernelFn::CmdPerform => {
            let task_e = arg!(0, "task")?;
            let f_e = arg!(1, "to_msg")?;
            let task_s = emit_expr_at(ctx, task_e, indent, child, generics)?;
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!("cmd_perform({task_s}, {f_s})")))
        }
        // ── Task.attempt : (Result Error a -> msg) -> Task Error a -> Cmd msg ──
        // Elm's arg order is `(to_msg, task)`; the runtime `cmd_perform` takes
        // `(task, to_msg)` (the exact `Cmd.perform` bridge), so the two args are
        // emitted swapped. Reuses `cmd_perform` — no dedicated runtime symbol.
        KernelFn::TaskAttempt => {
            let f_e = arg!(0, "to_msg")?;
            let task_e = arg!(1, "task")?;
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            let task_s = emit_expr_at(ctx, task_e, indent, child, generics)?;
            Ok(Some(format!("cmd_perform({task_s}, {f_s})")))
        }
        // ── Arity-2: Cmd.map / Sub.map (retag a sub-component's effects) ─────────
        // `Cmd.map : (a -> msg) -> Cmd a -> Cmd msg`  →  `cmd_map(<cmd>, <f>)`
        // `Sub.map : (a -> msg) -> Sub a -> Sub msg`  →  `sub_map(<sub>, <f>)`
        // The Ipê argument order is `(f, effect)`; the runtime takes
        // `(effect, f)` (effect first so `f` infers its `A` from the effect's
        // message type), so the two args are emitted swapped. `f` is passed
        // through unboxed — `cmd_map`/`sub_map` are generic over `F: Fn(A) -> M`
        // and share it via `Arc` internally, so the emitted closure value binds
        // directly with no re-wrap.
        KernelFn::CmdMap | KernelFn::SubMap => {
            let f_e = arg!(0, "f")?;
            let effect_e = arg!(1, "effect")?;
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            let effect_s = emit_expr_at(ctx, effect_e, indent, child, generics)?;
            let name = kernel_name(*k); // "cmd_map" / "sub_map"
            Ok(Some(format!("{name}({effect_s}, {f_s})")))
        }
        // ── Arity-2: tick subscriptions — standard path ──────────────────────────
        // `Sub.every : Int -> msg -> Sub msg` and
        // `Time.every : Int -> msg -> Sub msg`
        // Both pass through the default N-arg emitter (no boxing needed).
        KernelFn::SubEvery | KernelFn::TimeEvery => Ok(None),
        // ── Arity-2: pub/sub subscription — standard path ────────────────────────
        // `Sub.subscribeTopic : String -> (any -> msg) -> Sub msg`
        // The runtime `sub_subscribe_topic` is in live/pubsub.rs (live-feature
        // gated). The payload type T is resolved by Rust's type inference from
        // the matching `cmd_publish` call site; no boxing required here.
        KernelFn::SubSubscribeTopic => Ok(None),
        // `Http.Stream.chunks : Int -> (ChunkEvent -> msg) -> Sub msg`
        // Uses the same generic N-arg emit path as SubSubscribeTopic.
        // The runtime symbol `sub_subscribe_stream` is defined in http_stream.rs.
        //
        // This arm passes the boxed handler through unchanged (`Ok(None)`).
        // `to_msg` is moved exclusively into one detached `tokio::spawn` task,
        // never shared behind an `Arc`, so `Sync` is not structurally required;
        // the runtime signature bounds the handler `Send`-only (matching
        // `sub_subscribe_topic` — see `sub_subscribe_stream`'s doc comment in
        // `http_stream.rs`), so a plain `Box<dyn Fn(..) + Send>` satisfies it
        // with no re-wrap (avoiding E0277). Contrast with the sibling
        // `KernelFn::StreamStream` (`emit_server_call`), which DOES need the
        // re-wrap-in-a-fresh-closure technique because its runtime consumer
        // genuinely stores the handler behind a shared `Arc`.
        KernelFn::HttpStreamChunks => Ok(None),
        // ── Cmd.publish / Cmd.publishNoEcho ──────────────────────────────────────
        // `Cmd.publish : String -> Dict String String -> Cmd msg`
        // `Cmd.publishNoEcho : String -> Dict String String -> Cmd msg`
        // Both map to the standard N-arg emit path (runtime live/pubsub.rs).
        KernelFn::CmdPublish | KernelFn::CmdPublishNoEcho => Ok(None),
        // (`Ipe.PubSub.publish` / `publishNoEcho` are `class = Web`, Task-shaped —
        // emitted in `emit_ui_call`, not here. They are not TEA-loop kernels.)
        // ── Ipe.WebSocket: onOpen / onMessage / onClose / onError ───────────
        // `Sub_subscribeWebSocket : Int -> String -> (any -> msg) -> Sub msg`.
        //
        // The four `on*` stdlib wrappers all funnel through this single
        // `any`-typed kernel with a compile-time-literal `kind`
        // ("open" / "message" / "close" / "error"), because their heterogeneous
        // toMsg shapes (bare `msg` / `WebSocketMessage -> msg` / `CloseCode -> msg`
        // / `Error -> msg`) can't share one bounded Rust fn. This peephole does the
        // split a stdlib override would otherwise do: route by the literal `kind`
        // to a per-kind TYPED runtime fn (`sub_subscribe_ws_{open,message,close,
        // error}`), passing only the socket id + toMsg (the `kind` arg is consumed
        // here, never emitted). Mirrors the Go/Haskell reference's
        // `ExprEmitter.hs` peephole. Each runtime fn moves `to_msg` into exactly
        // one detached `tokio::spawn` task (never behind a shared `Arc`), so the
        // generic `Box<dyn Fn(..) -> .. + Send + 'static>` codegen value passes
        // straight through — no re-wrap needed (unlike `StreamStream`).
        KernelFn::SubSubscribeWebSocket => {
            let raw_e = arg!(0, "socketId")?;
            let kind_e = arg!(1, "kind")?;
            let to_msg_e = arg!(2, "toMsg")?;
            // The `kind` MUST be a compile-time string literal — the four stdlib
            // wrappers always pass one. A non-literal is a malformed call the
            // stdlib can't produce; fail closed (SEAL) rather than guess a kind.
            let Expr::Str(kind) = kind_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_tea_call::SubSubscribeWebSocket",
                    detail: "Sub.subscribeWebSocket requires a compile-time-literal kind \
                             (\"open\"/\"message\"/\"close\"/\"error\") — the four on* stdlib \
                             wrappers always pass one"
                        .to_owned(),
                });
            };
            let fn_name = match kind.as_str() {
                "open" => "sub_subscribe_ws_open",
                "message" => "sub_subscribe_ws_message",
                "close" => "sub_subscribe_ws_close",
                "error" => "sub_subscribe_ws_error",
                other => {
                    return Err(Diagnostic::CompilerBug {
                        where_: "ipe_backend_rust::emit_tea_call::SubSubscribeWebSocket",
                        detail: format!(
                            "Sub.subscribeWebSocket got unknown kind {other:?} — \
                             expected \"open\"/\"message\"/\"close\"/\"error\""
                        ),
                    });
                }
            };
            let raw_s = emit_expr_at(ctx, raw_e, indent, child, generics)?;
            let to_msg_s = emit_expr_at(ctx, to_msg_e, indent, child, generics)?;
            Ok(Some(format!("{fn_name}({raw_s}, {to_msg_s})")))
        }
        // Any other `k.is_tea()` variant not listed above is a new wired variant
        // that needs an explicit arm.  The `is_tea()` guard at the top of this
        // function means this arm is a hard compile-time-visible gap rather than
        // a silent `Ok(None)` pass-through.
        _ => Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_tea_call",
            detail: format!(
                "TEA kernel {k:?} is_tea() but has no emit arm — \
                 add it to emit_tea_call"
            ),
        }),
    }
}

/// Build the capture-clone prologue for the `StreamStream` re-wrap closure.
///
/// The `StreamStream` arm wraps the handler in `move |_x| (handler)(_x)` to
/// recover the runtime's `+Sync` bound by rebuilding the handler box as SOURCE
/// per call (see the `UiOnSubmit` doc). But the handler's own `move` captures
/// its enclosing non-`Copy` locals (the `header`/`body` `String`s of a
/// Csv-stream handler); the re-embedded box steals them from the `move |_x|`
/// wrapper's env on the first call, so the wrapper degrades to `FnOnce` and
/// `server_stream_stream`'s `Fn` bound rejects it (`E0507` after `ipe` exit
/// 0 — a SEAL break). The lowerer's capture-clone rewrite only reaches INTO
/// the handler body; this synthesized wrapper is emit-only, invisible to it.
///
/// So this returns a `let <v> = <v>.clone(); …` prologue for every free local
/// `v` the handler captures, spliced INSIDE the wrapper body: the box moves the
/// fresh shadowing clones, the wrapper keeps its originals for the next call.
/// Same shape as the `TaskSeq`/`TaskSeqSync` clone-capture prologue, applied at
/// an emit-synthesized closure. Every captured free local is `Clone`: an
/// enclosing value (`Clone` by its carrier type), a `let`-bound handler
/// promoted to `SharedLambda` (`Arc`, `Clone` — `StreamStream` is in
/// `requires_sync_capture`), or a `Copy` leaf (whose `.clone()` is a bitwise
/// copy).
fn stream_handler_capture_prologue(ctx: &EmitCtx, handler: &Expr) -> DResult<String> {
    let mut prologue = String::new();
    for sym in free_vars(handler) {
        let id = ctx.emit_ident(sym)?;
        write!(prologue, "let {id} = {id}.clone(); ").map_err(|_| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::stream_handler_capture_prologue",
            detail: "writing stream-handler capture-clone prologue failed".to_owned(),
        })?;
    }
    Ok(prologue)
}

/// Handle a `Ipe.Http.Server` / `Middleware` / `RateLimit` kernel call.
///
/// Returns `Ok(None)` for all wired server kernels (they all use the standard
/// N-arg call path — no boxing or special argument transformation needed).
/// Returns a hard [`Diagnostic::CompilerBug`] for any `is_server()` variant
/// not listed here, so a future addition that forgets this function fails at
/// compile time.
fn emit_server_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let Callee::Kernel(k) = callee else {
        return Ok(None);
    };
    if !k.is_server() {
        return Ok(None);
    }
    match k {
        // ── Request accessor kernels that take `ServerRequest` by value ───────
        //
        // `ServerRequest: Clone` (see `src/runtime/rust/src/server.rs`).
        // When a handler calls more than one accessor on the same `req` binding,
        // the first call would move `req` and subsequent calls would fail with
        // E0382 "use of moved value". Emitting `req.clone()` for the request
        // argument keeps the binding alive across all reads — identical pattern
        // to `DictGet` (see the DictGet arm below `emit_server_call`).
        //
        // Server.body   : Request -> String   — req is the only arg (index 0)
        // Server.path   : Request -> String   — req is the only arg (index 0)
        // Server.method : Request -> String   — req is the only arg (index 0)
        KernelFn::ServerBody | KernelFn::ServerPath | KernelFn::ServerMethod => {
            let [req_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_server_call",
                    detail: format!("{k:?} requires exactly 1 argument, got {}", args.len()),
                });
            };
            let fn_name = kernel_name(*k);
            let req_s = emit_expr_at(ctx, req_e, indent, child, generics)?;
            Ok(Some(format!("{fn_name}({req_s}.clone())")))
        }

        // Server.param      : String -> Request -> Maybe String — req is arg 1
        // Server.queryParam : String -> Request -> Maybe String — req is arg 1
        // Server.header     : String -> Request -> Maybe String — req is arg 1
        // Server.getCookie  : String -> Request -> Maybe String — req is arg 1
        KernelFn::ServerParam
        | KernelFn::ServerQueryParam
        | KernelFn::ServerHeader
        | KernelFn::ServerGetCookie => {
            let [name_e, req_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_server_call",
                    detail: format!("{k:?} requires exactly 2 arguments, got {}", args.len()),
                });
            };
            let fn_name = kernel_name(*k);
            let name_s = emit_expr_at(ctx, name_e, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req_e, indent, child, generics)?;
            Ok(Some(format!("{fn_name}({name_s}, {req_s}.clone())")))
        }

        // `Ipe.Http.Server.Stream.stream : String -> (StreamWriter -> Task Error ()) -> Task Error Response`
        //
        // `server_stream_stream`'s bound is
        // `H: Fn(StreamWriter) -> IpeTask<E, ()> + Send + Sync + 'static`,
        // and unlike `sub_subscribe_stream` (relaxed to Send-only — see that
        // fn's doc comment in `http_stream.rs`), THIS `+Sync` bound is
        // genuinely required: `server_stream_stream` internally does
        // `Arc::new(move |w| { let task = handler(w); .. })` and stores that
        // `Arc` in a process-global `pending_handlers()` registry, popped and
        // driven later by whichever axum worker thread services the
        // eventual request (`server_stream.rs`'s `serve_streaming_sentinel`).
        // Unsizing `Arc<ConcreteClosure>` to the registry's
        // `Arc<dyn Fn(..) -> .. + Send + Sync>` slot requires the captured
        // `handler: H` to itself be `Sync` — the same "value must legitimately
        // live behind a shared `Arc`" shape as `html_on_raw_`'s `Event::OnForm`
        // slot, not the "moved into exactly one spawned task" shape of
        // `sub_subscribe_stream` / `sub_subscribe_topic`. But this kernel
        // reaches codegen through the SAME shared generic N-arg call-emit
        // fallback that passes the codegen's `Box<dyn Fn(..) -> .. + Send +
        // 'static>` value straight through as `H` — a trait object's
        // auto-trait set is exactly its bound list, so that box can never
        // satisfy `+Sync` regardless of what the boxed closure captures.
        // Fix: apply the SAME re-wrap technique used for
        // `html_on_raw_`/`ui_on_submit_` — re-embed the box construction as
        // SOURCE inside a freshly-declared closure built anew at the call
        // site, so the wrapper's own Send+Sync-ness depends only on the Ipê
        // closure's legitimate `move` captures, never the erased trait-object
        // type.
        KernelFn::StreamStream => {
            let [ct_e, f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_server_call::StreamStream",
                    detail: format!("Stream.stream requires exactly 2 arguments, got {}", args.len()),
                });
            };
            let fn_name = kernel_name(*k);
            let ct_s = emit_expr_at(ctx, ct_e, indent, child, generics)?;
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            let prologue = stream_handler_capture_prologue(ctx, f_e)?;
            Ok(Some(format!(
                "{fn_name}({ct_s}, move |_x| {{ {prologue}({f_s})(_x) }})"
            )))
        }

        // All remaining server kernels use the standard N-arg call path — no
        // special boxing or argument projection is needed.
        KernelFn::ServerGet
        | KernelFn::ServerPost
        | KernelFn::ServerPut
        | KernelFn::ServerDelete
        | KernelFn::ServerAny
        | KernelFn::ServerApi
        | KernelFn::ServerStatic
        | KernelFn::ServerListen
        | KernelFn::ServerText
        | KernelFn::ServerJson
        | KernelFn::ServerHtml
        | KernelFn::ServerWithStatus
        | KernelFn::ServerWithHeader
        | KernelFn::ServerRedirect
        | KernelFn::ServerCookieNew
        | KernelFn::ServerWithCookie
        | KernelFn::MiddlewareWithCors
        | KernelFn::MiddlewareWithLogging
        | KernelFn::MiddlewareWithBasicAuth
        | KernelFn::MiddlewareWithRateLimit
        | KernelFn::MiddlewareWithCsrf
        | KernelFn::RateLimitAllow
        // ── Ipe.Http.Server.Stream (server-side streaming) ─────────────
        | KernelFn::StreamEmit
        | KernelFn::StreamFinish
        | KernelFn::StreamWithContentType
        // ── Ipe.Http.Stream (client-side relay) ───────────────────
        | KernelFn::HttpStreamOpen
        | KernelFn::HttpStreamForEachChunk
        | KernelFn::HttpStreamClose
        // ── Ipe.Http.Server.WebSocket (12 kernels) ─────────────────────
        | KernelFn::WsDefaultCfg
        | KernelFn::WsWithOnConnect
        | KernelFn::WsWithOnMessage
        | KernelFn::WsWithOnClose
        | KernelFn::WsWithOnError
        | KernelFn::WsWithMaxMessageBytes
        | KernelFn::WsWithOriginPatterns
        | KernelFn::WsUpgrade
        | KernelFn::WsSendToClient
        | KernelFn::WsSendBinaryToClient
        | KernelFn::WsBroadcast
        | KernelFn::WsCloseClient => Ok(None),
        // Any is_server() variant not listed above is a gap — hard error so
        // the Rust compiler's exhaustiveness check catches it at compile time.
        _ => Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_server_call",
            detail: format!(
                "server kernel {k:?} is_server() but has no emit arm — \
                 add it to emit_server_call"
            ),
        }),
    }
}

/// Find a record field by its Ipê source name in an IR field list.
///
/// Searches `fields` linearly for the entry whose interned symbol resolves to
/// `name`.  Returns a reference to the field's value expression on success.
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] when no field with the requested name
/// is present in the list.  Fail-closed — never silently drops a missing
/// required field (MAKE INVALID STATES UNREPRESENTABLE principle).
/// Render a UI kernel call whose inline cfg-record fields map to POSITIONAL
/// runtime arguments: every argument is hoisted into a block local in IR walk
/// order — `leading` args first, then the cfg fields in their STORED
/// (name-sorted) record order, then `trailing` args — and the call composes
/// the locals in the runtime's positional order (`leading…`, then
/// `positional_fields` by name, then `trailing…`).
///
/// The hoist is load-bearing: the multi-use-clone rewrite walks the record in
/// stored order and leaves the walk-order-LAST use of a value as a bare move.
/// Rendering the fields positionally without the hoist reorders evaluation,
/// so that bare move could run BEFORE an earlier use's `.clone()` (E0382 on
/// `Ui.button`'s `{ onPress, label }`, whose stored order is `label` first
/// but whose positional order passes `onPress` first).
#[allow(clippy::too_many_arguments)] // one hoist site per cfg-record kernel; the args mirror emit_expr_at's
fn emit_cfg_record_call(
    ctx: &EmitCtx,
    leading: &[&Expr],
    fields: &[(Symbol, Expr)],
    trailing: &[&Expr],
    positional_fields: &[&str],
    callee: &str,
    where_: &'static str,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<String> {
    use std::fmt::Write as _;
    let mut hoist = String::new();
    for (i, e) in leading.iter().enumerate() {
        let rendered = emit_expr_at(ctx, e, indent, child, generics)?;
        // Writing into a String is infallible.
        let _ = write!(hoist, "let __ui_lead{i} = {rendered}; ");
    }
    let mut local_of: Vec<(String, String)> = Vec::with_capacity(fields.len());
    for (i, (sym, fe)) in fields.iter().enumerate() {
        let fname = ctx.resolve_ident(*sym)?;
        let rendered = emit_expr_at(ctx, fe, indent, child, generics)?;
        let local = format!("__ui_f{i}");
        let _ = write!(hoist, "let {local} = {rendered}; ");
        local_of.push((fname.to_owned(), local));
    }
    for (i, e) in trailing.iter().enumerate() {
        let rendered = emit_expr_at(ctx, e, indent, child, generics)?;
        let _ = write!(hoist, "let __ui_trail{i} = {rendered}; ");
    }
    let mut call_args: Vec<String> = (0..leading.len())
        .map(|i| format!("__ui_lead{i}"))
        .collect();
    for name in positional_fields {
        let local = local_of
            .iter()
            .find(|(f, _)| f == name)
            .map(|(_, l)| l.clone())
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_,
                detail: format!("cfg record is missing required field `{name}`"),
            })?;
        call_args.push(local);
    }
    call_args.extend((0..trailing.len()).map(|i| format!("__ui_trail{i}")));
    Ok(format!("{{ {hoist}{callee}({}) }}", call_args.join(", ")))
}

fn lookup_field<'f>(
    ctx: &EmitCtx,
    fields: &'f [(Symbol, Expr)],
    name: &str,
    where_: &'static str,
) -> DResult<&'f Expr> {
    for (sym, expr) in fields {
        if ctx.resolve_ident(*sym)? == name {
            return Ok(expr);
        }
    }
    Err(Diagnostic::CompilerBug {
        where_,
        detail: format!(
            "required field `{name}` not found in Ui.layoutWith cfg record literal; \
             available fields: [{}]",
            fields
                .iter()
                .filter_map(|(s, _)| ctx.resolve_ident(*s).ok())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

/// Arc-wrap an already-emitted callback expression so it fills a runtime
/// `Arc<dyn Fn(_) -> _ + Send + Sync>` slot.
///
/// The `Ipe.Ui.Input.*` runtime functions (`input_text_`, `input_slider_`,
/// `input_checkbox_`, `input_radio_`, …) take their callback fields (`onChange`,
/// checkbox `icon`) as `Arc<dyn Fn(_) -> _ + Send + Sync + 'static>` — the same
/// shared-callback shape every Ipe.Ui / Ipe.Web event slot uses. But an
/// `onChange` field expression lowers as an ordinary value: a bare
/// `Msg`-constructor eta-expands to a plain lambda, and both [`emit_lambda`] and
/// [`emit_func_value`] pin `Box::new(..)` for every non-Server/WS `Fun` shape
/// (see [`wants_arc_ctor`]). Passing that `Box<dyn Fn(_) -> _ + Send>` into the
/// `Arc<.. + Send + Sync>` parameter is an E0308 — a SEAL break.
///
/// Rather than special-casing every callback-carrying shape at the box site,
/// this mirrors the existing `ui_on_input_` / `ui_on_change_` arms (this same
/// file): eta-wrap the emitted callback in a fresh `Arc`-owned closure
/// `::std::sync::Arc::new(move |_x| (f)(_x))`. Rust infers `_x`, so ONE wrap
/// serves every arity-1 callback regardless of arg type (`String` or `bool`) or
/// return type (`Msg` or `Element<Msg>`). The wrap is sound: an emitted Ipê
/// callback is always `'static` (it captures no borrow-lifetime context), so the
/// `move` capture yields a `Send + Sync` `Arc`. This is the reference's uniform
/// Arc-callback policy applied at the call-argument boundary.
fn arc_callback_wrap(f_s: &str) -> String {
    format!("::std::sync::Arc::new(move |_x| ({f_s})(_x))")
}

/// Emit a `Ipe.Ui.Input.*` callback field, Arc-wrapping it for the runtime's
/// `Arc<dyn Fn(_) -> _ + Send + Sync>` slot (see [`arc_callback_wrap`]) while
/// HOISTING any leading capture-clone `let`s OUTSIDE the `Arc`'s `move` closure.
///
/// the lowerer's multi-use-capture rewrite
/// ([`rewrite_multiuse_clones`]) wraps a callback lambda that captures a
/// non-`Copy` binding used again by a sibling in a pre-clone
/// `let sym = sym.clone() in Lambda { … }`. Emitted naively, that whole block
/// is the string `arc_callback_wrap` wraps, giving
/// `Arc::new(move |_x| (({ let habit = habit.clone(); … }))(_x))`. The `.clone()`
/// reads the FREE outer `habit`, but the enclosing `move |_x|` still
/// move-captures that same outer `habit` — so a later sibling use
/// (`StateMsg::RemoveHabit((habit).id)`) hits use-after-move (E0382). The
/// hoist was already correct for a plain (un-Arc-wrapped) callback arg — the
/// pre-clone `let` sat in the enclosing scope, and only the INNER `move`
/// captured the clone; the `Arc` re-wrap is what re-introduced the outer
/// `move`.
///
/// Fix: peel the leading pure-alias `let`s (`let n = <Var/CloneVar>`; each a
/// value-preserving re-bind) off the callback expression and emit them as a
/// prefix OUTSIDE the `Arc::new`, so the Arc closure owns the pre-made clone
/// and the original binding survives for later sibling uses:
///
/// ```text
/// { let habit = habit.clone(); ::std::sync::Arc::new(move |_x| ((INNER))(_x)) }
/// ```
///
/// Only a `let` whose value is a bare `Var`/`CloneVar` is peeled — a pure
/// alias/clone of an outer symbol whose hoist out of the `move` closure is
/// always semantics-preserving (Ipê values are immutable). A `let` binding a
/// COMPUTED value stays inside, untouched, so no re-ordering of effects or
/// widening of a capture's scope can occur. When there are no such leading
/// `let`s the output is byte-identical to the previous
/// `arc_callback_wrap(&emit_expr_at(..))`.
fn emit_arc_callback_field(
    ctx: &EmitCtx,
    field: &Expr,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<String> {
    // Peel leading pure-alias `let`s (`let n = Var(v)` / `let n = CloneVar(v)`).
    let mut hoisted: Vec<(Symbol, &Expr)> = Vec::new();
    let mut inner = field;
    while let Expr::Let { name, value, body } = inner {
        if matches!(value.as_ref(), Expr::Var(_) | Expr::CloneVar(_)) {
            hoisted.push((*name, value.as_ref()));
            inner = body.as_ref();
        } else {
            break;
        }
    }
    // An inline lambda literal goes STRAIGHT into the `Arc` — one closure
    // boundary. The generic wrap-and-redispatch below builds a fresh boxed
    // closure per call of the `Arc` closure, so a callee-position capture of
    // a `Box<dyn Fn>` param would be moved out of the `Fn` env per call
    // (E0507); the direct form moves the capture ONCE at `Arc` construction
    // and every call merely borrows it.
    let arc = if let Expr::Lambda { params, ret, body } | Expr::SharedLambda { params, ret, body } =
        inner
    {
        let closure = emit_lambda_unboxed(ctx, params, ret, body, indent, child, generics)?;
        format!("::std::sync::Arc::new({closure})")
    } else {
        let inner_s = emit_expr_at(ctx, inner, indent, child, generics)?;
        arc_callback_wrap(&inner_s)
    };
    if hoisted.is_empty() {
        return Ok(arc);
    }
    let mut prefix = String::new();
    for (name, value) in hoisted {
        let name_s = ctx.emit_ident(name)?;
        let value_s = emit_expr_at(ctx, value, indent, child, generics)?;
        write!(prefix, "let {name_s} = {value_s}; ").map_err(|e| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_arc_callback_field",
            detail: format!("fmt::Write into String failed: {e}"),
        })?;
    }
    Ok(format!("{{ {prefix}{arc} }}"))
}

/// Handle `Ipe.Ui` / `Ipe.Html` kernel calls.
///
/// The render kernels (`UiLayout`, `UiLayoutWith`, `HtmlRender`,
/// `HtmlEscapeText`, `HtmlEscapeAttr`, `HtmlAttrToString`) emit calls to
/// `ipe_runtime::ui::render::*` and `ipe_runtime::html::*` here. The app-entry
/// kernels (`WebApp`, `WebAppRouted`, `WebRoute`, `WebRenderStatic`,
/// `TerminalAppScreen`, `TerminalAppLines`, `WebViewApp`) delegate to their
/// respective `emit_web` / `emit_tui` / `emit_console` / `emit_webview` emitters.
///
/// Returns `None` for any kernel that is not a `Ui` / `Web` / `Terminal` /
/// `WebView` variant, letting the standard call path handle it.
#[allow(clippy::too_many_lines)] // declarative UI kernel dispatch — must list every variant explicitly
#[allow(clippy::many_single_char_names)] // r/g/b/a/k are conventional names for colour channels and kernel var
#[inline(never)]
fn emit_ui_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    on_form: ipe_ir::OnFormKind,
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let Callee::Kernel(k) = callee else {
        return Ok(None);
    };
    // Only handle Ui / Live / Tui / Webview / Cli kernels.
    if !k.is_ui() && !k.is_web() && !k.is_tui() && !k.is_webview() && !k.is_console() {
        return Ok(None);
    }
    match k {
        // ── Ipe.Ui / Ipe.Html render kernels ─────────────────────────────────

        // `Ui.layout : List (Attribute msg) -> Element msg -> Html msg`
        //
        // Emits: `ipe_runtime::ui::render::ui_layout(attrs, elem)`
        KernelFn::UiLayout => {
            let [attrs_e, elem_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiLayout",
                    detail: format!("Ui.layout requires exactly 2 arguments, got {}", args.len()),
                });
            };
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let elem_s = emit_expr_at(ctx, elem_e, indent, child, generics)?;
            // M is inferred bottom-up from the concrete element /
            // attrs types that the region-type–sourced emit propagates.  No
            // turbofish required; Rust unifies M from the element argument or from
            // the enclosing function's return type annotation — both supply a
            // concrete `Msg` type.  The old `enclosing_ui_msg` mechanism is gone.
            Ok(Some(format!(
                "ipe_runtime::ui::render::ui_layout({attrs_s}, {elem_s})"
            )))
        }

        // `Ui.layoutWith : { wrapperAttrs : ..., rootAttrs : ... } -> Element msg -> Html msg`
        //
        // Emits: `ipe_runtime::ui::render::ui_layout_with_vecs::<M>(wrapper, root, elem)`
        //
        // DESIGN: the runtime's generic `ui_layout_with<M, C>` stub was the
        // silent-drop path (`_cfg` ignored, falls back to `ui_layout(vec![], …)`).
        // That path is deleted (MAKE INVALID STATES UNREPRESENTABLE).
        //
        // We delegate at the emit site instead: extract `wrapperAttrs` and
        // `rootAttrs` directly from the IR record literal and pass them as
        // `Vec<Attribute<M>>` to `ui_layout_with_vecs`, bypassing the unsynthesised
        // record struct that would trigger IPE-I0001 if materialised.
        //
        // Non-literal cfg (e.g. `let cfg = { … } in Ui.layoutWith cfg elem`) is
        // rejected fail-closed with `CompilerBug` (unsupported).
        KernelFn::UiLayoutWith => {
            let [cfg_e, elem_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiLayoutWith",
                    detail: format!(
                        "Ui.layoutWith requires exactly 2 arguments, got {}",
                        args.len()
                    ),
                });
            };
            // Extract fields from the IR literal rather than materialising a
            // synthesised Rust struct (which would ICE with IPE-I0001 because
            // no struct for the {wrapperAttrs, rootAttrs} shape is registered).
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiLayoutWith",
                    detail: "Ui.layoutWith cfg must be an inline record literal \
                             in Phase 0; non-literal cfg is deferred to Phase 1"
                        .into(),
                });
            };
            // Same bottom-up M inference as UiLayout — no turbofish.
            Ok(Some(emit_cfg_record_call(
                ctx,
                &[],
                fields,
                &[elem_e],
                &["wrapperAttrs", "rootAttrs"],
                "ipe_runtime::ui::render::ui_layout_with_vecs",
                "ipe_backend_rust::emit_ui_call::UiLayoutWith",
                indent,
                child,
                generics,
            )?))
        }

        // `Html.render : Html msg -> String`
        //
        // Emits: `ipe_runtime::html::render_html(&html)`
        KernelFn::HtmlRender => {
            let [html_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlRender",
                    detail: format!(
                        "Html.render requires exactly 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let html_s = emit_expr_at(ctx, html_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::html::render_html(&{html_s})")))
        }

        // `Html.toString : Html msg -> String` — alias of `Html.render`.
        KernelFn::HtmlToString => {
            let [html_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlToString",
                    detail: format!(
                        "Html.toString requires exactly 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let html_s = emit_expr_at(ctx, html_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::html::render_html(&{html_s})")))
        }

        // `Html.escapeText : String -> String`
        //
        // Emits: `ipe_runtime::html::html_escape_text_(s)` (takes owned String).
        KernelFn::HtmlEscapeText => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlEscapeText",
                    detail: format!(
                        "Html.escapeText requires exactly 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let s_s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::html::html_escape_text_({s_s})")))
        }

        // `Html.escapeAttr : String -> String`
        //
        // Emits: `ipe_runtime::html::html_escape_attr_(s)` (takes owned String).
        KernelFn::HtmlEscapeAttr => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlEscapeAttr",
                    detail: format!(
                        "Html.escapeAttr requires exactly 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let s_s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::html::html_escape_attr_({s_s})")))
        }

        // `Html.attrToString : Html.Attribute msg -> String`
        //
        // Emits: `ipe_runtime::html::html_attr_to_string_(attr)` (takes owned Attribute<M>).
        KernelFn::HtmlAttrToString => {
            let [attr_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlAttrToString",
                    detail: format!(
                        "Html.attrToString requires exactly 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let attr_s = emit_expr_at(ctx, attr_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::html::html_attr_to_string_({attr_s})"
            )))
        }

        // ── Ipe.Ui element builders ───────────────────────────────────────────

        // `Ui.none : Element msg`
        KernelFn::UiNone => Ok(Some("ipe_runtime::ui::helpers::ui_none_()".to_owned())),

        // `Ui.text : String -> Element msg`
        KernelFn::UiText => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiText",
                    detail: format!("Ui.text requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_text_({s})")))
        }

        // `Ui.html : Html msg -> Element msg`
        KernelFn::UiHtml => {
            let [h_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiHtml",
                    detail: format!("Ui.html requires 1 argument, got {}", args.len()),
                });
            };
            let h = emit_expr_at(ctx, h_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_html_({h})")))
        }

        // `Ui.cells : List (List Char) -> Element msg` — raw terminal cell grid,
        // painted as an island inside an `Ipe.Ui` view under `Terminal.appScreen`.
        KernelFn::UiCells => {
            // seal (SECURITY, fail-closed): `Ui.cells` paints raw terminal cells
            // and has no browser denotation. In a Web/WebView build its runtime
            // helper degrades to plain text, so it would ipe-succeed and silently
            // render wrong. Reject it here — the one point it is emitted — with a
            // shape-keyed IPE-L0132, converting a wrong-render into an ipe error.
            if ctx.uses_web || ctx.uses_webview {
                let app = if ctx.uses_webview {
                    ipe_diagnostics::AppShape::WebView
                } else {
                    ipe_diagnostics::AppShape::Web
                };
                return Err(Diagnostic::Lower {
                    span: Span::DUMMY,
                    msg: LowerError::UiCellsInWebShape(app),
                });
            }
            let [grid_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiCells",
                    detail: format!("Ui.cells requires 1 argument, got {}", args.len()),
                });
            };
            let grid = emit_expr_at(ctx, grid_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_cells_({grid})")))
        }

        // `Ui.el : List (Attribute msg) -> Element msg -> Element msg`
        KernelFn::UiEl => {
            let [attrs_e, child_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiEl",
                    detail: format!("Ui.el requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let ch = emit_expr_at(ctx, child_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_el_({attrs}, {ch})"
            )))
        }

        // `Ui.row : List (Attribute msg) -> List (Element msg) -> Element msg`
        KernelFn::UiRow => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiRow",
                    detail: format!("Ui.row requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_row_({attrs}, {children})"
            )))
        }

        // `Ui.column : List (Attribute msg) -> List (Element msg) -> Element msg`
        KernelFn::UiColumn => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiColumn",
                    detail: format!("Ui.column requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_column_({attrs}, {children})"
            )))
        }

        // `Ui.wrappedRow : List (Attribute msg) -> List (Element msg) -> Element msg`
        KernelFn::UiWrappedRow => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiWrappedRow",
                    detail: format!("Ui.wrappedRow requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_wrapped_row_({attrs}, {children})"
            )))
        }

        // `Ui.grid : List (Attribute msg) -> List (Element msg) -> Element msg`
        KernelFn::UiGrid => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiGrid",
                    detail: format!("Ui.grid requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_grid_({attrs}, {children})"
            )))
        }

        // `Ui.paragraph : List (Attribute msg) -> List (Element msg) -> Element msg`
        KernelFn::UiParagraph => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiParagraph",
                    detail: format!("Ui.paragraph requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_paragraph_({attrs}, {children})"
            )))
        }

        // `Ui.textColumn : List (Attribute msg) -> List (Element msg) -> Element msg`
        KernelFn::UiTextColumn => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiTextColumn",
                    detail: format!("Ui.textColumn requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_text_column_({attrs}, {children})"
            )))
        }

        // `Ui.form : List (Attribute msg) -> List (Element msg) -> Element msg`
        KernelFn::UiForm => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiForm",
                    detail: format!("Ui.form requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_form_({attrs}, {children})"
            )))
        }

        // `Ui.above : Element msg -> Attribute msg`
        KernelFn::UiAbove => {
            let [elem_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiAbove",
                    detail: format!("Ui.above requires 1 argument, got {}", args.len()),
                });
            };
            let elem = emit_expr_at(ctx, elem_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_above_({elem})")))
        }

        // `Ui.below : Element msg -> Attribute msg`
        KernelFn::UiBelow => {
            let [elem_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiBelow",
                    detail: format!("Ui.below requires 1 argument, got {}", args.len()),
                });
            };
            let elem = emit_expr_at(ctx, elem_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_below_({elem})")))
        }

        // `Ui.onLeft : Element msg -> Attribute msg`
        KernelFn::UiOnLeft => {
            let [elem_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnLeft",
                    detail: format!("Ui.onLeft requires 1 argument, got {}", args.len()),
                });
            };
            let elem = emit_expr_at(ctx, elem_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_left_({elem})"
            )))
        }

        // `Ui.onRight : Element msg -> Attribute msg`
        KernelFn::UiOnRight => {
            let [elem_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnRight",
                    detail: format!("Ui.onRight requires 1 argument, got {}", args.len()),
                });
            };
            let elem = emit_expr_at(ctx, elem_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_right_({elem})"
            )))
        }

        // `Ui.inFront : Element msg -> Attribute msg`
        KernelFn::UiInFront => {
            let [elem_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiInFront",
                    detail: format!("Ui.inFront requires 1 argument, got {}", args.len()),
                });
            };
            let elem = emit_expr_at(ctx, elem_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_in_front_({elem})"
            )))
        }

        // `Ui.behind : Element msg -> Attribute msg`
        KernelFn::UiBehind => {
            let [elem_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiBehind",
                    detail: format!("Ui.behind requires 1 argument, got {}", args.len()),
                });
            };
            let elem = emit_expr_at(ctx, elem_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_behind_({elem})"
            )))
        }

        // `Ui.button : List (Attribute msg) -> { onPress : Maybe msg, label : Element msg } -> Element msg`
        //
        // Emits: `ipe_runtime::ui::helpers::ui_button_(attrs, on_press, label)`
        KernelFn::UiButton => {
            let [attrs_e, cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiButton",
                    detail: format!("Ui.button requires 2 arguments, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiButton",
                    detail: "Ui.button cfg must be an inline record literal \
                             in Phase 0; non-literal cfg is deferred to Phase 1"
                        .into(),
                });
            };
            Ok(Some(emit_cfg_record_call(
                ctx,
                &[attrs_e],
                fields,
                &[],
                &["onPress", "label"],
                "ipe_runtime::ui::helpers::ui_button_",
                "ipe_backend_rust::emit_ui_call::UiButton",
                indent,
                child,
                generics,
            )?))
        }

        // `Ui.link : List (Attribute msg) -> { url : String, label : Element msg } -> Element msg`
        KernelFn::UiLink => {
            let [attrs_e, cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiLink",
                    detail: format!("Ui.link requires 2 arguments, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiLink",
                    detail: "Ui.link cfg must be an inline record literal \
                             in Phase 0; non-literal cfg is deferred to Phase 1"
                        .into(),
                });
            };
            Ok(Some(emit_cfg_record_call(
                ctx,
                &[attrs_e],
                fields,
                &[],
                &["url", "label"],
                "ipe_runtime::ui::helpers::ui_link_",
                "ipe_backend_rust::emit_ui_call::UiLink",
                indent,
                child,
                generics,
            )?))
        }

        // `Ui.image : List (Attribute msg) -> { src : String, description : String } -> Element msg`
        KernelFn::UiImage => {
            let [attrs_e, cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiImage",
                    detail: format!("Ui.image requires 2 arguments, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiImage",
                    detail: "Ui.image cfg must be an inline record literal \
                             in Phase 0; non-literal cfg is deferred to Phase 1"
                        .into(),
                });
            };
            Ok(Some(emit_cfg_record_call(
                ctx,
                &[attrs_e],
                fields,
                &[],
                &["src", "description"],
                "ipe_runtime::ui::helpers::ui_image_",
                "ipe_backend_rust::emit_ui_call::UiImage",
                indent,
                child,
                generics,
            )?))
        }

        // ── Ipe.Ui attribute builders ─────────────────────────────────────────

        // `Ui.spacing : Int -> Attribute msg`
        KernelFn::UiSpacing => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiSpacing",
                    detail: format!("Ui.spacing requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_spacing_({n})")))
        }

        // `Ui.padding : Int -> Attribute msg`
        KernelFn::UiPadding => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiPadding",
                    detail: format!("Ui.padding requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_padding_({n})")))
        }

        // `Ui.paddingXY : Int -> Int -> Attribute msg`
        KernelFn::UiPaddingXY => {
            let [x_e, y_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiPaddingXY",
                    detail: format!("Ui.paddingXY requires 2 arguments, got {}", args.len()),
                });
            };
            let x = emit_expr_at(ctx, x_e, indent, child, generics)?;
            let y = emit_expr_at(ctx, y_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_padding_xy_({x}, {y})"
            )))
        }

        // `Ui.paddingEach : { top : Int, right : Int, bottom : Int, left : Int } -> Attribute msg`
        KernelFn::UiPaddingEach => {
            let [rec_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiPaddingEach",
                    detail: format!("Ui.paddingEach requires 1 argument, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = rec_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiPaddingEach",
                    detail: "Ui.paddingEach arg must be an inline record literal".into(),
                });
            };
            let top_e = lookup_field(
                ctx,
                fields,
                "top",
                "ipe_backend_rust::emit_ui_call::UiPaddingEach::top",
            )?;
            let right_e = lookup_field(
                ctx,
                fields,
                "right",
                "ipe_backend_rust::emit_ui_call::UiPaddingEach::right",
            )?;
            let bottom_e = lookup_field(
                ctx,
                fields,
                "bottom",
                "ipe_backend_rust::emit_ui_call::UiPaddingEach::bottom",
            )?;
            let left_e = lookup_field(
                ctx,
                fields,
                "left",
                "ipe_backend_rust::emit_ui_call::UiPaddingEach::left",
            )?;
            let top = emit_expr_at(ctx, top_e, indent, child, generics)?;
            let right = emit_expr_at(ctx, right_e, indent, child, generics)?;
            let bottom = emit_expr_at(ctx, bottom_e, indent, child, generics)?;
            let left = emit_expr_at(ctx, left_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_padding_each_({top}, {right}, {bottom}, {left})"
            )))
        }

        // `Ui.width : Length -> Attribute msg`
        KernelFn::UiWidth => {
            let [l_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiWidth",
                    detail: format!("Ui.width requires 1 argument, got {}", args.len()),
                });
            };
            let l = emit_expr_at(ctx, l_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_width_({l})")))
        }

        // `Ui.height : Length -> Attribute msg`
        KernelFn::UiHeight => {
            let [l_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiHeight",
                    detail: format!("Ui.height requires 1 argument, got {}", args.len()),
                });
            };
            let l = emit_expr_at(ctx, l_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_height_({l})")))
        }

        // `Ui.centerX : Attribute msg` (arity 0)
        KernelFn::UiCenterX => Ok(Some("ipe_runtime::ui::helpers::ui_center_x_()".to_owned())),
        // `Ui.centerY : Attribute msg` (arity 0)
        KernelFn::UiCenterY => Ok(Some("ipe_runtime::ui::helpers::ui_center_y_()".to_owned())),
        // `Ui.alignLeft : Attribute msg` (arity 0)
        KernelFn::UiAlignLeft => Ok(Some(
            "ipe_runtime::ui::helpers::ui_align_left_()".to_owned(),
        )),
        // `Ui.alignRight : Attribute msg` (arity 0)
        KernelFn::UiAlignRight => Ok(Some(
            "ipe_runtime::ui::helpers::ui_align_right_()".to_owned(),
        )),
        // `Ui.alignTop : Attribute msg` (arity 0)
        KernelFn::UiAlignTop => Ok(Some("ipe_runtime::ui::helpers::ui_align_top_()".to_owned())),
        // `Ui.alignBottom : Attribute msg` (arity 0)
        KernelFn::UiAlignBottom => Ok(Some(
            "ipe_runtime::ui::helpers::ui_align_bottom_()".to_owned(),
        )),
        // `Ui.pointer : Attribute msg` (arity 0)
        KernelFn::UiPointer => Ok(Some("ipe_runtime::ui::helpers::ui_pointer_()".to_owned())),
        // `Ui.clip : Attribute msg` (arity 0)
        KernelFn::UiClip => Ok(Some("ipe_runtime::ui::helpers::ui_clip_()".to_owned())),
        // `Ui.clipX : Attribute msg` (arity 0)
        KernelFn::UiClipX => Ok(Some("ipe_runtime::ui::helpers::ui_clip_x_()".to_owned())),
        // `Ui.clipY : Attribute msg` (arity 0)
        KernelFn::UiClipY => Ok(Some("ipe_runtime::ui::helpers::ui_clip_y_()".to_owned())),
        // `Ui.scrollbars : Attribute msg` (arity 0)
        KernelFn::UiScrollbars => Ok(Some(
            "ipe_runtime::ui::helpers::ui_scrollbars_()".to_owned(),
        )),
        // `Ui.scrollbarX : Attribute msg` (arity 0)
        KernelFn::UiScrollbarX => Ok(Some(
            "ipe_runtime::ui::helpers::ui_scrollbar_x_()".to_owned(),
        )),
        // `Ui.scrollbarY : Attribute msg` (arity 0)
        KernelFn::UiScrollbarY => Ok(Some(
            "ipe_runtime::ui::helpers::ui_scrollbar_y_()".to_owned(),
        )),

        // `Ui.gridColumns : Int -> Attribute msg`
        KernelFn::UiGridColumns => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiGridColumns",
                    detail: format!("Ui.gridColumns requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_grid_columns_({n})"
            )))
        }

        // ── Ipe.Ui Length builders ────────────────────────────────────────────

        // `Ui.px : Int -> Length`
        KernelFn::UiPx => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiPx",
                    detail: format!("Ui.px requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_px_({n})")))
        }

        // `Ui.fill : Length` (arity 0)
        KernelFn::UiFill => Ok(Some("ipe_runtime::ui::helpers::ui_fill_()".to_owned())),
        // `Ui.content : Length` (arity 0)
        KernelFn::UiContent => Ok(Some("ipe_runtime::ui::helpers::ui_content_()".to_owned())),
        // `Ui.shrink : Length` (arity 0)
        KernelFn::UiShrink => Ok(Some("ipe_runtime::ui::helpers::ui_shrink_()".to_owned())),

        // `Ui.fillPortion : Int -> Length`
        KernelFn::UiFillPortion => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiFillPortion",
                    detail: format!("Ui.fillPortion requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_fill_portion_({n})"
            )))
        }

        // `Ui.vh : Int -> Length`
        KernelFn::UiVh => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiVh",
                    detail: format!("Ui.vh requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_vh_({n})")))
        }

        // `Ui.vw : Int -> Length`
        KernelFn::UiVw => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiVw",
                    detail: format!("Ui.vw requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_vw_({n})")))
        }

        // `Ui.minimum : Int -> Length -> Length`
        KernelFn::UiMinimum => {
            let [n_e, l_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiMinimum",
                    detail: format!("Ui.minimum requires 2 arguments, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            let l = emit_expr_at(ctx, l_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_minimum_({n}, {l})"
            )))
        }

        // `Ui.maximum : Int -> Length -> Length`
        KernelFn::UiMaximum => {
            let [n_e, l_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiMaximum",
                    detail: format!("Ui.maximum requires 2 arguments, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            let l = emit_expr_at(ctx, l_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_maximum_({n}, {l})"
            )))
        }

        // ── Ipe.Ui Color builders ─────────────────────────────────────────────

        // `Ui.rgb : Int -> Int -> Int -> Color`
        KernelFn::UiRgb => {
            let [r_e, g_e, b_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiRgb",
                    detail: format!("Ui.rgb requires 3 arguments, got {}", args.len()),
                });
            };
            let r = emit_expr_at(ctx, r_e, indent, child, generics)?;
            let g = emit_expr_at(ctx, g_e, indent, child, generics)?;
            let b = emit_expr_at(ctx, b_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_rgb_({r}, {g}, {b})"
            )))
        }

        // `Ui.rgba : Int -> Int -> Int -> Float -> Color`
        KernelFn::UiRgba => {
            let [r_e, g_e, b_e, a_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiRgba",
                    detail: format!("Ui.rgba requires 4 arguments, got {}", args.len()),
                });
            };
            let r = emit_expr_at(ctx, r_e, indent, child, generics)?;
            let g = emit_expr_at(ctx, g_e, indent, child, generics)?;
            let b = emit_expr_at(ctx, b_e, indent, child, generics)?;
            let a = emit_expr_at(ctx, a_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_rgba_({r}, {g}, {b}, {a})"
            )))
        }

        // `Ui.white : Color` (arity 0)
        KernelFn::UiWhite => Ok(Some("ipe_runtime::ui::helpers::ui_white_()".to_owned())),
        // `Ui.black : Color` (arity 0)
        KernelFn::UiBlack => Ok(Some("ipe_runtime::ui::helpers::ui_black_()".to_owned())),
        // `Ui.transparent : Color` (arity 0)
        KernelFn::UiTransparent => Ok(Some(
            "ipe_runtime::ui::helpers::ui_transparent_()".to_owned(),
        )),
        // `Ui.colorCss : Color -> String` (arity 1)
        KernelFn::UiColorCss => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiColorCss",
                    detail: format!("Ui.colorCss requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_color_css_({c})"
            )))
        }

        // ── Background sub-module ─────────────────────────────────────────────

        // `Background.color : Color -> Attribute msg`
        KernelFn::BackgroundColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BackgroundColor",
                    detail: format!("Background.color requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_background_color_({c})"
            )))
        }

        // `Background.image : String -> Attribute msg`
        KernelFn::BackgroundImage => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BackgroundImage",
                    detail: format!("Background.image requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_background_image_({s})"
            )))
        }

        // `Background.linearGradient : Float -> List (Float, Color) -> Attribute msg`
        KernelFn::BackgroundLinearGradient => {
            let [angle_e, stops_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BackgroundLinearGradient",
                    detail: format!(
                        "Background.linearGradient requires 2 arguments, got {}",
                        args.len()
                    ),
                });
            };
            let angle = emit_expr_at(ctx, angle_e, indent, child, generics)?;
            let stops = emit_expr_at(ctx, stops_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_background_linear_gradient_({angle}, {stops})"
            )))
        }

        // ── Border sub-module ─────────────────────────────────────────────────

        // `Border.width : Int -> Attribute msg`
        KernelFn::BorderWidth => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderWidth",
                    detail: format!("Border.width requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_width_({n})"
            )))
        }

        // `Border.rounded : Int -> Attribute msg`
        KernelFn::BorderRounded => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderRounded",
                    detail: format!("Border.rounded requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_rounded_({n})"
            )))
        }

        // `Border.color : Color -> Attribute msg`
        KernelFn::BorderColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderColor",
                    detail: format!("Border.color requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_color_({c})"
            )))
        }

        // `Border.widthEach : { top : Int, right : Int, bottom : Int, left : Int } -> Attribute msg`
        KernelFn::BorderWidthEach => {
            let [rec_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderWidthEach",
                    detail: format!("Border.widthEach requires 1 argument, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = rec_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderWidthEach",
                    detail: "Border.widthEach arg must be an inline record literal".into(),
                });
            };
            let top_e = lookup_field(
                ctx,
                fields,
                "top",
                "ipe_backend_rust::emit_ui_call::BorderWidthEach::top",
            )?;
            let right_e = lookup_field(
                ctx,
                fields,
                "right",
                "ipe_backend_rust::emit_ui_call::BorderWidthEach::right",
            )?;
            let bottom_e = lookup_field(
                ctx,
                fields,
                "bottom",
                "ipe_backend_rust::emit_ui_call::BorderWidthEach::bottom",
            )?;
            let left_e = lookup_field(
                ctx,
                fields,
                "left",
                "ipe_backend_rust::emit_ui_call::BorderWidthEach::left",
            )?;
            let top = emit_expr_at(ctx, top_e, indent, child, generics)?;
            let right = emit_expr_at(ctx, right_e, indent, child, generics)?;
            let bottom = emit_expr_at(ctx, bottom_e, indent, child, generics)?;
            let left = emit_expr_at(ctx, left_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_width_each_({top}, {right}, {bottom}, {left})"
            )))
        }

        // `Border.shadow : { offsetX : Int, offsetY : Int, blur : Int, spread : Int, color : Color } -> Attribute msg`
        KernelFn::BorderShadow => {
            let [rec_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderShadow",
                    detail: format!("Border.shadow requires 1 argument, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = rec_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderShadow",
                    detail: "Border.shadow arg must be an inline record literal".into(),
                });
            };
            // Distinct binding names (`horiz`/`vert` rather than `offset_x`/
            // `offset_y`) keep clippy::similar_names quiet — the source record
            // fields are still `offsetX`/`offsetY`.
            let horiz_e = lookup_field(
                ctx,
                fields,
                "offsetX",
                "ipe_backend_rust::emit_ui_call::BorderShadow::offsetX",
            )?;
            let vert_e = lookup_field(
                ctx,
                fields,
                "offsetY",
                "ipe_backend_rust::emit_ui_call::BorderShadow::offsetY",
            )?;
            let blur_e = lookup_field(
                ctx,
                fields,
                "blur",
                "ipe_backend_rust::emit_ui_call::BorderShadow::blur",
            )?;
            let spread_e = lookup_field(
                ctx,
                fields,
                "spread",
                "ipe_backend_rust::emit_ui_call::BorderShadow::spread",
            )?;
            let color_e = lookup_field(
                ctx,
                fields,
                "color",
                "ipe_backend_rust::emit_ui_call::BorderShadow::color",
            )?;
            let horiz = emit_expr_at(ctx, horiz_e, indent, child, generics)?;
            let vert = emit_expr_at(ctx, vert_e, indent, child, generics)?;
            let blur = emit_expr_at(ctx, blur_e, indent, child, generics)?;
            let spread = emit_expr_at(ctx, spread_e, indent, child, generics)?;
            let color = emit_expr_at(ctx, color_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_shadow_({horiz}, {vert}, {blur}, {spread}, {color})"
            )))
        }

        // `Border.glow : Int -> Color -> Attribute msg` — convenience box-shadow
        // with 0,0 offset + 0 spread. Two positional args (blur Int + colour
        // Color); no record destructure, unlike `Border.shadow`.
        KernelFn::BorderGlow => {
            let [blur_e, color_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderGlow",
                    detail: format!("Border.glow requires 2 arguments, got {}", args.len()),
                });
            };
            let blur = emit_expr_at(ctx, blur_e, indent, child, generics)?;
            let color = emit_expr_at(ctx, color_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_glow_({blur}, {color})"
            )))
        }

        // `Border.innerShadow : { offsetX : Int, offsetY : Int, blur : Int, spread : Int, color : Color } -> Attribute msg`
        // Same record destructure as `Border.shadow`, emitting the INSET helper.
        KernelFn::BorderInnerShadow => {
            let [rec_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderInnerShadow",
                    detail: format!("Border.innerShadow requires 1 argument, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = rec_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderInnerShadow",
                    detail: "Border.innerShadow arg must be an inline record literal".into(),
                });
            };
            // Distinct binding names (`horiz`/`vert` rather than `offset_x`/
            // `offset_y`) keep clippy::similar_names quiet — the source record
            // fields are still `offsetX`/`offsetY`.
            let horiz_e = lookup_field(
                ctx,
                fields,
                "offsetX",
                "ipe_backend_rust::emit_ui_call::BorderInnerShadow::offsetX",
            )?;
            let vert_e = lookup_field(
                ctx,
                fields,
                "offsetY",
                "ipe_backend_rust::emit_ui_call::BorderInnerShadow::offsetY",
            )?;
            let blur_e = lookup_field(
                ctx,
                fields,
                "blur",
                "ipe_backend_rust::emit_ui_call::BorderInnerShadow::blur",
            )?;
            let spread_e = lookup_field(
                ctx,
                fields,
                "spread",
                "ipe_backend_rust::emit_ui_call::BorderInnerShadow::spread",
            )?;
            let color_e = lookup_field(
                ctx,
                fields,
                "color",
                "ipe_backend_rust::emit_ui_call::BorderInnerShadow::color",
            )?;
            let horiz = emit_expr_at(ctx, horiz_e, indent, child, generics)?;
            let vert = emit_expr_at(ctx, vert_e, indent, child, generics)?;
            let blur = emit_expr_at(ctx, blur_e, indent, child, generics)?;
            let spread = emit_expr_at(ctx, spread_e, indent, child, generics)?;
            let color = emit_expr_at(ctx, color_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_inner_shadow_({horiz}, {vert}, {blur}, {spread}, {color})"
            )))
        }

        // ── Font sub-module ───────────────────────────────────────────────────

        // `Font.size : Int -> Attribute msg`
        KernelFn::FontSize => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::FontSize",
                    detail: format!("Font.size requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_font_size_({n})"
            )))
        }

        // `Font.color : Color -> Attribute msg`
        KernelFn::FontColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::FontColor",
                    detail: format!("Font.color requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_font_color_({c})"
            )))
        }

        // `Font.family : String -> Attribute msg`
        KernelFn::FontFamily => {
            let [l_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::FontFamily",
                    detail: format!("Font.family requires 1 argument, got {}", args.len()),
                });
            };
            let l = emit_expr_at(ctx, l_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_font_family_({l})"
            )))
        }

        // `Font.bold : Attribute msg` (arity 0)
        KernelFn::FontBold => Ok(Some("ipe_runtime::ui::helpers::ui_font_bold_()".to_owned())),
        // `Font.italic : Attribute msg` (arity 0)
        KernelFn::FontItalic => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_italic_()".to_owned(),
        )),

        // ── extended Ipe.Ui / Font / Background / Border builders ──

        // Ui namespace — nullary aspect-ratio attrs
        KernelFn::UiSquare => Ok(Some("ipe_runtime::ui::helpers::ui_square_()".to_owned())),
        KernelFn::UiWidescreen => Ok(Some(
            "ipe_runtime::ui::helpers::ui_widescreen_()".to_owned(),
        )),
        KernelFn::UiCinemascope => Ok(Some(
            "ipe_runtime::ui::helpers::ui_cinemascope_()".to_owned(),
        )),

        // `Ui.name : String -> Attribute msg`
        KernelFn::UiName => {
            let [v_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiName",
                    detail: format!("Ui.name requires 1 argument, got {}", args.len()),
                });
            };
            let v = emit_expr_at(ctx, v_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_name_({v})")))
        }

        // `Ui.style : String -> String -> Attribute msg`
        KernelFn::UiStyle => {
            let [k_e, v_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiStyle",
                    detail: format!("Ui.style requires 2 arguments, got {}", args.len()),
                });
            };
            let k = emit_expr_at(ctx, k_e, indent, child, generics)?;
            let v = emit_expr_at(ctx, v_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_style_({k}, {v})"
            )))
        }

        // `Ui.transitionRaw : String -> Bool -> Attribute msg`
        KernelFn::UiTransitionRaw => {
            let [s_e, respect_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiTransitionRaw",
                    detail: format!("Ui.transitionRaw requires 2 arguments, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            let respect = emit_expr_at(ctx, respect_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_transition_raw_({s}, {respect})"
            )))
        }

        // `Ui.gridTracksRaw : String -> String -> Attribute msg`
        KernelFn::UiGridTracksRaw => {
            let [cols_e, rows_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiGridTracksRaw",
                    detail: format!("Ui.gridTracksRaw requires 2 arguments, got {}", args.len()),
                });
            };
            let cols = emit_expr_at(ctx, cols_e, indent, child, generics)?;
            let rows = emit_expr_at(ctx, rows_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_grid_tracks_raw_({cols}, {rows})"
            )))
        }

        // `Ui.animateRaw : String -> String -> String -> Bool -> Attribute msg`
        KernelFn::UiAnimateRaw => {
            let [name_e, shorthand_e, keyframes_e, respect_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiAnimateRaw",
                    detail: format!("Ui.animateRaw requires 4 arguments, got {}", args.len()),
                });
            };
            let name = emit_expr_at(ctx, name_e, indent, child, generics)?;
            let shorthand = emit_expr_at(ctx, shorthand_e, indent, child, generics)?;
            let keyframes = emit_expr_at(ctx, keyframes_e, indent, child, generics)?;
            let respect = emit_expr_at(ctx, respect_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_animate_raw_({name}, {shorthand}, {keyframes}, {respect})"
            )))
        }

        // `Ui.aspectRatio : Float -> Attribute msg`
        KernelFn::UiAspectRatio => {
            let [r_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiAspectRatio",
                    detail: format!("Ui.aspectRatio requires 1 argument, got {}", args.len()),
                });
            };
            let r = emit_expr_at(ctx, r_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_aspect_ratio_({r})"
            )))
        }

        // `Ui.aspectRatioWH : Int -> Int -> Attribute msg`
        KernelFn::UiAspectRatioWH => {
            let [w_e, h_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiAspectRatioWH",
                    detail: format!("Ui.aspectRatioWH requires 2 arguments, got {}", args.len()),
                });
            };
            let w = emit_expr_at(ctx, w_e, indent, child, generics)?;
            let h = emit_expr_at(ctx, h_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_aspect_ratio_wh_({w}, {h})"
            )))
        }

        // `Ui.htmlAttribute : String -> String -> Attribute msg`
        KernelFn::UiHtmlAttribute => {
            let [k_e, v_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiHtmlAttribute",
                    detail: format!("Ui.htmlAttribute requires 2 arguments, got {}", args.len()),
                });
            };
            let k = emit_expr_at(ctx, k_e, indent, child, generics)?;
            let v = emit_expr_at(ctx, v_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_html_attribute_({k}, {v})"
            )))
        }

        // Breakpoint constants — `Ui.mobile` / `Ui.tablet` / … : String (0-arity)
        KernelFn::UiMobile => Ok(Some("ipe_runtime::ui::helpers::ui_mobile_()".to_owned())),
        KernelFn::UiTablet => Ok(Some("ipe_runtime::ui::helpers::ui_tablet_()".to_owned())),
        KernelFn::UiDesktop => Ok(Some("ipe_runtime::ui::helpers::ui_desktop_()".to_owned())),
        KernelFn::UiDarkMode => Ok(Some("ipe_runtime::ui::helpers::ui_dark_mode_()".to_owned())),
        KernelFn::UiLightMode => Ok(Some(
            "ipe_runtime::ui::helpers::ui_light_mode_()".to_owned(),
        )),
        KernelFn::UiReducedMotion => Ok(Some(
            "ipe_runtime::ui::helpers::ui_reduced_motion_()".to_owned(),
        )),

        // PseudoClass constants — `Ui.hover` / `Ui.focus` / … : PseudoClass (0-arity)
        KernelFn::UiHover => Ok(Some("ipe_runtime::ui::helpers::ui_hover_()".to_owned())),
        KernelFn::UiFocus => Ok(Some("ipe_runtime::ui::helpers::ui_focus_()".to_owned())),
        KernelFn::UiFocusVisible => Ok(Some(
            "ipe_runtime::ui::helpers::ui_focus_visible_()".to_owned(),
        )),
        KernelFn::UiActive => Ok(Some("ipe_runtime::ui::helpers::ui_active_()".to_owned())),
        KernelFn::UiDisabled => Ok(Some("ipe_runtime::ui::helpers::ui_disabled_()".to_owned())),

        // `Ui.onPseudo : PseudoClass -> List (Attribute msg) -> Attribute msg`
        // — generic escape hatch: folds `attrs` into one CSS rules-string and
        // attaches it as `AttrPseudoRule(pc, css)`.
        KernelFn::UiOnPseudo => {
            let [pc_e, attrs_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnPseudo",
                    detail: format!("Ui.onPseudo requires 2 arguments, got {}", args.len()),
                });
            };
            let pc = emit_expr_at(ctx, pc_e, indent, child, generics)?;
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_pseudo_({pc}, {attrs})"
            )))
        }

        // `Ui.breakpoint : String -> List (Attribute msg) -> Element msg -> Element msg`
        // Eager passthrough — breakpoint CSS media queries are not yet
        // applied in the Rust runtime.  The element is returned unchanged.
        KernelFn::UiBreakpoint => {
            let [q_e, a_e, el_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiBreakpoint",
                    detail: format!("Ui.breakpoint requires 3 arguments, got {}", args.len()),
                });
            };
            let q = emit_expr_at(ctx, q_e, indent, child, generics)?;
            let a = emit_expr_at(ctx, a_e, indent, child, generics)?;
            let el = emit_expr_at(ctx, el_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_breakpoint_({q}, {a}, {el})"
            )))
        }

        // `Ui.mediaQuery : String -> List (Attribute msg) -> Element msg -> Element msg`
        // Raw-CSS-media-query escape hatch: wraps the child in a
        // marker-carrying `<div>` (`data-ipe-mq-q` / `data-ipe-mq-rules`)
        // consumed by `live::style_inject::build_mq` into a ipe-id-scoped
        // `<style data-ipe-mq=…>` block. The query string is gated through
        // `SafeCssMediaQuery` inside the runtime helper (fail-closed drop).
        // See docs/adr/0019-ui-mediaquery-safe-boundary.md.
        KernelFn::UiMediaQuery => {
            let [q_e, a_e, el_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiMediaQuery",
                    detail: format!("Ui.mediaQuery requires 3 arguments, got {}", args.len()),
                });
            };
            let q = emit_expr_at(ctx, q_e, indent, child, generics)?;
            let a = emit_expr_at(ctx, a_e, indent, child, generics)?;
            let el = emit_expr_at(ctx, el_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_media_query_({q}, {a}, {el})"
            )))
        }

        // Background pseudo-class colour attrs (Color -> Attribute msg)
        KernelFn::BackgroundHoverColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BackgroundHoverColor",
                    detail: format!(
                        "Background.hoverColor requires 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_bg_hover_color_({c})"
            )))
        }
        KernelFn::BackgroundFocusColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BackgroundFocusColor",
                    detail: format!(
                        "Background.focusColor requires 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_bg_focus_color_({c})"
            )))
        }
        KernelFn::BackgroundActiveColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BackgroundActiveColor",
                    detail: format!(
                        "Background.activeColor requires 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_bg_active_color_({c})"
            )))
        }
        KernelFn::BackgroundDisabledColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BackgroundDisabledColor",
                    detail: format!(
                        "Background.disabledColor requires 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_bg_disabled_color_({c})"
            )))
        }

        // Border namespace — nullary style attrs
        KernelFn::BorderSolid => Ok(Some(
            "ipe_runtime::ui::helpers::ui_border_solid_()".to_owned(),
        )),
        KernelFn::BorderDashed => Ok(Some(
            "ipe_runtime::ui::helpers::ui_border_dashed_()".to_owned(),
        )),
        KernelFn::BorderDotted => Ok(Some(
            "ipe_runtime::ui::helpers::ui_border_dotted_()".to_owned(),
        )),

        // Border pseudo-class attrs
        KernelFn::BorderHoverColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderHoverColor",
                    detail: format!("Border.hoverColor requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_hover_color_({c})"
            )))
        }
        KernelFn::BorderFocusColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderFocusColor",
                    detail: format!("Border.focusColor requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_focus_color_({c})"
            )))
        }
        KernelFn::BorderActiveColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderActiveColor",
                    detail: format!("Border.activeColor requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_active_color_({c})"
            )))
        }
        KernelFn::BorderHoverWidth => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderHoverWidth",
                    detail: format!("Border.hoverWidth requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_hover_width_({n})"
            )))
        }
        KernelFn::BorderHoverRounded => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::BorderHoverRounded",
                    detail: format!(
                        "Border.hoverRounded requires 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_border_hover_rounded_({n})"
            )))
        }

        // Font namespace — Int-keyed weight
        KernelFn::FontWeight => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::FontWeight",
                    detail: format!("Font.weight requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_font_weight_({n})"
            )))
        }

        // Font namespace — nullary weight presets
        KernelFn::FontSemiBold => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_semi_bold_()".to_owned(),
        )),
        KernelFn::FontRegular => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_regular_()".to_owned(),
        )),
        KernelFn::FontLight => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_light_()".to_owned(),
        )),
        KernelFn::FontExtraBold => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_extra_bold_()".to_owned(),
        )),
        KernelFn::FontBlack => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_black_()".to_owned(),
        )),

        // Font namespace — nullary decoration
        KernelFn::FontUnderline => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_underline_()".to_owned(),
        )),
        KernelFn::FontNoDecoration => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_no_decoration_()".to_owned(),
        )),
        KernelFn::FontLineThrough => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_line_through_()".to_owned(),
        )),

        // Font namespace — Float spacing attrs
        KernelFn::FontLetterSpacing => {
            let [v_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::FontLetterSpacing",
                    detail: format!("Font.letterSpacing requires 1 argument, got {}", args.len()),
                });
            };
            let v = emit_expr_at(ctx, v_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_font_letter_spacing_({v})"
            )))
        }
        KernelFn::FontWordSpacing => {
            let [v_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::FontWordSpacing",
                    detail: format!("Font.wordSpacing requires 1 argument, got {}", args.len()),
                });
            };
            let v = emit_expr_at(ctx, v_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_font_word_spacing_({v})"
            )))
        }

        // Font namespace — nullary text-alignment
        KernelFn::FontAlignLeft => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_align_left_()".to_owned(),
        )),
        KernelFn::FontAlignRight => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_align_right_()".to_owned(),
        )),
        KernelFn::FontAlignCenter => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_align_center_()".to_owned(),
        )),
        KernelFn::FontCenter => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_center_()".to_owned(),
        )),
        KernelFn::FontJustify => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_justify_()".to_owned(),
        )),

        // Font namespace — String constants (nullary, return String not Attr)
        KernelFn::FontSansSerif => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_sans_serif_()".to_owned(),
        )),
        KernelFn::FontSerif => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_serif_()".to_owned(),
        )),
        KernelFn::FontMonospace => Ok(Some(
            "ipe_runtime::ui::helpers::ui_font_monospace_()".to_owned(),
        )),

        // Font namespace — pseudo-class colour attrs (Color -> Attribute msg)
        KernelFn::FontHoverColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::FontHoverColor",
                    detail: format!("Font.hoverColor requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_font_hover_color_({c})"
            )))
        }
        KernelFn::FontFocusColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::FontFocusColor",
                    detail: format!("Font.focusColor requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_font_focus_color_({c})"
            )))
        }
        KernelFn::FontActiveColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::FontActiveColor",
                    detail: format!("Font.activeColor requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_font_active_color_({c})"
            )))
        }
        KernelFn::FontDisabledColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::FontDisabledColor",
                    detail: format!("Font.disabledColor requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_font_disabled_color_({c})"
            )))
        }
        KernelFn::FontHoverSize => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::FontHoverSize",
                    detail: format!("Font.hoverSize requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_font_hover_size_({n})"
            )))
        }

        // Html.Attributes — tabindex (Int → Html.Attribute msg)
        // Converted to string at emit time: `tabindex 3` → `<... tabindex="3">`.
        KernelFn::HtmlAttrTabindex => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlAttrTabindex",
                    detail: format!("Attr.tabindex requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::html::html_named_attr_(\"tabindex\".to_owned(), ({n}).to_string())"
            )))
        }

        // Html.Attributes — rows (Int → Html.Attribute msg)
        // Used on `<textarea rows="N">`.
        KernelFn::HtmlAttrRows => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlAttrRows",
                    detail: format!("Attr.rows requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::html::html_named_attr_(\"rows\".to_owned(), ({n}).to_string())"
            )))
        }

        // ── Ipe.Ui.Region ──────────────────────────────────────────────

        // `Region.mainContent : Attribute msg`
        KernelFn::RegionMainContent => Ok(Some(
            "ipe_runtime::ui::helpers::ui_region_main_content_()".to_owned(),
        )),

        // `Region.navigation : Attribute msg`
        KernelFn::RegionNavigation => Ok(Some(
            "ipe_runtime::ui::helpers::ui_region_navigation_()".to_owned(),
        )),

        // `Region.footer : Attribute msg`
        KernelFn::RegionFooter => Ok(Some(
            "ipe_runtime::ui::helpers::ui_region_footer_()".to_owned(),
        )),

        // `Region.aside : Attribute msg`
        KernelFn::RegionAside => Ok(Some(
            "ipe_runtime::ui::helpers::ui_region_aside_()".to_owned(),
        )),

        // `Region.heading : Int -> Attribute msg`
        KernelFn::RegionHeading => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::RegionHeading",
                    detail: format!("Region.heading requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_region_heading_({n})"
            )))
        }

        // `Region.label : String -> Attribute msg`
        KernelFn::RegionLabel => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::RegionLabel",
                    detail: format!("Region.label requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_region_label_({s})"
            )))
        }

        // `Region.announce : Attribute msg`
        KernelFn::RegionAnnounce => Ok(Some(
            "ipe_runtime::ui::helpers::ui_region_announce_()".to_owned(),
        )),

        // `Region.announceUrgently : Attribute msg`
        KernelFn::RegionAnnounceUrgently => Ok(Some(
            "ipe_runtime::ui::helpers::ui_region_announce_urgently_()".to_owned(),
        )),

        // ── Ui.input + Ui.describe + desc* constructors ───────────────────────

        // `Ui.input : List (Attribute msg) -> Element msg`
        KernelFn::UiInput => {
            let [attrs_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiInput",
                    detail: format!("Ui.input requires 1 argument, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_input_({attrs})"
            )))
        }

        // `Ui.describe : Description -> Attribute msg`
        KernelFn::UiDescribe => {
            let [d_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiDescribe",
                    detail: format!("Ui.describe requires 1 argument, got {}", args.len()),
                });
            };
            let d = emit_expr_at(ctx, d_e, indent, child, generics)?;
            Ok(Some(format!("ipe_runtime::ui::helpers::ui_describe_({d})")))
        }

        // Nullary `Description` constructors (arity 0).
        KernelFn::UiDescMain => Ok(Some("ipe_runtime::ui::helpers::ui_desc_main_()".to_owned())),
        KernelFn::UiDescNavigation => Ok(Some(
            "ipe_runtime::ui::helpers::ui_desc_navigation_()".to_owned(),
        )),
        KernelFn::UiDescContentInfo => Ok(Some(
            "ipe_runtime::ui::helpers::ui_desc_content_info_()".to_owned(),
        )),
        KernelFn::UiDescComplementary => Ok(Some(
            "ipe_runtime::ui::helpers::ui_desc_complementary_()".to_owned(),
        )),
        KernelFn::UiDescLivePolite => Ok(Some(
            "ipe_runtime::ui::helpers::ui_desc_live_polite_()".to_owned(),
        )),
        KernelFn::UiDescLiveAssertive => Ok(Some(
            "ipe_runtime::ui::helpers::ui_desc_live_assertive_()".to_owned(),
        )),

        // `Ui.descHeading : Int -> Description`
        KernelFn::UiDescHeading => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiDescHeading",
                    detail: format!("Ui.descHeading requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_desc_heading_({n})"
            )))
        }

        // `Ui.descLabel : String -> Description`
        KernelFn::UiDescLabel => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiDescLabel",
                    detail: format!("Ui.descLabel requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_desc_label_({s})"
            )))
        }

        // ── Ipe.Ui.Input — Label constructors ──────────────────────────
        KernelFn::InputLabelAbove => {
            let [attrs_e, el_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputLabelAbove",
                    detail: format!("Input.labelAbove requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let el_s = emit_expr_at(ctx, el_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_label_above_({attrs_s}, {el_s})"
            )))
        }

        KernelFn::InputLabelBelow => {
            let [attrs_e, el_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputLabelBelow",
                    detail: format!("Input.labelBelow requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let el_s = emit_expr_at(ctx, el_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_label_below_({attrs_s}, {el_s})"
            )))
        }

        KernelFn::InputLabelLeft => {
            let [attrs_e, el_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputLabelLeft",
                    detail: format!("Input.labelLeft requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let el_s = emit_expr_at(ctx, el_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_label_left_({attrs_s}, {el_s})"
            )))
        }

        KernelFn::InputLabelRight => {
            let [attrs_e, el_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputLabelRight",
                    detail: format!("Input.labelRight requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let el_s = emit_expr_at(ctx, el_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_label_right_({attrs_s}, {el_s})"
            )))
        }

        KernelFn::InputLabelHidden => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputLabelHidden",
                    detail: format!("Input.labelHidden requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_label_hidden_({s})"
            )))
        }

        KernelFn::InputPlaceholder => {
            let [attrs_e, el_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputPlaceholder",
                    detail: format!("Input.placeholder requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let el_s = emit_expr_at(ctx, el_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_placeholder_({attrs_s}, {el_s})"
            )))
        }

        // ── Ipe.Ui.Input — text-family controls ────────────────────────
        KernelFn::InputText
        | KernelFn::InputEmail
        | KernelFn::InputUsername
        | KernelFn::InputSearch
        | KernelFn::InputCurrentPassword
        | KernelFn::InputNewPassword => {
            let [attrs_e, cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputText",
                    detail: format!(
                        "Input.text/email/… requires 2 arguments, got {}",
                        args.len()
                    ),
                });
            };
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputText",
                    detail: "Input.text cfg must be an inline record literal in Phase 0; \
                             non-literal cfg is deferred to Phase 1"
                        .into(),
                });
            };
            let on_change_e = lookup_field(
                ctx,
                fields,
                "onChange",
                "ipe_backend_rust::emit_ui_call::InputText::onChange",
            )?;
            let text_e = lookup_field(
                ctx,
                fields,
                "text",
                "ipe_backend_rust::emit_ui_call::InputText::text",
            )?;
            let placeholder_e = lookup_field(
                ctx,
                fields,
                "placeholder",
                "ipe_backend_rust::emit_ui_call::InputText::placeholder",
            )?;
            let label_e = lookup_field(
                ctx,
                fields,
                "label",
                "ipe_backend_rust::emit_ui_call::InputText::label",
            )?;
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let on_change_s = emit_arc_callback_field(ctx, on_change_e, indent, child, generics)?;
            let text_s = emit_expr_at(ctx, text_e, indent, child, generics)?;
            let placeholder_s = emit_expr_at(ctx, placeholder_e, indent, child, generics)?;
            let label_s = emit_expr_at(ctx, label_e, indent, child, generics)?;
            let fn_name = match k {
                KernelFn::InputEmail => "input_email_",
                KernelFn::InputUsername => "input_username_",
                KernelFn::InputSearch => "input_search_",
                KernelFn::InputCurrentPassword => "input_current_password_",
                KernelFn::InputNewPassword => "input_new_password_",
                _ => "input_text_",
            };
            Ok(Some(format!(
                "ipe_runtime::ui::input::{fn_name}({attrs_s}, {on_change_s}, {text_s}, {placeholder_s}, {label_s})"
            )))
        }

        KernelFn::InputMultiline => {
            let [attrs_e, cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputMultiline",
                    detail: format!("Input.multiline requires 2 arguments, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputMultiline",
                    detail: "Input.multiline cfg must be an inline record literal in Phase 0"
                        .into(),
                });
            };
            let on_change_e = lookup_field(
                ctx,
                fields,
                "onChange",
                "ipe_backend_rust::emit_ui_call::InputMultiline::onChange",
            )?;
            let text_e = lookup_field(
                ctx,
                fields,
                "text",
                "ipe_backend_rust::emit_ui_call::InputMultiline::text",
            )?;
            let placeholder_e = lookup_field(
                ctx,
                fields,
                "placeholder",
                "ipe_backend_rust::emit_ui_call::InputMultiline::placeholder",
            )?;
            let label_e = lookup_field(
                ctx,
                fields,
                "label",
                "ipe_backend_rust::emit_ui_call::InputMultiline::label",
            )?;
            let spellcheck_e = lookup_field(
                ctx,
                fields,
                "spellcheck",
                "ipe_backend_rust::emit_ui_call::InputMultiline::spellcheck",
            )?;
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let on_change_s = emit_arc_callback_field(ctx, on_change_e, indent, child, generics)?;
            let text_s = emit_expr_at(ctx, text_e, indent, child, generics)?;
            let placeholder_s = emit_expr_at(ctx, placeholder_e, indent, child, generics)?;
            let label_s = emit_expr_at(ctx, label_e, indent, child, generics)?;
            let spellcheck_s = emit_expr_at(ctx, spellcheck_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_multiline_({attrs_s}, {on_change_s}, {text_s}, {placeholder_s}, {label_s}, {spellcheck_s})"
            )))
        }

        KernelFn::InputCheckbox => {
            let [attrs_e, cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputCheckbox",
                    detail: format!("Input.checkbox requires 2 arguments, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputCheckbox",
                    detail: "Input.checkbox cfg must be an inline record literal in Phase 0".into(),
                });
            };
            let on_change_e = lookup_field(
                ctx,
                fields,
                "onChange",
                "ipe_backend_rust::emit_ui_call::InputCheckbox::onChange",
            )?;
            let icon_e = lookup_field(
                ctx,
                fields,
                "icon",
                "ipe_backend_rust::emit_ui_call::InputCheckbox::icon",
            )?;
            let checked_e = lookup_field(
                ctx,
                fields,
                "checked",
                "ipe_backend_rust::emit_ui_call::InputCheckbox::checked",
            )?;
            let label_e = lookup_field(
                ctx,
                fields,
                "label",
                "ipe_backend_rust::emit_ui_call::InputCheckbox::label",
            )?;
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let on_change_s = emit_arc_callback_field(ctx, on_change_e, indent, child, generics)?;
            let icon_s = emit_arc_callback_field(ctx, icon_e, indent, child, generics)?;
            let checked_s = emit_expr_at(ctx, checked_e, indent, child, generics)?;
            let label_s = emit_expr_at(ctx, label_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_checkbox_({attrs_s}, {on_change_s}, {icon_s}, {checked_s}, {label_s})"
            )))
        }

        // `Input.slider attrs { onChange, value, min, max, step, label }`
        KernelFn::InputSlider => {
            let [attrs_e, cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputSlider",
                    detail: format!("Input.slider requires 2 arguments, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputSlider",
                    detail: "Input.slider cfg must be an inline record literal in Phase 0".into(),
                });
            };
            let on_change_e = lookup_field(
                ctx,
                fields,
                "onChange",
                "ipe_backend_rust::emit_ui_call::InputSlider::onChange",
            )?;
            let value_e = lookup_field(
                ctx,
                fields,
                "value",
                "ipe_backend_rust::emit_ui_call::InputSlider::value",
            )?;
            let min_e = lookup_field(
                ctx,
                fields,
                "min",
                "ipe_backend_rust::emit_ui_call::InputSlider::min",
            )?;
            let max_e = lookup_field(
                ctx,
                fields,
                "max",
                "ipe_backend_rust::emit_ui_call::InputSlider::max",
            )?;
            let step_e = lookup_field(
                ctx,
                fields,
                "step",
                "ipe_backend_rust::emit_ui_call::InputSlider::step",
            )?;
            let label_e = lookup_field(
                ctx,
                fields,
                "label",
                "ipe_backend_rust::emit_ui_call::InputSlider::label",
            )?;
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let on_change_s = emit_arc_callback_field(ctx, on_change_e, indent, child, generics)?;
            let value_s = emit_expr_at(ctx, value_e, indent, child, generics)?;
            let min_s = emit_expr_at(ctx, min_e, indent, child, generics)?;
            let max_s = emit_expr_at(ctx, max_e, indent, child, generics)?;
            let step_s = emit_expr_at(ctx, step_e, indent, child, generics)?;
            let label_s = emit_expr_at(ctx, label_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_slider_({attrs_s}, {on_change_s}, {value_s}, {min_s}, {max_s}, {step_s}, {label_s})"
            )))
        }

        // `Input.option value labelEl` — constructs a RadioOption
        KernelFn::InputOption => {
            let [value_e, label_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputOption",
                    detail: format!("Input.option requires 2 arguments, got {}", args.len()),
                });
            };
            let value_s = emit_expr_at(ctx, value_e, indent, child, generics)?;
            let label_s = emit_expr_at(ctx, label_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_option_({value_s}, {label_s})"
            )))
        }

        // `Input.radio attrs { onChange, options, selected, label }`
        KernelFn::InputRadio => {
            let [attrs_e, cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputRadio",
                    detail: format!("Input.radio requires 2 arguments, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputRadio",
                    detail: "Input.radio cfg must be an inline record literal in Phase 0".into(),
                });
            };
            let on_change_e = lookup_field(
                ctx,
                fields,
                "onChange",
                "ipe_backend_rust::emit_ui_call::InputRadio::onChange",
            )?;
            let options_e = lookup_field(
                ctx,
                fields,
                "options",
                "ipe_backend_rust::emit_ui_call::InputRadio::options",
            )?;
            let selected_e = lookup_field(
                ctx,
                fields,
                "selected",
                "ipe_backend_rust::emit_ui_call::InputRadio::selected",
            )?;
            let label_e = lookup_field(
                ctx,
                fields,
                "label",
                "ipe_backend_rust::emit_ui_call::InputRadio::label",
            )?;
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let on_change_s = emit_arc_callback_field(ctx, on_change_e, indent, child, generics)?;
            let options_s = emit_expr_at(ctx, options_e, indent, child, generics)?;
            let selected_s = emit_expr_at(ctx, selected_e, indent, child, generics)?;
            let label_s = emit_expr_at(ctx, label_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_radio_({attrs_s}, {on_change_s}, {options_s}, {selected_s}, {label_s})"
            )))
        }

        // `Input.radioRow attrs { onChange, options, selected, label }`
        KernelFn::InputRadioRow => {
            let [attrs_e, cfg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputRadioRow",
                    detail: format!("Input.radioRow requires 2 arguments, got {}", args.len()),
                });
            };
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::InputRadioRow",
                    detail: "Input.radioRow cfg must be an inline record literal in Phase 0".into(),
                });
            };
            let on_change_e = lookup_field(
                ctx,
                fields,
                "onChange",
                "ipe_backend_rust::emit_ui_call::InputRadioRow::onChange",
            )?;
            let options_e = lookup_field(
                ctx,
                fields,
                "options",
                "ipe_backend_rust::emit_ui_call::InputRadioRow::options",
            )?;
            let selected_e = lookup_field(
                ctx,
                fields,
                "selected",
                "ipe_backend_rust::emit_ui_call::InputRadioRow::selected",
            )?;
            let label_e = lookup_field(
                ctx,
                fields,
                "label",
                "ipe_backend_rust::emit_ui_call::InputRadioRow::label",
            )?;
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let on_change_s = emit_arc_callback_field(ctx, on_change_e, indent, child, generics)?;
            let options_s = emit_expr_at(ctx, options_e, indent, child, generics)?;
            let selected_s = emit_expr_at(ctx, selected_e, indent, child, generics)?;
            let label_s = emit_expr_at(ctx, label_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::input::input_radio_row_({attrs_s}, {on_change_s}, {options_s}, {selected_s}, {label_s})"
            )))
        }

        // ── Ipe.Html element builders ─────────────────────────────────────────

        // `Html.text : String -> Html msg`
        KernelFn::HtmlTextNode => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlTextNode",
                    detail: format!("Html.text requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_text_node_({s})"
            )))
        }

        // `Html.unsafeRaw : String -> Html msg`
        KernelFn::HtmlRawNode => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlRawNode",
                    detail: format!("Html.unsafeRaw requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_raw_node_({s})"
            )))
        }

        // `Html.node : String -> List Attr -> List Html -> Html msg`
        KernelFn::HtmlNode => {
            let [tag_e, attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlNode",
                    detail: format!("Html.node requires 3 arguments, got {}", args.len()),
                });
            };
            let tag = emit_expr_at(ctx, tag_e, indent, child, generics)?;
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_node_({tag}, {attrs}, {children})"
            )))
        }

        // `Html.voidNode : String -> List Attr -> Html msg` — the generic
        // void counterpart of `Html.node`: arbitrary runtime tag, no children
        // arg. Shares the same `html_node_` sink with an emit-baked empty
        // children vec, exactly like the fixed-tag void builders below.
        KernelFn::HtmlVoidNode => {
            let [tag_e, attrs_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlVoidNode",
                    detail: format!("Html.voidNode requires 2 arguments, got {}", args.len()),
                });
            };
            let tag = emit_expr_at(ctx, tag_e, indent, child, generics)?;
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_node_({tag}, {attrs}, ::std::vec::Vec::new())"
            )))
        }

        // `Html.doctype : List Html -> Html msg` — wraps children in the
        // `!doctype-wrapper` pseudo-tag; `html::render_into_ctx` already
        // special-cases that tag to emit `<!DOCTYPE html>`.
        KernelFn::HtmlDoctype => {
            let [children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlDoctype",
                    detail: format!("Html.doctype requires 1 argument, got {}", args.len()),
                });
            };
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_doctype_({children})"
            )))
        }

        // `Html.titleNode : String -> Html msg` — wraps a raw string
        // directly in `<title>`.
        KernelFn::HtmlTitleNode => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlTitleNode",
                    detail: format!("Html.titleNode requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_title_node_({s})"
            )))
        }

        // `Html.styleNode : List Attr -> String -> Html msg` (arity-2; the
        // dedicated kernel close-tag-neutralises the CSS body — F7).
        KernelFn::HtmlStyleNode => {
            let [attrs_e, css_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlStyleNode",
                    detail: format!("Html.styleNode requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let css = emit_expr_at(ctx, css_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_style_node_({attrs}, {css})"
            )))
        }

        // `Html.div : List Attr -> List Html -> Html msg`
        KernelFn::HtmlDiv => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlDiv",
                    detail: format!("Html.div requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_div_({attrs}, {children})"
            )))
        }

        // `Html.span : List Attr -> List Html -> Html msg`
        KernelFn::HtmlSpan => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlSpan",
                    detail: format!("Html.span requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_span_({attrs}, {children})"
            )))
        }

        // `Html.a : List Attr -> List Html -> Html msg`
        KernelFn::HtmlA => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlA",
                    detail: format!("Html.a requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_a_({attrs}, {children})"
            )))
        }

        // `Html.button : List Attr -> List Html -> Html msg`
        KernelFn::HtmlButton => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlButton",
                    detail: format!("Html.button requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_button_({attrs}, {children})"
            )))
        }

        // `Html.p (and other block elements) : List Attr -> List Html -> Html msg`
        KernelFn::HtmlP => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlP",
                    detail: format!("Html.p/block requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_p_({attrs}, {children})"
            )))
        }

        // `Html.input : List Attr -> Html msg` (void element)
        KernelFn::HtmlInput => {
            let [attrs_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlInput",
                    detail: format!("Html.input requires 1 argument, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_input_({attrs})"
            )))
        }

        // `Html.img : List Attr -> Html msg` (void element)
        KernelFn::HtmlImg => {
            let [attrs_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlImg",
                    detail: format!("Html.img requires 1 argument, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_img_({attrs})"
            )))
        }

        // ── Ipe.Html element builders (tag-as-data) ──────────────────
        //
        // Every container (`h1`/`nav`/`table`/…) and void (`br`/`hr`/`link`/…)
        // element routes through the SAME generic `html_node_(tag, attrs, children)`
        // runtime sink — no per-tag runtime fn. The wire tag is the kernel's
        // `html_element_tag()` literal (injected here as data), so `nav` renders
        // `<nav>`, `h1` renders `<h1>`, etc. — NOT the old wrong-render fold to
        // `<p>`/`<img>`. Void elements pass an empty child vec; the render sink
        // (`html::render_into`) additionally self-closes and drops children for any
        // tag in its `VOID` set, so no injected-child XSS surface exists.

        // Container: `<tag> : List Attr -> List Html -> Html msg`.
        k if k.is_html_container() => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlContainer",
                    detail: format!("{k:?} container requires 2 arguments, got {}", args.len()),
                });
            };
            let tag = k
                .html_element_tag()
                .ok_or_else(|| Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlContainer",
                    detail: format!("{k:?} is_html_container but html_element_tag returned None"),
                })?;
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_node_({tag:?}.to_owned(), {attrs}, {children})"
            )))
        }

        // Void: `<tag> : List Attr -> Html msg` (no children).
        k if k.is_html_void() => {
            let [attrs_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlVoid",
                    detail: format!("{k:?} void element requires 1 argument, got {}", args.len()),
                });
            };
            let tag = k
                .html_element_tag()
                .ok_or_else(|| Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlVoid",
                    detail: format!("{k:?} is_html_void but html_element_tag returned None"),
                })?;
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::html_node_({tag:?}.to_owned(), {attrs}, ::std::vec::Vec::new())"
            )))
        }

        // ── Event-attribute builders ─────────────────────────────────────────────
        //
        // Plain-message events (onClick/onFocus/onBlur/onMouseOver/onMouseOut):
        //   Ui.onClick : msg -> Attribute msg
        //   emit: ipe_runtime::ui::helpers::ui_on_click_(msg_expr)
        //
        // String-carrying events (onInput/onChange/onKeyDown/onKeyUp) — T6 trap:
        //   The Ipê fn arg is an emitted Rust fn-value (closure or fn-ptr).
        //   The runtime requires `Arc<dyn Fn(String)->M+Send+Sync>`.
        //   We emit: ui_on_input_(std::sync::Arc::new(move |_x| (f)(_x)))
        //   This is sound: the Arc captures `f` by move; `f` is always 'static
        //   since emitted Ipê fns carry no borrow-lifetime context.

        // `Ui.onClick / Event.onClick : msg -> Attribute msg`
        KernelFn::UiOnClick => {
            let [msg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnClick",
                    detail: format!("Ui.onClick requires 1 argument, got {}", args.len()),
                });
            };
            let msg_s = emit_expr_at(ctx, msg_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_click_({msg_s})"
            )))
        }

        // `Ui.onFocus : msg -> Attribute msg`
        KernelFn::UiOnFocus => {
            let [msg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnFocus",
                    detail: format!("Ui.onFocus requires 1 argument, got {}", args.len()),
                });
            };
            let msg_s = emit_expr_at(ctx, msg_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_focus_({msg_s})"
            )))
        }

        // `Ui.onBlur : msg -> Attribute msg`
        KernelFn::UiOnBlur => {
            let [msg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnBlur",
                    detail: format!("Ui.onBlur requires 1 argument, got {}", args.len()),
                });
            };
            let msg_s = emit_expr_at(ctx, msg_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_blur_({msg_s})"
            )))
        }

        // `Ui.onMouseOver : msg -> Attribute msg`
        KernelFn::UiOnMouseOver => {
            let [msg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnMouseOver",
                    detail: format!("Ui.onMouseOver requires 1 argument, got {}", args.len()),
                });
            };
            let msg_s = emit_expr_at(ctx, msg_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_mouse_over_({msg_s})"
            )))
        }

        // `Ui.onMouseOut : msg -> Attribute msg`
        KernelFn::UiOnMouseOut => {
            let [msg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnMouseOut",
                    detail: format!("Ui.onMouseOut requires 1 argument, got {}", args.len()),
                });
            };
            let msg_s = emit_expr_at(ctx, msg_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_mouse_out_({msg_s})"
            )))
        }

        // `Ui.onInput : (String -> msg) -> Attribute msg`  (Arc-wrap the fn)
        //
        // D5: route through `emit_arc_callback_field` so any lowerer-hoisted
        // capture-clone `let`s (pre-clone `Let { value: CloneVar }` wrapping the
        // Lambda) are peeled OUTSIDE the synthesized `Arc::new(move |_x| …)`.
        // Without this, the outer `move` still move-captures the free outer binding
        // and a sibling use hits E0382 — the same move-capture bug shape the
        // on_change FIELD path guards against, applied to the inline-wrap sites.
        // When there are no leading pure-alias `let`s, `emit_arc_callback_field`
        // produces output byte-identical to a plain `arc_callback_wrap` call.
        KernelFn::UiOnInput => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnInput",
                    detail: format!("Ui.onInput requires 1 argument, got {}", args.len()),
                });
            };
            // Peel any leading capture-clone `let`s outside the Arc closure.
            let peeled = emit_arc_callback_field(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_input_({peeled})"
            )))
        }

        // `Ui.onChange : (String -> msg) -> Attribute msg`  (Arc-wrap)
        // D5: same peel-hoist as UiOnInput above.
        KernelFn::UiOnChange => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnChange",
                    detail: format!("Ui.onChange requires 1 argument, got {}", args.len()),
                });
            };
            let peeled = emit_arc_callback_field(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_change_({peeled})"
            )))
        }

        // `Ui.onKeyDown : (String -> msg) -> Attribute msg`  (Arc-wrap)
        KernelFn::UiOnKeyDown => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnKeyDown",
                    detail: format!("Ui.onKeyDown requires 1 argument, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_key_down_(::std::sync::Arc::new(move |_x| ({f_s})(_x)))"
            )))
        }

        // `Ui.onKeyUp : (String -> msg) -> Attribute msg`  (Arc-wrap)
        KernelFn::UiOnKeyUp => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnKeyUp",
                    detail: format!("Ui.onKeyUp requires 1 argument, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_key_up_(::std::sync::Arc::new(move |_x| ({f_s})(_x)))"
            )))
        }

        // `Ui.onFile : (String -> msg) -> Attribute msg`  (Arc-wrap)
        KernelFn::UiOnFile => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnFile",
                    detail: format!("Ui.onFile requires 1 argument, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_file_(::std::sync::Arc::new(move |_x| ({f_s})(_x)))"
            )))
        }

        // `Event.onBool : (Bool -> msg) -> Attribute msg`  (Arc-wrap, bool arg)
        KernelFn::UiOnBool => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnBool",
                    detail: format!("Event.onBool requires 1 argument, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::helpers::ui_on_bool_(::std::sync::Arc::new(move |_x| ({f_s})(_x)))"
            )))
        }

        // `Ui.onSubmit : (a -> msg) -> Attribute msg`
        // `ui_on_submit_` builds `Event::OnForm` with the concrete argument
        // type recovered by Rust generic inference on the emitted handler
        // closure `f_s` — this emit site is generic over that type, and the
        // runtime function's signature carries it (never `Arc<dyn Any>`).
        //
        // `ui_on_submit_`'s generic bound is `F: Fn(T) -> M + Send +
        // Sync + 'static`, but `f_s` here is a `Box<dyn Fn(T) -> M + Send +
        // 'static>` trait object (the generic `IrType::Fun` rendering in
        // `emit_types.rs` never claims `+Sync`) — passed straight through as
        // `F`, that box can never satisfy `+ Sync` regardless of what the
        // closure inside captures (a trait object's auto-trait set is
        // exactly its bound list). Wrap in a freshly-declared closure
        // (`move |_x| ({f_s})(_x)`) the same way the `HtmlEvent` String/Bool
        // arms above do: `f_s`'s box-construction is re-embedded as SOURCE
        // inside the wrapper's body, so it is built anew on every call
        // rather than captured — the wrapper's own Send+Sync-ness then
        // depends only on the Ipê closure's legitimate `move` captures
        // (Send+'static by construction), not on the erased trait-object
        // type.
        //
        // this re-wrap ONLY helps when `f_e` is an INLINE
        // `Lambda`/`FuncValue` here (the box is rebuilt as source inside the
        // wrapper body, never captured). When `f_e` is `Expr::Var(sym)`
        // referencing a PREVIOUSLY `let`-bound closure, `f_s` is the bare
        // identifier, and `move |_x| (handler)(_x)` MOVES the already-built
        // `Box<dyn Fn + Send>` into the wrapper's captures — a non-`Sync`
        // capture makes the wrapper non-`Sync`, so no emit-site fix is
        // possible for that shape (the box already exists by the time this arm
        // runs). The real fix is upstream in `ipe_lower::lower_let_pvar`:
        // `flows_into_sync_kernel_call` promotes the LET-BOUND VALUE itself to
        // `Expr::SharedLambda` (`Arc<dyn Fn + Send + Sync>`), so `f_s` here is
        // already `Send + Sync` — no change needed in this arm.
        KernelFn::UiOnSubmit => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::UiOnSubmit",
                    detail: format!("Ui.onSubmit requires 1 argument, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            // Type-directed dispatch. The lowerer classified the handler
            // by its SOLVED type; a non-arrow value routes to the fixed-dispatch
            // runtime helper (no `(m)(_x)` call against a non-callable value —
            // the reported cargo `E0618` after `ipe` exit 0). An arrow handler
            // keeps the decode-and-map path. `NotForm` is unreachable for the
            // onSubmit kernel and fails closed rather than guessing.
            let call = match on_form {
                ipe_ir::OnFormKind::Decoder => {
                    format!("ipe_runtime::ui::helpers::ui_on_submit_(move |_x| ({f_s})(_x))")
                }
                ipe_ir::OnFormKind::FixedValue => {
                    format!("ipe_runtime::ui::helpers::ui_on_submit_fixed_({f_s})")
                }
                ipe_ir::OnFormKind::NotForm => {
                    return Err(Diagnostic::CompilerBug {
                        where_: "ipe_backend_rust::emit_ui_call::UiOnSubmit",
                        detail: "Ui.onSubmit lowered without a form-handler classification"
                            .to_owned(),
                    });
                }
            };
            Ok(Some(call))
        }

        // ── Ipe.Html.Events builders ────────────────────────────────────
        // Produce a `html::Attribute::EventAttr(Event::On*)` via a dedicated
        // runtime constructor. The fixed wire event name (`"click"`, `"input"`,
        // …) is a compile-time constant from `html_event_wire_name`; the payload
        // shape (Msg / String / Bool / Raw) comes from `html_event_shape`. The
        // `String`/`Bool` forms Arc-wrap the emitted Ipê fn (`f` is a 'static
        // closure); the `Raw` (onSubmit) form (`html_on_raw_`) builds
        // `Event::OnForm` with the concrete payload type recovered by Rust
        // generic inference on the emitted closure — never a type-erased
        // handler.
        k if k.html_event_shape().is_some() => {
            let (Some(shape), Some(name)) = (k.html_event_shape(), k.html_event_wire_name()) else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlEvent",
                    detail: format!("{k:?} is not a fully-classified Html event kernel"),
                });
            };
            let [payload_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlEvent",
                    detail: format!("{k:?} requires exactly 1 argument, got {}", args.len()),
                });
            };
            let payload_s = emit_expr_at(ctx, payload_e, indent, child, generics)?;
            let call = match shape {
                ipe_ir::HtmlEventShape::Msg => {
                    format!("ipe_runtime::html::html_on_msg_({name:?}.to_owned(), {payload_s})")
                }
                ipe_ir::HtmlEventShape::String => format!(
                    "ipe_runtime::html::html_on_string_({name:?}.to_owned(), \
                     ::std::sync::Arc::new(move |_x| ({payload_s})(_x)))"
                ),
                ipe_ir::HtmlEventShape::Bool => format!(
                    "ipe_runtime::html::html_on_bool_({name:?}.to_owned(), \
                     ::std::sync::Arc::new(move |_x| ({payload_s})(_x)))"
                ),
                // `html_on_raw_`'s own signature requires
                // `F: Fn(T) -> M + Send + Sync + 'static` (the runtime's
                // `Event::OnForm` slot is `Arc<dyn Fn(FormData) -> Option<M> +
                // Send + Sync>`, shared across the live session's dispatch
                // table — see `html.rs`'s `Event` doc comment). But
                // `payload_s` here is a `Box<dyn Fn(T) -> M + Send + 'static>`
                // trait object (the generic `IrType::Fun` rendering in
                // `emit_types.rs`, which never claims `+Sync` for a boxed
                // first-class function value) — a trait object's auto-trait
                // set is exactly what its bound lists, so passing that Box
                // value THROUGH unchanged as `F` can never satisfy `+ Sync`
                // regardless of what the closure inside actually captures.
                // The `String`/`Bool` arms above dodge this by re-embedding
                // `payload_s`'s SOURCE inside a freshly-declared wrapping
                // closure (`move |_x| ({payload_s})(_x)`) rather than passing
                // the boxed VALUE itself — the box is constructed anew each
                // call, inside the wrapping closure's body, so it is never
                // part of the wrapping closure's captured environment and
                // the wrapping closure's own Send+Sync-ness depends only on
                // whatever the Ipê closure itself legitimately captures
                // (`move` locals, all Send+'static by construction). Apply
                // the same technique here so `F` is this freshly-Sync outer
                // closure, not the non-Sync boxed trait object.
                //
                // `onSubmit`'s Ipê-level scheme (`constrain.rs`'s
                // `HtmlEventShape::Raw` arm) deliberately leaves the argument
                // type UNCONSTRAINED (decoupled from `msg`) so the typed-
                // record decode idiom above works. That also legitimately
                // types a BARE (non-function) `msg` value — the canonical
                // "form fields already synced into Model via onInput/
                // onChange; submit just triggers a fixed action" idiom
                // (`onSubmit DoSignUp` with `DoSignUp : Msg` carrying no
                // payload — `examples/12-ipevote`'s Auth/Submit/Detail
                // pages). `payload_s` there renders as the bare enum value
                // itself (e.g. `MainMsg::DoSignUp`), which is NOT callable —
                // `(payload_s)(_x)` is E0618 ("expected function, found
                // MainMsg"), a ipe-exit-0-then-cargo-fail SEAL violation.
                // `lower_expr`'s `VarCtor` arm already proves the shape: a
                // NULLARY constructor reference lowers straight to
                // `Expr::Ctor { args: [] }` (a saturated value), while a
                // PAYLOAD constructor reference used as a value is
                // eta-expanded into a genuine `Expr::Lambda` there — so
                // `Expr::Ctor` reaching this position (any arity — `Ctor`
                // is always fully saturated by construction, see its doc)
                // is PROVABLY not a function. Route it (and the other
                // leaf-literal shapes that are equally provably not
                // callable) to `html_on_raw_fixed_`, which dispatches the
                // fixed value directly and never attempts to decode
                // `FormData` into a placeholder type (that would risk a
                // spurious decode failure silently swallowing a real
                // form's submit — see that fn's doc). Every other shape
                // (`Lambda`, `FuncValue`, `Var`, `Apply`, `Call`, …) keeps
                // today's wrap-and-call path unchanged — conservative
                // default, since those CAN legitimately be a function
                // value (a let-bound handler, a named decoder function).
                // `onSubmit` (the only `Raw`-shape kernel). The
                // decode-vs-fixed decision is TYPE-DIRECTED: the lowerer read
                // the handler's SOLVED type and recorded the verdict on the
                // `Call` (`on_form`), so acceptance never depends on the
                // payload's syntactic shape — a `Var` bound to a bare `Msg`
                // and a `Var` bound to a decoder fn read identically
                // here, but the solver told them apart upstream.
                //
                // FixedValue → dispatch the value directly via
                // `html_on_raw_fixed_` (no `(payload_s)(_x)` call against a
                // non-callable value — the reported cargo `E0618` after `ipe`
                // exit 0). Decoder → the wrap-and-call path: `payload_s` (a
                // `Box<dyn Fn(T) -> M + Send + 'static>` trait object) is
                // re-embedded as SOURCE inside a freshly-declared wrapper
                // closure so its box is rebuilt per call rather than captured,
                // laundering the missing `+Sync` the `html_on_raw_` bound
                // (`F: Fn(T) -> M + Send + Sync + 'static`) requires.
                ipe_ir::HtmlEventShape::Raw => match on_form {
                    ipe_ir::OnFormKind::FixedValue => format!(
                        "ipe_runtime::html::html_on_raw_fixed_({name:?}.to_owned(), {payload_s})"
                    ),
                    ipe_ir::OnFormKind::Decoder => format!(
                        "ipe_runtime::html::html_on_raw_({name:?}.to_owned(), move |_x| ({payload_s})(_x))"
                    ),
                    ipe_ir::OnFormKind::NotForm => {
                        return Err(Diagnostic::CompilerBug {
                            where_: "ipe_backend_rust::emit_ui_call::HtmlOnSubmit",
                            detail: "onSubmit lowered without a form-handler classification"
                                .to_owned(),
                        });
                    }
                },
            };
            Ok(Some(call))
        }

        // ── Ipe.Html.Attributes builders ─────────────────────────────────
        // Fixed-key string attr: `class v` → `html_named_attr_("class", v)`.
        // The key is a compile-time literal (never attacker data); the VALUE is
        // escaped at the render sink (`escape_attr`), so no escaping here.
        k if k.is_html_str_attr() => {
            let [v_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlStrAttr",
                    detail: format!("{k:?} requires exactly 1 argument, got {}", args.len()),
                });
            };
            let key = k.html_attr_key().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_ui_call::HtmlStrAttr",
                detail: format!("{k:?} is_html_str_attr but html_attr_key returned None"),
            })?;
            let v_s = emit_expr_at(ctx, v_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::html::html_named_attr_({key:?}.to_owned(), {v_s})"
            )))
        }
        // Fixed-key bool attr: `checked b` → `html_bool_named_attr_("checked", b)`.
        k if k.is_html_bool_attr() => {
            let [b_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlBoolAttr",
                    detail: format!("{k:?} requires exactly 1 argument, got {}", args.len()),
                });
            };
            let key = k.html_attr_key().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_ui_call::HtmlBoolAttr",
                detail: format!("{k:?} is_html_bool_attr but html_attr_key returned None"),
            })?;
            let b_s = emit_expr_at(ctx, b_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::html::html_bool_named_attr_({key:?}.to_owned(), {b_s})"
            )))
        }
        // Generic `attribute k v` — runtime key gated at the render sink through
        // `SafeAttrName` (drops `on*`/`srcdoc`/charset-invalid names).
        KernelFn::HtmlAttribute => {
            let [k_e, v_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlAttribute",
                    detail: format!("Attr.attribute requires 2 arguments, got {}", args.len()),
                });
            };
            let k_s = emit_expr_at(ctx, k_e, indent, child, generics)?;
            let v_s = emit_expr_at(ctx, v_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::html::html_named_attr_({k_s}, {v_s})"
            )))
        }
        // Generic `boolAttribute k b`.
        KernelFn::HtmlBoolAttribute => {
            let [k_e, b_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::HtmlBoolAttribute",
                    detail: format!(
                        "Attr.boolAttribute requires 2 arguments, got {}",
                        args.len()
                    ),
                });
            };
            let k_s = emit_expr_at(ctx, k_e, indent, child, generics)?;
            let b_s = emit_expr_at(ctx, b_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::html::html_bool_named_attr_({k_s}, {b_s})"
            )))
        }
        // `noAttr : Attribute msg` — nullary identity attribute.
        KernelFn::HtmlNoAttr => Ok(Some("ipe_runtime::html::html_no_attr_()".to_owned())),

        // ── Web app-entry kernels ─────────────────────────────────────────────
        // Delegate to `emit_web::emit_web_call`; it returns `Some(s)` for the
        // four Web variants and `None` for anything else (the `_ => None` arm).
        // A `None` here is an internal error (the `is_web()` guard above already
        // filtered to Web variants), so promote it to a `CompilerBug`.
        KernelFn::WebApp
        | KernelFn::WebAppRouted
        | KernelFn::WebRoute
        | KernelFn::WebRenderStatic => {
            let s = crate::emit_web::emit_web_call(ctx, callee, args, indent, child, generics)?
                .ok_or_else(|| Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call",
                    detail: format!("emit_web returned None for Web kernel {k:?} — missing arm"),
                })?;
            Ok(Some(s))
        }

        // ── Terminal full-screen app-entry ───────────────────────────────────
        // Delegate to `emit_tui::emit_tui_call`; it returns `Some(s)` for the
        // `appScreen` variant and `None` for anything else. A `None` here is an
        // internal error (the `k.is_tui()` guard already filtered), so promote
        // it to a `CompilerBug`.
        KernelFn::TerminalAppScreen => {
            let s = crate::emit_tui::emit_tui_call(ctx, callee, args, indent, child, generics)?
                .ok_or_else(|| Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call",
                    detail: format!(
                        "emit_tui returned None for Terminal kernel {k:?} — missing arm"
                    ),
                })?;
            Ok(Some(s))
        }

        // ── Webview app-entry kernel ─────────────────────────────────────────
        // Delegate to `emit_webview::emit_webview_call`; it returns `Some(s)` for
        // the WebviewApp variant and `None` for anything else. A `None` here is an
        // internal error (the `k.is_webview()` guard above already filtered), so
        // promote it to a `CompilerBug`.
        KernelFn::WebViewApp => {
            let s =
                crate::emit_webview::emit_webview_call(ctx, callee, args, indent, child, generics)?
                    .ok_or_else(|| Diagnostic::CompilerBug {
                        where_: "ipe_backend_rust::emit_ui_call",
                        detail: format!(
                            "emit_webview returned None for Webview kernel {k:?} — missing arm"
                        ),
                    })?;
            Ok(Some(s))
        }

        // ── Terminal line-oriented app-entry ─────────────────────────────────
        // Delegate to `emit_console::emit_console_call`; it returns `Some(s)` for
        // the `appLines` variant and `None` for anything else. A `None` here is
        // an internal error (the `k.is_console()` guard above already filtered),
        // so promote it to a `CompilerBug`.
        KernelFn::TerminalAppLines => {
            let s =
                crate::emit_console::emit_console_call(ctx, callee, args, indent, child, generics)?
                    .ok_or_else(|| Diagnostic::CompilerBug {
                        where_: "ipe_backend_rust::emit_ui_call",
                        detail: format!(
                            "emit_console returned None for Terminal kernel {k:?} — missing arm"
                        ),
                    })?;
            Ok(Some(s))
        }

        // ── Ipe.Ui.Keyed — ipe-key diff identity ─────────────────────────────
        // `Keyed.column : List Attr -> List (String, Element msg) -> Element msg`
        KernelFn::KeyedColumn => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::KeyedColumn",
                    detail: format!("Keyed.column requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::keyed::keyed_column_({attrs}, {children})"
            )))
        }

        // `Keyed.row : List Attr -> List (String, Element msg) -> Element msg`
        KernelFn::KeyedRow => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::KeyedRow",
                    detail: format!("Keyed.row requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::keyed::keyed_row_({attrs}, {children})"
            )))
        }

        // ── Ipe.Ui.Lazy — deferred subtree helpers ───────────────────────────
        // Each variant carries (f, a..e) — f is a function-valued Ipê expr;
        // we eta-wrap it so any callable shape (fn item, Box<dyn Fn>, closure)
        // is accepted by the `impl Fn` bound without Arc overhead.
        // Arg order MUST match the runtime signature; a swap is a silent bug.
        KernelFn::LazyLazy => {
            let [f_e, a_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::LazyLazy",
                    detail: format!("Lazy.lazy requires 2 arguments, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            let a_s = emit_expr_at(ctx, a_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::lazy::lazy_lazy_(move |_a| ({f_s})(_a), {a_s})"
            )))
        }

        KernelFn::LazyLazy2 => {
            let [f_e, a_e, b_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::LazyLazy2",
                    detail: format!("Lazy.lazy2 requires 3 arguments, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            let a_s = emit_expr_at(ctx, a_e, indent, child, generics)?;
            let b_s = emit_expr_at(ctx, b_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::lazy::lazy_lazy2_(move |_a, _b| ({f_s})(_a, _b), {a_s}, {b_s})"
            )))
        }

        KernelFn::LazyLazy3 => {
            let [f_e, a_e, b_e, c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::LazyLazy3",
                    detail: format!("Lazy.lazy3 requires 4 arguments, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            let a_s = emit_expr_at(ctx, a_e, indent, child, generics)?;
            let b_s = emit_expr_at(ctx, b_e, indent, child, generics)?;
            let c_s = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::lazy::lazy_lazy3_(move |_a, _b, _c| ({f_s})(_a, _b, _c), {a_s}, {b_s}, {c_s})"
            )))
        }

        KernelFn::LazyLazy4 => {
            let [f_e, a_e, b_e, c_e, d_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::LazyLazy4",
                    detail: format!("Lazy.lazy4 requires 5 arguments, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            let a_s = emit_expr_at(ctx, a_e, indent, child, generics)?;
            let b_s = emit_expr_at(ctx, b_e, indent, child, generics)?;
            let c_s = emit_expr_at(ctx, c_e, indent, child, generics)?;
            let d_s = emit_expr_at(ctx, d_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::lazy::lazy_lazy4_(move |_a, _b, _c, _d| ({f_s})(_a, _b, _c, _d), {a_s}, {b_s}, {c_s}, {d_s})"
            )))
        }

        KernelFn::LazyLazy5 => {
            let [f_e, a_e, b_e, c_e, d_e, e_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::LazyLazy5",
                    detail: format!("Lazy.lazy5 requires 6 arguments, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            let a_s = emit_expr_at(ctx, a_e, indent, child, generics)?;
            let b_s = emit_expr_at(ctx, b_e, indent, child, generics)?;
            let c_s = emit_expr_at(ctx, c_e, indent, child, generics)?;
            let d_s = emit_expr_at(ctx, d_e, indent, child, generics)?;
            let e_s = emit_expr_at(ctx, e_e, indent, child, generics)?;
            Ok(Some(format!(
                "ipe_runtime::ui::lazy::lazy_lazy5_(move |_a, _b, _c, _d, _e| ({f_s})(_a, _b, _c, _d, _e), {a_s}, {b_s}, {c_s}, {d_s}, {e_s})"
            )))
        }

        // ── Ipe.PubSub.publish / publishNoEcho ────────────────────────────
        // `pubsub_publish<T, E>(topic, payload) -> IpeTask<E, i64>` — T (payload)
        // infers from arg 1; E (error) appears ONLY in the IpeTask<E, i64> result,
        // so anchor it to IpeError with `<_, IpeError>` (T first, E second).
        // Mirror of the CsvParse `::<IpeError>` anchor; two generic slots because T
        // precedes E.  `pubsub_publish` is re-exported at ipe_runtime root via
        // `pub use live::*`, so no full path needed in the emitted crate. These are
        // `class = Web` (Task-shaped), not TEA-loop kernels — the runtime bus lives
        // in the `web` module's `live::pubsub`, hence their home here.
        KernelFn::PubSubPublish | KernelFn::PubSubPublishNoEcho => {
            let [topic_e, payload_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_ui_call::PubSubPublish",
                    detail: format!(
                        "PubSub.publish requires exactly 2 arguments, got {}",
                        args.len()
                    ),
                });
            };
            let topic_s = emit_expr_at(ctx, topic_e, indent, child, generics)?;
            let payload_s = emit_expr_at(ctx, payload_e, indent, child, generics)?;
            let name = kernel_name(*k); // "pubsub_publish" / "pubsub_publish_no_echo"
            Ok(Some(format!(
                "{name}::<_, IpeError>({topic_s}, {payload_s})"
            )))
        }

        // Any is_ui/live/tui/webview/cli() variant not listed is a gap — hard error.
        _ => Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_ui_call",
            detail: format!(
                "UI/Web/Tui/WebView/Console kernel {k:?} has no emit arm — add it to emit_ui_call"
            ),
        }),
    }
}

/// Handle JSON / Db decoder kernel calls that require custom argument wrapping.
///
/// Returns `Some(emitted)` for the four special cases:
///
/// * **Arity-0 primitive decoders** (`JsonDecString/Int/Float/Bool`) — these
///   carry a free `E: From<String>` type parameter that Rust cannot infer when
///   passed to another polymorphic function (e.g. `decode_from_json_string`).
///   Emits with an explicit `IpeError` turbofish.
///
/// * **`JsonDecSucceed | DbDecSucceed`** applied to any argument — `decode_succeed`
///   expects a `Box<dyn Fn() -> A + Send>` FACTORY (not a raw value).
///   Three sub-cases:
///   1. Named N-arg function (`FuncValue`) → `decode_succeed(curry{n}(fn_name))`
///   2. Lambda with N params → `decode_succeed(curry{n}(move |p1: T1, …| -> R { body }))`
///   3. Any other value → `decode_succeed({ let __ipe_succeed = <arg>; Box::new(move || __ipe_succeed.clone()) })`
///
///   Cases 1+2 are fail-closed when N > 10 via [`LowerError::DecodeSucceedArityTooHigh`]
///   (no `curry11` exists in the runtime).
///
/// * **`JsonDecList`** — `decode_list` expects `impl Fn() -> Decoder<E, T> + Send`
///   (a factory) rather than the decoder value. Wraps the argument in a
///   `move` closure: `decode_list(move || { inner })`.
///
/// Returns `None` for all other `Expr::Call` shapes, which fall through to the
/// standard emitter.  Factored out of `emit_expr_at` to avoid inflating that
/// function's stack frame (the depth-guard test relies on a bounded frame size).
#[inline(never)]
fn emit_json_decoder_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    // ── Arity-0 primitives — turbofish IpeError ──────────────────────────────
    if args.is_empty()
        && matches!(
            callee,
            Callee::Kernel(
                ipe_ir::KernelFn::JsonDecString
                    | ipe_ir::KernelFn::JsonDecInt
                    | ipe_ir::KernelFn::JsonDecFloat
                    | ipe_ir::KernelFn::JsonDecBool
                    // `Config.{string,int,float,bool}` share the JSON primitive
                    // decoder fns — same arity-0 turbofish treatment.
                    | ipe_ir::KernelFn::ConfigString
                    | ipe_ir::KernelFn::ConfigInt
                    | ipe_ir::KernelFn::ConfigFloat
                    | ipe_ir::KernelFn::ConfigBool
            )
        )
    {
        let name = callee_name(ctx, callee)?;
        return Ok(Some(format!("{name}::<IpeError>()")));
    }
    // ── succeed(arg) — JsonDecSucceed / DbDecSucceed / ConfigSucceed share
    //    decode_succeed (Config over the same carrier).
    if matches!(
        callee,
        Callee::Kernel(KernelFn::JsonDecSucceed | KernelFn::DbDecSucceed | KernelFn::ConfigSucceed)
    ) && let Some(arg) = args.first()
    {
        match arg {
            // Case 1: named function (FuncValue) — curry{n}(fn_name)
            Expr::FuncValue {
                callee: fn_callee,
                ty: IrType::Fun(params, _),
            } if !params.is_empty() => {
                let n = params.len();
                if n > 10 {
                    return Err(Diagnostic::Lower {
                        span: Span::DUMMY,
                        msg: LowerError::DecodeSucceedArityTooHigh { n },
                    });
                }
                let fn_name = callee_name(ctx, fn_callee)?;
                return Ok(Some(format!("decode_succeed(curry{n}({fn_name}))")));
            }
            // Case 2: lambda — curry{n}(move |params| -> ret { body })
            Expr::Lambda { params, ret, body } if !params.is_empty() => {
                let n = params.len();
                if n > 10 {
                    return Err(Diagnostic::Lower {
                        span: Span::DUMMY,
                        msg: LowerError::DecodeSucceedArityTooHigh { n },
                    });
                }
                let closure = emit_lambda_unboxed(ctx, params, ret, body, indent, child, generics)?;
                return Ok(Some(format!("decode_succeed(curry{n}({closure}))")));
            }
            // Case 3: any other value — factory-wrap so it is called per run.
            // Turbofish `<IpeError, _>` pins the error type when there is no
            // surrounding pipeline to drive inference (E0283 otherwise).
            other => {
                let val = emit_expr_at(ctx, other, indent, child, generics)?;
                return Ok(Some(format!(
                    "decode_succeed::<IpeError, _>({{ let __ipe_succeed = {val}; Box::new(move || __ipe_succeed.clone()) }})"
                )));
            }
        }
    }
    // ── JsonDecList / ConfigList — wrap argument in factory closure ───────────
    // `decode_list` expects `impl Fn() -> Decoder<E, T>`; Config shares the fn.
    if matches!(
        callee,
        Callee::Kernel(ipe_ir::KernelFn::JsonDecList | ipe_ir::KernelFn::ConfigList)
    ) && let Some(inner) = args.first()
    {
        let inner_s = emit_expr_at(ctx, inner, indent, child, generics)?;
        return Ok(Some(format!("decode_list(move || {{ {inner_s} }})")));
    }
    // ── ConfigKeyValuePairs / ConfigDict — same factory-closure shape as
    // `decode_list`; both expect `impl Fn() -> Decoder<E, T>`.
    if let Callee::Kernel(
        k @ (ipe_ir::KernelFn::ConfigKeyValuePairs | ipe_ir::KernelFn::ConfigDict),
    ) = callee
        && let Some(inner) = args.first()
    {
        let inner_s = emit_expr_at(ctx, inner, indent, child, generics)?;
        let name = kernel_name(*k); // "decode_key_value_pairs" / "config_dict"
        return Ok(Some(format!("{name}(move || {{ {inner_s} }})")));
    }
    Ok(None)
}

/// Depth-tracked recursion behind [`emit_expr`]. `depth` is the IR-nesting level
/// of `expr` (0 at the function body); it gates the bounded-emit guard and is
/// independent of `indent` (the textual indentation of `match` arms).
///
/// `pub(crate)` so that `emit_web` can call it directly (Live kernel bodies
/// emit sub-expressions at the same depth level as their enclosing expression).
#[allow(clippy::too_many_lines)]
pub fn emit_expr_at(
    ctx: &EmitCtx,
    expr: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    if depth > MAX_EMIT_DEPTH {
        return Err(Diagnostic::Lower {
            span: Span::DUMMY,
            msg: LowerError::BackendNestingTooDeep {
                limit: MAX_EMIT_DEPTH,
            },
        });
    }
    let child = depth + 1;
    match expr {
        Expr::Int(n) => Ok(n.to_string()),
        // A float literal renders as an f64-typed Rust literal. A whole-number
        // value keeps its decimal point (`3.0`) so Rust never types it as an
        // integer; see [`float_literal`].
        Expr::Float(f) => Ok(float_literal(*f)),
        // A string literal renders as an owned `String` (Ipê `String` is Rust
        // `String`, never `&str`). The `{:?}` Debug form produces a valid Rust
        // string literal with deterministic escaping.
        Expr::Str(s) => Ok(format!("{s:?}.to_string()")),
        // A compile-time-validated `path "…"` literal. The string was already
        // validated and cleaned by the canonicaliser; emit a direct call to
        // `path_literal` (the compiler-only bypass constructor) so no runtime
        // re-validation is performed — the type is the proof.
        Expr::PathLit(s) => Ok(format!(
            "ipe_runtime::path::path_literal({s:?}.to_string())"
        )),
        // A character literal renders as a Rust `char`. The carried text is a
        // single character (lexer invariant); `{:?}` escapes it deterministically.
        // A malformed (non-single-char) value fails closed as a `CompilerBug`:
        // a string-literal fallback in `char` position is NOT a safe total
        // fallback — it emits Rust that `cargo` rejects (E0308), the exact
        // exit-0-then-cargo-fail shape THE SEAL forbids.
        Expr::Char(c) => {
            let mut chars = c.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Ok(format!("{ch:?}")),
                _ => Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_expr_at(Expr::Char)",
                    detail: format!(
                        "Expr::Char carried {} characters ({c:?}), not the single \
                         character the lexer's char-literal invariant guarantees",
                        c.chars().count()
                    ),
                }),
            }
        }
        // A boolean value renders as the Rust keyword constant.
        Expr::Bool(b) => Ok(if *b { "true" } else { "false" }.to_owned()),
        // The unit value renders as the Rust unit expression `()`.
        Expr::Unit => Ok("()".to_owned()),
        Expr::Var(sym) => ctx.emit_ident(*sym),
        Expr::CloneVar(sym) => Ok(format!("{}.clone()", ctx.emit_ident(*sym)?)),
        Expr::Ctor {
            home,
            ty,
            variant,
            args,
        } => emit_ctor(ctx, home, *ty, *variant, args, indent, depth, generics),
        Expr::BinOp { op, lhs, rhs } => {
            let l = emit_expr_at(ctx, lhs, indent, child, generics)?;
            let r = emit_expr_at(ctx, rhs, indent, child, generics)?;
            // Exhaustive match — no wildcard. Adding a new `BinOp` variant
            // without wiring it here is a compile error, not a silent gap.
            match op {
                // `++` (string append) has no Rust infix form for two owned
                // `String`s; `format!` borrows both via `Display` and yields a
                // fresh `String` — no ownership or clone obligation.
                BinOp::Append => Ok(format!("format!(\"{{}}{{}}\", {l}, {r})")),
                // `//` (integer division). Raw Rust `/` on `i64` panics on
                // `b == 0` AND on `i64::MIN / -1`; `//` is itself a Rust line
                // comment, so raw infix emit is doubly unsound. Route through
                // the total helper that matches Ipê-Go `rt.IntDiv` semantics:
                // b==0 → panic("attempt to divide by zero") (abort, exit 101);
                // i64::MIN / -1 → i64::MIN (wrapping, no abort).
                BinOp::IntDiv => Ok(format!("ipe_runtime::math::ipe_int_div({l}, {r})")),
                // Every remaining operator has a sound Rust infix form.
                BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Eq
                | BinOp::Neq
                | BinOp::Lt
                | BinOp::Gt
                | BinOp::Le
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or => Ok(format!("({} {} {})", l, op_str(*op), r)),
            }
        }
        Expr::Let { name, value, body } => {
            // A `let` expression renders as a parenthesised Rust block so it
            // composes inline anywhere an expression is expected:
            // `({ let <name> = <value>; <body> })`.
            //
            // `Vec<IpeTask<A>>` (a list of tasks) is non-Clone: using the binding
            // more than once causes E0382 "use of moved value" because the first
            // call moves the Vec.  Ipê has pure/immutable semantics so re-
            // evaluating the value at each use site is always correct — inline it
            // when the value is a task-containing list AND the body uses the name
            // more than once.  Plain Clone/Copy bindings (Int, Bool, records, …)
            // keep the let form so the compiler can share the computation.
            //
            // AUD-04: the multi-use count and the inline substitution both
            // operate on the IR (`scan_free_target` / `substitute_var`), not on
            // rendered Rust text — see those functions' doc comments for why
            // the old text-level passes could corrupt a string literal or a
            // record field name that happened to spell the same identifier.
            let (occurrences, has_clonevar) = scan_free_target(body, *name);
            let needs_inline = occurrences > 1 && expr_value_is_non_clone(value) && !has_clonevar;
            if needs_inline {
                let inlined_body = substitute_var((**body).clone(), *name, value);
                let inlined_s = emit_expr_at(ctx, &inlined_body, indent, child, generics)?;
                Ok(format!("({{ {inlined_s} }})"))
            } else {
                let name_s = ctx.emit_ident(*name)?;
                let value_s = emit_expr_at(ctx, value, indent, child, generics)?;
                let body_s = emit_expr_at(ctx, body, indent, child, generics)?;
                Ok(format!("({{ let {name_s} = {value_s}; {body_s} }})"))
            }
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            // An irrefutable destructuring binding renders as a parenthesised
            // Rust block, exactly like `Let`, but with a pattern binder:
            // `({ <binding stmts> <body> })`. The binder is irrefutable (the
            // lowerer guarantees it — vars / wildcards / tuples / aliases / a
            // top-level record), so the `let`s are exhaustive Rust.
            // `emit_binding_stmts` renders a bare binder as the single flat
            // `let <pat> = <value>;` and an aliased binder as the clone-split
            // sequence that closes the by-value partial-move (E0382) hole.
            let value = emit_expr_at(ctx, value, indent, child, generics)?;
            let stmts = emit_binding_stmts(ctx, binder, &value)?;
            let body = emit_expr_at(ctx, body, indent, child, generics)?;
            Ok(format!("({{ {} {body} }})", stmts.join(" ")))
        }
        Expr::If { cond, then_, else_ } => {
            // Parenthesised so the whole `if`/`else` is a single expression
            // value, independent of surrounding precedence.
            let cond = emit_expr_at(ctx, cond, indent, child, generics)?;
            let then_ = emit_expr_at(ctx, then_, indent, child, generics)?;
            let else_ = emit_expr_at(ctx, else_, indent, child, generics)?;
            Ok(format!("(if {cond} {{ {then_} }} else {{ {else_} }})"))
        }
        Expr::Call {
            callee,
            args,
            pin,
            on_form,
        } => {
            // Kernel-dispatch special cases apply ONLY to `Callee::Kernel` —
            // every probe below starts with a `let Callee::Kernel(..) = callee
            // else { return Ok(None) }` gate, so a plain user-function call
            // (`Callee::Func`) provably falls straight through all of them.
            // Gating once here skips eight non-inlined probe calls per
            // user-function call node (efficiency-audit §4 medium); kernel
            // calls still traverse the probes in the same order.
            if matches!(callee, Callee::Kernel(_)) {
                // JSON decoder kernel special cases are factored into a separate
                // `#[inline(never)]` helper to keep the `emit_expr_at` stack frame
                // small enough for the depth-guard test (IPE-L0200). The helper
                // returns `None` when no special case applies.
                if let Some(result) =
                    emit_json_decoder_call(ctx, callee, args, indent, child, generics)?
                {
                    return Ok(result);
                }
                // Http network kernel special cases: Http.get / Http.post /
                // Http.request need a task_map conversion closure (Design B).
                // Http.parseQuery falls through (standard path is correct).
                if let Some(result) = emit_http_call(ctx, callee, args, indent, child, generics)? {
                    return Ok(result);
                }
                // Http builder kernels: Http.defaultRequest / Http.withMethod /
                // Http.withTimeout / Http.withBody / Http.withHeader emit inline
                // struct construction or clone-and-reassign record updates.
                if let Some(result) =
                    emit_http_builder_call(ctx, callee, args, indent, child, generics)?
                {
                    return Ok(result);
                }
                // Task.RetryPolicy builders and Task.retryWith: inline struct
                // construction / move-update / runtime call.
                if let Some(result) =
                    emit_task_retry_call(ctx, callee, args, indent, child, generics)?
                {
                    return Ok(result);
                }
                // Db projection kernels: DbExec / DbQuery / DbQueryDecode /
                // DbInsertFields / DbUpdateFields / DbInsertFieldsReturning need
                // `List SqlValue` / `List (String, SqlField)` projected to
                // `Vec<SqlParam>` / `Vec<(String, Option<SqlParam>)>` at the call
                // site via the generated `into_sql_param` / `into_field_param` methods.
                if let Some(result) = emit_db_call(ctx, callee, args, indent, child, generics)? {
                    return Ok(result);
                }
                if let Some(result) = emit_tea_call(ctx, callee, args, indent, child, generics)? {
                    return Ok(result);
                }
                if let Some(result) = emit_server_call(ctx, callee, args, indent, child, generics)?
                {
                    return Ok(result);
                }
                // Ipe.Ui / Ipe.Html / Ipe.Web / Ipe.Tui / Ipe.WebView kernels.
                if let Some(result) =
                    emit_ui_call(ctx, callee, args, *on_form, indent, child, generics)?
                {
                    return Ok(result);
                }
                // `PubSub.topic : String -> Topic a` erases to the identity
                // function at runtime — `Topic a` lowers to `Str`, so the
                // call emits as the argument directly (no Rust runtime call needed).
                if matches!(callee, Callee::Kernel(KernelFn::PubSubTopic))
                    && let [name_arg] = args.as_slice()
                {
                    return emit_expr_at(ctx, name_arg, indent, child, generics);
                }
                // Dict.get borrows semantics: the runtime takes the HashMap by
                // value, but Ipê dicts are persistent — the same dict binding may
                // be passed to multiple Dict.get calls in one let-chain (e.g.
                // `let a = Dict.get "a" d; let b = Dict.get "b" d`).  Cloning the
                // dict arg before each call keeps the original binding alive and
                // avoids the "use of moved value" Rust compile error.
                if matches!(callee, Callee::Kernel(KernelFn::DictGet))
                    && let [key_arg, dict_arg] = args.as_slice()
                {
                    let key_s = emit_expr_at(ctx, key_arg, indent, child, generics)?;
                    let dict_s = emit_expr_at(ctx, dict_arg, indent, child, generics)?;
                    return Ok(format!("dict_get({key_s}, {dict_s}.clone())"));
                }
            }
            // A transparent-typed FFI call converts at the seam: arguments
            // the wrapper's glue map marks render as foreign struct/enum
            // constructions, and a glued result converts back to the
            // app-side record/union. Wrappers without glue fall through to
            // the generic tail unchanged.
            if let Callee::Ffi { ident } = callee
                && let Some(glue) = ctx.ffi_wrapper_glue(*ident)?
            {
                return emit_ffi_glued_call(ctx, *ident, glue, args, indent, child, generics);
            }
            let name = callee_name(ctx, callee)?;
            // a polymorphic-kernel turbofish the lowerer set because the
            // solver left this call's result type parameter genuinely
            // unconstrained (a discarded / empty / phantom position). Empty for
            // every other call — `CallPin::None::turbofish()` is `""` — so an
            // unpinned call emits no turbofish suffix. The
            // suffix goes between the kernel name and its `(` argument list:
            // `dict_empty::<String, i64>(…)`.
            let pin_turbofish = pin.turbofish();
            // `Ipe.Csv` parse kernels are generic over the error channel
            // (`csv_parse<E: From<String>>(...) -> IpeResult<E, CsvDoc>`); a
            // `Result`-returning call whose `Err` arm is often discarded leaves
            // `E` unconstrained (E0283). Anchor it to `IpeError`, mirroring the
            // network kernels (`http_get::<IpeError>`) and the arity-0 JSON
            // decoders. Only the `E`-free parse entries need it; `encode`
            // returns a bare `String` (no `E`).
            let turbofish: &str = if pin_turbofish.is_empty()
                && matches!(
                    callee,
                    Callee::Kernel(KernelFn::CsvParse | KernelFn::CsvParseWithDelimiter)
                ) {
                "::<IpeError>"
            } else {
                pin_turbofish
            };
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                parts.push(emit_expr_at(ctx, arg, indent, child, generics)?);
            }
            // A handful of Maybe/Result kernels take the container BEFORE the
            // function in the runtime (`ipe_maybe_map(m, f)`) whereas Ipê passes
            // the function first (`Maybe.map f m`). The lowerer keeps the Ipê
            // order; re-point the two arguments here so the runtime call is
            // well-formed.
            if matches!(callee, Callee::Kernel(k) if kernel_swaps_first_two(*k)) {
                parts.reverse();
            }
            Ok(format!("{name}{turbofish}({})", parts.join(", ")))
        }
        Expr::Tuple(elems) => {
            // A tuple constructor renders inline as `(e1, e2, ...)`. The IR
            // invariant guarantees arity ≥ 2, so this is always a genuine Rust
            // tuple; the emission stays total over any vector regardless.
            let mut parts = Vec::with_capacity(elems.len());
            for elem in elems {
                parts.push(emit_expr_at(ctx, elem, indent, child, generics)?);
            }
            Ok(format!("({})", parts.join(", ")))
        }
        Expr::List { elem, items } => emit_list(ctx, elem, items, indent, depth, generics),
        Expr::Cons { head, tail } => {
            // `head :: tail` renders through the runtime's move-only list prepend.
            let h = emit_expr_at(ctx, head, indent, child, generics)?;
            let t = emit_expr_at(ctx, tail, indent, child, generics)?;
            Ok(format!("ipe_runtime::list::ipe_list_cons({h}, {t})"))
        }
        Expr::ListIndexClone { list, index } => {
            // Clone the element at a constant index — the arm guard already
            // proved `list.len() > index`, so the Rust index is in
            // bounds by construction. `.clone()` keeps the list intact for the
            // sibling tail binder.
            let l = emit_expr_at(ctx, list, indent, child, generics)?;
            Ok(format!("({l})[{index}].clone()"))
        }
        Expr::ListLenCheck { list, len, exact } => {
            // Borrowing list-length guard. `.len()` never moves the
            // bound `Vec`, so this is legal in an arm-guard position.
            let l = emit_expr_at(ctx, list, indent, child, generics)?;
            let op = if *exact { "==" } else { ">=" };
            Ok(format!("({l}).len() {op} {len}"))
        }
        // The record arms own several `Vec`/`String` locals; keeping their
        // bodies in dedicated functions (not inlined into this match) holds
        // `emit_expr_at`'s own stack frame small, so the depth guard — not a
        // native overflow — is what bounds a deep `BinOp`/`Call` spine.
        Expr::Record(fields) => emit_record(ctx, fields, indent, depth, generics),
        Expr::Access {
            record,
            field,
            field_ty,
        } => {
            // Field access `<record>.<field>`. The base is parenthesised so a
            // record literal in record position (`{ ... }.field`) is never
            // misparsed; the field ident is keyword-mangled to match the struct.
            //
            // Type-directed Copy elision (AUD-09 — see
            // `docs/adr/0011-emitter-clone-borrow-discipline.md`
            // §3): Ipê is a purely-functional language with value semantics,
            // so every field read is logically a copy.  A field whose solved
            // type is UNCONDITIONALLY `Copy` in the emitted Rust (Int / Float
            // / Bool / Char / Unit / Order / Decimal / ErrorKind / the Copy
            // id-wrapper opaques) is read bare — the read IS the copy.  Every
            // other field (heap-backed String / Vec / synthesized structs /
            // generics) keeps `.clone()`: rustc does NOT elide a `.clone()`
            // call on a heap type, and the clone is what prevents partial-move
            // errors when the same owner or field is accessed more than once
            // (e.g. `view` and `update` both read `model.someField`).  The
            // audit's second half — last-use analysis to elide the clone on a
            // heap field's FINAL read — is explicitly deferred (spec §3.5).
            let base = emit_expr_at(ctx, record, indent, child, generics)?;
            let field = ctx.emit_ident(*field)?;
            if ir_type_is_definitely_copy(field_ty) {
                Ok(format!("({base}).{field}"))
            } else {
                Ok(format!("({base}).{field}.clone()"))
            }
        }
        Expr::Update { record, fields } => {
            emit_update(ctx, record, fields, indent, depth, generics)
        }
        Expr::Lambda { params, ret, body } => {
            emit_lambda(ctx, params, ret, body, indent, depth, generics)
        }
        Expr::SharedLambda { params, ret, body } => {
            emit_shared_lambda(ctx, params, ret, body, indent, depth, generics)
        }
        Expr::Apply { func, args } => emit_apply(ctx, func, args, indent, depth, generics),
        Expr::FuncValue { callee, ty } => emit_func_value(ctx, callee, ty, generics),
        Expr::Match(m) => emit_match(ctx, m, indent, depth, generics),
        // F1 (auto-force): a discarded Task binding becomes
        //   task_and_then(<effect>, Box::new(move |_| { <rest> }))
        // so the future is properly awaited rather than silently dropped.
        //
        // ARGUMENT ORDER: the runtime `task_and_then(task, f)` takes the effect
        // FIRST and the continuation SECOND. Rust evaluates function arguments
        // left-to-right, so the effect expression is evaluated before the
        // continuation closure is constructed. This matters when the same `Db`
        // pool handle is used in both the effect (e.g. `db_exec_raw(conn, ...)`)
        // and the continuation (`move |_| { ... conn ... }`): placing the effect
        // first lets the continuation capture the pool handle by move without a
        // double-move error, provided that Db kernels emit `conn.clone()` for the
        // pool argument (see `emit_db_call`).
        //
        // The closure parameter type and return type are inferred by Rust from
        // the task_and_then signature — `effect_s: IpeTask<A>` pins A (the
        // discarded type) and `rest_s: IpeTask<B>` pins B (the result type),
        // avoiding the incorrect hardcoded `()` that would fail for any non-unit
        // effect type or non-unit rest type.
        Expr::TaskSeq { effect, rest } => {
            let child = depth + 1;
            // Clone any identifier that `rest` (the move-closure continuation)
            // would capture but `effect` already moves.  Rust evaluates function
            // args left-to-right, so a String/record passed by value into
            // `effect_s` is moved before the closure in the second argument is
            // constructed.
            //
            // AUD-04: this rewrite runs on the IR, BEFORE `effect` is emitted to
            // text — `free_vars`/`clone_targets_in_expr` only ever touch genuine
            // `Var` nodes, so a captured-variable word inside a string literal or
            // a record field name in `effect` can never be corrupted (the prior
            // text-level `clone_captured_vars` pass matched on rendered source
            // and could rewrite either).
            let rest_captures = free_vars(rest);
            let effect_rw = clone_targets_in_expr((**effect).clone(), &rest_captures);
            let effect_s = emit_expr_at(ctx, &effect_rw, indent, child, generics)?;
            let rest_s = emit_expr_at(ctx, rest, indent, child, generics)?;
            Ok(format!(
                "task_and_then({effect_s}, Box::new(move |_| {{ {rest_s} }}))"
            ))
        }
        // Sync variant of TaskSeq: blocks on `effect` (discarding the result),
        // then evaluates `rest` in the same sync context. Used when a
        // `let _ = <task>` binding appears inside a non-Task (sync) function,
        // e.g. a helper that returns Vec<Row> or () but still wants to fire a
        // logging side-effect. `task_run` is the blocking scheduler entry point
        // in ipe_runtime (`pub fn task_run<E,A>(task: IpeTask<E,A>) -> IpeResult<E,A>`).
        //
        // AUD-04: `effect` and `rest` share ONE scope here (no closure), but
        // `effect`'s own evaluation can still move a variable `rest` needs next
        // (`let _ = Io.writeStdout msg in msg` moves `msg` into `writeStdout`,
        // then `rest` reads it again) — the same left-to-right move hazard as
        // `TaskSeq`, so it gets the identical IR-level clone-capture rewrite.
        // Pre-AUD-04 this arm had NO clone-capture handling at all.
        Expr::TaskSeqSync { effect, rest } => {
            let child = depth + 1;
            let rest_captures = free_vars(rest);
            let effect_rw = clone_targets_in_expr((**effect).clone(), &rest_captures);
            let effect_s = emit_expr_at(ctx, &effect_rw, indent, child, generics)?;
            let rest_s = emit_expr_at(ctx, rest, indent, child, generics)?;
            Ok(format!("{{ let _ = task_run({effect_s}); {rest_s} }}"))
        }
        // TCO nodes are produced by the lowerer's rewrite and consumed by
        // `emit_func` / `emit_expr_tail`; reaching one on the ordinary value-emit
        // path means the rewrite left a jump/loop outside a tail context — a
        // compiler bug, surfaced fail-closed (never a panic, never a wildcard).
        Expr::TailLoop { .. } | Expr::TailRecur { .. } => Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_expr_at",
            detail: "TailLoop/TailRecur reached the non-tail emit path".to_string(),
        }),
    }
}

/// Emit a list literal. A non-empty list renders as `vec![e0, e1, …]`; the empty
/// list as a typed `Vec::<T>::new()` so its element type is never ambiguous (a
/// bare `vec![]` could fail to infer in a polymorphic position). Kept out of the
/// `emit_expr_at` match (`#[inline(never)]`) so its locals don't inflate the
/// recursive frame.
#[inline(never)]
fn emit_list(
    ctx: &EmitCtx,
    elem: &IrType,
    items: &[Expr],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    if items.is_empty() {
        // For `IrType::Json` (unresolved / wildcard `Value = any`) the type
        // annotation `Vec::<JsonVal>::new()` CONFLICTS with callers that expect
        // `Vec<Attribute<M>>` / `Vec<Element<M>>` etc. — Rust's type checker
        // rejects the explicit annotation even though it would accept an
        // unannotated `Vec::new()` via inference.  Emit the bare form and let
        // Rust infer the element type from the surrounding call's expected type.
        // All other element types are precise enough that an explicit annotation
        // resolves ambiguity without breaking callers.
        if matches!(elem, IrType::Json) {
            return Ok("Vec::new()".to_owned());
        }
        let ty = render_type(ctx, elem, generics)?;
        return Ok(format!("Vec::<{ty}>::new()"));
    }
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        parts.push(emit_expr_at(ctx, item, indent, child, generics)?);
    }
    // Non-empty lists whose element type is a parametric Ui type (`Attribute<M>`
    // / `Element<M>` / `Html<M>` / …) need an explicit type annotation on the
    // emitted Rust `Vec` so that the `M` type parameter can be inferred by the
    // Rust compiler.  Without this, callers like `Ui.layoutWith` whose attrs
    // lists are always non-empty (no empty-list turbofish to anchor M) produce
    // E0283 because every helper (`ui_padding_`, `ui_spacing_`, …) is itself
    // generic in M and no concrete M appears elsewhere in the expression.
    //
    // The annotation wraps the vec in a typed `let` block:
    //   `{ let __ipe_m: Vec<Attribute<()>> = vec![ui_padding_(12)]; __ipe_m }`
    // The variable name `__ipe_m` is scoped to the anonymous block and cannot
    // shadow user-visible bindings.  The block is a Rust expression, valid in
    // every argument position.
    //
    // This path is skipped for `IrType::Json` (the elem type is unresolved)
    // because annotating with `Vec<JsonVal>` would CONFLICT with callers that
    // expect `Vec<Attribute<M>>` — the same reason empty Json lists emit bare
    // `Vec::new()` rather than a typed form.
    if matches!(elem, IrType::Ui { .. }) {
        let ty = render_type(ctx, elem, generics)?;
        return Ok(format!(
            "{{ let __ipe_m: Vec<{ty}> = vec![{}]; __ipe_m }}",
            parts.join(", ")
        ));
    }
    Ok(format!("vec![{}]", parts.join(", ")))
}

/// Emit a constructor application. A nullary constructor renders as the bare
/// path `EnumName::Variant`; a payload constructor renders
/// `EnumName::Variant(arg0, arg1, …)`. A payload position on a type-size cycle
/// back to its own enum is wrapped in `Box::new(…)` to balance the boxed enum
/// field (see [`crate::EmitCtx::is_cyclic_self_field`]). Kept out of the
/// `emit_expr_at` match (`#[inline(never)]`) so its locals don't inflate the
/// recursive frame.
#[inline(never)]
// The extra `home` param is the type's nominal-identity half `(home, ty)`;
// splitting the ctor emitter would obscure the boxing/runtime-enum flow.
#[allow(clippy::too_many_arguments)]
fn emit_ctor(
    ctx: &EmitCtx,
    home: &ModPath,
    ty: Symbol,
    variant: Symbol,
    args: &[Expr],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    // A built-in `Maybe` / `Result` constructor routes to the runtime enum
    // (`IpeMaybe::Just(..)`, `IpeResult::Err(..)`); its payload is never a
    // self-recursive user field, so no field-boxing lookup applies.
    if let Some(runtime) = ctx.builtin_runtime_enum(home, ty) {
        let path = format!("{runtime}::{}", ctx.emit_ident(variant)?);
        if args.is_empty() {
            return Ok(path);
        }
        let mut parts = Vec::with_capacity(args.len());
        for arg in args {
            parts.push(emit_expr_at(ctx, arg, indent, child, generics)?);
        }
        return Ok(format!("{path}({})", parts.join(", ")));
    }
    let path = format!("{}::{}", ctx.enum_name(home, ty)?, ctx.emit_ident(variant)?);
    if args.is_empty() {
        return Ok(path);
    }
    let fields = ctx.variant_fields(home, ty, variant)?;
    if fields.len() != args.len() {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_ctor",
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
    let mut parts = Vec::with_capacity(args.len());
    for (arg, field_ty) in args.iter().zip(fields.iter()) {
        let rendered = emit_expr_at(ctx, arg, indent, child, generics)?;
        // A cyclic self-edge field is boxed in the enum, so its construction
        // argument is boxed too.
        if ctx.is_cyclic_self_field(field_ty, home, ty) {
            parts.push(format!("Box::new({rendered})"));
        } else {
            parts.push(rendered);
        }
    }
    Ok(format!("{path}({})", parts.join(", ")))
}

/// Emit a `match`. An arm head is a constructor pattern (exhaustive
/// over the enum's variants) or — for a flat refutable match — a literal
/// (`0` / `'a'` / `"hi"` / `true` / `false`), a wildcard / variable binder, or
/// an alias. A cyclic self-edge constructor payload field is boxed in the enum,
/// so a variable bound to such a field is unboxed (`let x = *x;`) at the top of
/// the arm body, giving the binder the enum's own (owned) type rather than
/// `Box<…>`.
///
/// `String` scrutinees match against `scrut.as_str()` because Rust string
/// literal patterns are `&str`; any top-level binder in such an arm is rebound
/// to an owned `String` (`let name = name.to_string();`) so the arm body sees
/// the Ipê `String` type, keeping the lowering sound. Kept out of the
/// `emit_expr_at` match (`#[inline(never)]`) for the same frame-size reason as
/// the neighbouring helpers.
#[inline(never)]
fn emit_match(
    ctx: &EmitCtx,
    m: &Match,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let (scrut, mode) = emit_match_scrutinee(ctx, m, indent, depth, generics)?;
    let arm_indent = indent_of(indent + 1);
    let close_indent = indent_of(indent);
    let mut arms = Vec::with_capacity(m.arms().len());
    for arm in m.arms() {
        let (pat, prelude, synth_guard) = emit_arm_head(ctx, &arm.pat, &mode)?;
        let body = emit_expr_at(ctx, &arm.body, indent + 1, child, generics)?;
        let arm_body = if prelude.is_empty() {
            body
        } else {
            format!("{{ {prelude}{body} }}")
        };
        // A guard (a list-length arm guard and/or the synthesized `as_str()`
        // string-column guard) renders as a native Rust `if <guard>` on the arm —
        // `false` falls through to the next arm, matching the `case`'s refutable
        // semantics. When both are present they are ANDed. A guardless arm keeps
        // the plain `{pat} => …` shape.
        let ir_guard = match &arm.guard {
            Some(g) => Some(emit_expr_at(ctx, g, indent + 1, child, generics)?),
            None => None,
        };
        match combine_guards(synth_guard, ir_guard) {
            Some(guard) => arms.push(format!("{arm_indent}{pat} if {guard} => {arm_body},")),
            None => arms.push(format!("{arm_indent}{pat} => {arm_body},")),
        }
    }
    Ok(format!(
        "match {scrut} {{\n{}\n{close_indent}}}",
        arms.join("\n")
    ))
}

/// Emit the scrutinee of a `Match` plus its two mode flags. A string scrutinee is
/// matched as `&str` (so literal patterns apply) — the presence of a `Pat::Str`
/// head is the reliable signal (the type checker proved the scrutinee a
/// `String`). A LIST scrutinee (the runtime's `Vec<T>`) is matched as a slice so
/// the native Rust slice patterns `[]` / `[a, b]` / `[x, rest @ ..]` apply — a
/// `Pat::Slice` head is the signal. Shared by the value-context (`emit_match`)
/// and tail-context (`emit_expr_tail`) match emitters so the two agree exactly.
/// How a `match` scrutinee is coerced for pattern matching. A WHOLE scrutinee is
/// matched as `&str` (string `case`) or `&[T]` (list `case`) or as-is; a TUPLE
/// scrutinee (a multi-arm product `case`) is matched column-by-column, each
/// column carrying its own string / list coercion.
pub enum ScrutMode {
    Whole { str_mode: bool, list_mode: bool },
    Tuple(Vec<ColMode>),
}

/// The per-column coercion flags of a tuple-scrutinee `match`. A column is
/// matched as `&[T]` when some arm slices it (`… , x :: xs , …`) and as `&str`
/// when some arm matches it against a string literal.
#[derive(Clone, Copy)]
pub struct ColMode {
    str_mode: bool,
    list_mode: bool,
}

/// The arity of a tuple-scrutinee `match` — the element count of the first arm
/// whose head is a [`Pat::Tuple`], or `None` when no arm is a tuple pattern (the
/// whole-scrutinee shapes). The lowerer only builds a tuple-headed arm from a
/// literal-tuple scrutinee of the SAME arity, so this drives the tuple path.
fn tuple_arm_arity(arms: &[Arm]) -> Option<usize> {
    arms.iter().find_map(|a| match &a.pat {
        Pat::Tuple(elems) => Some(elems.len()),
        _ => None,
    })
}

/// Compute the per-column coercion flags of a tuple-scrutinee `match`: a column
/// is in list mode when some arm slices it, and in string mode when some arm
/// matches it against a string literal. (A column is never both — the scrutinee
/// element has a single type the checker pinned.)
fn tuple_col_modes(arms: &[Arm], arity: usize) -> Vec<ColMode> {
    let mut cols = vec![
        ColMode {
            str_mode: false,
            list_mode: false,
        };
        arity
    ];
    for arm in arms {
        if let Pat::Tuple(elems) = &arm.pat {
            for (c, sub) in elems.iter().enumerate() {
                if let Some(col) = cols.get_mut(c) {
                    if matches!(sub, Pat::Str(_)) {
                        col.str_mode = true;
                    }
                    if matches!(sub, Pat::Slice { .. }) {
                        col.list_mode = true;
                    }
                }
            }
        }
    }
    cols
}

pub fn emit_match_scrutinee(
    ctx: &EmitCtx,
    m: &Match,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<(String, ScrutMode)> {
    let child = depth + 1;
    // COERCED-COLUMN TUPLE mode: a multi-arm product `case` on a LITERAL-tuple
    // scrutinee. The scrutinee is built column-by-column with each column's own
    // slice / `&str` coercion — the only sound way to match `[a, rest @ ..]`
    // (needs `&[T]`) against a `Vec` element, or a string literal against a
    // `String` element. A NON-literal scrutinee whose arms are still tuple heads
    // (`case pair of (_, Passed) -> …`) carries no coercing column — the lowerer's
    // `tuple_case_supported` fail-closes any such column on the non-literal path —
    // so it falls through to WHOLE mode below, which matches the tuple value
    // directly (`match pair { (_, Passed) => … }`) via the alias-safe renderer.
    if let Some(arity) = tuple_arm_arity(m.arms())
        && let Expr::Tuple(elems) = m.scrutinee()
    {
        if elems.len() != arity {
            return Err(Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::emit_match_scrutinee",
                detail: format!(
                    "tuple match scrutinee has {} elements but arms have arity {arity}",
                    elems.len()
                ),
            });
        }
        let cols = tuple_col_modes(m.arms(), arity);
        let mut parts = Vec::with_capacity(arity);
        for (elem, col) in elems.iter().zip(&cols) {
            let e = emit_expr_at(ctx, elem, indent, child, generics)?;
            let e = if col.str_mode {
                format!("({e}).as_str()")
            } else if col.list_mode {
                format!("({e}).as_slice()")
            } else {
                e
            };
            parts.push(e);
        }
        return Ok((format!("({})", parts.join(", ")), ScrutMode::Tuple(cols)));
    }

    let scrut_expr = emit_expr_at(ctx, m.scrutinee(), indent, child, generics)?;
    let str_mode = m.arms().iter().any(|a| matches!(a.pat, Pat::Str(_)));
    let list_mode = m.arms().iter().any(|a| matches!(a.pat, Pat::Slice { .. }));
    let scrut = if str_mode {
        format!("({scrut_expr}).as_str()")
    } else if list_mode {
        format!("({scrut_expr}).as_slice()")
    } else {
        scrut_expr
    };
    Ok((
        scrut,
        ScrutMode::Whole {
            str_mode,
            list_mode,
        },
    ))
}

/// Render one match-arm head to its Rust pattern plus any leading rebind/unbox
/// prelude. A constructor head goes through `emit_ctor_arm_pat` (which unboxes a
/// cyclic self-field binder); a flat-match leaf head — literal / wildcard /
/// variable / alias / slice — goes through `render_pat` (total over the whole
/// set), with a `String`/slice binder rebind prelude in string/list mode. Shared
/// by the value-context and tail-context match emitters.
/// AND together the two guard sources on a match arm: the synthesized
/// string-column `as_str()` guard and the arm's own IR guard. Either,
/// both, or neither may be present; both present are joined `synth && ir` (the
/// synthesized `as_str()` checks come from the pattern, so they read first).
/// `None` when neither is present, leaving the arm's `=> …` shape guardless.
pub fn combine_guards(synth: Option<String>, ir: Option<String>) -> Option<String> {
    match (synth, ir) {
        (Some(s), Some(i)) => Some(format!("{s} && {i}")),
        (Some(s), None) => Some(s),
        (None, Some(i)) => Some(i),
        (None, None) => None,
    }
}

/// Render one arm head to its Rust pattern, any leading prelude, and any
/// synthesized match guard (the `__sgN.as_str() == "lit"` check for a
/// by-value string-literal column, joined with `&&` when several columns carry
/// one). `None` when no guard is synthesized, so the
/// caller's `if <guard>` clause stays absent.
pub fn emit_arm_head(
    ctx: &EmitCtx,
    pat: &Pat,
    mode: &ScrutMode,
) -> DResult<(String, String, Option<String>)> {
    let (rendered, prelude, guards) = match mode {
        ScrutMode::Whole {
            str_mode,
            list_mode,
        } => emit_whole_arm_head(ctx, pat, *str_mode, *list_mode)?,
        ScrutMode::Tuple(cols) => emit_tuple_arm_head(ctx, pat, cols)?,
    };
    let guard = if guards.is_empty() {
        None
    } else {
        Some(guards.join(" && "))
    };
    Ok((rendered, prelude, guard))
}

/// Render a WHOLE-scrutinee arm head (the string / list / plain shapes) to its
/// Rust pattern, any leading binder-rebind/unbox prelude, and any synthesized
/// match GUARDS. The guards are the `__sgN.as_str() == "lit"` checks for a
/// by-value string-literal column — the caller ANDs them onto the arm; they are
/// empty for every other shape (so existing emission is byte-identical).
fn emit_whole_arm_head(
    ctx: &EmitCtx,
    pat: &Pat,
    str_mode: bool,
    list_mode: bool,
) -> DResult<(String, String, Vec<String>)> {
    if let Pat::Ctor {
        home,
        ty,
        variant,
        args,
    } = pat
    {
        emit_ctor_arm_pat(ctx, home, *ty, *variant, args)
    } else if str_mode || list_mode {
        // STR/LIST mode: the scrutinee IS a reference (`.as_str()` /
        // `.as_slice()`), so `render_pat`'s `name @ inner` is a borrow and
        // sound for any inner shape. A top-level `Pat::Str`
        // matches the `&str`-wrapped scrutinee directly (a literal pattern), so no
        // guard is synthesized here.
        let prelude = if str_mode {
            str_binder_rebinds(ctx, pat)?
        } else {
            list_binder_rebinds(ctx, pat)?
        };
        Ok((render_pat(ctx, pat)?, prelude, Vec::new()))
    } else {
        // WHOLE mode, by value: a top-level dispatch-free alias head
        // (`(a, b) as w ->`) takes the alias-safe clone-rebuild path; a
        // by-value string-literal column (`( "transform", v )` on a variable
        // tuple scrutinee) accumulates its `as_str()` guard here.
        let mut alias_counter: usize = 0;
        let mut prelude = String::new();
        let mut guards = Vec::new();
        let rendered =
            render_arm_pat_alias_safe(ctx, pat, &mut alias_counter, &mut prelude, &mut guards)?;
        Ok((rendered, prelude, guards))
    }
}

/// Render a TUPLE-scrutinee arm head — a `(c0, c1, …)` tuple pattern or a `_`
/// catch-all — plus any per-column binder-rebind prelude. Each column renders
/// against its own coercion: a list column's binders rebind from `&T` / `&[T]`
/// to owned `T` / `Vec<T>`; a string column's binders rebind from `&str` to
/// `String`; a constructor column reuses the whole-scrutinee constructor path
/// (so a cyclic self-edge payload binder is unboxed). The lowerer only produces
/// a tuple or wildcard head here (`tuple_case_supported`), so a whole-value
/// variable / alias binder — which would see the wrong per-column-coerced type —
/// is an internal invariant violation, surfaced as a `CompilerBug`.
fn emit_tuple_arm_head(
    ctx: &EmitCtx,
    pat: &Pat,
    cols: &[ColMode],
) -> DResult<(String, String, Vec<String>)> {
    match pat {
        Pat::Tuple(elems) => {
            let mut rendered = Vec::with_capacity(elems.len());
            let mut prelude = String::new();
            let mut guards = Vec::new();
            for (c, sub) in elems.iter().enumerate() {
                // `unwrap_or` on a missing column would silently coerce a
                // wider-than-known tuple pattern to `str_mode: false,
                // list_mode: false` — the WRONG per-column coercion emits a
                // binder of the wrong type, an exit-0-then-cargo-fail (E0308)
                // THE SEAL forbids. Fail closed instead: this is the same
                // "lowerer only produces columns it schemed" invariant the
                // wildcard/tuple-only match arm below already enforces.
                let col = cols.get(c).copied().ok_or_else(|| {
                    let found = cols.len();
                    Diagnostic::CompilerBug {
                        where_: "ipe_backend_rust::emit_tuple_arm_head",
                        detail: format!(
                            "tuple-scrutinee match arm has {found} column(s) but the pattern \
                             reached column {c}; the lowerer's column table drifted from the \
                             pattern width"
                        ),
                    }
                })?;
                let (rp, pre, gs) = emit_whole_arm_head(ctx, sub, col.str_mode, col.list_mode)?;
                rendered.push(rp);
                prelude.push_str(&pre);
                guards.extend(gs);
            }
            Ok((format!("({})", rendered.join(", ")), prelude, guards))
        }
        Pat::Wildcard => Ok(("_".to_owned(), String::new(), Vec::new())),
        _ => Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_tuple_arm_head",
            detail: "tuple-scrutinee match arm head is neither a tuple nor a wildcard".to_owned(),
        }),
    }
}

/// Render a constructor arm head to its Rust pattern plus any leading unbox
/// statements. A cyclic self-edge payload field is boxed in the enum, so a
/// variable bound to it is unboxed (`let x = *x;`) at the arm body's head.
fn emit_ctor_arm_pat(
    ctx: &EmitCtx,
    home: &ModPath,
    ty: Symbol,
    variant: Symbol,
    args: &[Pat],
) -> DResult<(String, String, Vec<String>)> {
    // A built-in `Maybe` / `Result` pattern matches the runtime enum; its
    // payload is never a boxed self-edge field, so no unbox prelude is needed.
    if let Some(runtime) = ctx.builtin_runtime_enum(home, ty) {
        let path = format!("{runtime}::{}", ctx.emit_ident(variant)?);
        if args.is_empty() {
            return Ok((path, String::new(), Vec::new()));
        }
        // the concrete repro (`Just ((a, b) as w)`) lives HERE — a
        // builtin `Maybe`/`Result` payload matched by value. Route through
        // the alias-safe renderer; alias-free payloads are byte-identical. A
        // by-value string-literal payload (`Just "x"` on a `Maybe String`
        // scrutinee) accumulates its `as_str()` guard in `guards`.
        let mut alias_counter: usize = 0;
        let mut alias_prelude = String::new();
        let mut guards = Vec::new();
        let mut sub_pats = Vec::with_capacity(args.len());
        for sub in args {
            sub_pats.push(render_arm_pat_alias_safe(
                ctx,
                sub,
                &mut alias_counter,
                &mut alias_prelude,
                &mut guards,
            )?);
        }
        return Ok((
            format!("{path}({})", sub_pats.join(", ")),
            alias_prelude,
            guards,
        ));
    }
    let path = format!("{}::{}", ctx.enum_name(home, ty)?, ctx.emit_ident(variant)?);
    if args.is_empty() {
        return Ok((path, String::new(), Vec::new()));
    }
    let fields = ctx.variant_fields(home, ty, variant)?;
    if fields.len() != args.len() {
        return Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::emit_match",
            detail: format!(
                "constructor pattern {} of enum {} binds {} sub-patterns but the \
                 variant declares {} fields",
                variant.as_raw(),
                ty.as_raw(),
                args.len(),
                fields.len()
            ),
        });
    }
    let mut sub_pats = Vec::with_capacity(args.len());
    let mut unbox_lines = String::new();
    let mut guards = Vec::new();
    // a dispatch-free `as`-alias in a by-value ctor payload renders via
    // the alias-safe clone-rebuild path; its re-derivation `let`s share the
    // arm's existing prelude slot. Alias-free sub-patterns take the
    // byte-identical `render_pat` fast path inside.
    let mut alias_counter: usize = 0;
    for (sub, field_ty) in args.iter().zip(fields.iter()) {
        let self_edge = ctx.is_cyclic_self_field(field_ty, home, ty);
        // self-edge fix: an ALIAS over a cyclic-self-edge (recursive)
        // field is boxed in the enum (`Box<Self>`), so the clone-rebuild
        // path must re-derive its binders from the UNBOXED temp — otherwise
        // both the alias binder and the inner bindings stay `Box<T>` where
        // `T` is required (ipe-0-then-cargo-E0308). Bind the field to a fresh
        // raw temp, then re-derive the whole alias shape via the
        // `emit_binding_stmts` machinery against `*temp`.
        if self_edge && pat_contains_alias_in_arm(sub) {
            let temp = format!("__ipe_selfedge_alias_{alias_counter}");
            alias_counter += 1;
            for stmt in emit_binding_stmts(ctx, sub, &format!("*{temp}"))? {
                unbox_lines.push_str(&stmt);
                unbox_lines.push(' ');
            }
            sub_pats.push(temp);
            continue;
        }
        sub_pats.push(render_arm_pat_alias_safe(
            ctx,
            sub,
            &mut alias_counter,
            &mut unbox_lines,
            &mut guards,
        )?);
        // A variable bound to a boxed self-edge field is unboxed so the body
        // sees the payload's own type, not `Box<…>`.
        if self_edge && let Pat::Var(s) = sub {
            let binder = ctx.emit_ident(*s)?;
            write!(unbox_lines, "let {binder} = *{binder}; ").map_err(|e| {
                Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_match",
                    detail: format!("writing unbox binder failed: {e}"),
                }
            })?;
        }
    }
    Ok((
        format!("{path}({})", sub_pats.join(", ")),
        unbox_lines,
        guards,
    ))
}

/// Build the `let name = name.to_string();` prelude that rebinds every top-level
/// binder a string-match arm introduces from `&str` to an owned `String`, so the
/// arm body sees the Ipê `String` type. A variable binds itself; an alias binds
/// its name and recurses into its inner pattern; a wildcard / literal binds
/// nothing.
fn str_binder_rebinds(ctx: &EmitCtx, pat: &Pat) -> DResult<String> {
    let mut out = String::new();
    collect_str_rebinds(ctx, pat, &mut out)?;
    Ok(out)
}

fn collect_str_rebinds(ctx: &EmitCtx, pat: &Pat, out: &mut String) -> DResult<()> {
    match pat {
        Pat::Var(s) => {
            let name = ctx.emit_ident(*s)?;
            write!(out, "let {name} = {name}.to_string(); ").map_err(|e| {
                Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::str_binder_rebinds",
                    detail: format!("writing rebind binder failed: {e}"),
                }
            })?;
            Ok(())
        }
        Pat::Alias(inner, name) => {
            let n = ctx.emit_ident(*name)?;
            write!(out, "let {n} = {n}.to_string(); ").map_err(|e| Diagnostic::CompilerBug {
                where_: "ipe_backend_rust::str_binder_rebinds",
                detail: format!("writing rebind binder failed: {e}"),
            })?;
            collect_str_rebinds(ctx, inner, out)
        }
        // A string scrutinee admits no constructor / tuple / record / non-string
        // literal head (the type checker proves the scrutinee a `String`); these
        // introduce no `String`-typed binder to rebind.
        Pat::Wildcard
        | Pat::Str(_)
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Ctor { .. }
        | Pat::Tuple(_)
        | Pat::Record(_)
        | Pat::Slice { .. } => Ok(()),
        // Every alternative of an or-pattern binds the same names at the same
        // types, so rebinding via the first alternative produces the one correct
        // set of `let` rebinds (one per name, never per-alternative).
        Pat::Or(alts) => alts
            .first()
            .map_or(Ok(()), |first| collect_str_rebinds(ctx, first, out)),
    }
}

/// In LIST mode the scrutinee is matched as a slice (`(v).as_slice()`), so every
/// binder a list arm introduces is a borrow: an ELEMENT binder is `&T` and a
/// REST / whole-list binder is `&[T]`. This builds the `let … = …;` prelude that
/// rebinds each to the owned Ipê value the arm body expects — an element via
/// `.clone()` (so the body sees `T`), a rest / whole list via `.to_vec()` (so the
/// body sees `Vec<T>`). Cloning is the sound owned destructure of a shared slice;
/// the lowerer gates a list `case` binding a still-generic (non-`Clone`) element
/// type (IPE-L0102), so the `.clone()` / `.to_vec()` always resolve.
fn list_binder_rebinds(ctx: &EmitCtx, pat: &Pat) -> DResult<String> {
    let mut out = String::new();
    match pat {
        Pat::Slice { prefix, rest } => {
            for sub in prefix {
                collect_elem_rebinds(ctx, sub, &mut out)?;
            }
            if let Some(r) = rest {
                collect_list_rebinds(ctx, r, &mut out)?;
            }
        }
        // A whole-list catch-all binder (`xs ->`) or an alias over a list arm
        // (`(x :: rest) as whole ->`): the matched value IS the list.
        Pat::Var(_) => collect_list_rebinds(ctx, pat, &mut out)?,
        Pat::Alias(inner, name) => {
            rebind_to_vec(ctx, *name, &mut out)?;
            out.push_str(&list_binder_rebinds(ctx, inner)?);
        }
        // A wildcard binds nothing; other heads never reach a list `case`.
        Pat::Wildcard
        | Pat::Str(_)
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Ctor { .. }
        | Pat::Tuple(_)
        | Pat::Record(_) => {}
        // Every alternative binds the same names at the same types, so the first
        // alternative's rebinds are the whole arm's set.
        Pat::Or(alts) => {
            if let Some(first) = alts.first() {
                out.push_str(&list_binder_rebinds(ctx, first)?);
            }
        }
    }
    Ok(out)
}

/// Collect the owned-by-`clone` rebinds for an ELEMENT sub-pattern (a head
/// position of a slice). Every variable / alias binder there is `&T` and is
/// cloned to `T`; nested tuple / constructor / record element patterns recurse.
fn collect_elem_rebinds(ctx: &EmitCtx, pat: &Pat, out: &mut String) -> DResult<()> {
    match pat {
        Pat::Var(s) => rebind_clone(ctx, *s, out),
        Pat::Alias(inner, name) => {
            rebind_clone(ctx, *name, out)?;
            collect_elem_rebinds(ctx, inner, out)
        }
        Pat::Tuple(subs) => {
            for sub in subs {
                collect_elem_rebinds(ctx, sub, out)?;
            }
            Ok(())
        }
        Pat::Ctor { args, .. } => {
            for sub in args {
                collect_elem_rebinds(ctx, sub, out)?;
            }
            Ok(())
        }
        Pat::Record(fields) => {
            for (_, sub) in fields {
                collect_elem_rebinds(ctx, sub, out)?;
            }
            Ok(())
        }
        // A wildcard / literal element binds nothing. A nested slice element is
        // gated at lowering (it never reaches the backend), so it needs no rebind.
        Pat::Wildcard
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Slice { .. } => Ok(()),
        // Every alternative binds the same names at the same types; rebind via
        // the first alternative.
        Pat::Or(alts) => alts
            .first()
            .map_or(Ok(()), |first| collect_elem_rebinds(ctx, first, out)),
    }
}

/// Collect the owned-by-`to_vec` rebinds for a REST / whole-list binder (`&[T]`
/// → `Vec<T>`). The lowerer admits only a variable / wildcard rest, so this is a
/// single binder (an alias recurses defensively).
fn collect_list_rebinds(ctx: &EmitCtx, pat: &Pat, out: &mut String) -> DResult<()> {
    match pat {
        Pat::Var(s) => rebind_to_vec(ctx, *s, out),
        Pat::Alias(inner, name) => {
            rebind_to_vec(ctx, *name, out)?;
            collect_list_rebinds(ctx, inner, out)
        }
        Pat::Wildcard
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Ctor { .. }
        | Pat::Tuple(_)
        | Pat::Record(_)
        | Pat::Slice { .. } => Ok(()),
        // Every alternative binds the same names at the same types; rebind via
        // the first alternative.
        Pat::Or(alts) => alts
            .first()
            .map_or(Ok(()), |first| collect_list_rebinds(ctx, first, out)),
    }
}

/// Emit `let <name> = <name>.clone();` — rebind a slice ELEMENT binder (`&T`) to
/// the owned `T` the arm body expects.
fn rebind_clone(ctx: &EmitCtx, sym: Symbol, out: &mut String) -> DResult<()> {
    let name = ctx.emit_ident(sym)?;
    write!(out, "let {name} = {name}.clone(); ").map_err(|e| Diagnostic::CompilerBug {
        where_: "ipe_backend_rust::list_binder_rebinds",
        detail: format!("writing element rebind failed: {e}"),
    })
}

/// Emit `let <name> = <name>.to_vec();` — rebind a slice REST / whole-list binder
/// (`&[T]`) to the owned `Vec<T>` the arm body expects.
fn rebind_to_vec(ctx: &EmitCtx, sym: Symbol, out: &mut String) -> DResult<()> {
    let name = ctx.emit_ident(sym)?;
    write!(out, "let {name} = {name}.to_vec(); ").map_err(|e| Diagnostic::CompilerBug {
        where_: "ipe_backend_rust::list_binder_rebinds",
        detail: format!("writing rest rebind failed: {e}"),
    })
}

/// Render a pattern to its Rust spelling. Total and recursive over the entire
/// pattern set:
///
/// * a variable binder (the keyword-mangled name),
/// * a wildcard (`_`),
/// * a literal leaf — int (`0`), bool (`true`), char (`'a'`), string (`"hi"`),
/// * an alias / `as` pattern (`name @ <inner>`),
/// * a tuple pattern (`(sub0, sub1, …)`),
/// * a constructor pattern (`EnumName::Variant` / `EnumName::Variant(sub0, …)`),
/// * a record pattern (`RecXY { x: sub0, y: sub1, .. }`).
///
/// Every nested sub-position recurses through this same function, so an
/// arbitrarily nested shape (`Just (a, b)`, `Node (Node …) x r`,
/// `{ point = (a, b) }`) renders correctly. The renderer stays total: no arm
/// panics, and every fallible lookup is surfaced as a [`Diagnostic`].
fn render_pat(ctx: &EmitCtx, pat: &Pat) -> DResult<String> {
    match pat {
        Pat::Var(sym) => ctx.emit_ident(*sym),
        Pat::Wildcard => Ok("_".to_owned()),
        // Literal leaves render as Rust literals. Int reuses the same spelling as
        // the `Expr::Int` emitter; Bool maps to the Rust keyword constant; Char
        // and Str escape via the `{:?}` Debug form, which produces a valid Rust
        // literal (quotes, backslashes and control chars escaped) and is
        // deterministic.
        Pat::Int(n) => Ok(n.to_string()),
        Pat::Bool(b) => Ok(if *b { "true" } else { "false" }.to_owned()),
        // A well-formed Char pattern carries exactly one character → Rust char
        // literal. A malformed (multi-char / empty) carried string falls back to
        // a string literal rather than emitting invalid Rust, staying total.
        Pat::Char(c) => {
            let mut chars = c.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Ok(format!("{ch:?}")),
                _ => Ok(format!("{c:?}")),
            }
        }
        Pat::Str(s) => Ok(format!("{s:?}")),
        // `inner as name` → Rust binding-with-subpattern `name @ <inner>`. The
        // inner sub-pattern recurses through this same total renderer.
        //
        // This spelling is correct ONLY in a by-REF / refutable MATCH-ARM
        // position, where default binding modes make the sub-bindings borrows
        // so no move occurs. A by-VALUE irrefutable binding (`Expr::Destructure`
        // — the desugaring of a `let`, a single-arm product `case`, and a
        // function/lambda parameter pattern) must NOT reach this arm: `name @
        // inner` would move BOTH the whole (`name`) and each sub-binding, which
        // is a partial move (E0382) for any non-`Copy` payload. Those sites go
        // through `emit_binding_stmts`, which intercepts every alias — at any
        // nesting depth — before it can reach this renderer.
        Pat::Alias(inner, name) => {
            let name = ctx.emit_ident(*name)?;
            let inner = render_pat(ctx, inner)?;
            Ok(format!("{name} @ {inner}"))
        }
        Pat::Tuple(elems) => {
            // A tuple pattern destructures element-by-element: `(p0, p1, …)`.
            // Stays total over any element vector (no arity assumption).
            let mut subs = Vec::with_capacity(elems.len());
            for sub in elems {
                subs.push(render_pat(ctx, sub)?);
            }
            Ok(format!("({})", subs.join(", ")))
        }
        Pat::Ctor {
            home,
            ty,
            variant,
            args,
        } => {
            // A built-in `Maybe` / `Result` pattern routes to the runtime enum
            // path; otherwise it is a user enum resolved by `enum_name`.
            let path = match ctx.builtin_runtime_enum(home, *ty) {
                Some(runtime) => format!("{runtime}::{}", ctx.emit_ident(*variant)?),
                None => format!(
                    "{}::{}",
                    ctx.enum_name(home, *ty)?,
                    ctx.emit_ident(*variant)?
                ),
            };
            if args.is_empty() {
                Ok(path)
            } else {
                let mut subs = Vec::with_capacity(args.len());
                for sub in args {
                    subs.push(render_pat(ctx, sub)?);
                }
                Ok(format!("{path}({})", subs.join(", ")))
            }
        }
        Pat::Record(fields) => render_record_pat(ctx, fields),
        // A list / cons pattern renders as a native Rust slice pattern. A closed
        // (exact-length) pattern is `[p0, p1]`; an open cons tail is
        // `[p0, p1, rest @ ..]` (binding the rest) or `[p0, p1, ..]` (ignoring
        // it). The leading element patterns recurse through this same renderer.
        Pat::Slice { prefix, rest } => {
            let mut parts = Vec::with_capacity(prefix.len() + 1);
            for sub in prefix {
                parts.push(render_pat(ctx, sub)?);
            }
            match rest {
                Some(r) => {
                    parts.push(render_rest_pat(ctx, r)?);
                    Ok(format!("[{}]", parts.join(", ")))
                }
                None => Ok(format!("[{}]", parts.join(", "))),
            }
        }
        // An or-pattern renders as the native Rust or-pattern `p0 | p1 | …`,
        // joining each rendered alternative with ` | `. Every alternative binds
        // the same names (proved upstream), so the ONE arm body reads them
        // whichever alternative matched — no body duplication. Rust resolves
        // overlap and ordering across alternatives exactly as it does across
        // arms.
        Pat::Or(alts) => {
            let mut parts = Vec::with_capacity(alts.len());
            for alt in alts {
                parts.push(render_pat(ctx, alt)?);
            }
            Ok(parts.join(" | "))
        }
    }
}

/// Does this irrefutable binder carry an `as`-alias anywhere in its shape?
///
/// A by-VALUE binding of an alias cannot use Rust's `name @ inner` spelling
/// (it moves the whole AND the sub-bindings — a partial move / `E0382` for any
/// non-`Copy` payload), so [`emit_binding_stmts`] takes the clone-splitting
/// path whenever this returns `true`. This walks exactly the shapes the
/// destructure-binder grammar admits — variable, wildcard, tuple, alias, and a
/// top-level record whose fields are only variables / wildcards. A record field
/// therefore never carries an alias (the lowerer forbids it — IPE-L0112), and a
/// constructor / slice / literal never appears in an irrefutable binder, so
/// those return `false`. The predicate and [`emit_binding_stmts`] special-case
/// the SAME two shapes (`Alias`, `Tuple`); any disagreement fails closed there.
fn pat_contains_alias(pat: &Pat) -> bool {
    match pat {
        Pat::Alias(..) => true,
        Pat::Tuple(elems) => elems.iter().any(pat_contains_alias),
        Pat::Var(_)
        | Pat::Wildcard
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Ctor { .. }
        | Pat::Record(_)
        | Pat::Slice { .. }
        // An or-pattern is refutable, so it never appears in a by-value
        // irrefutable binder.
        | Pat::Or(_) => false,
    }
}

/// Does this pattern contain a [`Pat::Alias`] ANYWHERE in its shape —
/// unlike [`pat_contains_alias`] (which only recurses into `Tuple`, because
/// it exists solely for the by-VALUE Destructure grammar where
/// `Ctor`/`Record`/`Slice` never legitimately appear), this ALSO recurses
/// into `Ctor` args, `Record` fields, and `Slice` prefix/rest — all of which
/// DO appear in a refutable match-arm pattern.
fn pat_contains_alias_in_arm(pat: &Pat) -> bool {
    match pat {
        Pat::Alias(..) => true,
        Pat::Tuple(elems) => elems.iter().any(pat_contains_alias_in_arm),
        Pat::Ctor { args, .. } => args.iter().any(pat_contains_alias_in_arm),
        Pat::Record(fields) => fields.iter().any(|(_, p)| pat_contains_alias_in_arm(p)),
        Pat::Slice { prefix, rest } => {
            prefix.iter().any(pat_contains_alias_in_arm)
                || rest.as_deref().is_some_and(pat_contains_alias_in_arm)
        }
        // An or-pattern carries an alias iff any alternative does.
        Pat::Or(alts) => alts.iter().any(pat_contains_alias_in_arm),
        Pat::Var(_) | Pat::Wildcard | Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) | Pat::Str(_) => {
            false
        }
    }
}

/// Does this arm pattern carry a string-literal (`Pat::Str`) leaf anywhere in a
/// BY-VALUE-matched position (a tuple element, a ctor / record payload, or an
/// alias inner)? On the whole-scrutinee by-value path a `Pat::Str` is a `&str`
/// pattern against an owned `String` field (E0308); the emitter instead binds
/// the field and checks equality in a match guard
/// (`render_arm_pat_alias_safe`'s `guards` accumulator — mirrors the reference's
/// `renderPatGuarded`). This detects when that guard path is needed so the
/// alias-free / str-free fast path stays byte-identical for every other arm.
///
/// A `Pat::Slice` prefix/rest is deliberately NOT recursed: a slice column
/// reaches the reference-style LIST mode (matched by reference), never the
/// by-value renderer, and the lowerer keeps a list / cons tuple column
/// fail-closed on the variable-scrutinee path (IPE-L0115), so no `Pat::Str`
/// under a slice can reach here.
fn pat_contains_str_in_arm(pat: &Pat) -> bool {
    match pat {
        Pat::Str(_) => true,
        Pat::Alias(inner, _) => pat_contains_str_in_arm(inner),
        Pat::Tuple(elems) => elems.iter().any(pat_contains_str_in_arm),
        Pat::Ctor { args, .. } => args.iter().any(pat_contains_str_in_arm),
        Pat::Record(fields) => fields.iter().any(|(_, p)| pat_contains_str_in_arm(p)),
        // An or-pattern carries a by-value string leaf iff any alternative does.
        Pat::Or(alts) => alts.iter().any(pat_contains_str_in_arm),
        Pat::Var(_)
        | Pat::Wildcard
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Slice { .. } => false,
    }
}

/// Render a BY-VALUE (whole-scrutinee, non-str, non-list) match-arm
/// sub-pattern, routing any [`Pat::Alias`] through the SAME "bind the whole,
/// destructure the inner shape from a CLONE" strategy
/// [`emit_binding_stmts`] already proves sound for irrefutable Destructure
/// positions — because in THIS context the scrutinee is matched BY VALUE
/// (never `&str`/`&[T]`), so `render_pat`'s `name @ inner` spelling (sound
/// only under a by-REF default binding mode) would double-move `name` and
/// `inner`'s own bindings for any non-`Copy` payload.
///
/// A subtree with no alias anywhere renders through the existing,
/// byte-identical [`render_pat`] (fast path — zero behavior change for the
/// overwhelmingly common alias-free case). `prelude` accumulates the `let`
/// statements that re-derive every aliased binder; the caller splices it
/// into the SAME prelude slot `emit_ctor_arm_pat`'s cyclic-self-edge
/// unboxing already uses (`unbox_lines`) or `emit_whole_arm_head`'s
/// `prelude` return.
#[allow(clippy::too_many_lines)] // one arm per IR pattern shape — a rendering table, not branching logic
fn render_arm_pat_alias_safe(
    ctx: &EmitCtx,
    pat: &Pat,
    counter: &mut usize,
    prelude: &mut String,
    guards: &mut Vec<String>,
) -> DResult<String> {
    // Fast path: no alias AND no by-value string-literal leaf → the plain,
    // byte-identical renderer. A `Pat::Str` in a by-value position would render
    // as a `&str` literal pattern against an owned `String` field (E0308), so
    // its presence forces the guard walk below even when there is no alias.
    if !pat_contains_alias_in_arm(pat) && !pat_contains_str_in_arm(pat) {
        return render_pat(ctx, pat);
    }
    match pat {
        // A by-value string-literal column: Rust can't match an owned
        // `String` field against a `&str` literal pattern, so bind the field to a
        // fresh `__sgN` and emit an `if __sgN.as_str() == "lit"` match guard. The
        // caller ANDs the accumulated guards onto the arm — a false guard falls
        // through to the next arm, exactly matching the `case`'s literal-column
        // semantics. Mirrors the reference's `renderPatGuarded`.
        Pat::Str(s) => {
            let binder = format!("__sg{}", *counter);
            *counter += 1;
            guards.push(format!("{binder}.as_str() == {s:?}"));
            Ok(binder)
        }
        Pat::Alias(inner, _name) => {
            // IPE-L0128 (`gate_by_value_dispatch_needing_aliases`) guarantees
            // `inner` is dispatch-free by the time lowering succeeds; fail
            // closed rather than silently mis-emit if that invariant is ever
            // violated — never trust a backend-side "this can't happen"
            // silently.
            if !ipe_ir::is_dispatch_free(inner) {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::render_arm_pat_alias_safe",
                    detail: "alias over a dispatch-needing inner pattern reached the \
                             backend; IPE-L0128 should have rejected this at lowering"
                        .to_owned(),
                });
            }
            let temp = format!("__ipe_arm_alias_{}", *counter);
            *counter += 1;
            // `emit_binding_stmts` already handles
            // `Pat::Alias` exactly this way: `let <name> = <src>; let
            // <inner-pattern> = <name>.clone();` — reuse it verbatim, passing
            // the WHOLE alias node and the fresh temp as `src`.
            for stmt in emit_binding_stmts(ctx, pat, &temp)? {
                prelude.push_str(&stmt);
                prelude.push(' ');
            }
            Ok(temp)
        }
        Pat::Tuple(elems) => {
            let mut subs = Vec::with_capacity(elems.len());
            for e in elems {
                subs.push(render_arm_pat_alias_safe(ctx, e, counter, prelude, guards)?);
            }
            Ok(format!("({})", subs.join(", ")))
        }
        Pat::Ctor {
            home,
            ty,
            variant,
            args,
        } => {
            let path = match ctx.builtin_runtime_enum(home, *ty) {
                Some(runtime) => format!("{runtime}::{}", ctx.emit_ident(*variant)?),
                None => format!(
                    "{}::{}",
                    ctx.enum_name(home, *ty)?,
                    ctx.emit_ident(*variant)?
                ),
            };
            if args.is_empty() {
                Ok(path)
            } else {
                let mut subs = Vec::with_capacity(args.len());
                for a in args {
                    subs.push(render_arm_pat_alias_safe(ctx, a, counter, prelude, guards)?);
                }
                Ok(format!("{path}({})", subs.join(", ")))
            }
        }
        Pat::Record(fields) => {
            // Mirror [`render_record_pat`]'s struct-name resolution but
            // recurse sub-patterns through this alias-safe renderer instead
            // of the plain one.
            let mut key = Vec::with_capacity(fields.len());
            for (sym, _) in fields {
                key.push(ctx.resolve_ident(*sym)?.to_owned());
            }
            let struct_name = ctx.record_name_for_literal(&key)?.to_owned();
            let mut parts = Vec::with_capacity(fields.len());
            for (sym, sub) in fields {
                let field_ident = ctx.emit_ident(*sym)?;
                if let Pat::Var(var) = sub
                    && ctx.emit_ident(*var)? == field_ident
                {
                    parts.push(field_ident);
                } else {
                    let rendered = render_arm_pat_alias_safe(ctx, sub, counter, prelude, guards)?;
                    parts.push(format!("{field_ident}: {rendered}"));
                }
            }
            if parts.is_empty() {
                Ok(format!("{struct_name} {{ .. }}"))
            } else {
                Ok(format!("{struct_name} {{ {}, .. }}", parts.join(", ")))
            }
        }
        // A `Slice` carrying a nested alias reaches LIST mode, which matches
        // by reference and so needs no by-value alias-safety handling — this
        // by-VALUE renderer is never invoked from that path, so reaching here
        // is an internal invariant violation, not a real user program.
        Pat::Slice { .. } => Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::render_arm_pat_alias_safe",
            detail: "Pat::Slice reached the by-value alias-safe renderer; list-mode \
                     arms must route through render_pat directly"
                .to_owned(),
        }),
        // An or-pattern reaching the alias-safe body carries an alias or a
        // by-value string leaf inside SOME alternative. A per-alternative match
        // guard cannot attach to one branch of a Rust or-pattern, so a
        // string-literal alternative is the residual guarded-alternative case
        // (design §4.3) — fail closed rather than emit an invalid guarded
        // or-pattern. An alias-only or-pattern renders each alternative through
        // this same alias-safe renderer (its clone-split prelude binds the
        // shared names) and joins with ` | `.
        Pat::Or(alts) => {
            if alts.iter().any(pat_contains_str_in_arm) {
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::render_arm_pat_alias_safe",
                    detail: "a by-value string-literal leaf inside an or-pattern \
                             alternative needs a per-alternative match guard, which \
                             a Rust or-pattern cannot carry; the lowerer's residual \
                             shared-continuation fallback should have handled it"
                        .to_owned(),
                });
            }
            let mut parts = Vec::with_capacity(alts.len());
            for alt in alts {
                parts.push(render_arm_pat_alias_safe(
                    ctx, alt, counter, prelude, guards,
                )?);
            }
            Ok(parts.join(" | "))
        }
        // `Pat::Str` is intercepted above (binder + guard); the remaining
        // leaves render directly.
        Pat::Var(_) | Pat::Wildcard | Pat::Int(_) | Pat::Bool(_) | Pat::Char(_) => {
            render_pat(ctx, pat)
        }
    }
}

/// Emit the Rust `let` statement sequence for an irrefutable destructuring
/// binding `<binder> = <value>` (WITHOUT the trailing body). Shared by both
/// `Expr::Destructure` emit sites (value-context and tail-context), which is the
/// desugaring of a `let` destructure, a single-arm product `case`, and a
/// function / lambda parameter pattern.
///
/// The SEAL-upholding logic lives here: in a by-VALUE binding position an
/// `as`-alias must NOT render as `name @ inner`. That binds BOTH the whole
/// (`name`) and the sub-bindings by move, a partial move (`E0382`) for any
/// non-`Copy` payload (`\((a, b) as whole) -> …` over `(String, String)` is
/// otherwise `ipe`-0 then `cargo`-101). Instead the whole is bound first and
/// the inner shape is destructured from a CLONE:
///
/// ```ignore
/// let whole = <value>;
/// let (a, b) = whole.clone();
/// ```
///
/// A destructure-position value is `Clone` — the derive-seal already
/// rejects any non-`Clone` payload upstream — so the clone always resolves.
/// When the binder carries NO alias the fast path emits the single flat
/// `let <pat> = <value>;`, a plain clone-free binding. Aliases nested inside
/// tuples (`let (x, (a, b) as inner) = …`) are
/// handled at any depth: each tuple element binds to a fresh, uniquely-numbered
/// temporary, so a nested alias clones from its OWN temp and never shares a move
/// with a sibling binder.
pub fn emit_binding_stmts(ctx: &EmitCtx, binder: &Pat, value: &str) -> DResult<Vec<String>> {
    let mut out = Vec::new();
    let mut counter: usize = 0;
    push_binding_stmts(ctx, binder, value, &mut counter, &mut out)?;
    Ok(out)
}

fn push_binding_stmts(
    ctx: &EmitCtx,
    pat: &Pat,
    src: &str,
    counter: &mut usize,
    out: &mut Vec<String>,
) -> DResult<()> {
    // Fast path: an alias-free binder binds every name via a single flat,
    // move-only `let <pat> = <src>;` — no clone.
    if !pat_contains_alias(pat) {
        let rendered = render_pat(ctx, pat)?;
        out.push(format!("let {rendered} = {src};"));
        return Ok(());
    }
    match pat {
        // `inner as name`: bind the whole first, then destructure the inner
        // shape from a CLONE so the whole binding and the sub-bindings never
        // both move the same value.
        Pat::Alias(inner, name) => {
            let name = ctx.emit_ident(*name)?;
            out.push(format!("let {name} = {src};"));
            push_binding_stmts(ctx, inner, &format!("{name}.clone()"), counter, out)
        }
        // A tuple carrying an alias in some element: bind each element to a
        // fresh, uniquely-numbered temp (a plain move-only destructure), then
        // recurse per element. The unique counter guarantees a nested aliased
        // tuple never re-uses an outer temp name.
        Pat::Tuple(elems) => {
            let base = *counter;
            *counter += elems.len();
            let temps: Vec<String> = (0..elems.len())
                .map(|i| format!("__ipe_bind_{}", base + i))
                .collect();
            out.push(format!("let ({}) = {src};", temps.join(", ")));
            for (elem, temp) in elems.iter().zip(&temps) {
                push_binding_stmts(ctx, elem, temp, counter, out)?;
            }
            Ok(())
        }
        // No other binder shape carries an alias (see [`pat_contains_alias`]).
        // If the predicate and this match ever disagree, fail closed rather
        // than silently emit a moving `name @ inner`.
        Pat::Var(_)
        | Pat::Wildcard
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Ctor { .. }
        | Pat::Record(_)
        | Pat::Slice { .. }
        // An or-pattern is refutable, so it is never a by-value irrefutable
        // binder; reaching here is an invariant violation.
        | Pat::Or(_) => Err(Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::push_binding_stmts",
            detail: "an aliased binder resolved to a non-alias, non-tuple shape".to_owned(),
        }),
    }
}

/// Render the open TAIL of a slice pattern — the `rest @ ..` / `..` suffix. A
/// variable binds the remaining slice (`name @ ..`); a wildcard ignores it
/// (`..`). The lowerer admits only these two rest shapes ([`crate`]-side
/// `lower_rest_pat` gates the rest), so the renderer is total over them.
fn render_rest_pat(ctx: &EmitCtx, pat: &Pat) -> DResult<String> {
    match pat {
        Pat::Var(s) => Ok(format!("{} @ ..", ctx.emit_ident(*s)?)),
        // A wildcard ignores the tail (`..`). No other rest shape is produced by
        // the lowerer, so the catch-all stays total — a bare `..` ignores the
        // tail rather than mis-rendering.
        _ => Ok("..".to_owned()),
    }
}

/// Render a record pattern `{ field0 = p0, … }` to a Rust struct pattern
/// `RecXY { field0: p0, …, .. }`.
///
/// The struct is resolved by the pattern's field-name set, exactly as a record
/// LITERAL resolves its struct (Rust names struct-pattern fields, so write order
/// is free). The lowerer surfaces the complete field set, so this exact-set
/// lookup is unambiguous; a miss is an upstream-contract violation surfaced as a
/// [`Diagnostic::CompilerBug`] rather than a silent mis-emit.
///
/// A trailing `..` is always emitted: it both matches the canonical struct-
/// pattern shape and makes the rendering robust to a field the pattern does not
/// bind (zero remaining fields under the complete-set contract — a legal,
/// no-op `..`). A field whose sub-pattern is a variable bound to the field's own
/// name renders in Rust shorthand (`x` rather than the lint-flagged `x: x`).
fn render_record_pat(ctx: &EmitCtx, fields: &[(Symbol, Pat)]) -> DResult<String> {
    // Resolve the struct by the (sorted) set of bound field names.
    let mut key = Vec::with_capacity(fields.len());
    for (sym, _) in fields {
        key.push(ctx.resolve_ident(*sym)?.to_owned());
    }
    let struct_name = ctx.record_name_for_literal(&key)?.to_owned();

    let mut parts = Vec::with_capacity(fields.len());
    for (sym, sub) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        // Field-pun shorthand: `Rec { x, .. }` instead of `Rec { x: x, .. }`
        // (the latter trips rustc's `non_shorthand_field_patterns` lint). Only
        // when the sub-pattern is a variable whose emitted name equals the
        // field's emitted name.
        if let Pat::Var(var) = sub
            && ctx.emit_ident(*var)? == field_ident
        {
            parts.push(field_ident);
        } else {
            let rendered = render_pat(ctx, sub)?;
            parts.push(format!("{field_ident}: {rendered}"));
        }
    }
    // An empty entry vector is degenerate (the lowerer never produces it), but
    // stay total: render `Rec { .. }` rather than the invalid `Rec { , .. }`.
    if parts.is_empty() {
        Ok(format!("{struct_name} {{ .. }}"))
    } else {
        Ok(format!("{struct_name} {{ {}, .. }}", parts.join(", ")))
    }
}

/// Field names of the `HttpRequest` runtime struct, sorted alphabetically.
/// Used by [`emit_record`] as a FALLBACK to detect `HttpRequest` literals and
/// bypass the synthesised-struct lookup (the type is defined in
/// `ipe_runtime::http_client`, not emitted by the backend) — consulted only
/// when [`EmitCtx::has_record_struct_for`] finds no registered struct for the
/// literal's field-name set. See that method's doc comment for why the two
/// checks must run in THIS order (registry first, name-only fallback
/// second): `ipe_backend_rust` has no access to `ipe_lower`'s `Ty` /
/// `canon::Type` (no cross-crate dependency), so it cannot re-run the
/// lowerer's now-TYPE-AWARE `HttpRequest`-shape test
/// (`ipe_lower::lower::is_http_request_shape`) directly here — deferring to
/// the registry is how this call site stays in sync with that test without
/// duplicating it.
const HTTP_REQUEST_FIELDS: &[&str] = &[
    "body",
    "followRedirects",
    "headers",
    "maxRedirects",
    "method",
    "timeout",
    "url",
];

/// the sorted `Ipe.Cache.CacheCfg` field-name set — a record literal with
/// exactly these names (and no registered synthesised struct, because the
/// lowerer folded the shape to `IrType::CacheCfg`) constructs the runtime
/// `ipe_runtime::cache::CacheCfg` struct. Mirrors [`HTTP_REQUEST_FIELDS`]; kept
/// in sync with `ipe_lower::lower::CACHE_CFG_FIELDS`.
const CACHE_CFG_FIELDS: &[&str] = &["maxBytes", "maxEntries", "ttlMs"];

/// the sorted `Ipe.Csv.Csv` field-name set — a record literal with exactly
/// these names (and no registered synthesised struct, because the lowerer
/// folded the shape to `IrType::CsvDoc`) constructs the runtime
/// `ipe_runtime::csv::CsvDoc` struct. Mirrors [`CACHE_CFG_FIELDS`]; kept in
/// sync with `ipe_lower::lower::CSV_DOC_FIELDS`.
const CSV_DOC_FIELDS: &[&str] = &["header", "rows"];

/// the sorted `Ipe.WebSocket.WebSocketCfg` field-name set — a record
/// literal with exactly these names (and no registered synthesised struct,
/// because the lowerer folded the shape to `IrType::WebSocketClientCfg`)
/// constructs the runtime `ipe_runtime::ws_client::WsClientCfg` struct. Mirrors
/// [`CACHE_CFG_FIELDS`]; kept in sync with
/// `ipe_lower::lower::WEBSOCKET_CFG_FIELD_TYPES`.
const WEBSOCKET_CFG_FIELDS: &[&str] = &["headers", "pingInterval", "timeout", "url"];

/// the sorted `Ipe.Http.Server.Response` field-name set. A record literal
/// with exactly these names (and no registered synthesised struct, because the
/// lowerer folded the shape to `IrType::ServerResponse`) constructs the runtime
/// `ipe_runtime::server::ServerResponse` struct. That struct carries one EXTRA
/// runtime-only field, `cookies: Vec<String>` (multi-`Set-Cookie` support),
/// which the Ipê record alias does not expose — so the literal must default it
/// to `Vec::new()`. Kept in sync with `ipe_lower::lower::SERVER_RESPONSE_FIELD_TYPES`.
const SERVER_RESPONSE_FIELDS: &[&str] = &["body", "contentType", "headers", "status"];

/// the sorted `Ipe.Email` record field-name sets. A record literal with exactly
/// one of these name-sets (and no registered synthesised struct, because the
/// lowerer folded the shape to the matching `IrType::Email*`) constructs the
/// runtime struct (re-exported bare via `pub use email::*`). Mirror of the
/// `CsvDoc` fall-through; kept in sync with `ipe_lower::lower::EMAIL_*_FIELDS`.
/// The four name-sets are mutually distinct, so the name-only match is exact
/// (soundness note: a genuine `Ipe.Email` literal never gets a registered
/// struct because the lowerer intercepts it into the `IrType::Email*` fold
/// first — the same rationale as `CsvDoc`).
const EMAIL_MESSAGE_FIELDS: &[&str] = &[
    "attachments",
    "bcc",
    "cc",
    "from",
    "htmlBody",
    "replyTo",
    "subject",
    "textBody",
    "to",
];
const EMAIL_ATTACHMENT_FIELDS: &[&str] = &["content", "filename", "mimeType"];
const EMAIL_SES_FIELDS: &[&str] = &["key", "region", "secret"];
const EMAIL_SMTP_FIELDS: &[&str] = &["host", "pass", "port", "user"];

/// Emit a record literal `{ x = e1, ... }` as a named struct literal
/// `RecXY { x: <e1>, ... }`. `depth` is the literal's own IR-nesting level; its
/// field values are emitted one level deeper. Kept out of the `emit_expr_at`
/// match (`#[inline(never)]`) so its locals don't inflate the recursive frame.
#[inline(never)]
fn emit_record(
    ctx: &EmitCtx,
    fields: &[(Symbol, Expr)],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let (struct_name, is_server_response) = record_struct_name(ctx, fields)?;
    let mut parts = Vec::with_capacity(fields.len() + usize::from(is_server_response));
    for (sym, value) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        let rendered = emit_expr_at(ctx, value, indent, child, generics)?;
        parts.push(format!("{field_ident}: {rendered}"));
    }
    if is_server_response {
        // The runtime struct's multi-`Set-Cookie` field is not part of the Ipê
        // record alias; default it so the struct literal is complete.
        parts.push("cookies: Vec::new()".to_owned());
    }
    Ok(format!("{struct_name} {{ {} }}", parts.join(", ")))
}

/// Resolve the Rust struct name a record literal constructs, from its field-name
/// SET (Rust names struct-literal fields, so field write order is free). Returns
/// the struct name and whether it folds to the runtime `ServerResponse` struct
/// (which carries an extra `cookies: Vec<String>` field the Ipê record alias
/// omits, so the caller appends a `cookies: Vec::new()` field). Shared by
/// [`emit_record`] and the native Doc emitter so the two agree on the struct name
/// exactly.
pub fn record_struct_name(ctx: &EmitCtx, fields: &[(Symbol, Expr)]) -> DResult<(String, bool)> {
    // The struct is resolved by the literal's field-name set (Rust names
    // struct-literal fields, so write order is free); the field idents are
    // keyword-mangled to match the struct definition.
    let mut key = Vec::with_capacity(fields.len());
    for (sym, _) in fields {
        key.push(ctx.resolve_ident(*sym)?.to_owned());
    }
    // `true` when the shape folds to the runtime `ServerResponse` struct, which
    // carries an extra `cookies: Vec<String>` field the Ipê record alias omits.
    let mut is_server_response = false;
    let struct_name: String = {
        // Prefer an actual synthesised struct when one is registered for
        // this exact field-name set — that reflects `ipe_lower`'s
        // authoritative, TYPE-AWARE decision (see
        // `EmitCtx::has_record_struct_for`'s doc comment). Only fall back to
        // the field-NAME-only `HttpRequest` heuristic when NO struct is
        // registered, which is precisely the signature of a genuine
        // `HttpRequest` literal (the lowerer intercepts it into the opaque
        // `IrType::HttpRequest` before it ever reaches the struct registry).
        // This ordering closes the false-positive class where an unrelated
        // record sharing the 7 canonical field NAMES with unrelated field
        // TYPES (e.g. all-`Int`) would be mislabelled `HttpRequest` here
        // even after `ipe_lower` had already registered a correctly-typed
        // struct for it — a two-path divergence the registry check avoids.
        if ctx.has_record_struct_for(&key) {
            ctx.record_name_for_literal(&key)?.to_owned()
        } else {
            let mut sorted = key.clone();
            sorted.sort();
            let is_http_request = sorted.len() == HTTP_REQUEST_FIELDS.len()
                && sorted
                    .iter()
                    .zip(HTTP_REQUEST_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // same fall-through as HttpRequest — a `CacheCfg`-shaped literal
            // has no registered struct (folded to `IrType::CacheCfg`), so it
            // constructs the runtime `CacheCfg` (re-exported bare via the glob).
            let is_cache_cfg = sorted.len() == CACHE_CFG_FIELDS.len()
                && sorted
                    .iter()
                    .zip(CACHE_CFG_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // same fall-through — a `Csv`-shaped literal has no registered
            // struct (folded to `IrType::CsvDoc`), so it constructs the runtime
            // `CsvDoc` (re-exported bare via the `pub use csv::*` glob).
            let is_csv_doc = sorted.len() == CSV_DOC_FIELDS.len()
                && sorted
                    .iter()
                    .zip(CSV_DOC_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // same fall-through — a `WebSocketCfg`-shaped literal has no
            // registered struct (folded to `IrType::WebSocketClientCfg`), so it
            // constructs the runtime `WsClientCfg` (re-exported bare via the
            // `pub use ws_client::*` glob).
            let is_websocket_cfg = sorted.len() == WEBSOCKET_CFG_FIELDS.len()
                && sorted
                    .iter()
                    .zip(WEBSOCKET_CFG_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // same fall-through — a `Response`-shaped literal has no
            // registered struct (folded to `IrType::ServerResponse`), so it
            // constructs the runtime `ServerResponse` (re-exported bare via the
            // `pub use server::*` glob).
            is_server_response = sorted.len() == SERVER_RESPONSE_FIELDS.len()
                && sorted
                    .iter()
                    .zip(SERVER_RESPONSE_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // Ipe.Email fall-throughs — same rationale as `CsvDoc`: a
            // `defaultMessage`/`defaultAttachment`/… built literal has no
            // registered struct (folded to the matching `IrType::Email*`), so it
            // constructs the runtime struct (re-exported bare via `pub use
            // email::*`). The Ipê `Attachment` alias maps to `EmailAttachment`.
            let name_set_is = |expected: &[&str]| {
                sorted.len() == expected.len()
                    && sorted
                        .iter()
                        .zip(expected.iter())
                        .all(|(a, b)| a.as_str() == *b)
            };
            if is_http_request {
                "HttpRequest".to_owned()
            } else if is_cache_cfg {
                "CacheCfg".to_owned()
            } else if is_csv_doc {
                "CsvDoc".to_owned()
            } else if is_websocket_cfg {
                "WsClientCfg".to_owned()
            } else if is_server_response {
                "ServerResponse".to_owned()
            } else if name_set_is(EMAIL_MESSAGE_FIELDS) {
                "EmailMessage".to_owned()
            } else if name_set_is(EMAIL_ATTACHMENT_FIELDS) {
                "EmailAttachment".to_owned()
            } else if name_set_is(EMAIL_SES_FIELDS) {
                "SesConfig".to_owned()
            } else if name_set_is(EMAIL_SMTP_FIELDS) {
                "SmtpConfig".to_owned()
            } else {
                ctx.record_name_for_literal(&key)?.to_owned()
            }
        }
    };
    Ok((struct_name, is_server_response))
}

/// Emit a functional record update `{ record | f = v, ... }` as a clone-and-
/// reassign block: `{ let mut __ipe_rec = (<record>).clone(); __ipe_rec.f = v;
/// __ipe_rec }`. This needs no struct name and leaves the source record
/// untouched; the block scope makes the temporary safe under nesting. Kept out
/// of the match (`#[inline(never)]`) for the same frame-size reason as
/// [`emit_record`].
#[inline(never)]
fn emit_update(
    ctx: &EmitCtx,
    record: &Expr,
    fields: &[(Symbol, Expr)],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let base = emit_expr_at(ctx, record, indent, child, generics)?;
    let mut assigns = Vec::with_capacity(fields.len());
    for (sym, value) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        let rendered = emit_expr_at(ctx, value, indent, child, generics)?;
        assigns.push(format!(" __ipe_rec.{field_ident} = {rendered};"));
    }
    Ok(format!(
        "{{ let mut __ipe_rec = ({base}).clone();{} __ipe_rec }}",
        assigns.concat()
    ))
}

/// Lay a match-arm rebind `prelude` out one statement per line at `indent`.
///
/// The prelude is a run of `let …; ` binder-rebind statements the clone-split
/// helpers build joined by `"; "`; `rustfmt` puts each on its own line. Split on
/// the separator, re-indent each, and return the block (with its trailing
/// newline) — a trailing empty segment is skipped.
fn tail_arm_prelude_lines(prelude: &str, indent: usize) -> DResult<String> {
    let pad = indent_of(indent);
    let mut out = String::new();
    for stmt in prelude.split_inclusive("; ") {
        let stmt = stmt.trim_end();
        if stmt.is_empty() {
            continue;
        }
        writeln!(out, "{pad}{stmt}").map_err(|e| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::tail_arm_prelude_lines",
            detail: format!("writing TCO arm prelude failed: {e}"),
        })?;
    }
    Ok(out)
}

/// Emit an `Expr` in TAIL/STATEMENT context — the interior of a `TailLoop`'s
/// `loop { … }`. Every path ends in either a `return <expr>;` (a leaf
/// tail position) or a `continue;` (a `TailRecur` jump), so the `loop` types as
/// `!` and unifies with any `-> R` return type (no `break value`). The tail
/// propagators (`If` / `Match` / `Let` / `Destructure`) recurse in-tail; every
/// other node is a leaf whose VALUE is `return`ed. `loop_params` gives each
/// `TailRecur.args[i]` its destination parameter name.
///
/// The `other => return` arm is the intended value/statement split (the
/// reference's `walk True` leaf case), NOT a wildcard over `Expr` variants for
/// exhaustiveness purposes — `emit_expr_at` inside it is the exhaustive,
/// fail-closed walker: a stray `TailLoop`/`TailRecur` reaching it routes to the
/// `CompilerBug` arm (never a panic, never a silent swallow).
#[inline(never)]
fn emit_expr_tail(
    ctx: &EmitCtx,
    expr: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
    loop_params: &[(Symbol, IrType)],
) -> DResult<String> {
    let pad = indent_of(indent);
    let child = depth + 1;
    match expr {
        Expr::If { cond, then_, else_ } => {
            let c = emit_expr_at(ctx, cond, indent, child, generics)?;
            let t = emit_expr_tail(ctx, then_, indent + 1, child, generics, loop_params)?;
            let e = emit_expr_tail(ctx, else_, indent + 1, child, generics, loop_params)?;
            Ok(format!(
                "{pad}if {c} {{\n{t}\n{pad}}} else {{\n{e}\n{pad}}}"
            ))
        }
        Expr::Match(m) => {
            let (scrut, mode) = emit_match_scrutinee(ctx, m, indent, depth, generics)?;
            let arm_indent = indent_of(indent + 1);
            let close_indent = indent_of(indent);
            let mut arms = Vec::with_capacity(m.arms().len());
            for arm in m.arms() {
                let (patstr, prelude, synth_guard) = emit_arm_head(ctx, &arm.pat, &mode)?;
                // The arm body is a STATEMENT sequence ending in return/continue;
                // any binder-rebind prelude precedes it inside the arm's block.
                let body =
                    emit_expr_tail(ctx, &arm.body, indent + 2, child, generics, loop_params)?;
                let inner = if prelude.is_empty() {
                    body
                } else {
                    format!("{}{body}", tail_arm_prelude_lines(&prelude, indent + 2)?)
                };
                // Same `if <guard>` fall-through as the value-context emitter: the
                // list-length arm guard and the synthesized `as_str()` string-
                // column guard are ANDed; `None` leaves the arm guardless.
                let ir_guard = match &arm.guard {
                    Some(g) => Some(emit_expr_at(ctx, g, indent + 1, child, generics)?),
                    None => None,
                };
                let guard_clause = combine_guards(synth_guard, ir_guard)
                    .map_or_else(String::new, |guard| format!(" if {guard}"));
                arms.push(format!(
                    "{arm_indent}{patstr}{guard_clause} => {{\n{inner}\n{arm_indent}}}"
                ));
            }
            Ok(format!(
                "{pad}match {scrut} {{\n{}\n{close_indent}}}",
                arms.join("\n")
            ))
        }
        Expr::Let { name, value, body } => {
            let n = ctx.emit_ident(*name)?;
            let v = emit_expr_at(ctx, value, indent, child, generics)?;
            let b = emit_expr_tail(ctx, body, indent, child, generics, loop_params)?;
            Ok(format!("{pad}let {n} = {v};\n{b}"))
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            let v = emit_expr_at(ctx, value, indent, child, generics)?;
            let stmts = emit_binding_stmts(ctx, binder, &v)?;
            let b = emit_expr_tail(ctx, body, indent, child, generics, loop_params)?;
            let joined = stmts
                .iter()
                .map(|s| format!("{pad}{s}"))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(format!("{joined}\n{b}"))
        }
        // The jump: temporaries-first reassignment + `continue`. Reading EVERY
        // next-iteration argument into a fresh `__tco_<i>` temp BEFORE any
        // parameter write forecloses the arg-swap clobber (`go b a rest` must not
        // read an already-overwritten `a`); each temp reads the CURRENT params.
        Expr::TailRecur { args } => {
            if args.len() != loop_params.len() {
                // Invariant broken by the rewrite — fail closed, never panic.
                return Err(Diagnostic::CompilerBug {
                    where_: "ipe_backend_rust::emit_expr_tail",
                    detail: format!(
                        "TailRecur has {} args but the enclosing TailLoop has {} params",
                        args.len(),
                        loop_params.len()
                    ),
                });
            }
            let mut temps = String::new();
            for (idx, arg) in args.iter().enumerate() {
                let a = emit_expr_at(ctx, arg, indent, child, generics)?;
                writeln!(temps, "{pad}let __tco_{idx} = {a};").map_err(|e| {
                    Diagnostic::CompilerBug {
                        where_: "ipe_backend_rust::emit_expr_tail",
                        detail: format!("writing TCO jump temp failed: {e}"),
                    }
                })?;
            }
            let mut writes = String::new();
            for (idx, (name, _ty)) in loop_params.iter().enumerate() {
                let n = ctx.emit_ident(*name)?;
                writeln!(writes, "{pad}{n} = __tco_{idx};").map_err(|e| {
                    Diagnostic::CompilerBug {
                        where_: "ipe_backend_rust::emit_expr_tail",
                        detail: format!("writing TCO param reassignment failed: {e}"),
                    }
                })?;
            }
            Ok(format!("{temps}{writes}{pad}continue;"))
        }
        // Every other node is a leaf tail position → return its value.
        other => {
            let v = emit_expr_at(ctx, other, indent, child, generics)?;
            Ok(format!("{pad}return {v};"))
        }
    }
}

/// Emit an application of a first-class function value, `(<func>)(<args>)`. The
/// callee is parenthesised so a boxed `dyn Fn` (or any expression value) is
/// applied uniformly — a `Box<dyn Fn(..)>` auto-derefs at the call. `depth` is
/// the application's own IR-nesting level; its callee and arguments are emitted
/// one level deeper. Kept out of the `emit_expr_at` match (`#[inline(never)]`)
/// so its `Vec`/`String` locals don't inflate the recursive frame.
#[inline(never)]
fn emit_apply(
    ctx: &EmitCtx,
    func: &Expr,
    args: &[Expr],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    // ── Immediately-applied lambda inlining (Bug 3a / T4) ──────────────────
    // Pattern: `(Box::new(move |p0: T0, …| -> R { body }))(arg0, …)`
    //
    // Rust requires the closure inside `Box::new(…)` to implement `Fn` (not
    // just `FnOnce`) so that `(Box::new(closure))(arg)` can call it via
    // `Fn::call` (auto-deref path).  When the body creates an inner `move`
    // closure that captures a non-Copy variable from the outer closure's
    // environment (e.g. a `Box<dyn Fn>` HOF arg, or a `String` that is moved
    // into an inner `Box::new(move |…| …)`), the outer closure becomes
    // `FnOnce` — triggering E0525.
    //
    // When a lambda is *immediately applied* (`(lambda)(arg)`), the `Box::new`
    // wrapper is unnecessary.  Inlining as:
    //   `({ let p0: T0 = arg0; … body … })`
    // avoids the `Fn` requirement entirely.  Semantics are identical: the args
    // are evaluated and bound, then the body executes in the same scope.  Free
    // variables from the outer scope are used directly — no capture, no
    // ownership transfer.
    if let Expr::Lambda {
        params,
        ret: _,
        body,
    } = func
    {
        // Lower guarantees `params.len() == args.len()` here; the `zip` below
        // pairs them positionally and would silently drop any excess were that
        // invariant ever broken upstream.
        let child = depth + 1;
        let mut bindings = String::new();
        for ((param, ty), arg) in params.iter().zip(args.iter()) {
            let p = ctx.emit_ident(*param)?;
            let t = render_type(ctx, ty, generics)?;
            let a = emit_expr_at(ctx, arg, indent, child, generics)?;
            // write! to String is infallible (String::write_fmt delegates to push_str).
            let _ = write!(bindings, "let {p}: {t} = {a}; ");
        }
        let body_s = emit_expr_at(ctx, body, indent, child, generics)?;
        return Ok(format!("({{ {bindings}{body_s} }})"));
    }
    let child = depth + 1;
    let f = emit_expr_at(ctx, func, indent, child, generics)?;
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        parts.push(emit_expr_at(ctx, arg, indent, child, generics)?);
    }
    Ok(format!("({f})({})", parts.join(", ")))
}

/// Emit a top-level function (or kernel) named as a first-class *value* as a
/// type-pinned smart-pointer closure.
///
/// For the server-handler shape (`ServerRequest -> Task Error ServerResponse`,
/// which renders as `ServerHandler<IpeError>` — an `Arc<dyn Fn(…)>` alias in
/// the runtime), emits `Arc::new(<name>)` so the coercion produces the correct
/// runtime type.  For every other `Fun` shape, emits `Box::new(<name>)` as
/// before (`Box<dyn Fn(..) -> R + Send + 'static>`).
///
/// The explicit binding type drives the unsized coercion of the named `fn`
/// item (a zero-sized `Fn` implementor) to the smart-pointer trait object, so
/// the value fills the slot uniformly in every position — argument, return, or
/// let-binding — rather than relying on a coercion site that an `if`/`match`
/// branch or a bare `let` would not provide.
///
/// `ty` is the value's `Fun` IR type; [`render_type`] renders it as the typed
/// smart-pointer.  Kept `#[inline(never)]` for the same frame-size reason as
/// the neighbouring helpers.
/// Does a function value / lambda of IR type `ty` fill one of the runtime's
/// `Arc<dyn Fn + Send + Sync>` callback slots (so it must be boxed with
/// `Arc::new`, not `Box::new`)? The shapes:
///   • `ServerHandler<E>`: `Fn(ServerRequest) -> IpeTask<E, ServerResponse>`
///   • `WsServerCfg` callbacks, `-> IpeTask<E, ()>`:
///       - `Fn(WsHandle)`           (onConnect / onClose)
///       - `Fn(WsHandle, String)`   (onMessage)
///       - `Fn(WsHandle, Error)`    (onError — 2nd param is the error type,
///         NOT String; its setter `ws_server_with_on_error` takes `Arc<…>`)
///
/// This MUST dispatch on the `IrType` STRUCTURE, never on the rendered type
/// string. `render_type` renders `ServerHandler<E>` as the type-ALIAS name
/// `"ServerHandler<IpeError>"` — NOT the expanded `"Arc<dyn Fn…>"` — so a
/// `starts_with("Arc<")` string test silently misclassifies every handler shape
/// as `Box` and reintroduces the E0308 seal break for inline
/// `Server.post path (\req -> …)` handler lambdas (the regression this shared
/// helper closes). The param patterns are kept in LOCK-STEP with `render_type`'s
/// WS/ServerHandler Arc arms (`emit_types.rs`) — a shape rendered as `Arc<…>`
/// there but boxed with `Box::new` here (or vice-versa) is an E0308. Both
/// `emit_func_value` and `emit_lambda` route through here so the two emit paths
/// can never drift.
pub fn wants_arc_ctor(ty: &IrType) -> bool {
    // A promoted `SharedFun` slot renders `Arc<dyn Fn>` (`render_type`), so its
    // value must be built with `Arc::new`, not `Box::new` — the two carriers are
    // distinct Rust types and mixing them is an E0308.
    if matches!(ty, IrType::SharedFun(_, _)) {
        return true;
    }
    matches!(ty,
        IrType::Fun(params, ret)
            if (matches!(params.as_slice(), [IrType::ServerRequest])
                && matches!(ret.as_ref(), IrType::Task(inner)
                    if matches!(inner.as_ref(), IrType::ServerResponse)))
               || (matches!(
                    params.as_slice(),
                    [IrType::WebSocketServer]
                        | [IrType::WebSocketServer, IrType::Str | IrType::Error]
                ) && matches!(ret.as_ref(), IrType::Task(inner)
                    if matches!(inner.as_ref(), IrType::Unit)))
    )
}

#[inline(never)]
fn emit_func_value(
    ctx: &EmitCtx,
    callee: &Callee,
    ty: &IrType,
    generics: GenericScope,
) -> DResult<String> {
    let name = callee_name(ctx, callee)?;
    let typed = render_type(ctx, ty, generics)?;
    let ctor = if wants_arc_ctor(ty) { "Arc" } else { "Box" };
    Ok(format!(
        "{{ let __ipe_fn: {typed} = {ctor}::new({name}); __ipe_fn }}"
    ))
}

/// Emit the unboxed inner `move |p0: T0, …| -> R { <body> }` closure expression.
/// Used by both [`emit_lambda`] (wraps it in `Box::new(…)`) and the `succeed`
/// curry path in [`emit_json_decoder_call`] (wraps it in `curry{n}(…)` instead).
/// `depth` is the lambda's own IR-nesting level; the body is emitted one level
/// deeper.
pub fn emit_lambda_unboxed(
    ctx: &EmitCtx,
    params: &[(Symbol, IrType)],
    ret: &IrType,
    body: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let mut parts = Vec::with_capacity(params.len());
    for (param, ty) in params {
        parts.push(format!(
            "{}: {}",
            ctx.emit_ident(*param)?,
            render_type(ctx, ty, generics)?
        ));
    }
    let ret_s = render_type(ctx, ret, generics)?;
    let body_s = emit_expr_at(ctx, body, indent, child, generics)?;
    Ok(format!(
        "move |{}| -> {ret_s} {{ {body_s} }}",
        parts.join(", ")
    ))
}

/// Emit a lambda `\p0 p1 ... -> body` as a boxed closure whose static type is
/// pinned to the trait-object form
/// `{ let __ipe_fn: Box<dyn Fn(T0, ...) -> R + Send + 'static> = Box::new(move
/// |p0: T0, ...| -> R { <body> }); __ipe_fn }`. The `move` capture takes any
/// free locals by value; the explicit return type pins the closure's signature.
///
/// The `let`-binding type annotation is load-bearing: `Box::new(closure)` on
/// its own infers `Box<{closure@…}>` — a box of the CONCRETE, unnameable
/// closure type — which only unsize-coerces to `Box<dyn Fn(..) -> ..>` when the
/// surrounding position supplies the trait-object target (a kernel call arg, a
/// return slot, …). A lambda that flows into a `let` binding first, or into a
/// built-in `Ok`/`Just` payload (which routes to the runtime `IpeResult`/
/// `IpeMaybe` enum whose generic argument is inferred from the constructor arg,
/// NOT from a field type), has no such target at the box site, so Rust pins the
/// concrete closure type and a LATER use against `Box<dyn Fn>` fails as E0308.
/// Pinning the trait object HERE — the same technique [`emit_func_value`] uses
/// for a named function value — makes every lambda's static type the boxed
/// trait object regardless of where it flows, closing the IPE-L0114
/// `let f = Ok (\x -> …)` seal hole with no lowering / type-check change.
///
/// The pointer constructor matches the rendered type: a lambda filling one of
/// the runtime's `Arc<dyn Fn + Send + Sync>` slots (a `ServerHandler` /
/// `WsServerCfg` callback shape — see [`render_type`]'s special-case arms) is
/// boxed with `Arc::new`, everything else with `Box::new`. `depth` is the
/// lambda's own IR-nesting level; its body is emitted one level deeper. Kept
/// out of the `emit_expr_at` match (`#[inline(never)]`) for the same frame-size
/// reason as [`emit_record`] / [`emit_update`].
#[inline(never)]
fn emit_lambda(
    ctx: &EmitCtx,
    params: &[(Symbol, IrType)],
    ret: &IrType,
    body: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let inner = emit_lambda_unboxed(ctx, params, ret, body, indent, depth, generics)?;
    let fun_ty = IrType::Fun(
        params.iter().map(|(_, t)| t.clone()).collect(),
        Box::new(ret.clone()),
    );
    let typed = render_type(ctx, &fun_ty, generics)?;
    // The pointer constructor must match the smart pointer of the annotated
    // type: `Arc::new` for the two runtime handler shapes (ServerHandler /
    // WsServerCfg callbacks, whose fields are `Arc<dyn Fn + Send + Sync>`),
    // `Box::new` otherwise. Dispatch on the IR STRUCTURE via `wants_arc_ctor`,
    // NOT on the rendered string — `render_type` emits `ServerHandler<E>` as the
    // alias name, so a `starts_with("Arc<")` test would misclassify it as Box
    // and E0308 the handler-lambda shape.
    let ctor = if wants_arc_ctor(&fun_ty) {
        "Arc"
    } else {
        "Box"
    };
    Ok(format!(
        "{{ let __ipe_fn: {typed} = {ctor}::new({inner}); __ipe_fn }}"
    ))
}

/// Emit a `let`-bound closure literal that [`ipe_lower`]'s capture analysis
/// (`needs_shared_capture`, which prevents E0507) proved is captured-by-move
/// into 2+ nested/sibling closures, and therefore must be reference-counted
/// (`Arc`) rather than uniquely owned (`Box`) so the corresponding
/// `Expr::CloneVar` reads at every extra capture site (`Arc::clone`, a cheap
/// pointer bump) actually compile.
///
/// Unlike [`emit_lambda`], this does NOT go through `wants_arc_ctor` /
/// `render_type`'s generic `IrType::Fun` arm — that arm renders
/// `Box<dyn Fn(..) -> R + Send + 'static>` (no `Sync`), which would make the
/// `Arc<..>` wrapper itself neither `Send` nor `Sync` (`impl Send/Sync for
/// Arc<T>` both require `T: Send + Sync`) — silently breaking every
/// enclosing closure's OWN `Send + Sync` bound. The trait-object bound here
/// is built directly with the `+ Sync` `Arc<dyn Fn>` needs, mirroring the
/// runtime's existing `ServerHandler` / `WsServerCfg` Arc-callback shapes
/// (`emit_types.rs`) at the type-string level.
#[inline(never)]
fn emit_shared_lambda(
    ctx: &EmitCtx,
    params: &[(Symbol, IrType)],
    ret: &IrType,
    body: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let inner = emit_lambda_unboxed(ctx, params, ret, body, indent, depth, generics)?;
    let mut parts = Vec::with_capacity(params.len());
    for (_, ty) in params {
        parts.push(render_type(ctx, ty, generics)?);
    }
    let ret_s = render_type(ctx, ret, generics)?;
    let typed = format!(
        "::std::sync::Arc<dyn Fn({}) -> {ret_s} + Send + Sync + 'static>",
        parts.join(", ")
    );
    Ok(format!(
        "{{ let __ipe_fn: {typed} = ::std::sync::Arc::new({inner}); __ipe_fn }}"
    ))
}

/// Render one type parameter's trailing bound clause for the generic list:
/// `: ::core::ops::Add<Output = T{n}> + Copy` and the like, or the empty string
/// for an unbounded variable (so a structurally-parametric function emits a
/// bare `T{n}` with no bound clause).
///
/// `n` is the variable's 1-based position, which is also its own Rust name
/// `T{n}` — the arithmetic `::core::ops` traits take `Output = T{n}` so the
/// operation stays closed over the parameter's type (`x + x : T{n}`). The trait
/// order is fixed (`Add`, `Sub`, `Mul`, `PartialOrd`, `PartialEq`, `Ord`,
/// `Hash`, `Copy`, `Clone`, `Into<SqlParam>`) so the emission is deterministic
/// regardless of how the bound set was assembled.
fn render_bounds(bounds: BoundSet, n: usize) -> String {
    if bounds.is_unbounded() {
        return String::new();
    }
    let mut traits = Vec::new();
    if bounds.has_static() {
        // Boxed-callback `'static` lifetime bound: a generic type-param
        // that flows into a value boxed as `Box<dyn Fn(..) -> .. + Send +
        // 'static>` (a callback passed to `List.map` etc.) whose own type still
        // mentions that type-param requires `tv: 'static` for the trait-object
        // coercion. A LIFETIME bound — Rust requires it to PRECEDE every trait
        // bound in the list (`T{n}: 'static + Clone`), so it is pushed FIRST.
        // Satisfied by every concrete Ipê type (emitted values never borrow),
        // so no caller-side failure — see `BoundSet::STATIC`.
        traits.push("'static".to_owned());
    }
    if bounds.has_send() {
        // `Send` auto-trait: a bare `msg` value moved into a `IpeSub::Source`
        // closure (`Box<dyn FnOnce(..) + Send>`) — e.g. `WebSocket.onOpen`'s
        // `msg` into `sub_subscribe_ws_open<M: Send + 'static>`. Pushed after the
        // `'static` lifetime bound (a lifetime must precede trait bounds).
        // Satisfied by every concrete Ipê type (owned, never borrows).
        traits.push("Send".to_owned());
    }
    if bounds.has_add() {
        traits.push(format!("::core::ops::Add<Output = T{n}>"));
    }
    if bounds.has_sub() {
        traits.push(format!("::core::ops::Sub<Output = T{n}>"));
    }
    if bounds.has_mul() {
        traits.push(format!("::core::ops::Mul<Output = T{n}>"));
    }
    if bounds.has_ord() {
        traits.push("PartialOrd".to_owned());
    }
    if bounds.has_eq() {
        traits.push("PartialEq".to_owned());
    }
    if bounds.has_show() {
        // Ipê `toString` / `Log.*With`: the value must render. Fully qualified —
        // the trait is not in the Rust prelude. Every emitted record/ADT + every
        // scalar has a `IpeStringify` impl.
        traits.push("crate::ipe_runtime::stringify::IpeStringify".to_owned());
    }
    if bounds.has_ord_total() {
        // `Ord` (total order) for a `Set` element / sorted `Dict` op; carries
        // `Eq` + `PartialOrd` + `PartialEq` as supertraits, so a `Dict` key's
        // `HashMap` `Eq` requirement is met without a separate `Eq` bound.
        traits.push("Ord".to_owned());
    }
    if bounds.has_hash() {
        // `Hash` for a `Dict` key's `HashMap` backing. Fully qualified — the
        // trait (unlike its derive macro) is not in the Rust prelude.
        traits.push("::core::hash::Hash".to_owned());
    }
    if bounds.has_copy() {
        traits.push("Copy".to_owned());
    }
    if bounds.has_clone() {
        traits.push("Clone".to_owned());
    }
    if bounds.has_sql_param() {
        // SQL-bind-parameter obligation: the runtime's `SqlParam::from`
        // family is realised as `Into<SqlParam>` on the emitted generic (not a
        // `where SqlParam: From<T{n}>` clause) so it composes with the ordinary
        // `<T{n}: Bound1 + Bound2>` list this function already builds — no
        // separate `where`-clause plumbing needed in [`emit_func`].
        traits.push("Into<ipe_runtime::db::SqlParam>".to_owned());
    }
    if bounds.has_ipe_row() {
        // Db field-accessor row obligation: a wildcard `any` generic that
        // flows into a `Db.get*` accessor gains `IpeRow` so the runtime's generic
        // `db_get_*<R: IpeRow>(field, &row)` call type-checks and monomorphises
        // per call site. Fully qualified — the trait is not re-exported at the
        // emitted crate's `pub use ipe_runtime::*` root. Added ONLY to the `any`
        // var and ONLY when the body calls `db_get_*` (see [`emit_func`]).
        traits.push("ipe_runtime::db::IpeRow".to_owned());
    }
    format!(": {}", traits.join(" + "))
}

/// Recursively elide a `Task.run` / `Task.perform` call in EVERY tail
/// position of `expr`, returning the rewritten expression only when ALL tail
/// leaves are such a call. `None` when even one tail leaf is not — a partial
/// elision would leave some arms `Task<A>`-shaped and others
/// `Result<E, A>`-shaped, which cannot render as one Rust `match`/`if` with a
/// single type, so this is deliberately all-or-nothing.
///
/// Mirrors [`emit_func`]'s original flat single-call elision (a bare
/// `Call(TaskRun, [inner])` whole-function body) generalised through the
/// control-flow constructs that legally appear in a tail position: `Match`
/// (`case`), `If`, and `Let` / `Destructure` (only their BODY is a tail
/// position — the bound `value` is left untouched and un-recursed-into).
/// `Match` is rebuilt via [`Match::from_parts_unchecked`]: only arm BODIES
/// change here, never the arm patterns, so the exhaustiveness proof
/// [`Match::new`] / [`Match::new_flat`] already ran stays valid.
fn elide_task_run_tail(expr: &Expr) -> Option<Expr> {
    match expr {
        Expr::Call {
            callee: Callee::Kernel(KernelFn::TaskRun | KernelFn::TaskPerform),
            args,
            ..
        } => {
            let [inner] = args.as_slice() else {
                return None;
            };
            Some(inner.clone())
        }
        Expr::If { cond, then_, else_ } => {
            let then_e = elide_task_run_tail(then_)?;
            let else_e = elide_task_run_tail(else_)?;
            Some(Expr::If {
                cond: cond.clone(),
                then_: Box::new(then_e),
                else_: Box::new(else_e),
            })
        }
        Expr::Let { name, value, body } => {
            let body_e = elide_task_run_tail(body)?;
            Some(Expr::Let {
                name: *name,
                value: value.clone(),
                body: Box::new(body_e),
            })
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            let body_e = elide_task_run_tail(body)?;
            Some(Expr::Destructure {
                binder: binder.clone(),
                value: value.clone(),
                body: Box::new(body_e),
            })
        }
        Expr::Match(m) => {
            // Sealed rebuild via `try_map_bodies` (AUD-09): the scrutinee and
            // every arm's pattern/guard pass through UNCHANGED (by
            // construction, not by convention), so exhaustiveness is
            // preserved with no re-derivation needed — only each arm's body
            // is transformed, and any single arm declining the elision
            // (`elide_task_run_tail` returning `None`) must fail the WHOLE
            // match's elision, matching this function's existing `?`-based
            // all-or-nothing contract on every other tail-position arm.
            m.clone()
                .try_map_bodies(Ok::<_, ()>, |_pat, body, guard| {
                    let new_body = elide_task_run_tail(&body).ok_or(())?;
                    Ok((new_body, guard))
                })
                .ok()
                .map(Expr::Match)
        }
        // Every other expression shape is a genuine value in tail position
        // (not a control-flow construct that merely forwards to a nested tail
        // position), so it either IS the whole elidable call (handled above)
        // or it is not elidable at all.
        _ => None,
    }
}

/// [`emit_func`]'s `ipe_main` synchronous-body wrap decision.
///
/// When `ipe_main` was NOT elided (its body is not — or not uniformly in
/// every tail position — a `Task.run` call), the function currently returns
/// its declared value type directly, but the entry-point epilogue calls
/// `block_on(ipe_main())`, which requires `ipe_main` to return `IpeTask<A>`
/// (an unevaluated future), never a resolved value.
///
/// Two declared-return shapes reach here, and BOTH wrap the body rather than
/// change its VALUE — `ipe_main`'s body already runs to completion
/// synchronously either way (a bare `task_run()` call blocks in place); the
/// wrap only reshapes the return type so `block_on` type-checks:
///
/// * `func.ret == Unit` — Ipê CLI programs that use synchronous `task_run()`
///   calls (instead of building a top-level Task pipeline). The caller wraps:
///   `let _r = { <original body> }; task_succeed(())` — `ipe_main` returns
///   `IpeTask<()>`, discarding the body's (unit) value. Signalled by the
///   returned `wrap_unit = true`.
/// * `func.ret == Result(_, A)` with elision declined — the argv-dispatch
///   idiom's MIXED-arm sibling gap (adversarial-review Finding B): some
///   `case` tail leaves call `Task.run` (blocks synchronously, producing a
///   real `Result e a`), OTHER tail leaves are a plain `Result`-typed
///   expression with no `Task.run` at all (`Err e -> Err e` in a
///   validate-then-run idiom, e.g. `case validate () of Err e -> Err e; Ok
///   cfg -> app cfg |> Task.run`). `elide_task_run_tail` correctly declines a
///   partial elision (mismatched Task/Result arm shapes cannot render as one
///   `match` of a single type) — but the body AS A WHOLE already evaluates
///   synchronously to one uniform `Result e a`. The caller wraps:
///   `task_from_result({ <original body> })` — `ipe_main` returns `IpeTask<A>`,
///   an ALREADY-RESOLVED future carrying the body's actual computed
///   `Ok`/`Err`, so `block_on` unwraps it back to the exact `IpeResult<E, A>`
///   the un-wrapped body would have produced directly; `fn main`'s
///   `Ok(_)`/`Err(e)` epilogue match sees identical values. Signalled by
///   `Some(Task(ok_ty))` in the returned `Option`.
///
/// Returns `(wrap_unit, wrap_result_ok_ty)` — at most one is ever set (`Unit`
/// and `Result` are disjoint [`IrType`] shapes).
fn ipe_main_wrap_decision(
    name: &str,
    elided_ret: Option<&IrType>,
    func_ret: &IrType,
) -> (bool, Option<IrType>) {
    if name != "ipe_main" || elided_ret.is_some() {
        return (false, None);
    }
    match func_ret {
        IrType::Unit => (true, None),
        IrType::Result(_err_ty, ok_ty) => (false, Some(IrType::Task(ok_ty.clone()))),
        _ => (false, None),
    }
}

/// Emit a whole function item, including its trailing newline.
///
/// Shape: `pub fn <name>[<generics>](<params>) -> <ret> {\n    <body>\n}\n`. A
/// monomorphic function (empty `type_params`) emits no generic clause, so its
/// output is byte-identical to the golden `main_update` / `ipe_main`. A
/// fully-parametric function quantifying `[a, b]` emits `pub fn name<T1, T2>(..)`
/// and renders every [`IrType::Generic`] in its signature / body through the
/// matching scope. A variable carrying a [`BoundSet`] gains its
/// `: <bounds>` clause at its position. The body is an expression rendered
/// at indentation level 1; the closing brace sits at column 0.
pub fn emit_func(ctx: &EmitCtx, func: &Func) -> DResult<String> {
    emit_func_vis(ctx, func, "pub fn ")
}

/// Emit a whole function item with the given visibility prefix (`"pub fn "` for
/// the single-file layout, `"pub(crate) fn "` for a split `IpeModule` file where
/// the item lives inside a `mod` block). The prefix is threaded through to
/// [`render_fn_signature`] so the signature's flat-vs-broken width decision
/// measures against the prefix the emitted line actually carries — the
/// `pub(crate)` form is seven columns wider than `pub`, so a borderline signature
/// breaks under one and not the other.
pub fn emit_func_vis(ctx: &EmitCtx, func: &Func, vis_prefix: &str) -> DResult<String> {
    let name = ctx.func_name(func.id)?.to_owned();

    // ── Entry-point Task.run elision ──────────────────────────────────────────
    // When `ipe_main` is `main = someTask |> Task.run`, the lowerer sets:
    //   func.body = Call(TaskRun | TaskPerform, [inner_task])
    //   func.ret  = IrType::Result(IrType::Error, A)
    //
    // The Rust epilogue calls `block_on(ipe_main())`, which requires `ipe_main`
    // to return `IpeTask<A>` (an unevaluated future), NOT `IpeResult<E, A>`.
    // Elide the outer `task_run(...)` wrapper: use the inner task expression as
    // the body and convert the return type from `Result(Error, A)` to `Task(A)`.
    //
    // This is not always a FLAT `Call(TaskRun, …)` body — the Ipe.Terminal /
    // Ipe.Web `argv`-dispatch idiom branches on `System.args` before picking which
    // app to run, e.g. `main = case List.head argsList of Just "live" -> Web.app
    // cfg |> Task.run; _ -> Terminal.appScreen cfg |> Task.run`. Every arm still
    // tail-calls
    // `Task.run`, so the SAME elision must apply — otherwise `ipe_main` keeps
    // its `IpeResult<E, A>` return type and `block_on(ipe_main())` mismatches
    // exactly as the flat case would (a real SEAL violation found on
    // `examples/24-tui-kitchen-sink`, BACKLOG "24-tui-kitchen-sink").
    // `elide_task_run_tail` recurses through every tail-position control-flow
    // construct (`Match` / `If` / `Let` / `Destructure`) and elides ONLY when
    // EVERY leaf in tail position is a `Task.run` / `Task.perform` call — a
    // partial elision is never produced, so the rewritten body always has a
    // single uniform `Task<A>` shape.
    let elided: Option<(Expr, IrType)> = if name == "ipe_main"
        && let IrType::Result(_, ok_ty) = &func.ret
    {
        elide_task_run_tail(&func.body).map(|body| (body, IrType::Task(ok_ty.clone())))
    } else {
        None
    };
    let (body_expr, elided_ret): (&Expr, Option<IrType>) = match &elided {
        Some((body, ret)) => (body, Some(ret.clone())),
        None => (&func.body, None),
    };

    // ── ipe_main synchronous-body wrap ────────────────────────────────────────
    // When ipe_main was NOT elided, `block_on(ipe_main())` still needs
    // `IpeTask<A>`. See `ipe_main_wrap_decision`'s doc comment for the full
    // rationale (the CLI `task_run()`-calls idiom AND Finding B's mixed-arm
    // sibling gap).
    let (ipe_main_wrap_unit, ipe_main_wrap_result_ok_ty) =
        ipe_main_wrap_decision(&name, elided_ret.as_ref(), &func.ret);
    let ipe_main_wrap = ipe_main_wrap_unit || ipe_main_wrap_result_ok_ty.is_some();
    let wrapped_task_owned: Option<IrType> = if ipe_main_wrap_unit {
        Some(IrType::Task(Box::new(IrType::Unit)))
    } else {
        ipe_main_wrap_result_ok_ty
    };
    let ret_ty: &IrType = wrapped_task_owned
        .as_ref()
        .unwrap_or_else(|| elided_ret.as_ref().unwrap_or(&func.ret));

    // The generic scope resolves an `IrType::Generic` to its positional Rust
    // name; only the variable symbols participate, so project them out of the
    // `(Symbol, BoundSet)` pairs.
    let scope_syms: Vec<Symbol> = func.type_params.iter().map(|(sym, _)| *sym).collect();
    let generics = GenericScope::new(&scope_syms);

    let ret_is_task = matches!(ret_ty, IrType::Task(_));

    let mut params = Vec::with_capacity(func.params.len());
    for (param, ty) in &func.params {
        params.push(format!(
            "{}: {}",
            ctx.emit_ident(*param)?,
            render_type(ctx, ty, generics)?
        ));
    }
    let ret = render_type(ctx, ret_ty, generics)?;

    // M is inferred bottom-up from concrete element/attrs types
    // propagated by the region-type–sourced lowerer; `generics` is used
    // directly.
    // TCO: a `TailLoop` body emits `let mut`-shadowed params + a
    // `loop { … }` whose interior ends only in `return`/`continue`. Mutability is
    // introduced ONLY by the local `let mut p = p;` shadow, so the public `fn`
    // signature stays byte-identical to the non-TCO form (load-bearing for
    // `FuncValue` boxing / trait-object slots). The loop types as `!` (it never
    // falls through), so it unifies with any `-> R` — no `break value`. A
    // non-`TailLoop` body (the common case) routes to the ordinary value emitter,
    // which is exhaustive and fail-closed for any stray TCO node.
    let body = if ipe_main_wrap_unit {
        // Wrap the synchronous body so ipe_main returns IpeTask<()>; the
        // body's own (unit) value is discarded, only its side effects matter.
        let inner = emit_body_native(ctx, body_expr, generics)?;
        format!("let _r = {{ {inner} }};\n    task_succeed(())")
    } else if ipe_main_wrap {
        // Mixed-arm Task.run-elision-declined wrap (Finding B): the body
        // already evaluates synchronously to a `Result e a` — carry that
        // ACTUAL value into an already-resolved `IpeTask<a>` rather than
        // discarding it, so `fn main`'s Ok/Err match sees the real outcome.
        let inner = emit_body_native(ctx, body_expr, generics)?;
        format!("task_from_result({{ {inner} }})")
    } else {
        match body_expr {
            Expr::TailLoop {
                params: loop_params,
                body: loop_body,
            } => {
                let mut shadows = String::new();
                for (param, _ty) in loop_params {
                    let p = ctx.emit_ident(*param)?;
                    write!(shadows, "let mut {p} = {p};\n    ").map_err(|e| {
                        Diagnostic::CompilerBug {
                            where_: "ipe_backend_rust::emit_func",
                            detail: format!("writing TCO param shadow failed: {e}"),
                        }
                    })?;
                }
                let inner = emit_expr_tail(ctx, loop_body, 2, 1, generics, loop_params)?;
                format!("{shadows}loop {{\n{inner}\n    }}")
            }
            _ => emit_body_native(ctx, body_expr, generics)?,
        }
    };

    // the IpeRow bound (for a wildcard `any` param flowing into a
    // `Db.get*` accessor) is decided STRUCTURALLY at lowering time and carried
    // in the param's `BoundSet` — the generic clause just renders the BoundSet.
    let generic_clause = render_fn_generics(func, ret_is_task);

    // A zero-parameter top-level binding is a CAF (constant applicative form) — a
    // shared VALUE, not a function. Ipê (like Elm) evaluates it once and shares
    // the result; emitting the body inline re-evaluates it on every reference,
    // which reallocates a fresh value per use and, for a binding whose body
    // reads live runtime state, can observe a different value each time. Emit the
    // body behind a lazily-initialised, thread-safe cell so first use evaluates
    // it exactly once and every later use returns a clone of that one value.
    //
    // The gate is deliberately conservative (fail closed): a static cell requires
    // the value type to be `Sync + Send + Clone + 'static`, and the closure must
    // capture nothing type-parametric, so the wrapper applies only to a
    // monomorphic CAF whose return type is a plain shareable data type
    // ([`is_share_once_safe`]). `ipe_main` is excluded — the epilogue's
    // `block_on(ipe_main())` needs a fresh future each call. Every other binding
    // keeps the direct inline emission.
    let is_caf = func.params.is_empty()
        && func.type_params.is_empty()
        && name != "ipe_main"
        && is_share_once_safe(ret_ty);
    let body = if is_caf {
        let call_line = emit_caf_get_or_init(ctx, body_expr, generics)?;
        format!(
            "static CELL: std::sync::OnceLock<{ret}> = std::sync::OnceLock::new();\n    \
             {call_line}"
        )
    } else {
        body
    };

    let signature = render_fn_signature(vis_prefix, &name, &generic_clause, &params, &ret);
    Ok(format!("{signature} {{\n    {body}\n}}\n"))
}

/// Is `ty` a value type that a top-level CAF may share through a `static`
/// [`std::sync::OnceLock`] cell — i.e. unconditionally `Clone + Send + Sync +
/// 'static`?
///
/// A function-local `static OnceLock<T>` requires `T: Sync`, `get_or_init`
/// stores the value for the process lifetime (`'static`), and the emitted fn
/// returns `T` by value so the shared value must be `Clone`. This recognises
/// only the plain immutable data core plus structural composites built from it —
/// every leaf whose Rust rendering is known to satisfy all three bounds. It
/// fails closed: any effectful, opaque-handle, function-carrying, task, or
/// type-parametric leaf makes the whole type ineligible, so the caller keeps the
/// direct inline emission for that binding. A `Box<dyn Fn>` carrier
/// ([`IrType::Fun`]/[`IrType::FnOnceChain`]) is neither `Sync` nor `Clone`; an
/// [`IrType::Task`] future is single-poll and not `Sync`; an
/// [`IrType::Generic`] cannot appear in a `static` type at all.
fn is_share_once_safe(ty: &IrType) -> bool {
    match ty {
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes => true,
        IrType::Maybe(inner) | IrType::List(inner) | IrType::Set(inner) => {
            is_share_once_safe(inner)
        }
        IrType::Result(a, b) | IrType::Dict(a, b) => is_share_once_safe(a) && is_share_once_safe(b),
        IrType::Tuple(items) => items.iter().all(is_share_once_safe),
        // A closed record carries its whole field-type set, so recursing over it
        // is complete: a field holding a `Box<dyn Fn>` (`Fun`) or any other
        // non-shareable leaf makes the record ineligible.
        IrType::Record(fields) => fields.values().all(is_share_once_safe),
        // Everything else keeps the direct inline emission — Task/Cmd/Sub futures
        // and command descriptors, `Box<dyn Fn>` function carriers, opaque runtime
        // handles (Db, Decoder, server/web/UI/websocket types), the
        // Json/Decimal/Order/Error family, and `Generic` type variables. A user
        // `Enum` is excluded too: its `IrType` exposes only the type ARGUMENTS,
        // not the variant field types, so a variant carrying a `Box<dyn Fn>`
        // (neither `Send`/`Sync` nor `Clone`) cannot be ruled out from the type
        // alone — the conservative choice is to leave every enum-typed CAF inline.
        _ => false,
    }
}

/// Render a function signature `pub fn NAME<GEN>(PARAMS) -> RET`, laid out to the
/// exact bytes `rustfmt --edition 2024 --style-edition 2024` produces — flat when
/// it fits, otherwise broken to match rustfmt's fn-signature layout. The returned
/// string has NO trailing ` {`; the caller appends the body block.
///
/// `rustfmt`'s three tiers, keyed off `max_width` (100), reproduced here because
/// the native body path removed the whole-file `rustfmt` pass that used to reflow
/// these lines:
///
/// * **flat** — the whole `pub fn NAME<GEN>(P0, P1, …) -> RET {` line (counting
///   the trailing ` {` the caller adds) is at most 100 columns.
/// * **params broken** — otherwise, if the `pub fn NAME<GEN>(` opening line fits:
///   each parameter on its own line indented four columns with a trailing comma
///   (every parameter, including the last), then `) -> RET {` at column 0.
/// * **generics broken** — otherwise each generic on its own line indented four
///   columns with a trailing comma, `>(` at column 0, then the params-broken body.
///
/// The ` {` the caller appends is included in every fit test (rustfmt measures the
/// opening brace as part of the line), so the flat/broken decision matches the
/// formatter's own boundary — verified flat at width 100, broken at 101.
fn render_fn_signature(
    vis_prefix: &str,
    name: &str,
    generic_clause: &str,
    params: &[String],
    ret: &str,
) -> String {
    // `rustfmt` `max_width`; `BRACE` is the trailing ` {` the caller appends after
    // the return type, which rustfmt counts as part of the signature line.
    //
    // `vis_prefix` is the leading `pub fn ` / `pub(crate) fn ` the signature carries
    // BEFORE the name. It is threaded here — rather than prepended by the caller — so
    // the flat-vs-broken width decision measures against the SAME prefix the emitted
    // line carries: a split-module `pub(crate) fn ` is seven columns wider than the
    // single-file `pub fn `, so a signature that fits flat under `pub fn ` may still
    // overflow under `pub(crate) fn ` and must break.
    const MAX_WIDTH: usize = 100;
    const BRACE: usize = 2;
    let flat = format!(
        "{vis_prefix}{name}{generic_clause}({}) -> {ret}",
        params.join(", ")
    );
    if flat.len() + BRACE <= MAX_WIDTH {
        return flat;
    }

    // A zero-parameter signature never breaks its empty `()` — `rustfmt` keeps
    // `NAME() -> ` glued and instead wraps the RETURN TYPE at its outermost angle
    // brackets: `NAME() -> Ptr<\n    Inner,\n>`. Only a return type that is itself a
    // single angle-bracketed generic can wrap; anything else (or a return type whose
    // opening line still overflows) stays on the one line `rustfmt` cannot shorten.
    if params.is_empty() {
        let open = format!("{vis_prefix}{name}{generic_clause}() -> ");
        if let Some(wrapped) = wrap_return_type(&open, ret) {
            return wrapped;
        }
        return format!("{open}{ret}");
    }

    // The `pub fn NAME<GEN>(` opening line, with generics still flat.
    let params_open = format!("{vis_prefix}{name}{generic_clause}(");
    let broken_params = || {
        let mut out = String::new();
        for p in params {
            out.push_str("\n    ");
            out.push_str(p);
            out.push(',');
        }
        out.push_str("\n) -> ");
        out.push_str(ret);
        out
    };
    if params_open.len() <= MAX_WIDTH {
        return format!("{params_open}{}", broken_params());
    }

    // Both the flat and params-broken openings overflow: break the generic
    // clause too. `generic_clause` is `<T1: …, T2: …>` (or empty, but an empty
    // clause cannot overflow the opening line, so this branch is generics-only).
    let inner = generic_clause
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(generic_clause);
    let mut out = format!("{vis_prefix}{name}<");
    for g in inner.split(", ") {
        out.push_str("\n    ");
        out.push_str(g);
        out.push(',');
    }
    out.push_str("\n>(");
    out.push_str(&broken_params());
    out
}

/// Wrap a zero-parameter signature's RETURN TYPE at its outermost angle brackets when
/// the flat `open` + `ret` line overflows, matching `rustfmt`: `NAME() -> Ptr<\n
/// Inner,\n>`. Returns `None` when the return type is not a single top-level
/// angle-bracketed generic (`Ptr<…>` with the `<` after a path and the matching `>`
/// at the end) — `rustfmt` has no shorter layout for such a type, so the caller keeps
/// the flat line. The `Inner` is placed at one indent step with a trailing comma and
/// the `>` dedented to column 0, the same one-per-line break the params path uses.
fn wrap_return_type(open: &str, ret: &str) -> Option<String> {
    const MAX_WIDTH: usize = 100;
    const BRACE: usize = 2;
    if open.len() + ret.len() + BRACE <= MAX_WIDTH {
        return None;
    }
    // A single top-level generic: `Head<Inner>` where the first `<` opens the sole
    // bracket group and the matching `>` is the final character. A leading `Box<` /
    // `Decoder<` head with the whole remainder as one `Inner` argument.
    let lt = ret.find('<')?;
    if !ret.ends_with('>') {
        return None;
    }
    let head = &ret[..lt];
    let inner = &ret[lt + 1..ret.len() - 1];
    // The head must be a plain path (no earlier bracket / comma), and the wrapped
    // opening line `NAME() -> Head<` must itself fit; otherwise no shortening applies.
    if head.contains([',', '<', '>', '(', ')']) || open.len() + head.len() + 1 + BRACE > MAX_WIDTH {
        return None;
    }
    Some(format!("{open}{head}<\n    {inner},\n>"))
}

/// Render the CAF `CELL.get_or_init(|| body).clone()` line with the native Doc
/// path, so the closure body's braces are elided when the line fits the width —
/// matching `rustfmt`'s closure-body rule (`move |_| expr` when it fits, `move |_|
/// { … }` when it breaks). The returned string has no leading whitespace; it is
/// spliced after the `\n    ` the caller writes.
///
/// [`Doc::BraceBody`] carries the closure body's braces as SEAL-visible leaves
/// (the string emitter always writes `|| { body }`) but omits them from the render
/// when the body fits flat — matching the golden's `|| expr` form exactly. The
/// outer [`Doc::CallArgs`] tests the full `CELL.get_or_init(|| body).clone()` line
/// against `max_width` (100) and `fn_call_width` (60) before choosing flat.
fn emit_caf_get_or_init(
    ctx: &EmitCtx,
    body_expr: &Expr,
    generics: GenericScope,
) -> DResult<String> {
    let body_doc = crate::emit_doc::build_doc(ctx, body_expr, 1, 0, generics)?;
    // `|| BraceBody(body)` — the single closure argument. `BraceBody` renders
    // the body WITHOUT braces when it fits flat, and WITH braces on a new line
    // when it does not, matching `rustfmt`'s closure body layout.
    let closure_arg = Doc::concat(vec![Doc::text("|| "), Doc::brace_body(body_doc)]);
    // `CELL.get_or_init(closure)` — a single-argument function call whose sole
    // closure argument `rustfmt` combines onto the call head: `get_or_init(|| {`
    // on one line, the body broken inside, `})` at the call's indent, no trailing
    // comma.
    let receiver = Doc::call_args(
        Doc::text("CELL.get_or_init("),
        vec![closure_arg],
        Doc::text(")"),
        // A function-call argument list keeps a trailing comma when it breaks.
        true,
    );
    // `.clone()` glued when the receiver stays single-line, dropped onto its own
    // line at the call's indent when the receiver's closure body broke — `rustfmt`'s
    // method-chain layout after a multiline receiver.
    let call = Doc::method_chain(receiver, Doc::text(".clone()"));
    // Seeded at column 4 (fn-body indent) so the fit test measures from the
    // position where the line starts in the emitted file.
    Ok(render_seeded(&call, RenderConfig::default(), 4, 4))
}

/// Render a value body expression to the exact bytes a `rustfmt`-formatted
/// function body carries, laid out by the native [`crate::emit_doc::build_doc`] +
/// [`crate::render::render_seeded`] path instead of the flat string emitter.
///
/// The body opens at column 4 — right after the four-space prefix the caller
/// writes before `{body}` in `pub fn … {\n    {body}\n}` — and every line it
/// breaks onto nests from the fn-body block indent (4 columns). This is the same
/// framing the whole-corpus native-vs-legacy sweep proved byte-identical to
/// `emit_expr_at` + `rustfmt` for every function body in the corpus, so splicing
/// its result makes the emitted body `rustfmt`-clean by construction.
///
/// `build_doc` is threaded the fn-body context the string emitter used: block
/// indent 1, IR depth 0.
fn emit_body_native(ctx: &EmitCtx, body_expr: &Expr, generics: GenericScope) -> DResult<String> {
    let doc = crate::emit_doc::build_doc(ctx, body_expr, 1, 0, generics)?;
    Ok(render_seeded(&doc, RenderConfig::default(), 4, 4))
}

/// Render a function's generic clause `<T1, T2: <bounds>, ..>` — one entry per
/// quantified variable in declaration order, the position fixing its `T{i+1}`
/// name. Empty string for a monomorphic function.
///
/// `Clone` is always included: Ipê has value semantics so every type must be
/// cloneable (field reads emit `.clone()` to prevent partial-move errors). For
/// `Copy` types (`i64`, `bool`, …) the bound is trivially satisfied.
///
/// `Send + 'static` is injected only when `ret_is_task`: futures require their
/// captured values to be `Send + 'static`, but plain record/ADT-returning
/// functions have no such requirement. Adding the bounds unconditionally would
/// over-constrain callers of pure record-constructors (e.g. `wrap : a -> {
/// value : a }` must accept any `Clone` type, not only `Send + 'static` ones).
///
/// The `IpeRow` bound (for a wildcard `any` param that flows into a
/// `Db.get*` accessor) is already recorded in the relevant param's [`BoundSet`]
/// by the lowerer's structural IR walk (`ipe_lower`'s `apply_db_row_bounds` /
/// `body_calls_db_get_on_param`), so this function simply renders whatever
/// bounds each param carries.
fn render_fn_generics(func: &Func, ret_is_task: bool) -> String {
    if func.type_params.is_empty() {
        return String::new();
    }

    let entries = func
        .type_params
        .iter()
        .enumerate()
        .map(|(i, (sym, bounds))| {
            let n = i.saturating_add(1);
            let bounds = *bounds;
            // Always inject `Clone` — field reads emit `.clone()` to prevent
            // partial-move errors. The solver's BoundSet may already carry it,
            // but `with_clone()` is idempotent so this is safe.
            let clause = render_bounds(bounds.with_clone(), n);
            // `render_bounds` returns ": Clone[+ ...]" or "".
            //
            // Append `Send + 'static` when the return type is a task OR this
            // type variable appears inside a first-class-function-value
            // parameter. A `Fun` / `FnOnceChain` param renders as
            // `Box<dyn Fn(..) -> R + Send + Sync + 'static>` (see
            // `emit_types::render_type`), which pins EVERY type it mentions to
            // `'static`; without a `T{n}: 'static` bound the boxed param — or an
            // `Arc`-wrap of it into a UI/Live event slot (`arc_callback_wrap`) —
            // is an E0310 (`T{n} may not live long enough`), the deeper layer of
            // the `26-ui-showcase` seal break (a generic-over-Msg helper taking
            // an `onEdit : String -> msg` callback and forwarding it into
            // `input_multiline_`). Same `Send + 'static` treatment as the task
            // path, gated to exactly the vars that need it so pure
            // record/ADT-returning callers stay unconstrained.
            let needs_static = ret_is_task || type_var_in_fn_param(func, *sym);
            if needs_static {
                if clause.is_empty() {
                    format!("T{n}: Clone + Send + 'static")
                } else {
                    format!("T{n}{clause} + Send + 'static")
                }
            } else {
                // Pure function (returns a record, ADT, scalar …): Clone suffices.
                if clause.is_empty() {
                    format!("T{n}: Clone")
                } else {
                    format!("T{n}{clause}")
                }
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("<{entries}>")
}

/// `true` if the type variable `sym` appears anywhere inside a
/// first-class-function-value (`IrType::Fun` / `IrType::FnOnceChain`)
/// parameter of `func`.
///
/// Such a parameter is emitted as a boxed `dyn Fn` trait object carrying a
/// `+ 'static` bound (`emit_types::render_type`), so any type variable it
/// mentions must itself be `'static`. This predicate drives the `Send +
/// 'static` bound injection in [`render_fn_generics`] for exactly those
/// variables — narrower than the blanket `ret_is_task` gate so that a pure,
/// non-callback-taking generic function keeps a bare `Clone` bound.
fn type_var_in_fn_param(func: &Func, sym: Symbol) -> bool {
    func.params
        .iter()
        .any(|(_, ty)| ty_mentions_var_under_fn(ty, sym, false))
}

/// Walk `ty`, returning `true` if `IrType::Generic(sym)` occurs while `under_fn`
/// is set (i.e. inside a `Fun` / `FnOnceChain` sub-tree). Once a function-typed
/// node is entered, `under_fn` stays set for the whole sub-tree — the entire
/// boxed trait object is `'static`, so every variable it names needs the bound.
fn ty_mentions_var_under_fn(ty: &IrType, sym: Symbol, under_fn: bool) -> bool {
    match ty {
        IrType::Generic(s) => under_fn && *s == sym,
        IrType::Fun(params, ret) | IrType::FnOnceChain(params, ret) => {
            params
                .iter()
                .any(|p| ty_mentions_var_under_fn(p, sym, true))
                || ty_mentions_var_under_fn(ret, sym, true)
        }
        IrType::Task(inner)
        | IrType::Maybe(inner)
        | IrType::List(inner)
        | IrType::Set(inner)
        | IrType::Decoder(inner)
        | IrType::Cmd(inner)
        | IrType::Sub(inner)
        | IrType::WebRoute(inner) => ty_mentions_var_under_fn(inner, sym, under_fn),
        IrType::Result(a, b) | IrType::Dict(a, b) => {
            ty_mentions_var_under_fn(a, sym, under_fn) || ty_mentions_var_under_fn(b, sym, under_fn)
        }
        IrType::Tuple(items) => items
            .iter()
            .any(|t| ty_mentions_var_under_fn(t, sym, under_fn)),
        IrType::Enum { args, .. } => args
            .iter()
            .any(|t| ty_mentions_var_under_fn(t, sym, under_fn)),
        IrType::Record(fields) => fields
            .values()
            .any(|t| ty_mentions_var_under_fn(t, sym, under_fn)),
        _ => false,
    }
}
