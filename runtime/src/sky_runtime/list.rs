//! Sky.Core.List kernel — the single home for the List runtime surface.

use super::SkyMaybe;

/// `Sky.Core.List.length` — element count (kernel-routed call sites; the pure-Sky
/// `sky_core_list_length` is the recursive stdlib form).
pub fn list_length<T>(xs: Vec<T>) -> i64 {
    xs.len() as i64
}

/// `Sky.Core.List.head : List a -> Maybe a` — the first element, or `Nothing`
/// on the empty list. Total (no indexing panic).
pub fn list_head<T>(xs: Vec<T>) -> SkyMaybe<T> {
    match xs.into_iter().next() {
        Some(x) => SkyMaybe::Just(x),
        None => SkyMaybe::Nothing,
    }
}

/// `Sky.Core.List.tail : List a -> Maybe (List a)` — everything after the first
/// element, or `Nothing` on the empty list. Total (no indexing panic); mirrors
/// the pure-Sky `tail` (`[] -> Nothing`, `(_ :: rest) -> Just rest`).
pub fn list_tail<T>(xs: Vec<T>) -> SkyMaybe<Vec<T>> {
    if xs.is_empty() {
        SkyMaybe::Nothing
    } else {
        // Drop the head; the remaining elements move into the tail vector.
        SkyMaybe::Just(xs.into_iter().skip(1).collect())
    }
}

/// `Sky.Core.List.reverse : List a -> List a` — the elements in reverse order.
/// Total; no `T: Clone` bound (the elements only MOVE).
pub fn list_reverse<T>(xs: Vec<T>) -> Vec<T> {
    let mut xs = xs;
    xs.reverse();
    xs
}

/// `Sky.Core.List.drop : Int -> List a -> List a` — drops the first `n`
/// elements. `n <= 0` keeps the whole list; `n >= len` yields `[]`. Total.
pub fn list_drop<T>(n: i64, xs: Vec<T>) -> Vec<T> {
    if n <= 0 {
        xs
    } else {
        xs.into_iter().skip(n as usize).collect()
    }
}

/// Sky `filterMap : (a -> Maybe b) -> List a -> List b`.
/// Applies `f` to each element; keeps only `Just` results.
pub fn list_filter_map<A, B>(f: impl Fn(A) -> SkyMaybe<B>, xs: Vec<A>) -> Vec<B> {
    xs.into_iter()
        .filter_map(|x| match f(x) {
            SkyMaybe::Just(v) => Some(v),
            SkyMaybe::Nothing => None,
        })
        .collect()
}

// ── Core List kernels (relocated from core.rs so the List surface has one home) ──

/// Sky `::` cons — emitted by codegen for the cons operator.
// No `T: Clone` bound — `once(x).chain(xs)` only MOVES, so cons works for
// move-only element types too (e.g. `Cmd.batch [SkyCmd, …]`; SkyCmd isn't Clone).
pub fn sky_list_cons<T>(x: T, xs: Vec<T>) -> Vec<T> {
    std::iter::once(x).chain(xs).collect()
}

pub fn list_foldl<T0, T1>(f: impl Fn(T0, T1) -> T1 + Clone, init: T1, list: Vec<T0>) -> T1 {
    let mut acc = init;
    for item in list {
        acc = f(item, acc);
    }
    acc
}
pub fn list_foldr<T0, T1>(f: impl Fn(T0, T1) -> T1 + Clone, init: T1, list: Vec<T0>) -> T1 {
    let mut acc = init;
    // `into_iter().rev()` yields OWNED items, so no clone (and no `T0: Clone`
    // bound) is needed — matching `sky_list_cons`'s move-only-friendly shape.
    for item in list.into_iter().rev() {
        acc = f(item, acc);
    }
    acc
}
// Sky `List.range` is INCLUSIVE: range 1 3 = [1, 2, 3].
pub fn list_range(lo: i64, hi: i64) -> Vec<i64> {
    if hi < lo {
        return Vec::new();
    }
    // Bound the allocation: lo/hi are caller-controlled; an absurd span (e.g.
    // 0..i64::MAX) would OOM. Cap at 10M elements (any real list is far smaller).
    // Over the cap, emit the first 10M (a correct PREFIX) plus a structured warn,
    // never a silently-wrong empty list — `[]` for `List.range 1 20000000` is a
    // wrong result for input Sky's types accept, far more surprising than a
    // truncated-with-warning span.
    const CAP: usize = 10_000_000;
    let n = (hi as i128) - (lo as i128) + 1;
    if n > CAP as i128 {
        eprintln!(
            "[sky.list] List.range: span of {n} elements exceeds the {CAP}-element \
             allocation cap; returning the first {CAP} only"
        );
        return (lo..=hi).take(CAP).collect();
    }
    (lo..=hi).collect()
}
pub fn list_indexed_map<T0, T1>(f: impl Fn(i64, T0) -> T1 + Clone, list: Vec<T0>) -> Vec<T1> {
    list.into_iter()
        .enumerate()
        .map(|(i, x)| f(i as i64, x))
        .collect()
}
pub fn list_concat_map<T0, T1>(f: impl Fn(T0) -> Vec<T1> + Clone, list: Vec<T0>) -> Vec<T1> {
    list.into_iter().flat_map(f).collect()
}
pub fn list_zip<T0, T1>(a: Vec<T0>, b: Vec<T1>) -> Vec<(T0, T1)> {
    a.into_iter().zip(b).collect()
}
pub fn list_filter<T0: Clone>(f: impl Fn(T0) -> bool + Clone, list: Vec<T0>) -> Vec<T0> {
    list.into_iter().filter(|x| f(x.clone())).collect()
}
pub fn list_member<T0: PartialEq>(x: T0, list: Vec<T0>) -> bool {
    list.contains(&x)
}
pub fn list_any<T0>(f: impl Fn(T0) -> bool + Clone, list: Vec<T0>) -> bool {
    list.into_iter().any(f)
}
pub fn list_all<T0>(f: impl Fn(T0) -> bool + Clone, list: Vec<T0>) -> bool {
    list.into_iter().all(f)
}

