//! Layer-3 red-team tests for the wasm security gate: the emitted `Cargo.toml`
//! for a `--target wasm` build must contain the browser-glue crates
//! (`wasm-bindgen`, `web-sys`) and MUST NOT contain any server-side dep
//! (`tokio`, `axum`, `sqlx`, `reqwest`) or server-feature flag (`server`,
//! `db`, `live`).
//!
//! These are enforced BY CONSTRUCTION (the `WASM_CARGO_TOML` constant in
//! `project.rs` never includes those deps), but construction alone is not an
//! asserted contract — a future edit that inadvertently adds one would pass
//! every other test. This file makes that contract machine-checked.
//!
//! The emitted `Cargo.toml` is obtained by calling the real backend
//! (`RustBackend::with_target(WasmClient)`) on a minimal body-free program,
//! so the test exercises the same code path `ipe build --target wasm` uses.

use ipe_backend::Backend;
use ipe_backend_rust::RustBackend;
use ipe_intern::Interner;
use ipe_ir::{ModPath, Module, Program, Target};

/// Build the minimal `Program` + the `Interner` used to construct it.
/// Returns both because `RustBackend` borrows the interner for symbol resolution.
fn minimal_wasm_program() -> (Program, Interner) {
    let mut interner = Interner::new();
    #[allow(clippy::expect_used)]
    let main = interner.intern("Main").expect("intern");
    let program = Program {
        modules: vec![Module {
            name: ModPath(vec![main]),
            types: vec![],
            funcs: vec![],
            entry: None,
            records: vec![],
            uses_tea: false,
            uses_server: false,
            uses_http: false,
            uses_config: false,
            uses_compression: false,
            uses_csv: false,
            uses_crypto: false,
            uses_jwt: false,
            uses_ui: false,
            uses_web: false,
            uses_tui: false,
            uses_webview: false,
            uses_css: false,
            uses_auth: false,
            uses_websocket: false,
            uses_email: false,
            uses_env_public: false,
            uses_debug: false,
            uses_ffi: false,
        }],
    };
    (program, interner)
}

fn emit_wasm_cargo_toml() -> String {
    let (program, interner) = minimal_wasm_program();
    // A test precondition: a body-free wasm program must emit, and a failure
    // here should fail the test at its source rather than mask a real breach in
    // a downstream `contains` assertion.
    #[allow(clippy::expect_used)] // emit-must-succeed is the test's precondition
    RustBackend::new(&interner)
        .with_target(Target::WasmClient)
        .emit(&program)
        .expect("emit must succeed for a body-free wasm program")
        .cargo_toml
}

/// The wasm manifest MUST name `wasm-bindgen` — without it the `cdylib`
/// produces no JS glue and the browser cannot load the module.
#[test]
fn wasm_manifest_contains_wasm_bindgen() {
    let cargo_toml = emit_wasm_cargo_toml();
    assert!(
        cargo_toml.contains("wasm-bindgen"),
        "wasm Cargo.toml must declare wasm-bindgen;\ngot:\n{cargo_toml}"
    );
}

/// The wasm manifest MUST name `web-sys` — the DOM / browser API surface the
/// runtime sink (`wasm/mod.rs`) calls.
#[test]
fn wasm_manifest_contains_web_sys() {
    let cargo_toml = emit_wasm_cargo_toml();
    assert!(
        cargo_toml.contains("web-sys"),
        "wasm Cargo.toml must declare web-sys;\ngot:\n{cargo_toml}"
    );
}

/// `tokio` must never appear in the wasm manifest — it requires OS threads and
/// the Mio epoll backend, neither of which exist in `wasm32-unknown-unknown`.
/// Its presence would cause `cargo build --target wasm32-unknown-unknown` to
/// fail, breaking THE SEAL.
#[test]
fn wasm_manifest_excludes_tokio() {
    let cargo_toml = emit_wasm_cargo_toml();
    assert!(
        !cargo_toml.contains("tokio"),
        "wasm Cargo.toml must not contain tokio (SEAL breach);\ngot:\n{cargo_toml}"
    );
}

/// `axum` and `sqlx` depend on tokio and must equally be absent.
#[test]
fn wasm_manifest_excludes_axum_and_sqlx() {
    let cargo_toml = emit_wasm_cargo_toml();
    assert!(
        !cargo_toml.contains("axum"),
        "wasm Cargo.toml must not contain axum;\ngot:\n{cargo_toml}"
    );
    assert!(
        !cargo_toml.contains("sqlx"),
        "wasm Cargo.toml must not contain sqlx;\ngot:\n{cargo_toml}"
    );
}

/// The `server`, `db`, and `live` runtime features must not be activated in
/// the wasm manifest — they enable tokio-bound code paths in the runtime that
/// do not compile to `wasm32-unknown-unknown`.
///
/// The check targets feature-declaration and default-activation patterns
/// (`server = [`, `db = [`, `live = [`, `"server"` / `"db"` / `"live"` in the
/// `default = […]` line) rather than bare name strings, so a comment that
/// mentions the feature name (e.g. a `cfg(any(feature = "live", …))` prose
/// line) does not trigger a false positive.
#[test]
fn wasm_manifest_excludes_server_db_web_features() {
    let cargo_toml = emit_wasm_cargo_toml();
    // A feature is *declared* with `<name> = [` and *activated* as a dep via
    // `features = ["<name>"]` or the default list `default = ["…", "<name>"]`.
    // Neither form must appear for the forbidden names.
    for forbidden in ["server", "db", "live"] {
        let declared = format!("{forbidden} = [");
        // Activated in default or another feature list: `"server"`, `"db"`, `"live"`
        // followed by `]` or `,` (i.e. actually listed as a feature value).
        // We check both forms.
        let activated_bracket = format!("\"{forbidden}\"");
        // The comment prose in WASM_CARGO_TOML contains `feature = "live"` and
        // `feature = "wasm-client"` — those are inside `# …` lines. Strip all
        // comment lines before checking the activated form.
        let toml_no_comments: String = cargo_toml
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            !toml_no_comments.contains(&declared),
            "wasm Cargo.toml must not declare the '{forbidden}' feature;\ngot:\n{cargo_toml}"
        );
        assert!(
            !toml_no_comments.contains(&activated_bracket),
            "wasm Cargo.toml must not activate the '{forbidden}' feature;\ngot:\n{cargo_toml}"
        );
    }
}

/// The emitted crate type must be `cdylib` — a native `bin` type cannot be
/// loaded by the browser's WebAssembly runtime.
#[test]
fn wasm_manifest_is_cdylib() {
    let cargo_toml = emit_wasm_cargo_toml();
    assert!(
        cargo_toml.contains("cdylib"),
        "wasm Cargo.toml must declare crate-type = [\"cdylib\"];\ngot:\n{cargo_toml}"
    );
}

/// The emitted manifest must detach from any ancestor workspace (`[workspace]`
/// section with no members) so `cargo build --target wasm32-unknown-unknown`
/// inside the emitted dir is hermetic even when the dir lives inside a larger
/// workspace tree.
#[test]
fn wasm_manifest_detaches_from_workspace() {
    let cargo_toml = emit_wasm_cargo_toml();
    assert!(
        cargo_toml.contains("[workspace]"),
        "wasm Cargo.toml must include a bare [workspace] section to detach from ancestor \
         workspaces;\ngot:\n{cargo_toml}"
    );
}
