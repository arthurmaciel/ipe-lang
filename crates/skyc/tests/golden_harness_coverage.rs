//! Machine-checked proof that every golden test uses the shared directory-diff
//! harness (`support::assert_emitted_project_matches_golden_dir`) — see design
//! doc §2.4 step 5 / Task 9. A POSITIVE check: we assert the NEW helper is
//! present, not merely that some retired pattern is absent (a syntactically
//! different but still-stale hand-roll would pass a negative grep while staying
//! unmigrated).
//!
//! Two structural invariants, one file walk:
//!
//! 1. Every `golden_*.rs` NOT on the `NEVER_BYTE_DIFFED` allowlist MUST call the
//!    shared helper. A new byte-diffing golden that forgets it fails here.
//!
//! 2. Every file ON the allowlist MUST NOT actually byte-diff a golden `main.rs`
//!    (read the emitted `src/main.rs` AND a golden `main.rs`). This makes
//!    "on the allowlist" *structurally mean* "provably not a byte-diff test",
//!    closing the hole where a genuinely-byte-diffing new file is added to the
//!    allowlist to dodge invariant (1). The list is a typed proof of exemption,
//!    not a bare trusted name.
//!
//! The allowlist enumerates every `golden_*.rs` that never byte-diffs a golden
//! `main.rs` — exit-0-only, `outcome.stdout`, Pipeline-diagnostic, and
//! `emitted.contains(...)`/`!contains(...)` SEAL tests that read the emitted
//! source for a substring assertion but never compare it to a checked-in golden
//! `main.rs`. Every entry was machine-verified against `HEAD` when this gate was
//! authored (the walk's invariant (2) re-verifies it on every run). Extending
//! this list requires the SAME structural property — a guess that a file does
//! not byte-diff is caught by invariant (2), not trusted.

use std::path::PathBuf;

/// `golden_*.rs` tests that never byte-diff a golden `main.rs`. See the module
/// doc: invariant (2) below re-proves, on every run, that each of these does NOT
/// read the emitted `src/main.rs` and compare it to a golden `main.rs`.
const NEVER_BYTE_DIFFED: &[&str] = &[
    "golden_aud04_emit_expr_ir_capture.rs",
    "golden_aud08_function_name_collision.rs",
    "golden_aud12_append_number.rs",
    "golden_aud14_duplicate_qualifier.rs",
    // #179 / #185 — Attribute<msg> type-identity SEAL: asserts on
    // `outcome.stdout` (the rendered <svg>), never byte-diffs a golden main.rs.
    "golden_attribute_home_disambiguation_179.rs",
    "golden_class1_boundary_scheme_field_result.rs",
    "golden_core_stdlib.rs",
    "golden_cross_module_type_res.rs",
    "golden_css_source.rs",
    "golden_db_wrapper_empty_params_165.rs",
    "golden_error_adt_roundtrip.rs",
    "golden_error_details_roundtrip.rs",
    "golden_error_nominal_payload.rs",
    "golden_errortostring_poly.rs",
    "golden_http_request_name_only_fold_seal.rs",
    "golden_i101_color_seal.rs",
    "golden_i104_seal.rs",
    "golden_i111_cli_program_seal.rs",
    "golden_i117_region_seal.rs",
    "golden_i119_list_batch_seal.rs",
    "golden_i121_curried_seal.rs",
    "golden_i122_cli_program_separator.rs",
    "golden_i130_seal.rs",
    "golden_i136_alias_truncation.rs",
    "golden_i138_total_resolution.rs",
    "golden_i139_poly_fn_attr_list.rs",
    "golden_i142_access_copy_elision.rs",
    "golden_i146_lazy_emit_seal.rs",
    "golden_i147_ctor_as_fn_seal.rs",
    "golden_i148_http_stream_id.rs",
    "golden_i148_input_slider.rs",
    "golden_i149_noncl_var_hof.rs",
    "golden_i151_nested_let_fn.rs",
    "golden_i155_input_radio_row.rs",
    "golden_i161_list_filter_partial_app.rs",
    "golden_i164_poly_task_on_error_nested.rs",
    "golden_i99_alias_match_arm.rs",
    "golden_l0102_any_ctor_payload.rs",
    "golden_l0102_wildcard_lambda_pany.rs",
    "golden_l0105_refutable_gates.rs",
    "golden_l0114_ctor_payload_function.rs",
    "golden_l0114_server_handler_arc.rs",
    "golden_list_append_op.rs",
    "golden_list_cps.rs",
    "golden_list_ops_wiring.rs",
    "golden_m102_local_type_shadows_dep.rs",
    "golden_m158_nested_patterns.rs",
    "golden_m2c_function_field_gate.rs",
    "golden_m2d1_gates.rs",
    "golden_m3a_function_payload_gate.rs",
    "golden_m3a_gates.rs",
    "golden_m3b1_gates.rs",
    "golden_m3b2_gates.rs",
    "golden_m3b3_gates.rs",
    "golden_m3b3_malformed_char.rs",
    "golden_m3b4_gates.rs",
    "golden_m4b_string.rs",
    "golden_m4c_math_gate.rs",
    "golden_m4c_math.rs",
    "golden_m4d_gate.rs",
    "golden_m4d.rs",
    "golden_m4e.rs",
    "golden_m4f_encoding.rs",
    "golden_m4g_json_enc.rs",
    "golden_m4h_json_dec.rs",
    "golden_m5a_crypto.rs",
    "golden_m5a_ctor_task_gate.rs",
    "golden_m5a_task_gates.rs",
    "golden_m5a_task.rs",
    "golden_m5b_db_gates.rs",
    "golden_m5b_db.rs",
    "golden_m5b_http_builders_redirects.rs",
    "golden_m5b_http.rs",
    "golden_m5b_uuid_jwt.rs",
    "golden_m5c_tea.rs",
    "golden_m6_middleware_csrf.rs",
    "golden_m6_server.rs",
    "golden_m7_html_attrs.rs",
    "golden_m7_html_elements.rs",
    "golden_m7_live_lambda_view_routed.rs",
    "golden_m7_live_let_bound_routes.rs",
    "golden_m7_live_param_routes.rs",
    "golden_m7_live_routed_empty_routes.rs",
    "golden_m7_stdui_dualattr.rs",
    "golden_m7_stdui_event_illtyped.rs",
    "golden_m7_stdui_input.rs",
    "golden_m7_stdui_layoutwith.rs",
    "golden_m7_stdui_msg.rs",
    "golden_m7_stdui_onclick.rs",
    "golden_m7_stdui_oninput_closure.rs",
    "golden_m7_stdui.rs",
    "golden_m7_ui_length_color_json.rs",
    "golden_m82_record_ctor.rs",
    "golden_m86_error.rs",
    "golden_m88_combinators.rs",
    "golden_mixed_arm_task_run_elision_seal.rs",
    "golden_parser_gaps.rs",
    "golden_row_poly_records.rs",
    "golden_secret.rs",
    "golden_stdui_grid_seal.rs",
    "golden_stdui_transition_seal.rs",
    "golden_stringify.rs",
    "golden_t0012_cross_module_attr.rs",
    "golden_tco.rs",
    "golden_tui_entry_case_seal.rs",
    "golden_tuple_multiarm_case.rs",
    "golden_ui_html_wiring_batch.rs",
    "golden_ui_mediaquery.rs",
];

