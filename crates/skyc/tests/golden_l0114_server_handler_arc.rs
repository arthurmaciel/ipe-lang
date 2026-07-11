//! Regression guard for the `wants_arc_ctor` structural-dispatch fix.
//!
//! ## The bug this pins (2026-07-11 review finding)
//!
//! `emit_lambda` originally chose the smart-pointer constructor by testing the
//! RENDERED type string with `typed.starts_with("Arc<")`. `render_type` emits a
//! `ServerHandler<E>` slot as the type-ALIAS name `"ServerHandler<SkyError>"`,
//! NOT the expanded `"Arc<dyn Fn…>"` — so the string test misclassified every
//! inline `Server.post path (\req -> …)` handler lambda as `Box`, emitting
//! `Box::new(move |req: ServerRequest| …)` into an `Arc<dyn Fn>` field. `skyc`
//! still exited 0, but the emitted crate failed `cargo build` with E0308
//! (`expected Arc…, found Box…`) — a break of THE SEAL (skyc exit-0 must imply
//! the emitted Rust cargo-builds). The fix routes both `emit_lambda` and
//! `emit_func_value` through the shared structural `wants_arc_ctor` helper that
//! matches on the `IrType` shape, never the rendered string.
//!
//! ## Why this test is NOT `SKY_E2E`-gated
//!
//! The full cargo-build proof lives in `server_e2e.rs` (`SKY_E2E`), but that
//! suite does not run in the default `cargo nextest` gate — which is exactly
//! how the regression reached `master` green. This test needs only the emitted
//! Rust text (no cargo build), so it runs in the DEFAULT gate and can never be
//! silently regressed again. It reuses the existing
//! `m6_server_request_accessors` fixture, whose sole lambda is the inline
//! `Server.post "/introspect/:tag" (\req -> …)` handler.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn move_closure_lines(src: &str) -> String {
    src.lines()
        .filter(|l| l.contains("::new(move |") || l.contains("ServerHandler"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The inline `Server.post path (\req -> …)` handler lambda must be boxed with
/// `Arc::new` (the `ServerHandler` field is `Arc<dyn Fn + Send + Sync>`), never
/// `Box::new`. The fixture contains exactly one lambda — the handler — so the
/// emitted crate must contain an `Arc::new(move |` and no `Box::new(move |`.
#[test]
fn server_handler_lambda_boxes_with_arc_not_box() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("m6_server_request_accessors")
        .join("Main.sky");
    let out = std::env::temp_dir().join("skyc_l0114_server_handler_arc");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        // In an environment where the runtime dir can't be resolved the emit
        // step can't run; skip rather than false-fail (mirrors the byte
        // goldens' resolve dependency).
        return;
    };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for the server-handler fixture: {:?}",
        built.err()
    );

    let main_rs = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted src/main.rs must exist");

    assert!(
        main_rs.contains("Arc::new(move |"),
        "handler lambda must be boxed with `Arc::new` (ServerHandler is \
         Arc<dyn Fn>). wants_arc_ctor must dispatch on the IrType structure, \
         not the rendered `ServerHandler<E>` alias string.\n\
         --- move-closure / ServerHandler lines ---\n{}",
        move_closure_lines(&main_rs)
    );
    assert!(
        !main_rs.contains("Box::new(move |"),
        "the fixture's only lambda is the handler — any `Box::new(move |` means \
         it was misboxed into the Arc<dyn Fn> ServerHandler slot (E0308 SEAL \
         break).\n--- move-closure / ServerHandler lines ---\n{}",
        move_closure_lines(&main_rs)
    );
}

fn ws_onerror_lines(src: &str) -> String {
    src.lines()
        .filter(|l| l.contains("on_error") || l.contains("main_on_error"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The WS `onError` callback — `Fn(WsHandle, Error) -> SkyTask<()>` — fills an
/// `Arc<dyn Fn(WsHandle, E)>` setter slot, so it must box with `Arc::new`. Its
/// second param is the error type (NOT `String` like `onMessage`), the shape
/// that `wants_arc_ctor` / `render_type` originally omitted → generic
/// `Box<dyn Fn>` → skyc-0-then-cargo-fail E0308. Emit-only guard, default gate.
#[test]
fn ws_on_error_callback_boxes_with_arc_not_box() {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("l0114_ws_onerror")
        .join("Main.sky");
    let out = std::env::temp_dir().join("skyc_l0114_ws_onerror_arc");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };

    let built = skyc::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "skyc build must succeed for the WS onError fixture: {:?}",
        built.err()
    );

    let main_rs = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted src/main.rs must exist");

    assert!(
        main_rs.contains("Arc::new(main_on_error)"),
        "onError callback must box with `Arc::new` (setter takes Arc<dyn Fn>).\n\
         --- on_error lines ---\n{}",
        ws_onerror_lines(&main_rs)
    );
    assert!(
        !main_rs.contains("Box::new(main_on_error)"),
        "onError boxed with `Box::new` into the Arc<dyn Fn> setter slot = E0308 \
         SEAL break.\n--- on_error lines ---\n{}",
        ws_onerror_lines(&main_rs)
    );
}
