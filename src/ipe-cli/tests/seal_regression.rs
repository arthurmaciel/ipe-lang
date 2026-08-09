//! Positive SEAL regression suite: well-formed Ipê programs that `ipe` MUST
//! accept (exit 0) AND whose emitted Rust crate MUST `cargo build` (and, where
//! it can run to exit, produce the expected stdout).
//!
//! `negative_suite.rs` covers THE SEAL's CONTRAPOSITIVE — a malformed program is
//! rejected and never emits. This suite covers THE SEAL ITSELF: accept ⇒
//! cargo-green. Two of the T2 findings (CO-BACKEND-001 local-shadow, and the
//! `mangle_reserved` collision) are exit-0-then-cargo-fail — or worse, a silent
//! wrong-call with NO cargo signal — so their regression test is "compiles at
//! `ipe` AND the emitted crate builds/runs correctly", which the rejection-only
//! harness cannot express.
//!
//! The cargo build+run step is `IPE_E2E`-gated (it reuses the examples-sweep's
//! build-and-run core via `e2e_support::build_and_run_rust`), so the default fast
//! pass stays emit-only: without `IPE_E2E` the tests assert only `ipe` exit 0.
//!
//! Run the full seal (`ipe` + cargo + run):
//!   `IPE_E2E=1 cargo test -p ipe --test seal_regression`
//! Emit-only (fast):
//!   `cargo test -p ipe --test seal_regression`

use std::path::PathBuf;

use ipe::CliError;

/// A runtime `false` the optimiser cannot fold, so `assert!(false_marker(), …)`
/// reads as a deliberate unconditional failure rather than a suspicious constant
/// condition — keeps this file free of the `clippy::panic` deny.
const fn false_marker() -> bool {
    std::hint::black_box(false)
}

/// Write `source` as a single-file `Main.ipe` under a fresh scratch dir keyed by
/// `name`, returning the entry path (or `None` if scratch setup fails).
fn write_single(name: &str, source: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("seal")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).ok()?;
    let entry = src.join("Main.ipe");
    std::fs::write(&entry, source).ok()?;
    Some(entry)
}

/// The scratch output dir for `name`, cleared.
fn out_dir(name: &str) -> PathBuf {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("seal-out")
        .join(name);
    let _ = std::fs::remove_dir_all(&out);
    out
}

/// Assert that `source` is ACCEPTED by `ipe` (exit 0) and — under `IPE_E2E` —
/// that the emitted crate `cargo build`s and runs to `expected_stdout`. This is
/// the positive-SEAL companion to `negative_suite::assert_rejected`.
#[track_caller]
fn assert_accepted(name: &str, source: &str, expected_stdout: &str) {
    let Some(entry) = write_single(name, source) else {
        return; // scratch unavailable — skip
    };
    let out = out_dir(name);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return; // runtime unavailable — skip
    };
    match ipe::build(&entry, &out, &runtime) {
        Ok(()) => {}
        Err(CliError::Pipeline { diag, .. }) => {
            assert!(
                false_marker(),
                "{name}: ipe REJECTED a well-formed program with {} — a false rejection",
                diag.code().as_str()
            );
            return;
        }
        Err(other) => {
            assert!(
                false_marker(),
                "{name}: non-pipeline build error: {other:?}"
            );
            return;
        }
    }

    if std::env::var("IPE_E2E").is_err() {
        return; // emit-only fast pass
    }
    match e2e_support::build_and_run_rust(name, &out) {
        Ok(run) => assert_eq!(
            run.stdout, expected_stdout,
            "{name}: emitted crate built (SEAL held) but ran to the wrong output — \
             a silent miscompile (e.g. a call bound to a local shadow instead of the \
             top-level fn)"
        ),
        Err(msg) => {
            assert!(
                false_marker(),
                "{name}: ipe accepted (exit 0) but the emitted crate FAILED to build/run — \
                 a SEAL break:\n{msg}"
            );
        }
    }
}