// ── Sorting (mirrors Go's List_sort / List_sortBy; sortWith added for Rust) ──
//
// All three are STABLE (Rust's `Vec::sort_by` / `sort_by_key` are stable, matching
// Go's `sort.SliceStable`). None can panic on well-typed input: ordering is total
// (`total_cmp` via `cmp_total`), so a NaN key never trips the `Ord` contract the
// way a naive `partial_cmp().unwrap()` would.

/// Best-effort total ordering for any `PartialOrd` element. `partial_cmp` returns
/// `None` only for incomparable values (floating-point NaN); we map that to
/// `Equal`. NOTE: with MORE THAN ONE NaN present this is NOT transitive (NaN≈1.0
/// and NaN≈2.0 yet 1.0<2.0), and since Rust 1.81 `slice::sort_by` PANICS on a
/// comparator that violates a strict weak ordering. NaN IS reachable at runtime
/// (`0.0 / 0.0`, `sqrt(-1)`, an FFI float) even though no Sky literal spells it —
/// so the callers below wrap the sort in `catch_unwind` to stay total.
fn cmp_total<T: PartialOrd>(a: &T, b: &T) -> std::cmp::Ordering {
    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
}

/// Run `sort_by` but never panic: a non-strict-weak-ordering comparator (e.g.
/// `cmp_total` over a multi-NaN float list) panics std's sort since Rust 1.81;
/// catch it and leave the slice in its safe, element-complete (unspecified-order)
/// state. Shared by `list_sort`/`list_sort_by` (and mirrors `list_sort_with`).
fn sort_by_total<T, F: Fn(&T, &T) -> std::cmp::Ordering>(result: &mut [T], cmp: F) {
    let order = std::panic::AssertUnwindSafe(|| result.sort_by(&cmp));
    if std::panic::catch_unwind(order).is_err() {
        eprintln!(
            "[sky.list] sort: comparator is not a consistent total order (NaN?); unspecified order"
        );
    }
}

/// `Sky.Core.List.sort : List comparable -> List comparable` — stable ascending
/// sort by the element's natural order. Total (no panic on NaN).
pub fn list_sort<T: PartialOrd>(list: Vec<T>) -> Vec<T> {
    let mut result = list;
    sort_by_total(&mut result, cmp_total);
    result
}

/// `Sky.Core.List.sortBy : (a -> comparable) -> List a -> List a` — stable sort by
/// the `keyFn elem` projection. Decorate-sort-undecorate: `keyFn` is applied
/// exactly once per element (no repeated key recomputation during comparison).
/// `A: Clone` because the Sky closure ABI takes its element by value — same bound
/// `list_filter` already carries for its predicate.
pub fn list_sort_by<A: Clone, B: PartialOrd>(key_fn: impl Fn(A) -> B, list: Vec<A>) -> Vec<A> {
    // Decorate: compute each key once, pairing it with its element. The key fn
    // consumes its argument (owned ABI), so clone the element for the key call
    // and keep the original to emit after the sort.
    let mut decorated: Vec<(B, A)> = list.into_iter().map(|x| (key_fn(x.clone()), x)).collect();
    // Stable sort on the key only (so equal keys preserve input order). Via the
    // panic-safe wrapper: a multi-NaN key set makes cmp_total non-transitive.
    sort_by_total(&mut decorated, |a, b| cmp_total(&a.0, &b.0));
    // Undecorate.
    decorated.into_iter().map(|(_, x)| x).collect()
}

