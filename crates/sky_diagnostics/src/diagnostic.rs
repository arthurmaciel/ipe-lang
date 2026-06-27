//! Typed compiler diagnostics. No stage ever returns a `String` error; every
//! failure is one of these enums. `CompilerBug.detail` is the only free-form
//! `String`, reserved for "this should never happen" contract violations.

use crate::span::Span;

/// Errors raised during lexing / parsing. Variants grow per task.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ParseError {
    Unexpected,
}

/// Errors raised during name resolution / canonicalisation. Variants grow per
/// task.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NameError {
    Unknown,
}

/// Errors raised during type inference / checking. Variants grow per task.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TypeError {
    Mismatch,
}

/// The single typed error currency of the compiler.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Diagnostic {
    Parse { span: Span, msg: ParseError },
    Name { span: Span, msg: NameError },
    Type { span: Span, msg: TypeError },
    /// A violated internal invariant — illegal IR, missing region type, etc.
    /// `where_` names the stage; `detail` is the only free-form message.
    CompilerBug { where_: &'static str, detail: String },
}

/// Result alias used throughout the compiler.
pub type DResult<T> = Result<T, Diagnostic>;
