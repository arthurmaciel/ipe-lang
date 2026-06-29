//! End-of-checking exhaustiveness + redundancy analysis for `case`.
//!
//! Runs after the constraint solver has settled, walking the canonical AST and
//! judging every `case` against the constructor signature of its scrutinee.
//! Two findings are surfaced, both as owned, structured diagnostics:
//!
//! * **SKY-T0010 `NonExhaustiveCase`** — the arms do not cover every value; the
//!   missing patterns are listed (in declaration order for the top column).
//! * **SKY-T0011 `RedundantCaseBranch`** — a later arm matches no value the
//!   earlier arms left open.
//!
//! ## Why a full usefulness algorithm (not a shallow head check)
//!
//! The Rust backend renders each `case` arm as a native `match` arm and relies
//! on rustc to type-check it; a Sky `case` that is non-exhaustive over NESTED
//! patterns (`Just (Just a) -> …`, missing `Just Nothing`) would compile here
//! and then fail downstream with rustc's `E0004` — an exit-0-then-cargo-fail.
//! So this pass must be at least as strong as Rust's for nested patterns: it
//! ports Maranget's usefulness/matrix algorithm ("Warnings for pattern
//! matching", JFP 2007). The wildcard row `_` is *useful* against the arm
//! matrix exactly when the `case` is non-exhaustive, and the algorithm returns
//! the precise missing pattern(s) as the witness.
//!
//! ## Pattern abstraction
//!
//! Records are field-pun only in the source grammar, so a record pattern always
//! matches its (single-constructor) record value and binds variables only — it
//! is irrefutable. It therefore abstracts to a wildcard ([`UPat::Wild`]) for
//! coverage purposes. Tuples are single-constructor product types, abstracted to
//! [`UPat::Ctor`] with a [`Head::Tuple`] head whose element sub-patterns recurse.
//! ADT constructors abstract to [`Head::Adt`]. Literal / cons / alias patterns
//! are outside the analysed subset (the parser does not admit them in `case`),
//! so every pattern reaching here is var / wildcard / ctor / tuple / record.

use std::collections::{BTreeMap, BTreeSet};

use sky_canon::ast as canon;
use sky_diagnostics::{DResult, Diagnostic, TypeError};
use sky_intern::{Interner, Symbol};

/// `where_` tag for any internal-invariant bug raised while checking.
const STAGE: &str = "intern.resolve";

/// Upper bound on the number of distinct missing-pattern witnesses reported for
/// one non-exhaustive `case`. Keeps the diagnostic bounded (and the witness
/// search from fanning out) without losing the common small cases.
const WITNESS_CAP: usize = 32;

/// Constructor-signature tables, built once per module from its `type` decls.
struct Sigs {
    /// Constructor name → its owning union's name.
    ctor_to_union: BTreeMap<Symbol, Symbol>,
    /// Union name → its constructors in declaration (`index`) order, each paired
    /// with its payload arity.
    union_ctors: BTreeMap<Symbol, Vec<(Symbol, usize)>>,
    /// Constructor name → payload arity (the field count its pattern binds).
    ctor_arity: BTreeMap<Symbol, usize>,
}

