//! Overflow-wrap end-to-end proof for `Int` `+`/`-`/`*`.
//!
//! The two's-complement wrap contract of `Int` arithmetic must live in the
//! emitted code path (the `ipe_runtime::math::ipe_int_{add,sub,mul}` total
//! helpers), NOT in a `overflow-checks = false` Cargo profile flag. To prove
//! that, these tests build the emitted crate with `RUSTFLAGS=-Coverflow-checks=on`
//! — the exact condition the profile flag was silently relied upon to avoid —
//! and assert the boundary arithmetic wraps and the process exits 0.
//!
//! Under the pre-fix raw-infix emit (`(a + b)` on `i64`), `overflow-checks=on`
//! makes `i64::MAX + 1` panic ("attempt to add with overflow", exit 101); these
//! tests therefore FAIL on that code and PASS only once the wrapping helpers are
//! on the emit path — the definitive proof the class is closed independently of
//! any manifest flag.
//!
//! Gated on `IPE_E2E=1` so the default `cargo test` stays fast and offline.

mod seal_e2e;

use ipe_backend::Backend;
use ipe_backend_rust::RustBackend;
use ipe_diagnostics::DResult;
use ipe_intern::Interner;
use ipe_ir::{
    BinOp, CallPin, Callee, Expr, Func, FuncId, IrType, KernelFn, ModPath, Module, OnFormKind,
    Program,
};

/// `main = Io.println (String.fromInt (lhs <op> rhs))` at `FuncId(0)`.
fn wrap_main(interner: &mut Interner, op: BinOp, lhs: i64, rhs: i64) -> DResult<Func> {
    let main = interner.intern("main")?;
    Ok(Func {
        id: FuncId::from_raw(0),
        name: main,
        home: ModPath(vec![]),
        type_params: vec![],
        row_params: vec![],
        params: vec![],
        ret: IrType::Task(Box::new(IrType::Unit)),
        body: Expr::Call {
            callee: Callee::Kernel(KernelFn::IoPrintln),
            args: vec![Expr::Call {
                callee: Callee::Kernel(KernelFn::StringFromInt),
                args: vec![Expr::BinOp {
                    op,
                    lhs: Box::new(Expr::Int(lhs)),
                    rhs: Box::new(Expr::Int(rhs)),
                }],
                pin: CallPin::None,
                on_form: OnFormKind::NotForm,
            }],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        },
    })
}

fn wrap_program(interner: &mut Interner, op: BinOp, lhs: i64, rhs: i64) -> DResult<Program> {
    let main_mod = interner.intern("Main")?;
    let main_fn = wrap_main(interner, op, lhs, rhs)?;
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
            uses_debug: false,
            uses_ffi: false,
            uses_async_runtime: false,
        }],
    })
}

/// Emit the program, vendor the runtime beside it, `cargo build` it with
/// `overflow-checks` FORCED ON, run the binary, and assert stdout == `expected`
/// with a clean (exit-0) termination.
fn build_overflow_checked_and_assert(
    interner: &Interner,
    prog: &Program,
    slot: &str,
    expected: &str,
) -> DResult<()> {
    if std::env::var("IPE_E2E").is_err() {
        return Ok(());
    }
    let Some(runtime) = seal_e2e::resolve_runtime() else {
        return Ok(());
    };

    let emitted = RustBackend::new(interner).emit(prog)?;

    let out = std::env::temp_dir().join(slot);
    let _ = std::fs::remove_dir_all(&out);
    let src = out.join("src");
    std::fs::create_dir_all(&src).map_err(|e| seal_e2e::io_bug(&src, &e))?;
    seal_e2e::copy_dir(&runtime, &src.join("ipe_runtime"))?;

    let cargo_toml = out.join("Cargo.toml");
    std::fs::write(&cargo_toml, &emitted.cargo_toml)
        .map_err(|e| seal_e2e::io_bug(&cargo_toml, &e))?;
    for (rel, contents) in &emitted.files {
        let path = out.join(rel.as_str());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| seal_e2e::io_bug(parent, &e))?;
        }
        std::fs::write(&path, contents).map_err(|e| seal_e2e::io_bug(&path, &e))?;
    }

    // Force `overflow-checks=on` regardless of the emitted crate's dev profile.
    // Under raw-infix i64 arithmetic this makes the boundary op panic on build-
    // then-run; the wrapping helpers keep it total.
    let status = std::process::Command::new("cargo")
        .arg("build")
        .current_dir(&out)
        .env("CARGO_TARGET_DIR", out.join("target"))
        .env("RUSTFLAGS", "-Coverflow-checks=on")
        .status();
    assert!(
        matches!(&status, Ok(s) if s.success()),
        "emitted int-wrap project must build under overflow-checks=on: {status:?}"
    );

    let bin = out.join("target").join("debug").join("ipe-app");
    let output = std::process::Command::new(&bin)
        .output()
        .map_err(|e| seal_e2e::io_bug(&bin, &e))?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected,
        "int arithmetic must wrap (two's-complement), not panic, under overflow-checks=on"
    );
    assert!(
        output.status.success(),
        "exit 0 under overflow-checks=on — wrap semantics live in the emitted code, \
         not the Cargo profile flag; got {:?}",
        output.status.code()
    );
    let _ = std::fs::remove_dir_all(out.join("target"));
    Ok(())
}

fn run_wrap(op: BinOp, lhs: i64, rhs: i64, slot: &str, expected: &str) -> DResult<()> {
    let mut interner = Interner::new();
    let prog = wrap_program(&mut interner, op, lhs, rhs)?;
    build_overflow_checked_and_assert(&interner, &prog, slot, expected)
}

/// `i64::MAX + 1` wraps to `i64::MIN` (no "attempt to add with overflow").
#[test]
fn end_to_end_int_add_wraps_at_max() -> DResult<()> {
    run_wrap(
        BinOp::IntAdd,
        i64::MAX,
        1,
        "ipe_int_wrap_add_e2e",
        "-9223372036854775808\n",
    )
}

/// `i64::MIN - 1` wraps to `i64::MAX`.
#[test]
fn end_to_end_int_sub_wraps_at_min() -> DResult<()> {
    run_wrap(
        BinOp::IntSub,
        i64::MIN,
        1,
        "ipe_int_wrap_sub_e2e",
        "9223372036854775807\n",
    )
}

/// `i64::MAX * 2` wraps to `-2`.
#[test]
fn end_to_end_int_mul_wraps_on_overflow() -> DResult<()> {
    run_wrap(BinOp::IntMul, i64::MAX, 2, "ipe_int_wrap_mul_e2e", "-2\n")
}
