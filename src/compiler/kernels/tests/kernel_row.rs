//! Kernel-row invariants: the [`KernelDef`] descriptor must agree with the
//! authoritative per-variant methods it projects, and every kernel whose emitted
//! symbol lives in a declared runtime module must have that symbol actually
//! defined in the vendored runtime source.
//!
//! The second test is the fast pre-cargo tripwire for the exit-0-then-cargo-fail
//! class: a kernel that declares a `runtime_module` but whose `runtime_fn` names
//! no `pub` symbol in that module would let `ipe` accept a program whose emitted
//! crate then fails `cargo build` (E0425/E0412). This scan catches that in the
//! normal test path, before any downstream build.

use std::path::PathBuf;

use ipe_kernels::{RuntimeModule, StdlibKernel};

/// `def()` is a pure projection: every field must equal what the authoritative
/// method it delegates to returns.
#[test]
fn def_projects_the_authoritative_methods() {
    for &k in StdlibKernel::ALL {
        let def = k.def();
        let decl = k.decl();
        assert_eq!(def.qualifier, decl.qualifier, "qualifier drift for {k:?}");
        assert_eq!(def.name, decl.name, "name drift for {k:?}");
        assert_eq!(def.arity, decl.arity, "arity drift for {k:?}");
        assert_eq!(def.class, decl.class, "class drift for {k:?}");
        assert_eq!(def.runtime_fn, decl.emit, "runtime_fn drift for {k:?}");
        assert_eq!(def.capability, k.capability(), "capability drift for {k:?}");
        assert_eq!(
            def.runtime_module,
            k.required_runtime_module(),
            "runtime_module drift for {k:?}"
        );
        assert_eq!(def.scheme.0, k, "scheme key must point back at the variant");
    }
}

/// The vendored runtime source root, relative to this crate's manifest.
fn runtime_src_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/rust/src")
}

/// The set of source files that make up a conditionally-vendored
/// [`RuntimeModule`]. A `runtime_fn` declared to live in that module must be a
/// `pub` symbol defined in one of these files.
///
/// `Web` is the `web` feature-module (`web/*`, including `web::pubsub`).
/// `Server` is the server feature-module set (`server` + `server_stream` +
/// `http_stream`).
fn module_source_files(module: RuntimeModule) -> Vec<PathBuf> {
    let root = runtime_src_root();
    let rel: &[&str] = match module {
        RuntimeModule::Web => &["web/pubsub.rs", "web/mod.rs"],
        RuntimeModule::Server => &["server.rs", "server_stream.rs", "http_stream.rs"],
    };
    rel.iter().map(|r| root.join(r)).collect()
}

/// Whether `src` defines a `pub` item named `symbol` (fn, struct, enum, const,
/// static, or type alias). A source-symbol scan, not a build: it looks for a
/// `pub`-prefixed item declaration whose name is exactly `symbol`.
fn defines_pub_symbol(src: &str, symbol: &str) -> bool {
    const KINDS: &[&str] = &[
        "fn ", "struct ", "enum ", "const ", "static ", "type ", "trait ",
    ];
    src.lines().any(|line| {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("pub") else {
            return false;
        };
        // Skip visibility qualifiers like `pub(crate)` and the following space.
        let rest = rest.trim_start_matches(|c: char| c == '(' || c == ')' || c.is_alphabetic());
        let rest = rest.trim_start();
        KINDS.iter().any(|kind| {
            rest.strip_prefix(kind).is_some_and(|after| {
                // The item name is the leading identifier of `after`; it must be
                // exactly `symbol` (followed by a non-identifier boundary such as
                // `(`, `<`, `:`, `=`, `;`, or whitespace).
                after.strip_prefix(symbol).is_some_and(|tail| {
                    tail.chars()
                        .next()
                        .is_none_or(|c| !c.is_alphanumeric() && c != '_')
                })
            })
        })
    })
}

/// Every kernel that declares a `runtime_module` must emit a symbol that the
/// vendored runtime source actually defines in that module. A `runtime_fn` with
/// no defining `pub` symbol fails here, before the emitted crate would fail
/// `cargo build` — the fast tripwire for the module-set SEAL breach class.
#[test]
fn every_declared_runtime_symbol_is_defined_in_its_module() {
    let mut undefined: Vec<String> = Vec::new();
    for &k in StdlibKernel::ALL {
        let def = k.def();
        let Some(module) = def.runtime_module else {
            continue;
        };
        let files = module_source_files(module);
        let found = files.iter().any(|path| {
            std::fs::read_to_string(path).is_ok_and(|src| defines_pub_symbol(&src, def.runtime_fn))
        });
        if !found {
            undefined.push(format!(
                "{k:?}: runtime_fn `{}` declared in {module:?} but no `pub` symbol defines it",
                def.runtime_fn
            ));
        }
    }
    assert!(
        undefined.is_empty(),
        "kernels with an undefined runtime symbol in their declared module:\n{}",
        undefined.join("\n")
    );
}