impl Sigs {
    fn build(module: &canon::Module, interner: &mut Interner) -> DResult<Self> {
        let mut ctor_to_union = BTreeMap::new();
        let mut union_ctors = BTreeMap::new();
        let mut ctor_arity = BTreeMap::new();

        // Seed the Prelude-built-in closed unions `Maybe a` (`Just` / `Nothing`)
        // and `Result e a` (`Ok` / `Err`) so a `case` over them is ANALYSED for
        // exhaustiveness rather than skipped as an unknown-ctor scrutinee. Without
        // this, a non-exhaustive `case m of Just x -> …` would slip past the
        // soundness floor. `Bool` (`True` / `False`) is handled by the dedicated
        // [`Head::Bool`] literal path and needs no union entry.
        let maybe = interner.intern("Maybe")?;
        let result = interner.intern("Result")?;
        let just = interner.intern("Just")?;
        let nothing = interner.intern("Nothing")?;
        let ok = interner.intern("Ok")?;
        let err = interner.intern("Err")?;
        for (ctor, union, arity) in [
            (just, maybe, 1usize),
            (nothing, maybe, 0),
            (ok, result, 1),
            (err, result, 1),
        ] {
            ctor_to_union.insert(ctor, union);
            ctor_arity.insert(ctor, arity);
        }
        union_ctors.insert(maybe, vec![(just, 1), (nothing, 0)]);
        union_ctors.insert(result, vec![(ok, 1), (err, 1)]);

        for union in &module.unions {
            let mut ctors: Vec<&canon::Ctor> = union.ctors.iter().collect();
            ctors.sort_by_key(|c| c.index);
            let mut list = Vec::with_capacity(ctors.len());
            for c in ctors {
                ctor_to_union.insert(c.name, union.name);
                ctor_arity.insert(c.name, c.arity);
                list.push((c.name, c.arity));
            }
            union_ctors.insert(union.name, list);
        }
        Ok(Self {
            ctor_to_union,
            union_ctors,
            ctor_arity,
        })
    }

    /// The payload arity of a head constructor. A [`Head::Tuple`] carries its own
    /// arity; an ADT head is looked up. A missing ADT entry can only arise for a
    /// constructor outside this module's unions — and [`case_analysable`] has
    /// already excluded any such `case` from the matrix walk — so the `0`
    /// fallback is unreachable in practice yet keeps the function total (no panic).
    fn arity(&self, head: &Head) -> usize {
        match head {
            Head::Tuple(n) => *n,
            Head::Adt(c) => self.ctor_arity.get(c).copied().unwrap_or(0),
            // Literal heads carry no sub-patterns.
            Head::Bool(_) | Head::Int(_) | Head::Char(_) | Head::Str(_) => 0,
        }
    }
}

/// A head constructor in the usefulness matrix.
#[derive(Clone, PartialEq, Eq)]
enum Head {
    /// An ADT constructor, identified by name.
    Adt(Symbol),
    /// The single constructor of a tuple type of the given arity.
    Tuple(usize),
    /// A boolean literal head — `Bool` is a CLOSED two-constructor type, so a
    /// `True` + `False` pair completes the signature.
    Bool(bool),
    /// An integer literal head. `Int` is an OPEN (infinite) type, so a literal
    /// column never completes a signature — a wildcard / var is required.
    Int(i64),
    /// A character literal head. OPEN, like [`Head::Int`].
    Char(String),
    /// A string literal head. OPEN, like [`Head::Int`].
    Str(String),
}

/// A pattern abstracted for the usefulness algorithm: either a wildcard (which
/// also represents a variable binder and an always-matching record pattern) or a
/// head constructor applied to abstracted sub-patterns.
#[derive(Clone)]
enum UPat {
    Wild,
    Ctor(Head, Vec<Self>),
}

/// Abstract a resolved pattern into a [`UPat`]. Variables, wildcards, and
/// field-pun record patterns all become [`UPat::Wild`] (each matches its value
/// unconditionally and binds names only). Constructor and tuple patterns recurse.
fn to_upat(p: &canon::Pattern_) -> UPat {
    match p {
        canon::Pattern_::PAnything | canon::Pattern_::PVar(_) | canon::Pattern_::PRecord(_) => {
            UPat::Wild
        }
        canon::Pattern_::PCtor { name, args, .. } => UPat::Ctor(
            Head::Adt(*name),
            args.iter().map(|a| to_upat(&a.value)).collect(),
        ),
        canon::Pattern_::PTuple(elems) => UPat::Ctor(
            Head::Tuple(elems.len()),
            elems.iter().map(|e| to_upat(&e.value)).collect(),
        ),
        // Literal leaves abstract to a zero-arity head of their value.
        canon::Pattern_::PInt(n) => UPat::Ctor(Head::Int(*n), Vec::new()),
        canon::Pattern_::PBool(b) => UPat::Ctor(Head::Bool(*b), Vec::new()),
        canon::Pattern_::PChar(c) => UPat::Ctor(Head::Char(c.clone()), Vec::new()),
        canon::Pattern_::PStr(s) => UPat::Ctor(Head::Str(s.clone()), Vec::new()),
        // An alias is transparent for coverage — it matches exactly what its
        // inner pattern matches.
        canon::Pattern_::PAlias(inner, _) => to_upat(&inner.value),
        // List / cons patterns: a `case` containing one is excluded from the
        // usefulness walk by [`pattern_uses_unknown_ctor`] (list-pattern lowering
        // is a fail-closed gap, so no unsound code is emitted), so this arm is not
        // reached in practice; it abstracts to a wildcard to stay total.
        canon::Pattern_::PList(_) | canon::Pattern_::PCons(_, _) => UPat::Wild,
    }
}