/// `Sky.Core.List.sortWith : (a -> a -> Int) -> List a -> List a` — stable sort by
/// a user comparator returning a Sky `Int` (negative → first arg orders before the
/// second, zero → equal, positive → after; matching `Basics.compare`'s -1/0/+1).
/// The comparator takes its two elements by value (the Sky closure ABI), so `a`
/// must be `Clone`. The `Int` → `Ordering` map is total — every `i64` lands in
/// exactly one of Less / Equal / Greater.
pub fn list_sort_with<A: Clone>(cmp: impl Fn(A, A) -> i64, list: Vec<A>) -> Vec<A> {
    let mut result = list;
    // Soundness (no-panic thesis): the comparator is arbitrary user Sky code.
    // Since Rust 1.81 the standard sort PANICS when a comparator violates a
    // strict weak ordering (e.g. `cmp a b` and `cmp b a` both return a positive
    // Int). A well-typed Sky `List.sortWith` could supply exactly that, so the
    // bare `sort_by` is a Sky-reachable panic. Catch the unwind and return the
    // list in its (safe, unspecified-order) post-sort state — std guarantees the
    // elements are all still present and no UB on a panicking comparator.
    let order = &mut result;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        order.sort_by(|a, b| cmp(a.clone(), b.clone()).cmp(&0));
    }));
    if outcome.is_err() {
        eprintln!(
            "[sky.list] List.sortWith: comparator is not a consistent total order; \
             returning input in unspecified order"
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // SOUNDNESS regression (no-panic thesis): a comparator that is NOT a strict
    // weak ordering makes std's sort panic since Rust 1.81. A well-typed Sky
    // `List.sortWith` can supply one, so the kernel must NOT panic — it returns
    // the elements in unspecified (but safe, complete) order instead.
    // SOUNDNESS regression: a multi-NaN float list makes cmp_total non-transitive,
    // which panics std sort since Rust 1.81. list_sort / list_sort_by must stay total.
    #[test]
    fn sort_multi_nan_does_not_panic() {
        let nan = f64::NAN;
        let xs: Vec<f64> = vec![3.0, nan, 1.0, nan, 2.0, nan, 0.5];
        let out = list_sort(xs.clone());
        assert_eq!(out.len(), xs.len(), "no elements lost");
        let keyed = list_sort_by(|x: f64| x, xs.clone());
        assert_eq!(keyed.len(), xs.len());
    }

    #[test]
    fn sort_with_inconsistent_comparator_does_not_panic() {
        let xs: Vec<i64> = (0..64).collect();
        // Always-greater: cmp a b = 1 AND cmp b a = 1 — violates antisymmetry.
        let out = list_sort_with(|_a, _b| 1, xs.clone());
        // No panic; every element preserved (multiset equal).
        let mut got = out.clone();
        got.sort();
        assert_eq!(got, (0..64).collect::<Vec<i64>>());
    }

    #[test]
    fn test_filter_map_doubles_evens() {
        let xs: Vec<i64> = vec![1, 2, 3, 4];
        let result = list_filter_map(
            |x| {
                if x % 2 == 0 {
                    SkyMaybe::Just(x * 2)
                } else {
                    SkyMaybe::Nothing
                }
            },
            xs,
        );
        assert_eq!(result, vec![4i64, 8]);
    }

    #[test]
    fn test_filter_map_all_nothing() {
        let xs: Vec<i64> = vec![1, 2, 3];
        let result = list_filter_map(|_: i64| SkyMaybe::<i64>::Nothing, xs);
        assert!(result.is_empty());
    }

    #[test]
    fn test_filter_map_all_just() {
        let xs: Vec<i64> = vec![1, 2, 3];
        let result = list_filter_map(|x| SkyMaybe::Just(x + 10), xs);
        assert_eq!(result, vec![11i64, 12, 13]);
    }

    #[test]
    fn test_filter_map_empty() {
        let xs: Vec<i64> = vec![];
        let result = list_filter_map(SkyMaybe::Just, xs);
        assert!(result.is_empty());
    }

    #[test]
    fn test_sort_ints() {
        assert_eq!(list_sort(vec![3i64, 1, 2]), vec![1i64, 2, 3]);
        assert_eq!(list_sort(Vec::<i64>::new()), Vec::<i64>::new());
    }

    #[test]
    fn test_sort_strings() {
        assert_eq!(
            list_sort(vec!["banana".to_string(), "apple".into(), "cherry".into()]),
            vec!["apple".to_string(), "banana".into(), "cherry".into()]
        );
    }

    #[test]
    fn test_sort_floats_with_nan_no_panic() {
        // NaN must not panic the comparator (total order falls back to Equal).
        let r = list_sort(vec![3.0f64, f64::NAN, 1.0]);
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn test_sort_by_key_applied_once_and_stable() {
        // sortBy String.length — stable: equal-length keep input order.
        let r = list_sort_by(
            |s: String| s.len() as i64,
            vec!["ccc".to_string(), "a".into(), "bb".into(), "dd".into()],
        );
        assert_eq!(
            r,
            vec!["a".to_string(), "bb".into(), "dd".into(), "ccc".into()]
        );
    }

    #[test]
    fn test_sort_with_reverse() {
        // Comparator b - a → descending.
        let r = list_sort_with(|a: i64, b: i64| b - a, vec![1i64, 3, 2]);
        assert_eq!(r, vec![3i64, 2, 1]);
    }

    #[test]
    fn test_sort_with_stable_on_equal() {
        // All-equal comparator preserves input order (stable).
        let r = list_sort_with(|_a: i64, _b: i64| 0, vec![3i64, 1, 2]);
        assert_eq!(r, vec![3i64, 1, 2]);
    }
}
