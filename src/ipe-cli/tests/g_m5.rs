//! Consolidated golden binary for the `m5` theme.
//! Each `mod` below is one original `golden_*.rs`; test identity is
//! preserved as `<module>::<fn>`. Merged to cut per-file link cost.

mod support;

#[path = "g_m5/golden_m158_nested_patterns.rs"]
mod golden_m158_nested_patterns;
#[path = "g_m5/golden_m5a_crypto.rs"]
mod golden_m5a_crypto;
#[path = "g_m5/golden_m5a_ctor_task_gate.rs"]
mod golden_m5a_ctor_task_gate;
#[path = "g_m5/golden_m5a_task.rs"]
mod golden_m5a_task;
#[path = "g_m5/golden_m5a_task_gates.rs"]
mod golden_m5a_task_gates;
#[path = "g_m5/golden_m5c_tea.rs"]
mod golden_m5c_tea;
#[path = "g_m5/golden_m5pipe.rs"]
mod golden_m5pipe;
#[path = "g_m5/golden_m86_error.rs"]
mod golden_m86_error;
#[path = "g_m5/golden_m88_combinators.rs"]
mod golden_m88_combinators;
#[path = "g_m5/golden_mm.rs"]
mod golden_mm;
