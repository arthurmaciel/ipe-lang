//! The injectable Ipê interface module — the consumer-side seed artifact.
//!
//! For each bound crate the driver injects one ordinary, fully-annotated Ipê
//! module (`module Rust.Semver exposing (…)`) whose value bodies are
//! `Ffi.binding "<wrapper_fn_ident>" a0 …` forwarders. FFI signatures thus
//! flow through the SAME annotation → `Ty` path every user annotation takes:
//! there is no second, hand-maintained scheme table to drift against
//! (the kernel-registry design's OPEN DECISION 1, resolved by construction).
//!
//! Inclusion is gated fail-closed: a function reaches the interface only when
//! its wrapper region actually exists in `_bindings.rs`, its signature's
//! opaque foreign types all resolve to unambiguous Rust paths, and no foreign
//! type shadows an Ipê builtin head. Anything else is skipped with a recorded
//! reason (over-drop, never an under-bind that `cargo` rejects).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::emit::{ipe_builtin_heads, opaque_names_in, wrapper_ipe_signature};
use crate::pkginfo::{FnInfo, PkgInfo};

/// Ipê keywords that can never be a binding name in the generated module.
const IPE_KEYWORDS: &[&str] = &[
    "module", "import", "exposing", "type", "alias", "let", "in", "case", "of", "if", "then",
    "else", "as", "port",
];

/// One binding included in the interface module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceBinding {
    /// The Ipê-visible name (= `wrapper_ref_name`, the `kernel.json` key).
    pub ref_name: String,
    /// The `_bindings.rs` wrapper `pub fn` identifier the body forwards to.
    pub wrapper_ident: String,
    /// Ipê-side arity (unit param for a zero-arg foreign fn).
    pub arity: usize,
    /// The full Ipê HM signature string.
    pub sig: String,
}

/// A binding excluded from the interface, with the reason — surfaced in the
/// coverage report so an over-drop is visible, never silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedBinding {
    /// The `wrapper_ref_name` of the skipped function.
    pub ref_name: String,
    /// Why it was excluded.
    pub reason: String,
}

/// The complete consumer-side view of one bound crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateInterface {
    /// Ipê module qualifier, e.g. `Rust.Semver`.
    pub module_name: String,
    /// Kernel-name prefix, e.g. `Rust_Semver`.
    pub kernel_name: String,
    /// The injectable Ipê module source.
    pub source: String,
    /// Opaque foreign type name → absolute Rust path (`Version` →
    /// `::semver::Version`), for backend type rendering.
    pub opaque_types: BTreeMap<String, String>,
    /// The included bindings.
    pub bindings: Vec<InterfaceBinding>,
    /// The excluded bindings, with reasons.
    pub skipped: Vec<SkippedBinding>,
}

/// Collect `Name → ::absolute::path` for every nominal foreign type mentioned
/// in the package's Rust type strings.
///
/// A base name claimed by two DIFFERENT paths is poisoned (removed): two
/// distinct foreign types would otherwise unify nominally on the Ipê side.
/// Poisoned names travel back so the per-fn gate can drop their users.
fn foreign_path_map(pkg: &PkgInfo) -> (BTreeMap<String, String>, BTreeSet<String>) {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    let mut poisoned: BTreeSet<String> = BTreeSet::new();
    let mut visit = |raw: &str| {
        for (base, path) in path_tokens(raw) {
            match map.get(&base) {
                Some(prev) if *prev != path => {
                    poisoned.insert(base);
                }
                Some(_) => {}
                None => {
                    map.insert(base, path);
                }
            }
        }
    };
    for f in pkg.fns() {
        visit(f.recv_rust_type());
        for p in f.params().iter().chain(f.results().iter()) {
            visit(&p.rust_type);
            visit(&p.foreign_ty);
        }
    }
    for name in &poisoned {
        map.remove(name);
    }
    (map, poisoned)
}