/// Does `p` reference a name / shape this end-of-checking pass cannot analyse
/// soundly here? Two cases are excluded from the usefulness walk: a constructor
/// outside this module's unions (an imported / unknown enum whose full
/// constructor set is unavailable — the lowerer rejects the unknown scrutinee
/// enum separately), and a list / cons pattern (whose lowering is a fail-closed
/// not-yet gap, so the lowerer rejects it before any code is emitted — skipping
/// the coverage walk for it cannot let unsound code through).
fn pattern_uses_unknown_ctor(p: &canon::Pattern_, sigs: &Sigs) -> bool {
    match p {
        // Wildcards, variables, field-pun records, and literal leaves reference
        // no ADT constructor.
        canon::Pattern_::PAnything
        | canon::Pattern_::PVar(_)
        | canon::Pattern_::PRecord(_)
        | canon::Pattern_::PInt(_)
        | canon::Pattern_::PBool(_)
        | canon::Pattern_::PChar(_)
        | canon::Pattern_::PStr(_) => false,
        canon::Pattern_::PCtor { name, args, .. } => {
            !sigs.ctor_to_union.contains_key(name)
                || args
                    .iter()
                    .any(|a| pattern_uses_unknown_ctor(&a.value, sigs))
        }
        canon::Pattern_::PTuple(elems) => elems
            .iter()
            .any(|e| pattern_uses_unknown_ctor(&e.value, sigs)),
        canon::Pattern_::PAlias(inner, _) => pattern_uses_unknown_ctor(&inner.value, sigs),
        // List / cons patterns are gated at lowering; exclude their `case` from
        // the coverage walk (sound — no code is emitted for a gated feature).
        canon::Pattern_::PList(_) | canon::Pattern_::PCons(_, _) => true,
    }
}

/// Check every `case` in `module` for exhaustiveness + redundancy.
///
/// # Errors
/// * [`TypeError::RedundantCaseBranch`] when an arm covers no new value.
/// * [`TypeError::NonExhaustiveCase`] when the arms miss a value.
/// * [`Diagnostic::CompilerBug`] if a constructor symbol cannot be resolved.
pub fn check(module: &canon::Module, interner: &mut Interner) -> DResult<()> {
    let sigs = Sigs::build(module, interner)?;
    for def in &module.defs {
        let body = match def {
            canon::Def::Untyped { body, .. } | canon::Def::Typed { body, .. } => body,
        };
        check_expr(body, &sigs, interner)?;
    }
    Ok(())
}

