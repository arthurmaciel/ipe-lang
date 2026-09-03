//! Change-signature deltas: closed shape-delta set + call-site transformer.
//!
//! # What this module does
//!
//! Given a resolved [`ast::Expr_::Call`] node and a [`ShapeDelta`] value
//! (produced by a lint rule), it computes the set of source edits that
//! mechanically transform every call-site argument to the new signature shape,
//! or reports a [`ManualReviewSpan`] for call sites the engine cannot safely
//! rewrite.
//!
//! # Fail-closed invariant
//!
//! Every ambiguous or structurally opaque call site becomes a
//! [`ApplyOutcome::ManualReview`] span — never a guessed edit. A partial,
//! honest result beats a complete, wrong one.
//!
//! # Supported deltas
//!
//! The delta set is closed: only the variants of [`ShapeDelta`] are legal
//! transforms. Lint rules produce values of this type; the engine consumes
//! them. No open-coded text surgery is possible.
//!
//! | Delta | Effect |
//! |---|---|
//! | [`ShapeDelta::WrapPrimitive`] | Wrap one positional arg in a constructor: `f "x"` → `f (Ctor "x")` |
//! | [`ShapeDelta::GroupAdjacentBoolsIntoRecord`] | Replace two adjacent `Bool` args with a named record: `f True False` → `f { a = True, b = False }` |
//! | [`ShapeDelta::Reorder`] | Reorder positional args by index permutation: `f a b c` → `f b a c` |
//! | [`ShapeDelta::InsertDefault`] | Insert a new arg at a given position using a literal default value |
//!
//! # Source text
//!
//! The transformer reads argument source text via spans over the supplied
//! `source` byte slice (the containing module's UTF-8 source). Every edit
//! replaces a span in that source; the caller applies edits atomically.

use ipe_diagnostics::Span;
use ipe_intern::Symbol;

use crate::ast::{Expr, Expr_};
use crate::rename::{Edit, EditSet};

// ── Public types ──────────────────────────────────────────────────────────────

/// A closed set of mechanical argument-shape transforms a lint rule may declare.
///
/// Variants are deliberately non-extensible from outside this module: new
/// deltas require a deliberate addition here and a corresponding test.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ShapeDelta {
    /// Wrap the argument at `arg_index` in a constructor application.
    ///
    /// `f primitive_value` → `f (Ctor primitive_value)`
    ///
    /// `ctor_name` is the constructor identifier to prepend. The engine wraps
    /// only a *simple* argument (literal, variable, or constructor application
    /// with no trailing unresolved spread); any other shape → manual review.
    WrapPrimitive {
        /// Zero-based index of the argument to wrap.
        arg_index: usize,
        /// Constructor name to wrap with, e.g. `"Tag"`.
        ctor_name: String,
    },

    /// Collapse two adjacent boolean arguments into a named record.
    ///
    /// `f True False` → `f { field_a = True, field_b = False }`
    ///
    /// Both arguments must be statically boolean literals or variables; any
    /// other shape (lambda, point-free, spread) → manual review.
    GroupAdjacentBoolsIntoRecord {
        /// Zero-based index of the first of the two adjacent bool arguments.
        first_arg_index: usize,
        /// Record field name for the first argument.
        field_a: String,
        /// Record field name for the second argument.
        field_b: String,
    },

    /// Reorder positional arguments according to a permutation.
    ///
    /// `permutation[new_index] = old_index` — i.e. the argument currently at
    /// `permutation[i]` moves to position `i` in the rewritten call.
    ///
    /// Errors if the permutation length does not equal the call's argument
    /// count or contains an out-of-range index → manual review.
    Reorder {
        /// `permutation[i]` = index of the argument to place at position `i`.
        permutation: Vec<usize>,
    },

    /// Insert a new argument at `arg_index` carrying the literal `default_text`
    /// source value (e.g. `"Nothing"`, `"0"`, `"\"\""`).
    ///
    /// Existing arguments at `>= arg_index` shift right.
    InsertDefault {
        /// Zero-based position where the new argument is inserted.
        arg_index: usize,
        /// Literal source text of the default value to insert.
        default_text: String,
    },
}

