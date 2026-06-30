//! `Sky.Core.Set` kernels backed by `std::collections::BTreeSet<A>`.
//!
//! Sky's `Set` is keyed on `comparable` values (Int, String, …), all of which
//! are `Ord` in Rust — so `BTreeSet<A>` is the natural backing. Go's runtime
//! Set is a `map[string]any` (unordered iteration), so Sky guarantees no
//! particular Set order; `BTreeSet`'s sorted iteration is a CONFORMING and
//! strictly MORE deterministic choice (same rationale as `Dict.keys` returning
//! sorted keys on the Rust backend). Every op consumes its set(s) by value and
//! returns the modified copy (functional update) — no `Clone` bound is needed
//! on the element type for any kernel; a Sky-level reuse of a Set value is
//! `.clone()`d at the use site by codegen, exactly like `HashMap`/`Vec`.
//!
//! Codegen: TypeRenderer renders `Set a` as `BTreeSet<a>`; the empty-set
//! turbofish (`EKSet`) pins `A` from the expected type, mirroring `dict_empty`.

use std::collections::BTreeSet;

/// `Set.empty : Set a`.
pub fn set_empty<A>() -> BTreeSet<A> {
    BTreeSet::new()
}

/// `Set.fromList : List a -> Set a`. Duplicates collapse.
pub fn set_from_list<A: Ord>(xs: Vec<A>) -> BTreeSet<A> {
    xs.into_iter().collect()
}

/// `Set.insert : a -> Set a -> Set a`. Functional update.
pub fn set_insert<A: Ord>(v: A, s: BTreeSet<A>) -> BTreeSet<A> {
    let mut s = s;
    s.insert(v);
    s
}

/// `Set.remove : a -> Set a -> Set a`. Absent element → unchanged.
pub fn set_remove<A: Ord>(v: A, s: BTreeSet<A>) -> BTreeSet<A> {
    let mut s = s;
    s.remove(&v);
    s
}

/// `Set.member : a -> Set a -> Bool`.
pub fn set_member<A: Ord>(v: A, s: BTreeSet<A>) -> bool {
    s.contains(&v)
}

/// `Set.toList : Set a -> List a`. Sorted (BTreeSet iterates in order).
pub fn set_to_list<A>(s: BTreeSet<A>) -> Vec<A> {
    s.into_iter().collect()
}

/// `Set.size : Set a -> Int`.
pub fn set_size<A>(s: BTreeSet<A>) -> i64 {
    s.len() as i64
}

/// `Set.union : Set a -> Set a -> Set a`. Every element of either set.
pub fn set_union<A: Ord>(a: BTreeSet<A>, b: BTreeSet<A>) -> BTreeSet<A> {
    let mut a = a;
    a.extend(b);
    a
}

/// `Set.intersect : Set a -> Set a -> Set a`. Elements in BOTH sets.
pub fn set_intersect<A: Ord>(a: BTreeSet<A>, b: BTreeSet<A>) -> BTreeSet<A> {
    a.into_iter().filter(|x| b.contains(x)).collect()
}

/// `Set.diff : Set a -> Set a -> Set a`. Elements in `a` but NOT in `b`.
pub fn set_diff<A: Ord>(a: BTreeSet<A>, b: BTreeSet<A>) -> BTreeSet<A> {
    a.into_iter().filter(|x| !b.contains(x)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_list_dedups_and_sorts() {
        let s = set_from_list(vec![3, 1, 2, 1, 3]);
        assert_eq!(set_to_list(s), vec![1, 2, 3]);
    }

    #[test]
    fn insert_remove_member_size() {
        let s = set_insert(2, set_insert(1, set_empty::<i64>()));
        assert!(set_member(1, s.clone()));
        assert!(!set_member(9, s.clone()));
        assert_eq!(set_size(s.clone()), 2);
        let s = set_remove(1, s);
        assert_eq!(set_to_list(s), vec![2]);
    }

    #[test]
    fn union_intersect_diff() {
        let a = set_from_list(vec![1, 2, 3]);
        let b = set_from_list(vec![2, 3, 4]);
        assert_eq!(
            set_to_list(set_union(a.clone(), b.clone())),
            vec![1, 2, 3, 4]
        );
        assert_eq!(set_to_list(set_intersect(a.clone(), b.clone())), vec![2, 3]);
        assert_eq!(set_to_list(set_diff(a, b)), vec![1]);
    }
}
