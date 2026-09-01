//! Phase-2 partial evaluation — fold a pure literal builder pipeline to a
//! constant.
//!
//! A pure builder pipeline whose every leaf is a literal
//! (`defaultSpec "spin" |> withDuration 300 |> withEasing easeInOut |>
//! withKeyframes […]`, rendered through `buildShorthandTail` /
//! `buildKeyframesBody`) computes a value that does not depend on the `Model`,
//! any effect, or any run-time input. [`fold_const`] evaluates such an
//! expression to a [`ConstValue`] IFF every sub-expression is a literal OR an
//! application of a PROVEN-PURE function over already-constant arguments,
//! returning `None` the instant it meets anything else (a `Var` it cannot
//! resolve, a `Model` field access, an impure kernel, a non-constant argument).
//!
//! The point of folding early: a folded [`ConstValue::Str`] substituted back
//! into the kernel-call argument makes the argument a DIRECT [`Expr::Str`], so
//! the appearance-literal registry ([`crate::emit_ui_plan::appearance_literal_args`])
//! hot-swaps it under `IPE_WATCH_HOT_APPEARANCE` exactly as it already
//! hot-swaps a hand-written literal — no registry change. The folded literal
//! then flows through the SAME emit and render sink as an unfolded one (it is
//! the same value, computed at compile time instead of run time), so the sink
//! gates (`SafeCssValue` / `sink_safe_keyframes_body`) still run at render:
//! dev == prod, no sink bypass.
//!
//! ## Soundness
//!
//! * Evaluation is FUEL-BOUNDED: a step counter decremented on every
//!   sub-expression visit; exhaustion returns `None` rather than looping at
//!   compile time (the "bounded by construction" rule). A pipeline that would
//!   not terminate — or is merely larger than the budget — recompiles as an
//!   unfolded computed argument, which is correct, just slower.
//! * A function is evaluated through its body ONLY when it is proven pure: a
//!   [`KernelFn`] with no security-relevant [`Capability`] (the reused kernel
//!   analysis), or a user stdlib function on the explicit whitelist below. A
//!   `Model`-dependent, effectful, or non-constant argument never folds, so a
//!   `Model`-dependent duration correctly recompiles.
//!
//! ## Whitelist scope
//!
//! There is no general whole-program purity result for USER functions here, so
//! evaluation through a user function body is gated by an explicit whitelist:
//! the value builders of `Ipe.Ui.Animation` and `Ipe.Css`. This is the current
//! scope — the two appearance-builder modules whose pipelines the appearance
//! hot-swap targets. Any user function outside the whitelist yields `None` (the
//! conservative recompile fallback), never an unsound fold.

use std::collections::BTreeMap;

use ipe_intern::{Interner, Symbol};
use ipe_ir::{BinOp, Callee, Expr, Func, FuncId, KernelFn, ModPath, Pat};

/// The starting evaluation budget: the maximum number of sub-expression visits
/// one [`fold_const`] call may perform before giving up with `None`. Sized
/// generously for a realistic appearance pipeline (a `withKeyframes` list of a
/// few frames, each a handful of props) while staying finite — a pathological
/// or genuinely large input exhausts the budget and recompiles unfolded.
const FUEL: u32 = 100_000;

/// The maximum native recursion depth [`eval`] may reach before returning
/// `None`. Fuel bounds total WORK; this separately bounds DEPTH, so a
/// pathologically deep (but under-fuel) nested expression returns `None`
/// instead of overflowing the compile-time native stack. A realistic
/// appearance pipeline nests only a handful of levels, far under this bound.
const MAX_DEPTH: u32 = 512;

/// A compile-time constant produced by evaluating a pure literal pipeline.
///
/// Covers exactly the literal kinds the whitelisted pure builders produce and
/// consume: scalars (`Int` / `Float` / `Str` / `Bool`), homogeneous `List`s,
/// records (field name → value), and tagged-union constructor values. A value
/// the evaluator cannot represent here is never folded — [`fold_const`] returns
/// `None` instead of an approximate constant.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    /// A list value — its elements in order.
    List(Vec<ConstValue>),
    /// A record value — field name → constant, keyed for order-independent
    /// field lookup during an `Access`.
    Record(BTreeMap<Symbol, ConstValue>),
    /// A tagged-union constructor value: the constructor name and its (already
    /// constant) payload arguments in declared order. A nullary constructor has
    /// an empty payload.
    Ctor {
        variant: Symbol,
        args: Vec<ConstValue>,
    },
    /// A tuple value — its elements in order.
    Tuple(Vec<ConstValue>),
}

