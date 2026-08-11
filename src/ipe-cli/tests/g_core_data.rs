//! Consolidated golden binary for the `core_data` theme.
//! Each `mod` below is one original `golden_*.rs`; test identity is
//! preserved as `<module>::<fn>`. Merged to cut per-file link cost.

mod support;

#[path = "g_core_data/golden_append.rs"]
mod golden_append;
#[path = "g_core_data/golden_append_number.rs"]
mod golden_append_number;
#[path = "g_core_data/golden_binops.rs"]
mod golden_binops;
#[path = "g_core_data/golden_bitwise_ops.rs"]
mod golden_bitwise_ops;
#[path = "g_core_data/golden_bool_literal_patterns.rs"]
mod golden_bool_literal_patterns;
#[path = "g_core_data/golden_char_pattern.rs"]
mod golden_char_pattern;
#[path = "g_core_data/golden_char_predicates.rs"]
mod golden_char_predicates;
#[path = "g_core_data/golden_combine_cps.rs"]
mod golden_combine_cps;
#[path = "g_core_data/golden_comparable.rs"]
mod golden_comparable;
#[path = "g_core_data/golden_dict_fills.rs"]
mod golden_dict_fills;
#[path = "g_core_data/golden_display_bound.rs"]
mod golden_display_bound;
#[path = "g_core_data/golden_display_false_positive.rs"]
mod golden_display_false_positive;
#[path = "g_core_data/golden_equality.rs"]
mod golden_equality;
#[path = "g_core_data/golden_errortostring_poly.rs"]
mod golden_errortostring_poly;
#[path = "g_core_data/golden_float_literal.rs"]
mod golden_float_literal;
#[path = "g_core_data/golden_list_append_op.rs"]
mod golden_list_append_op;
#[path = "g_core_data/golden_list_cps.rs"]
mod golden_list_cps;
#[path = "g_core_data/golden_list_fills.rs"]
mod golden_list_fills;
#[path = "g_core_data/golden_list_filter_partial_app.rs"]
mod golden_list_filter_partial_app;
#[path = "g_core_data/golden_list_ops_wiring.rs"]
mod golden_list_ops_wiring;
#[path = "g_core_data/golden_literals.rs"]
mod golden_literals;
#[path = "g_core_data/golden_number_typeclass.rs"]
mod golden_number_typeclass;
#[path = "g_core_data/golden_poly_fn_attr_list.rs"]
mod golden_poly_fn_attr_list;
#[path = "g_core_data/golden_recursion_guard.rs"]
mod golden_recursion_guard;
#[path = "g_core_data/golden_set_hofs.rs"]
mod golden_set_hofs;
#[path = "g_core_data/golden_string_fills.rs"]
mod golden_string_fills;
#[path = "g_core_data/golden_stringify.rs"]
mod golden_stringify;
#[path = "g_core_data/golden_tco.rs"]
mod golden_tco;
