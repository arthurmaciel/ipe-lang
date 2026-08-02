pub mod ipe;
pub mod treesitter;

use crate::model::{facing_of, Kind, Lang};
use crate::store::{unit_uid, Store};
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

fn re_sh_source() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^\s*(?:source|\.)\s+(\S+)").unwrap()) }
/// Top-level Bash function: `name() {` or `function name {`.
fn re_sh_func() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^\s*(?:function\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(\s*\)\s*\{?").unwrap()) }

/// Deterministic body hash over the exact span bytes.
pub fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Language-aware module path for a repo-tagged file path:
/// - Ipe: dotted module name (`Ipe.Core.List`), stripping `src/stdlib/`/`src/` roots
/// - Rust: `crate::a::b` chain from the crate root (deepest dir named `src`)
/// - Ts: file path, separators as dots, minus tag + extension
/// - Bash: the bare relative path
pub fn module_path(path: &str, lang: Lang) -> String {
    let (_tag, rel) = crate::model::split_tag(path);
    match lang {
        Lang::Ipe => {
            let rel = rel
                .strip_prefix("src/stdlib/")
                .or_else(|| rel.strip_prefix("src/"))
                .unwrap_or(rel);
            rel.strip_suffix(".ipe").unwrap_or(rel).replace('/', ".")
        }
        Lang::Rust => {
            let mut parts: Vec<&str> = rel.split('/').collect();
            parts.pop(); // the file itself
            let chain = match parts.iter().rposition(|p| *p == "src") {
                Some(i) => &parts[i + 1..],
                None => &parts[..],
            };
            if chain.is_empty() {
                "crate".to_string()
            } else {
                format!("crate::{}", chain.join("::"))
            }
        }
        Lang::Ts => {
            let rel = rel
                .strip_suffix(".ts")
                .or_else(|| rel.strip_suffix(".tsx"))
                .or_else(|| rel.strip_suffix(".mts"))
                .or_else(|| rel.strip_suffix(".js"))
                .or_else(|| rel.strip_suffix(".jsx"))
                .or_else(|| rel.strip_suffix(".mjs"))
                .unwrap_or(rel);
            rel.replace('/', ".")
        }
        _ => rel.to_string(),
    }
}

/// Emit one `units` row. Duplicate `(path, kind, qualified)` identities (e.g.
/// several `impl` blocks of the same type in one file) get an ordinal suffix
/// (`#2`) so each keeps a distinct, stable uid. Returns the unit's uid.
pub fn emit_unit(
    store: &Store,
    path: &str,
    kind: Kind,
    name: &str,
    qualified: &str,
    line_start: i64,
    line_end: i64,
    facing: crate::model::Facing,
    purpose: Option<String>,
    body_hash: &str,
    updated_sha: &str,
    ord: &mut HashMap<(String, String), i64>,
) -> Result<String> {
    let key = (path.to_string(), format!("{}|{qualified}", kind.as_str()));
    let n = ord.entry(key).or_insert(0);
    let q = if *n == 0 {
        qualified.to_string()
    } else {
        format!("{qualified}#{}", *n + 1)
    };
    *n += 1;
    let unit = crate::model::Unit {
        path: path.to_string(),
        kind,
        name: name.to_string(),
        qualified: q.clone(),
        line_start,
        line_end,
        facing,
        purpose,
        body_hash: body_hash.to_string(),
        updated_sha: updated_sha.to_string(),
    };
    store.put_unit(&unit)?;
    Ok(unit_uid(path, kind, &q))
}

/// First line of the leading `#` comment block directly above `line` in `src`
/// (walked upward; stops at the first non-comment line), with the `#` stripped.
/// No comment block → `None` (never fabricates).
fn bash_doc_purpose(src: &str, line: i64) -> Option<String> {
    let lines: Vec<&str> = src.lines().collect();
    let mut top: Option<String> = None;
    for l in lines[..(line - 1).max(0) as usize].iter().rev() {
        let t = l.trim_start();
        if let Some(rest) = t.strip_prefix('#') {
            // The FIRST line of the block is the topmost (last visited).
            top = Some(rest.trim_start().to_string());
        } else {
            break;
        }
    }
    top.filter(|s| !s.is_empty())
}

/// Extract symbols + import edges + units for one file's contents. Bounded:
/// caller passes the already-read `src`; tree-sitter trees are created + dropped
/// inside. `updated_sha` is the git sha the extraction is attributed to.
pub fn extract_file(store: &Store, path: &str, lang: Lang, src: &str, updated_sha: &str) -> Result<()> {
    let line_count = src.lines().count() as i64;
    let mut ord: HashMap<(String, String), i64> = HashMap::new();
    match lang {
        Lang::Ipe => {
            let r = ipe::scan_ipe(src);
            for i in r.imports { store.put_edge(path, &i, "import")?; }
            let base = r.module.clone().unwrap_or_else(|| module_path(path, lang));
            for b in &r.bindings {
                store.put_symbol(path, &b.name, "binding", b.line, 0)?;
                let body = ipe::binding_text(src, b.line, b.line_end);
                emit_unit(
                    store, path, Kind::Binding, &b.name,
                    &format!("{base}.{}", b.name),
                    b.line, b.line_end,
                    facing_of(path, ipe::is_pub(&r.exposing, &b.name)),
                    ipe::doc_purpose(src, b.line),
                    &blake3_hex(body.as_bytes()), updated_sha, &mut ord,
                )?;
            }
        }
        Lang::Bash => {
            let mut funcs: Vec<(String, i64)> = Vec::new();
            for (i, line) in src.lines().enumerate() {
                if let Some(c) = re_sh_source().captures(line) { store.put_edge(path, &c[1], "import")?; }
                else if let Some(c) = re_sh_func().captures(line) {
                    let name = c[1].to_string();
                    store.put_symbol(path, &name, "def", i as i64 + 1, 0)?;
                    funcs.push((name, i as i64 + 1));
                }
            }
            for (idx, (name, line)) in funcs.iter().enumerate() {
                let end = funcs.get(idx + 1).map_or(line_count, |(_, l)| l - 1);
                let body = lines_between(src, *line, end);
                emit_unit(
                    store, path, Kind::Fn, name,
                    &format!("{}::{name}", module_path(path, lang)),
                    *line, end,
                    facing_of(path, false),
                    bash_doc_purpose(src, *line),
                    &blake3_hex(body.as_bytes()), updated_sha, &mut ord,
                )?;
            }
        }
        Lang::Rust | Lang::Ts => treesitter::extract(store, path, lang, src, updated_sha, &mut ord)?,
        Lang::Other => return Ok(()),
    }
    // Whole-file unit (the reviewable floor for every indexed file).
    let name = crate::model::split_tag(path)
        .1
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string();
    let base = module_path(path, lang);
    emit_unit(
        store, path, Kind::File, &name,
        &format!("{base}::FILE"),
        1, line_count,
        facing_of(path, false),
        None,
        &blake3_hex(src.as_bytes()), updated_sha, &mut ord,
    )?;
    Ok(())
}

/// `src` lines `[start..=end]` joined with `\n` (1-indexed, clamped).
fn lines_between(src: &str, start: i64, end: i64) -> String {
    src.lines()
        .skip((start - 1).max(0) as usize)
        .take(((end - start + 1).max(0)) as usize)
        .collect::<Vec<_>>()
        .join("\n")
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
        extract_file(&store, "m.rs", Lang::Rust, src, "sha").unwrap();
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
        extract_file(&store, "x.sh", Lang::Bash, src, "sha").unwrap();
        let names: Vec<String> = defs(&store, "x.sh").into_iter().map(|(n, _)| n).collect();
        assert!(names.contains(&"helper".to_string()));
        assert!(names.contains(&"other".to_string()));
    }

    #[test]
    fn rust_unit_span_covers_whole_fn() {
        let store = Store::open(":memory:").unwrap();
        let src = "pub fn list_head(xs: Vec<i64>) -> i64 {\n    0\n}\n";
        extract_file(&store, "src/lib.rs", Lang::Rust, src, "sha").unwrap();
        let row: (String, i64, i64, String) = store.conn.query_row(
            "SELECT qualified,line_start,line_end,body_hash FROM units WHERE kind='fn'",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).unwrap();
        assert_eq!(row.0, "crate::list_head");
        assert_eq!((row.1, row.2), (1, 3)); // whole fn incl. body
        // One-byte edit → hash changes.
        let store2 = Store::open(":memory:").unwrap();
        extract_file(&store2, "src/lib.rs", Lang::Rust, "pub fn list_head(xs: Vec<i64>) -> i64 {\n    1\n}\n", "sha").unwrap();
        let h2: String = store2.conn.query_row(
            "SELECT body_hash FROM units WHERE kind='fn'", [], |r| r.get(0),
        ).unwrap();
        assert_ne!(row.3, h2);
    }

    #[test]
    fn file_unit_exists_for_unknown_langs_only() {
        // `Other` is skipped entirely (no file unit); every indexed lang gets one.
        let store = Store::open(":memory:").unwrap();
        extract_file(&store, "foo.txt", Lang::Other, "x", "sha").unwrap();
        assert_eq!(store.count("units").unwrap(), 0);
        let store = Store::open(":memory:").unwrap();
        extract_file(&store, "src/lib.rs", Lang::Rust, "pub fn f() {}\n", "sha").unwrap();
        assert_eq!(store.count("units").unwrap(), 2); // fn + FILE
        let store = Store::open(":memory:").unwrap();
        extract_file(&store, "src/a.ipe", Lang::Ipe, "x = 1\n", "sha").unwrap();
        assert_eq!(store.count("units").unwrap(), 2); // binding + FILE
    }
}