impl ConstValue {
    /// The [`Expr`] literal this constant substitutes back into the IR, when it
    /// has a direct literal form. A folded [`ConstValue::Str`] becomes an
    /// [`Expr::Str`] the downstream emit and the appearance registry treat
    /// exactly as a hand-written literal. Only the scalar kinds have a direct
    /// literal `Expr`; a folded list / record / ctor is an intermediate value
    /// on the way to a scalar, never itself substituted, so it returns `None`.
    #[must_use]
    pub fn to_literal_expr(&self) -> Option<Expr> {
        match self {
            ConstValue::Int(n) => Some(Expr::Int(*n)),
            ConstValue::Float(f) => Some(Expr::Float(*f)),
            ConstValue::Str(s) => Some(Expr::Str(s.clone())),
            ConstValue::Bool(b) => Some(Expr::Bool(*b)),
            ConstValue::List(_)
            | ConstValue::Record(_)
            | ConstValue::Ctor { .. }
            | ConstValue::Tuple(_) => None,
        }
    }
}

/// The evaluation environment: the whole-program function table (for evaluating
/// through a whitelisted user-function or kernel-wrapper body), the interner
/// (to resolve a module / function name against the whitelist), and the
/// per-call local bindings (a parameter symbol → its already-constant value).
pub struct FoldEnv<'a> {
    funcs: &'a BTreeMap<FuncId, &'a Func>,
    interner: &'a Interner,
    locals: BTreeMap<Symbol, ConstValue>,
}

impl<'a> FoldEnv<'a> {
    /// Build an environment over a whole-program `FuncId → Func` table and the
    /// interner. Locals start empty; a function body evaluation binds the
    /// parameters before descending.
    #[must_use]
    pub fn new(funcs: &'a BTreeMap<FuncId, &'a Func>, interner: &'a Interner) -> Self {
        Self {
            funcs,
            interner,
            locals: BTreeMap::new(),
        }
    }
}

/// Evaluate `expr` to a [`ConstValue`] if it is a pure computation over
/// compile-time constants, else `None`.
///
/// This is the public entry: it seeds the fuel budget and delegates to the
/// recursive [`eval`]. `None` means "not a compile-time constant" — either
/// genuinely (a `Model`-dependent or effectful sub-expression) or
/// conservatively (an expression shape or function the evaluator does not
/// cover, or fuel exhaustion). Every `None` path is safe: the caller leaves the
/// expression unfolded and it emits exactly as before.
#[must_use]
pub fn fold_const(expr: &Expr, env: &FoldEnv) -> Option<ConstValue> {
    let mut budget = Budget {
        fuel: FUEL,
        depth: 0,
    };
    eval(expr, env, &mut budget)
}

/// The two hard bounds on evaluation, threaded through the recursion: `fuel`
/// caps total sub-expression visits (work), `depth` caps native recursion
/// depth (stack). Either bound reaching its limit aborts the fold with `None`.
struct Budget {
    fuel: u32,
    depth: u32,
}

/// The fuel- and depth-bounded recursive evaluator. Every call decrements
/// `fuel` and returns `None` on exhaustion (total visits ≤ [`FUEL`]); it also
/// returns `None` once native recursion reaches [`MAX_DEPTH`]. Together the two
/// bounds guarantee the evaluator can neither loop nor overflow the compile-time
/// stack, regardless of the input expression.
fn eval(expr: &Expr, env: &FoldEnv, budget: &mut Budget) -> Option<ConstValue> {
    if budget.fuel == 0 || budget.depth >= MAX_DEPTH {
        return None;
    }
    budget.fuel -= 1;
    budget.depth += 1;
    let result = eval_inner(expr, env, budget);
    budget.depth -= 1;
    result
}

