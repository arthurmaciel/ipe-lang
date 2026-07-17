use crate::*;

pub(crate) fn main_summary(count: i64) -> String {
    format!(
        "{}{}",
        lib_label(LibStatus::Seeded),
        format!("{}{}", ":".to_string(), string_from_int(count))
    )
}
pub(crate) fn ipe_main() -> IpeTask<()> {
    task_and_then(lib_seed_and_count(), {
        let __sky_fn: Box<dyn Fn(i64) -> IpeTask<()> + Send + Sync + 'static> =
            Box::new(move |count: i64| -> IpeTask<()> { log_println(main_summary(count)) });
        __sky_fn
    })
}
