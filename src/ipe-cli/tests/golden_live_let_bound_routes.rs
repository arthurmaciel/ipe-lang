//! SKY-I0001 regression — `List LiveRoute` via a let-bound top-level binding.
//!
//! ## Background
//!
//! `Live.route` is typed `String -> page -> LiveRoute` (opaque).  A top-level
//! `routeTable` binding whose elements are `Live.route …` calls therefore has
//! inferred type `List LiveRoute`.  The `routes = routeTable` field of the
//! `Live.app` cfg must accept that type (T3 open-record scheme), and the emitter
//! must lower the `routes` expression as a normal Expr ref — NOT assume it is an
//! inline `[Expr::Ctor, …]` literal.
//!
//! ## What is tested
//!
//! * `routeTable` (top-level, not inlined in the cfg) type-checks as `List LiveRoute`.
//! * The T3 open-record scheme (`routes : List LiveRoute` row field) accepts a
//!   let-bound reference, not only an inline literal.
//! * `emit_live_app_inner` lowers `routes = routeTable` (an `Expr::Var` ref) correctly.
//! * `routed_page_field` detects the `page` field and emits `live_app_routed`.
//! * No ICE — the full skyc pipeline (parse → canon → types → lower → emit) exits Ok.
//!
//! This is a pure skyc-pipeline check (no cargo build / runtime binary required)
//! and runs without `SKY_E2E=1`.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Run the skyc pipeline on `tests/golden/live_let_bound_routes/Main.sky`.
/// Returns `None` when the embedded runtime is unavailable (skip).
fn run_skyc() -> Option<Result<(), skyc::CliError>> {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join("live_let_bound_routes")
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m7_live_let_bound_routes_emit");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return None;
    };
    Some(skyc::build(&entry, &out, &runtime))
}

/// SKY-I0001 regression: a `Live.app` whose `routes` field references a
/// top-level `routeTable` binding (type `List LiveRoute`) MUST compile
/// through the full skyc pipeline without an ICE.
///
/// Pre-regression: the emitter assumed all `routes` elements were inlined
/// `Expr::Ctor` nodes and panicked (ICE) on a let-bound reference.
/// Post-fix (T5): the `routes` field is lowered as a normal expression;
/// individual route-closure builders are only inspected at the call sites of
/// `Live.route`, not at the list-collection level.
#[test]
fn live_let_bound_routes_compiles_no_ice() {
    let Some(result) = run_skyc() else {
        return;
    };
    assert!(
        result.is_ok(),
        "SKY-I0001 regression: let-bound routeTable must compile, got: {:?}",
        result.err(),
    );
}

// ── round-4 hole 1: the let-bound golden must also CARGO-build ───────
//
// `routeTable`'s top-level fn signature renders the binding's inferred type
// `List (LiveRoute Page)`. Pre-round-4 `IrType::LiveRoute` rendered a bare
// `ipe_runtime::live::route::Route` — but the runtime `Route<Page>` has NO
// default type parameter, so THIS golden itself was skyc-0 then cargo-fail
// (E0107) at the `routeTable` signature. Post-fix the signature renders
// `Vec<Route<MainPage>>`.

/// The emitted `main.rs` must render the page-parametrised `Route<MainPage>`
/// in `routeTable`'s signature. Compile-only — always runs.
#[test]
fn live_let_bound_routes_renders_route_page() {
    let Some(result) = run_skyc() else {
        return;
    };
    assert!(result.is_ok(), "must compile: {:?}", result.err());
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m7_live_let_bound_routes_emit");
    let main_rs = std::fs::read_to_string(out.join("src").join("main.rs"))
        .expect("emitted main.rs must exist");
    assert!(
        main_rs.contains("route::Route<MainPage>"),
        "#108 hole 1: the let-bound route table's signature must render \
         `Route<MainPage>` (bare `Route` is the E0107 cargo failure)",
    );
}

/// `SKY_E2E` tier: the emitted project must CARGO-build. Isolated
/// `CARGO_TARGET_DIR` per fixture — never shared (fingerprint reuse can mask
/// an E0107/E0308 as a false pass).
#[test]
fn live_let_bound_routes_cargo_builds() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let Some(result) = run_skyc() else {
        return;
    };
    assert!(result.is_ok(), "must compile: {:?}", result.err());
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m7_live_let_bound_routes_emit");
    let target = std::env::temp_dir().join("r4").join("m7_let_bound_routes");
    let build = std::process::Command::new("cargo")
        .arg("build")
        .env("CARGO_TARGET_DIR", &target)
        .current_dir(&out)
        .output()
        .expect("cargo must spawn");
    assert!(
        build.status.success(),
        "#108 hole 1: the let-bound routes golden must cargo-build \
         (pre-fix: E0107 at the `routeTable` fn signature)\n--- cargo stderr ---\n{}",
        String::from_utf8_lossy(&build.stderr),
    );
}
