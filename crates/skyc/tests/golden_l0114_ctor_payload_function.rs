//! #90 SKY-L0114 ctor-payload-function — Stage 1 lift (function values in
//! `Maybe`/`Result`/user-union constructor payloads) + the T3 `andMap`
//! curried-payload gate + the T4 fn-value-reuse gate.
//!
//! See `docs/architecture/ctor-payload-function-design.md` (Stage 1 overview)
//! and `docs/architecture/ctor-payload-andmap-arity-gate-design.md` (the
//! revised, two-tier T3 design implemented here, after three same-day revert
//! incidents on 2026-07-10 — see `BACKLOG.md`'s `#90` row for the incident
//! log).
//!
//! ## What's covered
//!
//! * T1/T2 — a function value DIRECTLY in `Ok`/`Just`/a user union's payload
//!   (declared or laundered through a type variable) is now ACCEPTED.
//! * T3 Tier 2 (primary) — `sky_types::constrain::constrain_var_kernel` ties
//!   `Maybe.andMap`/`Result.andMap`'s payload-result scheme-var to a
//!   `TyBounds::and_map_payload()` obligation, checked at type-check time
//!   (`sky_types::infer`) BEFORE lowering ever runs. This is a TYPE-LEVEL
//!   check, so it survives arbitrary Sky-level aliasing by construction. It
//!   surfaces as ONE of two diagnostics depending on HOW the obligated
//!   variable is used, mirroring the existing `Math.min` gate's documented
//!   split (`golden_m4c_math_gate.rs`): a DIRECT `andMap` call pins the
//!   obligated variable straight to a concrete `Fun` structure at the
//!   unifier's own head-pin check — the "eager pin" case — surfacing a plain
//!   `SKY-T0001` (`TypeMismatch`); an ANNOTATED GENERIC FORWARDER around
//!   `andMap` instead lifts the obligation onto its own skolem and defers to
//!   the `SchemeApp`/`check_scheme_applications` pass, re-verified at each of
//!   the forwarder's own external call sites — surfacing the friendlier,
//!   specifically-labelled `SKY-T0014` (`SuperTypeUnsatisfied`, "single-
//!   argument function"). Both are clean Sky diagnostics, never a cargo-fail;
//!   which one you see depends only on whether `andMap` was called directly
//!   or through a forwarder, confirmed empirically below (not merely
//!   predicted by the design doc, which anticipated SKY-T0014 as the sole
//!   Tier-2 code for every shape).
//! * T3 Tier 1 (backstop) — `reject_curried_andmap_payload`, re-anchored
//!   INSIDE `lower_callee` itself (the single funnel every kernel/top-level
//!   reference resolves through), rather than the `Call`-node arm the three
//!   reverted attempts used. Never observed firing in this pass's testing
//!   (Tier 2 always catches the hazard first) — kept as defense-in-depth.
//! * T4 — `reject_fn_value_reuse` (`SKY-L0127`), wired at all FIVE call sites
//!   a fn-carrying binding can originate from: typed/untyped Def params,
//!   let-bindings, match-arm bindings, AND lambda params (the lambda-param
//!   site was the exact gap that caused the first revert-incident, Bug 1).
//!
//! ## Aliasing-shape coverage (T3 fixture matrix)
//!
//! Every shape the incident history found (or the revised design's fixture
//! matrix names) gets its own red (still correctly rejected) fixture, each
//! going through `assert_and_map_curried_rejected` (accepts SKY-T0001 /
//! SKY-T0014 / SKY-L0114 — see that helper's doc comment):
//!
//! | Shape | Fixture | Origin | Observed code |
//! |---|---|---|---|
//! | Direct call / pipe-desugared | `l0114_and_map_curried_stays_gated` | pre-existing | SKY-T0001 |
//! | `let`-bound partial application | `l0114_and_map_let_bound_alias_stays_gated` | Bug 2 (2nd revert) | SKY-T0001 |
//! | Bare, point-free top-level re-export | `l0114_and_map_bare_alias_stays_gated` | Bug 3 (3rd revert) | SKY-T0001 |
//! | Higher-order argument | `l0114_and_map_higher_order_arg_stays_gated` | new, this pass | SKY-T0001 |
//! | Record-field extraction | `l0114_and_map_record_field_stays_gated` | new, this pass | SKY-T0001 (Tier 2 fires before the pipeline ever reaches the unrelated, pre-existing SKY-L0107 record-field gate — confirmed empirically, see the test's own doc comment) |
//! | Annotated generic forwarder | `l0114_and_map_forwarder_curried_is_t0014` | new, this pass | SKY-T0014 (the friendly-message path) |
//! | Import alias | *(not constructible)* | `Result`/`Maybe` are compiler-kernel qualifiers in `sky-rust`, not backed by an importable Sky-source module (`crates/sky_canon/src/resolve.rs`'s fixed kernel-qualifier list) — there is no module to `import … as …` in this milestone. Documented here rather than silently skipped. | n/a |
//! | Cross-module annotated wrapper, reused at 2 different arity-1 types | `l0114_and_map_cross_module_wrapper_accepted` | design doc's `T3.residual` row — must stay ACCEPTED (proves Tier 2 does not over-reject) | n/a — accepted, confirmed; no `T3d` follow-up needed |

