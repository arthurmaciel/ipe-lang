use crate::*;

pub(crate) fn ipe_main() -> IpeTask<()> {
    let _ipe_recursion_guard = crate::recursion_guard();
    io_println(crate::lib_greeting())
}
