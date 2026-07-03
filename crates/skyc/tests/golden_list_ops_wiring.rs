//! #68 — the non-HOF `Sky.Core.List` combinators are CALLABLE from user code.
//!
//! Before this task, a qualified `List.append` / `concat` / `take` / `drop` /
//! `zip` / `cons` / `isEmpty` / `concatMap` call resolved (via the canon
//! prelude-qualifier install) to `VarHome::Kernel` with NO `KernelFn` variant,
//! no lower arm, and no constrain scheme — so `skyc` emitted
//! `error[SKY-L0108]: kernel function not available yet` at the first such call.
//! `List.indexedMap` was worse: absent from the qualifier member array, it
//! failed even earlier at canon with `SKY-N0005` (no such member).
//!
//! The fix (design: `docs/architecture/list-ops-lower-wiring.md`) wires all nine
//! as kernels — `KernelFn` variant + `d(...)` decl + lower arm + fail-closed
//! `stdlib_scheme` entry — reusing the existing iterative runtime fns where they
//! already existed (`list_drop`/`list_zip`/`list_concat_map`/`list_indexed_map`/
//! `sky_list_cons`) and adding four total, iterative ones (`list_append`/
//! `list_concat`/`list_take`/`list_is_empty`). Kernel (not pure-Sky routing) is
//! the only exit-0-safe wiring: canon anchors `List.*` to `VarHome::Kernel`
//! unconditionally, so the pure-Sky `Sky.Core.List` bodies are never on the
//! resolution path, and the two HOFs (`concatMap`/`indexedMap`) would trip the
//! `cannot infer T2` cross-module inference hole under pure-Sky anyway.
//!
//! This golden exercises all nine in one program, hitting the Elm/Go edge
//! semantics (negative/over-length `take`/`drop`, `zip` truncating to the
//! shorter operand, empty `concat`). Gated on `SKY_E2E=1` (emitted-project
//! cargo build/run). Run: `SKY_E2E=1 cargo test --test golden_list_ops_wiring`.

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

/// Compile `tests/golden/<name>/Main.sky` into an emitted Rust project and
/// return its directory. Fails the test loudly on a compile error (so the
/// SKY-L0108/N0005 regression, were it to return, fails here rather than
/// silently skipping).
fn compile_golden(name: &str) -> PathBuf {
    let root = repo_root();
    let entry = golden_dir(&root, name).join("Main.sky");
    let out = std::env::temp_dir().join(format!("skyc_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else {
        return out;
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());
    out
}

fn e2e_enabled() -> bool {
    std::env::var("SKY_E2E").is_ok()
}

/// All nine newly-wired List ops compile and produce Elm/Go-parity output.
#[test]
fn list_ops_wiring_runs_with_parity() {
    if !e2e_enabled() {
        return;
    }
    let dir = compile_golden("m_list_ops_wiring");
    let out = support::build_and_run_emitted("m_list_ops_wiring", &dir);
    assert_eq!(
        out.exit_code,
        Some(0),
        "expected a clean exit; got {:?}",
        out.exit_code
    );
    assert_eq!(
        out.stdout.trim(),
        "[1,2,3,4] [1,2,3] [9,8] [] [] [9,8] [0,1,2] [1,1,2,2] [0,1,2] zip=2 T F"
    );
}
