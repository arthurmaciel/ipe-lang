use crate::*;

pub(crate) fn main_lengths() -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    static CELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        format!(
            "{}{}",
            "0px".to_string(),
            format!(
                "{}{}",
                "\n".to_string(),
                format!(
                    "{}{}",
                    "16px".to_string(),
                    format!(
                        "{}{}",
                        "\n".to_string(),
                        format!(
                            "{}{}",
                            "100px".to_string(),
                            format!(
                                "{}{}",
                                "\n".to_string(),
                                format!(
                                    "{}{}",
                                    "50vh".to_string(),
                                    format!(
                                        "{}{}",
                                        "\n".to_string(),
                                        format!(
                                            "{}{}",
                                            "100vh".to_string(),
                                            format!(
                                                "{}{}",
                                                "\n".to_string(),
                                                format!(
                                                    "{}{}",
                                                    "50vw".to_string(),
                                                    format!(
                                                        "{}{}",
                                                        "\n".to_string(),
                                                        "100vw".to_string()
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
                crate::user_ipe_css_rgba(
                    0i64,
                    0i64,
                    0i64,
                    (if math_is_nan(1.0) {
                        IpeCssOpacity::Opacity(0.0)
                    } else {
                        IpeCssOpacity::Opacity(basics_clamp(0.0, 1.0, 1.0))
                    }),
                ),
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
                            (if math_is_nan(1.0) {
                                IpeCssOpacity::Opacity(0.0)
                            } else {
                                IpeCssOpacity::Opacity(basics_clamp(0.0, 1.0, 1.0))
                            }),
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
                                    (if math_is_nan(1.0) {
                                        IpeCssOpacity::Opacity(0.0)
                                    } else {
                                        IpeCssOpacity::Opacity(basics_clamp(0.0, 1.0, 1.0))
                                    }),
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
                                            (if math_is_nan(0.0) {
                                                IpeCssOpacity::Opacity(0.0)
                                            } else {
                                                IpeCssOpacity::Opacity(basics_clamp(0.0, 1.0, 0.0))
                                            }),
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
                                                (if math_is_nan(0.5) {
                                                    IpeCssOpacity::Opacity(0.0)
                                                } else {
                                                    IpeCssOpacity::Opacity(
                                                        basics_clamp(0.0, 1.0, 0.5),
                                                    )
                                                }),
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