/// A span that requires human review because the engine cannot mechanically
/// rewrite it with certainty.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ManualReviewSpan {
    /// Module path of the file containing the call site.
    pub file: Vec<Symbol>,
    /// Byte range of the entire call expression that needs manual attention.
    pub call_span: Span,
    /// Human-readable reason the engine declined to rewrite this site.
    pub reason: String,
}

/// The outcome of applying a [`ShapeDelta`] to one call site.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ApplyOutcome {
    /// The engine produced a set of edits that mechanically rewrite the call.
    Edits(EditSet),
    /// The engine cannot safely rewrite this site; human review required.
    ManualReview(ManualReviewSpan),
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Apply `delta` to the call site `call_expr` in module `file`.
///
/// `source` is the UTF-8 source of the file identified by `file`. The
/// `call_expr` must be an [`Expr_::Call`] node; passing any other variant
/// yields a [`ApplyOutcome::ManualReview`] (fail-closed, not a panic).
///
/// Returns [`ApplyOutcome::Edits`] when every affected argument can be
/// mechanically rewritten, or [`ApplyOutcome::ManualReview`] when the call
/// site's shape prevents a safe automatic transform.
#[must_use]
pub fn apply_sig_delta(
    source: &str,
    file: &[Symbol],
    call_expr: &Expr,
    delta: &ShapeDelta,
) -> ApplyOutcome {
    let Expr_::Call(_, args) = &call_expr.value else {
        return ApplyOutcome::ManualReview(ManualReviewSpan {
            file: file.to_owned(),
            call_span: call_expr.span,
            reason: "expression is not a function call".to_owned(),
        });
    };

    match delta {
        ShapeDelta::WrapPrimitive {
            arg_index,
            ctor_name,
        } => apply_wrap_primitive(source, file, call_expr.span, args, *arg_index, ctor_name),

        ShapeDelta::GroupAdjacentBoolsIntoRecord {
            first_arg_index,
            field_a,
            field_b,
        } => apply_group_bools(
            source,
            file,
            call_expr.span,
            args,
            *first_arg_index,
            field_a,
            field_b,
        ),

        ShapeDelta::Reorder { permutation } => {
            apply_reorder(source, file, call_expr.span, args, permutation)
        }

        ShapeDelta::InsertDefault {
            arg_index,
            default_text,
        } => apply_insert_default(source, file, call_expr.span, args, *arg_index, default_text),
    }
}

// ── Delta implementations ─────────────────────────────────────────────────────

fn apply_wrap_primitive(
    source: &str,
    file: &[Symbol],
    call_span: Span,
    args: &[Expr],
    arg_index: usize,
    ctor_name: &str,
) -> ApplyOutcome {
    let Some(arg) = args.get(arg_index) else {
        return manual_review(
            file,
            call_span,
            format!(
                "argument index {arg_index} out of range (call has {} argument(s))",
                args.len()
            ),
        );
    };

    // Fail-closed: only wrap simple shapes. A lambda or point-free partial
    // application cannot be mechanically parenthesised without type context.
    if is_opaque(arg) {
        return manual_review(
            file,
            call_span,
            format!(
                "argument {arg_index} has a complex shape (lambda or partial application); \
                 cannot wrap mechanically"
            ),
        );
    }

    let arg_text = span_text(source, arg.span);
    let Some(arg_text) = arg_text else {
        return manual_review(
            file,
            call_span,
            format!("argument {arg_index} span is out of bounds for the supplied source"),
        );
    };

    // Wrap: replace the argument span with `(Ctor <arg_text>)`.
    // If the argument is already parenthesised (starts with `(`), we still
    // wrap — the result is `(Ctor (inner))`, which is always unambiguous.
    let replacement = format!("({ctor_name} {arg_text})");

    ApplyOutcome::Edits(EditSet {
        edits: vec![Edit {
            file: file.to_owned(),
            span: arg.span,
            replacement,
        }],
    })
}

