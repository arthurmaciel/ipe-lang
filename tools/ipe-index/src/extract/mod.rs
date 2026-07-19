pub mod ipe;
pub mod treesitter;

use crate::model::Lang;
use crate::store::Store;
use anyhow::Result;
use regex::Regex;
use std::sync::OnceLock;

fn re_sh_source() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^\s*(?:source|\.)\s+(\S+)").unwrap()) }
/// Top-level Bash function: `name() {` or `function name {`.
fn re_sh_func() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^\s*(?:function\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(\s*\)\s*\{?").unwrap()) }

/// Extract symbols + import edges for one file's contents. Bounded: caller passes
/// the already-read `src`; tree-sitter trees are created + dropped inside.
pub fn extract_file(store: &Store, path: &str, lang: Lang, src: &str) -> Result<()> {
    match lang {
        Lang::Ipe => {
            let r = ipe::scan_ipe(src);
            for i in r.imports { store.put_edge(path, &i, "import")?; }
            for (b, line) in r.bindings { store.put_symbol(path, &b, "binding", line, 0)?; }
        }
        Lang::Bash => {
            for (i, line) in src.lines().enumerate() {
                if let Some(c) = re_sh_source().captures(line) { store.put_edge(path, &c[1], "import")?; }
                else if let Some(c) = re_sh_func().captures(line) { store.put_symbol(path, &c[1], "def", i as i64 + 1, 0)?; }
            }
        }
        Lang::Rust | Lang::Ts => treesitter::extract(store, path, lang, src)?,
        Lang::Other => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn defs(store: &Store, path: &str) -> Vec<(String, i64)> {
        let mut st = store.conn
            .prepare("SELECT name,line FROM symbols WHERE file=? AND kind='def' ORDER BY line")
            .unwrap();
        st.query_map([path], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    }

    // Rust impl-target capture stored as kind `impl`.
    #[test]
    fn rust_captures_impl_target() {
        let store = Store::open(":memory:").unwrap();
        let src = "struct Foo;\nimpl Foo { fn a(&self) {} }\nimpl std::fmt::Debug for Foo { }\n";
        extract_file(&store, "m.rs", Lang::Rust, src).unwrap();
        let mut st = store.conn
            .prepare("SELECT name,kind FROM symbols WHERE file='m.rs'")
            .unwrap();
        let rows: Vec<(String, String)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(rows.contains(&("Foo".to_string(), "def".to_string())));
        // Both impl blocks target Foo → two `impl` rows.
        assert_eq!(rows.iter().filter(|(n, k)| n == "Foo" && k == "impl").count(), 2);
    }

    #[test]
    fn bash_captures_functions() {
        let store = Store::open(":memory:").unwrap();
        // Both the `name()` and `function name()` forms carry the `()` the
        // column-0 scanner keys on.
        let src = "helper() {\n  :\n}\nfunction other() {\n  :\n}\n";
        extract_file(&store, "x.sh", Lang::Bash, src).unwrap();
        let names: Vec<String> = defs(&store, "x.sh").into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"helper".to_string()));
        assert!(names.contains(&"other".to_string()));
    }
}