/// The per-node evaluation body, entered only after [`eval`] has charged one
/// unit of fuel and one level of depth (and restored the depth on return).
fn eval_inner(expr: &Expr, env: &FoldEnv, budget: &mut Budget) -> Option<ConstValue> {
    match expr {
        Expr::Int(n) => Some(ConstValue::Int(*n)),
        Expr::Float(f) => Some(ConstValue::Float(*f)),
        Expr::Str(s) => Some(ConstValue::Str(s.clone())),
        Expr::Bool(b) => Some(ConstValue::Bool(*b)),

        // A local parameter bound to an already-constant argument resolves to
        // that value; any other variable (a free variable, a `Model` binder, a
        // captured symbol we did not bind) is not a compile-time constant.
        Expr::Var(sym) | Expr::CloneVar(sym) => env.locals.get(sym).cloned(),

        Expr::List { items, .. } => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(eval(it, env, budget)?);
            }
            Some(ConstValue::List(out))
        }

        Expr::Cons { head, tail } => {
            let head_v = eval(head, env, budget)?;
            let ConstValue::List(mut rest) = eval(tail, env, budget)? else {
                return None;
            };
            rest.insert(0, head_v);
            Some(ConstValue::List(rest))
        }

        Expr::Tuple(elems) => {
            let mut out = Vec::with_capacity(elems.len());
            for e in elems {
                out.push(eval(e, env, budget)?);
            }
            Some(ConstValue::Tuple(out))
        }

        Expr::Record { fields, .. } => {
            let mut map = BTreeMap::new();
            for (name, value) in fields {
                map.insert(*name, eval(value, env, budget)?);
            }
            Some(ConstValue::Record(map))
        }

        Expr::Update { record, fields } => {
            let ConstValue::Record(mut map) = eval(record, env, budget)? else {
                return None;
            };
            for (name, value) in fields {
                map.insert(*name, eval(value, env, budget)?);
            }
            Some(ConstValue::Record(map))
        }

        Expr::Access { record, field, .. } => {
            let ConstValue::Record(map) = eval(record, env, budget)? else {
                return None;
            };
            map.get(field).cloned()
        }

        Expr::Ctor { variant, args, .. } => {
            let mut out = Vec::with_capacity(args.len());
            for a in args {
                out.push(eval(a, env, budget)?);
            }
            Some(ConstValue::Ctor {
                variant: *variant,
                args: out,
            })
        }

        Expr::BinOp { op, lhs, rhs } => {
            let l = eval(lhs, env, budget)?;
            let r = eval(rhs, env, budget)?;
            eval_binop(*op, &l, &r)
        }

        Expr::If { cond, then_, else_ } => match eval(cond, env, budget)? {
            ConstValue::Bool(true) => eval(then_, env, budget),
            ConstValue::Bool(false) => eval(else_, env, budget),
            _ => None,
        },

        Expr::Let { name, value, body } => {
            let v = eval(value, env, budget)?;
            eval_with_local(*name, v, body, env, budget)
        }

        Expr::Match(m) => eval_match(m, env, budget),

        Expr::Call { callee, args, .. } => eval_call(callee, args, env, budget),

        // Every remaining shape is either not a value (a tail-loop marker, a
        // task sequence), a first-class function value, or a form the evaluator
        // does not model. None of them is a compile-time scalar constant, so
        // folding conservatively stops here.
        _ => None,
    }
}

/// Evaluate `body` with one additional local binding in scope, restoring the
/// environment's locals afterwards. A fresh child map is cloned rather than
/// mutating the shared environment, so sibling evaluations never see a leaked
/// binding.
fn eval_with_local(
    name: Symbol,
    value: ConstValue,
    body: &Expr,
    env: &FoldEnv,
    budget: &mut Budget,
) -> Option<ConstValue> {
    let mut child_locals = env.locals.clone();
    child_locals.insert(name, value);
    let child = FoldEnv {
        funcs: env.funcs,
        interner: env.interner,
        locals: child_locals,
    };
    eval(body, &child, budget)
}

