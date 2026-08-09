use crate::model::stage_of;
use crate::store::Store;
use anyhow::Result;

/// Records a `file → compiler-stage` edge (kind `in-stage`) for compiler files
/// that map to a known compiler pipeline stage.
pub fn record_stage(store: &Store, path: &str) -> Result<()> {
    if let Some(st) = stage_of(path) {
        store.put_edge(path, st.as_str(), "in-stage")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn stage_edge() {
        let s = Store::open(":memory:").unwrap();
        record_stage(&s, "src/compiler/types/src/solve.rs").unwrap();
        assert_eq!(s.count("edges").unwrap(), 1);
    }

    #[test]
    fn no_stage_no_edge() {
        let s = Store::open(":memory:").unwrap();
        record_stage(&s, "src/runtime/rust/src/rt.rs").unwrap();
        assert_eq!(s.count("edges").unwrap(), 0);
    }
}