fn apply_group_bools(
    source: &str,
    file: &[Symbol],
    call_span: Span,
    args: &[Expr],
    first_arg_index: usize,
    field_a: &str,
    field_b: &str,
) -> ApplyOutcome {
    let second_arg_index = first_arg_index.saturating_add(1);

    let (Some(arg_a), Some(arg_b)) = (args.get(first_arg_index), args.get(second_arg_index)) else {
        return manual_review(
            file,
            call_span,
            format!(
                "expected two adjacent arguments at indices {first_arg_index}+{second_arg_index}, \
                 but call has {} argument(s)",
                args.len()
            ),
        );
    };

    // Fail-closed: both must be simple (no lambda/partial-application).
    if is_opaque(arg_a) || is_opaque(arg_b) {
        return manual_review(
            file,
            call_span,
            format!(
                "one or both arguments at indices {first_arg_index}/{second_arg_index} have \
                 complex shapes; cannot group mechanically"
            ),
        );
    }

    let (Some(text_a), Some(text_b)) =
        (span_text(source, arg_a.span), span_text(source, arg_b.span))
    else {
        return manual_review(
            file,
            call_span,
            "argument span(s) out of bounds for the supplied source".to_owned(),
        );
    };

    // The two argument spans may not be contiguous in source (whitespace between
    // them). We replace the wider span [arg_a.span.lo, arg_b.span.hi) with the
    // record literal, collapsing both into one edit.
    let merged_span = Span::new(arg_a.span.lo, arg_b.span.hi);
    let replacement = format!("{{ {field_a} = {text_a}, {field_b} = {text_b} }}");

    ApplyOutcome::Edits(EditSet {
        edits: vec![Edit {
            file: file.to_owned(),
            span: merged_span,
            replacement,
        }],
    })
}

fn apply_reorder(
    source: &str,
    file: &[Symbol],
    call_span: Span,
    args: &[Expr],
    permutation: &[usize],
) -> ApplyOutcome {
    if permutation.len() != args.len() {
        return manual_review(
            file,
            call_span,
            format!(
                "permutation length {} does not match argument count {}",
                permutation.len(),
                args.len()
            ),
        );
    }

    // Validate permutation is a bijection over [0, n).
    let n = args.len();
    let mut seen = vec![false; n];
    for &idx in permutation {
        if idx >= n {
            return manual_review(
                file,
                call_span,
                format!("permutation index {idx} out of range for {n} argument(s)"),
            );
        }
        if seen.get(idx).copied().unwrap_or(false) {
            return manual_review(
                file,
                call_span,
                format!("permutation index {idx} appears more than once — not a valid bijection"),
            );
        }
        if let Some(slot) = seen.get_mut(idx) {
            *slot = true;
        }
    }

    // Fail-closed: any opaque arg → manual review.
    for (i, arg) in args.iter().enumerate() {
        if is_opaque(arg) {
            return manual_review(
                file,
                call_span,
                format!(
                    "argument {i} has a complex shape (lambda or partial application); \
                     cannot reorder mechanically"
                ),
            );
        }
    }

    // Extract source text for every argument.
    let mut texts: Vec<&str> = Vec::with_capacity(n);
    for (i, arg) in args.iter().enumerate() {
        match span_text(source, arg.span) {
            Some(t) => texts.push(t),
            None => {
                return manual_review(
                    file,
                    call_span,
                    format!("argument {i} span is out of bounds for the supplied source"),
                );
            }
        }
    }

    // Emit one edit per argument position where the content changes.
    // Safety: `new_pos` is a loop index into `permutation` (length == `n` == `args.len()`),
    // and `old_pos` was range-checked against `n` above, so both `.get()` calls always
    // return `Some`. The `?`-like `.and_then` path returns `manual_review` on any
    // unexpected None (fail-closed).
    let mut edits: Vec<Edit> = Vec::new();
    for (new_pos, &old_pos) in permutation.iter().enumerate() {
        if new_pos == old_pos {
            continue;
        }
        let (Some(arg_at_new), Some(old_text)) = (args.get(new_pos), texts.get(old_pos)) else {
            return manual_review(
                file,
                call_span,
                format!("internal: slot {new_pos}/{old_pos} out of range during reorder"),
            );
        };
        edits.push(Edit {
            file: file.to_owned(),
            span: arg_at_new.span,
            replacement: (*old_text).to_owned(),
        });
    }

    ApplyOutcome::Edits(EditSet { edits })
}