/// Evaluate a binary operation over two already-constant operands. Only the
/// total, deterministic operations the pure builders use are folded: string
/// append (`++`), integer/float arithmetic (wrapping / IEEE-754, matching the
/// runtime's total semantics), and the boolean / comparison operators. Integer
/// division by zero and any operand-kind mismatch yield `None` (unfolded), so a
/// fold never diverges from the runtime.
fn eval_binop(op: BinOp, l: &ConstValue, r: &ConstValue) -> Option<ConstValue> {
    use ConstValue::{Bool, Float, Int, Str};
    match (op, l, r) {
        (BinOp::Append, Str(a), Str(b)) => Some(Str(format!("{a}{b}"))),

        (BinOp::IntAdd | BinOp::Add, Int(a), Int(b)) => Some(Int(a.wrapping_add(*b))),
        (BinOp::IntSub | BinOp::Sub, Int(a), Int(b)) => Some(Int(a.wrapping_sub(*b))),
        (BinOp::IntMul | BinOp::Mul, Int(a), Int(b)) => Some(Int(a.wrapping_mul(*b))),
        // Integer division is total in the runtime only through the checked
        // helper (`b == 0` and `MIN / -1` are guarded there); rather than
        // reproduce that guard's exact result, division by zero simply does not
        // fold. A non-zero divisor folds to the same wrapping quotient.
        (BinOp::IntDiv, Int(a), Int(b)) if *b != 0 => Some(Int(a.wrapping_div(*b))),

        (BinOp::FloatAdd | BinOp::Add, Float(a), Float(b)) => Some(Float(a + b)),
        (BinOp::FloatSub | BinOp::Sub, Float(a), Float(b)) => Some(Float(a - b)),
        (BinOp::FloatMul | BinOp::Mul, Float(a), Float(b)) => Some(Float(a * b)),
        (BinOp::Div, Float(a), Float(b)) => Some(Float(a / b)),

        (BinOp::And, Bool(a), Bool(b)) => Some(Bool(*a && *b)),
        (BinOp::Or, Bool(a), Bool(b)) => Some(Bool(*a || *b)),

        (BinOp::Eq, _, _) => Some(Bool(l == r)),
        (BinOp::Neq, _, _) => Some(Bool(l != r)),
        (BinOp::Lt, Int(a), Int(b)) => Some(Bool(a < b)),
        (BinOp::Gt, Int(a), Int(b)) => Some(Bool(a > b)),
        (BinOp::Le, Int(a), Int(b)) => Some(Bool(a <= b)),
        (BinOp::Ge, Int(a), Int(b)) => Some(Bool(a >= b)),

        _ => None,
    }
}

/// Evaluate a `case` expression: fold the scrutinee, then take the first arm
/// whose pattern matches (binding its variables), evaluate that arm's body.
/// An arm with a guard is only taken when the guard folds to `true`. A
/// scrutinee or pattern shape the matcher does not model yields `None`.
fn eval_match(m: &ipe_ir::Match, env: &FoldEnv, budget: &mut Budget) -> Option<ConstValue> {
    let scrut = eval(m.scrutinee(), env, budget)?;
    for arm in m.arms() {
        let mut bindings = BTreeMap::new();
        if match_pat(&arm.pat, &scrut, &mut bindings) {
            let mut child_locals = env.locals.clone();
            child_locals.append(&mut bindings);
            let child = FoldEnv {
                funcs: env.funcs,
                interner: env.interner,
                locals: child_locals,
            };
            if let Some(guard) = &arm.guard {
                match eval(guard, &child, budget) {
                    Some(ConstValue::Bool(true)) => {}
                    // A `false` guard falls through to the next arm; an
                    // unfoldable guard aborts the whole fold (we cannot prove
                    // which arm the runtime takes).
                    Some(ConstValue::Bool(false)) => continue,
                    _ => return None,
                }
            }
            return eval(&arm.body, &child, budget);
        }
    }
    None
}