/// Extract every `seg::…::Base` path token from a Rust type string, returning
/// `(Base, ::seg::…::Base)` pairs. Generic arguments split into their own
/// tokens; a bare identifier (no `::`) carries no path and is skipped.
fn path_tokens(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut Vec<(String, String)>| {
        if token.contains("::") {
            let normalized = if token.starts_with("::") {
                token.clone()
            } else {
                format!("::{token}")
            };
            if let Some(base) = normalized.rsplit("::").next()
                && !base.is_empty()
                && base.chars().next().is_some_and(char::is_uppercase)
            {
                out.push((base.to_owned(), normalized));
            }
        }
        token.clear();
    };
    for c in raw.chars() {
        if c.is_alphanumeric() || c == '_' || c == ':' {
            token.push(c);
        } else {
            flush(&mut token, &mut out);
        }
    }
    flush(&mut token, &mut out);
    out
}

/// Collect the foreign NOMINAL base names that reach the Ipê signature from
/// one foreign type string. Mirrors [`crate::emit::foreign_to_ipe`]'s
/// structure: containers recurse, a `Result`'s error arm folds into the typed
/// `Error` at the boundary (so it never reaches the signature and is NOT
/// collected), scalars/strings map to builtins and are not nominal.
fn foreign_nominal_bases(t: &str, out: &mut BTreeSet<String>) {
    let t = t.trim();
    let t = t.strip_prefix('&').unwrap_or(t).trim();
    let t = t.strip_prefix("mut ").unwrap_or(t).trim();
    // Rust containers.
    for ctor in ["Option", "Vec"] {
        if let Some(rest) = t.strip_prefix(ctor)
            && let Some(inner) = rest.trim().strip_prefix('<')
            && let Some(inner) = inner.strip_suffix('>')
        {
            foreign_nominal_bases(inner, out);
            return;
        }
    }
    if let Some(rest) = t.strip_prefix("Result")
        && let Some(inner) = rest.trim().strip_prefix('<')
        && let Some(inner) = inner.strip_suffix('>')
    {
        // Ok arm only — the error arm folds into the typed `Error`.
        let ok = inner.split(',').next().unwrap_or(inner);
        foreign_nominal_bases(ok, out);
        return;
    }
    // Ipê containers — the inspector's `type` field carries already-mapped
    // Ipê spellings for scalar-ish positions.
    for ctor in ["Maybe ", "List "] {
        if let Some(inner) = t.strip_prefix(ctor) {
            foreign_nominal_bases(inner.trim_start_matches('(').trim_end_matches(')'), out);
            return;
        }
    }
    if let Some(inner) = t.strip_prefix("Result Error ") {
        foreign_nominal_bases(inner.trim_start_matches('(').trim_end_matches(')'), out);
        return;
    }
    match t {
        // Rust scalar/string leaves plus their Ipê spellings — none nominal.
        "str" | "String" | "OsStr" | "OsString" | "Path" | "PathBuf" | "CStr" | "CString"
        | "bool" | "char" | "()" | "i8" | "i16" | "i32" | "i64" | "i128" | "u8" | "u16" | "u32"
        | "u64" | "u128" | "isize" | "usize" | "f32" | "f64" | "error" | "" | "Int" | "Float"
        | "Bool" | "Char" => {}
        other => {
            let base = other.rsplit_once("::").map_or(other, |(_, l)| l);
            let base = base.split('<').next().unwrap_or(base).trim();
            if base.chars().next().is_some_and(char::is_uppercase) {
                out.insert(base.to_owned());
            }
        }
    }
}

/// `true` when the fn's Ipê-visible signature would carry a foreign nominal
/// whose base name shadows an Ipê builtin head (`semver::Error` vs the
/// builtin `Error`) — such a signature silently means the BUILTIN on the Ipê
/// side while the wrapper expects the foreign type, an E0308 `cargo` would
/// reject.
fn shadows_builtin_head(f: &FnInfo) -> bool {
    let mut bases = BTreeSet::new();
    for p in f.params().iter().chain(f.results().iter()) {
        // `rust_type` is the Rust-side truth when the inspector supplied it;
        // `foreign_ty` may already carry the mapped Ipê spelling.
        let src = if p.rust_type.is_empty() {
            &p.foreign_ty
        } else {
            &p.rust_type
        };
        foreign_nominal_bases(src, &mut bases);
    }
    if f.recv_type().chars().next().is_some_and(char::is_uppercase) {
        bases.insert(f.recv_type().to_owned());
    }
    bases
        .iter()
        .any(|b| ipe_builtin_heads().contains(&b.as_str()))
}

