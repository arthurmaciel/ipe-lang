#![forbid(unsafe_code)]
//! SEAL: the emitted Cargo project's `[package] name` matches the sanitized
//! project name from `ipe.toml`, and the renamed crate actually `cargo build`s.
//!
//! Gated on `IPE_E2E=1` so the default `cargo test` stays fast and offline.

use std::path::{Path, PathBuf};
use std::process::Command;

use ipe_backend::Backend;
use ipe_backend_rust::RustBackend;
use ipe_diagnostics::DResult;
use ipe_intern::Interner;
use ipe_ir::{Expr, Func, FuncId, IrType, ModPath, Module, Program};

// Helpers copied from the records test: locate the vendored runtime tree.
fn io_bug(p: &Path, e: &std::io::Error) -> ipe_diagnostics::Diagnostic {
    ipe_diagnostics::Diagnostic::CompilerBug {
        where_: "cargo_name seal",
        detail: format!("{}: {e}", p.display()),
    }
}

fn resolve_runtime() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.ancestors().find_map(|a| {
        let p = a.join("src/runtime/rust/src/ipe_runtime");
        p.is_dir().then_some(p)
    })
}

fn copy_dir(src: &Path, dst: &Path) -> DResult<()> {
    std::fs::create_dir_all(dst).map_err(|e| io_bug(dst, &e))?;
    for entry in std::fs::read_dir(src)
        .map_err(|e| io_bug(src, &e))?
        .flatten()
    {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| io_bug(&from, &e))?;
        }
    }
    Ok(())
}

/// The minimal program: `main = 0` — enough for a cargo build.
fn trivial_program(interner: &mut Interner) -> DResult<Program> {
    let main_mod = interner.intern("Main")?;
    let main_sym = interner.intern("main")?;
    let main_fn = Func {
        id: FuncId::from_raw(0),
        name: main_sym,
        home: ModPath(vec![main_mod]),
        type_params: vec![],
        row_params: vec![],
        params: vec![],
        ret: IrType::Int,
        body: Expr::Int(0),
    };
    Ok(Program {
        imports_unsafe_submodule: false,
        modules: vec![Module {
            name: ModPath(vec![main_mod]),
            types: vec![],
            funcs: vec![main_fn],
            entry: Some(FuncId::from_raw(0)),
            records: vec![],
            uses_tea: false,
            uses_server: false,
            uses_http: false,
            uses_config: false,
            uses_compression: false,
            uses_csv: false,
            uses_cache: false,
            uses_encoding: false,
            uses_regex: false,
            uses_uuid: false,
            uses_random: false,
            uses_log: false,
            uses_decimal: false,
            uses_char_category: false,
            uses_crypto_core: false,
            uses_secret: false,
            uses_json: false,
            uses_crypto: false,
            uses_jwt: false,
            uses_url: false,
            uses_ui: false,
            uses_web: false,
            uses_tui: false,
            uses_webview: false,
            uses_css: false,
            uses_auth: false,
            uses_websocket: false,
            uses_email: false,
            uses_time: false,
            uses_env_public: false,
            uses_async_runtime: false,
            uses_debug: false,
            uses_ffi: false,
        }],
    })
}

/// SEAL: `with_project_name` rewrites the emitted `Cargo.toml`'s `[package]
/// name` to the sanitized project name, and the resulting crate successfully
/// `cargo build`s under that name.
///
/// Covers pathological inputs: uppercase, spaces, leading digit.
#[test]
fn sanitized_name_project_cargo_builds() -> DResult<()> {
    if std::env::var("IPE_E2E").is_err() {
        return Ok(());
    }

    let cases: &[(&str, &str)] = &[
        ("My App", "my-app"),
        ("1game", "app-1game"),
        ("Hello World", "hello-world"),
    ];

    for (raw_name, expected_cargo_name) in cases {
        let mut interner = Interner::new();
        let prog = trivial_program(&mut interner)?;

        let emitted = RustBackend::new(&interner)
            .with_project_name(raw_name)
            .emit(&prog)?;

        // The `[package] name` in the emitted Cargo.toml must match the
        // sanitized form, not the raw ipe.toml value.
        assert!(
            emitted
                .cargo_toml
                .contains(&format!("name = \"{expected_cargo_name}\"")),
            "raw name {raw_name:?}: expected Cargo.toml to contain \
             `name = \"{expected_cargo_name}\"`, got:\n{}",
            emitted.cargo_toml,
        );

        // Full build gate: the renamed crate must cargo build without error.
        let out = std::env::temp_dir().join(format!("ipe_cargo_name_seal_{expected_cargo_name}"));
        let _ = std::fs::remove_dir_all(&out);
        let src = out.join("src");
        std::fs::create_dir_all(&src).map_err(|e| io_bug(&src, &e))?;

        let runtime =
            resolve_runtime().ok_or_else(|| ipe_diagnostics::Diagnostic::CompilerBug {
                where_: "cargo_name seal",
                detail: "could not locate the ipe_runtime tree".to_owned(),
            })?;
        copy_dir(&runtime, &src.join("ipe_runtime"))?;

        let cargo_toml_path = out.join("Cargo.toml");
        std::fs::write(&cargo_toml_path, &emitted.cargo_toml)
            .map_err(|e| io_bug(&cargo_toml_path, &e))?;
        for (rel, contents) in &emitted.files {
            let path = out.join(rel.as_str());
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| io_bug(parent, &e))?;
            }
            std::fs::write(&path, contents).map_err(|e| io_bug(&path, &e))?;
        }

        let status = Command::new("cargo")
            .arg("build")
            .current_dir(&out)
            .env("CARGO_TARGET_DIR", out.join("target"))
            .status();
        let _ = std::fs::remove_dir_all(out.join("target"));

        assert!(
            matches!(&status, Ok(s) if s.success()),
            "sanitized name {expected_cargo_name:?} (from {raw_name:?}): \
             emitted crate must cargo build: {status:?}"
        );
    }

    Ok(())
}
