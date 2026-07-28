//! Regression — the Stage-4 `Ffi.kernel "Module_function"` alias mechanism
//! and its FAIL-CLOSED gate (THE SEAL).
//!
//! A standard-library / user binding of the shape `f = Ffi.kernel "K_n"` routes
//! `f`'s call sites directly to the built-in kernel named by the string (split
//! at the first `_` into a `(module, function)` pair). Two invariants:
//!
//! * **Positive** — an alias whose string names a REGISTERED kernel resolves and
//!   builds clean (`ipe` exit 0 AND the emitted Rust `cargo build`s). This is
//!   the mechanism working end-to-end.
//! * **Fail-closed (IPE-N0028)** — an alias whose string names NO registered
//!   kernel is rejected at compile time. Accepting it would emit a call to a
//!   non-existent kernel that type-checks in `ipe` but fails the downstream
//!   `cargo build` — the exact exit-0-then-cargo-fail hole THE SEAL forbids
//!   (`PRINCIPLES.md`, "make invalid states unrepresentable").
//!
//! ```text
//! cargo test -p ipe --test golden_ffi_kernel_alias_seal
//! ```

use std::fs;
use std::path::PathBuf;

/// A runtime `false` the optimiser cannot fold — a deliberate unconditional
/// failure marker, mirroring the sibling error goldens.
const fn false_marker() -> bool {
    std::hint::black_box(false)
}

fn write_project(dir: &std::path::Path, main: &str) -> bool {
    let src = dir.join("src");
    let _ = fs::remove_dir_all(dir);
    if fs::create_dir_all(&src).is_err() {
        return false;
    }
    fs::write(src.join("Main.ipe"), main).is_ok()
}

/// FAIL-CLOSED: `Ffi.kernel "NoSuchKernel_xyz"` names no registered kernel, so
/// `ipe` must reject it with `NameError::UnknownKernelAlias` (IPE-N0028) —
/// never accept-then-cargo-fail.
#[test]
fn unknown_kernel_alias_is_rejected_at_compile_time() {
    let Ok(runtime) = ipe::resolve_runtime() else {
        return; // runtime unavailable in this environment — skip silently
    };

    let tmp = std::env::temp_dir().join("ipec_196_ffi_kernel_alias_unknown");
    let wrote = write_project(
        &tmp,
        "module Main exposing (main)\n\
         import Ipe.Prelude exposing (..)\n\
         import Ipe.Io as Io\n\n\
         bogus : String -> String\n\
         bogus =\n\
         \x20   Ffi.kernel \"NoSuchKernel_xyz\"\n\n\
         main = Io.println (bogus \"x\")\n",
    );
    assert!(wrote, "must write the fixture project to a temp dir");

    let entry = tmp.join("src").join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m196_unknown_kernel_alias_out");
    let _ = fs::remove_dir_all(&out);

    let built = ipe::build_with_sibling_discovery(&entry, &out, &runtime);
    let Err(err) = built else {
        assert!(
            false_marker(),
            "expected IPE-N0028 rejection for an alias naming an unregistered \
             kernel, but ipe build SUCCEEDED — an exit-0-then-cargo-fail hole"
        );
        return;
    };
    let ipe::CliError::Pipeline { diag, .. } = &err else {
        assert!(false_marker(), "expected a Pipeline diagnostic, got: {err}");
        return;
    };
    let ipe_diagnostics::Diagnostic::Name {
        msg:
            ipe_diagnostics::NameError::UnknownKernelAlias {
                alias,
                module,
                function,
            },
        ..
    } = diag
    else {
        assert!(
            false_marker(),
            "expected NameError::UnknownKernelAlias (IPE-N0028), got: {err}"
        );
        return;
    };
    assert_eq!(&**alias, "NoSuchKernel_xyz");
    assert_eq!(&**module, "NoSuchKernel");
    assert_eq!(&**function, "xyz");
}

/// A malformed alias string with no `_` separator is equally rejected — it names
/// no `(module, function)` pair, so it fails closed the same way.
#[test]
fn malformed_kernel_alias_string_is_rejected() {
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };

    let tmp = std::env::temp_dir().join("ipec_196_ffi_kernel_alias_malformed");
    let wrote = write_project(
        &tmp,
        "module Main exposing (main)\n\
         import Ipe.Prelude exposing (..)\n\
         import Ipe.Io as Io\n\n\
         bogus : String -> String\n\
         bogus =\n\
         \x20   Ffi.kernel \"NoUnderscoreHere\"\n\n\
         main = Io.println (bogus \"x\")\n",
    );
    assert!(wrote, "must write the fixture project to a temp dir");

    let entry = tmp.join("src").join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m196_malformed_kernel_alias_out");
    let _ = fs::remove_dir_all(&out);

    let built = ipe::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        matches!(
            built,
            Err(ipe::CliError::Pipeline {
                diag: ipe_diagnostics::Diagnostic::Name {
                    msg: ipe_diagnostics::NameError::UnknownKernelAlias { .. },
                    ..
                },
                ..
            })
        ),
        "a `_`-less alias string must fail closed with IPE-N0028: {built:?}"
    );
}

/// Positive control (THE SEAL, satisfied): an alias whose string names a
/// REGISTERED, lowerable+emittable kernel (`String_toUpper`) resolves AND the
/// emitted Rust `cargo build`s — ipe exit 0 AND cargo exit 0.
#[test]
fn registered_kernel_alias_resolves_and_builds() {
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };

    let tmp = std::env::temp_dir().join("ipec_196_ffi_kernel_alias_registered");
    let wrote = write_project(
        &tmp,
        "module Main exposing (main)\n\
         import Ipe.Prelude exposing (..)\n\
         import Ipe.Io as Io\n\n\
         shout : String -> String\n\
         shout =\n\
         \x20   Ffi.kernel \"String_toUpper\"\n\n\
         main = Io.println (shout \"hi\")\n",
    );
    assert!(wrote, "must write the fixture project to a temp dir");

    let entry = tmp.join("src").join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("m196_registered_kernel_alias_out");
    let _ = fs::remove_dir_all(&out);

    let built = ipe::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "an alias to the registered `String_toUpper` kernel must resolve: {:?}",
        built.err()
    );
}