/// `true` when `name` is a well-formed Ipê value identifier the generated
/// module may bind: lowercase-led, alphanumeric/underscore, not a keyword.
fn valid_ipe_value_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !IPE_KEYWORDS.contains(&name)
}

/// Build the consumer-side interface for one validated package.
#[must_use]
pub fn crate_interface(pkg: &PkgInfo) -> CrateInterface {
    let module_name = crate::naming::rust_module_name(pkg.pkg_path());
    let kernel_name = crate::naming::rust_kernel_name(pkg.pkg_path());
    let survivors = crate::bindings::surviving_ref_names(pkg);
    let (path_map, poisoned) = foreign_path_map(pkg);

    let mut bindings: Vec<InterfaceBinding> = Vec::new();
    let mut skipped: Vec<SkippedBinding> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut used_opaques: BTreeSet<String> = BTreeSet::new();

    for f in pkg.fns() {
        let ref_name = f.wrapper_ref_name();
        let skip = |reason: &str, skipped: &mut Vec<SkippedBinding>| {
            skipped.push(SkippedBinding {
                ref_name: ref_name.clone(),
                reason: reason.to_owned(),
            });
        };
        if ref_name.starts_with('_') {
            continue; // internal probe artifact, not a binding
        }
        if !valid_ipe_value_name(&ref_name) {
            skip("name is not a legal Ipê identifier", &mut skipped);
            continue;
        }
        if f.generic().is_some_and(|g| !g.params.is_empty()) {
            skip(
                "parametric generic — monomorphised instances are not consumer-wired yet",
                &mut skipped,
            );
            continue;
        }
        if !survivors.contains(&ref_name) {
            skip("no wrapper region in _bindings.rs", &mut skipped);
            continue;
        }
        if shadows_builtin_head(f) {
            skip("a foreign type shadows an Ipê builtin head", &mut skipped);
            continue;
        }
        let sig = wrapper_ipe_signature(f);
        let mut opaques = BTreeSet::new();
        opaque_names_in(&sig, &mut opaques);
        if let Some(bad) = opaques.iter().find(|n| poisoned.contains(*n)) {
            skip(
                &format!("foreign type `{bad}` is claimed by two distinct Rust paths"),
                &mut skipped,
            );
            continue;
        }
        if let Some(bad) = opaques.iter().find(|n| !path_map.contains_key(*n)) {
            skip(
                &format!("foreign type `{bad}` has no resolvable Rust path"),
                &mut skipped,
            );
            continue;
        }
        if !seen.insert(ref_name.clone()) {
            skip(
                "duplicate binding name — first occurrence kept",
                &mut skipped,
            );
            continue;
        }
        used_opaques.extend(opaques);
        bindings.push(InterfaceBinding {
            wrapper_ident: crate::naming::wrapper_fn_ident(&kernel_name, &ref_name),
            arity: f.params().len().max(1),
            sig,
            ref_name,
        });
    }

    let opaque_types: BTreeMap<String, String> = used_opaques
        .iter()
        .filter_map(|n| path_map.get(n).map(|p| (n.clone(), p.clone())))
        .collect();

    let source = render_module(&module_name, &opaque_types, &bindings);
    CrateInterface {
        module_name,
        kernel_name,
        source,
        opaque_types,
        bindings,
        skipped,
    }
}