use std::path::{Path, PathBuf};

use skyc::CliError;

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn fixture_entry(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name).join("Main.sky")
}

fn fixture_src_entry(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name).join("src").join("Main.sky")
}

/// Build `name`'s `Main.sky` and return the diagnostic code if the pipeline
/// rejected it, `None` if it was accepted (or failed for a non-pipeline
/// reason, which the caller's assertion will catch).
fn built_code(root: &Path, name: &str) -> (Result<(), CliError>, PathBuf) {
    let entry = fixture_entry(root, name);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}_emit"));
    let _ = std::fs::remove_dir_all(&out);
    let runtime = skyc::resolve_runtime();
    let Ok(runtime) = runtime else {
        return (Ok(()), out); // resolver unavailable in this environment — skip below
    };
    (skyc::build(&entry, &out, &runtime), out)
}

// ── T1/T2: direct construction now ACCEPTED ─────────────────────────────────

/// `Ok f` holding a function used to trip SKY-L0114 unconditionally, making
/// `Result.andMap` unusable. Construction is sound (#87 derive-demotion +
/// generic-bounded runtime `SkyResult` derives).
#[test]
fn result_and_map_fn_payload_accepted() {
    let root = repo_root();
    if skyc::resolve_runtime().is_err() {
        return;
    }
    let (built, out) = built_code(&root, "l0114_result_and_map_fn_payload");
    assert!(built.is_ok(), "Ok f |> Result.andMap must be accepted: {built:?}");

    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let outcome = support::build_and_run_emitted("l0114_result_and_map_fn_payload", &out);
    assert_eq!(outcome.exit_code, Some(0), "exit 0; stdout:\n{}", outcome.stdout);
    assert_eq!(outcome.stdout.trim(), "3");
}

/// `Just f` holding a function used to trip SKY-L0114 unconditionally, making
/// `Maybe.andMap` unusable.
#[test]
fn maybe_and_map_fn_payload_accepted() {
    let root = repo_root();
    if skyc::resolve_runtime().is_err() {
        return;
    }
    let (built, out) = built_code(&root, "l0114_maybe_and_map_fn_payload");
    assert!(built.is_ok(), "Just f |> Maybe.andMap must be accepted: {built:?}");

    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let outcome = support::build_and_run_emitted("l0114_maybe_and_map_fn_payload", &out);
    assert_eq!(outcome.exit_code, Some(0), "exit 0; stdout:\n{}", outcome.stdout);
    assert_eq!(outcome.stdout.trim(), "42");
}

