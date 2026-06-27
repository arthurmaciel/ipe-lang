//! The typed IR node definitions (M0 subset). Widened in later milestones; for
//! M0 the surface is deliberately narrow so that every constructible value is a
//! well-formed program fragment.

use std::collections::BTreeSet;

use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::Symbol;

/// A dotted module path, e.g. `Main` or `Sky.Core.Io`, as interned segments in
/// source order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModPath(pub Vec<Symbol>);

/// A function identifier, unique within a [`Program`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FuncId(pub u32);

impl FuncId {
    #[must_use]
    pub const fn from_raw(n: u32) -> Self {
        Self(n)
    }

    #[must_use]
    pub const fn as_raw(self) -> u32 {
        self.0
    }
}

/// A whole compiled program: an ordered list of modules.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Program {
    pub modules: Vec<Module>,
}

/// A single module: its declared types and functions, plus an optional entry
/// point (the `main` function, when this module carries it).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Module {
    pub name: ModPath,
    pub types: Vec<TypeDef>,
    pub funcs: Vec<Func>,
    pub entry: Option<FuncId>,
}

/// A user-declared type. M0 supports only enums with nullary variants.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TypeDef {
    Enum(EnumDef),
}

/// An enum declaration with nullary variants only (M0 subset).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EnumDef {
    pub name: Symbol,
    pub variants: Vec<Symbol>,
}

/// A function: typed parameters, a return type, and a body expression.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Func {
    pub id: FuncId,
    pub name: Symbol,
    pub params: Vec<(Symbol, IrType)>,
    pub ret: IrType,
    pub body: Expr,
}

/// The M0 type lattice. Widened in later milestones.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum IrType {
    Int,
    Float,
    Bool,
    Str,
    Unit,
    TaskUnit,
    Enum(Symbol),
}

/// An expression in the typed IR.
///
/// Note: the [`Match`] variant wraps the opaque [`Match`] type rather than
/// inlining `scrutinee` / `arms` fields. That is deliberate — it keeps the
/// exhaustiveness invariant unbreakable, because the only constructor for a
/// [`Match`] is [`Match::new`], which validates the arm set. An inline
/// struct-variant with public fields could be built directly, bypassing the
/// check, and would make illegal IR representable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Expr {
    Int(i64),
    Var(Symbol),
    Ctor { ty: Symbol, variant: Symbol },
    BinOp { op: BinOp, lhs: Box<Self>, rhs: Box<Self> },
    Match(Match),
    Call { callee: Callee, args: Vec<Self> },
}

/// The target of a [`Expr::Call`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Callee {
    Func(FuncId),
    Kernel(KernelFn),
}

/// Built-in kernel functions reachable from M0 programs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KernelFn {
    StringFromInt,
    LogPrintln,
}

/// Binary operators in the M0 subset.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
}

/// One arm of a [`Match`]: a constructor pattern and the body it guards.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Arm {
    pub pat: Pat,
    pub body: Expr,
}

/// A pattern. M0 supports only nullary constructor patterns.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Pat {
    Ctor { ty: Symbol, variant: Symbol },
}

/// An exhaustive case analysis over an enum scrutinee.
///
/// Fields are private: the sole way to obtain a `Match` is [`Match::new`],
/// which proves exhaustiveness at construction time. This makes a
/// non-exhaustive `Match` unrepresentable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Match {
    scrutinee: Box<Expr>,
    arms: Vec<Arm>,
}