/// Recursively check a single expression (and its sub-expressions) for `case`
/// defects. The recursion depth is bounded by the parser's nesting cap.
fn check_expr(e: &canon::Expr, sigs: &Sigs, interner: &Interner) -> DResult<()> {
    match &e.value {
        canon::Expr_::Int(_)
        | canon::Expr_::Float(_)
        | canon::Expr_::Str(_)
        | canon::Expr_::Char(_)
        | canon::Expr_::Unit
        | canon::Expr_::VarLocal(_)
        | canon::Expr_::VarTopLevel { .. }
        | canon::Expr_::VarKernel { .. }
        | canon::Expr_::VarCtor { .. } => Ok(()),
        canon::Expr_::Call(callee, args) => {
            check_expr(callee, sigs, interner)?;
            for a in args {
                check_expr(a, sigs, interner)?;
            }
            Ok(())
        }
        canon::Expr_::Binop { lhs, rhs, .. } => {
            check_expr(lhs, sigs, interner)?;
            check_expr(rhs, sigs, interner)
        }
        canon::Expr_::Case(scrut, branches) => {
            check_case(scrut, branches, sigs, interner)?;
            check_expr(scrut, sigs, interner)?;
            for br in branches {
                check_expr(&br.body, sigs, interner)?;
            }
            Ok(())
        }
        canon::Expr_::Let(bindings, body) => {
            // A `let` binder is irrefutable (a name or an irrefutable destructure);
            // there is no coverage obligation, only the binding bodies to recurse.
            for b in bindings {
                check_expr(&b.body, sigs, interner)?;
            }
            check_expr(body, sigs, interner)
        }
        canon::Expr_::If(branches, else_expr) => {
            for (cond, body) in branches {
                check_expr(cond, sigs, interner)?;
                check_expr(body, sigs, interner)?;
            }
            check_expr(else_expr, sigs, interner)
        }
        canon::Expr_::Tuple(elems) | canon::Expr_::List(elems) => {
            for elem in elems {
                check_expr(elem, sigs, interner)?;
            }
            Ok(())
        }
        canon::Expr_::Cons(head, tail) => {
            check_expr(head, sigs, interner)?;
            check_expr(tail, sigs, interner)
        }
        canon::Expr_::Record(fields) => {
            for (_, value) in fields {
                check_expr(value, sigs, interner)?;
            }
            Ok(())
        }
        canon::Expr_::Lambda(_, body) => check_expr(body, sigs, interner),
        canon::Expr_::Access(record, _) => check_expr(record, sigs, interner),
        canon::Expr_::Update(base, fields) => {
            check_expr(base, sigs, interner)?;
            for (_, value) in fields {
                check_expr(value, sigs, interner)?;
            }
            Ok(())
        }
    }
}

/// Check one `case`: first redundancy (a later arm useless against the earlier
/// ones), then exhaustiveness (the wildcard row useful against the whole arm
/// matrix). A `case` mentioning a constructor outside this module's unions is
/// skipped — its signature is unavailable, so it cannot be judged soundly here.
fn check_case(
    scrut: &canon::Expr,
    branches: &[canon::CaseBranch],
    sigs: &Sigs,
    interner: &Interner,
) -> DResult<()> {
    if branches
        .iter()
        .any(|br| pattern_uses_unknown_ctor(&br.pat.value, sigs))
    {
        return Ok(());
    }

    let rows: Vec<UPat> = branches.iter().map(|br| to_upat(&br.pat.value)).collect();

    // Redundancy: an arm is redundant when its pattern is not useful against the
    // arms before it (those already cover every value it would match). Reported
    // by the arm's top-level constructor name, mirroring the M3a diagnostic. The
    // prior-arm matrix grows one row per step, so no indexing is needed.
    let mut prior: Vec<Vec<UPat>> = Vec::with_capacity(rows.len());
    for (br, row) in branches.iter().zip(rows.iter()) {
        let q = [row.clone()];
        // A tuple / record arm is reported through the dedicated multi-arm
        // product gate at lowering (SKY-L0115), which gives a clearer message
        // than "redundant branch"; redundancy reporting covers the constructor /
        // literal / wildcard / variable / alias arm shapes.
        let is_product = matches!(
            br.pat.value,
            canon::Pattern_::PTuple(_) | canon::Pattern_::PRecord(_)
        );
        if !is_product && useful(&prior, &q, sigs, 1).is_empty() {
            return Err(Diagnostic::Type {
                span: br.pat.span,
                msg: TypeError::RedundantCaseBranch {
                    constructor: arm_label(&br.pat.value, interner)?,
                },
            });
        }
        prior.push(vec![row.clone()]);
    }

    // Exhaustiveness: the wildcard row is useful against the arm matrix exactly
    // when some value escapes every arm. Each witness is a missing pattern.
    let matrix: Vec<Vec<UPat>> = rows.into_iter().map(|p| vec![p]).collect();
    let witnesses = useful(&matrix, &[UPat::Wild], sigs, WITNESS_CAP);
    if witnesses.is_empty() {
        return Ok(());
    }

    let mut missing: Vec<Box<str>> = Vec::with_capacity(witnesses.len());
    for w in &witnesses {
        // Each witness is a single-column row; render its one pattern.
        let head = w.first().unwrap_or(&UPat::Wild);
        missing.push(render_upat(head, interner, false)?.into_boxed_str());
    }
    missing.dedup();

    Err(Diagnostic::Type {
        span: scrut.span,
        msg: TypeError::NonExhaustiveCase {
            missing: missing.into_boxed_slice(),
        },
    })
}

