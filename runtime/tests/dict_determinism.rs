//! Dict determinism + negative coverage. Sky guarantees **sorted-key iteration**
//! for `Dict.keys` / `values` / `toList` (the `_fieldIndex` emission contract),
//! so these must return key-sorted output regardless of insertion order. Also
//! asserts the absent-key / idempotent-remove behaviours that must NOT panic.

use proptest::prelude::*;
use sky_runtime_rust::*;

fn build_string(pairs: &[(&str, i64)]) -> std::collections::HashMap<String, i64> {
    let mut d = sky_runtime::dict::dict_empty();
    for (k, v) in pairs {
        d = sky_runtime::dict::dict_insert(k.to_string(), *v, d);
    }
    d
}

#[test]
fn keys_values_tolist_are_key_sorted_regardless_of_insertion_order() {
    // Insert in deliberately non-sorted order.
    let d = build_string(&[("c", 3), ("a", 1), ("b", 2)]);
    assert_eq!(
        sky_runtime::dict::dict_keys(d.clone()),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
    assert_eq!(sky_runtime::dict::dict_values(d.clone()), vec![1, 2, 3]);
    assert_eq!(
        sky_runtime::dict::dict_to_list(d.clone()),
        vec![
            ("a".to_string(), 1),
            ("b".to_string(), 2),
            ("c".to_string(), 3)
        ]
    );
}

#[test]
fn int_keys_sort_numerically() {
    let mut d = sky_runtime::dict::dict_empty();
    for k in [10i64, 2, 33, 1] {
        d = sky_runtime::dict::dict_insert(k, k * 100, d);
    }
    assert_eq!(
        sky_runtime::dict::dict_keys(d.clone()),
        vec![1i64, 2, 10, 33]
    );
    assert_eq!(
        sky_runtime::dict::dict_values(d),
        vec![100, 200, 1000, 3300]
    );
}

#[test]
fn get_present_is_just_absent_is_nothing() {
    let d = build_string(&[("x", 7)]);
    assert!(matches!(
        sky_runtime::dict::dict_get("x".to_string(), d.clone()),
        SkyMaybe::Just(7)
    ));
    assert!(matches!(
        sky_runtime::dict::dict_get("missing".to_string(), d),
        SkyMaybe::Nothing
    ));
}

#[test]
fn member_true_false() {
    let d = build_string(&[("k", 1)]);
    assert!(sky_runtime::dict::dict_member("k".to_string(), d.clone()));
    assert!(!sky_runtime::dict::dict_member("nope".to_string(), d));
}

#[test]
fn remove_absent_is_idempotent_present_removes() {
    let d = build_string(&[("a", 1), ("b", 2)]);
    // Removing an absent key leaves the dict unchanged (no panic).
    let d2 = sky_runtime::dict::dict_remove("zzz".to_string(), d.clone());
    assert_eq!(
        sky_runtime::dict::dict_keys(d2),
        vec!["a".to_string(), "b".to_string()]
    );
    // Removing a present key drops it.
    let d3 = sky_runtime::dict::dict_remove("a".to_string(), d);
    assert_eq!(sky_runtime::dict::dict_keys(d3), vec!["b".to_string()]);
}

#[test]
fn empty_dict_ops_are_total() {
    let d: std::collections::HashMap<String, i64> = sky_runtime::dict::dict_empty();
    assert!(matches!(
        sky_runtime::dict::dict_get("a".to_string(), d.clone()),
        SkyMaybe::Nothing
    ));
    assert!(!sky_runtime::dict::dict_member("a".to_string(), d.clone()));
    assert_eq!(
        sky_runtime::dict::dict_keys(d.clone()),
        Vec::<String>::new()
    );
    // remove on empty: no panic.
    let d2 = sky_runtime::dict::dict_remove("a".to_string(), d);
    assert_eq!(sky_runtime::dict::dict_keys(d2), Vec::<String>::new());
}

#[test]
fn from_list_last_wins_on_duplicate_key() {
    let d = sky_runtime::dict::dict_from_list(vec![("k".to_string(), 1i64), ("k".to_string(), 2)]);
    // HashMap::from_iter keeps the last value for a duplicate key.
    assert!(matches!(
        sky_runtime::dict::dict_get("k".to_string(), d),
        SkyMaybe::Just(2)
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    // keys() is ALWAYS sorted ascending, for any insertion order / contents.
    #[test]
    fn prop_keys_always_sorted(mut pairs in proptest::collection::vec((any::<i64>(), any::<i64>()), 0..40)) {
        let mut d = sky_runtime::dict::dict_empty();
        for (k, v) in pairs.drain(..) {
            d = sky_runtime::dict::dict_insert(k, v, d);
        }
        let keys = sky_runtime::dict::dict_keys(d);
        let mut sorted = keys.clone();
        sorted.sort();
        prop_assert_eq!(keys, sorted);
    }
}
