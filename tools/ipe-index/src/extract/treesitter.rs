use crate::model::Lang;
use crate::store::Store;
use anyhow::Result;
use tree_sitter::{Parser, Query, QueryCursor};

fn lang_grammar(path: &str, lang: Lang) -> Option<(tree_sitter::Language, &'static str)> {
    // (grammar, query) — query captures @def (a defined symbol) and @imp (an import target)
    match lang {
        Lang::Rust => Some((tree_sitter_rust::language(),
            "(function_item name:(identifier)@def) \
             (struct_item name:(type_identifier)@def) \
             (enum_item name:(type_identifier)@def) \
             (trait_item name:(type_identifier)@def) \
             (type_item name:(type_identifier)@def) \
             (const_item name:(identifier)@def) \
             (static_item name:(identifier)@def) \
             (macro_definition name:(identifier)@def) \
             (mod_item name:(identifier)@def) \
             (impl_item type:(type_identifier)@impldef) \
             (impl_item type:(generic_type (type_identifier)@impldef)) \
             (use_declaration argument:(_)@imp)")),
        // ALL of JS/TS/MJS/TSX land here (lang_of maps js/mjs/ts/tsx -> Ts). Pick the
        // grammar variant by extension so plain JS + JSX + ESM all parse:
        //   .tsx/.jsx/.js/.mjs -> tsx grammar (superset, most permissive)
        //   .ts/.mts           -> typescript grammar
        // Same ESM import query + a const/let/var-arrow def capture for JS modules.
        // Also captures re-export statements: `export { foo } from './bar'`.
        Lang::Ts => {
            let tsx = path.ends_with(".tsx") || path.ends_with(".jsx")
                   || path.ends_with(".js")  || path.ends_with(".mjs");
            let g = if tsx { tree_sitter_typescript::language_tsx() }
                    else   { tree_sitter_typescript::language_typescript() };
            Some((g, "(function_declaration name:(identifier)@def) \
                      (lexical_declaration (variable_declarator name:(identifier)@def value:(arrow_function))) \
                      (import_statement source:(string)@imp) \
                      (export_statement source:(string)@imp)"))
        }
        _ => None,
    }
}

