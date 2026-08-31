use crate::*;

pub(crate) fn lib_greeting() -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    static CELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CELL.get_or_init(|| "hello from Lib".to_string()).clone()
}
