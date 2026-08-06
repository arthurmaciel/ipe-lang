use super::{UnitSpec, blake3_hex, emit_unit, module_path};
use crate::model::{Facing, Kind, Lang, facing_of};
use crate::store::Store;
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;
use tree_sitter::{Node, Parser, Query, QueryCursor};

/// `https?://` URL inside a doc comment → an external link row.
fn re_url() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"https?://\S+").unwrap())
}

fn lang_grammar(path: &str, lang: Lang) -> Option<(tree_sitter::Language, &'static str)> {
    // (grammar, query) — query captures @def (a defined symbol), @imp (an import
    // target) and @call (a call_expression, whose `function` field is the callee).
    match lang {
        Lang::Rust => Some((
            tree_sitter_rust::language(),
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
             (use_declaration argument:(_)@imp) \
             (call_expression)@call",
        )),
        // ALL of JS/TS/MJS/TSX land here (lang_of maps js/mjs/ts/tsx -> Ts). Pick the
        // grammar variant by extension so plain JS + JSX + ESM all parse:
        //   .tsx/.jsx/.js/.mjs -> tsx grammar (superset, most permissive)
        //   .ts/.mts           -> typescript grammar
        // Same ESM import query + a const/let/var-arrow def capture for JS modules.
        // Also captures re-export statements: `export { foo } from './bar'`.
        Lang::Ts => {
            let tsx = path.ends_with(".tsx")
                || path.ends_with(".jsx")
                || path.ends_with(".js")
                || path.ends_with(".mjs");
            let g = if tsx {
                tree_sitter_typescript::language_tsx()
            } else {
                tree_sitter_typescript::language_typescript()
            };
            Some((
                g,
                "(function_declaration name:(identifier)@def) \
                      (lexical_declaration (variable_declarator name:(identifier)@def value:(arrow_function))) \
                      (import_statement source:(string)@imp) \
                      (export_statement source:(string)@imp) \
                      (call_expression)@call",
            ))
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
///   `crate::lib as renamed`             →  `["crate::lib"]` (alias stripped)
fn expand_rust_use(text: &str) -> Vec<String> {
    // Strip ` as <ident>` alias if present
    let text = if let Some(idx) = text.find(" as ") {
        text[..idx].trim()
    } else {
        text
    };
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
            '{' => {
                depth += 1;
                current.push(ch);
            }
            '}' => {
                if depth > 0 {
                    depth -= 1;
                    current.push(ch);
                }
            }
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
    if results.is_empty() {
        vec![text.to_string()]
    } else {
        results
    }
}

/// Node kinds that form an item boundary — the unit span is the whole item.
/// Rust items all end in `_item`; the TS/JS grammar uses `*_declaration` nodes
/// instead, so those are listed explicitly.
fn is_item_kind(kind: &str) -> bool {
    kind.ends_with("_item")
        || matches!(
            kind,
            "impl_item"
                | "const_item"
                | "static_item"
                | "macro_definition"
                | "function_declaration"
                | "lexical_declaration"
                | "variable_declaration"
                | "class_declaration"
        )
}

/// Walk up from a captured name node to the enclosing item node (the name's
/// parent for most items; through `generic_type` for `impl Foo<T>` targets).
fn enclosing_item(node: Node) -> Node {
    let mut cur = node;
    while let Some(p) = cur.parent() {
        if is_item_kind(p.kind()) {
            return p;
        }
        cur = p;
    }
    node
}

/// Map a tree-sitter item kind to the `units.kind` vocabulary.
fn unit_kind(item_kind: &str) -> Kind {
    match item_kind {
        "function_item"
        | "function_declaration"
        | "variable_declarator"
        | "lexical_declaration"
        | "variable_declaration" => Kind::Fn,
        "struct_item" | "class_declaration" => Kind::Struct,
        "enum_item" => Kind::Enum,
        "trait_item" => Kind::Trait,
        "const_item" | "static_item" => Kind::Const,
        "impl_item" => Kind::Impl,
        "mod_item" => Kind::Module,
        // Type aliases + macros are named bindings, not first-class items.
        "type_item" | "macro_definition" => Kind::Binding,
        _ => Kind::Block,
    }
}

/// Rust `pub` (incl. `pub(crate)`/`pub(super)`/`pub(in …)`) → visibility
/// modifier on the item; TS exports via an `export_statement` ancestor.
fn is_pub(item: Node, lang: Lang, src: &str) -> bool {
    if lang == Lang::Rust {
        let mut c = item.child(0);
        while let Some(ch) = c {
            if ch.kind() == "visibility_modifier" {
                return true;
            }
            c = ch.next_sibling();
        }
        return false;
    }
    let mut cur = item;
    while let Some(p) = cur.parent() {
        if p.kind() == "export_statement" {
            return true;
        }
        let text = &src[p.byte_range()];
        if text.starts_with("export ") {
            return true;
        }
        cur = p;
    }
    false
}

/// `#[cfg(test)]`-annotated item → test-facing even outside a test path.
fn is_cfg_test(item: Node, src: &str) -> bool {
    let mut prev = item.prev_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {
                if src[p.byte_range()].contains("cfg(test)") {
                    return true;
                }
                prev = p.prev_sibling();
            }
            "line_comment" | "block_comment" => {
                prev = p.prev_sibling();
            }
            _ => break,
        }
    }
    false
}

/// Leading doc-comment info for an item: the first line's purpose text (for the
/// unit's `purpose` column) plus every external URL (`https?://…`) in the whole
/// comment block with its 1-indexed source line (for the `links` table). The
/// comment walk mirrors the old `doc_purpose` (adjacent comments/attributes,
/// stops at the first gap). `///` and `/** */` markers are stripped for the
/// purpose line; URLs are scanned from the raw comment text.
fn doc_scan(item: Node, src: &str) -> (Option<String>, Vec<(String, i64)>) {
    let mut top: Option<String> = None;
    let mut links: Vec<(String, i64)> = Vec::new();
    let mut next_byte = item.start_byte();
    let mut prev = item.prev_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "line_comment" | "block_comment" => {
                if next_byte - p.end_byte() > 1 {
                    break;
                } // blank-line gap
                let line = p.start_position().row as i64 + 1;
                let t = src[p.byte_range()].trim_start();
                let body = if let Some(rest) = t.strip_prefix("///") {
                    Some(rest.trim().to_string())
                } else if t.starts_with("/**") {
                    Some(
                        t.trim_start_matches('/')
                            .trim()
                            .trim_start_matches('*')
                            .trim()
                            .to_string(),
                    )
                } else {
                    None
                };
                // The FIRST line of the block (topmost = last visited).
                if let Some(b) = body {
                    top = Some(b);
                }
                for m in re_url().find_iter(&src[p.byte_range()]) {
                    let url = m
                        .as_str()
                        .trim_end_matches(|c: char| ".,;:)'\"`>]}".contains(c))
                        .to_string();
                    if !url.is_empty() {
                        links.push((url, line));
                    }
                }
                next_byte = p.start_byte();
                prev = p.prev_sibling();
            }
            "attribute_item" => {
                next_byte = p.start_byte();
                prev = p.prev_sibling();
            } // doc attaches via attributes
            _ => break,
        }
    }
    (top.filter(|s| !s.is_empty()), links)
}

