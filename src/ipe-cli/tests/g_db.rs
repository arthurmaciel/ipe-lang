//! Consolidated golden binary for the `db` theme.
//! Each `mod` below is one original `golden_*.rs`; test identity is
//! preserved as `<module>::<fn>`. Merged to cut per-file link cost.

mod support;

#[path = "g_db/golden_db_get_alias_row.rs"]
mod golden_db_get_alias_row;
#[path = "g_db/golden_db_get_iperow_bound.rs"]
mod golden_db_get_iperow_bound;
#[path = "g_db/golden_db_store_accessor_leaves.rs"]
mod golden_db_store_accessor_leaves;
#[path = "g_db/golden_db_wrapper_empty_params_165.rs"]
mod golden_db_wrapper_empty_params_165;
#[path = "g_db/golden_i177_db_get_false_positive.rs"]
mod golden_i177_db_get_false_positive;
#[path = "g_db/golden_m5b_db.rs"]
mod golden_m5b_db;
#[path = "g_db/golden_m5b_db_gates.rs"]
mod golden_m5b_db_gates;
#[path = "g_db/golden_m5b_http.rs"]
mod golden_m5b_http;
#[path = "g_db/golden_m5b_uuid_jwt.rs"]
mod golden_m5b_uuid_jwt;
