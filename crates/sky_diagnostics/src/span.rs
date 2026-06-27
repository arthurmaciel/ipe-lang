//! Source spans (byte offsets) and the `Located<T>` carrier.

/// A half-open byte range `[lo, hi)` into a source file.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Span {
    pub lo: u32,
    pub hi: u32,
}

impl Span {
    /// A placeholder span used for compiler-synthesised nodes that have no
    /// source location.
    pub const DUMMY: Self = Self { lo: 0, hi: 0 };

    #[must_use]
    pub const fn new(lo: u32, hi: u32) -> Self {
        Self { lo, hi }
    }
}

/// A value tagged with the source span it originated from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Located<T> {
    pub span: Span,
    pub value: T,
}

impl<T> Located<T> {
    #[must_use]
    pub const fn new(span: Span, value: T) -> Self {
        Self { span, value }
    }

    /// Transform the carried value while preserving the span.
    #[must_use]
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Located<U> {
        Located { span: self.span, value: f(self.value) }
    }
}
