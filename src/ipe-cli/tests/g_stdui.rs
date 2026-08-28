//! Consolidated golden binary for the `stdui` theme.
//! Each `mod` below is one original `golden_*.rs`; test identity is
//! preserved as `<module>::<fn>`. Merged to cut per-file link cost.

mod support;

#[path = "g_stdui/golden_css_length_color_ssot.rs"]
mod golden_css_length_color_ssot;
#[path = "g_stdui/golden_css_opacity_refinement.rs"]
mod golden_css_opacity_refinement;
#[path = "g_stdui/golden_css_source.rs"]
mod golden_css_source;
#[path = "g_stdui/golden_css_transform_ssot.rs"]
mod golden_css_transform_ssot;
#[path = "g_stdui/golden_html_attrs.rs"]
mod golden_html_attrs;
#[path = "g_stdui/golden_html_elements.rs"]
mod golden_html_elements;
#[path = "g_stdui/golden_html_render_raw.rs"]
mod golden_html_render_raw;
#[path = "g_stdui/golden_input_arc_capture.rs"]
mod golden_input_arc_capture;
#[path = "g_stdui/golden_input_callback_maybe_field.rs"]
mod golden_input_callback_maybe_field;
#[path = "g_stdui/golden_input_radio_row.rs"]
mod golden_input_radio_row;
#[path = "g_stdui/golden_input_slider.rs"]
mod golden_input_slider;
#[path = "g_stdui/golden_shape.rs"]
mod golden_shape;
#[path = "g_stdui/golden_stdui.rs"]
mod golden_stdui;
#[path = "g_stdui/golden_stdui_animation_seal.rs"]
mod golden_stdui_animation_seal;
#[path = "g_stdui/golden_stdui_cubic_bezier_seal.rs"]
mod golden_stdui_cubic_bezier_seal;
#[path = "g_stdui/golden_stdui_dualattr.rs"]
mod golden_stdui_dualattr;
#[path = "g_stdui/golden_stdui_event_illtyped.rs"]
mod golden_stdui_event_illtyped;
#[path = "g_stdui/golden_stdui_grid_seal.rs"]
mod golden_stdui_grid_seal;
#[path = "g_stdui/golden_stdui_input.rs"]
mod golden_stdui_input;
#[path = "g_stdui/golden_stdui_layoutwith.rs"]
mod golden_stdui_layoutwith;
#[path = "g_stdui/golden_stdui_msg.rs"]
mod golden_stdui_msg;
#[path = "g_stdui/golden_stdui_onclick.rs"]
mod golden_stdui_onclick;
#[path = "g_stdui/golden_stdui_oninput_closure.rs"]
mod golden_stdui_oninput_closure;
#[path = "g_stdui/golden_stdui_transition_seal.rs"]
mod golden_stdui_transition_seal;
#[path = "g_stdui/golden_tui_entry_case_seal.rs"]
mod golden_tui_entry_case_seal;
#[path = "g_stdui/golden_ui_html_wiring_batch.rs"]
mod golden_ui_html_wiring_batch;
#[path = "g_stdui/golden_ui_length_color_json.rs"]
mod golden_ui_length_color_json;
#[path = "g_stdui/golden_ui_mediaquery.rs"]
mod golden_ui_mediaquery;
