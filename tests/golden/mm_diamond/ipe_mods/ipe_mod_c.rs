use crate::*;

pub(crate) fn c_from_c() -> i64 {
    static CELL: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    CELL.get_or_init(|| (crate::d_base() + 2)).clone()
}