/// The `crates/skyc/tests` directory holding every `golden_*.rs`.
fn tests_dir() -> PathBuf {
    let joined = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// True if `src` reads the emitted `<out>/src/main.rs`. Two spellings occur:
/// the `out.join("src").join("main.rs")` builder and a raw `src/main.rs`.
fn reads_emitted_main_rs(src: &str) -> bool {
    (src.contains("join(\"src\")") && src.contains("join(\"main.rs\")"))
        || src.contains("src/main.rs")
}

/// True if `src` reads a CHECKED-IN GOLDEN `main.rs` to compare against.
///
/// The discriminating signature is a read of the golden path, NOT the emitted
/// one: `read_to_string(&golden)` and `let want = std::fs::read_to_string(...)`
/// are the two shapes every hand-rolled byte-diff carried, and both survive a
/// stale hand-roll that got parked on the allowlist. Deliberately does NOT
/// match a bare `.join("main.rs")`: that substring is AMBIGUOUS — the emitted
/// read `out.join("src").join("main.rs")` contains it too, so keying off it
/// would falsely flag every seal test that reads the emitted source for a
/// `.contains(...)` assertion (which is the bulk of the allowlist).
fn reads_golden_main_rs(src: &str) -> bool {
    src.contains("read_to_string(&golden)") || src.contains("let want = std::fs::read_to_string")
}

#[test]
fn every_non_allowlisted_golden_test_calls_the_shared_helper() {
    let dir = tests_dir();
    // A coverage gate that can pass because it "couldn't look" is not a gate:
    // hard-fail if the tests directory is unreadable rather than silently
    // returning green (guardian ruling — no vacuous pass).
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("tests dir {} must be readable: {e}", dir.display()));

    let mut offenders = Vec::new(); // non-allowlisted golden missing the helper
    let mut liars = Vec::new(); // allowlisted golden that actually byte-diffs

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("golden_") || !name.ends_with(".rs") {
            continue;
        }
        // This gate's own file never calls the helper — it checks for it.
        if name == "golden_harness_coverage.rs" {
            continue;
        }
        // A file that will not read is neither provably migrated nor provably
        // exempt — treat as an offender rather than skipping it (no vacuous
        // pass at the per-file level either).
        let Ok(src) = std::fs::read_to_string(&path) else {
            offenders.push(format!("{name} (unreadable)"));
            continue;
        };

        let allowlisted = NEVER_BYTE_DIFFED.contains(&name);
        let calls_helper = src.contains("assert_emitted_project_matches_golden_dir");

        if allowlisted {
            // Invariant (2): an allowlisted file must EARN its exemption — it
            // must not actually byte-diff a golden `main.rs`.
            if reads_emitted_main_rs(&src) && reads_golden_main_rs(&src) {
                liars.push(name.to_owned());
            }
        } else if !calls_helper {
            // Invariant (1): a non-allowlisted golden must use the shared helper.
            offenders.push(name.to_owned());
        }
    }

    assert!(
        offenders.is_empty(),
        "golden tests not migrated to the shared directory-diff helper (and not \
         on the allowlist): {offenders:?}"
    );
    assert!(
        liars.is_empty(),
        "golden tests on the NEVER_BYTE_DIFFED allowlist that actually byte-diff \
         a golden main.rs — remove them from the allowlist and migrate them to \
         the shared helper instead: {liars:?}"
    );
}
