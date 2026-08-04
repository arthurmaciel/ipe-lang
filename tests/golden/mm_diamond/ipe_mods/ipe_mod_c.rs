use crate::*;

pub(crate) fn c_from_c() -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    static CELL: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    CELL.get_or_init(|| (crate::d_base() + 2)).clone()
}