/// Maranget usefulness with witness collection. Returns up to `cap` witness rows
/// — each a value vector matched by `q` but by no row of `matrix`. An empty
/// result means `q` is not useful (every value it matches is already covered).
///
/// `matrix` rows and `q` all share the same width; the recursion peels one
/// column at a time. The implementation is total (no panic, no raw indexing).
fn useful(matrix: &[Vec<UPat>], q: &[UPat], sigs: &Sigs, cap: usize) -> Vec<Vec<UPat>> {
    if cap == 0 {
        return Vec::new();
    }
    let Some((first, rest_q)) = q.split_first() else {
        // Base case: the empty row is useful iff the matrix has no rows.
        return if matrix.is_empty() {
            vec![Vec::new()]
        } else {
            Vec::new()
        };
    };

    match first {
        UPat::Ctor(c, args) => {
            let specialised = specialise(matrix, c, sigs);
            let mut sub_q = args.clone();
            sub_q.extend_from_slice(rest_q);
            useful(&specialised, &sub_q, sigs, cap)
                .into_iter()
                .map(|w| rebuild(c, sigs.arity(c), w))
                .collect()
        }
        UPat::Wild => {
            let roots = column_heads(matrix);
            if let Some(signature) = complete_signature(&roots, sigs) {
                // The first column's constructors are complete: a witness must
                // refine the wildcard into one of them. Try each in turn.
                let mut out: Vec<Vec<UPat>> = Vec::new();
                for (head, arity) in signature {
                    let specialised = specialise(matrix, &head, sigs);
                    let mut sub_q = vec![UPat::Wild; arity];
                    sub_q.extend_from_slice(rest_q);
                    for w in useful(&specialised, &sub_q, sigs, cap - out.len()) {
                        out.push(rebuild(&head, arity, w));
                        if out.len() >= cap {
                            return out;
                        }
                    }
                }
                out
            } else {
                // The first column is missing constructors (or has none): drop it
                // via the default matrix and witness the gap with a missing head
                // (or a wildcard when the column carries no constructor).
                let defaulted = default_matrix(matrix);
                let tails = useful(&defaulted, rest_q, sigs, cap);
                if tails.is_empty() {
                    return Vec::new();
                }
                let heads = missing_heads(&roots, sigs);
                let mut out: Vec<Vec<UPat>> = Vec::new();
                for head in &heads {
                    for tail in &tails {
                        let mut row = Vec::with_capacity(tail.len() + 1);
                        row.push(head.clone());
                        row.extend_from_slice(tail);
                        out.push(row);
                        if out.len() >= cap {
                            return out;
                        }
                    }
                }
                out
            }
        }
    }
}

/// Specialise `matrix` by head constructor `c` (Maranget's `S(c, P)`): rows whose
/// first pattern is `c` expand its sub-patterns into the leading columns; rows
/// with a wildcard first contribute `arity(c)` fresh wildcards; rows with a
/// different head are dropped.
fn specialise(matrix: &[Vec<UPat>], c: &Head, sigs: &Sigs) -> Vec<Vec<UPat>> {
    let arity = sigs.arity(c);
    let mut out = Vec::new();
    for row in matrix {
        let Some((first, rest)) = row.split_first() else {
            continue;
        };
        match first {
            UPat::Ctor(head, args) if head == c => {
                let mut new_row = args.clone();
                new_row.extend_from_slice(rest);
                out.push(new_row);
            }
            UPat::Ctor(_, _) => {}
            UPat::Wild => {
                let mut new_row = vec![UPat::Wild; arity];
                new_row.extend_from_slice(rest);
                out.push(new_row);
            }
        }
    }
    out
}