/// Try to match a constant value against a pattern, collecting variable
/// bindings on success. Returns `false` for a non-match (the caller tries the
/// next arm) and — conservatively — for any pattern shape the matcher does not
/// model, so an unmodelled pattern simply never matches rather than matching
/// unsoundly.
fn match_pat(pat: &Pat, value: &ConstValue, out: &mut BTreeMap<Symbol, ConstValue>) -> bool {
    match (pat, value) {
        (Pat::Wildcard, _) => true,
        (Pat::Var(sym), _) => {
            out.insert(*sym, value.clone());
            true
        }
        (Pat::Alias(inner, sym), _) => {
            out.insert(*sym, value.clone());
            match_pat(inner, value, out)
        }
        (Pat::Int(a), ConstValue::Int(b)) => a == b,
        (Pat::Bool(a), ConstValue::Bool(b)) => a == b,
        (Pat::Str(a), ConstValue::Str(b)) => a == b,
        (Pat::Ctor { variant, args, .. }, ConstValue::Ctor { variant: v, args: vs })
            if variant == v && args.len() == vs.len() =>
        {
            args.iter().zip(vs).all(|(p, v)| match_pat(p, v, out))
        }
        (Pat::Tuple(ps), ConstValue::Tuple(vs)) if ps.len() == vs.len() => {
            ps.iter().zip(vs).all(|(p, v)| match_pat(p, v, out))
        }
        (Pat::Record(entries), ConstValue::Record(map)) => entries
            .iter()
            .all(|(name, sub)| map.get(name).is_some_and(|v| match_pat(sub, v, out))),
        (Pat::Slice { prefix, rest }, ConstValue::List(items)) => match_slice(prefix, rest, items, out),
        _ => false,
    }
}

/// Match a slice pattern (`[]`, `[a, b]`, `x :: xs`) against a constant list.
fn match_slice(
    prefix: &[Pat],
    rest: &Option<Box<Pat>>,
    items: &[ConstValue],
    out: &mut BTreeMap<Symbol, ConstValue>,
) -> bool {
    match rest {
        // Closed, exact-length list pattern.
        None => {
            items.len() == prefix.len()
                && prefix.iter().zip(items).all(|(p, v)| match_pat(p, v, out))
        }
        // Open cons tail: at least `prefix.len()` elements; `rest` binds the
        // remainder as a list value.
        Some(tail) => {
            if items.len() < prefix.len() {
                return false;
            }
            let (head, remainder) = items.split_at(prefix.len());
            prefix.iter().zip(head).all(|(p, v)| match_pat(p, v, out))
                && match_pat(tail, &ConstValue::List(remainder.to_vec()), out)
        }
    }
}

/// Evaluate a `Call`: a kernel call folds through [`eval_kernel`] (only pure
/// kernels), a call to a whitelisted user function folds through its body with
/// the arguments bound as parameters. Anything else — an impure kernel, a
/// non-whitelisted user function, an FFI callee — yields `None`.
fn eval_call(
    callee: &Callee,
    args: &[Expr],
    env: &FoldEnv,
    budget: &mut Budget,
) -> Option<ConstValue> {
    // Evaluate every argument to a constant first; a single non-constant
    // argument means the call is not a compile-time constant.
    let mut arg_vals = Vec::with_capacity(args.len());
    for a in args {
        arg_vals.push(eval(a, env, budget)?);
    }

    match callee {
        Callee::Kernel(k) => eval_kernel(*k, &arg_vals),
        Callee::Func(id) => {
            let func = env.funcs.get(id)?;
            if !is_whitelisted_func(func, env.interner) {
                return None;
            }
            eval_func_body(func, &arg_vals, env, budget)
        }
        // A foreign FFI callee is never a compile-time constant.
        Callee::Ffi { .. } => None,
    }
}

/// Evaluate a whitelisted user function's body with its arguments bound to its
/// parameters. A parameter-count mismatch (a partial application reaching here)
/// aborts the fold.
fn eval_func_body(
    func: &Func,
    arg_vals: &[ConstValue],
    env: &FoldEnv,
    budget: &mut Budget,
) -> Option<ConstValue> {
    if func.params.len() != arg_vals.len() {
        return None;
    }
    let mut child_locals = BTreeMap::new();
    for ((param, _), value) in func.params.iter().zip(arg_vals) {
        child_locals.insert(*param, value.clone());
    }
    let child = FoldEnv {
        funcs: env.funcs,
        interner: env.interner,
        locals: child_locals,
    };
    eval(&func.body, &child, budget)
}

