#![forbid(unsafe_code)]
//! Shared diagnostic vocabulary for the Ipê compiler: source spans, the
//! `Located<T>` carrier, and the typed `Diagnostic` enum. Every stage speaks
//! these types — there are no `String` errors across stage boundaries.

mod code;
mod diagnostic;
pub mod path_check;
mod render;
mod span;

pub use code::{
    ALL_CODES, Code, IPE_F4400, IPE_F4401, IPE_F4402, IPE_F4410, IPE_F4411, IPE_F4412, IPE_F4413,
    IPE_F4414, IPE_I0001, IPE_I0010, IPE_I0011, IPE_I0100, IPE_I0101, IPE_I0102, IPE_I0103,
    IPE_I0200, IPE_I0201, IPE_I0202, IPE_I0203, IPE_L0100, IPE_L0101, IPE_L0102, IPE_L0103,
    IPE_L0104, IPE_L0105, IPE_L0106, IPE_L0107, IPE_L0108, IPE_L0110, IPE_L0111, IPE_L0112,
    IPE_L0113, IPE_L0114, IPE_L0115, IPE_L0116, IPE_L0117, IPE_L0118, IPE_L0119, IPE_L0120,
    IPE_L0121, IPE_L0122, IPE_L0123, IPE_L0124, IPE_L0125, IPE_L0126, IPE_L0127, IPE_L0128,
    IPE_L0129, IPE_L0130, IPE_L0131, IPE_L0132, IPE_L0133, IPE_L0134, IPE_L0140, IPE_L0200,
    IPE_N0001, IPE_N0002, IPE_N0003, IPE_N0004, IPE_N0005, IPE_N0010, IPE_N0011, IPE_N0012,
    IPE_N0013, IPE_N0020, IPE_N0021, IPE_N0022, IPE_N0023, IPE_N0024, IPE_N0025, IPE_N0026,
    IPE_N0027, IPE_N0028, IPE_N0029, IPE_N0030, IPE_N0031, IPE_N0032, IPE_N0033, IPE_N0034,
    IPE_N0035, IPE_N0038, IPE_P0001, IPE_P0002, IPE_P0003, IPE_P0010, IPE_P0011, IPE_P0012,
    IPE_P0013, IPE_P0014, IPE_P0015, IPE_P0016, IPE_P0017, IPE_P0018, IPE_P0020, IPE_P0021,
    IPE_P0030, IPE_P0031, IPE_P0040, IPE_P0041, IPE_P0050, IPE_P0060, IPE_P0061, IPE_P0062,
    IPE_P0063, IPE_T0001, IPE_T0002, IPE_T0003, IPE_T0004, IPE_T0010, IPE_T0011, IPE_T0012,
    IPE_T0013, IPE_T0014, IPE_T0015, IPE_T0016, IPE_T0017, IPE_T0019, IPE_T0020, ISSUE_TRACKER_URL,
    Severity, explain_page, title,
};
pub use diagnostic::{
    AliasExpansionKind, AppShape, Applicability, CaseDefect, CmdSubShapeMismatch, Construct,
    DResult, Diagnostic, Expected, ExpectedSet, ExposingDefect, Feature, HOF_KERNEL_RESULT_CLASS,
    HeaderDefect, HelpLine, Hint, IfDefect, LetDefect, LowerError, ModelLeaf, NameError,
    ParseError, SealRejection, SpanRole, Suggestion, TokenKind, TyDoc, TypeDeclDefect, TypeError,
};
pub use render::{plain_message, render, render_ty};
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
        // Additive guarantee: every coarse variant remains buildable.
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
        assert_eq!(d.code(), IPE_P0010);
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
        assert_eq!(coarse.code(), IPE_T0001);
        assert_eq!(rich.code(), IPE_T0001);
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
        assert_eq!(d.code(), IPE_T0011);
    }

    #[test]
    fn lower_channel_is_distinct_from_bug() {
        let d = Diagnostic::Lower {
            span: Span::new(4, 9),
            msg: LowerError::Unsupported(Feature::BinOps),
        };
        assert_eq!(d.code(), IPE_L0101);
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
        assert_eq!(generic.code(), IPE_I0001);
        assert_eq!(generic.severity(), Severity::Bug);
        assert_eq!(generic.primary_span(), Span::DUMMY);

        let specific = Diagnostic::CompilerBug {
            where_: "intern.resolve",
            detail: "y".into(),
        };
        assert_eq!(specific.code(), IPE_I0010);
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
        assert_eq!(d.code(), IPE_N0010);
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
        assert_eq!(d.code(), IPE_T0010);
        assert_eq!(
            d.help(),
            vec![
                HelpLine::MissingConstructor("Green".into()),
                HelpLine::MissingConstructor("Blue".into()),
            ]
        );
    }
}
