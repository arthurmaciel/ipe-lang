use crate::*;

pub(crate) fn ipe_main() -> IpeTask<()> {
    log_println(string_from_int((crate::b_from_b() + crate::c_from_c())))
}
