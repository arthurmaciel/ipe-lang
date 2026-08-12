use crate::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LibStatus {
    Empty,
    Seeded,
}
impl IpeStringify for LibStatus {
    fn ipe_show(&self) -> String {
        match self {
            LibStatus::Empty => "Empty".to_string(),
            LibStatus::Seeded => "Seeded".to_string(),
        }
    }
}
pub(crate) fn lib_label(status: LibStatus) -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    match status {
        LibStatus::Empty => "empty".to_string(),
        LibStatus::Seeded => "seeded".to_string(),
    }
}
pub(crate) fn lib_seed_and_count() -> IpeTask<i64> {
    let _ipe_recursion_guard = crate::recursion_guard();
    task_and_then(
        db_open("sqlite".to_string(), "sqlite::memory:".to_string()),
        {
            let __ipe_fn: Box<dyn Fn(Db) -> IpeTask<i64> + Send + Sync + 'static> =
                Box::new(move |conn: Db| -> IpeTask<i64> {
                    db_with_transaction(conn.clone(), {
                        let __ipe_fn: Box<dyn Fn(Db) -> IpeTask<i64> + Send + Sync + 'static> =
                            Box::new(move |txconn: Db| -> IpeTask<i64> {
                                ({
                                    let txconn = txconn.clone();
                                    {
                                        let __ipe_fn: Box<
                                            dyn Fn(IpeTask<i64>) -> IpeTask<i64>
                                                + Send
                                                + Sync
                                                + 'static,
                                        > = Box::new(move |eta_0: IpeTask<i64>| -> IpeTask<i64> {
                                            task_and_then(
                                                eta_0,
                                                ({
                                                    let txconn = txconn.clone();
                                                    {
                                                        let __ipe_fn: Box<
                                                            dyn Fn(i64) -> IpeTask<i64>
                                                                + Send
                                                                + Sync
                                                                + 'static,
                                                        > = Box::new(
                                                            move |arg_2: i64| -> IpeTask<i64> {
                                                                task_map(
                                                                    {
                                                                        let __ipe_fn: Box<
                                                                            dyn Fn(Vec<HashMap<String, String>>) -> i64
                                                                                + Send
                                                                                + Sync
                                                                                + 'static,
                                                                        > = Box::new(
                                                                            move |rows: Vec<HashMap<String, String>>| -> i64 {
                                                                                list_length(rows)
                                                                            },
                                                                        );
                                                                        __ipe_fn
                                                                    },
                                                                    db_query_params(
                                                                        txconn.clone().clone(),
                                                                        "SELECT name, qty FROM widgets"
                                                                            .to_string(),
                                                                        (Vec::<MainSqlValue>::new())
                                                                            .into_iter()
                                                                            .map(::core::convert::Into::into)
                                                                            .collect::<Vec<ipe_runtime::db::SqlParam>>(),
                                                                    ),
                                                                )
                                                            },
                                                        );
                                                        __ipe_fn
                                                    }
                                                }),
                                            )
                                        });
                                        __ipe_fn
                                    }
                                })(
                                    ({
                                        let txconn = txconn.clone();
                                        {
                                            let __ipe_fn: Box<
                                                dyn Fn(IpeTask<i64>) -> IpeTask<i64>
                                                    + Send
                                                    + Sync
                                                    + 'static,
                                            > = Box::new(
                                                move |eta_0: IpeTask<i64>| -> IpeTask<i64> {
                                                    task_and_then(
                                                        eta_0,
                                                        ({
                                                            let txconn = txconn.clone();
                                                            {
                                                                let __ipe_fn: Box<
                                                                    dyn Fn(i64) -> IpeTask<i64>
                                                                        + Send
                                                                        + Sync
                                                                        + 'static,
                                                                > = Box::new(
                                                                    move |arg_1: i64| -> IpeTask<i64> {
                                                                        db_exec_params(
                                                                            txconn.clone().clone(),
                                                                            "INSERT INTO widgets (name, qty) VALUES (?, ?)"
                                                                                .to_string(),
                                                                            (vec![
                                                                                MainSqlValue::SqlString(
                                                                                    "cog"
                                                                                        .to_string(),
                                                                                ),
                                                                                MainSqlValue::SqlInt(
                                                                                    2,
                                                                                ),
                                                                            ])
                                                                            .into_iter()
                                                                            .map(::core::convert::Into::into)
                                                                            .collect::<Vec<ipe_runtime::db::SqlParam>>(),
                                                                        )
                                                                    },
                                                                );
                                                                __ipe_fn
                                                            }
                                                        }),
                                                    )
                                                },
                                            );
                                            __ipe_fn
                                        }
                                    })(
                                        ({
                                            let txconn = txconn.clone();
                                            {
                                                let __ipe_fn: Box<
                                                    dyn Fn(IpeTask<i64>) -> IpeTask<i64>
                                                        + Send
                                                        + Sync
                                                        + 'static,
                                                > = Box::new(
                                                    move |eta_0: IpeTask<i64>| -> IpeTask<i64> {
                                                        task_and_then(
                                                            eta_0,
                                                            ({
                                                                let txconn = txconn.clone();
                                                                {
                                                                    let __ipe_fn: Box<
                                                                        dyn Fn(i64) -> IpeTask<i64>
                                                                            + Send
                                                                            + Sync
                                                                            + 'static,
                                                                    > = Box::new(
                                                                        move |arg_0: i64| -> IpeTask<i64> {
                                                                            db_exec_params(
                                                                                txconn.clone().clone(),
                                                                                "INSERT INTO widgets (name, qty) VALUES (?, ?)"
                                                                                    .to_string(),
                                                                                (vec![
                                                                                    MainSqlValue::SqlString(
                                                                                        "gear"
                                                                                            .to_string(),
                                                                                    ),
                                                                                    MainSqlValue::SqlInt(
                                                                                        4,
                                                                                    ),
                                                                                ])
                                                                                .into_iter()
                                                                                .map(::core::convert::Into::into)
                                                                                .collect::<Vec<ipe_runtime::db::SqlParam>>(),
                                                                            )
                                                                        },
                                                                    );
                                                                    __ipe_fn
                                                                }
                                                            }),
                                                        )
                                                    },
                                                );
                                                __ipe_fn
                                            }
                                        })(
                                            db_exec_raw(
                                                txconn.clone(),
                                                "CREATE TABLE widgets (name TEXT, qty INTEGER)"
                                                    .to_string(),
                                            ),
                                        ),
                                    ),
                                )
                            });
                        __ipe_fn
                    })
                });
            __ipe_fn
        },
    )
}
