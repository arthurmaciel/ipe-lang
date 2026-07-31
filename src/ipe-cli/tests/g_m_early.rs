//! Consolidated golden binary for the `m_early` theme.
//! Each `mod` below is one original `golden_*.rs`; test identity is
//! preserved as `<module>::<fn>`. Merged to cut per-file link cost.

mod support;

#[path = "g_m_early/golden_m2d1_gates.rs"]
mod golden_m2d1_gates;
#[path = "g_m_early/golden_m3a_gates.rs"]
mod golden_m3a_gates;
#[path = "g_m_early/golden_m3b1_gates.rs"]
mod golden_m3b1_gates;
#[path = "g_m_early/golden_m3b2_gates.rs"]
mod golden_m3b2_gates;
#[path = "g_m_early/golden_m3b3_gates.rs"]
mod golden_m3b3_gates;
#[path = "g_m_early/golden_m3b3_malformed_char.rs"]
mod golden_m3b3_malformed_char;
#[path = "g_m_early/golden_m3b4_gates.rs"]
mod golden_m3b4_gates;
#[path = "g_m_early/golden_m3b4_nested.rs"]
mod golden_m3b4_nested;
