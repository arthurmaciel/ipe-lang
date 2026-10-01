//! Views that recover a surface form from the node the parser desugars it to.
//!
//! The parser lowers three sugars to ordinary nodes: a field accessor `.a.b`
//! to a getter lambda, a negation `-e` to `Basics.negate e`, and a local
//! function `let f x = body` to `let f = \x -> body`. A printer that must give
//! the source form back asks these views. Each recognises its desugar by a span
//! shape no written source has, so a hand-written equivalent (a lambda named
//! like the getter, a written `Basics.negate`) is never mistaken for the sugar.

use ipe_intern::{Interner, Symbol};
use ipe_syntax::{Expr, Expr_, Pattern, Pattern_};

/// The field path `[a, b]` of a first-class accessor `.a.b`, or `None`.
///
/// The accessor desugars to `\p -> p.a.b`, where the binder `p` and its one
/// use both carry the span of the `.`. Two written tokens never share a span,
/// so a written lambda never matches.
#[must_use]
pub fn field_accessor(expr: &Expr) -> Option<Vec<Symbol>> {
    let Expr_::Lambda(params, body) = &expr.value else {
        return None;
    };
    let [param] = params.as_slice() else {
        return None;
    };
    let Pattern_::PVar(binder) = &param.value else {
        return None;
    };
    let mut fields = Vec::new();
    let mut cur = body.as_ref();
    while let Expr_::Access(base, field) = &cur.value {
        fields.push(field.value);
        cur = base.as_ref();
    }
    let Expr_::VarLocal(var) = &cur.value else {
        return None;
    };
    if var != binder || cur.span != param.span || fields.is_empty() {
        return None;
    }
    fields.reverse();
    Some(fields)
}

/// The operand `e` of a negation `-e` of a non-literal, or `None`.
///
/// The negation desugars to `Basics.negate e` whose callee carries the
/// one-byte span of the `-`, flush against the operand. A written qualified
/// name spans at least three bytes, so a written `Basics.negate e` never
/// matches.
#[must_use]
pub fn negation<'e>(expr: &'e Expr, interner: &Interner) -> Option<&'e Expr> {
    let Expr_::Call(callee, args) = &expr.value else {
        return None;
    };
    let [operand] = args.as_slice() else {
        return None;
    };
    let Expr_::VarQual(module, name) = &callee.value else {
        return None;
    };
    let minus_sign =
        callee.span.hi.checked_sub(callee.span.lo) == Some(1) && callee.span.hi == operand.span.lo;
    let negate =
        interner.resolve(*module) == Some("Basics") && interner.resolve(*name) == Some("negate");
    (minus_sign && negate).then_some(operand)
}

/// The parameters and body of a local function `let f x y = body`, or `None`.
///
/// `body` is the value a `let` binding holds. The function sugar desugars to a
/// lambda whose span starts at its first parameter; a written lambda starts at
/// its `\` (or at an enclosing `(`), before that parameter. An accessor's
/// getter lambda also starts at its binder, so it is excluded first.
#[must_use]
pub fn let_function(body: &Expr) -> Option<(&[Pattern], &Expr)> {
    if field_accessor(body).is_some() {
        return None;
    }
    let Expr_::Lambda(params, inner) = &body.value else {
        return None;
    };
    let first = params.first()?;
    (body.span.lo == first.span.lo).then_some((params.as_slice(), inner.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::{field_accessor, let_function, negation};
    use crate::parse_module;
    use ipe_intern::Interner;
    use ipe_syntax::{Expr, Expr_, Module};

    /// Parse `module M exposing (x)` with `x =` followed by `body`.
    fn parse_body(body: &str) -> (Module, Interner) {
        let mut interner = Interner::new();
        let src = format!("module M exposing (x)\n\n\nx =\n    {body}\n");
        let module = parse_module(&src, &mut interner).expect("fixture parses");
        (module, interner)
    }

    fn value_body(m: &Module) -> Option<&Expr> {
        m.values.first().map(|v| &v.value.body)
    }

    /// The first `let` binding's value.
    fn let_value(m: &Module) -> Option<&Expr> {
        let Expr_::Let(bindings, _) = &value_body(m)?.value else {
            return None;
        };
        bindings.first().map(|b| &b.body)
    }

    fn names(fields: &[ipe_intern::Symbol], i: &Interner) -> Vec<String> {
        fields
            .iter()
            .map(|s| i.resolve(*s).unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn an_accessor_is_recognised_with_its_path() {
        let (m, i) = parse_body(".a.b");
        let path = value_body(&m)
            .and_then(field_accessor)
            .map(|fs| names(&fs, &i));
        assert_eq!(path, Some(vec!["a".to_owned(), "b".to_owned()]));
    }

    /// A written getter lambda, even one named like the desugar, stays a lambda.
    #[test]
    fn a_written_getter_lambda_is_not_an_accessor() {
        let (m, _) = parse_body("\\ipe_accessor_arg -> ipe_accessor_arg.a");
        assert_eq!(value_body(&m).and_then(field_accessor), None);
    }

    #[test]
    fn a_negation_is_recognised() {
        let (m, i) = parse_body("-y");
        let operand = value_body(&m).and_then(|e| negation(e, &i));
        assert!(matches!(
            operand.map(|e| &e.value),
            Some(Expr_::VarLocal(_))
        ));
    }

    /// A written `Basics.negate y` stays a call.
    #[test]
    fn a_written_negate_call_is_not_a_negation() {
        let (m, i) = parse_body("Basics.negate y");
        assert!(value_body(&m).and_then(|e| negation(e, &i)).is_none());
    }

    #[test]
    fn a_local_function_is_recognised() {
        let (m, _) = parse_body("let\n        f z =\n            z\n    in\n    f");
        let params = let_value(&m).and_then(let_function).map(|(ps, _)| ps.len());
        assert_eq!(params, Some(1));
    }

    /// A binding to a written lambda, bare or parenthesised, stays a lambda.
    #[test]
    fn a_binding_to_a_written_lambda_is_not_a_local_function() {
        for lambda in ["\\z -> z", "(\\z -> z)"] {
            let (m, _) = parse_body(&format!(
                "let\n        f =\n            {lambda}\n    in\n    f"
            ));
            assert!(let_value(&m).and_then(let_function).is_none(), "{lambda}");
        }
    }

    /// A binding to an accessor is the accessor, not a one-parameter function.
    #[test]
    fn a_binding_to_an_accessor_is_not_a_local_function() {
        let (m, _) = parse_body("let\n        f =\n            .a\n    in\n    f");
        let value = let_value(&m);
        assert!(value.and_then(let_function).is_none());
        assert!(value.and_then(field_accessor).is_some());
    }
}
