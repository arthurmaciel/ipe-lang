use crate::*;

pub(crate) fn ipe_main() -> IpeTask<()> {
    let _ipe_recursion_guard = crate::recursion_guard();
    io_println(string_from_int(ipe_runtime::math::ipe_int_add(
        crate::b_from_b(),
        crate::c_from_c(),
    )))
}