/// Render the injectable module text.
///
/// Opaque types are exported WITHOUT `(..)` so their placeholder constructor
/// never escapes the module; the lowerer additionally fails closed on any
/// constructor use of a foreign union.
fn render_module(
    module_name: &str,
    opaque_types: &BTreeMap<String, String>,
    bindings: &[InterfaceBinding],
) -> String {
    let mut exports: Vec<String> = opaque_types.keys().cloned().collect();
    exports.extend(bindings.iter().map(|b| b.ref_name.clone()));
    let mut out = format!("module {module_name} exposing ({})\n", exports.join(", "));
    for name in opaque_types.keys() {
        // Writing into a String is infallible.
        let _ = write!(out, "\ntype {name} = {name}\n");
    }
    for b in bindings {
        let args: Vec<String> = (0..b.arity).map(crate::naming::arg_name).collect();
        let args_joined = args.join(" ");
        let _ = write!(
            out,
            "\n{} : {}\n{} {} =\n    Ffi.binding \"{}\" {}\n",
            b.ref_name, b.sig, b.ref_name, args_joined, b.wrapper_ident, args_joined
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkginfo::PkgInfo;

    fn pkg() -> PkgInfo {
        let doc = serde_json::json!({
            "pkg": "semver",
            "name": "semver",
            "version": "1.0.26",
            "functions": [
                {
                    "name": "parse",
                    "params": [{"name": "text", "type": "&str", "ipeType": "String"}],
                    "results": [{"name": "", "type": "Result<Version, Error>",
                                 "rustType": "Result<semver::Version, semver::Error>"}],
                    "effect": "fallible"
                },
                {
                    "name": "major_field",
                    "params": [{"name": "self", "type": "&Version", "ipeType": "Version",
                                "rustType": "semver::Version"}],
                    "results": [{"name": "", "type": "u64", "rustType": "u64"}],
                    "effect": "pure",
                    "recvType": "Version",
                    "recvRustType": "semver::Version",
                    "methodName": "major",
                    "isField": true
                },
                {
                    "name": "explain",
                    "params": [{"name": "self", "type": "&Error", "ipeType": "Error",
                                "rustType": "semver::Error"}],
                    "results": [{"name": "", "type": "String"}],
                    "effect": "pure",
                    "recvType": "Error",
                    "recvRustType": "semver::Error"
                }
            ],
            "errors": [],
            "transitiveDeps": [
                {"ident": "semver", "name": "semver", "version": "1.0.26"}
            ]
        });
        PkgInfo::decode_json(&doc.to_string()).expect("decodes")
    }

    #[test]
    fn interface_includes_survivors_and_maps_opaque_paths() {
        let iface = crate_interface(&pkg());
        assert_eq!(iface.module_name, "Rust.Semver");
        assert_eq!(iface.kernel_name, "Rust_Semver");
        let names: Vec<&str> = iface.bindings.iter().map(|b| b.ref_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["parse", "major_field_from_version"],
            "{:?}",
            iface.skipped
        );
        assert_eq!(
            iface.opaque_types.get("Version").map(String::as_str),
            Some("::semver::Version")
        );
        // `Error` is a builtin head — never an opaque decl, and `explain`
        // (whose receiver is the foreign `semver::Error`) is dropped.
        assert!(!iface.opaque_types.contains_key("Error"));
        assert!(
            iface
                .skipped
                .iter()
                .any(|s| s.ref_name == "explain_from_error"
                    && s.reason.contains("shadows an Ipê builtin head")),
            "{:?}",
            iface.skipped
        );
    }

    #[test]
    fn rendered_module_is_annotated_forwarders() {
        let iface = crate_interface(&pkg());
        let src = &iface.source;
        assert!(
            src.starts_with(
                "module Rust.Semver exposing (Version, parse, major_field_from_version)"
            ),
            "{src}"
        );
        assert!(src.contains("\ntype Version = Version\n"), "{src}");
        assert!(
            src.contains("\nparse : String -> Result Error Version\nparse arg0 =\n    Ffi.binding \"semver_parse\" arg0\n"),
            "{src}"
        );
        assert!(
            src.contains(
                "\nmajor_field_from_version : Version -> Int\nmajor_field_from_version arg0 =\n    Ffi.binding \"semver_major_field_from_version\" arg0\n"
            ),
            "{src}"
        );
    }

    #[test]
    fn conflicting_paths_poison_the_base_name() {
        let (map, poisoned) = foreign_path_map(&pkg());
        assert_eq!(
            map.get("Version").map(String::as_str),
            Some("::semver::Version")
        );
        assert!(poisoned.is_empty());
        assert_eq!(
            path_tokens("HashMap<foo::Bar, baz::Bar>"),
            vec![
                ("Bar".to_owned(), "::foo::Bar".to_owned()),
                ("Bar".to_owned(), "::baz::Bar".to_owned()),
            ]
        );
    }
}
