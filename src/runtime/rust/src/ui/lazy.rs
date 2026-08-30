//! `Ipe.Ui.Lazy` kernel helpers — deferred subtree evaluation.
//!
//! **Ipê v1 semantics: EAGER.** Ipê's Go runtime memoises the subtree using an
//! LRU keyed on the function pointer and argument equality; Ipê does not yet
//! have a TEA-integrated memoisation layer. In v1 these functions evaluate
//! immediately, which is semantically equivalent (same output, just no caching
//! benefit). Sanctioned divergence §B-Lazy.
//!
//! Every function carries a trailing underscore per the `naming.rs` convention.

use super::element::Element;

/// `Lazy.lazy : (a -> Element msg) -> a -> Element msg`
///
/// Applies `f` to `a` eagerly.  In Ipe's Go backend this memoises `f(a)`;
/// Ipê v1 evaluates immediately without caching.
pub fn lazy_lazy_<M, A>(f: impl Fn(A) -> Element<M>, a: A) -> Element<M> {
    f(a)
}

/// `Lazy.lazy2 : (a -> b -> Element msg) -> a -> b -> Element msg` (eager)
pub fn lazy_lazy2_<M, A, B>(f: impl Fn(A, B) -> Element<M>, a: A, b: B) -> Element<M> {
    f(a, b)
}

/// `Lazy.lazy3 : (a -> b -> c -> Element msg) -> a -> b -> c -> Element msg` (eager)
pub fn lazy_lazy3_<M, A, B, C>(f: impl Fn(A, B, C) -> Element<M>, a: A, b: B, c: C) -> Element<M> {
    f(a, b, c)
}

/// `Lazy.lazy4 : (a -> b -> c -> d -> Element msg) -> a -> b -> c -> d -> Element msg` (eager)
pub fn lazy_lazy4_<M, A, B, C, D>(
    f: impl Fn(A, B, C, D) -> Element<M>,
    a: A,
    b: B,
    c: C,
    d: D,
) -> Element<M> {
    f(a, b, c, d)
}

/// `Lazy.lazy5 : (a -> b -> c -> d -> e -> Element msg) -> ... -> Element msg` (eager)
pub fn lazy_lazy5_<M, A, B, C, D, E>(
    f: impl Fn(A, B, C, D, E) -> Element<M>,
    a: A,
    b: B,
    c: C,
    d: D,
    e: E,
) -> Element<M> {
    f(a, b, c, d, e)
}