fn apply_insert_default(
    source: &str,
    file: &[Symbol],
    call_span: Span,
    args: &[Expr],
    arg_index: usize,
    default_text: &str,
) -> ApplyOutcome {
    if arg_index > args.len() {
        return manual_review(
            file,
            call_span,
            format!(
                "insert position {arg_index} out of range (call has {} argument(s))",
                args.len()
            ),
        );
    }

    // The insertion point: we insert before the argument currently at
    // `arg_index`, or append after all args when `arg_index == args.len()`.
    //
    // Strategy: emit a zero-width edit at the insertion byte offset that
    // prepends `default_text ` (with a trailing space) before the existing arg,
    // or appends ` default_text` (with a leading space) after the last arg.
    //
    // Returns `(text, span)` or propagates a `ManualReview` early return.
    let (insertion_text, insertion_span) = if let Some(before) = args.get(arg_index) {
        // Insert before the argument at arg_index.
        if span_text(source, before.span).is_none() {
            return manual_review(
                file,
                call_span,
                format!("argument {arg_index} span is out of bounds for the supplied source"),
            );
        }
        let sp = Span::new(before.span.lo, before.span.lo);
        (format!("{default_text} "), sp)
    } else {
        // Append after the last arg (arg_index == args.len()).
        // When there are no args yet, insert at call_span.hi (zero-width).
        let insert_lo = if let Some(last) = args.last() {
            if span_text(source, last.span).is_none() {
                return manual_review(
                    file,
                    call_span,
                    "last argument span is out of bounds for the supplied source".to_owned(),
                );
            }
            last.span.hi
        } else {
            call_span.hi
        };
        (format!(" {default_text}"), Span::new(insert_lo, insert_lo))
    };

    ApplyOutcome::Edits(EditSet {
        edits: vec![Edit {
            file: file.to_owned(),
            span: insertion_span,
            replacement: insertion_text,
        }],
    })
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Whether an expression is structurally opaque to mechanical rewriting.
///
/// Lambdas and point-free partial applications cannot be mechanically
/// parenthesised or regrouped without type context. Fail-closed: returning
/// `true` here causes the call site to be flagged for manual review.
const fn is_opaque(expr: &Expr) -> bool {
    matches!(expr.value, Expr_::Lambda(_, _))
}

/// Extract the source slice for `span`, or `None` when the span is out of
/// bounds for `source` or would produce a non-UTF-8 boundary.
fn span_text(source: &str, span: Span) -> Option<&str> {
    let lo = span.lo as usize;
    let hi = span.hi as usize;
    source.get(lo..hi)
}

/// Construct a [`ApplyOutcome::ManualReview`] with the given reason.
fn manual_review(file: &[Symbol], call_span: Span, reason: String) -> ApplyOutcome {
    ApplyOutcome::ManualReview(ManualReviewSpan {
        file: file.to_owned(),
        call_span,
        reason,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    //! TDD: each delta over representative call sites; an untransformable site
    //! becomes a manual-review span.
    //!
    //! Fixture setup: we construct minimal canonical [`Expr`] trees directly
    //! (no full parse+canonicalise round-trip) and pair them with source strings
    //! whose byte offsets match the [`Span`]s in the tree.
    //!
    //! Invariants asserted per delta:
    //!
    //! * **Happy path** — expected edits produced with correct spans + text.
    //! * **Opaque arg** — lambda arg → `ManualReview`.
    //! * **Out-of-range index** — index ≥ arg count → `ManualReview`.
    //! * **Span OOB** — span outside source bytes → `ManualReview`.
    //! * **Non-call expr** — passing a non-`Call` node → `ManualReview`.

    use ipe_diagnostics::{Located, Span};
    use ipe_intern::{Interner, Symbol};

    use crate::ast::{Expr, Expr_};
    use crate::sig_delta::{ApplyOutcome, ShapeDelta, apply_sig_delta};

    // ── Fixture helpers ───────────────────────────────────────────────────────

    fn sym(interner: &mut Interner, s: &str) -> Symbol {
        interner.intern(s).expect("intern ok in test")
    }

    /// Build a `Located<T>` at a given byte span.
    fn at<T>(lo: u32, hi: u32, value: T) -> Located<T> {
        Located::new(Span::new(lo, hi), value)
    }

    /// A string-literal `Expr` at the given byte range.
    fn str_arg(lo: u32, hi: u32, text: &str) -> Expr {
        at(lo, hi, Expr_::Str(text.to_owned()))
    }

    /// An int-literal `Expr` at the given byte range.
    fn int_arg(lo: u32, hi: u32, v: i64) -> Expr {
        at(lo, hi, Expr_::Int(v))
    }

    /// A bool constructor `Expr` (`True`/`False`) at the given byte range.
    fn bool_arg(interner: &mut Interner, lo: u32, hi: u32, val: bool) -> Expr {
        let name = sym(interner, if val { "True" } else { "False" });
        let home = vec![sym(interner, "Basics")];
        let type_name = sym(interner, "Bool");
        at(
            lo,
            hi,
            Expr_::VarCtor {
                home,
                type_name,
                name,
                index: usize::from(!val),
            },
        )
    }

    /// A variable `Expr` at the given byte range.
    fn var_arg(interner: &mut Interner, lo: u32, hi: u32, name: &str) -> Expr {
        let s = sym(interner, name);
        at(lo, hi, Expr_::VarLocal(s))
    }

    /// A lambda `Expr` — opaque to mechanical rewriting.
    fn lambda_arg(interner: &mut Interner, lo: u32, hi: u32) -> Expr {
        let x = sym(interner, "x");
        let pat = at(lo, lo + 1, crate::ast::Pattern_::PVar(x));
        let body = var_arg(interner, lo + 5, hi, "x");
        at(lo, hi, Expr_::Lambda(vec![pat], Box::new(body)))
    }

    /// A minimal `Call` expr wrapping `callee` + `args`.
    fn call_expr(lo: u32, hi: u32, callee: Expr, args: Vec<Expr>) -> Expr {
        at(lo, hi, Expr_::Call(Box::new(callee), args))
    }

    /// A file path (single module segment "Main").
    fn file(interner: &mut Interner) -> Vec<Symbol> {
        vec![sym(interner, "Main")]
    }

    // ── WrapPrimitive ─────────────────────────────────────────────────────────

    /// Wrap the second argument (index 1) of `f "hello" "world"` in `Tag`.
    ///
    /// Source: `f "hello" "world"` (18 bytes)
    ///         0123456789012345678
    ///         f = [0,1), "hello" = [2,9), "world" = [10,17)
    #[test]
    fn wrap_primitive_replaces_correct_arg_span() {
        let mut interner = Interner::default();
        let file = file(&mut interner);

        // source: `f "hello" "world"`
        let source = r#"f "hello" "world""#;
        //              0         1
        //              0123456789012345678
        // f = [0,1), "hello" = [2,9), "world" = [10,17)

        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = str_arg(2, 9, "hello");
        let arg1 = str_arg(10, 17, "world");
        let call = call_expr(0, 17, callee, vec![arg0, arg1]);

        let delta = ShapeDelta::WrapPrimitive {
            arg_index: 1,
            ctor_name: "Tag".to_owned(),
        };

        let outcome = apply_sig_delta(source, &file, &call, &delta);
        assert!(
            matches!(outcome, ApplyOutcome::Edits(_)),
            "expected Edits, got ManualReview"
        );
        let ApplyOutcome::Edits(edit_set) = outcome else {
            return;
        };
        assert_eq!(edit_set.edits.len(), 1);
        let edit = edit_set.edits.first().expect("one edit");
        assert_eq!(
            edit.span,
            Span::new(10, 17),
            "must target the second arg span"
        );
        assert_eq!(edit.replacement, r#"(Tag "world")"#);
    }

    /// Wrapping the first (and only) argument.
    #[test]
    fn wrap_primitive_first_arg() {
        let mut interner = Interner::default();
        let file = file(&mut interner);

        // source: `validate 42`
        //          0        9 11
        let source = "validate 42";
        let callee = var_arg(&mut interner, 0, 8, "validate");
        let arg0 = int_arg(9, 11, 42);
        let call = call_expr(0, 11, callee, vec![arg0]);

        let delta = ShapeDelta::WrapPrimitive {
            arg_index: 0,
            ctor_name: "Id".to_owned(),
        };
        let outcome = apply_sig_delta(source, &file, &call, &delta);
        assert!(matches!(outcome, ApplyOutcome::Edits(_)), "expected Edits");
        let ApplyOutcome::Edits(es) = outcome else {
            return;
        };
        assert_eq!(es.edits.len(), 1);
        let e = es.edits.first().expect("one edit");
        assert_eq!(e.span, Span::new(9, 11));
        assert_eq!(e.replacement, "(Id 42)");
    }

    /// A lambda argument at the target index → `ManualReview`.
    #[test]
    fn wrap_primitive_opaque_arg_is_manual_review() {
        let mut interner = Interner::default();
        let file = file(&mut interner);

        let source = "f (\\x -> x)";
        //             0 1         11
        let callee = var_arg(&mut interner, 0, 1, "f");
        let lam = lambda_arg(&mut interner, 3, 10);
        let call = call_expr(0, 11, callee, vec![lam]);

        let delta = ShapeDelta::WrapPrimitive {
            arg_index: 0,
            ctor_name: "Tag".to_owned(),
        };
        let outcome = apply_sig_delta(source, &file, &call, &delta);
        assert!(
            matches!(outcome, ApplyOutcome::ManualReview(_)),
            "lambda arg must yield ManualReview"
        );
    }

    /// Out-of-range `arg_index` → `ManualReview`.
    #[test]
    fn wrap_primitive_index_out_of_range() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f 1";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = int_arg(2, 3, 1);
        let call = call_expr(0, 3, callee, vec![arg0]);

        let delta = ShapeDelta::WrapPrimitive {
            arg_index: 5,
            ctor_name: "X".to_owned(),
        };
        assert!(matches!(
            apply_sig_delta(source, &file, &call, &delta),
            ApplyOutcome::ManualReview(_)
        ));
    }

    /// Span out of bounds for source → `ManualReview`.
    #[test]
    fn wrap_primitive_span_oob_is_manual_review() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        // Source only 3 bytes but arg span says [10,15).
        let source = "f 1";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = int_arg(10, 15, 1); // span OOB
        let call = call_expr(0, 15, callee, vec![arg0]);

        let delta = ShapeDelta::WrapPrimitive {
            arg_index: 0,
            ctor_name: "X".to_owned(),
        };
        assert!(matches!(
            apply_sig_delta(source, &file, &call, &delta),
            ApplyOutcome::ManualReview(_)
        ));
    }

    /// Passing a non-`Call` expr → `ManualReview`.
    #[test]
    fn non_call_expr_is_manual_review() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "42";
        let expr = int_arg(0, 2, 42);
        let delta = ShapeDelta::WrapPrimitive {
            arg_index: 0,
            ctor_name: "X".to_owned(),
        };
        assert!(matches!(
            apply_sig_delta(source, &file, &expr, &delta),
            ApplyOutcome::ManualReview(_)
        ));
    }

    // ── GroupAdjacentBoolsIntoRecord ──────────────────────────────────────────

    /// Group two bool args at indices 0,1 into `{ enabled = True, visible = False }`.
    ///
    /// source: `f True False`
    ///          0 2    7    12
    #[test]
    fn group_bools_produces_record_literal() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f True False";
        //             0 2    7    12

        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = bool_arg(&mut interner, 2, 6, true);
        let arg1 = bool_arg(&mut interner, 7, 12, false);
        let call = call_expr(0, 12, callee, vec![arg0, arg1]);

        let delta = ShapeDelta::GroupAdjacentBoolsIntoRecord {
            first_arg_index: 0,
            field_a: "enabled".to_owned(),
            field_b: "visible".to_owned(),
        };

        let outcome = apply_sig_delta(source, &file, &call, &delta);
        assert!(
            matches!(outcome, ApplyOutcome::Edits(_)),
            "expected Edits, got ManualReview"
        );
        let ApplyOutcome::Edits(es) = outcome else {
            return;
        };
        assert_eq!(es.edits.len(), 1, "one merged edit");
        let edit = es.edits.first().expect("one edit");
        // Merged span covers both args: [2, 12).
        assert_eq!(edit.span, Span::new(2, 12));
        assert_eq!(edit.replacement, "{ enabled = True, visible = False }");
    }

    /// Second arg index out of range → `ManualReview`.
    #[test]
    fn group_bools_missing_second_arg() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f True";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = bool_arg(&mut interner, 2, 6, true);
        let call = call_expr(0, 6, callee, vec![arg0]);

        let delta = ShapeDelta::GroupAdjacentBoolsIntoRecord {
            first_arg_index: 0,
            field_a: "a".to_owned(),
            field_b: "b".to_owned(),
        };
        assert!(matches!(
            apply_sig_delta(source, &file, &call, &delta),
            ApplyOutcome::ManualReview(_)
        ));
    }

    /// Lambda as one of the two grouped args → `ManualReview`.
    #[test]
    fn group_bools_opaque_second_arg() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f True (\\x -> x)";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = bool_arg(&mut interner, 2, 6, true);
        let arg1 = lambda_arg(&mut interner, 8, 16);
        let call = call_expr(0, 16, callee, vec![arg0, arg1]);

        let delta = ShapeDelta::GroupAdjacentBoolsIntoRecord {
            first_arg_index: 0,
            field_a: "a".to_owned(),
            field_b: "b".to_owned(),
        };
        assert!(matches!(
            apply_sig_delta(source, &file, &call, &delta),
            ApplyOutcome::ManualReview(_)
        ));
    }

    // ── Reorder ───────────────────────────────────────────────────────────────

    /// Reorder `f a b c` → `f c a b` via permutation [2, 0, 1].
    ///
    /// source: `f a b c`
    ///          0 2 4 6
    #[test]
    fn reorder_produces_correct_edits() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f a b c";
        //             0 2 4 6

        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = var_arg(&mut interner, 2, 3, "a");
        let arg1 = var_arg(&mut interner, 4, 5, "b");
        let arg2 = var_arg(&mut interner, 6, 7, "c");
        let call = call_expr(0, 7, callee, vec![arg0, arg1, arg2]);

        // permutation[i] = old index for new position i.
        // new[0] = old[2]=c, new[1] = old[0]=a, new[2] = old[1]=b → [2,0,1]
        let delta = ShapeDelta::Reorder {
            permutation: vec![2, 0, 1],
        };

        let outcome = apply_sig_delta(source, &file, &call, &delta);
        assert!(matches!(outcome, ApplyOutcome::Edits(_)), "expected Edits");
        let ApplyOutcome::Edits(es) = outcome else {
            return;
        };
        // All three positions differ — 3 edits.
        assert_eq!(es.edits.len(), 3);
        // Position 0 (span [2,3)) gets text of old[2] = "c".
        let e0 = es
            .edits
            .iter()
            .find(|e| e.span == Span::new(2, 3))
            .expect("edit at span [2,3)");
        assert_eq!(e0.replacement, "c");
        // Position 1 (span [4,5)) gets text of old[0] = "a".
        let e1 = es
            .edits
            .iter()
            .find(|e| e.span == Span::new(4, 5))
            .expect("edit at span [4,5)");
        assert_eq!(e1.replacement, "a");
        // Position 2 (span [6,7)) gets text of old[1] = "b".
        let e2 = es
            .edits
            .iter()
            .find(|e| e.span == Span::new(6, 7))
            .expect("edit at span [6,7)");
        assert_eq!(e2.replacement, "b");
    }

    /// Identity permutation produces zero edits (no-op).
    #[test]
    fn reorder_identity_no_edits() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f a b";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = var_arg(&mut interner, 2, 3, "a");
        let arg1 = var_arg(&mut interner, 4, 5, "b");
        let call = call_expr(0, 5, callee, vec![arg0, arg1]);

        let delta = ShapeDelta::Reorder {
            permutation: vec![0, 1],
        };
        let outcome = apply_sig_delta(source, &file, &call, &delta);
        assert!(matches!(outcome, ApplyOutcome::Edits(_)), "expected Edits");
        let ApplyOutcome::Edits(es) = outcome else {
            return;
        };
        assert_eq!(es.edits.len(), 0, "identity permutation → zero edits");
    }

    /// Permutation length mismatch → `ManualReview`.
    #[test]
    fn reorder_length_mismatch() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f a b";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = var_arg(&mut interner, 2, 3, "a");
        let arg1 = var_arg(&mut interner, 4, 5, "b");
        let call = call_expr(0, 5, callee, vec![arg0, arg1]);

        let delta = ShapeDelta::Reorder {
            permutation: vec![0, 1, 2], // 3 items for 2-arg call
        };
        assert!(matches!(
            apply_sig_delta(source, &file, &call, &delta),
            ApplyOutcome::ManualReview(_)
        ));
    }

    /// Duplicate index in permutation → `ManualReview`.
    #[test]
    fn reorder_duplicate_index_in_permutation() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f a b";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = var_arg(&mut interner, 2, 3, "a");
        let arg1 = var_arg(&mut interner, 4, 5, "b");
        let call = call_expr(0, 5, callee, vec![arg0, arg1]);

        let delta = ShapeDelta::Reorder {
            permutation: vec![0, 0], // 0 appears twice
        };
        assert!(matches!(
            apply_sig_delta(source, &file, &call, &delta),
            ApplyOutcome::ManualReview(_)
        ));
    }

    /// Lambda arg in a reorder → `ManualReview`.
    #[test]
    fn reorder_opaque_arg() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f (\\x -> x) b";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = lambda_arg(&mut interner, 3, 11);
        let arg1 = var_arg(&mut interner, 12, 13, "b");
        let call = call_expr(0, 13, callee, vec![arg0, arg1]);

        let delta = ShapeDelta::Reorder {
            permutation: vec![1, 0],
        };
        assert!(matches!(
            apply_sig_delta(source, &file, &call, &delta),
            ApplyOutcome::ManualReview(_)
        ));
    }

    // ── InsertDefault ─────────────────────────────────────────────────────────

    /// Insert `Nothing` before the first existing argument.
    ///
    /// source: `f 42`  →  edit inserts `Nothing ` before `42`
    #[test]
    fn insert_default_prepends_before_first_arg() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f 42";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = int_arg(2, 4, 42);
        let call = call_expr(0, 4, callee, vec![arg0]);

        let delta = ShapeDelta::InsertDefault {
            arg_index: 0,
            default_text: "Nothing".to_owned(),
        };

        let outcome = apply_sig_delta(source, &file, &call, &delta);
        assert!(matches!(outcome, ApplyOutcome::Edits(_)), "expected Edits");
        let ApplyOutcome::Edits(es) = outcome else {
            return;
        };
        assert_eq!(es.edits.len(), 1);
        let e = es.edits.first().expect("one edit");
        // Zero-width edit at lo=2 inserts `Nothing ` before `42`.
        assert_eq!(e.span, Span::new(2, 2));
        assert_eq!(e.replacement, "Nothing ");
    }

    /// Append `Nothing` after all existing arguments.
    ///
    /// source: `f 42`  →  edit appends ` Nothing` after `42`
    #[test]
    fn insert_default_appends_after_last_arg() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f 42";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = int_arg(2, 4, 42);
        let call = call_expr(0, 4, callee, vec![arg0]);

        let delta = ShapeDelta::InsertDefault {
            arg_index: 1, // after last (1-arg call → append)
            default_text: "Nothing".to_owned(),
        };

        let outcome = apply_sig_delta(source, &file, &call, &delta);
        assert!(matches!(outcome, ApplyOutcome::Edits(_)), "expected Edits");
        let ApplyOutcome::Edits(es) = outcome else {
            return;
        };
        assert_eq!(es.edits.len(), 1);
        let e = es.edits.first().expect("one edit");
        // Zero-width edit at hi=4 appends ` Nothing`.
        assert_eq!(e.span, Span::new(4, 4));
        assert_eq!(e.replacement, " Nothing");
    }

    /// Insert into zero-arg call (append case).
    #[test]
    fn insert_default_into_zero_arg_call() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let call = call_expr(0, 1, callee, vec![]);

        let delta = ShapeDelta::InsertDefault {
            arg_index: 0,
            default_text: "42".to_owned(),
        };

        let outcome = apply_sig_delta(source, &file, &call, &delta);
        assert!(matches!(outcome, ApplyOutcome::Edits(_)), "expected Edits");
        let ApplyOutcome::Edits(es) = outcome else {
            return;
        };
        assert_eq!(es.edits.len(), 1);
        assert_eq!(es.edits.first().expect("one edit").replacement, " 42");
    }

    /// `arg_index` beyond append position → `ManualReview`.
    #[test]
    fn insert_default_index_too_large() {
        let mut interner = Interner::default();
        let file = file(&mut interner);
        let source = "f 1";
        let callee = var_arg(&mut interner, 0, 1, "f");
        let arg0 = int_arg(2, 3, 1);
        let call = call_expr(0, 3, callee, vec![arg0]);

        let delta = ShapeDelta::InsertDefault {
            arg_index: 5, // way beyond
            default_text: "x".to_owned(),
        };
        assert!(matches!(
            apply_sig_delta(source, &file, &call, &delta),
            ApplyOutcome::ManualReview(_)
        ));
    }
}
