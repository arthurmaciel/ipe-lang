//! The lowering core: a name-resolved [`canon::Module`] plus its
//! [`SolvedTypes`] become a backend-agnostic [`sky_ir::Program`].
//!
//! This is the narrowed M0 port of the Haskell compiler's `Sky.Build.Compile`
//! lowering walk and `Sky.Build.LowerCtx`. Every step is total: an input shape
//! the M0 subset does not model, or a type slot the solver did not record, is
//! an internal-invariant violation surfaced as
//! [`sky_diagnostics::Diagnostic::CompilerBug`] — never a panic, never a guess.

use std::collections::BTreeMap;

use sky_canon::ast as canon;
use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::{Interner, Symbol};
use sky_ir::{
    Arm, BinOp, Callee, EnumDef, Expr, Func, FuncId, IrType, KernelFn, Match, ModPath, Module, Pat,
    Program, TypeDef,
};
use sky_types::{SolvedTypes, Ty};

/// Build a [`Diagnostic::CompilerBug`] for a violated lowering invariant.
fn bug(where_: &'static str, detail: impl Into<String>) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_,
        detail: detail.into(),
    }
}

/// The lowering pass over a single canonical module.
pub struct Lowerer<'a> {
    m: &'a canon::Module,
    types: &'a SolvedTypes,
    interner: &'a Interner,
    /// Each top-level binding's [`FuncId`], assigned in declaration order so a
    /// `VarTopLevel` reference can resolve to a [`Callee::Func`].
    func_ids: BTreeMap<Symbol, FuncId>,
    /// Each union's complete, in-declaration-order constructor set — the *true*
    /// variant set handed to [`Match::new`] (the IR layer cannot self-confirm
    /// this; the lowerer carries the obligation).
    enum_variants: BTreeMap<Symbol, Vec<Symbol>>,
}

impl<'a> Lowerer<'a> {
    pub fn new(m: &'a canon::Module, types: &'a SolvedTypes, interner: &'a Interner) -> Self {
        let mut func_ids = BTreeMap::new();
        for (idx, def) in m.defs.iter().enumerate() {
            let id = FuncId::from_raw(u32::try_from(idx).unwrap_or(u32::MAX));
            func_ids.insert(def.name().value, id);
        }

        let mut enum_variants = BTreeMap::new();
        for union in &m.unions {
            enum_variants.insert(union.name, union.ctors.iter().map(|c| c.name).collect());
        }

        Self {
            m,
            types,
            interner,
            func_ids,
            enum_variants,
        }
    }