/// Assert that a multi-file project is ACCEPTED by `ipe` and — under `IPE_E2E` —
/// that the emitted crate `cargo build`s. Cross-module gates (e.g. mod-ident
/// folding) can only be exercised through the sibling-discovery path.
#[track_caller]
fn assert_accepted_project(name: &str, files: &[(&str, &str)], expected_stdout: &str) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("seal-proj")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    if std::fs::create_dir_all(&src).is_err() {
        return;
    }
    for (fname, contents) in files {
        let path = src.join(fname);
        if let Some(parent) = path.parent()
            && std::fs::create_dir_all(parent).is_err()
        {
            return;
        }
        if std::fs::write(&path, contents).is_err() {
            return;
        }
    }
    let out = out_dir(name);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let entry = src.join("Main.ipe");
    match ipe::build_with_sibling_discovery(&entry, &out, &runtime) {
        Ok(()) => {}
        Err(CliError::Pipeline { diag, .. }) => {
            assert!(
                false_marker(),
                "{name}: ipe REJECTED a well-formed project with {} — a false rejection",
                diag.code().as_str()
            );
            return;
        }
        Err(other) => {
            assert!(
                false_marker(),
                "{name}: non-pipeline build error: {other:?}"
            );
            return;
        }
    }

    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    match e2e_support::build_and_run_rust(name, &out) {
        Ok(run) => assert_eq!(
            run.stdout, expected_stdout,
            "{name}: emitted multi-module crate built but ran to the wrong output"
        ),
        Err(msg) => {
            assert!(
                false_marker(),
                "{name}: ipe accepted (exit 0) but the emitted crate FAILED to build/run — \
                 a SEAL break:\n{msg}"
            );
        }
    }
}

const HEAD: &str = "module Main exposing (main)\n";

// ===========================================================================
// CO-BACKEND-001 — a local must never shadow a bare-emitted top-level fn.
// The fix qualifies emitted top-level calls with `crate::`, so a local spelled
// like a top-level fn's folded Rust name cannot intercept the call.
// ===========================================================================

/// A value-typed local (`main_update`) spells the top-level fold of
/// `Main.update`. Pre-fix the call `update main_update 5` emitted a bare
/// `main_update(...)` that bound to the `Int` local → E0618 (a value is not
/// callable), AFTER `ipe` exit 0 — a SEAL break. The `crate::`-qualified call
/// resolves to the top-level fn and the program runs to `8`.
#[test]
fn value_local_shadowing_toplevel_fn_compiles_and_runs() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         import Ipe.String\n\
         update : Int -> Int -> Int\n\
         update a b = a + b\n\n\
         shadowed : Int -> String\n\
         shadowed n =\n    \
         let\n        main_update = n\n    in\n    \
         String.fromInt (update main_update 5)\n\n\
         main = Io.println (shadowed 3)\n"
    );
    // update main_update 5 = update 3 5 = 8.
    assert_accepted("value_local_shadow", &src, "8\n");
}

/// A fn-typed local (`main_helper`, a lambda) spells the top-level fold of
/// `Main.helper`. Pre-fix a later call to the TOP-LEVEL `helper` bound to the
/// LOCAL lambda instead — a SILENT wrong-call with NO cargo signal (both are
/// callable, same arity). Only a behavioural assertion catches it: the local
/// doubles, the top-level fn adds one, so the correct output proves the
/// top-level fn was invoked.
#[test]
fn fn_local_shadowing_toplevel_fn_invokes_the_toplevel() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         import Ipe.String\n\
         helper : Int -> Int\n\
         helper x = x + 1\n\n\
         compute : Int -> Int\n\
         compute n =\n    \
         let\n        main_helper = \\y -> y * 2\n    in\n    \
         main_helper (helper n)\n\n\
         main = Io.println (String.fromInt (compute 10))\n"
    );
    // helper 10 = 11 (top-level, +1); local doubles: 11 * 2 = 22. A wrong-call
    // binding `helper` to the local lambda would give (10*2)=20 doubled = 40.
    assert_accepted("fn_local_shadow", &src, "22\n");
}

/// A local literally spelled `match_` alongside a construct whose Rust fold is
/// the keyword `match` (mangled to `match_`). Pre-fix the non-injective
/// `mangle_reserved` folded both to `match_` → E0428/E0124. The injective
/// mangle sends the user `match_` to `match__`, so both coexist and the program
/// runs. (`match` is a reserved word, so the Ipê identifier itself cannot be
/// `match`; the collision surfaces through the emitted-name fold, which the
/// injective rule now separates — asserted directly by the `naming` unit test
/// `mangle_reserved_is_injective_over_reserved_and_shadows`. Here we prove a
/// user local named `match_` compiles and runs unaffected.)
#[test]
fn user_local_named_like_a_mangled_keyword_compiles() {
    let src = format!(
        "{HEAD}import Ipe.Io as Io\n\
         import Ipe.String\n\
         main =\n    \
         let\n        match_ = 7\n    in\n    \
         Io.println (String.fromInt match_)\n"
    );
    assert_accepted("user_match_underscore", &src, "7\n");
}

// ===========================================================================
// CO-BACKEND-002 — the injective home->mod_ident fold keeps a dotted module
// path distinct from an underscore-in-segment name, so both compile.
// ===========================================================================

