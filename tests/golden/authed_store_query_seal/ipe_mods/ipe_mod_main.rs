use crate::*;

pub(crate) fn main_doc_codec() -> IpeCodecCodec<RecAuthorBody> {
    let _ipe_recursion_guard = crate::recursion_guard();
    IpeCodecCodec::Codec(RecEncMkDecShp {
        enc: {
            let __ipe_fn: ::std::sync::Arc<
                dyn Fn(RecAuthorBody) -> JsonVal + Send + Sync + 'static,
            > = ::std::sync::Arc::new(move |codec_rec_4026630916: RecAuthorBody| -> JsonVal {
                json_enc_object(vec![
                    (
                        "author".to_string(),
                        json_enc_string((codec_rec_4026630916.clone()).author.clone()),
                    ),
                    (
                        "body".to_string(),
                        json_enc_string((codec_rec_4026630916).body.clone()),
                    ),
                ])
            });
            __ipe_fn
        },
        mkDec: {
            let __ipe_fn: ::std::sync::Arc<
                dyn Fn(Rec_) -> Decoder<RecAuthorBody> + Send + Sync + 'static,
            > = ::std::sync::Arc::new(move |arg_13: Rec_| -> Decoder<RecAuthorBody> {
                ({
                    let cap_0 = "body".to_string();
                    ({
                        let cap_1 = json_decode_string::<IpeError>();
                        {
                            let __ipe_fn: Box<
                                dyn Fn(Decoder<Box<dyn FnOnce(String) -> RecAuthorBody + Send + 'static>>) -> Decoder<RecAuthorBody>
                                    + Send
                                    + Sync
                                    + 'static,
                            > = Box::new(
                                move |eta_0: Decoder<Box<dyn FnOnce(String) -> RecAuthorBody + Send + 'static>>| -> Decoder<RecAuthorBody> {
                                    decode_pipeline_required(cap_0.clone(), cap_1.clone(), eta_0)
                                },
                            );
                            __ipe_fn
                        }
                    })
                })(
                    ({
                        let cap_0 = "author".to_string();
                        ({
                            let cap_1 = json_decode_string::<IpeError>();
                            {
                                let __ipe_fn: Box<
                                    dyn Fn(Decoder<Box<dyn FnOnce(String) -> Box<dyn FnOnce(String) -> RecAuthorBody + Send + 'static> + Send + 'static>>) -> Decoder<Box<dyn FnOnce(String) -> RecAuthorBody + Send + 'static>>
                                        + Send
                                        + Sync
                                        + 'static,
                                > = Box::new(
                                    move |eta_0: Decoder<Box<dyn FnOnce(String) -> Box<dyn FnOnce(String) -> RecAuthorBody + Send + 'static> + Send + 'static>>| -> Decoder<Box<dyn FnOnce(String) -> RecAuthorBody + Send + 'static>> {
                                        decode_pipeline_required(
                                            cap_0.clone(),
                                            cap_1.clone(),
                                            eta_0,
                                        )
                                    },
                                );
                                __ipe_fn
                            }
                        })
                    })(
                        decode_succeed(curry2(move |author: String, body: String| -> RecAuthorBody { RecAuthorBody { author: author, body: body } })),
                    ),
                )
            });
            __ipe_fn
        },
        shp: IpeCodecShape::SRecord(vec![("author".to_string(), IpeCodecColType::CText), (
            "body".to_string(),
            IpeCodecColType::CText,
        )]),
    })
}
pub(crate) fn main_doc_policy() -> IpeDbStorePolicy {
    let _ipe_recursion_guard = crate::recursion_guard();
    crate::user_ipe_db_store_owner_column_named("author".to_string())
}
pub(crate) fn main_secured_docs() -> IpeResult<
    ipe_runtime::error::IpeError, IpeDbStoreSecured<RecAuthorBody>,
> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match crate::user_ipe_db_store_from_codec("docs".to_string(), crate::main_doc_codec()) {
        IpeResult::Err(e) => IpeResult::Err(e),
        IpeResult::Ok(store) => crate::user_ipe_db_store_secured(crate::main_doc_policy(), store),
    }
}
pub(crate) fn main_handle_my_docs(
    req: ServerRequest,
    principal: ipe_runtime::principal::Principal,
) -> IpeTask<ServerResponse> {
    let _ipe_recursion_guard = crate::recursion_guard();
    task_on_error(
        {
            let __ipe_fn: Box<
                dyn Fn(ipe_runtime::error::IpeError) -> IpeTask<ServerResponse>
                    + Send
                    + Sync
                    + 'static,
            > = Box::new(move |arg_14: ipe_runtime::error::IpeError| -> IpeTask<ServerResponse> {
                task_succeed(server_text("none".to_string()))
            });
            __ipe_fn
        },
        task_and_then(
            db_connect(()),
            Box::new(move |db: Db| -> IpeTask<ServerResponse> {
                match crate::main_secured_docs() {
                    IpeResult::Err(_) => task_succeed(server_text("policy-error".to_string())),
                    IpeResult::Ok(secured) => {
                        task_and_then(
                            crate::user_ipe_db_store_all_as(principal.clone(), db, secured),
                            Box::new(move |docs: Vec<RecAuthorBody>| -> IpeTask<ServerResponse> {
                                task_succeed(server_text(string_join(
                                    "\n".to_string(),
                                    list_map_consume(
                                        {
                                            let __ipe_fn: Box<
                                                dyn Fn(RecAuthorBody) -> String
                                                    + Send
                                                    + Sync
                                                    + 'static,
                                            > = Box::new(
                                                move |ipe_accessor_arg: RecAuthorBody| -> String {
                                                    (ipe_accessor_arg).body.clone()
                                                },
                                            );
                                            __ipe_fn
                                        },
                                        docs,
                                    ),
                                )))
                            }),
                        )
                    }
                }
            }),
        ),
    )
}
pub(crate) fn main_auth_cfg() -> ipe_runtime::server::AuthConfig {
    let _ipe_recursion_guard = crate::recursion_guard();
    server_auth_config(
        secret_from_string(system_getenv_or(
            "SIGNING_KEY".to_string(),
            "this-is-a-32-byte-or-longer-secret-key-value".to_string(),
        )),
        server_token_bearer(),
    )
}
pub(crate) fn ipe_main() -> IpeTask<()> {
    let _ipe_recursion_guard = crate::recursion_guard();
    task_on_error(
        {
            let __ipe_fn: Box<
                dyn Fn(ipe_runtime::error::IpeError) -> IpeTask<()> + Send + Sync + 'static,
            > = Box::new(move |arg_15: ipe_runtime::error::IpeError| -> IpeTask<()> {
                io_println("authed-store-query-seal".to_string())
            });
            __ipe_fn
        },
        task_and_then(
            server_listen(
                8000i64,
                vec![server_get_authed(
                    "/my/docs".to_string(),
                    crate::main_auth_cfg(),
                    {
                        let __ipe_fn: Box<
                            dyn Fn(ServerRequest, ipe_runtime::principal::Principal) -> IpeTask<ServerResponse>
                                + Send
                                + Sync
                                + 'static,
                        > = Box::new(crate::main_handle_my_docs);
                        __ipe_fn
                    },
                )],
            ),
            Box::new(move |arg_16: ()| -> IpeTask<()> {
                io_println("authed-store-query-seal".to_string())
            }),
        ),
    )
}
