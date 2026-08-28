use crate::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssCssProp {
    CssPropSafe(String, String),
    CssDropped,
}
impl IpeStringify for IpeCssCssProp {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssCssProp::CssPropSafe(p0, p1) => format!(
                "CssPropSafe {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
            IpeCssCssProp::CssDropped => "CssDropped".to_string(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssCssRule {
    CssRule(String, Vec<IpeCssCssProp>),
    CssMedia(String, Box<Vec<IpeCssCssRule>>),
    CssKeyframes(String, Vec<String>),
    CssRaw(String),
    CssRuleDropped,
}
impl IpeStringify for IpeCssCssRule {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssCssRule::CssRule(p0, p1) => format!(
                "CssRule {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
            IpeCssCssRule::CssMedia(p0, p1) => format!(
                "CssMedia {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
            IpeCssCssRule::CssKeyframes(p0, p1) => format!(
                "CssKeyframes {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
            IpeCssCssRule::CssRaw(p0) => {
                format!("CssRaw {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssCssRule::CssRuleDropped => "CssRuleDropped".to_string(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssLength {
    Px(i64),
    Rem(f64),
    Em(f64),
    Pct(f64),
    Vh(i64),
    Vw(i64),
    Ch(f64),
    Fr(i64),
    Num(f64),
    LenAuto,
    LenZero,
    LenRaw(String),
}
impl IpeStringify for IpeCssLength {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssLength::Px(p0) => {
                format!("Px {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssLength::Rem(p0) => {
                format!("Rem {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssLength::Em(p0) => {
                format!("Em {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssLength::Pct(p0) => {
                format!("Pct {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssLength::Vh(p0) => {
                format!("Vh {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssLength::Vw(p0) => {
                format!("Vw {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssLength::Ch(p0) => {
                format!("Ch {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssLength::Fr(p0) => {
                format!("Fr {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssLength::Num(p0) => {
                format!("Num {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssLength::LenAuto => "LenAuto".to_string(),
            IpeCssLength::LenZero => "LenZero".to_string(),
            IpeCssLength::LenRaw(p0) => {
                format!("LenRaw {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssColor {
    Hex(String),
    Rgb(i64, i64, i64),
    Rgba(i64, i64, i64, IpeCssOpacity),
    Hsl(i64, i64, i64),
    Hsla(i64, i64, i64, IpeCssOpacity),
    ColorTransparent,
    ColorCurrent,
    ColorRaw(String),
}
impl IpeStringify for IpeCssColor {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssColor::Hex(p0) => {
                format!("Hex {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssColor::Rgb(p0, p1, p2) => format!(
                "Rgb {} {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p2)).dispatch()
            ),
            IpeCssColor::Rgba(p0, p1, p2, p3) => format!(
                "Rgba {} {} {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p2)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p3)).dispatch()
            ),
            IpeCssColor::Hsl(p0, p1, p2) => format!(
                "Hsl {} {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p2)).dispatch()
            ),
            IpeCssColor::Hsla(p0, p1, p2, p3) => format!(
                "Hsla {} {} {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p2)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p3)).dispatch()
            ),
            IpeCssColor::ColorTransparent => "ColorTransparent".to_string(),
            IpeCssColor::ColorCurrent => "ColorCurrent".to_string(),
            IpeCssColor::ColorRaw(p0) => format!(
                "ColorRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssOpacity {
    Opacity(f64),
}
impl IpeStringify for IpeCssOpacity {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssOpacity::Opacity(p0) => {
                format!("Opacity {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssDisplay {
    Block,
    Inline,
    InlineBlock,
    Flex,
    InlineFlex,
    Grid,
    InlineGrid,
    DisplayNone,
    DisplayRaw(String),
}
impl IpeStringify for IpeCssDisplay {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssDisplay::Block => "Block".to_string(),
            IpeCssDisplay::Inline => "Inline".to_string(),
            IpeCssDisplay::InlineBlock => "InlineBlock".to_string(),
            IpeCssDisplay::Flex => "Flex".to_string(),
            IpeCssDisplay::InlineFlex => "InlineFlex".to_string(),
            IpeCssDisplay::Grid => "Grid".to_string(),
            IpeCssDisplay::InlineGrid => "InlineGrid".to_string(),
            IpeCssDisplay::DisplayNone => "DisplayNone".to_string(),
            IpeCssDisplay::DisplayRaw(p0) => format!(
                "DisplayRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssPosition {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
    PositionRaw(String),
}
impl IpeStringify for IpeCssPosition {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssPosition::Static => "Static".to_string(),
            IpeCssPosition::Relative => "Relative".to_string(),
            IpeCssPosition::Absolute => "Absolute".to_string(),
            IpeCssPosition::Fixed => "Fixed".to_string(),
            IpeCssPosition::Sticky => "Sticky".to_string(),
            IpeCssPosition::PositionRaw(p0) => format!(
                "PositionRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssTextAlign {
    AlignLeft,
    AlignRight,
    AlignCenter,
    AlignJustify,
    TextAlignRaw(String),
}
impl IpeStringify for IpeCssTextAlign {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssTextAlign::AlignLeft => "AlignLeft".to_string(),
            IpeCssTextAlign::AlignRight => "AlignRight".to_string(),
            IpeCssTextAlign::AlignCenter => "AlignCenter".to_string(),
            IpeCssTextAlign::AlignJustify => "AlignJustify".to_string(),
            IpeCssTextAlign::TextAlignRaw(p0) => format!(
                "TextAlignRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssCursor {
    Pointer,
    CursorDefault,
    CursorText,
    NotAllowed,
    Grab,
    Grabbing,
    CursorRaw(String),
}
impl IpeStringify for IpeCssCursor {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssCursor::Pointer => "Pointer".to_string(),
            IpeCssCursor::CursorDefault => "CursorDefault".to_string(),
            IpeCssCursor::CursorText => "CursorText".to_string(),
            IpeCssCursor::NotAllowed => "NotAllowed".to_string(),
            IpeCssCursor::Grab => "Grab".to_string(),
            IpeCssCursor::Grabbing => "Grabbing".to_string(),
            IpeCssCursor::CursorRaw(p0) => format!(
                "CursorRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssFlexDirection {
    Row,
    RowReverse,
    Column,
    ColumnReverse,
    FlexDirectionRaw(String),
}
impl IpeStringify for IpeCssFlexDirection {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssFlexDirection::Row => "Row".to_string(),
            IpeCssFlexDirection::RowReverse => "RowReverse".to_string(),
            IpeCssFlexDirection::Column => "Column".to_string(),
            IpeCssFlexDirection::ColumnReverse => "ColumnReverse".to_string(),
            IpeCssFlexDirection::FlexDirectionRaw(p0) => format!(
                "FlexDirectionRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssFlexWrap {
    NoWrap,
    Wrap,
    WrapReverse,
    FlexWrapRaw(String),
}
impl IpeStringify for IpeCssFlexWrap {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssFlexWrap::NoWrap => "NoWrap".to_string(),
            IpeCssFlexWrap::Wrap => "Wrap".to_string(),
            IpeCssFlexWrap::WrapReverse => "WrapReverse".to_string(),
            IpeCssFlexWrap::FlexWrapRaw(p0) => format!(
                "FlexWrapRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssAlign {
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
    Stretch,
    Baseline,
    AlignRaw(String),
}
impl IpeStringify for IpeCssAlign {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssAlign::Start => "Start".to_string(),
            IpeCssAlign::End => "End".to_string(),
            IpeCssAlign::Center => "Center".to_string(),
            IpeCssAlign::SpaceBetween => "SpaceBetween".to_string(),
            IpeCssAlign::SpaceAround => "SpaceAround".to_string(),
            IpeCssAlign::SpaceEvenly => "SpaceEvenly".to_string(),
            IpeCssAlign::Stretch => "Stretch".to_string(),
            IpeCssAlign::Baseline => "Baseline".to_string(),
            IpeCssAlign::AlignRaw(p0) => format!(
                "AlignRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssFontWeight {
    Normal,
    Bold,
    Lighter,
    Bolder,
    Weight(i64),
    FontWeightRaw(String),
}
impl IpeStringify for IpeCssFontWeight {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssFontWeight::Normal => "Normal".to_string(),
            IpeCssFontWeight::Bold => "Bold".to_string(),
            IpeCssFontWeight::Lighter => "Lighter".to_string(),
            IpeCssFontWeight::Bolder => "Bolder".to_string(),
            IpeCssFontWeight::Weight(p0) => {
                format!("Weight {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeCssFontWeight::FontWeightRaw(p0) => format!(
                "FontWeightRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssFontStyle {
    FontNormal,
    Italic,
    Oblique,
    FontStyleRaw(String),
}
impl IpeStringify for IpeCssFontStyle {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssFontStyle::FontNormal => "FontNormal".to_string(),
            IpeCssFontStyle::Italic => "Italic".to_string(),
            IpeCssFontStyle::Oblique => "Oblique".to_string(),
            IpeCssFontStyle::FontStyleRaw(p0) => format!(
                "FontStyleRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssOverflow {
    Visible,
    Hidden,
    Scroll,
    OverflowAuto,
    OverflowRaw(String),
}
impl IpeStringify for IpeCssOverflow {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssOverflow::Visible => "Visible".to_string(),
            IpeCssOverflow::Hidden => "Hidden".to_string(),
            IpeCssOverflow::Scroll => "Scroll".to_string(),
            IpeCssOverflow::OverflowAuto => "OverflowAuto".to_string(),
            IpeCssOverflow::OverflowRaw(p0) => format!(
                "OverflowRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssWhiteSpace {
    WsNormal,
    NoWrapWs,
    Pre,
    PreWrap,
    PreLine,
    WhiteSpaceRaw(String),
}
impl IpeStringify for IpeCssWhiteSpace {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssWhiteSpace::WsNormal => "WsNormal".to_string(),
            IpeCssWhiteSpace::NoWrapWs => "NoWrapWs".to_string(),
            IpeCssWhiteSpace::Pre => "Pre".to_string(),
            IpeCssWhiteSpace::PreWrap => "PreWrap".to_string(),
            IpeCssWhiteSpace::PreLine => "PreLine".to_string(),
            IpeCssWhiteSpace::WhiteSpaceRaw(p0) => format!(
                "WhiteSpaceRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssBoxSizing {
    ContentBox,
    BorderBox,
    BoxSizingRaw(String),
}
impl IpeStringify for IpeCssBoxSizing {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssBoxSizing::ContentBox => "ContentBox".to_string(),
            IpeCssBoxSizing::BorderBox => "BorderBox".to_string(),
            IpeCssBoxSizing::BoxSizingRaw(p0) => format!(
                "BoxSizingRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssPointerEvents {
    PeAuto,
    PeNone,
    PointerEventsRaw(String),
}
impl IpeStringify for IpeCssPointerEvents {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssPointerEvents::PeAuto => "PeAuto".to_string(),
            IpeCssPointerEvents::PeNone => "PeNone".to_string(),
            IpeCssPointerEvents::PointerEventsRaw(p0) => format!(
                "PointerEventsRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssTextDecoration {
    NoDecoration,
    Underline,
    LineThrough,
    Overline,
    TextDecorationRaw(String),
}
impl IpeStringify for IpeCssTextDecoration {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssTextDecoration::NoDecoration => "NoDecoration".to_string(),
            IpeCssTextDecoration::Underline => "Underline".to_string(),
            IpeCssTextDecoration::LineThrough => "LineThrough".to_string(),
            IpeCssTextDecoration::Overline => "Overline".to_string(),
            IpeCssTextDecoration::TextDecorationRaw(p0) => format!(
                "TextDecorationRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeCssBorderStyle {
    Solid,
    Dashed,
    Dotted,
    BorderNone,
    BorderStyleRaw(String),
}
impl IpeStringify for IpeCssBorderStyle {
    fn ipe_show(&self) -> String {
        match self {
            IpeCssBorderStyle::Solid => "Solid".to_string(),
            IpeCssBorderStyle::Dashed => "Dashed".to_string(),
            IpeCssBorderStyle::Dotted => "Dotted".to_string(),
            IpeCssBorderStyle::BorderNone => "BorderNone".to_string(),
            IpeCssBorderStyle::BorderStyleRaw(p0) => format!(
                "BorderStyleRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
pub(crate) fn user_ipe_css_opacity_of(n: f64) -> IpeCssOpacity {
    let _ipe_recursion_guard = crate::recursion_guard();
    (if math_is_nan(n) {
        IpeCssOpacity::Opacity(0.0)
    } else {
        IpeCssOpacity::Opacity(basics_clamp(0.0, 1.0, n))
    })
}
pub(crate) fn user_ipe_css_opacity_to_string(o: IpeCssOpacity) -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    match o {
        IpeCssOpacity::Opacity(n) => crate::user_ipe_css_float_str(n),
    }
}
pub(crate) fn user_ipe_css_float_str(n: f64) -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    string_from_float(n)
}
pub(crate) fn user_ipe_css_length_to_string(lengthVal: IpeCssLength) -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    match lengthVal {
        IpeCssLength::Px(n) => format!("{}{}", string_from_int(n), "px".to_string()),
        IpeCssLength::Rem(n) => format!("{}{}", crate::user_ipe_css_float_str(n), "rem".to_string()),
        IpeCssLength::Em(n) => format!("{}{}", crate::user_ipe_css_float_str(n), "em".to_string()),
        IpeCssLength::Pct(n) => format!("{}{}", crate::user_ipe_css_float_str(n), "%".to_string()),
        IpeCssLength::Vh(n) => format!("{}{}", string_from_int(n), "vh".to_string()),
        IpeCssLength::Vw(n) => format!("{}{}", string_from_int(n), "vw".to_string()),
        IpeCssLength::Ch(n) => format!("{}{}", crate::user_ipe_css_float_str(n), "ch".to_string()),
        IpeCssLength::Fr(n) => format!("{}{}", string_from_int(n), "fr".to_string()),
        IpeCssLength::Num(n) => crate::user_ipe_css_float_str(n),
        IpeCssLength::LenAuto => "auto".to_string(),
        IpeCssLength::LenZero => "0".to_string(),
        IpeCssLength::LenRaw(s) => s,
    }
}
pub(crate) fn user_ipe_css_color_to_string(c: IpeCssColor) -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    match c {
        IpeCssColor::Hex(s) => format!("{}{}", "#".to_string(), s),
        IpeCssColor::Rgb(r, g, b) => format!(
            "{}{}",
            "rgb(".to_string(),
            format!(
                "{}{}",
                string_from_int(r),
                format!(
                    "{}{}",
                    ",".to_string(),
                    format!(
                        "{}{}",
                        string_from_int(g),
                        format!(
                            "{}{}",
                            ",".to_string(),
                            format!("{}{}", string_from_int(b), ")".to_string())
                        )
                    )
                )
            )
        ),
        IpeCssColor::Rgba(r, g, b, a) => format!(
            "{}{}",
            "rgba(".to_string(),
            format!(
                "{}{}",
                string_from_int(r),
                format!(
                    "{}{}",
                    ",".to_string(),
                    format!(
                        "{}{}",
                        string_from_int(g),
                        format!(
                            "{}{}",
                            ",".to_string(),
                            format!(
                                "{}{}",
                                string_from_int(b),
                                format!(
                                    "{}{}",
                                    ",".to_string(),
                                    format!(
                                        "{}{}",
                                        crate::user_ipe_css_opacity_to_string(a),
                                        ")".to_string()
                                    )
                                )
                            )
                        )
                    )
                )
            )
        ),
        IpeCssColor::Hsl(h, s, l) => format!(
            "{}{}",
            "hsl(".to_string(),
            format!(
                "{}{}",
                string_from_int(h),
                format!(
                    "{}{}",
                    ",".to_string(),
                    format!(
                        "{}{}",
                        string_from_int(s),
                        format!(
                            "{}{}",
                            "%,".to_string(),
                            format!("{}{}", string_from_int(l), "%)".to_string())
                        )
                    )
                )
            )
        ),
        IpeCssColor::Hsla(h, s, l, a) => format!(
            "{}{}",
            "hsla(".to_string(),
            format!(
                "{}{}",
                string_from_int(h),
                format!(
                    "{}{}",
                    ",".to_string(),
                    format!(
                        "{}{}",
                        string_from_int(s),
                        format!(
                            "{}{}",
                            "%,".to_string(),
                            format!(
                                "{}{}",
                                string_from_int(l),
                                format!(
                                    "{}{}",
                                    "%,".to_string(),
                                    format!(
                                        "{}{}",
                                        crate::user_ipe_css_opacity_to_string(a),
                                        ")".to_string()
                                    )
                                )
                            )
                        )
                    )
                )
            )
        ),
        IpeCssColor::ColorTransparent => "transparent".to_string(),
        IpeCssColor::ColorCurrent => "currentColor".to_string(),
        IpeCssColor::ColorRaw(s) => s,
    }
}
pub(crate) fn user_ipe_css_px(n: i64) -> IpeCssLength {
    let _ipe_recursion_guard = crate::recursion_guard();
    IpeCssLength::Px(n)
}
pub(crate) fn user_ipe_css_vh(n: i64) -> IpeCssLength {
    let _ipe_recursion_guard = crate::recursion_guard();
    IpeCssLength::Vh(n)
}
pub(crate) fn user_ipe_css_vw(n: i64) -> IpeCssLength {
    let _ipe_recursion_guard = crate::recursion_guard();
    IpeCssLength::Vw(n)
}
pub(crate) fn user_ipe_css_rgba(r: i64, g: i64, b: i64, a: IpeCssOpacity) -> IpeCssColor {
    let _ipe_recursion_guard = crate::recursion_guard();
    IpeCssColor::Rgba(r, g, b, a)
}