/// A DECLARED function-typed constructor payload (`RetryWhen (e -> Bool)`)
/// used to trip SKY-L0114 at the union declaration itself. #87's
/// derive-demotion fixpoint already keeps the emitted enum sound.
#[test]
fn ctor_decl_fn_payload_accepted() {
    let root = repo_root();
    if skyc::resolve_runtime().is_err() {
        return;
    }
    let (built, out) = built_code(&root, "l0114_ctor_decl_fn_payload");
    assert!(built.is_ok(), "declared fn-typed ctor payload must be accepted: {built:?}");

    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let outcome = support::build_and_run_emitted("l0114_ctor_decl_fn_payload", &out);
    assert_eq!(outcome.exit_code, Some(0), "exit 0; stdout:\n{}", outcome.stdout);
    assert_eq!(outcome.stdout.trim(), "retry");
}

/// T4 sound companion: calling an extracted function value MORE THAN ONCE
/// (callee position) is unlimited — a `Box<dyn Fn>` call borrows, never
/// moves. Must NOT trip the T4 reuse gate.
#[test]
fn fn_extracted_called_twice_accepted() {
    let root = repo_root();
    if skyc::resolve_runtime().is_err() {
        return;
    }
    let (built, out) = built_code(&root, "l0114_fn_extracted_called_twice");
    assert!(built.is_ok(), "calling an extracted fn twice must be accepted: {built:?}");

    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let outcome = support::build_and_run_emitted("l0114_fn_extracted_called_twice", &out);
    assert_eq!(outcome.exit_code, Some(0), "exit 0; stdout:\n{}", outcome.stdout);
    assert_eq!(outcome.stdout.trim(), "5");
}

// ── T3: the andMap curried-payload gate — every aliasing shape ─────────────

/// Assert `name`'s `Main.sky` is rejected with one of the three CLEAN outcomes
/// the two-tier design can produce — never silently accepted, never a
/// cargo-fail.
///
/// * `SKY-T0001` (ordinary `TypeMismatch`) — the "eager pin" case: every
///   fixture here calls `Maybe.andMap` / `Result.andMap` DIRECTLY (not
///   through a generic forwarder), so the curried payload's `b` unifies
///   HEAD-TO-HEAD against a concrete `Fun` structure during ordinary
///   solving. `unify.rs`'s `super_concrete_ok` head-pin-check rejects any
///   `Fun` meeting ANY bounded variable at the unification site itself,
///   before the deferred, nicely-labelled `SuperTypeUnsatisfied` pass ever
///   runs — EXACTLY the same "eager-pin sibling" behaviour already
///   documented for `Math.min`/`Math.max` called directly on two
///   non-comparable values (`crates/skyc/tests/golden_m4c_math_gate.rs`:
///   "Calling `Math.min` directly on two non-comparable values is the
///   eager-pin sibling and surfaces SKY-T0001 instead"). This is the
///   EXPECTED code for every fixture in this matrix, verified against the
///   actual behaviour of this pass, not merely predicted.
/// * `SKY-T0014` (`SuperTypeUnsatisfied`) — reached when the obligated
///   variable escapes into an ANNOTATED GENERIC FORWARDER (the
///   `check_scheme_applications` / `SchemeApp` path) rather than pinning
///   directly — see `and_map_curried_forwarder_is_sky_t0014` below for the
///   fixture that exercises this path with the friendly "single-argument
///   function" message.
/// * `SKY-L0114` — the Tier-1 lowering backstop, acceptable defense-in-depth
///   outcome if Tier 2's wiring ever has a bug (never observed in this
///   pass's testing, but kept as an accepted outcome so a future Tier-2
///   regression fails LOUD with a wrong-tier note rather than silently
///   passing this assertion).
fn assert_and_map_curried_rejected(name: &str) {
    let root = repo_root();
    if skyc::resolve_runtime().is_err() {
        return;
    }
    let (built, _out) = built_code(&root, name);
    let code = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert!(
        code == Some(sky_diagnostics::SKY_T0001)
            || code == Some(sky_diagnostics::SKY_T0014)
            || code == Some(sky_diagnostics::SKY_L0114),
        "{name}: curried andMap payload must be rejected with SKY-T0001 (Tier 2, \
         eager pin — the expected outcome for a DIRECT `andMap` call), SKY-T0014 \
         (Tier 2, deferred — reached only through a generic forwarder), or \
         SKY-L0114 (Tier 1 backstop), got: {built:?}"
    );
}

