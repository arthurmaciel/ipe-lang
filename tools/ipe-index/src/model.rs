use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang { Rust, Bash, Ts, Ipe, Other }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    // Full-Rust compiler + runtime + tooling, plus the `.ipe` stdlib/examples.
    CompilerRs, RuntimeRs, StdlibIpe, ToolRs, ScriptSh, ConsoleTs, Example, Fixture,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage { Parse, Canonicalise, Type, Build, Generate }

/// Split a repo-tagged path (`"ipe:crates/foo.rs"`) into `(tag, relpath)`.
/// Untagged paths (no `:` before the first `/`) return `("", path)`.
pub fn split_tag(path: &str) -> (&str, &str) {
    // Only treat a `:` that precedes the first path separator as a repo tag —
    // never mis-split a path that legitimately contains a colon in a segment.
    match path.find('/') {
        Some(slash) => match path[..slash].find(':') {
            Some(colon) => (&path[..colon], &path[colon + 1..]),
            None => ("", path),
        },
        None => match path.find(':') {
            Some(colon) => (&path[..colon], &path[colon + 1..]),
            None => ("", path),
        },
    }
}

pub fn lang_of(path: &str) -> Lang {
    let (_, rel) = split_tag(path);
    match Path::new(rel).extension().and_then(|e| e.to_str()) {
        Some("rs") => Lang::Rust,
        Some("sh") => Lang::Bash,
        Some("ts") | Some("tsx") | Some("mjs") | Some("js") => Lang::Ts,
        Some("ipe") => Lang::Ipe,
        _ => Lang::Other,
    }
}

pub fn role_of(path: &str) -> Role {
    let (_tag, rel) = split_tag(path);
    // Any `.ipe` source (compiled-in stdlib, examples, fixtures) classifies as
    // stdlib-ipe; the `example` overlay below refines example trees so coverage
    // edges attribute back to the example that exercises a module.
    if rel.starts_with("examples/") { Role::Example }
    else if rel.ends_with(".ipe") { Role::StdlibIpe }
    else if rel.starts_with("crates/") && rel.ends_with(".rs") { Role::CompilerRs }
    else if rel.starts_with("runtime/") && rel.ends_with(".rs") { Role::RuntimeRs }
    else if rel.starts_with("tools/") && rel.ends_with(".rs") { Role::ToolRs }
    else if rel.ends_with(".sh") { Role::ScriptSh }
    else if matches!(lang_of(rel), Lang::Ts) { Role::ConsoleTs }
    else { Role::Other }
}

pub fn stage_of(path: &str) -> Option<Stage> {
    let (_tag, rel) = split_tag(path);
    if rel.starts_with("crates/ipe_parse/") { Some(Stage::Parse) }
    else if rel.starts_with("crates/ipe_canon/") { Some(Stage::Canonicalise) }
    else if rel.starts_with("crates/ipe_types/") { Some(Stage::Type) }
    else if rel.starts_with("crates/ipe_lower/") { Some(Stage::Build) }
    else if rel.starts_with("crates/ipe_ir/") || rel.starts_with("crates/ipe_backend") { Some(Stage::Generate) }
    else { None }
}

impl Lang { pub fn as_str(&self) -> &'static str { use Lang::*; match self { Rust=>"rs",Bash=>"sh",Ts=>"ts",Ipe=>"ipe",Other=>"other" } } }
impl Role { pub fn as_str(&self) -> &'static str { use Role::*; match self { CompilerRs=>"compiler-rs",RuntimeRs=>"runtime-rs",StdlibIpe=>"stdlib-ipe",ToolRs=>"tool-rs",ScriptSh=>"script-sh",ConsoleTs=>"console-ts",Example=>"example",Fixture=>"fixture",Other=>"other" } } }
impl Stage { pub fn as_str(&self) -> &'static str { use Stage::*; match self { Parse=>"parse",Canonicalise=>"canonicalise",Type=>"type",Build=>"build",Generate=>"generate" } } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classifies_paths() {
        assert_eq!(lang_of("crates/ipe_lower/src/compile.rs"), Lang::Rust);
        assert_eq!(lang_of("a.ipe"), Lang::Ipe);
        assert_eq!(role_of("examples/wasm/counter/src/Main.ipe"), Role::Example);
        assert_eq!(role_of("crates/ipe_parse/src/lexer.rs"), Role::CompilerRs);
        assert_eq!(role_of("runtime/src/list.rs"), Role::RuntimeRs);
        assert_eq!(role_of("tools/ipe-index/src/main.rs"), Role::ToolRs);
        assert_eq!(role_of("tools/scripts/lib/wasm-verify.mjs"), Role::ConsoleTs); // JS/TS/MJS not Other
        assert_eq!(lang_of("tools/scripts/x.mjs"), Lang::Ts);
        assert_eq!(stage_of("crates/ipe_canon/src/module.rs"), Some(Stage::Canonicalise));
        assert_eq!(stage_of("crates/ipe_backend_rust/src/builder.rs"), Some(Stage::Generate));
    }
}
