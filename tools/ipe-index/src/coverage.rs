use crate::extract::sky::scan_sky;
use crate::store::Store;
use anyhow::Result;

/// A fixture/example `.sky` that imports a stdlib module `covers` that module
/// (kind `covers`). Only `Sky.*` / `Std.*` imports count — local-module imports
/// are not stdlib coverage.
pub fn record_coverage(store: &Store, path: &str, src: &str) -> Result<()> {
    for imp in scan_sky(src).imports {
        if imp.starts_with("Sky.") || imp.starts_with("Std.") {
            store.put_edge(path, &imp, "covers")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn covers_edge() {
        let s = Store::open(":memory:").unwrap();
        record_coverage(&s, "examples/x/src/Main.sky", "import Sky.Core.List as L\n").unwrap();
        assert!(s.count("edges").unwrap() >= 1);
    }

    #[test]
    fn ignores_local_imports() {
        let s = Store::open(":memory:").unwrap();
        record_coverage(&s, "examples/x/src/Main.sky", "import State\nimport Update\n").unwrap();
        assert_eq!(s.count("edges").unwrap(), 0);
    }
}
