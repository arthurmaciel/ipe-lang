//! Consolidated golden binary for the `issues` theme.
//! Each `mod` below is one original `golden_*.rs`; test identity is
//! preserved as `<module>::<fn>`. Merged to cut per-file link cost.

mod support;

#[path = "g_issues/golden_i101_color_seal.rs"]
mod golden_i101_color_seal;
#[path = "g_issues/golden_i104_seal.rs"]
mod golden_i104_seal;
#[path = "g_issues/golden_i119_list_batch_seal.rs"]
mod golden_i119_list_batch_seal;
#[path = "g_issues/golden_i121_curried_seal.rs"]
mod golden_i121_curried_seal;
#[path = "g_issues/golden_i122_cli_program_separator.rs"]
mod golden_i122_cli_program_separator;
#[path = "g_issues/golden_i125_decoder_destructure_thunk.rs"]
mod golden_i125_decoder_destructure_thunk;
#[path = "g_issues/golden_i130_seal.rs"]
mod golden_i130_seal;
#[path = "g_issues/golden_i136_alias_truncation.rs"]
mod golden_i136_alias_truncation;
#[path = "g_issues/golden_i138_total_resolution.rs"]
mod golden_i138_total_resolution;
#[path = "g_issues/golden_i142_access_copy_elision.rs"]
mod golden_i142_access_copy_elision;
#[path = "g_issues/golden_i147_ctor_as_fn_seal.rs"]
mod golden_i147_ctor_as_fn_seal;
#[path = "g_issues/golden_i151_nested_let_fn.rs"]
mod golden_i151_nested_let_fn;
#[path = "g_issues/golden_i172_mixed_arc_box_handler.rs"]
mod golden_i172_mixed_arc_box_handler;
#[path = "g_issues/golden_i178_fn_composite_reuse.rs"]
mod golden_i178_fn_composite_reuse;
#[path = "g_issues/golden_i217_stdlib_contract_drift.rs"]
mod golden_i217_stdlib_contract_drift;
#[path = "g_issues/golden_i221_fn_value_carrier.rs"]
mod golden_i221_fn_value_carrier;
#[path = "g_issues/golden_i665_retry_policy_value_callee.rs"]
mod golden_i665_retry_policy_value_callee;
#[path = "g_issues/golden_i672_random_members.rs"]
mod golden_i672_random_members;
#[path = "g_issues/golden_i789_record_fn_carrier.rs"]
mod golden_i789_record_fn_carrier;
#[path = "g_issues/golden_i793_record_fn_read.rs"]
mod golden_i793_record_fn_read;
#[path = "g_issues/golden_i798_generic_combinator_seal.rs"]
mod golden_i798_generic_combinator_seal;
#[path = "g_issues/golden_i801_decoder_storage_reuse.rs"]
mod golden_i801_decoder_storage_reuse;
#[path = "g_issues/golden_i802_generic_optional_sync.rs"]
mod golden_i802_generic_optional_sync;
#[path = "g_issues/golden_i99_alias_match_arm.rs"]
mod golden_i99_alias_match_arm;