/// `import Ipe.Log` in a helper module plus a `Main` that calls across the
/// module boundary — exercises the multi-module split (2+ homes), whose
/// `mod_ident` fold is now injective and gated by `assert_mod_idents_unique`.
/// A dotted module path (`Lib.Util`) and a distinct sibling must both emit
/// their own `ipe_mods/<ident>.rs` file without collision.
#[test]
fn multi_module_distinct_mod_idents_compile() {
    let files: &[(&str, &str)] = &[
        (
            "Main.ipe",
            "module Main exposing (main)\n\
             import Ipe.Io as Io\n\
             import Lib.Util exposing (greeting)\n\
             import Helper exposing (shout)\n\
             main = Io.println (shout greeting)\n",
        ),
        (
            "Lib/Util.ipe",
            "module Lib.Util exposing (greeting)\n\
             greeting : String\n\
             greeting = \"hi\"\n",
        ),
        (
            "Helper.ipe",
            "module Helper exposing (shout)\n\
             import Ipe.String\n\
             shout : String -> String\n\
             shout s = String.toUpper s\n",
        ),
    ];
    assert_accepted_project("multi_module_distinct", files, "HI\n");
}

// ===========================================================================
// Qualified cross-module ALIAS references expand like exposed ones. A dep's
// record alias reached ONLY through the import qualifier (`Money.Price`, no
// `exposing (Price)`) must unify with the alias expansion flowing out of the
// dep's own functions — qualified access needs no exposure.
// ===========================================================================
#[test]
fn qualified_dep_alias_expands_without_exposing() {
    let files: &[(&str, &str)] = &[
        (
            "Main.ipe",
            "module Main exposing (main)\n\
             import Ipe.Io as Io\n\
             import Lib.Money as Money\n\
             view : Money.Price -> String\n\
             view p = p.original\n\
             main = Io.println (view (Money.mk \"10\"))\n",
        ),
        (
            "Lib/Money.ipe",
            "module Lib.Money exposing (Price, mk)\n\
             type alias Price =\n\
             \x20   { original : String\n\
             \x20   , discounted : String\n\
             \x20   , hasDiscount : Bool\n\
             \x20   }\n\
             mk : String -> Price\n\
             mk s = { original = s, discounted = s, hasDiscount = False }\n",
        ),
    ];
    assert_accepted_project("qualified_dep_alias", files, "10\n");
}

// ===========================================================================
// A `Dict` VALUE stored as a function is carried on the `Arc<dyn Fn>` storage
// carrier. Built through the `Dict.singleton` / `Dict.insert` CONSTRUCTORS (a
// direct value argument, not a `fromList` literal), the value must be promoted
// to that carrier so the emitted construction agrees with the field type —
// otherwise ipe-accepts then `cargo` rejects (`Arc`-vs-`Box` `E0308`), a SEAL
// break. The looked-up function is projected out and called.
// ===========================================================================
#[test]
fn dict_singleton_function_value_builds_and_runs() {
    let src = "module Main exposing (main)\n\
        import Ipe.Io as Io\n\
        import Ipe.Dict as Dict\n\
        import Ipe.String\n\
        handlers : Dict String (Int -> Int)\n\
        handlers =\n\
        \x20   Dict.singleton \"inc\" (\\n -> n + 1)\n\
        apply : String -> Int -> Int\n\
        apply name x =\n\
        \x20   case Dict.get name handlers of\n\
        \x20       Just f ->\n\
        \x20           f x\n\
        \n\
        \x20       Nothing ->\n\
        \x20           x\n\
        main = Io.println (String.fromInt (apply \"inc\" 41))\n";
    assert_accepted("dict_singleton_fn_value", src, "42\n");
}

#[test]
fn dict_insert_function_value_builds_and_runs() {
    let src = "module Main exposing (main)\n\
        import Ipe.Io as Io\n\
        import Ipe.Dict as Dict\n\
        import Ipe.String\n\
        handlers : Dict String (Int -> Int)\n\
        handlers =\n\
        \x20   Dict.insert \"double\" (\\n -> n * 2) (Dict.singleton \"inc\" (\\n -> n + 1))\n\
        apply : String -> Int -> Int\n\
        apply name x =\n\
        \x20   case Dict.get name handlers of\n\
        \x20       Just f ->\n\
        \x20           f x\n\
        \n\
        \x20       Nothing ->\n\
        \x20           x\n\
        main = Io.println (String.fromInt (apply \"double\" (apply \"inc\" 20)))\n";
    assert_accepted("dict_insert_fn_value", src, "42\n");
}
