//! Indexed table of a `view`'s appearance literals (style values, attribute
//! strings, static text), read at render time.
//!
//! The compiler bakes a `view`'s appearance literals in as this table's
//! defaults and emits each literal site as a `get(idx)` read. Prod holds only
//! the defaults, so the table-reading `view` renders exactly what a direct
//! literal emit would — one render semantics, dev == prod. In dev an
//! appearance edit ships a patch (`apply_patch`) over the live socket; the
//! running program swaps the affected entries and re-renders with its current
//! Model, with no recompile.
//!
//! `get` is total: an out-of-range index returns `""` rather than panicking, so
//! a stale patch index can never make a well-typed program fall over.

/// A `view`'s appearance literals, indexed by emit-assigned position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiteralTable {
    values: Vec<String>,
}

impl LiteralTable {
    /// Build a table from the compiler-baked default literals, in emit order.
    #[must_use]
    pub fn from_defaults(defaults: &[&str]) -> Self {
        Self {
            values: defaults.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    /// The literal at `idx`, or `""` when `idx` is out of range.
    ///
    /// Total by construction: no index — stale patch or otherwise — can panic.
    #[must_use]
    pub fn get(&self, idx: usize) -> &str {
        self.values.get(idx).map_or("", String::as_str)
    }

    /// Apply an appearance patch: replace the value at each given index.
    ///
    /// An out-of-range index in the patch is ignored (the patch describes
    /// entries that must already exist in the baked table), keeping the
    /// operation total.
    pub fn apply_patch(&mut self, patch: &[(usize, String)]) {
        for (idx, value) in patch {
            if let Some(slot) = self.values.get_mut(*idx) {
                *slot = value.clone();
            }
        }
    }

    /// The number of literals in the table.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the table holds no literals.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::LiteralTable;

    #[test]
    fn table_get_is_total_and_patchable() {
        let mut t = LiteralTable::from_defaults(&["12px", "red"]);
        assert_eq!(t.get(0), "12px");
        assert_eq!(t.get(99), ""); // out of range is total, never panics
        t.apply_patch(&[(0, "16px".to_string())]);
        assert_eq!(t.get(0), "16px");
        assert_eq!(t.get(1), "red"); // untouched
    }

    #[test]
    fn empty_table_get_is_empty_string() {
        let t = LiteralTable::from_defaults(&[]);
        assert!(t.is_empty());
        assert_eq!(t.len(), 0);
        assert_eq!(t.get(0), "");
    }

    #[test]
    fn out_of_range_patch_index_is_ignored() {
        let mut t = LiteralTable::from_defaults(&["a"]);
        t.apply_patch(&[(5, "z".to_string())]);
        assert_eq!(t.get(0), "a");
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn multi_entry_patch_replaces_each_named_index() {
        let mut t = LiteralTable::from_defaults(&["a", "b", "c"]);
        t.apply_patch(&[(0, "A".to_string()), (2, "C".to_string())]);
        assert_eq!(t.get(0), "A");
        assert_eq!(t.get(1), "b");
        assert_eq!(t.get(2), "C");
    }
}
