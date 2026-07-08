use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub struct Kernel {
    pub name: String,
    pub rust_fn: String,
    pub go_impl: bool,
    pub rust_impl: bool,
    /// True when this kernel is declared `Ffi.kernel "Name"` in sky-stdlib — meaning
    /// any Sky program that uses this function routes it as a kernel call that the
    /// Rust runtime MUST provide.  False means Go has a kernel optimisation for it
    /// but the stdlib implements it as pure Sky, so the Rust backend needs no kernel.
    pub sky_decl: bool,
    pub parity: String,
    /// "path:line" of the routing row in Kernel.hs (if known).
    pub hs_route_loc: Option<String>,
}

/// The Rust Builder Kernel.hs routing shape: `("Mod","fn") -> "rust_fn"`.
fn re_route_rust() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"\(\s*"(\w+)"\s*,\s*"(\w+)"\s*\)\s*->\s*"(\w+)""#).unwrap())
}

/// The Go Kernel.hs routing shape: `(("Mod","fn"), KernelInfo "rt.Mod_fn" arity ...)`.
/// A kernel present here but absent from the Rust file is exactly the go-only gap
/// class (e.g. `Dict.union`).
fn re_route_go() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"\(\(\s*"(\w+)"\s*,\s*"(\w+)"\s*\)\s*,\s*KernelInfo"#).unwrap())
}

/// Conventional Rust kernel-fn name for a `Mod.fn` kernel: snake_case the dotted
/// name (`Dict.union` -> `dict_union`). Mirrors the Rust Builder Kernel.hs naming
/// so a Go-only route reconciles as a real gap (absent from rust_fns).
/// A few kernels are reached via an ExprEmitter PEEPHOLE rewriter, so their
/// runtime fn name differs from the conventional `mod_fn` snake-case AND isn't in
/// the Kernel.hs route table. Map those explicitly so parity stays honest — else
/// they read as a phantom `go-only` gap even though they're fully implemented.
fn peephole_alias_present(kernel: &str, rust_fns: &HashSet<String>) -> bool {
    let alias = match kernel {
        // Sub.subscribeWebSocket lowers via the ExprEmitter.hs peephole, which
        // splits on the literal kind into ws_client::sub_subscribe_ws_{message,
        // open,close,error}. Presence of the message variant proves it's wired.
        "Sub.subscribeWebSocket" => Some("sub_subscribe_ws_message"),
        _ => None,
    };
    alias.is_some_and(|a| rust_fns.contains(a))
}

