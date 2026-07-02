#![forbid(unsafe_code)]
//! Shared diagnostic vocabulary for the Sky compiler: source spans, the
//! `Located<T>` carrier, and the typed `Diagnostic` enum. Every stage speaks
//! these types — there are no `String` errors across stage boundaries.

mod code;
mod diagnostic;
mod render;
mod span;

pub use code::{
    Code, ISSUE_TRACKER_URL, SKY_I0001, SKY_I0010, SKY_I0011, SKY_I0100, SKY_I0101, SKY_I0102,
    SKY_I0103, SKY_I0200, SKY_I0201, SKY_I0202, SKY_I0203, SKY_L0100, SKY_L0101, SKY_L0102,
    SKY_L0103, SKY_L0104, SKY_L0105, SKY_L0106, SKY_L0107, SKY_L0108, SKY_L0110, SKY_L0111,
    SKY_L0112, SKY_L0113, SKY_L0114, SKY_L0115, SKY_L0116, SKY_L0117, SKY_L0118, SKY_L0200,
    SKY_N0001, SKY_N0002, SKY_N0003, SKY_N0004, SKY_N0005, SKY_N0010, SKY_N0011, SKY_N0012,
    SKY_N0013, SKY_N0020, SKY_N0021, SKY_N0022, SKY_N0023, SKY_N0024, SKY_N0025, SKY_P0001,
    SKY_P0002, SKY_P0003, SKY_P0010, SKY_P0011, SKY_P0012, SKY_P0013, SKY_P0014, SKY_P0015,
    SKY_P0016, SKY_P0017, SKY_P0020, SKY_P0021, SKY_P0030, SKY_P0031, SKY_P0040, SKY_P0041,
    SKY_P0050, SKY_P0060, SKY_P0061, SKY_P0062, SKY_T0001, SKY_T0002, SKY_T0003, SKY_T0004,
    SKY_T0010, SKY_T0011, SKY_T0012, SKY_T0013, SKY_T0014, Severity, explain_page, title,
};
pub use diagnostic::{
    Applicability, CaseDefect, Construct, DResult, Diagnostic, Expected, ExpectedSet,
    ExposingDefect, Feature, HeaderDefect, HelpLine, Hint, IfDefect, LetDefect, LowerError,
    NameError, ParseError, SpanRole, Suggestion, TokenKind, TyDoc, TypeDeclDefect, TypeError,
};
pub use render::render;
pub use span::{Located, Span};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_bug_carries_context() {
        let d = Diagnostic::CompilerBug {
            where_: "lower",
            detail: "no type for region".into(),
        };
        assert!(matches!(
            d,
            Diagnostic::CompilerBug {
                where_: "lower",
                ..
            }
        ));
    }

    #[test]
    fn span_dummy_is_empty() {
        assert_eq!(Span::DUMMY, Span::new(0, 0));
    }

    #[test]
    fn located_map_preserves_span() {
        let l = Located::new(Span::new(3, 7), 1i32);
        let m = l.map(|v| v + 1);
        assert_eq!(m.span, Span::new(3, 7));
        assert_eq!(m.value, 2);
    }

    #[test]
    fn diagnostic_is_clone_eq() {
        let a = Diagnostic::Parse {
            span: Span::DUMMY,
            msg: ParseError::Unexpected,
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn coarse_variants_still_construct() {
        // Additive guarantee: every Milestone-0 variant remains buildable.
        let _ = Diagnostic::Parse {
            span: Span::DUMMY,
            msg: ParseError::TooDeep,
        };
        let _ = Diagnostic::Name {
            span: Span::DUMMY,
            msg: NameError::Unknown,
        };
        let _ = Diagnostic::Type {
            span: Span::DUMMY,
            msg: TypeError::Mismatch,
        };
        let _ = Diagnostic::Type {
            span: Span::DUMMY,
            msg: TypeError::BudgetExceeded,
        };
    }

    #[test]
    fn code_maps_payload_variants() {
        let d = Diagnostic::Parse {
            span: Span::new(1, 2),
            msg: ParseError::UnknownChar('@'),
        };
        assert_eq!(d.code(), SKY_P0010);
        assert_eq!(d.severity(), Severity::Error);
        assert_eq!(d.primary_span(), Span::new(1, 2));
    }

    #[test]
    fn coarse_and_payload_share_a_code() {
        let coarse = Diagnostic::Type {
            span: Span::DUMMY,
            msg: TypeError::Mismatch,
        };
        let rich = Diagnostic::Type {
            span: Span::DUMMY,
            msg: TypeError::TypeMismatch {
                expected: Box::new(TyDoc::Unit),
                found: Box::new(TyDoc::Var("a".into())),
                definition: None,
                path: Box::new([]),
            },
        };
        assert_eq!(coarse.code(), SKY_T0001);
        assert_eq!(rich.code(), SKY_T0001);
    }

    #[test]
    fn redundant_branch_is_a_warning() {
        let d = Diagnostic::Type {
            span: Span::DUMMY,
            msg: TypeError::RedundantCaseBranch {
                constructor: "Red".into(),
            },
        };
        assert_eq!(d.severity(), Severity::Warning);
        assert_eq!(d.code(), SKY_T0011);
    }

    #[test]
    fn lower_channel_is_distinct_from_bug() {
        let d = Diagnostic::Lower {
            span: Span::new(4, 9),
            msg: LowerError::Unsupported(Feature::BinOps),
        };
        assert_eq!(d.code(), SKY_L0101);
        assert_eq!(d.severity(), Severity::Error);
        assert_eq!(
            d.help(),
            vec![HelpLine::Hint(Hint::FeatureNotSupported(Feature::BinOps))]
        );
    }

    #[test]
    fn compiler_bug_maps_where_to_internal_code() {
        let generic = Diagnostic::CompilerBug {
            where_: "lower",
            detail: "x".into(),
        };
        assert_eq!(generic.code(), SKY_I0001);
        assert_eq!(generic.severity(), Severity::Bug);
        assert_eq!(generic.primary_span(), Span::DUMMY);

        let specific = Diagnostic::CompilerBug {
            where_: "intern.resolve",
            detail: "y".into(),
        };
        assert_eq!(specific.code(), SKY_I0010);
    }

    #[test]
    fn duplicate_value_points_at_first_definition() {
        let d = Diagnostic::Name {
            span: Span::new(20, 24),
            msg: NameError::DuplicateValue {
                name: "foo".into(),
                first: Span::new(2, 5),
            },
        };
        assert_eq!(d.code(), SKY_N0010);
        assert_eq!(
            d.help(),
            vec![HelpLine::SecondarySpan {
                span: Span::new(2, 5),
                role: SpanRole::FirstDefinition
            }]
        );
    }

    #[test]
    fn did_you_mean_preserves_producer_order() {
        let d = Diagnostic::Name {
            span: Span::DUMMY,
            msg: NameError::ValueNotFound {
                name: "lenght".into(),
                suggestions: Box::new(["length".into(), "list".into()]),
            },
        };
        assert_eq!(
            d.help(),
            vec![
                HelpLine::DidYouMean("length".into()),
                HelpLine::DidYouMean("list".into()),
            ]
        );
    }

    #[test]
    fn single_candidate_becomes_machine_applicable_suggestion() {
        let d = Diagnostic::Name {
            span: Span::new(0, 6),
            msg: NameError::ValueNotFound {
                name: "lenght".into(),
                suggestions: Box::new(["length".into()]),
            },
        };
        assert_eq!(
            d.help(),
            vec![HelpLine::Suggest(Suggestion {
                span: Span::new(0, 6),
                replacement: "length".into(),
                applicability: Applicability::MachineApplicable,
            })]
        );
    }

    #[test]
    fn non_exhaustive_lists_missing_constructors() {
        let d = Diagnostic::Type {
            span: Span::DUMMY,
            msg: TypeError::NonExhaustiveCase {
                missing: Box::new(["Green".into(), "Blue".into()]),
            },
        };
        assert_eq!(d.code(), SKY_T0010);
        assert_eq!(
            d.help(),
            vec![
                HelpLine::MissingConstructor("Green".into()),
                HelpLine::MissingConstructor("Blue".into()),
            ]
        );
    }
}
