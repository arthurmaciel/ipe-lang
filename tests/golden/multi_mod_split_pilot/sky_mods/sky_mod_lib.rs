use crate::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LibStatus {
    Empty,
    Seeded,
}
impl SkyStringify for LibStatus {
    fn sky_show(&self) -> String {
        match self {
            LibStatus::Empty => "Empty".to_string(),
            LibStatus::Seeded => "Seeded".to_string(),
        }
    }
}
pub(crate) fn lib_label(status: LibStatus) -> String {
    match status {
        LibStatus::Empty => "empty".to_string(),
        LibStatus::Seeded => "seeded".to_string(),
    }
}
pub(crate) fn lib_seed_and_count() -> SkyTask<i64> {
    task_and_then(db_open("sqlite".to_string(), "sqlite::memory:".to_string()), { let __sky_fn: Box<dyn Fn(Db) -> SkyTask<i64> + Send + Sync + 'static> = Box::new(move |conn: Db| -> SkyTask<i64> { db_with_transaction(conn.clone(), { let __sky_fn: Box<dyn Fn(Db) -> SkyTask<i64> + Send + Sync + 'static> = Box::new(move |txconn: Db| -> SkyTask<i64> { task_and_then(db_exec_raw(txconn.clone().clone(), "CREATE TABLE widgets (name TEXT, qty INTEGER)".to_string()), Box::new(move |_| { task_and_then(db_exec_params(txconn.clone().clone(), "INSERT INTO widgets (name, qty) VALUES (?, ?)".to_string(), (vec![MainSqlValue::SqlString("gear".to_string()), MainSqlValue::SqlInt(4)]).into_iter().map(::core::convert::Into::into).collect::<Vec<sky_runtime::db::SqlParam>>()), Box::new(move |_| { task_and_then(db_exec_params(txconn.clone().clone(), "INSERT INTO widgets (name, qty) VALUES (?, ?)".to_string(), (vec![MainSqlValue::SqlString("cog".to_string()), MainSqlValue::SqlInt(2)]).into_iter().map(::core::convert::Into::into).collect::<Vec<sky_runtime::db::SqlParam>>()), Box::new(move |_| { task_map({ let __sky_fn: Box<dyn Fn(Vec<HashMap<String, String>>) -> i64 + Send + Sync + 'static> = Box::new(move |rows: Vec<HashMap<String, String>>| -> i64 { list_length(rows) }); __sky_fn }, db_query_params(txconn.clone(), "SELECT name, qty FROM widgets".to_string(), (Vec::<MainSqlValue>::new()).into_iter().map(::core::convert::Into::into).collect::<Vec<sky_runtime::db::SqlParam>>())) })) })) })) }); __sky_fn }) }); __sky_fn })
}
