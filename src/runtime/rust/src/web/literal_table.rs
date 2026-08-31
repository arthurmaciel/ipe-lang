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

    // Load-bearing dev == prod conformance at the mechanism level: a view whose
    // appearance literals (style value, attribute string, static text) are read
    // from a baked-default `LiteralTable` renders byte-identically to the same
    // view with those literals written directly. This is the property the whole
    // appearance-hot-swap transform rests on — reading `get(idx)` on the baked
    // defaults is indistinguishable from the direct literal, so prod (which only
    // ever holds the defaults) renders exactly what a direct emit would.
    #[test]
    fn baked_default_table_renders_identically_to_direct_literals() {
        use crate::html::{Attribute, Html, render_html};

        // A representative view: an element carrying a style attribute value and
        // a plain attribute string, wrapping a static text node — the three
        // appearance-literal kinds in Step 2's scope.
        fn view_direct() -> Html<()> {
            Html::HElement(
                "div".to_string(),
                vec![
                    Attribute::Attr("style".to_string(), "padding: 12px".to_string()),
                    Attribute::Attr("class".to_string(), "card".to_string()),
                ],
                vec![Html::HText("Hello".to_string())],
            )
        }

        fn view_tabled(t: &LiteralTable) -> Html<()> {
            Html::HElement(
                "div".to_string(),
                vec![
                    Attribute::Attr("style".to_string(), t.get(0).to_string()),
                    Attribute::Attr("class".to_string(), t.get(1).to_string()),
                ],
                vec![Html::HText(t.get(2).to_string())],
            )
        }

        let table = LiteralTable::from_defaults(&["padding: 12px", "card", "Hello"]);

        let direct = render_html(&view_direct());
        let tabled = render_html(&view_tabled(&table));
        assert_eq!(
            direct, tabled,
            "baked-default table must render byte-identically to direct literals (dev == prod)"
        );

        // And an appearance edit — a patch swapping the style value — changes
        // only that literal in the render, with no recompile and no other drift.
        let mut patched = table;
        patched.apply_patch(&[(0, "padding: 16px".to_string())]);
        let patched_render = render_html(&view_tabled(&patched));
        assert!(patched_render.contains("padding: 16px"));
        assert!(patched_render.contains("Hello"));
        assert!(patched_render.contains(r#"class="card""#));
    }
}
