use crate::*;

pub(crate) fn d_base() -> i64 {
    static CELL: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    CELL.get_or_init(|| 42).clone()
}
