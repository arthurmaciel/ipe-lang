use crate::*;

pub(crate) fn main_apply_i<FN0: Fn(i64) -> i64 + Send + Sync + 'static>(f: FN0, x: i64) -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    (f)(x)
}
pub(crate) fn main_apply_p<FN0: Fn((i64, i64)) -> i64 + Send + Sync + 'static>(
    f: FN0,
    p: (i64, i64),
) -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    (f)(p)
}
pub(crate) fn main_apply_r<FN0: Fn(RecXY) -> i64 + Send + Sync + 'static>(f: FN0, r: RecXY) -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    (f)(r)
}
pub(crate) fn main_apply_m<FN0: Fn(i64, i64, (i64, i64)) -> i64 + Send + Sync + 'static>(
    f: FN0,
) -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    (f)(100i64, 3i64, (4i64, 5i64))
}
pub(crate) fn main_ignore_arg(arg_0: i64) -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    7i64
}
pub(crate) fn main_sum_pair(arg_1: (i64, i64)) -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    ({
        let (a, b) = arg_1;
        ipe_runtime::math::ipe_int_add(a, b)
    })
}
pub(crate) fn main_get_y(arg_2: RecXY) -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    ({
        let RecXY { x: _, y, .. } = arg_2;
        y
    })
}
pub(crate) fn main_first_of_alias(arg_3: (i64, i64)) -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    ({
        let whole = arg_3;
        let (a, b) = whole.clone();
        a
    })
}
pub(crate) fn main_countdown(arg_4: (i64, i64)) -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    let mut arg_4 = arg_4;
    loop {
        let (n, acc) = arg_4;
        match n {
            0 => {
                return acc;
            }
            _ => {
                let __tco_0 = (ipe_runtime::math::ipe_int_sub(n, 1i64), ipe_runtime::math::ipe_int_add(acc, n));
                arg_4 = __tco_0;
                continue;
            }
        }
    }
}
pub(crate) fn ipe_main() -> IpeTask<()> {
    let _ipe_recursion_guard = crate::recursion_guard();
    io_println(string_from_int(ipe_runtime::math::ipe_int_add(
        ipe_runtime::math::ipe_int_add(
            ipe_runtime::math::ipe_int_add(
                ipe_runtime::math::ipe_int_add(
                    ipe_runtime::math::ipe_int_add(
                        ipe_runtime::math::ipe_int_add(
                            ipe_runtime::math::ipe_int_add(
                                ipe_runtime::math::ipe_int_add(
                                    crate::main_apply_i(move |arg_5: i64| -> i64 { 42i64 }, 0i64),
                                    crate::main_apply_p(
                                        move |arg_6: (i64, i64)| -> i64 {
                                            ({
                                                let (a, b) = arg_6;
                                                ipe_runtime::math::ipe_int_add(a, b)
                                            })
                                        },
                                        (1i64, 2i64),
                                    ),
                                ),
                                crate::main_apply_r(
                                    move |arg_7: RecXY| -> i64 {
                                        ({
                                            let RecXY { x, y: _, .. } = arg_7;
                                            x
                                        })
                                    },
                                    RecXY { x: 10i64, y: 5i64 },
                                ),
                            ),
                            crate::main_apply_m(
                                move |arg_8: i64, x: i64, arg_9: (i64, i64)| -> i64 {
                                    ({
                                        let (a, b) = arg_9;
                                        ipe_runtime::math::ipe_int_add(
                                            ipe_runtime::math::ipe_int_add(x, a),
                                            b,
                                        )
                                    })
                                },
                            ),
                        ),
                        crate::main_ignore_arg(99i64),
                    ),
                    crate::main_sum_pair((4i64, 5i64)),
                ),
                crate::main_get_y(RecXY { x: 1i64, y: 8i64 }),
            ),
            crate::main_first_of_alias((6i64, 7i64)),
        ),
        crate::main_countdown((5i64, 0i64)),
    )))
}
