use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

/// A top-level Ipê binding with its coarse extent: the binding's own line
/// through the line before the next top-level binding (or EOF).
pub struct IpeBinding {
    pub name: String,
    /// 1-indexed source line of the binding.
    pub line: i64,
    /// 1-indexed inclusive last line of the binding's extent.
    pub line_end: i64,
}

pub struct IpeScan {
    /// `module X.Y exposing (…)` declaration, if present.
    pub module: Option<String>,
    /// Names in the `exposing (…)` list; `[".."]` when `exposing (..)`.
    pub exposing: Vec<String>,
    pub imports: Vec<String>,
    /// Top-level bindings in source order, first-seen deduped.
    pub bindings: Vec<IpeBinding>,
}

/// `module X.Y exposing (..)` or `module X.Y` / `module X.Y exposing (a, b)`.
fn re_module() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^module\s+([\w.]+)(?:\s+exposing\s*\(([^)]*)\))?").unwrap()) }
fn re_import() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^import\s+([\w.]+)").unwrap()) }
/// A candidate top-level binding: a lowercase identifier followed by
/// whitespace. Declarations that are not bindings (`module`, `import`,
/// `exposing`, `let`) are excluded — the regex would otherwise capture them as
/// bogus "bindings".
fn re_binding() -> &'static Regex { static R: OnceLock<Regex> = OnceLock::new(); R.get_or_init(|| Regex::new(r"^([a-z][\w]*)\s").unwrap()) }

fn is_decl(line: &str) -> bool {
    line.starts_with("module ") || line.starts_with("import ") || line.starts_with("exposing") || line.starts_with("let ")
}

pub fn scan_ipe(src: &str) -> IpeScan {
    let mut module = None;
    let mut exposing: Vec<String> = Vec::new();
    let mut imports = Vec::new();
    let mut names: Vec<(String, usize)> = Vec::new(); // (name, 0-based lineno)
    // O(1) dedup guard for bindings — the Vec preserves first-seen order for output
    // stability while `seen` avoids an O(n²) `bindings.contains` scan.
    let mut seen: HashSet<String> = HashSet::new();
    for (lineno, line) in src.lines().enumerate() {
        if let Some(c) = re_module().captures(line) {
            module = Some(c[1].to_string());
            if let Some(list) = c.get(2) {
                let raw = list.as_str().trim();
                exposing = if raw == ".." {
                    vec!["..".to_string()]
                } else {
                    raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
                };
            }
            continue;
        }
        if let Some(c) = re_import().captures(line) { imports.push(c[1].to_string()); continue; }
        if is_decl(line) { continue; }
        if let Some(c) = re_binding().captures(line) {
            let b = c[1].to_string();
            if seen.insert(b.clone()) { names.push((b, lineno)); }
        }
    }
    let bindings = names
        .iter()
        .enumerate()
        .map(|(idx, (name, line))| {
            let end = names.get(idx + 1).map_or(src.lines().count(), |(_, l)| *l);
            IpeBinding { name: name.clone(), line: *line as i64 + 1, line_end: end as i64 }
        })
        .collect();
    IpeScan { module, exposing, imports, bindings }
}

/// Is this binding exported from its module (`exposing (..)` exports all)?
pub fn is_pub(exposing: &[String], name: &str) -> bool {
    exposing.iter().any(|e| e == ".." || e == name)
}

/// First line of the leading `--` doc comment block directly above the binding
/// line, if any (walked upward from the binding; stops at the first non-comment
/// line). `-- | `/`-- ` markers are stripped. Nothing is fabricated: no
/// comment block → `None`.
pub fn doc_purpose(src: &str, line: i64) -> Option<String> {
    let lines: Vec<&str> = src.lines().collect();
    let mut top: Option<String> = None;
    for l in lines[..(line - 1).max(0) as usize].iter().rev() {
        let t = l.trim_start();
        if let Some(rest) = t.strip_prefix("--") {
            // The FIRST line of the block is the topmost (last visited).
            top = Some(rest.trim_start_matches(|c| c == ' ' || c == '|').trim().to_string());
        } else {
            break;
        }
    }
    top.filter(|s| !s.is_empty())
}

/// The exact source text of a binding's extent (1-indexed, inclusive).
pub fn binding_text(src: &str, line: i64, line_end: i64) -> String {
    src.lines()
        .skip((line - 1).max(0) as usize)
        .take(((line_end - line + 1).max(0)) as usize)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scans_ipe() {
        let src = "module Ipe.Core.List exposing (..)\nimport Ipe.Core.Maybe as M\nhead xs = ...\nmap f xs = ...\n";
        let r = scan_ipe(src);
        assert_eq!(r.module.as_deref(), Some("Ipe.Core.List"));
        assert_eq!(r.exposing, vec![".."]);
        assert!(r.imports.contains(&"Ipe.Core.Maybe".to_string()));
        assert!(r.bindings.iter().any(|b| b.name == "head"));
        assert!(r.bindings.iter().any(|b| b.name == "map"));
        // module/import/exposing/let are NOT bindings.
        assert!(r.bindings.iter().all(|b| b.name != "module" && b.name != "import" && b.name != "exposing" && b.name != "let"));
    }

    #[test]
    fn exposing_list_membership() {
        let src = "module X exposing (render, renderInline)\nrender s = s\nhelper x = x\n";
        let r = scan_ipe(src);
        assert_eq!(r.exposing, vec!["render".to_string(), "renderInline".to_string()]);
        assert!(is_pub(&r.exposing, "render"));
        assert!(!is_pub(&r.exposing, "helper"));
        assert!(is_pub(&["..".to_string()], "anything"));
        assert!(!is_pub(&[], "anything"));
    }

    #[test]
    fn purpose_from_leading_doc_comment() {
        let src = "-- | Docs for head\n-- more detail\nhead xs = xs\n\nmap f xs = xs\n";
        let r = scan_ipe(src);
        let head = r.bindings.iter().find(|b| b.name == "head").unwrap();
        let purpose = doc_purpose(src, head.line);
        assert_eq!(purpose.as_deref(), Some("Docs for head"));
        let map = r.bindings.iter().find(|b| b.name == "map").unwrap();
        assert_eq!(doc_purpose(src, map.line), None); // undocumented → None
    }

    #[test]
    fn binding_extents_cover_until_next_binding() {
        let src = "head xs = xs\n\nmap f xs = xs\nlast = 0\n";
        let r = scan_ipe(src);
        assert_eq!(r.bindings.len(), 3);
        assert_eq!(r.bindings[0].line, 1);
        assert_eq!(r.bindings[0].line_end, 2); // blank line + next binding start − 1
        assert_eq!(r.bindings[1].line, 3);
        assert_eq!(r.bindings[1].line_end, 3); // next binding (last) starts at 4
        assert_eq!(r.bindings[2].line, 4);
        assert_eq!(r.bindings[2].line_end, 4); // last binding → EOF
        let text = binding_text(src, 4, 4);
        assert_eq!(text, "last = 0");
    }
}
