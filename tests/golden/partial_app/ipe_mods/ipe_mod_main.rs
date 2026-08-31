use crate::*;

pub(crate) fn main_f(a: i64) -> Box<dyn Fn(i64, i64) -> i64 + Send + Sync + 'static> {
    let _ipe_recursion_guard = crate::recursion_guard();
    {
        let __ipe_fn: Box<dyn Fn(i64, i64) -> i64 + Send + Sync + 'static> =
            Box::new(move |b: i64, c: i64| -> i64 {
                ipe_runtime::math::ipe_int_add(ipe_runtime::math::ipe_int_add(a, b), c)
            });
        __ipe_fn
    }
}
pub(crate) fn main_add3(a: i64, b: i64, c: i64) -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    ipe_runtime::math::ipe_int_add(ipe_runtime::math::ipe_int_add(a, b), c)
}
pub(crate) fn ipe_main() -> IpeTask<()> {
    let _ipe_recursion_guard = crate::recursion_guard();
    ({
        let g = crate::main_f(1i64);
        ({
            let h = {
                let __ipe_fn: Box<dyn Fn(i64) -> i64 + Send + Sync + 'static> =
                    Box::new(move |eta_0: i64| -> i64 { (g)(2i64, eta_0) });
                __ipe_fn
            };
            ({
                let boundPartial = (h)(3i64);
                ({
                    let overPartial = ({
                        let eta_0: i64 = 3i64;
                        (crate::main_f(10i64))(20i64, eta_0)
                    });
                    ({
                        let pipePartial = ({
                            let eta_0: i64 = 100i64;
                            crate::main_add3(1i64, 2i64, eta_0)
                        });
                        task_and_then(
                            io_println(string_from_int(boundPartial)),
                            Box::new(move |_| {
                                task_and_then(
                                    io_println(string_from_int(overPartial)),
                                    Box::new(move |_| io_println(string_from_int(pipePartial))),
                                )
                            }),
                        )
                    })
                })
            })
        })
    })
}
