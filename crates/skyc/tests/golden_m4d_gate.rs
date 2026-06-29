//! M4d `Sky.Core.Dict` / `Sky.Core.Set` comparable-key gate.
//!
//! A `Set` element and a `Dict` key carry the Sky `comparable`-key obligation
//! (the kernel's element / key variable is minted as a super-typed variable, the
//! same path `Math.min` / `Math.max` take for ordering). Two failure classes
//! must be caught at `skyc` — never left to emit Rust `cargo` rejects:
//!
//! * **Non-comparable element / key** (a record, a user ADT, a function): fails
//!   closed at type-check as `SKY-T0001` (the eager-pin mismatch — the
//!   super-typed element variable meets a structure that does not support
//!   ordering). This is also stricter than Sky's runtime, which keys a Set /
//!   Dict on a stringified value.
//! * **`Float` element / key** (`Set Float` / `Dict Float v`): `Float` IS Sky
//!   `comparable`, so the type checker accepts it; but Rust's `f64` is neither
//!   `Ord` nor `Hash` / `Eq`, so `BTreeSet<f64>` / `HashMap<f64, _>` cannot
//!   exist. Fails closed at lowering as `SKY-L0117` (a dedicated diagnostic).
//!   Divergence from Sky, rationale: Rust backend capability.
//!
//! Every fixture must stop the pipeline before codegen — no emitted crate.

use std::path::{Path, PathBuf};

use skyc::CliError;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Build `tests/golden/<fixture>/Main.sky`, assert it fails with `expected`, and
/// assert NO Rust was emitted (the pipeline stopped before codegen). Skips
/// silently when the runtime cannot be resolved.
fn assert_gate(fixture: &str, out_suffix: &str, expected: sky_diagnostics::Code) {
    let root = repo_root();
    let entry = root
        .join("tests")
        .join("golden")
        .join(fixture)
        .join("Main.sky");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(out_suffix);
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let built = skyc::build(&entry, &out, &runtime);
    let got = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        got,
        Some(expected),
        "fixture {fixture}: expected {expected:?}, got build result {built:?}"
    );

    let emitted = out.join("src").join("main.rs");
    assert!(
        !emitted.exists(),
        "fixture {fixture}: no Rust must be emitted on a rejection, but {} exists",
        emitted.display()
    );
}

// ── Non-comparable element / key → SKY-T0001 (eager-pin mismatch) ─────────────

/// `Set.insert { x = 1 } Set.empty` — a record is not orderable, so the Set
/// element variable cannot pin to it.
#[test]
fn set_record_element_is_sky_t0001() {
    assert_gate(
        "m4d_set_record_gate",
        "m4d_set_record_gate_emit",
        sky_diagnostics::SKY_T0001,
    );
}

/// `Dict.insert Red 1 Dict.empty` — a user ADT is not a comparable Dict key.
#[test]
fn dict_adt_key_is_sky_t0001() {
    assert_gate(
        "m4d_dict_adt_gate",
        "m4d_dict_adt_gate_emit",
        sky_diagnostics::SKY_T0001,
    );
}

// ── Float element / key → SKY-L0117 (Rust backend capability) ─────────────────

/// `Set Float` type-checks (Sky `Float` is `comparable`) but has no sound Rust
/// backing (`f64` is not `Ord`), so lowering rejects it.
#[test]
fn set_float_element_is_sky_l0117() {
    assert_gate(
        "m4d_set_float_gate",
        "m4d_set_float_gate_emit",
        sky_diagnostics::SKY_L0117,
    );
}

/// `Dict Float v` type-checks but `HashMap<f64, _>` cannot exist (`f64` is not
/// `Hash` / `Eq`), so lowering rejects it.
#[test]
fn dict_float_key_is_sky_l0117() {
    assert_gate(
        "m4d_dict_float_gate",
        "m4d_dict_float_gate_emit",
        sky_diagnostics::SKY_L0117,
    );
}

// ── Float Set via INFERENCE (no annotation) → SKY-L0117 ───────────────────────
//
// The fixtures above carry a `: Set Float` / `: Dict Float v` annotation, which
// drives the shape gate in `ir_type_from_ty`. A Set produced purely by inference
// never drives that conversion, so its own region type is the only place `Float`
// surfaces — these fixtures exercise that inference path.

/// Inline, unannotated `Set.fromList [1.5, 2.5]` — the producing call's region
/// type (`Set Float`) is the only carrier of the `Float` element.
#[test]
fn set_float_inline_is_sky_l0117() {
    assert_gate(
        "m4d_set_float_inline_gate",
        "m4d_set_float_inline_gate_emit",
        sky_diagnostics::SKY_L0117,
    );
}

/// `let s = Set.fromList [1.5, 2.5]` — a `let`-bound float Set with no
/// annotation on the binding.
#[test]
fn set_float_let_is_sky_l0117() {
    assert_gate(
        "m4d_set_float_let_gate",
        "m4d_set_float_let_gate_emit",
        sky_diagnostics::SKY_L0117,
    );
}

/// A Set built from a `List.map` result (`Set.fromList floats` where `floats`
/// is a mapped `List Float`) — the float-ness is map-derived, not a literal
/// float list.
#[test]
fn set_float_mapped_is_sky_l0117() {
    assert_gate(
        "m4d_set_float_mapped_gate",
        "m4d_set_float_mapped_gate_emit",
        sky_diagnostics::SKY_L0117,
    );
}
