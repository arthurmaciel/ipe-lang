use crate::*;

pub(crate) fn main_lengths() -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    static CELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        format!(
            "{}{}",
            crate::user_ipe_css_length_to_string(crate::user_ipe_css_px(0i64)),
            format!(
                "{}{}",
                "\n".to_string(),
                format!(
                    "{}{}",
                    crate::user_ipe_css_length_to_string(crate::user_ipe_css_px(16i64)),
                    format!(
                        "{}{}",
                        "\n".to_string(),
                        format!(
                            "{}{}",
                            crate::user_ipe_css_length_to_string(crate::user_ipe_css_px(100i64)),
                            format!(
                                "{}{}",
                                "\n".to_string(),
                                format!(
                                    "{}{}",
                                    crate::user_ipe_css_length_to_string(
                                        crate::user_ipe_css_vh(50i64),
                                    ),
                                    format!(
                                        "{}{}",
                                        "\n".to_string(),
                                        format!(
                                            "{}{}",
                                            crate::user_ipe_css_length_to_string(
                                                crate::user_ipe_css_vh(100i64),
                                            ),
                                            format!(
                                                "{}{}",
                                                "\n".to_string(),
                                                format!(
                                                    "{}{}",
                                                    crate::user_ipe_css_length_to_string(
                                                        crate::user_ipe_css_vw(50i64),
                                                    ),
                                                    format!(
                                                        "{}{}",
                                                        "\n".to_string(),
                                                        crate::user_ipe_css_length_to_string(
                                                            crate::user_ipe_css_vw(100i64),
                                                        )
                                                    )
                                                )
                                            )
                                        )
                                    )
                                )
                            )
                        )
                    )
                )
            )
        )
    })
    .clone()
}
pub(crate) fn main_colors() -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    static CELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        format!(
            "{}{}",
            crate::user_ipe_css_color_to_string(
                crate::user_ipe_css_rgba(0i64, 0i64, 0i64, crate::user_ipe_css_opacity_of(1.0)),
            ),
            format!(
                "{}{}",
                "\n".to_string(),
                format!(
                    "{}{}",
                    crate::user_ipe_css_color_to_string(
                        crate::user_ipe_css_rgba(
                            255i64,
                            0i64,
                            0i64,
                            crate::user_ipe_css_opacity_of(1.0),
                        ),
                    ),
                    format!(
                        "{}{}",
                        "\n".to_string(),
                        format!(
                            "{}{}",
                            crate::user_ipe_css_color_to_string(
                                crate::user_ipe_css_rgba(
                                    0i64,
                                    128i64,
                                    255i64,
                                    crate::user_ipe_css_opacity_of(1.0),
                                ),
                            ),
                            format!(
                                "{}{}",
                                "\n".to_string(),
                                format!(
                                    "{}{}",
                                    crate::user_ipe_css_color_to_string(
                                        crate::user_ipe_css_rgba(
                                            0i64,
                                            0i64,
                                            0i64,
                                            crate::user_ipe_css_opacity_of(0.0),
                                        ),
                                    ),
                                    format!(
                                        "{}{}",
                                        "\n".to_string(),
                                        crate::user_ipe_css_color_to_string(
                                            crate::user_ipe_css_rgba(
                                                255i64,
                                                128i64,
                                                0i64,
                                                crate::user_ipe_css_opacity_of(0.5),
                                            ),
                                        )
                                    )
                                )
                            )
                        )
                    )
                )
            )
        )
    })
    .clone()
}
pub(crate) fn ipe_main() -> IpeTask<()> {
    let _ipe_recursion_guard = crate::recursion_guard();
    io_println(format!(
        "{}{}",
        crate::main_lengths(),
        format!("{}{}", "\n".to_string(), crate::main_colors())
    ))
}
