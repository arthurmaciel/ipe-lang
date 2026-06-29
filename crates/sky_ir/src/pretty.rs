//! A readable, indented pretty-printer for the typed IR, backing the
//! `--emit-ir` developer flag. This is intentionally *not* the derived
//! `Debug` rendering: it resolves interned [`Symbol`]s back to their source
//! names and lays the program out as a shallow tree (modules → types / funcs →
//! params / expressions / match arms / kernels) that a human can scan.
//!
//! The function is pure and total: it never panics, never indexes a slice
//! directly, and resolves a forged or cross-interner symbol to an explicit
//! `<sym#N>` placeholder rather than crashing or silently emitting an empty
//! name. Output is deterministic — the same `(program, interner)` always
//! renders the same string.

use sky_intern::{Interner, Symbol};

use crate::ir::{
    Arm, BinOp, BoundSet, Callee, EnumDef, Expr, Func, IrType, KernelFn, Match, ModPath, Module,
    Pat, Program, TypeDef, Variant,
};

/// Render `program` as a readable indented tree, resolving every [`Symbol`]
/// against `interner`.
///
/// Pure and total: no panics, no direct indexing, deterministic output.
#[must_use]
pub fn pretty(program: &Program, interner: &Interner) -> String {
    let mut out = String::new();
    out.push_str("program\n");
    for module in &program.modules {
        write_module(&mut out, module, interner);
    }
    out
}

/// Append `text` at the given indentation `level` (two spaces per level),
/// followed by a newline.
fn line(out: &mut String, level: usize, text: &str) {
    for _ in 0..level {
        out.push_str("  ");
    }
    out.push_str(text);
    out.push('\n');
}

/// Resolve a symbol to its interned name, or an explicit placeholder when the
/// symbol was never handed out by this interner.
fn sym_name(interner: &Interner, sym: Symbol) -> String {
    interner
        .resolve(sym)
        .map_or_else(|| format!("<sym#{}>", sym.as_raw()), str::to_owned)
}

/// Render a dotted module path, e.g. `Sky.Core.Io`.
fn mod_path_name(interner: &Interner, path: &ModPath) -> String {
    path.0
        .iter()
        .map(|seg| sym_name(interner, *seg))
        .collect::<Vec<_>>()
        .join(".")
}

/// Render an [`IrType`] as its source-facing name.
fn ir_type_name(interner: &Interner, ty: &IrType) -> String {
    match ty {
        IrType::Int => "Int".to_owned(),
        IrType::Float => "Float".to_owned(),
        IrType::Bool => "Bool".to_owned(),
        IrType::Str => "String".to_owned(),
        IrType::Char => "Char".to_owned(),
        IrType::Unit => "()".to_owned(),
        IrType::TaskUnit => "Task Error ()".to_owned(),
        // A generic type variable renders by its source name (e.g. `a`); the
        // Rust generic spelling (`T1`, …) is a backend concern, so the IR view
        // keeps the source-facing name.
        IrType::Generic(name) => sym_name(interner, *name),
        // An enum renders by its type name, applied to its type arguments in
        // source-like prefix form (`Maybe Int`). A non-generic enum (empty
        // `args`) is just the bare type name.
        IrType::Enum { name, args } => {
            let base = sym_name(interner, *name);
            if args.is_empty() {
                base
            } else {
                let rendered = args
                    .iter()
                    .map(|t| ir_type_name(interner, t))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{base} {rendered}")
            }
        }
        // The built-in `Maybe a` / `Result e a` render in source-like prefix
        // form, exactly like a generic enum would.
        IrType::Maybe(elem) => format!("Maybe {}", ir_type_name(interner, elem)),
        IrType::Result(err, ok) => format!(
            "Result {} {}",
            ir_type_name(interner, err),
            ir_type_name(interner, ok)
        ),
        IrType::List(elem) => format!("List {}", ir_type_name(interner, elem)),
        IrType::Tuple(elems) => {
            let inner = elems
                .iter()
                .map(|t| ir_type_name(interner, t))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
        IrType::Record(fields) => {
            // Render in field-name order (the BTreeMap is keyed by Symbol, so
            // sort the resolved names for a deterministic, source-like form).
            let mut entries: Vec<(String, String)> = fields
                .iter()
                .map(|(name, ty)| (sym_name(interner, *name), ir_type_name(interner, ty)))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            if entries.is_empty() {
                "{}".to_owned()
            } else {
                let inner = entries
                    .iter()
                    .map(|(n, t)| format!("{n} : {t}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{ {inner} }}")
            }
        }
        IrType::Fun(params, ret) => {
            // Source-like arrow form `T0 -> T1 -> R`. A nullary function type
            // shows its unit parameter explicitly so it stays distinct from its
            // bare return type.
            let mut parts: Vec<String> = params.iter().map(|t| ir_type_name(interner, t)).collect();
            if parts.is_empty() {
                parts.push("()".to_owned());
            }
            parts.push(ir_type_name(interner, ret));
            parts.join(" -> ")
        }
    }
}

/// Render a binary operator's surface (Sky source) token.
const fn binop_token(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Eq => "==",
        BinOp::Neq => "/=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        BinOp::Append => "++",
    }
}

/// Render a kernel function's qualified source name.
const fn kernel_name(kernel: KernelFn) -> &'static str {
    match kernel {
        KernelFn::StringFromInt => "String.fromInt",
        KernelFn::StringFromFloat => "String.fromFloat",
        KernelFn::LogPrintln => "Log.println",
    }
}

