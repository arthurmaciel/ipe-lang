use crate::*;

pub(crate) fn b_from_b() -> i64 {
    static CELL: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    CELL.get_or_init(|| (crate::d_base() + 1)).clone()
}
