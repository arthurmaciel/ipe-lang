//! Source spans (byte offsets) and the `Located<T>` carrier.

/// A half-open byte range `[lo, hi)` into a source file.
///
/// **Invariant:** `lo <= hi` always holds. Both constructors enforce it:
/// - [`Span::new`] normalises by clamping `hi` up to `lo` when `lo > hi`.
/// - [`Span::from_start_width`] computes `hi = lo.saturating_add(width)`,
///   which cannot produce `hi < lo`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Span {
    pub lo: u32,
    pub hi: u32,
}

impl Span {
    /// A placeholder span used for compiler-synthesised nodes that have no
    /// source location.
    pub const DUMMY: Self = Self { lo: 0, hi: 0 };

    /// Construct a span from two byte offsets, normalising so `hi >= lo`.
    ///
    /// If `hi < lo` the arguments are swapped: the result is always a valid
    /// `[lo, hi)` range. Prefer [`Span::from_start_width`] when you have a
    /// start position and a width, to avoid the implicit normalisation.
    #[must_use]
    pub const fn new(lo: u32, hi: u32) -> Self {
        let hi = if hi < lo { lo } else { hi };
        Self { lo, hi }
    }

    /// Construct a span from a start offset and a byte width.
    ///
    /// `hi` is computed as `lo.saturating_add(width)`, so the result is always
    /// a valid `[lo, hi)` range even when `lo` is near `u32::MAX`.
    #[must_use]
    pub const fn from_start_width(lo: u32, width: u32) -> Self {
        Self {
            lo,
            hi: lo.saturating_add(width),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Span;

    /// `Span::new` normalises an inverted range so `hi >= lo` always holds.
    #[test]
    fn new_normalises_inverted_span() {
        let s = Span::new(5, 2);
        assert!(
            s.hi >= s.lo,
            "Span::new must normalise lo > hi; got lo={} hi={}",
            s.lo,
            s.hi
        );
        assert_eq!(s.lo, 5, "lo is preserved");
        assert_eq!(s.hi, 5, "hi is clamped up to lo");
    }

    /// `Span::new` with `lo == hi` (zero-width, e.g. DUMMY) is valid.
    #[test]
    fn new_allows_zero_width_span() {
        let s = Span::new(7, 7);
        assert_eq!(s.lo, 7);
        assert_eq!(s.hi, 7);
    }

    /// `Span::new` with `lo < hi` (normal range) is unchanged.
    #[test]
    fn new_preserves_valid_range() {
        let s = Span::new(3, 10);
        assert_eq!(s.lo, 3);
        assert_eq!(s.hi, 10);
    }

    /// `Span::from_start_width` saturates at `u32::MAX` and never inverts.
    #[test]
    fn from_start_width_saturates_near_max() {
        let s = Span::from_start_width(u32::MAX, 3);
        assert_eq!(s.lo, u32::MAX, "lo is u32::MAX");
        assert_eq!(s.hi, u32::MAX, "hi saturates at u32::MAX, not wrap");
        assert!(s.hi >= s.lo, "span is never inverted");
    }

    /// `Span::from_start_width` with width 0 gives a zero-width span.
    #[test]
    fn from_start_width_zero_width() {
        let s = Span::from_start_width(42, 0);
        assert_eq!(s.lo, 42);
        assert_eq!(s.hi, 42);
    }

    /// `Span::from_start_width` with a normal width computes `hi = lo + width`.
    #[test]
    fn from_start_width_normal() {
        let s = Span::from_start_width(10, 5);
        assert_eq!(s.lo, 10);
        assert_eq!(s.hi, 15);
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
        Located {
            span: self.span,
            value: f(self.value),
        }
    }
}