/// Emit one `external` link row per URL found in the unit's doc comment.
fn emit_doc_links(store: &Store, uid: &str, links: &[(String, i64)]) -> Result<()> {
    for (url, line) in links {
        store.put_link(uid, "external", None, url, *line)?;
    }
    Ok(())
}

/// The callee name of a `call_expression` — the text of its `function` field.
/// A Rust turbofish (`foo::<T>(…)`) is stripped so the name matches the unit's
/// qualified name.
fn call_function_name(call: Node, src: &str) -> Option<String> {
    let f = call.child_by_field_name("function")?;
    let text = &src[f.byte_range()];
    let text = match text.find("::<") {
        Some(i) => &text[..i],
        None => text,
    };
    Some(text.to_string())
}

/// The uid of the smallest emitted unit whose item span contains `node` — the
/// innermost enclosing item — if any.
fn enclosing_unit_uid(node: Node, unit_spans: &[(usize, usize, String)]) -> Option<String> {
    let (start, end) = (node.start_byte(), node.end_byte());
    unit_spans
        .iter()
        .filter(|(s, e, _)| *s <= start && end <= *e)
        .min_by_key(|(s, e, _)| e - s)
        .map(|(_, _, uid)| uid.clone())
}

pub fn extract(
    store: &Store,
    path: &str,
    lang: Lang,
    src: &str,
    updated_sha: &str,
    ord: &mut HashMap<(String, String), i64>,
) -> Result<()> {
    let Some((grammar, query_src)) = lang_grammar(path, lang) else {
        return Ok(());
    };
    let mut parser = Parser::new();
    parser.set_language(&grammar)?;
    let Some(tree) = parser.parse(src, None) else {
        return Ok(());
    }; // tree lives only in this scope
    let query = Query::new(&grammar, query_src)?;
    let def_idx = query.capture_index_for_name("def");
    let imp_idx = query.capture_index_for_name("imp");
    let impl_idx = query.capture_index_for_name("impldef");
    let call_idx = query.capture_index_for_name("call");
    let base_qual = module_path(path, lang);

    // Two passes. Pass 1 emits units (and their doc links) and buffers the call
    // nodes; pass 2 resolves calls against the file's COMPLETE unit set, so a
    // call to a fn defined later in the file still finds its callee.
    let mut unit_spans: Vec<(usize, usize, String)> = Vec::new();
    let mut calls: Vec<Node> = Vec::new();

    let mut cur = QueryCursor::new();
    for m in cur.matches(&query, tree.root_node(), src.as_bytes()) {
        for cap in m.captures {
            let text = &src[cap.node.byte_range()];
            let line = cap.node.start_position().row as i64 + 1;
            let col = cap.node.start_position().column as i64 + 1;
            if Some(cap.index) == def_idx {
                store.put_symbol(path, text, "def", line, col)?;
                let item = enclosing_item(cap.node);
                let span = &src[item.byte_range()];
                let (lstart, lend) = (
                    item.start_position().row as i64 + 1,
                    item.end_position().row as i64 + 1,
                );
                let facing = if is_cfg_test(item, src) {
                    Facing::Test
                } else {
                    facing_of(path, is_pub(item, lang, src))
                };
                let (purpose, links) = doc_scan(item, src);
                let uid = emit_unit(
                    store,
                    UnitSpec {
                        path,
                        kind: unit_kind(item.kind()),
                        name: text,
                        qualified: &format!("{base_qual}::{text}"),
                        line_start: lstart,
                        line_end: lend,
                        facing,
                        purpose,
                        body_hash: &blake3_hex(span.as_bytes()),
                        updated_sha,
                    },
                    ord,
                )?;
                emit_doc_links(store, &uid, &links)?;
                unit_spans.push((item.start_byte(), item.end_byte(), uid));
            } else if Some(cap.index) == impl_idx {
                // The type an `impl` block is FOR — stored as kind `impl` so
                // `locate <Type>` surfaces its impl sites alongside its def.
                store.put_symbol(path, text, "impl", line, col)?;
                let item = enclosing_item(cap.node);
                let span = &src[item.byte_range()];
                let (lstart, lend) = (
                    item.start_position().row as i64 + 1,
                    item.end_position().row as i64 + 1,
                );
                let facing = if is_cfg_test(item, src) {
                    Facing::Test
                } else {
                    facing_of(path, is_pub(item, lang, src))
                };
                let (purpose, links) = doc_scan(item, src);
                let uid = emit_unit(
                    store,
                    UnitSpec {
                        path,
                        kind: Kind::Impl,
                        name: text,
                        qualified: &format!("{base_qual}::{text}"),
                        line_start: lstart,
                        line_end: lend,
                        facing,
                        purpose,
                        body_hash: &blake3_hex(span.as_bytes()),
                        updated_sha,
                    },
                    ord,
                )?;
                emit_doc_links(store, &uid, &links)?;
                unit_spans.push((item.start_byte(), item.end_byte(), uid));
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
            } else if Some(cap.index) == call_idx {
                calls.push(cap.node);
            }
        }
    }

    // Pass 2 — callgraph edges. The callee lookup is an exact match on the unit's
    // qualified name (`uid_for_qualified`); an unqualified callee additionally
    // tries the same-module name (`{base_qual}::{callee}` — units use `::`
    // separators for every language). The caller is the innermost emitted unit
    // whose item span contains the call.
    for node in calls {
        let Some(callee) = call_function_name(node, src) else {
            continue;
        };
        let callee_uid = if callee.contains("::") || callee.contains('.') {
            store.uid_for_qualified(&callee, path)?
        } else {
            store
                .uid_for_qualified(&callee, path)?
                .or(store.uid_for_qualified(&format!("{base_qual}::{callee}"), path)?)
        };
        if let Some(callee_uid) = callee_uid
            && let Some(caller_uid) = enclosing_unit_uid(node, &unit_spans)
        {
            store.put_call(&caller_uid, &callee_uid)?;
        }
    }
    Ok(()) // `tree` dropped here, before the next file — the bounded-memory invariant
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Lang;
    use crate::store::Store;
    use std::collections::HashMap;

    fn do_extract(s: &Store, path: &str, lang: Lang, src: &str) {
        let mut ord: HashMap<(String, String), i64> = HashMap::new();
        extract(s, path, lang, src, "sha", &mut ord).unwrap();
    }

    #[test]
    fn extracts_rust_fn_and_use() {
        let s = Store::open(":memory:").unwrap();
        let src = "use crate::model::Lang;\npub fn list_head(xs: Vec<i64>) -> i64 { 0 }\n";
        do_extract(&s, "a.rs", Lang::Rust, src);
        assert_eq!(s.symbols_named("list_head").unwrap().len(), 1);
        // a `use` edge was recorded
        assert!(s.count("edges").unwrap() >= 1);
    }
    #[test]
    fn extracts_js_import_and_arrow() {
        // JS/MJS go through the tsx grammar variant; ESM import + arrow-const def.
        let s = Store::open(":memory:").unwrap();
        let src = "import { foo } from './bar.mjs';\nexport const handler = (x) => x + 1;\n";
        do_extract(&s, "x.mjs", Lang::Ts, src);
        assert_eq!(s.symbols_named("handler").unwrap().len(), 1);
        assert!(s.count("edges").unwrap() >= 1);
    }

    #[test]
    fn test_rust_grouped_use() {
        // `use std::{collections, io}` should emit ≥2 edges.
        let s = Store::open(":memory:").unwrap();
        let src = "use std::{collections, io};\npub fn foo() {}\n";
        do_extract(&s, "x.rs", Lang::Rust, src);
        let n = s.count("edges").unwrap();
        assert!(n >= 2, "expected >=2 edges for grouped use, got {n}");
    }

    #[test]
    fn test_ts_reexport() {
        // `export { foo } from './bar'` should emit an import edge for './bar'.
        let s = Store::open(":memory:").unwrap();
        let src = "export { foo } from './bar';\n";
        do_extract(&s, "x.ts", Lang::Ts, src);
        let n = s.count("edges").unwrap();
        assert!(n >= 1, "expected >=1 edge for re-export, got {n}");
    }

    #[test]
    fn rust_units_record_spans_and_hashes() {
        // `pub` fn + struct + const in one crate-root file → qualified as crate::Name.
        let s = Store::open(":memory:").unwrap();
        let src = "pub fn f() {}\npub struct S;\nimpl S { fn g(&self) {} }\n";
        do_extract(&s, "src/lib.rs", Lang::Rust, src);
        let rows: Vec<(String, String, i64, i64)> = s.conn
            .prepare("SELECT name,kind,line_start,line_end FROM units WHERE kind != 'file' ORDER BY line_start")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("f".to_string(), "fn".to_string(), 1, 1),
                ("S".to_string(), "struct".to_string(), 2, 2),
                ("S".to_string(), "impl".to_string(), 3, 3),
                // Methods are function items too — captured as fn units.
                ("g".to_string(), "fn".to_string(), 3, 3),
            ]
        );
        let q: String = s
            .conn
            .query_row("SELECT qualified FROM units WHERE kind='struct'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(q, "crate::S");
    }

    #[test]
    fn duplicate_impls_get_ordinal_uids() {
        // Two impls of the same type in one file → distinct qualified names,
        // hence distinct uids, hence two units.
        let s = Store::open(":memory:").unwrap();
        let src = "struct S;\nimpl S { fn a(&self) {} }\nimpl S { fn b(&self) {} }\n";
        do_extract(&s, "src/lib.rs", Lang::Rust, src);
        let mut st = s
            .conn
            .prepare("SELECT qualified FROM units WHERE kind='impl' ORDER BY line_start")
            .unwrap();
        let qs: Vec<String> = st
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(qs, vec!["crate::S".to_string(), "crate::S#2".to_string()]);
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
        assert!(
            result.contains(&"std::collections".to_string()),
            "got: {result:?}"
        );
        assert!(result.contains(&"std::io".to_string()), "got: {result:?}");
    }

    #[test]
    fn rust_calls_resolve_to_units() {
        // `caller` calls `helper`; the callee unit is defined after the call
        // (forward reference) — pass-2 resolution still finds it.
        let s = Store::open(":memory:").unwrap();
        let src = "pub fn caller() { helper(); }\npub fn helper() {}\n";
        do_extract(&s, "src/lib.rs", Lang::Rust, src);
        let n = s.count("callgraph").unwrap();
        assert_eq!(n, 1, "expected 1 callgraph edge, got {n}");
        let (caller, callee) = s
            .conn
            .query_row(
                "SELECT cu.qualified, ce.qualified FROM callgraph cg \
                 JOIN units cu ON cu.uid = cg.caller_uid \
                 JOIN units ce ON ce.uid = cg.callee_uid",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .unwrap();
        assert_eq!(caller, "crate::caller");
        assert_eq!(callee, "crate::helper");
    }

    #[test]
    fn ts_calls_resolve_to_units() {
        let s = Store::open(":memory:").unwrap();
        let src = "export function caller() { helper(); }\nexport function helper() {}\n";
        do_extract(&s, "x.mjs", Lang::Ts, src);
        assert_eq!(s.count("callgraph").unwrap(), 1);
    }

    #[test]
    fn doc_urls_become_external_links() {
        // A URL in the leading doc comment → an external link row keyed to the
        // unit it documents, at the comment's line.
        let s = Store::open(":memory:").unwrap();
        let src = "/// See https://example.com/docs for more.\npub fn f() {}\n";
        do_extract(&s, "src/lib.rs", Lang::Rust, src);
        assert!(s.count("links").unwrap() >= 1);
        let (from, kind, to_ref, line): (String, String, String, i64) = s
            .conn
            .query_row(
                "SELECT l.from_uid, l.to_kind, l.to_ref, l.line FROM links l \
                 JOIN units u ON u.uid = l.from_uid",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(kind, "external");
        assert_eq!(to_ref, "https://example.com/docs");
        assert_eq!(line, 1);
        assert_eq!(
            from,
            crate::store::unit_uid("src/lib.rs", Kind::Fn, "crate::f")
        );
    }

    #[test]
    fn test_ipe_module_resolves() {
        // The resolution helper maps a dotted `.ipe` module name to its file path.
        use crate::query::resolve_edges;
        let s = Store::open(":memory:").unwrap();
        s.put_file("Ipe/Core/List.ipe", "ipe", "stdlib-ipe", 0, "")
            .unwrap();
        s.put_file("Ipe/Core/Maybe.ipe", "ipe", "stdlib-ipe", 0, "")
            .unwrap();
        s.put_edge("Ipe/Core/List.ipe", "Ipe.Core.Maybe", "import")
            .unwrap();
        resolve_edges(&s, ".").unwrap();
        let resolved: Option<String> = s
            .conn
            .query_row(
                "SELECT resolved FROM edges WHERE src='Ipe/Core/List.ipe' AND dst='Ipe.Core.Maybe'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolved.as_deref(), Some("Ipe/Core/Maybe.ipe"));
    }
}
