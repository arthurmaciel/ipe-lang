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

/// Unit kinds stored in `units.kind` — a closed set mirrored by the DB CHECK
/// constraint, so an invalid kind is unrepresentable at both layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Module, File, Fn, Struct, Enum, Impl, Const, Binding, Block, Trait }

/// `units.facing` — closed set mirrored by the DB CHECK constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing { User, Internal, Test }

/// A reviewable source unit: a content-stable id (`uid` = blake3 of
/// `path|kind|qualified`), a span, classification, and a body hash binding
/// the row to the exact source bytes it describes.
pub struct Unit {
    pub path: String,
    pub kind: Kind,
    pub name: String,
    pub qualified: String,
    pub line_start: i64,
    pub line_end: i64,
    pub facing: Facing,
    pub purpose: Option<String>,
    pub body_hash: String,
    pub updated_sha: String,
}

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
impl Kind { pub fn as_str(&self) -> &'static str { use Kind::*; match self { Module=>"module",File=>"file",Fn=>"fn",Struct=>"struct",Enum=>"enum",Impl=>"impl",Const=>"const",Binding=>"binding",Block=>"block",Trait=>"trait" } } }
impl Facing { pub fn as_str(&self) -> &'static str { use Facing::*; match self { User=>"user",Internal=>"internal",Test=>"test" } } }

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
        assert_eq!(role_of("scripts/lib/wasm-verify.mjs"), Role::ConsoleTs); // JS/TS/MJS not Other
        assert_eq!(lang_of("scripts/x.mjs"), Lang::Ts);
        assert_eq!(stage_of("crates/ipe_canon/src/module.rs"), Some(Stage::Canonicalise));
        assert_eq!(stage_of("crates/ipe_backend_rust/src/builder.rs"), Some(Stage::Generate));
    }
}
