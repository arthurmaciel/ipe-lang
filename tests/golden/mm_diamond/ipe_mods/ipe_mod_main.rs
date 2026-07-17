use crate::*;

pub(crate) fn ipe_main() -> IpeTask<()> {
    log_println(string_from_int((b_from_b() + c_from_c())))
}
