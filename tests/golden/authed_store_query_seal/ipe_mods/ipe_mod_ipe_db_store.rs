use crate::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbStoreColumn {
    Column(RecColTypeName),
}
impl IpeStringify for IpeDbStoreColumn {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreColumn::Column(p0) => {
                format!("Column {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbStoreColumnSpec {
    PrimaryKey(String),
    Serial(String),
    Unique(String),
    DefaultNow(String),
    DefaultText(String, String),
    DefaultInt(String, i64),
    TouchOnUpdate(String),
}
impl IpeStringify for IpeDbStoreColumnSpec {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreColumnSpec::PrimaryKey(p0) => format!(
                "PrimaryKey {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
            IpeDbStoreColumnSpec::Serial(p0) => {
                format!("Serial {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStoreColumnSpec::Unique(p0) => {
                format!("Unique {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStoreColumnSpec::DefaultNow(p0) => format!(
                "DefaultNow {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
            IpeDbStoreColumnSpec::DefaultText(p0, p1) => format!(
                "DefaultText {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
            IpeDbStoreColumnSpec::DefaultInt(p0, p1) => format!(
                "DefaultInt {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
            IpeDbStoreColumnSpec::TouchOnUpdate(p0) => format!(
                "TouchOnUpdate {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbStoreSchemaOp {
    RenameColumn(String, String),
    RenameTable(String, String),
}
impl IpeStringify for IpeDbStoreSchemaOp {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreSchemaOp::RenameColumn(p0, p1) => format!(
                "RenameColumn {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
            IpeDbStoreSchemaOp::RenameTable(p0, p1) => format!(
                "RenameTable {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
        }
    }
}
pub(crate) enum IpeDbStoreStore<T1: 'static> {
    Store(RecCodecCurrentColumnsFrozenColumnsFrozenTableOpsPkSpecsTable<T1>),
}
impl<T1: Clone + 'static> Clone for IpeDbStoreStore<T1> {
    fn clone(&self) -> Self {
        match self {
            IpeDbStoreStore::Store(p0) => IpeDbStoreStore::Store(p0.clone()),
        }
    }
}
impl<T1: IpeStringify + std::fmt::Debug + 'static> IpeStringify for IpeDbStoreStore<T1> {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreStore::Store(_) => format!("Store {}", "<fn>"),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbStoreCompareOp {
    OpEq,
    OpNeq,
    OpGt,
    OpGte,
    OpLt,
    OpLte,
}
impl IpeStringify for IpeDbStoreCompareOp {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreCompareOp::OpEq => "OpEq".to_string(),
            IpeDbStoreCompareOp::OpNeq => "OpNeq".to_string(),
            IpeDbStoreCompareOp::OpGt => "OpGt".to_string(),
            IpeDbStoreCompareOp::OpGte => "OpGte".to_string(),
            IpeDbStoreCompareOp::OpLt => "OpLt".to_string(),
            IpeDbStoreCompareOp::OpLte => "OpLte".to_string(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbStoreCond {
    Compare(IpeDbStoreCompareOp, String, MainSqlValue),
    Like(String, String),
    IsNull(String),
    NotNull(String),
    InList(String, Vec<MainSqlValue>),
    AndList(Box<Vec<IpeDbStoreCond>>),
    OrList(Box<Vec<IpeDbStoreCond>>),
    NotCond(Box<IpeDbStoreCond>),
}
impl IpeStringify for IpeDbStoreCond {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreCond::Compare(p0, p1, p2) => format!(
                "Compare {} {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p2)).dispatch()
            ),
            IpeDbStoreCond::Like(p0, p1) => format!(
                "Like {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
            IpeDbStoreCond::IsNull(p0) => {
                format!("IsNull {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStoreCond::NotNull(p0) => {
                format!("NotNull {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStoreCond::InList(p0, p1) => format!(
                "InList {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
            IpeDbStoreCond::AndList(p0) => {
                format!("AndList {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStoreCond::OrList(p0) => {
                format!("OrList {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStoreCond::NotCond(p0) => {
                format!("NotCond {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
        }
    }
}
pub(crate) enum IpeDbStoreQuery<T1: 'static> {
    Query(RecFragLimOffOrderingsPoisonStore<T1>),
}
impl<T1: Clone + 'static> Clone for IpeDbStoreQuery<T1> {
    fn clone(&self) -> Self {
        match self {
            IpeDbStoreQuery::Query(p0) => IpeDbStoreQuery::Query(p0.clone()),
        }
    }
}
impl<T1: IpeStringify + std::fmt::Debug + 'static> IpeStringify for IpeDbStoreQuery<T1> {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreQuery::Query(_) => format!("Query {}", "<fn>"),
        }
    }
}
pub(crate) enum IpeDbStoreJoined<T1: 'static, T2: 'static> {
    Joined(RecFragKeyAKeyBPoisonStoreAStoreB<T1, T2>),
}
impl<T1: Clone + 'static, T2: Clone + 'static> Clone for IpeDbStoreJoined<T1, T2> {
    fn clone(&self) -> Self {
        match self {
            IpeDbStoreJoined::Joined(p0) => IpeDbStoreJoined::Joined(p0.clone()),
        }
    }
}
impl<T1: IpeStringify + std::fmt::Debug + 'static, T2: IpeStringify + std::fmt::Debug + 'static> IpeStringify
    for IpeDbStoreJoined<T1, T2>
{
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreJoined::Joined(_) => format!("Joined {}", "<fn>"),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbStoreRule {
    OwnerColumn(String),
    PublicRead,
    Immutable(String),
}
impl IpeStringify for IpeDbStoreRule {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreRule::OwnerColumn(p0) => format!(
                "OwnerColumn {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
            IpeDbStoreRule::PublicRead => "PublicRead".to_string(),
            IpeDbStoreRule::Immutable(p0) => format!(
                "Immutable {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbStorePolicy {
    Policy(Vec<IpeDbStoreRule>),
}
impl IpeStringify for IpeDbStorePolicy {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStorePolicy::Policy(p0) => {
                format!("Policy {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
        }
    }
}
pub(crate) enum IpeDbStoreSecured<T1: 'static> {
    Secured(IpeDbStoreStore<T1>, IpeDbStorePolicy),
}
impl<T1: Clone + 'static> Clone for IpeDbStoreSecured<T1> {
    fn clone(&self) -> Self {
        match self {
            IpeDbStoreSecured::Secured(p0, p1) => IpeDbStoreSecured::Secured(p0.clone(), p1.clone()),
        }
    }
}
impl<T1: IpeStringify + std::fmt::Debug + 'static> IpeStringify for IpeDbStoreSecured<T1> {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreSecured::Secured(_, p1) => format!(
                "Secured {} {}",
                "<fn>",
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
        }
    }
}
pub(crate) fn user_ipe_db_store_valid_sql_ident(name: String) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    (basics_not(string_is_empty(name.clone()))
    && string_all(
        {
            let __ipe_fn: Box<dyn Fn(char) -> bool + Send + Sync + 'static> =
                Box::new(crate::user_ipe_db_store_valid_ident_char);
            __ipe_fn
        },
        name,
    ))
}
pub(crate) fn user_ipe_db_store_valid_ident_char(c: char) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    ({
        let code = char_to_code(c);
        (crate::user_ipe_db_store_is_ascii_digit(code)
            || (crate::user_ipe_db_store_is_ascii_upper(code)
                || (crate::user_ipe_db_store_is_ascii_lower(code)
                    || ((code == crate::user_ipe_db_store_underscore_code())
                        || (code == crate::user_ipe_db_store_dot_code())))))
    })
}
pub(crate) fn user_ipe_db_store_is_ascii_digit(code: i64) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    ((code >= 48) && (code <= 57))
}
pub(crate) fn user_ipe_db_store_is_ascii_upper(code: i64) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    ((code >= 65) && (code <= 90))
}
pub(crate) fn user_ipe_db_store_is_ascii_lower(code: i64) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    ((code >= 97) && (code <= 122))
}
pub(crate) fn user_ipe_db_store_underscore_code() -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    static CELL: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    CELL.get_or_init(|| 95).clone()
}
pub(crate) fn user_ipe_db_store_dot_code() -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    static CELL: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    CELL.get_or_init(|| 46).clone()
}
pub(crate) fn user_ipe_db_store_column(name: String, colType: IpeCodecColType) -> IpeDbStoreColumn {
    let _ipe_recursion_guard = crate::recursion_guard();
    IpeDbStoreColumn::Column(RecColTypeName {
        colType: colType,
        name: name,
    })
}
pub(crate) fn user_ipe_db_store_from_codec<T1: Clone>(
    table: String,
    codec: IpeCodecCodec<T1>,
) -> IpeResult<ipe_runtime::error::IpeError, IpeDbStoreStore<T1>> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match crate::user_ipe_codec_shape(codec.clone()) {
        IpeCodecShape::SRecord(cols) => {
            crate::user_ipe_db_store_build_store(
                table,
                list_map_consume(
                    {
                        let __ipe_fn: Box<
                            dyn Fn((String, IpeCodecColType)) -> IpeDbStoreColumn
                                + Send
                                + Sync
                                + 'static,
                        > = Box::new(crate::user_ipe_db_store_column_from_shape);
                        __ipe_fn
                    },
                    cols,
                ),
                codec,
            )
        }
        IpeCodecShape::SScalar(_) => IpeResult::Err(crate::user_ipe_db_store_not_a_record_error()),
        IpeCodecShape::SBlob => IpeResult::Err(crate::user_ipe_db_store_not_a_record_error()),
    }
}
pub(crate) fn user_ipe_db_store_column_from_shape(
    pair: (String, IpeCodecColType),
) -> IpeDbStoreColumn {
    let _ipe_recursion_guard = crate::recursion_guard();
    ({
        let (name, colType) = pair;
        crate::user_ipe_db_store_column(name, colType)
    })
}
pub(crate) fn user_ipe_db_store_build_store<T1: Clone>(
    table: String,
    columns: Vec<IpeDbStoreColumn>,
    codec: IpeCodecCodec<T1>,
) -> IpeResult<ipe_runtime::error::IpeError, IpeDbStoreStore<T1>> {
    let _ipe_recursion_guard = crate::recursion_guard();
    (if basics_not(crate::user_ipe_db_store_valid_sql_ident(table.clone())) {
        IpeResult::Err(
            crate::user_ipe_db_store_invalid_ident_error("table".to_string(), table),
        )
    } else {
        match crate::user_ipe_db_store_first_invalid_column(columns.clone()) {
            IpeMaybe::Just(bad) => IpeResult::Err(
                crate::user_ipe_db_store_invalid_ident_error("column".to_string(), bad),
            ),
            IpeMaybe::Nothing => {
                IpeResult::Ok(IpeDbStoreStore::Store(
                    RecCodecCurrentColumnsFrozenColumnsFrozenTableOpsPkSpecsTable {
                        codec: codec,
                        currentColumns: columns.clone(),
                        frozenColumns: columns,
                        frozenTable: table.clone(),
                        ops: Vec::<IpeDbStoreSchemaOp>::new(),
                        pk: IpeMaybe::Nothing,
                        specs: Vec::<IpeDbStoreColumnSpec>::new(),
                        table: table,
                    },
                ))
            }
        }
    })
}
pub(crate) fn user_ipe_db_store_first_invalid_column(
    columns: Vec<IpeDbStoreColumn>,
) -> IpeMaybe<String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    let mut columns = columns;
    loop {
        match (columns).as_slice() {
            [] => {
                return IpeMaybe::Nothing;
            }
            [first, rest @ ..] => {
                let first = first.clone();
                let rest = rest.to_vec();
                let name = crate::user_ipe_db_store_column_name(first);
                if crate::user_ipe_db_store_valid_sql_ident(name.clone()) {
                    let __tco_0 = rest;
                    columns = __tco_0;
                    continue;
                } else {
                    return IpeMaybe::Just(name);
                }
            }
        }
    }
}
pub(crate) fn user_ipe_db_store_column_name(col: IpeDbStoreColumn) -> String {
    let _ipe_recursion_guard = crate::recursion_guard();
    match col {
        IpeDbStoreColumn::Column(r) => (r).name.clone(),
    }
}
pub(crate) fn user_ipe_db_store_has_column(columns: Vec<IpeDbStoreColumn>, name: String) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    list_any(
        {
            let __ipe_fn: Box<dyn Fn(IpeDbStoreColumn) -> bool + Send + Sync + 'static> =
                Box::new(move |col: IpeDbStoreColumn| -> bool {
                    (crate::user_ipe_db_store_column_name(col) == name.clone())
                });
            __ipe_fn
        },
        columns,
    )
}
pub(crate) fn user_ipe_db_store_always_true() -> ipe_runtime::db::SqlFragment {
    let _ipe_recursion_guard = crate::recursion_guard();
    sql_eq(
        sql_param(MainSqlValue::SqlInt(1)),
        sql_param(MainSqlValue::SqlInt(1)),
    )
}
pub(crate) fn user_ipe_db_store_decode_rows<T1: 'static + Send + Sync + Clone>(
    codec: IpeCodecCodec<T1>,
    rows: Vec<HashMap<String, String>>,
) -> IpeTask<Vec<T1>> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match (rows).as_slice() {
        [] => task_succeed(Vec::<T1>::new()),
        [first, rest @ ..] => {
            let first = first.clone();
            let rest = rest.to_vec();
            match crate::user_ipe_db_codec_codec_from_row(codec.clone(), first) {
                IpeResult::Err(e) => task_fail(e),
                IpeResult::Ok(value) => task_map({
                    let __ipe_fn: Box<dyn Fn(Vec<T1>) -> Vec<T1> + Send + Sync + 'static> = Box::new(move |more: Vec<T1>| -> Vec<T1> { ipe_runtime::list::ipe_list_cons(value.clone(), more) });
                    __ipe_fn
                }, crate::user_ipe_db_store_decode_rows(codec, rest)),
            }
        }
    }
}
pub(crate) fn user_ipe_db_store_owner_column_named(col: String) -> IpeDbStorePolicy {
    let _ipe_recursion_guard = crate::recursion_guard();
    IpeDbStorePolicy::Policy(vec![IpeDbStoreRule::OwnerColumn(col)])
}
pub(crate) fn user_ipe_db_store_secured<T1: Clone>(
    policy: IpeDbStorePolicy,
    store: IpeDbStoreStore<T1>,
) -> IpeResult<ipe_runtime::error::IpeError, IpeDbStoreSecured<T1>> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match store.clone() {
        IpeDbStoreStore::Store(r) => match policy.clone() {
            IpeDbStorePolicy::Policy(rules) => match crate::user_ipe_db_store_first_unknown_policy_column((r).currentColumns.clone(), rules)
            {
                IpeMaybe::Just(bad) => {
                    IpeResult::Err(crate::user_ipe_db_store_unknown_column_error(bad))
                }
                IpeMaybe::Nothing => IpeResult::Ok(IpeDbStoreSecured::Secured(store, policy)),
            },
        },
    }
}
pub(crate) fn user_ipe_db_store_first_unknown_policy_column(
    columns: Vec<IpeDbStoreColumn>,
    rules: Vec<IpeDbStoreRule>,
) -> IpeMaybe<String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    let mut columns = columns;
    let mut rules = rules;
    loop {
        match (rules).as_slice() {
            [] => {
                return IpeMaybe::Nothing;
            }
            [first, rest @ ..] => {
                let first = first.clone();
                let rest = rest.to_vec();
                match crate::user_ipe_db_store_rule_column(first) {
                    IpeMaybe::Just(name) => {
                        if crate::user_ipe_db_store_has_column(columns.clone(), name.clone()) {
                            let __tco_0 = columns;
                            let __tco_1 = rest;
                            columns = __tco_0;
                            rules = __tco_1;
                            continue;
                        } else {
                            return IpeMaybe::Just(name);
                        }
                    }
                    IpeMaybe::Nothing => {
                        let __tco_0 = columns;
                        let __tco_1 = rest;
                        columns = __tco_0;
                        rules = __tco_1;
                        continue;
                    }
                }
            }
        }
    }
}
pub(crate) fn user_ipe_db_store_rule_column(rule: IpeDbStoreRule) -> IpeMaybe<String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match rule {
        IpeDbStoreRule::OwnerColumn(c) => IpeMaybe::Just(c),
        IpeDbStoreRule::Immutable(c) => IpeMaybe::Just(c),
        IpeDbStoreRule::PublicRead => IpeMaybe::Nothing,
    }
}
pub(crate) fn user_ipe_db_store_policy_fragment(
    principal: ipe_runtime::principal::Principal,
    policy: IpeDbStorePolicy,
) -> ipe_runtime::db::SqlFragment {
    let _ipe_recursion_guard = crate::recursion_guard();
    match policy {
        IpeDbStorePolicy::Policy(rules) => list_foldl({
            let __ipe_fn: Box<dyn Fn(IpeDbStoreRule, ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment + Send + Sync + 'static> = Box::new(move |rule: IpeDbStoreRule, acc: ipe_runtime::db::SqlFragment| -> ipe_runtime::db::SqlFragment { sql_and(acc, crate::user_ipe_db_store_rule_fragment(principal.clone(), rule)) });
            __ipe_fn
        }, crate::user_ipe_db_store_always_true(), rules),
    }
}
pub(crate) fn user_ipe_db_store_rule_fragment(
    principal: ipe_runtime::principal::Principal,
    rule: IpeDbStoreRule,
) -> ipe_runtime::db::SqlFragment {
    let _ipe_recursion_guard = crate::recursion_guard();
    match rule {
        IpeDbStoreRule::OwnerColumn(col) => sql_eq(
            sql_column(col),
            sql_param(MainSqlValue::SqlString(principal_subject(principal))),
        ),
        IpeDbStoreRule::Immutable(_) => crate::user_ipe_db_store_always_true(),
        IpeDbStoreRule::PublicRead => crate::user_ipe_db_store_always_true(),
    }
}
pub(crate) fn user_ipe_db_store_all_as<T1: 'static + Send + Sync + Clone>(
    principal: ipe_runtime::principal::Principal,
    conn: Db,
    secured: IpeDbStoreSecured<T1>,
) -> IpeTask<Vec<T1>> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match secured {
        IpeDbStoreSecured::Secured(store, policy) => match store {
            IpeDbStoreStore::Store(r) => {
                task_and_then(
                    db_find_where(conn.clone(), (r.clone()).table.clone(), crate::user_ipe_db_store_policy_fragment(principal, policy)),
                    ({
                        let r = r.clone();
                        {
                            let __ipe_fn: Box<
                                dyn Fn(Vec<HashMap<String, String>>) -> IpeTask<Vec<T1>>
                                    + Send
                                    + Sync
                                    + 'static,
                            > = Box::new(
                                move |rows: Vec<HashMap<String, String>>| -> IpeTask<Vec<T1>> {
                                    crate::user_ipe_db_store_decode_rows(
                                        (r.clone()).codec.clone(),
                                        rows,
                                    )
                                },
                            );
                            __ipe_fn
                        }
                    }),
                )
            }
        },
    }
}
pub(crate) fn user_ipe_db_store_invalid_ident_error(
    kind: String,
    name: String,
) -> ipe_runtime::error::IpeError {
    let _ipe_recursion_guard = crate::recursion_guard();
    ipe_error_invalid_input(string_concat(vec![
        "Ipe.Db.Store: rejected ".to_string(),
        kind,
        " identifier \"".to_string(),
        name,
        "\" — not a valid SQL identifier".to_string(),
    ]))
}
pub(crate) fn user_ipe_db_store_not_a_record_error() -> ipe_runtime::error::IpeError {
    let _ipe_recursion_guard = crate::recursion_guard();
    ipe_error_invalid_input(
        "Ipe.Db.Store: the codec's shape is not a record, so it declares no columns to build a store from"
            .to_string(),
    )
}
pub(crate) fn user_ipe_db_store_unknown_column_error(name: String) -> ipe_runtime::error::IpeError {
    let _ipe_recursion_guard = crate::recursion_guard();
    ipe_error_invalid_input(string_concat(vec![
        "Ipe.Db.Store: \"".to_string(),
        name,
        "\" is not a column of this store — a query may only reference the store's own derived columns"
            .to_string(),
    ]))
}
