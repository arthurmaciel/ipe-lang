use crate::*;

pub(crate) fn sky_main() -> SkyTask<()> {
    log_println(string_from_int((b_from_b() + c_from_c())))
}
