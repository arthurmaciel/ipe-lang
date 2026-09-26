//! SEAL: a `succeed` in a typed tail position obliges `Sync` on its generic.
//!
//! A generic decoder combinator's `andThen` continuation is declared
//! `-> Decoder a` and succeeds with a match-arm local `decoded : a`. The emitter
//! moves that value into a `Box<dyn Fn() -> T1 + Send + Sync>` factory, so the
//! enclosing generic must carry `T1: Send + Sync`. The signature carries `a`
//! only under `Decoder` and a boxed function, and `decoded` is no parameter, so
//! the bound comes from the succeed's typed tail position alone. Without it the
//! emitted crate is ipe-accepted but fails `cargo build` with E0277.
//!
//! The emit gate asserts the bound on the emitted signature; under `IPE_E2E=1`
//! the program is built, run, and its stdout matched against `expected.txt`.

use std::path::{Path, PathBuf};

const GOLDEN: &str = "generic_succeed_tail_sync_seal";

fn fixture_dir(root: &Path) -> PathBuf {
    root.join("tests").join("golden").join(GOLDEN)
}

/// Emit gate: the generic `rewrap` signature must bound its `T1` by `Sync`.
#[test]
fn generic_succeed_tail_sync_bound_emitted() {
    let root = crate::support::repo_root();
    let entry = fixture_dir(&root).join("Main.ipe");
    let out = std::env::temp_dir().join(format!("ipec_{GOLDEN}_emit"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "{GOLDEN}: ipe build must accept the program, got: {built:?}"
    );

    let main_rs = std::fs::read_to_string(out.join("src").join("main.rs"));
    assert!(main_rs.is_ok(), "{GOLDEN}: emitted src/main.rs must exist");
    let Ok(main_rs) = main_rs else { return };
    let signature = main_rs
        .lines()
        .find(|line| line.contains("fn main_rewrap<"))
        .unwrap_or_default();
    assert!(
        signature.contains("T1: 'static + Send + Sync"),
        "{GOLDEN}: `rewrap`'s T1 must carry `Send + Sync` (its value is captured \
         by a `decode_succeed` factory), got signature: {signature:?}"
    );
}

/// THE SEAL: under `IPE_E2E=1` the emitted crate must build and print `42`.
#[test]
fn generic_succeed_tail_sync_seal_runs() {
    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let root = crate::support::repo_root();
    let dir = fixture_dir(&root);
    let entry = dir.join("Main.ipe");
    let out = std::env::temp_dir().join(format!("ipec_{GOLDEN}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "{GOLDEN}: ipe build must accept the program, got: {built:?}"
    );

    let outcome = crate::support::build_and_run_emitted(GOLDEN, &out);
    crate::support::assert_go_parity(GOLDEN, &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "{GOLDEN}: must exit 0");
}
