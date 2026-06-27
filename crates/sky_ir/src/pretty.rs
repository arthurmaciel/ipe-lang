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
    Arm, BinOp, Callee, EnumDef, Expr, Func, IrType, KernelFn, Match, ModPath, Module, Pat,
    Program, TypeDef,
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
        IrType::Unit => "()".to_owned(),
        IrType::TaskUnit => "Task Error ()".to_owned(),
        IrType::Enum(name) => sym_name(interner, *name),
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
    }
}

/// Render a kernel function's qualified source name.
const fn kernel_name(kernel: KernelFn) -> &'static str {
    match kernel {
        KernelFn::StringFromInt => "String.fromInt",
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

/// Render a pattern, e.g. `Msg.Increment`.
fn pat_name(interner: &Interner, pat: &Pat) -> String {
    match pat {
        Pat::Ctor { ty, variant } => {
            format!(
                "{}.{}",
                sym_name(interner, *ty),
                sym_name(interner, *variant)
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
        TypeDef::Enum(EnumDef { name, variants }) => {
            let rendered = variants
                .iter()
                .map(|v| sym_name(interner, *v))
                .collect::<Vec<_>>()
                .join(" | ");
            line(
                out,
                2,
                &format!("type {} = {rendered}", sym_name(interner, *name)),
            );
        }
    }
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
    line(
        out,
        2,
        &format!(
            "fn#{} {}({params}) -> {}",
            func.id.as_raw(),
            sym_name(interner, func.name),
            ir_type_name(interner, &func.ret)
        ),
    );
    write_expr(out, &func.body, interner, 3);
}

fn write_expr(out: &mut String, expr: &Expr, interner: &Interner, level: usize) {
    match expr {
        Expr::Int(n) => line(out, level, &format!("Int {n}")),
        Expr::Var(sym) => line(out, level, &format!("Var {}", sym_name(interner, *sym))),
        Expr::Ctor { ty, variant } => line(
            out,
            level,
            &format!(
                "Ctor {}.{}",
                sym_name(interner, *ty),
                sym_name(interner, *variant)
            ),
        ),
        Expr::BinOp { op, lhs, rhs } => {
            line(out, level, &format!("BinOp {}", binop_token(*op)));
            write_expr(out, lhs, interner, level + 1);
            write_expr(out, rhs, interner, level + 1);
        }
        Expr::Let { name, value, body } => {
            line(out, level, &format!("Let {}", sym_name(interner, *name)));
            line(out, level + 1, "value");
            write_expr(out, value, interner, level + 2);
            line(out, level + 1, "body");
            write_expr(out, body, interner, level + 2);
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
        Expr::Record(fields) => {
            line(out, level, "Record");
            for (name, value) in fields {
                line(
                    out,
                    level + 1,
                    &format!("field {}", sym_name(interner, *name)),
                );
                write_expr(out, value, interner, level + 2);
            }
        }
        Expr::Access { record, field } => {
            line(
                out,
                level,
                &format!("Access .{}", sym_name(interner, *field)),
            );
            write_expr(out, record, interner, level + 1);
        }
        Expr::Update { record, fields } => {
            line(out, level, "Update");
            line(out, level + 1, "record");
            write_expr(out, record, interner, level + 2);
            for (name, value) in fields {
                line(
                    out,
                    level + 1,
                    &format!("field {}", sym_name(interner, *name)),
                );
                write_expr(out, value, interner, level + 2);
            }
        }
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
            params: vec![(m, IrType::Enum(msg)), (count, IrType::Int)],
            ret: IrType::Int,
            body: Expr::Match(Match::new(Expr::Var(m), tick_arms, &[inc, dec])?),
        };

        Ok(Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    name: msg,
                    variants: vec![inc, dec],
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
