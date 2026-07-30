use crate::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LibColor {
    Red,
    Blue,
}
impl IpeStringify for LibColor {
    fn ipe_show(&self) -> String {
        match self {
            LibColor::Red => "Red".to_string(),
            LibColor::Blue => "Blue".to_string(),
        }
    }
}
pub(crate) fn lib_greeting() -> String {
    static CELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CELL.get_or_init(|| "hello from Lib".to_string()).clone()
}
