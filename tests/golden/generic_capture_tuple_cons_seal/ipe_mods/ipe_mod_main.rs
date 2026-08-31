use crate::*;

pub(crate) fn main_tag_rows<T1: 'static + Send + Sync + Clone, T2: 'static + Send + Clone>(
    read: Box<dyn Fn(T2) -> IpeResult<ipe_runtime::error::IpeError, T1> + Send + Sync + 'static>,
    rows: Vec<T2>,
) -> IpeTask<Vec<(T1, i64)>> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match (rows).as_slice() {
        [] => task_succeed(Vec::<(T1, i64)>::new()),
        [first, rest @ ..] => {
            let first = first.clone();
            let rest = rest.to_vec();
            match (read)(first) {
                IpeResult::Err(e) => task_fail(e),
                IpeResult::Ok(value) => task_map({
                    let __ipe_fn: Box<dyn Fn(Vec<(T1, i64)>) -> Vec<(T1, i64)> + Send + Sync + 'static> = Box::new(move |more: Vec<(T1, i64)>| -> Vec<(T1, i64)> { ipe_runtime::list::ipe_list_cons((value.clone(), 0i64), more) });
                    __ipe_fn
                }, crate::main_tag_rows(read, rest)),
            }
        }
    }
}
pub(crate) fn main_read_int(n: i64) -> IpeResult<ipe_runtime::error::IpeError, i64> {
    let _ipe_recursion_guard = crate::recursion_guard();
    IpeResult::Ok(n)
}
pub(crate) fn ipe_main() -> IpeTask<()> {
    let _ipe_recursion_guard = crate::recursion_guard();
    task_and_then(
        task_map(
            {
                let __ipe_fn: Box<dyn Fn(Vec<(i64, i64)>) -> () + Send + Sync + 'static> =
                    Box::new(move |arg_1: Vec<(i64, i64)>| -> () { () });
                __ipe_fn
            },
            crate::main_tag_rows(
                {
                    let __ipe_fn: Box<
                        dyn Fn(i64) -> IpeResult<ipe_runtime::error::IpeError, i64>
                            + Send
                            + Sync
                            + 'static,
                    > = Box::new(crate::main_read_int);
                    __ipe_fn
                },
                vec![1i64, 2i64, 3i64],
            ),
        ),
        Box::new(move |arg_0: ()| -> IpeTask<()> {
            io_println("generic-capture-tuple-cons-seal".to_string())
        }),
    )
}