/// Whether a user function may be evaluated through its body.
///
/// There is no general whole-program purity result for user functions here, so
/// this is an EXPLICIT whitelist of the two appearance-builder modules whose
/// literal pipelines the appearance hot-swap targets: `Ipe.Ui.Animation` and
/// `Ipe.Css`. A function in any other module is not evaluated (its call folds
/// to `None`, recompiling unfolded). This is the current scope; widening it is
/// a deliberate, separately-audited act, not an accident of a general analysis.
fn is_whitelisted_func(func: &Func, interner: &Interner) -> bool {
    modpath_is(&func.home, &["Ipe", "Ui", "Animation"], interner)
        || modpath_is(&func.home, &["Ipe", "Css"], interner)
}

/// Whether a module path resolves, segment for segment, to `expected`.
fn modpath_is(home: &ModPath, expected: &[&str], interner: &Interner) -> bool {
    home.0.len() == expected.len()
        && home
            .0
            .iter()
            .zip(expected)
            .all(|(seg, want)| interner.resolve(*seg) == Some(*want))
}

/// Evaluate a pure kernel over already-constant arguments.
///
/// Gated on [`KernelFn::capability`] being `None` (no security-relevant effect
/// — the reused kernel analysis): an effectful kernel is never folded even if a
/// case below happened to match its arity. The set of kernels with a folded
/// implementation is the pure string / numeric builders the appearance
/// pipelines reach (`String.fromInt` / `fromFloat` / `append` / `concat` /
/// `join`). A pure kernel with no case here yields `None` — unfolded, never a
/// wrong constant.
fn eval_kernel(k: KernelFn, args: &[ConstValue]) -> Option<ConstValue> {
    use ConstValue::{Float, Int, List, Str};

    // A kernel carrying any security-relevant capability is effectful; never
    // fold it, regardless of the arms below.
    if k.capability().is_some() {
        return None;
    }

    match (k, args) {
        (KernelFn::StringFromInt, [Int(n)]) => Some(Str(string_from_int(*n))),
        (KernelFn::StringFromFloat, [Float(f)]) => Some(Str(string_from_float(*f))),
        (KernelFn::StringAppend, [Str(a), Str(b)]) => Some(Str(format!("{a}{b}"))),
        (KernelFn::StringConcat, [List(parts)]) => {
            let mut out = String::new();
            for p in parts {
                let Str(s) = p else { return None };
                out.push_str(s);
            }
            Some(Str(out))
        }
        (KernelFn::StringJoin, [Str(sep), List(parts)]) => {
            let mut pieces = Vec::with_capacity(parts.len());
            for p in parts {
                let Str(s) = p else { return None };
                pieces.push(s.as_str());
            }
            Some(Str(pieces.join(sep)))
        }
        _ => None,
    }
}

/// `String.fromInt`: the runtime's `ipe_runtime::string::string_from_int` is
/// `format!("{i}")`. Reproduced here byte-for-byte; the conformance test
/// asserts equality against the real runtime function.
fn string_from_int(i: i64) -> String {
    format!("{i}")
}

/// `String.fromFloat`: a byte-for-byte reproduction of the runtime's
/// `ipe_runtime::string::string_from_float` shortest-round-trip `'g'`-style
/// formatter. Reproduced (rather than depending on the whole runtime crate)
/// because the backend does not link the runtime; the conformance test asserts
/// this reproduction is byte-identical to the real runtime function across a
/// representative float set, so a drift is caught in CI, not in a user's build.
fn string_from_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-Inf" } else { "+Inf" }.to_string();
    }
    let neg = f.is_sign_negative();
    if f == 0.0 {
        return if neg { "-0" } else { "0" }.to_string();
    }

    let sci = format!("{:e}", f.abs());
    let Some((mantissa, exp_str)) = sci.split_once('e') else {
        return sci;
    };
    let sci_exp: i32 = exp_str.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();

    let dp = sci_exp + 1;
    let exp = dp - 1;

    if (-4..6).contains(&exp) {
        fmt_g_positional(neg, &digits, dp)
    } else {
        fmt_g_exponent(neg, &digits, exp)
    }
}

