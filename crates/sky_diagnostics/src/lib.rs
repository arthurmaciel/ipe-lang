#![forbid(unsafe_code)]
//! Shared diagnostic vocabulary for the Sky compiler: source spans, the
//! `Located<T>` carrier, and the typed `Diagnostic` enum. Every stage speaks
//! these types — there are no `String` errors across stage boundaries.

mod diagnostic;
mod span;

pub use diagnostic::{Diagnostic, DResult, NameError, ParseError, TypeError};
pub use span::{Located, Span};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_bug_carries_context() {
        let d = Diagnostic::CompilerBug { where_: "lower", detail: "no type for region".into() };
        assert!(matches!(d, Diagnostic::CompilerBug { where_: "lower", .. }));
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
        let a = Diagnostic::Parse { span: Span::DUMMY, msg: ParseError::Unexpected };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