/// Render a call target. Functions are shown by id (their name lives at the
/// declaration site); kernels by their qualified source name.
fn callee_name(callee: &Callee) -> String {
    match callee {
        Callee::Func(id) => format!("fn#{}", id.as_raw()),
        Callee::Kernel(kernel) => format!("kernel {}", kernel_name(*kernel)),
    }
}

/// Render a pattern, e.g. `Msg.Increment`, `Maybe.Just x`, `Maybe.Just _`.
fn pat_name(interner: &Interner, pat: &Pat) -> String {
    match pat {
        Pat::Var(sym) => sym_name(interner, *sym),
        Pat::Wildcard => "_".to_owned(),
        Pat::Int(n) => n.to_string(),
        Pat::Bool(b) => if *b { "True" } else { "False" }.to_owned(),
        Pat::Char(c) => format!("'{c}'"),
        Pat::Str(s) => format!("{s:?}"),
        Pat::Alias(inner, name) => {
            format!(
                "{} as {}",
                pat_name(interner, inner),
                sym_name(interner, *name)
            )
        }
        Pat::Tuple(elems) => {
            let inner = elems
                .iter()
                .map(|p| pat_name(interner, p))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
        Pat::Ctor { ty, variant, args } => {
            let head = format!(
                "{}.{}",
                sym_name(interner, *ty),
                sym_name(interner, *variant)
            );
            if args.is_empty() {
                head
            } else {
                let subs = args
                    .iter()
                    .map(|p| pat_name(interner, p))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{head} {subs}")
            }
        }
        Pat::Record(fields) => {
            let inner = fields
                .iter()
                .map(|(sym, p)| format!("{} = {}", sym_name(interner, *sym), pat_name(interner, p)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {inner} }}")
        }
        Pat::Slice { prefix, rest } => {
            let parts = prefix
                .iter()
                .map(|p| pat_name(interner, p))
                .collect::<Vec<_>>()
                .join(", ");
            rest.as_ref().map_or_else(
                || format!("[{parts}]"),
                |r| format!("[{parts}, {} @ ..]", pat_name(interner, r)),
            )
        }
    }
}

fn write_module(out: &mut String, module: &Module, interner: &Interner) {
    line(
        out,
        1,
        &format!("module {}", mod_path_name(interner, &module.name)),
    );
    for ty in &module.types {
        write_type(out, ty, interner);
    }
    for func in &module.funcs {
        write_func(out, func, interner);
    }
    if let Some(entry) = module.entry {
        let name = module.funcs.iter().find(|f| f.id == entry).map_or_else(
            || format!("fn#{}", entry.as_raw()),
            |f| sym_name(interner, f.name),
        );
        line(out, 2, &format!("entry {name}"));
    }
}

fn write_type(out: &mut String, ty: &TypeDef, interner: &Interner) {
    match ty {
        TypeDef::Enum(EnumDef {
            name,
            type_params,
            variants,
        }) => {
            let rendered = variants
                .iter()
                .map(|v| variant_name(interner, v))
                .collect::<Vec<_>>()
                .join(" | ");
            // A generic enum shows its quantified type variables after the name
            // (`Maybe a`); a non-generic enum shows nothing, so existing output
            // is unchanged.
            let gens = if type_params.is_empty() {
                String::new()
            } else {
                let vars = type_params
                    .iter()
                    .map(|sym| sym_name(interner, *sym))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(" {vars}")
            };
            line(
                out,
                2,
                &format!("type {}{gens} = {rendered}", sym_name(interner, *name)),
            );
        }
    }
}

/// Render one enum variant in source-like form: `Increment`, `Just a`,
/// `Rect Float Float`.
fn variant_name(interner: &Interner, v: &Variant) -> String {
    let head = sym_name(interner, v.name);
    if v.fields.is_empty() {
        head
    } else {
        let fields = v
            .fields
            .iter()
            .map(|t| ir_type_name(interner, t))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{head} {fields}")
    }
}

/// The debug-text suffix for one type parameter's bounds: empty for an
/// unbounded variable (so a structurally-parametric function's rendering is
/// byte-identical to the M2a form), or `: Add+Sub+…` listing each set flag in a
/// fixed order. This is the IR's human-readable dump, not the Rust emission —
/// the backend renders the real `::core::ops::*` / `PartialOrd` spellings.
fn bound_suffix(bounds: BoundSet) -> String {
    if bounds.is_unbounded() {
        return String::new();
    }
    let mut parts = Vec::new();
    if bounds.has_add() {
        parts.push("Add");
    }
    if bounds.has_sub() {
        parts.push("Sub");
    }
    if bounds.has_mul() {
        parts.push("Mul");
    }
    if bounds.has_ord() {
        parts.push("Ord");
    }
    if bounds.has_eq() {
        parts.push("Eq");
    }
    if bounds.has_copy() {
        parts.push("Copy");
    }
    if bounds.has_clone() {
        parts.push("Clone");
    }
    format!(": {}", parts.join("+"))
}

fn write_func(out: &mut String, func: &Func, interner: &Interner) {
    let params = func
        .params
        .iter()
        .map(|(sym, ty)| {
            format!(
                "{} : {}",
                sym_name(interner, *sym),
                ir_type_name(interner, ty)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    // A fully-parametric function shows its quantified type variables as
    // `<a, b>` after the name; a monomorphic function shows nothing, so
    // existing (empty `type_params`) output is unchanged.
    let generics = if func.type_params.is_empty() {
        String::new()
    } else {
        let vars = func
            .type_params
            .iter()
            .map(|(sym, bounds)| {
                let name = sym_name(interner, *sym);
                let suffix = bound_suffix(*bounds);
                format!("{name}{suffix}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("<{vars}>")
    };
    line(
        out,
        2,
        &format!(
            "fn#{} {}{generics}({params}) -> {}",
            func.id.as_raw(),
            sym_name(interner, func.name),
            ir_type_name(interner, &func.ret)
        ),
    );
    write_expr(out, &func.body, interner, 3);
}

/// Render a `let`-like binding node (`Let` / `Destructure`): a `header` line,
/// then the `value` and `body` sub-trees under labelled children. Shared by both
/// binding forms so each match arm stays a single call.
fn write_binding(
    out: &mut String,
    header: &str,
    value: &Expr,
    body: &Expr,
    interner: &Interner,
    level: usize,
) {
    line(out, level, header);
    line(out, level + 1, "value");
    write_expr(out, value, interner, level + 2);
    line(out, level + 1, "body");
    write_expr(out, body, interner, level + 2);
}

fn write_expr(out: &mut String, expr: &Expr, interner: &Interner, level: usize) {
    match expr {
        Expr::Int(n) => line(out, level, &format!("Int {n}")),
        Expr::Bool(b) => line(out, level, &format!("Bool {b}")),
        Expr::Float(f) => line(out, level, &format!("Float {f}")),
        Expr::Str(s) => line(out, level, &format!("Str {s:?}")),
        Expr::Char(c) => line(out, level, &format!("Char '{c}'")),
        Expr::Unit => line(out, level, "Unit"),
        Expr::Var(sym) => line(out, level, &format!("Var {}", sym_name(interner, *sym))),
        Expr::Ctor { ty, variant, args } => {
            line(
                out,
                level,
                &format!(
                    "Ctor {}.{}",
                    sym_name(interner, *ty),
                    sym_name(interner, *variant)
                ),
            );
            for arg in args {
                write_expr(out, arg, interner, level + 1);
            }
        }
        Expr::BinOp { op, lhs, rhs } => {
            line(out, level, &format!("BinOp {}", binop_token(*op)));
            write_expr(out, lhs, interner, level + 1);
            write_expr(out, rhs, interner, level + 1);
        }
        Expr::Let { name, value, body } => {
            write_binding(
                out,
                &format!("Let {}", sym_name(interner, *name)),
                value,
                body,
                interner,
                level,
            );
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            write_binding(
                out,
                &format!("Destructure {}", pat_name(interner, binder)),
                value,
                body,
                interner,
                level,
            );
        }
        Expr::If { cond, then_, else_ } => {
            line(out, level, "If");
            line(out, level + 1, "cond");
            write_expr(out, cond, interner, level + 2);
            line(out, level + 1, "then");
            write_expr(out, then_, interner, level + 2);
            line(out, level + 1, "else");
            write_expr(out, else_, interner, level + 2);
        }
        Expr::Match(m) => write_match(out, m, interner, level),
        Expr::Call { callee, args } => {
            line(out, level, &format!("Call {}", callee_name(callee)));
            for arg in args {
                write_expr(out, arg, interner, level + 1);
            }
        }
        Expr::Tuple(elems) => {
            line(out, level, "Tuple");
            for elem in elems {
                write_expr(out, elem, interner, level + 1);
            }
        }
        Expr::List { elem, items } => write_list(out, elem, items, interner, level),
        Expr::Cons { head, tail } => write_cons(out, head, tail, interner, level),
        Expr::Record(fields) => write_record(out, fields, interner, level),
        Expr::Access { record, field } => {
            line(
                out,
                level,
                &format!("Access .{}", sym_name(interner, *field)),
            );
            write_expr(out, record, interner, level + 1);
        }
        Expr::Update { record, fields } => write_update(out, record, fields, interner, level),
        Expr::Lambda { params, ret, body } => write_lambda(out, params, ret, body, interner, level),
        Expr::Apply { func, args } => write_apply(out, func, args, interner, level),
        Expr::FuncValue { callee, ty } => line(
            out,
            level,
            &format!(
                "FuncValue {} : {}",
                callee_name(callee),
                ir_type_name(interner, ty)
            ),
        ),
    }
}

/// Render the labelled `field <name>` / value child lines of a record literal /
/// update. Shared by [`write_record`] and [`write_update`].
fn write_fields(out: &mut String, fields: &[(Symbol, Expr)], interner: &Interner, level: usize) {
    for (name, value) in fields {
        line(out, level, &format!("field {}", sym_name(interner, *name)));
        write_expr(out, value, interner, level + 1);
    }
}

/// Render a `Record` literal node. Split from [`write_expr`] to keep that match
/// small.
/// Render a list literal node: a `List : <elem>` header line followed by each
/// element expression one level deeper.
fn write_list(out: &mut String, elem: &IrType, items: &[Expr], interner: &Interner, level: usize) {
    line(
        out,
        level,
        &format!("List : {}", ir_type_name(interner, elem)),
    );
    for item in items {
        write_expr(out, item, interner, level + 1);
    }
}

/// Render a cons node: a `Cons` header line followed by the head then the tail,
/// each one level deeper.
fn write_cons(out: &mut String, head: &Expr, tail: &Expr, interner: &Interner, level: usize) {
    line(out, level, "Cons");
    write_expr(out, head, interner, level + 1);
    write_expr(out, tail, interner, level + 1);
}

fn write_record(out: &mut String, fields: &[(Symbol, Expr)], interner: &Interner, level: usize) {
    line(out, level, "Record");
    write_fields(out, fields, interner, level + 1);
}

/// Render an `Update` node: the copied `record` then the changed fields. Split
/// from [`write_expr`] to keep that match small.
fn write_update(
    out: &mut String,
    record: &Expr,
    fields: &[(Symbol, Expr)],
    interner: &Interner,
    level: usize,
) {
    line(out, level, "Update");
    line(out, level + 1, "record");
    write_expr(out, record, interner, level + 2);
    write_fields(out, fields, interner, level + 1);
}

/// Render a `Lambda` node: a header `Lambda (p0 : T0, ...) -> R` followed by its
/// body one level deeper. Split from [`write_expr`] to keep that match small.
fn write_lambda(
    out: &mut String,
    params: &[(Symbol, IrType)],
    ret: &IrType,
    body: &Expr,
    interner: &Interner,
    level: usize,
) {
    let rendered = params
        .iter()
        .map(|(sym, ty)| {
            format!(
                "{} : {}",
                sym_name(interner, *sym),
                ir_type_name(interner, ty)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    line(
        out,
        level,
        &format!("Lambda ({rendered}) -> {}", ir_type_name(interner, ret)),
    );
    write_expr(out, body, interner, level + 1);
}

/// Render an `Apply` node: a `func` sub-tree then one `arg` sub-tree per
/// argument. Split from [`write_expr`] to keep that match small.
fn write_apply(out: &mut String, func: &Expr, args: &[Expr], interner: &Interner, level: usize) {
    line(out, level, "Apply");
    line(out, level + 1, "func");
    write_expr(out, func, interner, level + 2);
    for arg in args {
        line(out, level + 1, "arg");
        write_expr(out, arg, interner, level + 2);
    }
}

fn write_match(out: &mut String, m: &Match, interner: &Interner, level: usize) {
    line(out, level, "Match");
    line(out, level + 1, "scrutinee");
    write_expr(out, m.scrutinee(), interner, level + 2);
    for Arm { pat, body } in m.arms() {
        line(out, level + 1, &format!("arm {}", pat_name(interner, pat)));
        write_expr(out, body, interner, level + 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FuncId;
    use sky_diagnostics::DResult;

    /// Build the canonical M0 program: a `Main` module with a `Msg` enum and a
    /// `main` function whose body is `Log.println (String.fromInt 1)`, plus a
    /// `tick` function with a `Match` over `Msg`.
    fn m0_program(i: &mut Interner) -> DResult<Program> {
        let main_mod = i.intern("Main")?;
        let msg = i.intern("Msg")?;
        let inc = i.intern("Increment")?;
        let dec = i.intern("Decrement")?;
        let main_sym = i.intern("main")?;
        let tick = i.intern("tick")?;
        let count = i.intern("count")?;
        let m = i.intern("m")?;

        let main_func = Func {
            id: FuncId::from_raw(0),
            name: main_sym,
            type_params: vec![],
            params: vec![],
            ret: IrType::TaskUnit,
            body: Expr::Call {
                callee: Callee::Kernel(KernelFn::LogPrintln),
                args: vec![Expr::Call {
                    callee: Callee::Kernel(KernelFn::StringFromInt),
                    args: vec![Expr::Int(1)],
                }],
            },
        };

        let tick_arms = vec![
            Arm {
                pat: Pat::Ctor {
                    ty: msg,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::BinOp {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Var(count)),
                    rhs: Box::new(Expr::Int(1)),
                },
            },
            Arm {
                pat: Pat::Ctor {
                    ty: msg,
                    variant: dec,
                    args: vec![],
                },
                body: Expr::BinOp {
                    op: BinOp::Sub,
                    lhs: Box::new(Expr::Var(count)),
                    rhs: Box::new(Expr::Int(1)),
                },
            },
        ];
        let tick_func = Func {
            id: FuncId::from_raw(1),
            name: tick,
            type_params: vec![],
            params: vec![
                (
                    m,
                    IrType::Enum {
                        name: msg,
                        args: vec![],
                    },
                ),
                (count, IrType::Int),
            ],
            ret: IrType::Int,
            body: Expr::Match(Match::new(Expr::Var(m), tick_arms, &[inc, dec])?),
        };

        Ok(Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    name: msg,
                    type_params: vec![],
                    variants: vec![
                        Variant {
                            name: inc,
                            fields: vec![],
                        },
                        Variant {
                            name: dec,
                            fields: vec![],
                        },
                    ],
                })],
                funcs: vec![main_func, tick_func],
                entry: Some(FuncId::from_raw(0)),
                records: vec![],
            }],
        })
    }

    #[test]
    fn pretty_renders_m0_program() -> DResult<()> {
        let mut i = Interner::new();
        let program = m0_program(&mut i)?;
        let rendered = pretty(&program, &i);

        let expected = "\
program
  module Main
    type Msg = Increment | Decrement
    fn#0 main() -> Task Error ()
      Call kernel Log.println
        Call kernel String.fromInt
          Int 1
    fn#1 tick(m : Msg, count : Int) -> Int
      Match
        scrutinee
          Var m
        arm Msg.Increment
          BinOp +
            Var count
            Int 1
        arm Msg.Decrement
          BinOp -
            Var count
            Int 1
    entry main
";
        assert_eq!(rendered, expected);
        Ok(())
    }

    #[test]
    fn pretty_is_deterministic() -> DResult<()> {
        let mut i = Interner::new();
        let program = m0_program(&mut i)?;
        assert_eq!(pretty(&program, &i), pretty(&program, &i));
        Ok(())
    }

    #[test]
    fn pretty_renders_let_if_and_extended_binops() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let f = i.intern("f")?;
        let n = i.intern("n")?;
        let x = i.intern("x")?;

        // f(n : Int) -> Int = let x = n * 2 in if x >= 10 then x / 2 else x + 1
        let body = Expr::Let {
            name: x,
            value: Box::new(Expr::BinOp {
                op: BinOp::Mul,
                lhs: Box::new(Expr::Var(n)),
                rhs: Box::new(Expr::Int(2)),
            }),
            body: Box::new(Expr::If {
                cond: Box::new(Expr::BinOp {
                    op: BinOp::Ge,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(10)),
                }),
                then_: Box::new(Expr::BinOp {
                    op: BinOp::Div,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(2)),
                }),
                else_: Box::new(Expr::BinOp {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(1)),
                }),
            }),
        };
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: f,
                    type_params: vec![],
                    params: vec![(n, IrType::Int)],
                    ret: IrType::Int,
                    body,
                }],
                entry: None,
                records: vec![],
            }],
        };

        let expected = "\
program
  module Main
    fn#0 f(n : Int) -> Int
      Let x
        value
          BinOp *
            Var n
            Int 2
        body
          If
            cond
              BinOp >=
                Var x
                Int 10
            then
              BinOp /
                Var x
                Int 2
            else
              BinOp +
                Var x
                Int 1
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_tuple_expr_and_type() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let f = i.intern("pair")?;
        let n = i.intern("n")?;

        // pair(n : Int) -> (Int, Bool) = (n, n)  (shape only; types illustrative)
        let body = Expr::Tuple(vec![Expr::Var(n), Expr::Int(1)]);
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: f,
                    type_params: vec![],
                    params: vec![(n, IrType::Int)],
                    ret: IrType::Tuple(vec![IrType::Int, IrType::Bool]),
                    body,
                }],
                entry: None,
                records: vec![],
            }],
        };

        let expected = "\
program
  module Main
    fn#0 pair(n : Int) -> (Int, Bool)
      Tuple
        Var n
        Int 1
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_record_expr_access_update_and_type() -> DResult<()> {
        use std::collections::BTreeMap;

        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let func = i.intern("move_")?;
        let param = i.intern("p")?;
        let x = i.intern("x")?;
        let y = i.intern("y")?;

        // move_(p : { x : Int, y : Int }) -> { x : Int, y : Int }
        //   = { p | x = p.x }   (shape only; values illustrative)
        let body = Expr::Update {
            record: Box::new(Expr::Var(param)),
            fields: vec![(
                x,
                Expr::Access {
                    record: Box::new(Expr::Var(param)),
                    field: x,
                },
            )],
        };
        let mut rec_fields = BTreeMap::new();
        rec_fields.insert(x, IrType::Int);
        rec_fields.insert(y, IrType::Int);
        let rec_ty = IrType::Record(rec_fields);
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: func,
                    type_params: vec![],
                    params: vec![(param, rec_ty.clone())],
                    ret: rec_ty,
                    body,
                }],
                entry: None,
                records: vec![],
            }],
        };

        let expected = "\
program
  module Main
    fn#0 move_(p : { x : Int, y : Int }) -> { x : Int, y : Int }
      Update
        record
          Var p
        field x
          Access .x
            Var p
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_record_literal() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let f = i.intern("origin")?;
        let x = i.intern("x")?;
        let y = i.intern("y")?;

        // origin() -> ... = { x = 1, y = 2 }
        let body = Expr::Record(vec![(x, Expr::Int(1)), (y, Expr::Int(2))]);
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: f,
                    type_params: vec![],
                    params: vec![],
                    ret: IrType::Int,
                    body,
                }],
                entry: None,
                records: vec![],
            }],
        };

        let expected = "\
program
  module Main
    fn#0 origin() -> Int
      Record
        field x
          Int 1
        field y
          Int 2
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_lambda_apply_and_fun_type() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let f = i.intern("apply2")?;
        let g = i.intern("g")?;
        let x = i.intern("x")?;

        // apply2(g : Int -> Int) -> Int = (\x -> g x) 2
        let body = Expr::Apply {
            func: Box::new(Expr::Lambda {
                params: vec![(x, IrType::Int)],
                ret: IrType::Int,
                body: Box::new(Expr::Apply {
                    func: Box::new(Expr::Var(g)),
                    args: vec![Expr::Var(x)],
                }),
            }),
            args: vec![Expr::Int(2)],
        };
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: f,
                    type_params: vec![],
                    params: vec![(g, IrType::Fun(vec![IrType::Int], Box::new(IrType::Int)))],
                    ret: IrType::Int,
                    body,
                }],
                entry: None,
                records: vec![],
            }],
        };

        let expected = "\
program
  module Main
    fn#0 apply2(g : Int -> Int) -> Int
      Apply
        func
          Lambda (x : Int) -> Int
            Apply
              func
                Var g
              arg
                Var x
        arg
          Int 2
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_nullary_fun_type() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let f = i.intern("thunk")?;

        // thunk(k : () -> Bool) -> Bool = ...  (body shape illustrative)
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: f,
                    type_params: vec![],
                    params: vec![(i.intern("k")?, IrType::Fun(vec![], Box::new(IrType::Bool)))],
                    ret: IrType::Bool,
                    body: Expr::Int(0),
                }],
                entry: None,
                records: vec![],
            }],
        };

        let expected = "\
program
  module Main
    fn#0 thunk(k : () -> Bool) -> Bool
      Int 0
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_generic_adt_decl_ctor_and_pattern() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let a = i.intern("a")?;
        let maybe = i.intern("Maybe")?;
        let just = i.intern("Just")?;
        let nothing = i.intern("Nothing")?;
        let unwrap = i.intern("unwrap")?;
        let m = i.intern("m")?;
        let x = i.intern("x")?;

        // type Maybe a = Just a | Nothing
        // unwrap(m : Maybe Int) -> Int =
        //   case m of Just x -> x ; Nothing -> 0
        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    ty: maybe,
                    variant: just,
                    args: vec![Pat::Var(x)],
                },
                body: Expr::Var(x),
            },
            Arm {
                pat: Pat::Ctor {
                    ty: maybe,
                    variant: nothing,
                    args: vec![],
                },
                body: Expr::Int(0),
            },
        ];
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    name: maybe,
                    type_params: vec![a],
                    variants: vec![
                        Variant {
                            name: just,
                            fields: vec![IrType::Generic(a)],
                        },
                        Variant {
                            name: nothing,
                            fields: vec![],
                        },
                    ],
                })],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: unwrap,
                    type_params: vec![],
                    params: vec![(
                        m,
                        IrType::Enum {
                            name: maybe,
                            args: vec![IrType::Int],
                        },
                    )],
                    ret: IrType::Int,
                    body: Expr::Match(Match::new(Expr::Var(m), arms, &[just, nothing])?),
                }],
                entry: None,
                records: vec![],
            }],
        };

        let expected = "\
program
  module Main
    type Maybe a = Just a | Nothing
    fn#0 unwrap(m : Maybe Int) -> Int
      Match
        scrutinee
          Var m
        arm Maybe.Just x
          Var x
        arm Maybe.Nothing
          Int 0
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_renders_tuple_pattern_and_unit() -> DResult<()> {
        let mut i = Interner::new();
        let main_mod = i.intern("Main")?;
        let wrap = i.intern("Wrap")?;
        let mk_wrap = i.intern("MkWrap")?;
        let fst_of = i.intern("fstOf")?;
        let w = i.intern("w")?;
        let a = i.intern("a")?;
        let b = i.intern("b")?;
        let nop = i.intern("nop")?;

        // type Wrap = MkWrap (Int, Int)
        // fstOf(w : Wrap) -> Int = case w of MkWrap (a, b) -> a
        // nop() -> () = ()
        let arms = vec![Arm {
            pat: Pat::Ctor {
                ty: wrap,
                variant: mk_wrap,
                args: vec![Pat::Tuple(vec![Pat::Var(a), Pat::Var(b)])],
            },
            body: Expr::Var(a),
        }];
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    name: wrap,
                    type_params: vec![],
                    variants: vec![Variant {
                        name: mk_wrap,
                        fields: vec![IrType::Tuple(vec![IrType::Int, IrType::Int])],
                    }],
                })],
                funcs: vec![
                    Func {
                        id: FuncId::from_raw(0),
                        name: fst_of,
                        type_params: vec![],
                        params: vec![(
                            w,
                            IrType::Enum {
                                name: wrap,
                                args: vec![],
                            },
                        )],
                        ret: IrType::Int,
                        body: Expr::Match(Match::new(Expr::Var(w), arms, &[mk_wrap])?),
                    },
                    Func {
                        id: FuncId::from_raw(1),
                        name: nop,
                        type_params: vec![],
                        params: vec![],
                        ret: IrType::Unit,
                        body: Expr::Unit,
                    },
                ],
                entry: None,
                records: vec![],
            }],
        };

        let expected = "\
program
  module Main
    type Wrap = MkWrap (Int, Int)
    fn#0 fstOf(w : Wrap) -> Int
      Match
        scrutinee
          Var w
        arm Wrap.MkWrap (a, b)
          Var a
    fn#1 nop() -> ()
      Unit
";
        assert_eq!(pretty(&program, &i), expected);
        Ok(())
    }

    #[test]
    fn pretty_resolves_forged_symbol_to_placeholder() {
        let i = Interner::new();
        // A program referencing a symbol this interner never handed out.
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![Symbol::from_raw(999)]),
                types: vec![],
                funcs: vec![],
                entry: None,
                records: vec![],
            }],
        };
        let rendered = pretty(&program, &i);
        assert!(rendered.contains("module <sym#999>"));
    }
}
