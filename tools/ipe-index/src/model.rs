use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang { Haskell, Go, Rust, Bash, Ts, Sky, Other }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    // Sky reference repo (../sky): Haskell compiler + Go backend/runtime + Sky stdlib
    // + the Rust ancestor port (runtime-rust/).
    CompilerHs, RuntimeGo, RuntimeRust, StdlibSky, ScriptSh, ConsoleTs, Example, Fixture,
    // Ipê repo (this one): full-Rust compiler + runtime + tooling. Distinct roles so
    // `roles`/`parity` can tell an Ipê-Rust impl from the Sky-Rust ancestor.
    IpeCompilerRs, IpeRuntimeRs, IpeStdlibSky, IpeToolRs,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage { Parse, Canonicalise, Type, Build, Generate }

/// Split a repo-tagged path (`"ipe:crates/foo.rs"`) into `(tag, relpath)`.
/// Untagged paths (no `:` before the first `/`) return `("", path)` so the
/// classifier keeps its legacy Sky-repo behaviour for callers that pass raw paths.
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
        Some("hs") => Lang::Haskell,
        Some("go") => Lang::Go,
        Some("rs") => Lang::Rust,
        Some("sh") => Lang::Bash,
        Some("ts") | Some("tsx") | Some("mjs") | Some("js") => Lang::Ts,
        Some("sky") => Lang::Sky,
        _ => Lang::Other,
    }
}

pub fn role_of(path: &str) -> Role {
    let (tag, rel) = split_tag(path);
    if tag == "ipe" {
        return role_of_ipe(rel);
    }
    // Sky reference repo (tag "sky" or untagged legacy paths).
    if rel.starts_with("runtime-rust/tests/sky/") { Role::Fixture }
    else if rel.starts_with("examples/") { Role::Example }
    else if rel.starts_with("src/Sky/") || rel.starts_with("app/") { Role::CompilerHs }
    else if rel.starts_with("runtime-go/") { Role::RuntimeGo }
    else if rel.starts_with("runtime-rust/src/") { Role::RuntimeRust }
    else if rel.starts_with("sky-stdlib/") { Role::StdlibSky }
    else if rel.starts_with("sky-bundled/") { Role::ConsoleTs }
    else if rel.ends_with(".sh") { Role::ScriptSh }
    else if matches!(lang_of(rel), Lang::Ts) { Role::ConsoleTs } // any .ts/.tsx/.js/.mjs (incl. scripts)
    else { Role::Other }
}

/// Ipê-repo (`crates/` compiler, `runtime/` runtime, `tools/` tooling,
/// `crates/skyc/stdlib/**.sky` Sky stdlib) classifier.
fn role_of_ipe(rel: &str) -> Role {
    // Any .sky source is stdlib-Sky (the `crates/skyc/stdlib/` bundle and any
    // other bundled Sky file classify identically).
    if rel.ends_with(".sky") { Role::IpeStdlibSky }
    else if rel.starts_with("crates/") && rel.ends_with(".rs") { Role::IpeCompilerRs }
    else if rel.starts_with("runtime/") && rel.ends_with(".rs") { Role::IpeRuntimeRs }
    else if rel.starts_with("tools/") && rel.ends_with(".rs") { Role::IpeToolRs }
    else if rel.ends_with(".sh") { Role::ScriptSh }
    else if matches!(lang_of(rel), Lang::Ts) { Role::ConsoleTs }
    else { Role::Other }
}

pub fn stage_of(path: &str) -> Option<Stage> {
    let (tag, rel) = split_tag(path);
    if tag == "ipe" {
        // Ipê stage crates mirror the Haskell stage modules.
        if rel.starts_with("crates/sky_parse/") { Some(Stage::Parse) }
        else if rel.starts_with("crates/sky_canon/") { Some(Stage::Canonicalise) }
        else if rel.starts_with("crates/sky_types/") { Some(Stage::Type) }
        else if rel.starts_with("crates/sky_lower/") { Some(Stage::Build) }
        else if rel.starts_with("crates/sky_ir/") || rel.starts_with("crates/sky_backend") { Some(Stage::Generate) }
        else { None }
    } else if rel.starts_with("src/Sky/Parse/") { Some(Stage::Parse) }
    else if rel.starts_with("src/Sky/Canonicalise/") { Some(Stage::Canonicalise) }
    else if rel.starts_with("src/Sky/Type/") { Some(Stage::Type) }
    else if rel.starts_with("src/Sky/Build/") { Some(Stage::Build) }
    else if rel.starts_with("src/Sky/Generate/") { Some(Stage::Generate) }
    else { None }
}

impl Lang { pub fn as_str(&self) -> &'static str { use Lang::*; match self { Haskell=>"hs",Go=>"go",Rust=>"rs",Bash=>"sh",Ts=>"ts",Sky=>"sky",Other=>"other" } } }
impl Role { pub fn as_str(&self) -> &'static str { use Role::*; match self { CompilerHs=>"compiler-hs",RuntimeGo=>"runtime-go",RuntimeRust=>"runtime-rust",StdlibSky=>"stdlib-sky",ScriptSh=>"script-sh",ConsoleTs=>"console-ts",Example=>"example",Fixture=>"fixture",IpeCompilerRs=>"ipe-compiler-rs",IpeRuntimeRs=>"ipe-runtime-rs",IpeStdlibSky=>"ipe-stdlib-sky",IpeToolRs=>"ipe-tool-rs",Other=>"other" } } }
impl Stage { pub fn as_str(&self) -> &'static str { use Stage::*; match self { Parse=>"parse",Canonicalise=>"canonicalise",Type=>"type",Build=>"build",Generate=>"generate" } } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classifies_paths() {
        assert_eq!(lang_of("src/Sky/Build/Compile.hs"), Lang::Haskell);
        assert_eq!(lang_of("runtime-rust/src/sky_runtime/list.rs"), Lang::Rust);
        assert_eq!(lang_of("a.sky"), Lang::Sky);
        assert_eq!(role_of("runtime-rust/tests/sky/49-x/src/Main.sky"), Role::Fixture);
        assert_eq!(role_of("examples/13-skyshop/src/Main.sky"), Role::Example);
        assert_eq!(role_of("src/Sky/Parse/Lexer.hs"), Role::CompilerHs);
        assert_eq!(role_of("runtime-go/rt/rt.go"), Role::RuntimeGo);
        assert_eq!(role_of("sky-stdlib/Sky/Core/List.sky"), Role::StdlibSky);
        assert_eq!(role_of("scripts/web-verify.mjs"), Role::ConsoleTs); // JS/TS/MJS not Other
        assert_eq!(lang_of("scripts/x.mjs"), Lang::Ts);
        assert_eq!(stage_of("src/Sky/Canonicalise/Module.hs"), Some(Stage::Canonicalise));
        assert_eq!(stage_of("src/Sky/Generate/Rust/Builder.hs"), Some(Stage::Generate));
    }
}
