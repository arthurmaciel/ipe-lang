pub mod sky;
pub mod treesitter;

pub use treesitter::go_registered_kernels;
pub use treesitter::treesitter_defs;

use crate::model::Lang;
use crate::store::Store;
use anyhow::Result;
use regex::Regex;
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
            for (i, line) in src.lines().enumerate() {
                if let Some(c) = re_hs_import().captures(line) { store.put_edge(path, &c[1], "import")?; }
                // Top-level defs (column-0 signature or data/type/class decl).
                else if let Some(c) = re_hs_sig().captures(line) { store.put_symbol(path, &c[1], "def", i as i64 + 1, 0)?; }
                else if let Some(c) = re_hs_decl().captures(line) { store.put_symbol(path, &c[1], "def", i as i64 + 1, 0)?; }
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