/// Expand a Rust grouped use path like `crate::{model::Lang, store::Store}` into
/// individual module paths. Returns the individual paths if the text contains `{`,
/// otherwise returns a single-element vec with the original text.
///
/// Examples:
///   `std::{collections::HashMap, io}`  →  `["std::collections::HashMap", "std::io"]`
///   `crate::model`                      →  `["crate::model"]`
fn expand_rust_use(text: &str) -> Vec<String> {
    if !text.contains('{') {
        return vec![text.to_string()];
    }
    // Find the prefix before `{` and the items inside `{...}`.
    let brace_start = match text.find('{') {
        Some(i) => i,
        None => return vec![text.to_string()],
    };
    let prefix = text[..brace_start].trim_end_matches(':').trim();
    let inner_start = brace_start + 1;
    let inner_end = text.rfind('}').unwrap_or(text.len());
    // Defence-in-depth: error-recovered / malformed parse text can place `}` before
    // `{` (e.g. `a::}{b`), giving inner_end < inner_start. Slicing a reversed range
    // panics, so bail to the unexpanded text instead. Well-formed tree-sitter output
    // never hits this.
    if inner_end <= inner_start {
        return vec![text.to_string()];
    }
    let inner = &text[inner_start..inner_end];

    // Split on commas at depth 0 (no nested braces for now — covers the common case).
    let mut results = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for ch in inner.chars() {
        match ch {
            '{' => { depth += 1; current.push(ch); }
            '}' => { if depth > 0 { depth -= 1; current.push(ch); } }
            ',' if depth == 0 => {
                let item = current.trim().to_string();
                if !item.is_empty() {
                    // Handle `self` (re-export of the prefix itself) as just the prefix.
                    if item == "self" {
                        results.push(prefix.to_string());
                    } else if prefix.is_empty() {
                        results.push(item);
                    } else {
                        results.push(format!("{prefix}::{item}"));
                    }
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let item = current.trim().to_string();
    if !item.is_empty() {
        if item == "self" {
            results.push(prefix.to_string());
        } else if prefix.is_empty() {
            results.push(item);
        } else {
            results.push(format!("{prefix}::{item}"));
        }
    }
    if results.is_empty() { vec![text.to_string()] } else { results }
}

pub fn extract(store: &Store, path: &str, lang: Lang, src: &str) -> Result<()> {
    let Some((grammar, query_src)) = lang_grammar(path, lang) else { return Ok(()) };
    let mut parser = Parser::new();
    parser.set_language(&grammar)?;
    let Some(tree) = parser.parse(src, None) else { return Ok(()) };   // tree lives only in this scope
    let query = Query::new(&grammar, query_src)?;
    let def_idx = query.capture_index_for_name("def");
    let imp_idx = query.capture_index_for_name("imp");
    let impl_idx = query.capture_index_for_name("impldef");
    let mut cur = QueryCursor::new();
    for m in cur.matches(&query, tree.root_node(), src.as_bytes()) {
        for cap in m.captures {
            let text = &src[cap.node.byte_range()];
            let line = cap.node.start_position().row as i64 + 1;
            let col  = cap.node.start_position().column as i64 + 1;
            if Some(cap.index) == def_idx {
                store.put_symbol(path, text, "def", line, col)?;
            } else if Some(cap.index) == impl_idx {
                // The type an `impl` block is FOR — stored as kind `impl` so
                // `locate <Type>` surfaces its impl sites alongside its def.
                store.put_symbol(path, text, "impl", line, col)?;
            } else if Some(cap.index) == imp_idx {
                let target = text.trim_matches(|c| c == '"' || c == '\'');
                // For Rust, expand grouped use paths: `a::{b, c}` → `a::b`, `a::c`.
                if lang == Lang::Rust {
                    for expanded in expand_rust_use(target) {
                        store.put_edge(path, &expanded, "import")?;
                    }
                } else {
                    store.put_edge(path, target, "import")?;
                }
            }
        }
    }
    Ok(())   // `tree` dropped here, before the next file — the bounded-memory invariant
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store; use crate::model::Lang;

    #[test]
    fn extracts_rust_fn_and_use() {
        let s = Store::open(":memory:").unwrap();
        let src = "use crate::model::Lang;\npub fn list_head(xs: Vec<i64>) -> i64 { 0 }\n";
        extract(&s, "a.rs", Lang::Rust, src).unwrap();
        assert_eq!(s.symbols_named("list_head").unwrap().len(), 1);
        // a `use` edge was recorded
        assert!(s.count("edges").unwrap() >= 1);
    }
    #[test]
    fn extracts_js_import_and_arrow() {
        // JS/MJS go through the tsx grammar variant; ESM import + arrow-const def.
        let s = Store::open(":memory:").unwrap();
        let src = "import { foo } from './bar.mjs';\nexport const handler = (x) => x + 1;\n";
        extract(&s, "x.mjs", Lang::Ts, src).unwrap();
        assert_eq!(s.symbols_named("handler").unwrap().len(), 1);
        assert!(s.count("edges").unwrap() >= 1);
    }

    #[test]
    fn test_rust_grouped_use() {
        // `use std::{collections, io}` should emit ≥2 edges.
        let s = Store::open(":memory:").unwrap();
        let src = "use std::{collections, io};\npub fn foo() {}\n";
        extract(&s, "x.rs", Lang::Rust, src).unwrap();
        let n = s.count("edges").unwrap();
        assert!(n >= 2, "expected >=2 edges for grouped use, got {n}");
    }

    #[test]
    fn test_ts_reexport() {
        // `export { foo } from './bar'` should emit an import edge for './bar'.
        let s = Store::open(":memory:").unwrap();
        let src = "export { foo } from './bar';\n";
        extract(&s, "x.ts", Lang::Ts, src).unwrap();
        let n = s.count("edges").unwrap();
        assert!(n >= 1, "expected >=1 edge for re-export, got {n}");
    }

    #[test]
    fn expand_rust_use_simple() {
        let result = expand_rust_use("crate::model::Lang");
        assert_eq!(result, vec!["crate::model::Lang"]);
    }

    #[test]
    fn expand_rust_use_grouped() {
        let mut result = expand_rust_use("std::{collections, io}");
        result.sort();
        assert!(result.contains(&"std::collections".to_string()), "got: {result:?}");
        assert!(result.contains(&"std::io".to_string()), "got: {result:?}");
    }

    #[test]
    fn test_ipe_module_resolves() {
        // The resolution helper maps a dotted `.ipe` module name to its file path.
        use crate::query::resolve_edges;
        let s = Store::open(":memory:").unwrap();
        s.put_file("Ipe/Core/List.ipe", "ipe", "stdlib-ipe", 0, "").unwrap();
        s.put_file("Ipe/Core/Maybe.ipe", "ipe", "stdlib-ipe", 0, "").unwrap();
        s.put_edge("Ipe/Core/List.ipe", "Ipe.Core.Maybe", "import").unwrap();
        resolve_edges(&s, ".").unwrap();
        let resolved: Option<String> = s.conn.query_row(
            "SELECT resolved FROM edges WHERE src='Ipe/Core/List.ipe' AND dst='Ipe.Core.Maybe'",
            [], |r| r.get(0)
        ).unwrap();
        assert_eq!(resolved.as_deref(), Some("Ipe/Core/Maybe.ipe"));
    }
}