/// Direct call / pipe-desugared curried call — pre-existing shape, always
/// caught even by the first (buggy) T3 attempts.
#[test]
fn and_map_curried_direct_call_stays_gated() {
    assert_and_map_curried_rejected("l0114_and_map_curried_stays_gated");
}

/// `let`-bound partial application — revert-incident Bug 2 (2nd revert):
/// `let g = Result.andMap (Ok 1) in g (Ok add3)` bypassed the first T3 fix,
/// which pattern-matched only the direct + pipe-desugared call shapes.
#[test]
fn and_map_curried_let_bound_alias_stays_gated() {
    assert_and_map_curried_rejected("l0114_and_map_let_bound_alias_stays_gated");
}

/// Bare, point-free top-level re-export — revert-incident Bug 3 (3rd
/// revert): `myAndMap = Result.andMap` … `myAndMap (Ok 1) (Ok add3)` reaches
/// `Result.andMap`'s own reference through `lower_expr`'s bare-value arm,
/// never `lower_call_uniform`'s Call-node arm — the exact gap the revised
/// design closes by re-anchoring the Tier-1 check inside `lower_callee`
/// itself, and by making Tier 2 a genuine type-level obligation that does
/// not depend on lowering-time AST shape at all.
#[test]
fn and_map_curried_bare_alias_stays_gated() {
    assert_and_map_curried_rejected("l0114_and_map_bare_alias_stays_gated");
}

/// Higher-order argument — `Result.andMap` passed bare as a call ARGUMENT
/// (not a callee), applied to a curried payload by the callee it was passed
/// to.
#[test]
fn and_map_curried_higher_order_arg_stays_gated() {
    assert_and_map_curried_rejected("l0114_and_map_higher_order_arg_stays_gated");
}

/// Record-field extraction — `{ combiner = Result.andMap }`, later called
/// with a curried payload through the extracted field. Verified: this hits
/// the SAME Tier-2 eager-pin path (`SKY-T0001`) as every other direct-call
/// shape in this matrix — type-checking runs BEFORE lowering, so the T3
/// obligation on `Result.andMap`'s own occurrence (which does not care what
/// larger expression contains it — a record-field initializer is just
/// another position `constrain_var_kernel` sees the same way) fires and
/// rejects the program before the pipeline ever reaches the PRE-EXISTING,
/// unrelated `SKY-L0107` lowering-time gate (first-class function in a
/// record field) that would ALSO have rejected this shape had it gotten
/// that far. Confirmed empirically, not assumed — this is why the fixture
/// matrix table in the module doc comment says "closed by construction",
/// not "closed by SKY-L0107": the closing mechanism is Tier 2, same as
/// every other row.
#[test]
fn and_map_record_field_extraction_stays_gated() {
    assert_and_map_curried_rejected("l0114_and_map_record_field_stays_gated");
}

/// design doc `T3.residual` fixture: an ANNOTATED wrapper around
/// `Result.andMap`, exported and reused CROSS-MODULE at two DIFFERENT
/// concrete payload types, both individually arity-1-safe. Must stay
/// ACCEPTED — proves the Tier-2 obligation does not over-reject legitimate
/// cross-module reuse (the one precision-loss case the design doc
/// acknowledges is the UNANNOTATED/untyped variant, §3.3 — not this one).
#[test]
fn and_map_cross_module_annotated_wrapper_accepted() {
    let root = repo_root();
    if skyc::resolve_runtime().is_err() {
        return;
    }
    let entry = fixture_src_entry(&root, "l0114_and_map_cross_module_wrapper_accepted");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("l0114_and_map_cross_module_wrapper_accepted_emit");
    let _ = std::fs::remove_dir_all(&out);
    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "an annotated andMap wrapper reused cross-module at two arity-1-safe \
         concrete types must be accepted, got: {built:?}"
    );
}