fn conventional_rust_fn(kernel: &str) -> String {
    let mut out = String::with_capacity(kernel.len() + 4);
    for ch in kernel.chars() {
        if ch == '.' {
            out.push('_');
        } else if ch.is_ascii_uppercase() {
            if !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Route info for a single kernel: the rust_fn alias and optional source location.
#[derive(Debug, Clone)]
pub struct RouteInfo {
    pub kernel_name: String,
    /// `"path:line"` of the routing row in Kernel.hs (empty string = unknown).
    pub hs_loc: String,
}

/// rust_fn -> RouteInfo, unioning BOTH Kernel.hs shapes.
/// `hs_src_pairs` is a slice of `(file_path, source_text)` pairs so we can
/// track per-file line numbers accurately.
pub fn parse_routes_with_locs(hs_src_pairs: &[(&str, &str)]) -> HashMap<String, RouteInfo> {
    let mut m: HashMap<String, RouteInfo> = HashMap::new();
    let mut rust_kernels: HashSet<String> = HashSet::new();

    // Pass 1: Rust explicit routes (authoritative rust_fn names).
    for (file, src) in hs_src_pairs {
        for (lineno, line) in src.lines().enumerate() {
            if let Some(c) = re_route_rust().captures(line) {
                let kernel = format!("{}.{}", &c[1], &c[2]);
                rust_kernels.insert(kernel.clone());
                let loc = format!("{}:{}", file, lineno + 1);
                m.insert(c[3].to_string(), RouteInfo { kernel_name: kernel, hs_loc: loc });
            }
        }
    }
    // Pass 2: Go KernelInfo routes — only the ones the Rust file didn't name.
    for (file, src) in hs_src_pairs {
        for (lineno, line) in src.lines().enumerate() {
            if let Some(c) = re_route_go().captures(line) {
                let kernel = format!("{}.{}", &c[1], &c[2]);
                if rust_kernels.contains(&kernel) {
                    continue;
                }
                let loc = format!("{}:{}", file, lineno + 1);
                m.entry(conventional_rust_fn(&kernel)).or_insert(RouteInfo { kernel_name: kernel, hs_loc: loc });
            }
        }
    }
    m
}

/// Backward-compat wrapper used by tests that pass a single concatenated string
/// with no file-path info. Produces the same map shape as v1 (HashMap<String,String>)
/// for the existing test helpers; the new call sites use `parse_routes_with_locs`.
#[allow(dead_code)]
pub fn parse_routes(hs: &str) -> HashMap<String, String> {
    let pairs = vec![("Kernel.hs", hs)];
    parse_routes_with_locs(&pairs)
        .into_iter()
        .map(|(rust_fn, ri)| (rust_fn, ri.kernel_name))
        .collect()
}

/// Go impl name convention: `Mod_fn` (PascalCase module). Derive from "Mod.fn".
fn go_name(kernel: &str) -> String { kernel.replace('.', "_") }

/// Classify kernels with stdlib-declaration awareness.
///
/// `sky_kernel_decls` is the set of kernel names (e.g. `"Set_insert"`, `"Dict_union"`)
/// that appear as `Ffi.kernel "Name"` declarations inside `sky-stdlib/`.  A kernel in
/// this set is a REAL Rust gap when Go has an impl but Rust does not — any Sky program
/// that calls it routes the call to the runtime.  A kernel absent from this set is
/// either a Go-internal optimisation or a function the stdlib defines as pure Sky; the
/// Rust backend handles it without a runtime kernel, so it is NOT a real gap.
///
/// Parity values:
///   - `ok`             — both backends have an impl.
///   - `go-only`        — Go impl present, Rust absent, sky_decl=true → REAL gap.
///   - `go-kernel-opt`  — Go impl present, Rust absent, sky_decl=false → NOT a gap
///                        (Go optimisation / pure-Sky in stdlib; Rust needs no kernel).
///   - `rust-only`      — Rust impl present, Go absent.
///   - `orphan-route`   — neither backend has an impl.
pub fn reconcile_with_locs(
    routes: &HashMap<String, RouteInfo>,
    go_fns: &HashSet<String>,
    rust_fns: &HashSet<String>,
    sky_kernel_decls: &HashSet<String>,
) -> Vec<Kernel> {
    routes.iter().map(|(rust_fn, ri)| {
        // The `Mod_fn` form is used both for the Go impl lookup and as the
        // sky-stdlib Ffi.kernel name — compute it once per kernel.
        let go_form = go_name(&ri.kernel_name);
        let go = go_fns.contains(&go_form);
        let rust = rust_fns.contains(rust_fn)
            || peephole_alias_present(&ri.kernel_name, rust_fns);
        // The Ffi.kernel name used in sky-stdlib is the Go-convention `Mod_fn` form.
        let sky_decl = sky_kernel_decls.contains(&go_form);
        let parity = match (go, rust) {
            (true, true)   => "ok",
            (true, false)  => if sky_decl { "go-only" } else { "go-kernel-opt" },
            (false, true)  => "rust-only",
            (false, false) => "orphan-route",
        }.to_string();
        let hs_route_loc = if ri.hs_loc.is_empty() { None } else { Some(ri.hs_loc.clone()) };
        Kernel { name: ri.kernel_name.clone(), rust_fn: rust_fn.clone(), go_impl: go, rust_impl: rust, sky_decl, parity, hs_route_loc }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_kernel_routes() {
        let hs = r#"  ("List", "head") -> "list_head"
  ("List", "drop") -> "list_drop""#;
        let routes = parse_routes(hs);
        assert_eq!(routes.get("list_head"), Some(&"List.head".to_string()));
        assert_eq!(routes.get("list_drop"), Some(&"List.drop".to_string()));
    }
    #[test]
    fn parses_go_kernelinfo_shape() {
        // The Go Kernel.hs uses `(("Mod","fn"), KernelInfo ...)`, not `-> "..."`.
        let hs = r#"
    , (("Dict", "union"),         KernelInfo "rt.Dict_union" 2 False)
    , (("List", "head"),          KernelInfo "rt.List_headAny" 1 False)
"#;
        let routes = parse_routes(hs);
        // Go-only kernels are keyed by the conventional snake-case rust_fn.
        assert_eq!(routes.get("dict_union"), Some(&"Dict.union".to_string()));
        assert_eq!(routes.get("list_head"), Some(&"List.head".to_string()));
    }

    #[test]
    fn rust_explicit_wins_over_go_synth() {
        // Both files name List.head; the Rust explicit rust_fn must win and there
        // must be no duplicate kernel keyed by the synthesised name.
        let hs = r#"
    ("List", "head") -> "list_head_real"
    , (("List", "head"), KernelInfo "rt.List_headAny" 1 False)
"#;
        let routes = parse_routes(hs);
        assert_eq!(routes.get("list_head_real"), Some(&"List.head".to_string()));
        // No second List.head row keyed by the conventional name.
        assert_eq!(routes.get("list_head"), None);
    }

    #[test]
    fn flags_missing_rust_impl() {
        // go has List_head + Dict_union; rust has only list_head
        // Dict.union is declared Ffi.kernel in stdlib (sky_decl=true) → real go-only gap.
        let go: std::collections::HashSet<String> = ["List_head","Dict_union"].iter().map(|s|s.to_string()).collect();
        let rust: std::collections::HashSet<String> = ["list_head"].iter().map(|s|s.to_string()).collect();
        let sky: std::collections::HashSet<String> = ["Dict_union"].iter().map(|s|s.to_string()).collect();
        // Build routes via parse_routes_with_locs (the only non-dead path).
        let hs = r#"  ("List", "head") -> "list_head"
  ("Dict", "union") -> "dict_union""#;
        let pairs = vec![("Kernel.hs", hs)];
        let routes = parse_routes_with_locs(&pairs);
        let kernels = reconcile_with_locs(&routes, &go, &rust, &sky);
        let dict = kernels.iter().find(|k| k.name=="Dict.union").unwrap();
        assert_eq!(dict.parity, "go-only"); // routed, Go impl present, Rust impl missing, sky_decl=true
        assert!(dict.sky_decl);
        let head = kernels.iter().find(|k| k.name=="List.head").unwrap();
        assert_eq!(head.parity, "ok");
    }

    #[test]
    fn go_kernel_opt_for_pure_sky_functions() {
        // List.map is defined as pure Sky in stdlib (sky_decl=false) even though Go
        // has a kernel optimisation for it.  It must NOT be flagged as a real Rust gap.
        let go: std::collections::HashSet<String> = ["List_map", "Set_insert"].iter().map(|s|s.to_string()).collect();
        let rust: std::collections::HashSet<String> = HashSet::new(); // neither impl in Rust
        // Only Set.insert is declared Ffi.kernel in sky-stdlib.
        let sky: std::collections::HashSet<String> = ["Set_insert"].iter().map(|s|s.to_string()).collect();
        let hs = r#"  ("List", "map") -> "list_map"
  ("Set", "insert") -> "set_insert""#;
        let pairs = vec![("Kernel.hs", hs)];
        let routes = parse_routes_with_locs(&pairs);
        let kernels = reconcile_with_locs(&routes, &go, &rust, &sky);

        let list_map = kernels.iter().find(|k| k.name == "List.map").unwrap();
        assert_eq!(list_map.parity, "go-kernel-opt",
            "List.map is pure-Sky in stdlib — not a real Rust gap");
        assert!(!list_map.sky_decl);

        let set_insert = kernels.iter().find(|k| k.name == "Set.insert").unwrap();
        assert_eq!(set_insert.parity, "go-only",
            "Set.insert is Ffi.kernel in stdlib — real Rust gap");
        assert!(set_insert.sky_decl);
    }

    #[test]
    fn parse_routes_with_locs_captures_line_numbers() {
        let hs = r#"  ("List", "head") -> "list_head"
  ("Dict", "union") -> "dict_union""#;
        let pairs = vec![("src/Sky/Generate/Rust/Kernel.hs", hs)];
        let routes = parse_routes_with_locs(&pairs);
        let ri = routes.get("list_head").unwrap();
        assert_eq!(ri.kernel_name, "List.head");
        assert!(ri.hs_loc.contains("Kernel.hs:1"), "expected line 1, got: {}", ri.hs_loc);
        let ri2 = routes.get("dict_union").unwrap();
        assert!(ri2.hs_loc.contains("Kernel.hs:2"), "expected line 2, got: {}", ri2.hs_loc);
    }
}
