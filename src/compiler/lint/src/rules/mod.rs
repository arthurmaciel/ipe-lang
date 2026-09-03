//! The shipped rule set and the shared walk context.
//!
//! Every rule is a pure function `Ctx -> Vec<Finding>`. [`run_all`] invokes each
//! shipped rule in registry order and concatenates the results; the engine
//! ([`crate::run`]) then filters by configured severity and inline suppression
//! and sorts. Rules never mutate the AST or the source — a rewrite is expressed
//! only as a [`crate::Fix`] the engine applies later.

mod adjacent_bools;
mod prefer_pipeline;
mod prim_param;
mod unsafe_convention;
mod wrapper_consistency;

use ipe_diagnostics::{Located, Span};
use ipe_intern::{Interner, Symbol};
use ipe_syntax::{Module, TypeAnnotation, Value};

use crate::finding::{Finding, SigFix};

/// The read-only context every rule shares for one module: its path, its source
/// text (for span-based `--fix` slicing), the interner that parsed it, and the
/// parsed AST.
pub struct Ctx<'a> {
    /// The owning module's dotted path segments.
    pub module: &'a [String],
    /// The module's full source text.
    pub source: &'a str,
    /// The interner used to parse this module — resolves every [`Symbol`].
    pub interner: &'a Interner,
    /// The parsed module AST.
    pub ast: &'a Module,
}

impl Ctx<'_> {
    /// Resolve a [`Symbol`] to its interned text, or `""` when unresolvable
    /// (never expected for a parser-produced symbol).
    pub fn text(&self, sym: Symbol) -> &str {
        self.interner.resolve(sym).unwrap_or("")
    }

    /// A finding for this module carrying no fix.
    pub fn advisory(
        &self,
        rule: &'static str,
        span: Span,
        message: String,
        help: Vec<String>,
    ) -> Finding {
        Finding {
            rule,
            module: self.module.to_vec(),
            span,
            message,
            help,
            fix: None,
            sig_fix: None,
        }
    }

    /// A finding that carries a cross-module signature fix.
    pub fn with_sig_fix(
        &self,
        rule: &'static str,
        span: Span,
        message: String,
        help: Vec<String>,
        sig_fix: SigFix,
    ) -> Finding {
        Finding {
            rule,
            module: self.module.to_vec(),
            span,
            message,
            help,
            fix: None,
            sig_fix: Some(sig_fix),
        }
    }

    /// The source slice for `span`, or `""` for an out-of-range / non-boundary
    /// range (never panics).
    pub fn slice(&self, span: Span) -> &str {
        let lo = span.lo as usize;
        let hi = span.hi as usize;
        if lo > hi {
            return "";
        }
        self.source.get(lo..hi).unwrap_or("")
    }
}

/// Run every shipped rule over `ctx`, in registry order.
pub fn run_all(ctx: &Ctx) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(prim_param::check(ctx));
    findings.extend(adjacent_bools::check(ctx));
    findings.extend(wrapper_consistency::check(ctx));
    findings.extend(unsafe_convention::check(ctx));
    findings.extend(prefer_pipeline::check(ctx));
    findings
}

/// True when `name` appears in the module's `exposing (...)` list (or the list
/// is `exposing (..)`). Rules that reason about the API edge use this to look
/// only at exported bindings — a private helper's bare primitive is nobody's
/// business.
pub fn is_exported(ctx: &Ctx, name: Symbol) -> bool {
    use ipe_syntax::{Exposed, Exposing};
    match &ctx.ast.exposing.value {
        Exposing::All => true,
        Exposing::List(items) => items.iter().any(|item| match &item.value {
            Exposed::Value(sym) => *sym == name,
            Exposed::Type(..) => false,
        }),
    }
}

/// Flatten a curried arrow type into its parameter types and its result type.
///
/// `A -> B -> C` yields params `[A, B]` and result `C`. A non-arrow type yields
/// an empty parameter list and itself as the result.
pub fn flatten_arrow(ann: &TypeAnnotation) -> (Vec<&TypeAnnotation>, &TypeAnnotation) {
    let mut params = Vec::new();
    let mut cursor = ann;
    while let TypeAnnotation::TLambda(arg, rest) = cursor {
        params.push(arg.as_ref());
        cursor = rest.as_ref();
    }
    (params, cursor)
}

/// The head constructor name of a type annotation, unqualified (`Int`, `Bool`,
/// `Port`), or `None` when the annotation is not a bare type constructor (an
/// arrow, a tuple, a record, or a type variable).
pub fn con_head_name<'a>(ctx: &'a Ctx, ann: &TypeAnnotation) -> Option<&'a str> {
    match ann {
        TypeAnnotation::TType(_qualifier, segments, args) if args.is_empty() => {
            segments.last().map(|s| ctx.text(*s))
        }
        _ => None,
    }
}

/// Each top-level value binding that carries a type annotation, as
/// `(value, annotation)`. The common driver for the signature-shape rules.
pub fn annotated_values<'a>(
    ctx: &'a Ctx,
) -> impl Iterator<Item = (&'a Located<Value>, &'a Located<TypeAnnotation>)> {
    ctx.ast
        .values
        .iter()
        .filter_map(|value| value.value.type_annotation.as_ref().map(|ann| (value, ann)))
}