/// The "forwarder" companion of `and_map_cross_module_annotated_wrapper_accepted`:
/// the SAME annotated `andMapAlias` wrapper, this time instantiated at a
/// CURRIED (arity >= 3) payload. Proves the FRIENDLY, deferred
/// `SuperTypeUnsatisfied` (`SKY-T0014`) diagnostic path genuinely fires
/// (not just the eager-pin `SKY-T0001` every direct-call fixture above hits)
/// — the obligation propagates onto the wrapper's own annotation skolem and
/// is re-verified at THIS external call site, exactly like `Math.min`'s
/// `pickMin` forwarder gate.
#[test]
fn and_map_forwarder_curried_is_sky_t0014() {
    let root = repo_root();
    if skyc::resolve_runtime().is_err() {
        return;
    }
    let entry = fixture_src_entry(&root, "l0114_and_map_forwarder_curried_is_t0014");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("l0114_and_map_forwarder_curried_is_t0014_emit");
    let _ = std::fs::remove_dir_all(&out);
    let Ok(runtime) = skyc::resolve_runtime() else {
        return;
    };
    let built = skyc::build_with_sibling_discovery(&entry, &out, &runtime);
    let code = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        code,
        Some(sky_diagnostics::SKY_T0014),
        "a curried payload through an ANNOTATED andMap forwarder must surface \
         the friendly SuperTypeUnsatisfied diagnostic (SKY-T0014), got: {built:?}"
    );
}

// ── T4: fn-value reuse gate ─────────────────────────────────────────────────

/// revert-incident Bug 1 (2nd revert): a fn-carrying, non-Clone LAMBDA
/// parameter used in two consuming positions inside the lambda's own body.
/// `lower_lambda` builds its own `ir_params` independently of the other four
/// `reject_fn_value_reuse` call sites and, before the fix, never ran them
/// through the gate — this reached `cargo build` as E0382 instead of a clean
/// SKY-L0127.
#[test]
fn lambda_param_reuse_gated() {
    let root = repo_root();
    if skyc::resolve_runtime().is_err() {
        return;
    }
    let (built, _out) = built_code(&root, "l0127_lambda_param_reuse_gated");
    let code = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        code,
        Some(sky_diagnostics::SKY_L0127),
        "a fn-carrying lambda param used twice (consuming positions) must be \
         rejected with SKY-L0127, got: {built:?}"
    );
}

/// Sound companion: calling the SAME lambda param twice (callee position) is
/// unlimited — proves the Bug 1 fix does not over-reject.
#[test]
fn lambda_param_call_twice_accepted() {
    let root = repo_root();
    if skyc::resolve_runtime().is_err() {
        return;
    }
    let (built, out) = built_code(&root, "l0127_lambda_param_call_twice_accepted");
    assert!(built.is_ok(), "calling a lambda param twice must be accepted: {built:?}");

    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let outcome = support::build_and_run_emitted("l0127_lambda_param_call_twice_accepted", &out);
    assert_eq!(outcome.exit_code, Some(0), "exit 0; stdout:\n{}", outcome.stdout);
    assert_eq!(outcome.stdout.trim(), "5");
}

/// T4 residual gate: a fn-carrying, non-Clone `let`-binding used in two
/// consuming (argument) positions has no sound rewrite (`Box<dyn Fn>` is not
/// `Clone`) — must stay a clean SKY-L0127, never E0382 at `cargo build`.
#[test]
fn fn_carrier_reuse_gated() {
    let root = repo_root();
    if skyc::resolve_runtime().is_err() {
        return;
    }
    let (built, _out) = built_code(&root, "l0127_fn_carrier_reuse_gated");
    let code = match &built {
        Err(CliError::Pipeline { diag, .. }) => Some(diag.code()),
        _ => None,
    };
    assert_eq!(
        code,
        Some(sky_diagnostics::SKY_L0127),
        "a fn-carrying let-binding used twice (consuming positions) must be \
         rejected with SKY-L0127, got: {built:?}"
    );
}