/// The default matrix (Maranget's `D(P)`): keep only rows whose first pattern is
/// a wildcard, dropping that column.
fn default_matrix(matrix: &[Vec<UPat>]) -> Vec<Vec<UPat>> {
    let mut out = Vec::new();
    for row in matrix {
        if let Some((UPat::Wild, rest)) = row.split_first() {
            out.push(rest.to_vec());
        }
    }
    out
}

/// The distinct head constructors appearing in `matrix`'s first column, in
/// first-seen order.
fn column_heads(matrix: &[Vec<UPat>]) -> Vec<Head> {
    let mut seen: Vec<Head> = Vec::new();
    for row in matrix {
        if let Some(UPat::Ctor(head, _)) = row.first()
            && !seen.contains(head)
        {
            seen.push(head.clone());
        }
    }
    seen
}

/// If the column's `roots` form a complete constructor signature, return the full
/// signature to branch over (each head with its arity), in declaration order for
/// ADTs. A tuple column has a single constructor and is always complete. An empty
/// or constructor-incomplete column returns `None` (use the default matrix).
fn complete_signature(roots: &[Head], sigs: &Sigs) -> Option<Vec<(Head, usize)>> {
    let first = roots.first()?;
    match first {
        Head::Tuple(n) => Some(vec![(Head::Tuple(*n), *n)]),
        // `Bool` is closed: the signature is complete once both `True` and
        // `False` appear in the column.
        Head::Bool(_) => {
            let has_true = roots.contains(&Head::Bool(true));
            let has_false = roots.contains(&Head::Bool(false));
            if has_true && has_false {
                Some(vec![(Head::Bool(true), 0), (Head::Bool(false), 0)])
            } else {
                None
            }
        }
        // Int / Char / String are OPEN — a finite set of literals never covers
        // the type, so the signature is never complete (a wildcard is required).
        Head::Int(_) | Head::Char(_) | Head::Str(_) => None,
        Head::Adt(c) => {
            let union = sigs.ctor_to_union.get(c)?;
            let all = sigs.union_ctors.get(union)?;
            let present: BTreeSet<Symbol> = roots
                .iter()
                .filter_map(|h| match h {
                    Head::Adt(name) => Some(*name),
                    _ => None,
                })
                .collect();
            if all.iter().all(|(name, _)| present.contains(name)) {
                Some(
                    all.iter()
                        .map(|(name, ar)| (Head::Adt(*name), *ar))
                        .collect(),
                )
            } else {
                None
            }
        }
    }
}

/// The witness heads for an incomplete first column: each ADT constructor the
/// column is missing (with wildcard arguments), in declaration order — or a bare
/// wildcard when the column carries no constructor at all (nothing to refine).
fn missing_heads(roots: &[Head], sigs: &Sigs) -> Vec<UPat> {
    match roots.first() {
        // A `Bool` column missing one literal: the precise witness is that
        // literal (`True` / `False`), not a bare wildcard.
        Some(Head::Bool(_)) => {
            let mut out = Vec::new();
            if !roots.contains(&Head::Bool(true)) {
                out.push(UPat::Ctor(Head::Bool(true), Vec::new()));
            }
            if !roots.contains(&Head::Bool(false)) {
                out.push(UPat::Ctor(Head::Bool(false), Vec::new()));
            }
            if out.is_empty() {
                out.push(UPat::Wild);
            }
            out
        }
        Some(Head::Adt(c)) => {
            let Some(union) = sigs.ctor_to_union.get(c) else {
                return vec![UPat::Wild];
            };
            let Some(all) = sigs.union_ctors.get(union) else {
                return vec![UPat::Wild];
            };
            let present: BTreeSet<Symbol> = roots
                .iter()
                .filter_map(|h| match h {
                    Head::Adt(name) => Some(*name),
                    _ => None,
                })
                .collect();
            let mut out: Vec<UPat> = all
                .iter()
                .filter(|(name, _)| !present.contains(name))
                .map(|(name, ar)| UPat::Ctor(Head::Adt(*name), vec![UPat::Wild; *ar]))
                .collect();
            if out.is_empty() {
                out.push(UPat::Wild);
            }
            out
        }
        // An OPEN literal column (Int / Char / String) is only completed by a
        // catch-all → witness `_`. An empty column, or a tuple column (always
        // complete, never reaches here), also witness `_`.
        Some(Head::Int(_) | Head::Char(_) | Head::Str(_) | Head::Tuple(_)) | None => {
            vec![UPat::Wild]
        }
    }
}

