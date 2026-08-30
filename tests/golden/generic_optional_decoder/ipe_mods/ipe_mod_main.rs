use crate::*;

pub(crate) fn main_json_optional<T1: 'static + Send + Sync + Clone, T2: 'static + Send + Clone>(
    field: String,
    dec: Decoder<T1>,
    default: T1,
    next: Decoder<Box<dyn FnOnce(T1) -> T2 + Send + 'static>>,
) -> Decoder<T2> {
    let _ipe_recursion_guard = crate::recursion_guard();
    decode_pipeline_optional(field, dec, default, next)
}
pub(crate) fn main_db_optional<T1: 'static + Send + Sync + Clone, T2: 'static + Send + Clone>(
    col: String,
    dec: Decoder<T1>,
    fallback: T1,
    next: Decoder<Box<dyn FnOnce(T1) -> T2 + Send + 'static>>,
) -> Decoder<T2> {
    let _ipe_recursion_guard = crate::recursion_guard();
    db_decode_optional(col, dec, fallback, next)
}
pub(crate) fn main_make_label(name: String, age: i64) -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    format!(
        "{}{}",
        name,
        format!("{}{}", "|".to_string(), string_from_int(age))
    )
}
pub(crate) fn main_make_tag(name: String, nick: String) -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    format!("{}{}", name, format!("{}{}", "#".to_string(), nick))
}
pub(crate) fn main_json_int_result(s: String) -> IpeResult<ipe_runtime::error::IpeError, String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    decode_from_json_string(
        ({
            let cap_0 = "age".to_string();
            ({
                let cap_1 = json_decode_int::<IpeError>();
                {
                    let __ipe_fn: Box<
                        dyn Fn(Decoder<Box<dyn FnOnce(i64) -> String + Send + 'static>>) -> Decoder<String>
                            + Send
                            + Sync
                            + 'static,
                    > = Box::new(
                        move |eta_0: Decoder<Box<dyn FnOnce(i64) -> String + Send + 'static>>| -> Decoder<String> {
                            crate::main_json_optional(cap_0.clone(), cap_1.clone(), 0, eta_0)
                        },
                    );
                    __ipe_fn
                }
            })
        })(
            ({
                let cap_0 = "name".to_string();
                ({
                    let cap_1 = json_decode_string::<IpeError>();
                    {
                        let __ipe_fn: Box<
                            dyn Fn(Decoder<Box<dyn FnOnce(String) -> Box<dyn FnOnce(i64) -> String + Send + 'static> + Send + 'static>>) -> Decoder<Box<dyn FnOnce(i64) -> String + Send + 'static>>
                                + Send
                                + Sync
                                + 'static,
                        > = Box::new(
                            move |eta_0: Decoder<Box<dyn FnOnce(String) -> Box<dyn FnOnce(i64) -> String + Send + 'static> + Send + 'static>>| -> Decoder<Box<dyn FnOnce(i64) -> String + Send + 'static>> {
                                decode_pipeline_required(cap_0.clone(), cap_1.clone(), eta_0)
                            },
                        );
                        __ipe_fn
                    }
                })
            })(decode_succeed(curry2(crate::main_make_label))),
        ),
        s,
    )
}
pub(crate) fn main_json_string_result(
    s: String,
) -> IpeResult<ipe_runtime::error::IpeError, String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    decode_from_json_string(
        ({
            let cap_0 = "nick".to_string();
            ({
                let cap_1 = json_decode_string::<IpeError>();
                ({
                    let cap_2 = "none".to_string();
                    {
                        let __ipe_fn: Box<
                            dyn Fn(Decoder<Box<dyn FnOnce(String) -> String + Send + 'static>>) -> Decoder<String>
                                + Send
                                + Sync
                                + 'static,
                        > = Box::new(
                            move |eta_0: Decoder<Box<dyn FnOnce(String) -> String + Send + 'static>>| -> Decoder<String> {
                                crate::main_json_optional(
                                    cap_0.clone(),
                                    cap_1.clone(),
                                    cap_2.clone(),
                                    eta_0,
                                )
                            },
                        );
                        __ipe_fn
                    }
                })
            })
        })(
            ({
                let cap_0 = "name".to_string();
                ({
                    let cap_1 = json_decode_string::<IpeError>();
                    {
                        let __ipe_fn: Box<
                            dyn Fn(Decoder<Box<dyn FnOnce(String) -> Box<dyn FnOnce(String) -> String + Send + 'static> + Send + 'static>>) -> Decoder<Box<dyn FnOnce(String) -> String + Send + 'static>>
                                + Send
                                + Sync
                                + 'static,
                        > = Box::new(
                            move |eta_0: Decoder<Box<dyn FnOnce(String) -> Box<dyn FnOnce(String) -> String + Send + 'static> + Send + 'static>>| -> Decoder<Box<dyn FnOnce(String) -> String + Send + 'static>> {
                                decode_pipeline_required(cap_0.clone(), cap_1.clone(), eta_0)
                            },
                        );
                        __ipe_fn
                    }
                })
            })(decode_succeed(curry2(crate::main_make_tag))),
        ),
        s,
    )
}
pub(crate) fn main_row_decoder() -> Decoder<String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    ({
        let cap_0 = "age".to_string();
        ({
            let cap_1 = db_decode_int("age".to_string());
            {
                let __ipe_fn: Box<
                    dyn Fn(Decoder<Box<dyn FnOnce(i64) -> String + Send + 'static>>) -> Decoder<String>
                        + Send
                        + Sync
                        + 'static,
                > = Box::new(
                    move |eta_0: Decoder<Box<dyn FnOnce(i64) -> String + Send + 'static>>| -> Decoder<String> {
                        crate::main_db_optional(cap_0.clone(), cap_1.clone(), 0, eta_0)
                    },
                );
                __ipe_fn
            }
        })
    })(
        ({
            let cap_0 = "name".to_string();
            ({
                let cap_1 = db_decode_string("name".to_string());
                {
                    let __ipe_fn: Box<
                        dyn Fn(Decoder<Box<dyn FnOnce(String) -> Box<dyn FnOnce(i64) -> String + Send + 'static> + Send + 'static>>) -> Decoder<Box<dyn FnOnce(i64) -> String + Send + 'static>>
                            + Send
                            + Sync
                            + 'static,
                    > = Box::new(
                        move |eta_0: Decoder<Box<dyn FnOnce(String) -> Box<dyn FnOnce(i64) -> String + Send + 'static> + Send + 'static>>| -> Decoder<Box<dyn FnOnce(i64) -> String + Send + 'static>> {
                            db_decode_required(cap_0.clone(), cap_1.clone(), eta_0)
                        },
                    );
                    __ipe_fn
                }
            })
        })(decode_succeed(curry2(crate::main_make_label))),
    )
}
pub(crate) fn main_report_json(res: IpeResult<ipe_runtime::error::IpeError, String>) -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    match res {
        IpeResult::Ok(label) => label,
        IpeResult::Err(_) => "json-err".to_string(),
    }
}
pub(crate) fn ipe_main() -> IpeTask<()> {
    let _ipe_recursion_guard = crate::recursion_guard();
    task_and_then(
        db_open("sqlite".to_string(), "sqlite::memory:".to_string()),
        Box::new(move |conn: Db| -> IpeTask<()> {
            db_with_transaction(conn.clone(), {
                let __ipe_fn: Box<dyn Fn(Db) -> IpeTask<()> + Send + Sync + 'static> = Box::new(
                    move |txconn: Db| -> IpeTask<()> {
                        task_and_then(
                            db_exec_raw(
                                txconn.clone().clone(),
                                "CREATE TABLE people (name TEXT, age INT)".to_string(),
                            ),
                            Box::new(move |_| {
                                task_and_then(
                                    db_exec_params(
                                        txconn.clone().clone(),
                                        "INSERT INTO people VALUES (?, ?)".to_string(),
                                        (vec![
                                            MainSqlValue::SqlString("Alice".to_string()),
                                            MainSqlValue::SqlInt(30),
                                        ])
                                        .into_iter()
                                        .map(::core::convert::Into::into)
                                        .collect::<Vec<ipe_runtime::db::SqlParam>>(),
                                    ),
                                    Box::new(move |_| {
                                        ({
                                            let j1 = crate::main_report_json(
                                                crate::main_json_int_result(
                                                    "{\"name\":\"Bob\"}"
                                                        .to_string(),
                                                ),
                                            );
                                            ({
                                                let j2 = crate::main_report_json(
                                                    crate::main_json_int_result(
                                                        "{\"name\":\"Cara\",\"age\":25}"
                                                            .to_string(),
                                                    ),
                                                );
                                                ({
                                                    let j3 = crate::main_report_json(
                                                        crate::main_json_string_result(
                                                            "{\"name\":\"Dan\"}"
                                                                .to_string(),
                                                        ),
                                                    );
                                                    ({
                                                        let j4 = crate::main_report_json(
                                                            crate::main_json_string_result(
                                                                "{\"name\":\"Eve\",\"nick\":\"E\"}"
                                                                    .to_string(),
                                                            ),
                                                        );
                                                        task_and_then(
                                                            db_query_decode_params(txconn.clone(), "SELECT name, age FROM people".to_string(), (Vec::<MainSqlValue>::new()).into_iter().map(::core::convert::Into::into).collect::<Vec<ipe_runtime::db::SqlParam>>(), crate::main_row_decoder()),
                                                            Box::new(move |rows: Vec<String>| -> IpeTask<()> {
                                                                io_println(string_join(
                                                                    "\n".to_string(),
                                                                    vec![
                                                                        j1.clone(),
                                                                        j2.clone(),
                                                                        j3.clone(),
                                                                        j4.clone(),
                                                                        string_join(
                                                                            ",".to_string(),
                                                                            rows,
                                                                        ),
                                                                    ],
                                                                ))
                                                            }),
                                                        )
                                                    })
                                                })
                                            })
                                        })
                                    }),
                                )
                            }),
                        )
                    },
                );
                __ipe_fn
            })
        }),
    )
}