    /// Resolve a symbol the IR guarantees was interned by `interner`. A `None`
    /// means the canonical AST carried a foreign symbol — an internal invariant
    /// violation, surfaced as a [`Diagnostic::CompilerBug`] rather than a silent
    /// empty name.
    fn resolve(&self, sym: Symbol) -> DResult<&'a str> {
        self.interner.resolve(sym).ok_or_else(|| {
            bug(
                "sky_lower::resolve",
                format!("symbol {} not present in interner", sym.as_raw()),
            )
        })
    }

    /// Run the pass, producing the single-module program.
    pub fn run(self) -> DResult<Program> {
        let types_ir: Vec<TypeDef> = self
            .m
            .unions
            .iter()
            .map(|u| {
                TypeDef::Enum(EnumDef {
                    name: u.name,
                    variants: u.ctors.iter().map(|c| c.name).collect(),
                })
            })
            .collect();

        let mut funcs = Vec::with_capacity(self.m.defs.len());
        let mut entry = None;
        for def in &self.m.defs {
            let func = self.lower_def(def)?;
            if self.interner.resolve(func.name) == Some("main") {
                entry = Some(func.id);
            }
            funcs.push(func);
        }

        let module = Module {
            name: ModPath(self.m.name.clone()),
            types: types_ir,
            funcs,
            entry,
        };
        Ok(Program {
            modules: vec![module],
        })
    }

    fn lower_def(&self, def: &canon::Def) -> DResult<Func> {
        let name = def.name().value;
        let id = *self
            .func_ids
            .get(&name)
            .ok_or_else(|| bug("sky_lower::lower_def", "missing func id"))?;

        match def {
            canon::Def::Typed {
                patterns, body, ty, ..
            } => {
                let (params, ret) = self.split_typed_sig(ty, patterns)?;
                Ok(Func {
                    id,
                    name,
                    params,
                    ret,
                    body: self.lower_expr(body)?,
                })
            }
            canon::Def::Untyped { patterns, body, .. } => {
                if !patterns.is_empty() {
                    return Err(bug(
                        "sky_lower::lower_def",
                        "untyped definition with parameters is unsupported in M0",
                    ));
                }
                let ret_ty =
                    self.types.env.get(&name).ok_or_else(|| {
                        bug("sky_lower::lower_def", "no inferred type for binding")
                    })?;
                let ret = self.ir_type_from_ty(ret_ty)?;
                Ok(Func {
                    id,
                    name,
                    params: Vec::new(),
                    ret,
                    body: self.lower_expr(body)?,
                })
            }
        }
    }

    /// Split a typed binding's arrow annotation into one [`IrType`] per
    /// parameter pattern plus the trailing return type.
    fn split_typed_sig(
        &self,
        ty: &canon::Type,
        patterns: &[canon::Pattern],
    ) -> DResult<(Vec<(Symbol, IrType)>, IrType)> {
        let mut cur = ty;
        let mut params = Vec::with_capacity(patterns.len());
        for pat in patterns {
            let canon::Type::Lambda(arg, rest) = cur else {
                return Err(bug(
                    "sky_lower::split_typed_sig",
                    "annotation has fewer arrows than parameters",
                ));
            };
            params.push((Self::pattern_var(pat)?, self.ir_type_from_canon(arg)?));
            cur = rest.as_ref();
        }
        Ok((params, self.ir_type_from_canon(cur)?))
    }

    fn pattern_var(pat: &canon::Pattern) -> DResult<Symbol> {
        match &pat.value {
            canon::Pattern_::PVar(s) => Ok(*s),
            _ => Err(bug(
                "sky_lower::pattern_var",
                "non-variable parameter pattern is unsupported in M0",
            )),
        }
    }

    /// Convert a canonical annotation type (no `Task`/unit appears in M0
    /// annotations) into an [`IrType`].
    fn ir_type_from_canon(&self, t: &canon::Type) -> DResult<IrType> {
        match t {
            canon::Type::Con { name, .. } => self.con_name_to_ir(*name),
            canon::Type::Lambda(_, _) => Err(bug(
                "sky_lower::ir_type_from_canon",
                "function type in value position is unsupported in M0",
            )),
            canon::Type::Var(_) => Err(bug(
                "sky_lower::ir_type_from_canon",
                "type variable is unsupported in M0",
            )),
        }
    }

    /// Convert a solved [`Ty`] (used for the return type of untyped bindings,
    /// e.g. `main : Task ()`) into an [`IrType`].
    fn ir_type_from_ty(&self, t: &Ty) -> DResult<IrType> {
        match t {
            Ty::Unit => Ok(IrType::Unit),
            Ty::Con { name, args, .. } => match self.resolve(*name)? {
                "Int" => Ok(IrType::Int),
                "Float" => Ok(IrType::Float),
                "Bool" => Ok(IrType::Bool),
                "String" => Ok(IrType::Str),
                "Task" if args.len() == 1 && matches!(args.first(), Some(Ty::Unit)) => {
                    Ok(IrType::TaskUnit)
                }
                "Task" => Err(bug(
                    "sky_lower::ir_type_from_ty",
                    "only Task () is supported in M0",
                )),
                _ if self.enum_variants.contains_key(name) => Ok(IrType::Enum(*name)),
                other => Err(bug(
                    "sky_lower::ir_type_from_ty",
                    format!("unknown type constructor `{other}`"),
                )),
            },
            Ty::Fun(_, _) => Err(bug(
                "sky_lower::ir_type_from_ty",
                "function type in value position is unsupported in M0",
            )),
            Ty::Var(_) => Err(bug(
                "sky_lower::ir_type_from_ty",
                "unresolved type variable in value position",
            )),
        }
    }

    /// Map a built-in or user type constructor *name* to an [`IrType`].
    fn con_name_to_ir(&self, name: Symbol) -> DResult<IrType> {
        match self.resolve(name)? {
            "Int" => Ok(IrType::Int),
            "Float" => Ok(IrType::Float),
            "Bool" => Ok(IrType::Bool),
            "String" => Ok(IrType::Str),
            _ if self.enum_variants.contains_key(&name) => Ok(IrType::Enum(name)),
            other => Err(bug(
                "sky_lower::con_name_to_ir",
                format!("unknown type constructor `{other}`"),
            )),
        }
    }

    fn lower_expr(&self, e: &canon::Expr) -> DResult<Expr> {
        match &e.value {
            canon::Expr_::Int(n) => Ok(Expr::Int(*n)),
            canon::Expr_::VarLocal(s) => Ok(Expr::Var(*s)),
            canon::Expr_::VarCtor {
                type_name, name, ..
            } => Ok(Expr::Ctor {
                ty: *type_name,
                variant: *name,
            }),
            canon::Expr_::Binop { func, lhs, rhs, .. } => Ok(Expr::BinOp {
                op: self.binop(*func)?,
                lhs: Box::new(self.lower_expr(lhs)?),
                rhs: Box::new(self.lower_expr(rhs)?),
            }),
            canon::Expr_::Call(callee, args) => {
                let callee = self.lower_callee(callee)?;
                let args = args
                    .iter()
                    .map(|a| self.lower_expr(a))
                    .collect::<DResult<Vec<_>>>()?;
                Ok(Expr::Call { callee, args })
            }
            canon::Expr_::Case(scrut, branches) => self.lower_case(scrut, branches),
            canon::Expr_::VarTopLevel { .. } | canon::Expr_::VarKernel { .. } => Err(bug(
                "sky_lower::lower_expr",
                "a bare function reference is unsupported in M0 (only as a call callee)",
            )),
        }
    }

    fn lower_callee(&self, callee: &canon::Expr) -> DResult<Callee> {
        match &callee.value {
            canon::Expr_::VarKernel { module, name } => {
                match (self.resolve(*module)?, self.resolve(*name)?) {
                    ("Log", "println") => Ok(Callee::Kernel(KernelFn::LogPrintln)),
                    ("String", "fromInt") => Ok(Callee::Kernel(KernelFn::StringFromInt)),
                    (m, n) => Err(bug(
                        "sky_lower::lower_callee",
                        format!("unknown kernel `{m}.{n}` in M0"),
                    )),
                }
            }
            canon::Expr_::VarTopLevel { name, .. } => {
                let id = *self
                    .func_ids
                    .get(name)
                    .ok_or_else(|| bug("sky_lower::lower_callee", "unknown top-level binding"))?;
                Ok(Callee::Func(id))
            }
            _ => Err(bug(
                "sky_lower::lower_callee",
                "unsupported callee shape in M0 (expected a kernel or top-level binding)",
            )),
        }
    }

    fn binop(&self, func: Symbol) -> DResult<BinOp> {
        match self.resolve(func)? {
            "add" => Ok(BinOp::Add),
            "sub" => Ok(BinOp::Sub),
            other => Err(bug(
                "sky_lower::binop",
                format!("unsupported binary operator `{other}` in M0"),
            )),
        }
    }

    fn lower_case(&self, scrut: &canon::Expr, branches: &[canon::CaseBranch]) -> DResult<Expr> {
        let scrutinee = self.lower_expr(scrut)?;

        let first = branches
            .first()
            .ok_or_else(|| bug("sky_lower::lower_case", "empty case expression"))?;
        let canon::Pattern_::PCtor { type_name, .. } = &first.pat.value else {
            return Err(bug(
                "sky_lower::lower_case",
                "non-constructor pattern is unsupported in M0",
            ));
        };
        let variants = self
            .enum_variants
            .get(type_name)
            .ok_or_else(|| bug("sky_lower::lower_case", "unknown scrutinee enum"))?;

        let arms = branches
            .iter()
            .map(|br| {
                let canon::Pattern_::PCtor {
                    type_name, name, ..
                } = &br.pat.value
                else {
                    return Err(bug(
                        "sky_lower::lower_case",
                        "non-constructor pattern is unsupported in M0",
                    ));
                };
                Ok(Arm {
                    pat: Pat::Ctor {
                        ty: *type_name,
                        variant: *name,
                    },
                    body: self.lower_expr(&br.body)?,
                })
            })
            .collect::<DResult<Vec<_>>>()?;

        Ok(Expr::Match(Match::new(scrutinee, arms, variants)?))
    }
}
