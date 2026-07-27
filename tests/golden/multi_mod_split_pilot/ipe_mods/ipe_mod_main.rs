use crate::*;

pub(crate) fn main_summary(count: i64) -> String {
    format!(
        "{}{}",
        crate::lib_label(LibStatus::Seeded),
        format!("{}{}", ":".to_string(), string_from_int(count))
    )
}
pub(crate) fn ipe_main() -> IpeTask<()> {
    task_and_then(crate::lib_seed_and_count(), {
        let __ipe_fn: Box<dyn Fn(i64) -> IpeTask<()> + Send + Sync + 'static> =
            Box::new(move |count: i64| -> IpeTask<()> { io_println(crate::main_summary(count)) });
        __ipe_fn
    })
}