/// `'g'`'s `%e` rendering (shortest mode) — mirrors the runtime's
/// `fmt_g_exponent`: `d[.ddd]e±NN`, sign always present, exponent ≥ two digits.
fn fmt_g_exponent(neg: bool, digits: &str, exp: i32) -> String {
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    let mut chars = digits.chars();
    if let Some(first) = chars.next() {
        out.push(first);
    }
    let rest: String = chars.collect();
    if !rest.is_empty() {
        out.push('.');
        out.push_str(&rest);
    }
    out.push('e');
    let (sign, mag) = if exp < 0 { ('-', -exp) } else { ('+', exp) };
    out.push(sign);
    if mag < 10 {
        out.push('0');
    }
    out.push_str(&mag.to_string());
    out
}

/// `'g'`'s `%f` rendering (shortest mode) — mirrors the runtime's
/// `fmt_g_positional`: `ddd[.ddd]`, zero-padding the integer part and reading
/// fraction digits past the point.
fn fmt_g_positional(neg: bool, digits: &str, dp: i32) -> String {
    let bytes = digits.as_bytes();
    let nd = bytes.len() as i32;
    let frac = (nd - dp).max(0);
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if dp > 0 {
        let take = nd.min(dp);
        for i in 0..take {
            if let Some(&b) = bytes.get(i as usize) {
                out.push(b as char);
            }
        }
        for _ in take..dp {
            out.push('0');
        }
    } else {
        out.push('0');
    }
    if frac > 0 {
        out.push('.');
        for i in 0..frac {
            let j = dp + i;
            let ch = if j < 0 {
                b'0'
            } else {
                bytes.get(j as usize).copied().unwrap_or(b'0')
            };
            out.push(ch as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipe_diagnostics::DResult;
    use ipe_ir::{CallPin, IrType, OnFormKind};

    /// A `Call` to a kernel with no pin / form classification — the common shape.
    fn kernel_call(k: KernelFn, args: Vec<Expr>) -> Expr {
        Expr::Call {
            callee: Callee::Kernel(k),
            args,
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }
    }

    /// A whitelisted-or-not user `Func` with a single string parameter whose
    /// body is the identity over that parameter — the fixture the whitelist
    /// tests evaluate through.
    fn passthrough_func(home: ModPath, name: Symbol, param: Symbol) -> Func {
        Func {
            id: FuncId::from_raw(0),
            name,
            home,
            type_params: vec![],
            row_params: vec![],
            params: vec![(param, IrType::Str)],
            ret: IrType::Str,
            body: Expr::Var(param),
        }
    }

    #[test]
    fn nested_pure_literal_pipeline_folds_to_str() -> DResult<()> {
        // `String.join " " [String.fromInt 300, "ms", "linear"]` — a nested pure
        // pipeline of only literals folds to a single constant string.
        let interner = Interner::new();
        let funcs = BTreeMap::new();
        let env = FoldEnv::new(&funcs, &interner);

        let pipeline = kernel_call(
            KernelFn::StringJoin,
            vec![
                Expr::Str(" ".to_string()),
                Expr::List {
                    elem: IrType::Str,
                    items: vec![
                        kernel_call(KernelFn::StringFromInt, vec![Expr::Int(300)]),
                        Expr::Str("ms".to_string()),
                        Expr::Str("linear".to_string()),
                    ],
                },
            ],
        );

        assert_eq!(
            fold_const(&pipeline, &env),
            Some(ConstValue::Str("300 ms linear".to_string()))
        );
        Ok(())
    }

    #[test]
    fn string_append_pipeline_folds() -> DResult<()> {
        let interner = Interner::new();
        let funcs = BTreeMap::new();
        let env = FoldEnv::new(&funcs, &interner);

        // `(String.fromInt 300 ++ "ms")` via the `Append` BinOp.
        let expr = Expr::BinOp {
            op: BinOp::Append,
            lhs: Box::new(kernel_call(KernelFn::StringFromInt, vec![Expr::Int(300)])),
            rhs: Box::new(Expr::Str("ms".to_string())),
        };
        assert_eq!(
            fold_const(&expr, &env),
            Some(ConstValue::Str("300ms".to_string()))
        );
        Ok(())
    }

    #[test]
    fn pipeline_with_a_free_variable_does_not_fold() -> DResult<()> {
        // A pipeline reaching an unbound variable (a `Model` field binder, a
        // free parameter) is not a compile-time constant — folds to `None`, so
        // the argument recompiles rather than baking a wrong value.
        let mut interner = Interner::new();
        let model_dur = interner.intern("modelDuration")?;
        let funcs = BTreeMap::new();
        let env = FoldEnv::new(&funcs, &interner);

        let expr = Expr::BinOp {
            op: BinOp::Append,
            lhs: Box::new(kernel_call(
                KernelFn::StringFromInt,
                vec![Expr::Var(model_dur)],
            )),
            rhs: Box::new(Expr::Str("ms".to_string())),
        };
        assert_eq!(fold_const(&expr, &env), None);
        Ok(())
    }

    #[test]
    fn model_field_access_does_not_fold() -> DResult<()> {
        // `record.duration` where `record` is an unbound variable — a
        // `Model`-dependent read never folds.
        let mut interner = Interner::new();
        let model = interner.intern("model")?;
        let field = interner.intern("duration")?;
        let funcs = BTreeMap::new();
        let env = FoldEnv::new(&funcs, &interner);

        let expr = Expr::Access {
            record: Box::new(Expr::Var(model)),
            field,
            field_ty: IrType::Int,
        };
        assert_eq!(fold_const(&expr, &env), None);
        Ok(())
    }

    #[test]
    fn fuel_exhaustion_returns_none() {
        // A wide input whose total sub-expression count exceeds the fuel budget
        // exhausts it and returns `None` rather than folding — the "bounded by
        // construction" guard. The shape is a single flat list of more than
        // `FUEL` literal items (shallow, so it drains fuel by breadth, not by
        // native recursion depth), each item visited once.
        let interner = Interner::new();
        let funcs = BTreeMap::new();
        let env = FoldEnv::new(&funcs, &interner);

        let items: Vec<Expr> = (0..(FUEL + 10)).map(|_| Expr::Int(0)).collect();
        let expr = Expr::List {
            elem: IrType::Int,
            items,
        };
        assert_eq!(fold_const(&expr, &env), None);
    }

    #[test]
    fn non_whitelisted_user_function_does_not_fold() -> DResult<()> {
        // A user function outside the whitelist is not evaluated through its
        // body, even over constant arguments.
        let mut interner = Interner::new();
        let home = ModPath(vec![interner.intern("Main")?]);
        let name = interner.intern("helper")?;
        let param = interner.intern("x")?;
        let func = passthrough_func(home, name, param);
        let mut funcs = BTreeMap::new();
        funcs.insert(FuncId::from_raw(0), &func);
        let env = FoldEnv::new(&funcs, &interner);

        let call = Expr::Call {
            callee: Callee::Func(FuncId::from_raw(0)),
            args: vec![Expr::Str("hi".to_string())],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        };
        assert_eq!(fold_const(&call, &env), None);
        Ok(())
    }

    #[test]
    fn whitelisted_user_function_folds_through_body() -> DResult<()> {
        // A function in `Ipe.Ui.Animation` folds through its body: an identity
        // over a constant string.
        let mut interner = Interner::new();
        let home = ModPath(vec![
            interner.intern("Ipe")?,
            interner.intern("Ui")?,
            interner.intern("Animation")?,
        ]);
        let name = interner.intern("passthrough")?;
        let param = interner.intern("s")?;
        let func = passthrough_func(home, name, param);
        let mut funcs = BTreeMap::new();
        funcs.insert(FuncId::from_raw(0), &func);
        let env = FoldEnv::new(&funcs, &interner);

        let call = Expr::Call {
            callee: Callee::Func(FuncId::from_raw(0)),
            args: vec![Expr::Str("hi".to_string())],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        };
        assert_eq!(
            fold_const(&call, &env),
            Some(ConstValue::Str("hi".to_string()))
        );
        Ok(())
    }
}