impl Match {
    /// Build an exhaustive `Match`.
    ///
    /// `variants` is the complete set of constructors of the scrutinee's enum.
    /// The arm set is accepted only when it covers exactly that set, with no
    /// duplicate, unknown, or missing variant.
    ///
    /// # Errors
    ///
    /// Returns [`Diagnostic::CompilerBug`] when the arms do not form an
    /// exhaustive, non-redundant cover of `variants` — an internal invariant
    /// violation the lowerer must never produce.
    pub fn new(scrutinee: Expr, arms: Vec<Arm>, variants: &[Symbol]) -> DResult<Self> {
        let expected: BTreeSet<Symbol> = variants.iter().copied().collect();

        let mut covered: BTreeSet<Symbol> = BTreeSet::new();
        for arm in &arms {
            let Pat::Ctor { variant, .. } = arm.pat;
            if !expected.contains(&variant) {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_ir::Match::new",
                    detail: format!(
                        "match arm covers variant {} not in the scrutinee's enum",
                        variant.as_raw()
                    ),
                });
            }
            if !covered.insert(variant) {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_ir::Match::new",
                    detail: format!("duplicate match arm for variant {}", variant.as_raw()),
                });
            }
        }

        if covered != expected {
            return Err(Diagnostic::CompilerBug {
                where_: "sky_ir::Match::new",
                detail: format!(
                    "non-exhaustive match: covered {} of {} variants",
                    covered.len(),
                    expected.len()
                ),
            });
        }

        Ok(Self { scrutinee: Box::new(scrutinee), arms })
    }

    #[must_use]
    pub fn scrutinee(&self) -> &Expr {
        &self.scrutinee
    }

    #[must_use]
    pub fn arms(&self) -> &[Arm] {
        &self.arms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sky_intern::Interner;

    fn msg_enum(i: &mut Interner) -> (Symbol, Symbol, Symbol) {
        let ty = i.intern("Msg");
        let inc = i.intern("Increment");
        let dec = i.intern("Decrement");
        (ty, inc, dec)
    }

    #[test]
    fn match_new_accepts_exhaustive_and_round_trips_debug() {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i);
        let count = i.intern("count");

        // case msg of Increment -> count + 1 ; Decrement -> count - 1
        let arms = vec![
            Arm {
                pat: Pat::Ctor { ty, variant: inc },
                body: Expr::BinOp {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Var(count)),
                    rhs: Box::new(Expr::Int(1)),
                },
            },
            Arm {
                pat: Pat::Ctor { ty, variant: dec },
                body: Expr::BinOp {
                    op: BinOp::Sub,
                    lhs: Box::new(Expr::Var(count)),
                    rhs: Box::new(Expr::Int(1)),
                },
            },
        ];
        let res = Match::new(Expr::Var(i.intern("msg")), arms, &[inc, dec]);

        assert_eq!(res.as_ref().map(|m| m.arms().len()), Ok(2));
        assert!(matches!(res.as_ref().map(Match::scrutinee), Ok(Expr::Var(_))));
        // Debug round-trips (no panic, stable shape).
        let rendered = format!("{res:?}");
        assert!(rendered.contains("Match"));
    }

    #[test]
    fn match_new_rejects_non_exhaustive() {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i);
        let count = i.intern("count");

        // Only the Increment arm — Decrement uncovered.
        let arms = vec![Arm {
            pat: Pat::Ctor { ty, variant: inc },
            body: Expr::Var(count),
        }];
        let r = Match::new(Expr::Var(i.intern("msg")), arms, &[inc, dec]);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
    }

    #[test]
    fn match_new_rejects_duplicate_arm() {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i);

        let arms = vec![
            Arm { pat: Pat::Ctor { ty, variant: inc }, body: Expr::Int(0) },
            Arm { pat: Pat::Ctor { ty, variant: inc }, body: Expr::Int(1) },
        ];
        let r = Match::new(Expr::Var(i.intern("msg")), arms, &[inc, dec]);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
    }

    #[test]
    fn match_new_rejects_unknown_variant() {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i);
        let bogus = i.intern("Reset");

        let arms = vec![
            Arm { pat: Pat::Ctor { ty, variant: inc }, body: Expr::Int(0) },
            Arm { pat: Pat::Ctor { ty, variant: bogus }, body: Expr::Int(1) },
        ];
        let r = Match::new(Expr::Var(i.intern("msg")), arms, &[inc, dec]);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
    }

    #[test]
    fn program_round_trips_debug() {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i);
        let main_sym = i.intern("main");
        let main_mod = i.intern("Main");

        let func = Func {
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
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef { name: ty, variants: vec![inc, dec] })],
                funcs: vec![func],
                entry: Some(FuncId::from_raw(0)),
            }],
        };
        let clone = program.clone();
        assert_eq!(program, clone);
        assert!(format!("{program:?}").contains("Program"));
    }
}
