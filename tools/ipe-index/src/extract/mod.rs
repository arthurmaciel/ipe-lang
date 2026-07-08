pub mod sky;
pub mod treesitter;

pub use treesitter::go_registered_kernels;
pub use treesitter::treesitter_defs;

use crate::model::Lang;
use crate::store::Store;
use anyhow::Result;
use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

fn re_hs_import() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^import\s+(?:qualified\s+)?([\w.]+)").unwrap()) }
fn re_sh_source() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^\s*(?:source|\.)\s+(\S+)").unwrap()) }
/// Top-level Haskell value binding via its signature `name :: T` at column 0.
/// tree-sitter-haskell lags the ABI (see Cargo.toml note), so a robust column-0
/// line-scan of the signature line — the canonical def-site for compiler modules
/// — beats a fragile grammar. Captures the leading name of a possibly
/// comma-separated signature (`a, b :: T` → `a`).
fn re_hs_sig() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^([a-z_][A-Za-z0-9_']*)\s*::").unwrap()) }
/// Top-level Haskell data/newtype/type/class declaration.
fn re_hs_decl() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^(?:data|newtype|type|class)\s+([A-Z][A-Za-z0-9_']*)").unwrap()) }
/// Top-level Haskell equation / value binding at column 0: `name args… =` or
/// `name = …`. Captures the leading name and everything up to the FIRST `=`
/// (group 2) so the caller can reject a signature-continuation (`::`) and an
/// equality operator (`==`). Only used when no signature/decl matched first, so
/// signed functions keep their signature line as the canonical def-site.
fn re_hs_eqn() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^([a-z_][A-Za-z0-9_']*)([^=]*)=(=?)").unwrap()) }
/// Keywords that can begin a column-0 line but are never a value binding.
const HS_KEYWORDS: &[&str] = &[
    "module", "import", "instance", "class", "data", "newtype", "type",
    "deriving", "infixl", "infixr", "infix", "foreign", "where", "let", "in", "do",
];
/// Top-level Bash function: `name() {` or `function name {`.
fn re_sh_func() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^\s*(?:function\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(\s*\)\s*\{?").unwrap()) }

/// Extract symbols + import edges for one file's contents. Bounded: caller passes
/// the already-read `src`; tree-sitter trees are created + dropped inside.
pub fn extract_file(store: &Store, path: &str, lang: Lang, src: &str) -> Result<()> {
    match lang {
        Lang::Sky => {
            let r = sky::scan_sky(src);
            for i in r.imports { store.put_edge(path, &i, "import")?; }
            for (b, line) in r.bindings { store.put_symbol(path, &b, "binding", line, 0)?; }
            // kernels handled by parity.rs over the whole repo
        }
        Lang::Haskell => {
            // Track names already emitted in THIS file so a signed function is
            // recorded once (at its signature), not again at its equation, and a
            // multi-equation function is recorded once (at its first clause).
            let mut seen: HashSet<&str> = HashSet::new();
            for (i, line) in src.lines().enumerate() {
                let ln = i as i64 + 1;
                if let Some(c) = re_hs_import().captures(line) {
                    store.put_edge(path, &c[1], "import")?;
                } else if let Some(m) = re_hs_sig().captures(line) {
                    let n = m.get(1).map_or("", |x| x.as_str());
                    if seen.insert(n) { store.put_symbol(path, n, "def", ln, 0)?; }
                } else if let Some(m) = re_hs_decl().captures(line) {
                    let n = m.get(1).map_or("", |x| x.as_str());
                    if seen.insert(n) { store.put_symbol(path, n, "def", ln, 0)?; }
                } else if let Some(m) = re_hs_eqn().captures(line) {
                    let n = m.get(1).map_or("", |x| x.as_str());
                    let mid = m.get(2).map_or("", |x| x.as_str()); // text before the first '='
                    let dbl = m.get(3).map_or("", |x| x.as_str());  // "=" if the op was "=="
                    // Reject: keyword, a signature-continuation (`::` before `=`),
                    // an equality/operator (`==`, `/=`, `<=`, `>=`), and re-emits.
                    let is_cmp = dbl == "=" || mid.ends_with('/') || mid.ends_with('<') || mid.ends_with('>');
                    if !n.is_empty() && !HS_KEYWORDS.contains(&n) && !mid.contains("::")
                        && !is_cmp && seen.insert(n)
                    {
                        store.put_symbol(path, n, "def", ln, 0)?;
                    }
                }
            }
        }
        Lang::Bash => {
            for (i, line) in src.lines().enumerate() {
                if let Some(c) = re_sh_source().captures(line) { store.put_edge(path, &c[1], "import")?; }
                else if let Some(c) = re_sh_func().captures(line) { store.put_symbol(path, &c[1], "def", i as i64 + 1, 0)?; }
            }
        }
        Lang::Go => {
            treesitter::extract(store, path, lang, src)?;
            // Also capture kernels registered via string literals, e.g.
            //   RegisterPure("Decimal_add", func(args []any) any { ... })
            // tree-sitter only sees the anonymous closure, never the name.
            // Store with the real line number so loc lookups find them.
            for (name, line) in treesitter::go_registered_kernels(src) {
                store.put_symbol(path, &name, "def", line, 0)?;
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

    // v2: Haskell equation defs, deduped against the signature line.
    #[test]
    fn haskell_dedups_sig_and_equations() {
        let store = Store::open(":memory:").unwrap();
        // `foo` is signed then defined over two clauses; `bar` has no signature.
        let src = "foo :: Int -> Int\nfoo x = x + 1\nfoo 0 = 0\nbar y = y\n";
        extract_file(&store, "m.hs", Lang::Haskell, src).unwrap();
        // foo recorded ONCE (at its signature, line 1); bar ONCE (its equation, line 4).
        assert_eq!(defs(&store, "m.hs"), vec![("foo".to_string(), 1), ("bar".to_string(), 4)]);
    }

    #[test]
    fn haskell_rejects_keywords_and_comparisons() {
        let store = Store::open(":memory:").unwrap();
        // `module ... where` has no '='; a guard-style `==` line must not be a def.
        let src = "module M where\nx = 1\ny == z = False\n";
        extract_file(&store, "k.hs", Lang::Haskell, src).unwrap();
        // Only `x` (a real value binding). `module` and the `==` line are rejected.
        assert_eq!(defs(&store, "k.hs"), vec![("x".to_string(), 2)]);
    }

    // v2: Rust impl-target capture stored as kind `impl`.
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
}