/// Re-wrap a specialised witness `w` (its leading `arity` columns are the
/// constructor's arguments, the remainder is the tail) back into a row whose head
/// is `Ctor(head, args)`.
fn rebuild(head: &Head, arity: usize, mut w: Vec<UPat>) -> Vec<UPat> {
    let take = arity.min(w.len());
    let tail = w.split_off(take);
    let mut row = Vec::with_capacity(tail.len() + 1);
    row.push(UPat::Ctor(head.clone(), w));
    row.extend(tail);
    row
}

/// Render a witness pattern to its Sky surface spelling for the diagnostic.
/// `atom` requests parentheses when the pattern would otherwise be ambiguous as
/// a constructor argument (a non-nullary ADT application).
fn render_upat(p: &UPat, interner: &Interner, atom: bool) -> DResult<String> {
    match p {
        UPat::Wild => Ok("_".to_owned()),
        UPat::Ctor(Head::Bool(b), _) => Ok(if *b { "True" } else { "False" }.to_owned()),
        UPat::Ctor(Head::Int(n), _) => Ok(n.to_string()),
        UPat::Ctor(Head::Char(c), _) => Ok(format!("'{c}'")),
        UPat::Ctor(Head::Str(s), _) => Ok(format!("{s:?}")),
        UPat::Ctor(Head::Tuple(_), args) => {
            let mut parts = Vec::with_capacity(args.len());
            for a in args {
                parts.push(render_upat(a, interner, false)?);
            }
            Ok(format!("({})", parts.join(", ")))
        }
        UPat::Ctor(Head::Adt(name), args) => {
            let name = resolve(interner, *name)?;
            if args.is_empty() {
                return Ok(name.into());
            }
            let mut parts = Vec::with_capacity(args.len());
            for a in args {
                parts.push(render_upat(a, interner, true)?);
            }
            let inner = format!("{name} {}", parts.join(" "));
            if atom {
                Ok(format!("({inner})"))
            } else {
                Ok(inner)
            }
        }
    }
}

/// The surface spelling of a `case` arm's pattern, used to name a redundant arm
/// in the SKY-T0011 diagnostic. A constructor / variable resolves through the
/// interner; a literal spells itself; a wildcard is `_`; an alias spells its
/// inner pattern (the part that drives the match).
fn arm_label(p: &canon::Pattern_, interner: &Interner) -> DResult<Box<str>> {
    let s = match p {
        canon::Pattern_::PAnything => "_".to_owned(),
        canon::Pattern_::PVar(name) | canon::Pattern_::PCtor { name, .. } => {
            resolve(interner, *name)?.to_string()
        }
        canon::Pattern_::PInt(n) => n.to_string(),
        canon::Pattern_::PBool(b) => if *b { "True" } else { "False" }.to_owned(),
        canon::Pattern_::PChar(c) => format!("'{c}'"),
        canon::Pattern_::PStr(s) => format!("{s:?}"),
        canon::Pattern_::PAlias(inner, _) => return arm_label(&inner.value, interner),
        canon::Pattern_::PTuple(_) => "(…)".to_owned(),
        canon::Pattern_::PRecord(_) => "{…}".to_owned(),
        canon::Pattern_::PList(_) => "[…]".to_owned(),
        canon::Pattern_::PCons(_, _) => "_ :: _".to_owned(),
    };
    Ok(s.into_boxed_str())
}

/// Resolve a constructor symbol to an owned name, or a `CompilerBug` on a forged
/// symbol.
fn resolve(interner: &Interner, sym: Symbol) -> DResult<Box<str>> {
    interner
        .resolve(sym)
        .map(Box::from)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: STAGE,
            detail: format!("no backing string for constructor symbol {}", sym.as_raw()),
        })
}
