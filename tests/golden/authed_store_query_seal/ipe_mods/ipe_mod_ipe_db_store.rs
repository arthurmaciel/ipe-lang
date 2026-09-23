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
pub(crate) enum IpeDbStoreIndexSpec {
    Index(Vec<String>),
    IndexNamed(String, Vec<String>),
}
impl IpeStringify for IpeDbStoreIndexSpec {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreIndexSpec::Index(p0) => {
                format!("Index {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStoreIndexSpec::IndexNamed(p0, p1) => format!(
                "IndexNamed {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
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
pub(crate) enum IpeDbStoreDraft<T1: 'static> {
    Draft(RecCodecCurrentColumnsFrozenColumnsFrozenTableIndexesOpsPkSpecsTable<T1>),
}
impl<T1: Clone + 'static> Clone for IpeDbStoreDraft<T1> {
    fn clone(&self) -> Self {
        match self {
            IpeDbStoreDraft::Draft(p0) => IpeDbStoreDraft::Draft(p0.clone()),
        }
    }
}
impl<T1: IpeStringify + std::fmt::Debug + 'static> IpeStringify for IpeDbStoreDraft<T1> {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreDraft::Draft(_) => format!("Draft {}", "<fn>"),
        }
    }
}
pub(crate) enum IpeDbStoreStore<T1: 'static> {
    Store(RecCodecCurrentColumnsFrozenColumnsFrozenTableIndexesOpsPkSpecsTable<T1>),
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
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbStorePred {
    PAll(Box<Vec<IpeDbStorePred>>),
    PAny(Box<Vec<IpeDbStorePred>>),
    PNotP(Box<IpeDbStorePred>),
    PAlways,
    PNever,
    PMatch(IpeDbStoreCond),
    POwner(String),
    PExists(Box<IpeDbStoreExistsRef>),
}
impl IpeStringify for IpeDbStorePred {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStorePred::PAll(p0) => {
                format!("PAll {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStorePred::PAny(p0) => {
                format!("PAny {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStorePred::PNotP(p0) => {
                format!("PNotP {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStorePred::PAlways => "PAlways".to_string(),
            IpeDbStorePred::PNever => "PNever".to_string(),
            IpeDbStorePred::PMatch(p0) => {
                format!("PMatch {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStorePred::POwner(p0) => {
                format!("POwner {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
            IpeDbStorePred::PExists(p0) => {
                format!("PExists {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbStoreExistsRef {
    ExistsRef(Box<RecOuterColShareColShareColumnsShareReadShareTable>),
}
impl IpeStringify for IpeDbStoreExistsRef {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbStoreExistsRef::ExistsRef(p0) => format!(
                "ExistsRef {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbStorePolicy {
    Policy(RecDeleteImmutablesInsertOwnersReadUpdate),
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
pub(crate) fn user_ipe_db_store_valid_sql_ident_plain(name: String) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    (basics_not(string_is_empty(name.clone()))
    && string_all(
        {
            let __ipe_fn: Box<dyn Fn(char) -> bool + Send + Sync + 'static> =
                Box::new(crate::user_ipe_db_store_plain_ident_char);
            __ipe_fn
        },
        name,
    ))
}
pub(crate) fn user_ipe_db_store_plain_ident_char(c: char) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    ({
        let code = char_to_code(c);
        (crate::user_ipe_db_store_is_ascii_digit(code)
            || (crate::user_ipe_db_store_is_ascii_upper(code)
                || (crate::user_ipe_db_store_is_ascii_lower(code)
                    || (code == crate::user_ipe_db_store_underscore_code()))))
    })
}
pub(crate) fn user_ipe_db_store_is_ascii_digit(code: i64) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    ((code >= 48i64) && (code <= 57i64))
}
pub(crate) fn user_ipe_db_store_is_ascii_upper(code: i64) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    ((code >= 65i64) && (code <= 90i64))
}
pub(crate) fn user_ipe_db_store_is_ascii_lower(code: i64) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    ((code >= 97i64) && (code <= 122i64))
}
pub(crate) fn user_ipe_db_store_underscore_code() -> i64 {
    let _ipe_recursion_guard = crate::recursion_guard();
    static CELL: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    CELL.get_or_init(|| 95i64).clone()
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
) -> IpeResult<ipe_runtime::error::IpeError, IpeDbStoreDraft<T1>> {
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
) -> IpeResult<ipe_runtime::error::IpeError, IpeDbStoreDraft<T1>> {
    let _ipe_recursion_guard = crate::recursion_guard();
    (if basics_not(crate::user_ipe_db_store_valid_sql_ident_plain(
        table.clone(),
    )) {
        IpeResult::Err(
            crate::user_ipe_db_store_invalid_ident_error("table".to_string(), table),
        )
    } else {
        match crate::user_ipe_db_store_first_invalid_column(columns.clone()) {
            IpeMaybe::Just(bad) => IpeResult::Err(
                crate::user_ipe_db_store_invalid_ident_error("column".to_string(), bad),
            ),
            IpeMaybe::Nothing => {
                IpeResult::Ok(IpeDbStoreDraft::Draft(
                    RecCodecCurrentColumnsFrozenColumnsFrozenTableIndexesOpsPkSpecsTable {
                        codec: codec,
                        currentColumns: columns.clone(),
                        frozenColumns: columns,
                        frozenTable: table.clone(),
                        indexes: Vec::<IpeDbStoreIndexSpec>::new(),
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
                if crate::user_ipe_db_store_valid_sql_ident_plain(name.clone()) {
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
pub(crate) fn user_ipe_db_store_public<T1: Clone>(
    draft: IpeDbStoreDraft<T1>,
) -> IpeDbStoreStore<T1> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match draft {
        IpeDbStoreDraft::Draft(r) => {
            IpeDbStoreStore::Store(
                RecCodecCurrentColumnsFrozenColumnsFrozenTableIndexesOpsPkSpecsTable {
                    codec: (r.clone()).codec.clone(),
                    currentColumns: (r.clone()).currentColumns.clone(),
                    frozenColumns: (r.clone()).frozenColumns.clone(),
                    frozenTable: (r.clone()).frozenTable.clone(),
                    indexes: (r.clone()).indexes.clone(),
                    ops: (r.clone()).ops.clone(),
                    pk: (r.clone()).pk.clone(),
                    specs: (r.clone()).specs.clone(),
                    table: (r).table.clone(),
                },
            )
        }
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
                IpeResult::Ok(value) => task_map(
                    {
                        let __ipe_fn: Box<dyn Fn(Vec<T1>) -> Vec<T1> + Send + Sync + 'static> =
                            Box::new(move |more: Vec<T1>| -> Vec<T1> {
                                ipe_runtime::list::ipe_list_cons(value.clone(), more)
                            });
                        __ipe_fn
                    },
                    crate::user_ipe_db_store_decode_rows(codec, rest),
                ),
            }
        }
    }
}
pub(crate) fn user_ipe_db_store_cond_fragment_in(
    columns: Vec<IpeDbStoreColumn>,
    cond: IpeDbStoreCond,
) -> IpeResult<ipe_runtime::error::IpeError, ipe_runtime::db::SqlFragment> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match cond {
        IpeDbStoreCond::Compare(op, col, value) => crate::user_ipe_db_store_check_column(
            columns,
            col.clone(),
            crate::user_ipe_db_store_compare_fragment(op, col, value),
        ),
        IpeDbStoreCond::Like(col, pattern) => {
            crate::user_ipe_db_store_check_column(
                columns,
                col.clone(),
                sql_like(sql_column(col), pattern),
            )
        }
        IpeDbStoreCond::IsNull(col) => {
            crate::user_ipe_db_store_check_column(
                columns,
                col.clone(),
                sql_is_null(sql_column(col)),
            )
        }
        IpeDbStoreCond::NotNull(col) => {
            crate::user_ipe_db_store_check_column(
                columns,
                col.clone(),
                sql_is_not_null(sql_column(col)),
            )
        }
        IpeDbStoreCond::InList(col, values) => crate::user_ipe_db_store_check_column(
            columns,
            col.clone(),
            sql_in_list(sql_column(col), (values).into_iter().map(::core::convert::Into::into).collect::<Vec<ipe_runtime::db::SqlParam>>()),
        ),
        IpeDbStoreCond::AndList(conds) => {
            let conds = *conds;
            crate::user_ipe_db_store_fold_conds(
                columns,
                {
                    let __ipe_fn: Box<
                        dyn Fn(ipe_runtime::db::SqlFragment, ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                            + Send
                            + Sync
                            + 'static,
                    > = Box::new(sql_and);
                    __ipe_fn
                },
                crate::user_ipe_db_store_true_fragment(),
                conds,
            )
        }
        IpeDbStoreCond::OrList(conds) => {
            let conds = *conds;
            crate::user_ipe_db_store_fold_conds(
                columns,
                {
                    let __ipe_fn: Box<
                        dyn Fn(ipe_runtime::db::SqlFragment, ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                            + Send
                            + Sync
                            + 'static,
                    > = Box::new(sql_or);
                    __ipe_fn
                },
                crate::user_ipe_db_store_false_fragment(),
                conds,
            )
        }
        IpeDbStoreCond::NotCond(inner) => {
            let inner = *inner;
            ipe_result_map(
                crate::user_ipe_db_store_cond_fragment_in(columns, inner),
                {
                    let __ipe_fn: Box<
                        dyn Fn(ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                            + Send
                            + Sync
                            + 'static,
                    > = Box::new(sql_not);
                    __ipe_fn
                },
            )
        }
    }
}
pub(crate) fn user_ipe_db_store_check_column(
    columns: Vec<IpeDbStoreColumn>,
    col: String,
    built: ipe_runtime::db::SqlFragment,
) -> IpeResult<ipe_runtime::error::IpeError, ipe_runtime::db::SqlFragment> {
    let _ipe_recursion_guard = crate::recursion_guard();
    (if crate::user_ipe_db_store_has_column(columns, col.clone()) {
        IpeResult::Ok(built)
    } else {
        IpeResult::Err(crate::user_ipe_db_store_unknown_column_error(col))
    })
}
pub(crate) fn user_ipe_db_store_compare_fragment(
    op: IpeDbStoreCompareOp,
    col: String,
    value: MainSqlValue,
) -> ipe_runtime::db::SqlFragment {
    let _ipe_recursion_guard = crate::recursion_guard();
    ({
        let lhs = sql_column(col);
        ({
            let rhs = sql_param(value);
            match op {
                IpeDbStoreCompareOp::OpEq => sql_eq(lhs, rhs),
                IpeDbStoreCompareOp::OpNeq => sql_ne(lhs, rhs),
                IpeDbStoreCompareOp::OpGt => sql_gt(lhs, rhs),
                IpeDbStoreCompareOp::OpGte => sql_gte(lhs, rhs),
                IpeDbStoreCompareOp::OpLt => sql_lt(lhs, rhs),
                IpeDbStoreCompareOp::OpLte => sql_lte(lhs, rhs),
            }
        })
    })
}
pub(crate) fn user_ipe_db_store_fold_conds(
    columns: Vec<IpeDbStoreColumn>,
    join: Box<dyn Fn(ipe_runtime::db::SqlFragment, ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment + Send + Sync + 'static>,
    emptyFragment: ipe_runtime::db::SqlFragment,
    conds: Vec<IpeDbStoreCond>,
) -> IpeResult<ipe_runtime::error::IpeError, ipe_runtime::db::SqlFragment> {
    let _ipe_recursion_guard = crate::recursion_guard();
    ({
        let join = {
            let __ipe_fn: ::std::sync::Arc<
                dyn Fn(ipe_runtime::db::SqlFragment, ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                    + Send
                    + Sync
                    + 'static,
            > = ::std::sync::Arc::new(
                move |eta_0: ipe_runtime::db::SqlFragment, eta_1: ipe_runtime::db::SqlFragment| -> ipe_runtime::db::SqlFragment {
                    (join)(eta_0, eta_1)
                },
            );
            __ipe_fn
        };
        match (conds).as_slice() {
            [] => IpeResult::Ok(emptyFragment),
            [single] => {
                let single = single.clone();
                crate::user_ipe_db_store_cond_fragment_in(columns, single)
            }
            [first, rest @ ..] => {
                let first = first.clone();
                let rest = rest.to_vec();
                match crate::user_ipe_db_store_cond_fragment_in(columns.clone(), first) {
                    IpeResult::Err(e) => IpeResult::Err(e),
                    IpeResult::Ok(head) => {
                        ipe_result_map(
                            crate::user_ipe_db_store_fold_conds(
                                columns,
                                ({
                                    let join = join.clone();
                                    {
                                        let __ipe_fn: Box<
                                            dyn Fn(ipe_runtime::db::SqlFragment, ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                                                + Send
                                                + Sync
                                                + 'static,
                                        > = Box::new(
                                            move |eta_0: ipe_runtime::db::SqlFragment, eta_1: ipe_runtime::db::SqlFragment| -> ipe_runtime::db::SqlFragment {
                                                (join.clone())(eta_0, eta_1)
                                            },
                                        );
                                        __ipe_fn
                                    }
                                }),
                                emptyFragment,
                                rest,
                            ),
                            ({
                                let join = join.clone();
                                {
                                    let __ipe_fn: Box<
                                        dyn Fn(ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                                            + Send
                                            + Sync
                                            + 'static,
                                    > = Box::new(
                                        move |tail: ipe_runtime::db::SqlFragment| -> ipe_runtime::db::SqlFragment {
                                            (join)(head.clone(), tail)
                                        },
                                    );
                                    __ipe_fn
                                }
                            }),
                        )
                    }
                }
            }
        }
    })
}
pub(crate) fn user_ipe_db_store_true_fragment() -> ipe_runtime::db::SqlFragment {
    let _ipe_recursion_guard = crate::recursion_guard();
    sql_eq(
        sql_param(MainSqlValue::SqlInt(1i64)),
        sql_param(MainSqlValue::SqlInt(1i64)),
    )
}
pub(crate) fn user_ipe_db_store_false_fragment() -> ipe_runtime::db::SqlFragment {
    let _ipe_recursion_guard = crate::recursion_guard();
    sql_eq(
        sql_param(MainSqlValue::SqlInt(1i64)),
        sql_param(MainSqlValue::SqlInt(0i64)),
    )
}
pub(crate) fn user_ipe_db_store_exists_in_named<T1: Clone>(
    shareSecured: IpeDbStoreSecured<T1>,
    shareCol: String,
    outerCol: String,
) -> IpeDbStorePred {
    let _ipe_recursion_guard = crate::recursion_guard();
    match shareSecured {
        IpeDbStoreSecured::Secured(shareStore, sharePolicy) => match shareStore {
            IpeDbStoreStore::Store(s) => {
                IpeDbStorePred::PExists(Box::new(IpeDbStoreExistsRef::ExistsRef(Box::new(
                    RecOuterColShareColShareColumnsShareReadShareTable {
                        outerCol: outerCol,
                        shareCol: shareCol,
                        shareColumns: (s.clone()).currentColumns.clone(),
                        shareRead: crate::user_ipe_db_store_recast_pred(
                            crate::user_ipe_db_store_read_pred(sharePolicy),
                        ),
                        shareTable: (s).table.clone(),
                    },
                ))))
            }
        },
    }
}
pub(crate) fn user_ipe_db_store_recast_pred(pred: IpeDbStorePred) -> IpeDbStorePred {
    let _ipe_recursion_guard = crate::recursion_guard();
    match pred {
        IpeDbStorePred::PAll(preds) => {
            let preds = *preds;
            IpeDbStorePred::PAll(Box::new(list_map_consume(
                {
                    let __ipe_fn: Box<
                        dyn Fn(IpeDbStorePred) -> IpeDbStorePred + Send + Sync + 'static,
                    > = Box::new(crate::user_ipe_db_store_recast_pred);
                    __ipe_fn
                },
                preds,
            )))
        }
        IpeDbStorePred::PAny(preds) => {
            let preds = *preds;
            IpeDbStorePred::PAny(Box::new(list_map_consume(
                {
                    let __ipe_fn: Box<
                        dyn Fn(IpeDbStorePred) -> IpeDbStorePred + Send + Sync + 'static,
                    > = Box::new(crate::user_ipe_db_store_recast_pred);
                    __ipe_fn
                },
                preds,
            )))
        }
        IpeDbStorePred::PNotP(inner) => {
            let inner = *inner;
            IpeDbStorePred::PNotP(Box::new(crate::user_ipe_db_store_recast_pred(inner)))
        }
        IpeDbStorePred::PAlways => IpeDbStorePred::PAlways,
        IpeDbStorePred::PNever => IpeDbStorePred::PNever,
        IpeDbStorePred::PMatch(cond) => {
            IpeDbStorePred::PMatch(crate::user_ipe_db_store_recast_cond(cond))
        }
        IpeDbStorePred::POwner(col) => IpeDbStorePred::POwner(col),
        IpeDbStorePred::PExists(ref_) => {
            let ref_ = *ref_;
            IpeDbStorePred::PExists(Box::new(crate::user_ipe_db_store_recast_exists_ref(ref_)))
        }
    }
}
pub(crate) fn user_ipe_db_store_recast_cond(cond: IpeDbStoreCond) -> IpeDbStoreCond {
    let _ipe_recursion_guard = crate::recursion_guard();
    match cond {
        IpeDbStoreCond::Compare(op, col, value) => IpeDbStoreCond::Compare(op, col, value),
        IpeDbStoreCond::Like(col, pattern) => IpeDbStoreCond::Like(col, pattern),
        IpeDbStoreCond::IsNull(col) => IpeDbStoreCond::IsNull(col),
        IpeDbStoreCond::NotNull(col) => IpeDbStoreCond::NotNull(col),
        IpeDbStoreCond::InList(col, values) => IpeDbStoreCond::InList(col, values),
        IpeDbStoreCond::AndList(conds) => {
            let conds = *conds;
            IpeDbStoreCond::AndList(Box::new(list_map_consume(
                {
                    let __ipe_fn: Box<
                        dyn Fn(IpeDbStoreCond) -> IpeDbStoreCond + Send + Sync + 'static,
                    > = Box::new(crate::user_ipe_db_store_recast_cond);
                    __ipe_fn
                },
                conds,
            )))
        }
        IpeDbStoreCond::OrList(conds) => {
            let conds = *conds;
            IpeDbStoreCond::OrList(Box::new(list_map_consume(
                {
                    let __ipe_fn: Box<
                        dyn Fn(IpeDbStoreCond) -> IpeDbStoreCond + Send + Sync + 'static,
                    > = Box::new(crate::user_ipe_db_store_recast_cond);
                    __ipe_fn
                },
                conds,
            )))
        }
        IpeDbStoreCond::NotCond(inner) => {
            let inner = *inner;
            IpeDbStoreCond::NotCond(Box::new(crate::user_ipe_db_store_recast_cond(inner)))
        }
    }
}
pub(crate) fn user_ipe_db_store_recast_exists_ref(
    ref_: IpeDbStoreExistsRef,
) -> IpeDbStoreExistsRef {
    let _ipe_recursion_guard = crate::recursion_guard();
    match ref_ {
        IpeDbStoreExistsRef::ExistsRef(r) => {
            let r = *r;
            IpeDbStoreExistsRef::ExistsRef(Box::new(
                RecOuterColShareColShareColumnsShareReadShareTable {
                    outerCol: (r.clone()).outerCol.clone(),
                    shareCol: (r.clone()).shareCol.clone(),
                    shareColumns: (r.clone()).shareColumns.clone(),
                    shareRead: crate::user_ipe_db_store_recast_pred((r.clone()).shareRead.clone()),
                    shareTable: (r).shareTable.clone(),
                },
            ))
        }
    }
}
pub(crate) fn user_ipe_db_store_pred_columns(pred: IpeDbStorePred) -> Vec<String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match pred {
        IpeDbStorePred::PAll(preds) => {
            let preds = *preds;
            list_concat_map(
                {
                    let __ipe_fn: Box<
                        dyn Fn(IpeDbStorePred) -> Vec<String> + Send + Sync + 'static,
                    > = Box::new(crate::user_ipe_db_store_pred_columns);
                    __ipe_fn
                },
                preds,
            )
        }
        IpeDbStorePred::PAny(preds) => {
            let preds = *preds;
            list_concat_map(
                {
                    let __ipe_fn: Box<
                        dyn Fn(IpeDbStorePred) -> Vec<String> + Send + Sync + 'static,
                    > = Box::new(crate::user_ipe_db_store_pred_columns);
                    __ipe_fn
                },
                preds,
            )
        }
        IpeDbStorePred::PNotP(inner) => {
            let inner = *inner;
            crate::user_ipe_db_store_pred_columns(inner)
        }
        IpeDbStorePred::PAlways => Vec::<String>::new(),
        IpeDbStorePred::PNever => Vec::<String>::new(),
        IpeDbStorePred::PMatch(cond) => crate::user_ipe_db_store_cond_columns(cond),
        IpeDbStorePred::POwner(col) => vec![col],
        IpeDbStorePred::PExists(ref_) => {
            let ref_ = *ref_;
            match ref_ {
                IpeDbStoreExistsRef::ExistsRef(r) => {
                    let r = *r;
                    vec![(r).outerCol.clone()]
                }
            }
        }
    }
}
pub(crate) fn user_ipe_db_store_first_unknown_exists_share_column(
    pred: IpeDbStorePred,
) -> IpeMaybe<String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    let mut pred = pred;
    loop {
        match pred {
            IpeDbStorePred::PAll(preds) => {
                let preds = *preds;
                return crate::user_ipe_db_store_first_unknown_exists_share_column_in(preds);
            }
            IpeDbStorePred::PAny(preds) => {
                let preds = *preds;
                return crate::user_ipe_db_store_first_unknown_exists_share_column_in(preds);
            }
            IpeDbStorePred::PNotP(inner) => {
                let inner = *inner;
                let __tco_0 = inner;
                pred = __tco_0;
                continue;
            }
            IpeDbStorePred::PAlways => {
                return IpeMaybe::Nothing;
            }
            IpeDbStorePred::PNever => {
                return IpeMaybe::Nothing;
            }
            IpeDbStorePred::PMatch(_) => {
                return IpeMaybe::Nothing;
            }
            IpeDbStorePred::POwner(_) => {
                return IpeMaybe::Nothing;
            }
            IpeDbStorePred::PExists(ref_) => {
                let ref_ = *ref_;
                match ref_ {
                    IpeDbStoreExistsRef::ExistsRef(r) => {
                        let r = *r;
                        return crate::user_ipe_db_store_first_unknown_policy_column((r.clone()).shareColumns.clone(), ipe_runtime::list::ipe_list_cons((r.clone()).shareCol.clone(), crate::user_ipe_db_store_pred_columns((r).shareRead.clone())));
                    }
                }
            }
        }
    }
}
pub(crate) fn user_ipe_db_store_first_unknown_exists_share_column_in(
    preds: Vec<IpeDbStorePred>,
) -> IpeMaybe<String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    let mut preds = preds;
    loop {
        match (preds).as_slice() {
            [] => {
                return IpeMaybe::Nothing;
            }
            [first, rest @ ..] => {
                let first = first.clone();
                let rest = rest.to_vec();
                match crate::user_ipe_db_store_first_unknown_exists_share_column(first) {
                    IpeMaybe::Just(bad) => {
                        return IpeMaybe::Just(bad);
                    }
                    IpeMaybe::Nothing => {
                        let __tco_0 = rest;
                        preds = __tco_0;
                        continue;
                    }
                }
            }
        }
    }
}
pub(crate) fn user_ipe_db_store_cond_columns(cond: IpeDbStoreCond) -> Vec<String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match cond {
        IpeDbStoreCond::Compare(_, col, _) => vec![col],
        IpeDbStoreCond::Like(col, _) => vec![col],
        IpeDbStoreCond::IsNull(col) => vec![col],
        IpeDbStoreCond::NotNull(col) => vec![col],
        IpeDbStoreCond::InList(col, _) => vec![col],
        IpeDbStoreCond::AndList(conds) => {
            let conds = *conds;
            list_concat_map(
                {
                    let __ipe_fn: Box<
                        dyn Fn(IpeDbStoreCond) -> Vec<String> + Send + Sync + 'static,
                    > = Box::new(crate::user_ipe_db_store_cond_columns);
                    __ipe_fn
                },
                conds,
            )
        }
        IpeDbStoreCond::OrList(conds) => {
            let conds = *conds;
            list_concat_map(
                {
                    let __ipe_fn: Box<
                        dyn Fn(IpeDbStoreCond) -> Vec<String> + Send + Sync + 'static,
                    > = Box::new(crate::user_ipe_db_store_cond_columns);
                    __ipe_fn
                },
                conds,
            )
        }
        IpeDbStoreCond::NotCond(inner) => {
            let inner = *inner;
            crate::user_ipe_db_store_cond_columns(inner)
        }
    }
}
pub(crate) fn user_ipe_db_store_simplify(pred: IpeDbStorePred) -> IpeDbStorePred {
    let _ipe_recursion_guard = crate::recursion_guard();
    match pred {
        IpeDbStorePred::PAll(preds) => {
            let preds = *preds;
            crate::user_ipe_db_store_simplify_all(
                list_map_consume(
                    {
                        let __ipe_fn: Box<
                            dyn Fn(IpeDbStorePred) -> IpeDbStorePred + Send + Sync + 'static,
                        > = Box::new(crate::user_ipe_db_store_simplify);
                        __ipe_fn
                    },
                    preds,
                ),
            )
        }
        IpeDbStorePred::PAny(preds) => {
            let preds = *preds;
            crate::user_ipe_db_store_simplify_any(
                list_map_consume(
                    {
                        let __ipe_fn: Box<
                            dyn Fn(IpeDbStorePred) -> IpeDbStorePred + Send + Sync + 'static,
                        > = Box::new(crate::user_ipe_db_store_simplify);
                        __ipe_fn
                    },
                    preds,
                ),
            )
        }
        IpeDbStorePred::PNotP(inner) => {
            let inner = *inner;
            crate::user_ipe_db_store_simplify_not(crate::user_ipe_db_store_simplify(inner))
        }
        IpeDbStorePred::PAlways => IpeDbStorePred::PAlways,
        IpeDbStorePred::PNever => IpeDbStorePred::PNever,
        IpeDbStorePred::PMatch(cond) => IpeDbStorePred::PMatch(cond),
        IpeDbStorePred::POwner(col) => IpeDbStorePred::POwner(col),
        IpeDbStorePred::PExists(ref_) => {
            let ref_ = *ref_;
            IpeDbStorePred::PExists(Box::new(ref_))
        }
    }
}
pub(crate) fn user_ipe_db_store_simplify_all(preds: Vec<IpeDbStorePred>) -> IpeDbStorePred {
    let _ipe_recursion_guard = crate::recursion_guard();
    (if list_any(
        {
            let __ipe_fn: Box<dyn Fn(IpeDbStorePred) -> bool + Send + Sync + 'static> =
                Box::new(crate::user_ipe_db_store_pred_is_never);
            __ipe_fn
        },
        preds.clone(),
    ) {
        IpeDbStorePred::PNever
    } else {
        match (crate::user_ipe_db_store_dedupe_preds(list_filter({ let __ipe_fn: Box<dyn Fn(IpeDbStorePred) -> bool + Send + Sync + 'static> = Box::new(move |p: IpeDbStorePred| -> bool { basics_not(crate::user_ipe_db_store_pred_is_always(p)) }); __ipe_fn }, preds))).as_slice()
        {
            [] => IpeDbStorePred::PAlways,
            [single] => {
                let single = single.clone();
                single
            }
            kept => {
                let kept = kept.to_vec();
                IpeDbStorePred::PAll(Box::new(kept))
            }
        }
    })
}
pub(crate) fn user_ipe_db_store_simplify_any(preds: Vec<IpeDbStorePred>) -> IpeDbStorePred {
    let _ipe_recursion_guard = crate::recursion_guard();
    (if list_any(
        {
            let __ipe_fn: Box<dyn Fn(IpeDbStorePred) -> bool + Send + Sync + 'static> =
                Box::new(crate::user_ipe_db_store_pred_is_always);
            __ipe_fn
        },
        preds.clone(),
    ) {
        IpeDbStorePred::PAlways
    } else {
        match (crate::user_ipe_db_store_dedupe_preds(list_filter({ let __ipe_fn: Box<dyn Fn(IpeDbStorePred) -> bool + Send + Sync + 'static> = Box::new(move |p: IpeDbStorePred| -> bool { basics_not(crate::user_ipe_db_store_pred_is_never(p)) }); __ipe_fn }, preds))).as_slice()
        {
            [] => IpeDbStorePred::PNever,
            [single] => {
                let single = single.clone();
                single
            }
            kept => {
                let kept = kept.to_vec();
                IpeDbStorePred::PAny(Box::new(kept))
            }
        }
    })
}
pub(crate) fn user_ipe_db_store_simplify_not(inner: IpeDbStorePred) -> IpeDbStorePred {
    let _ipe_recursion_guard = crate::recursion_guard();
    match inner.clone() {
        IpeDbStorePred::PAlways => IpeDbStorePred::PNever,
        IpeDbStorePred::PNever => IpeDbStorePred::PAlways,
        IpeDbStorePred::PNotP(again) => {
            let again = *again;
            again
        }
        IpeDbStorePred::PAll(_)
        | IpeDbStorePred::PAny(_)
        | IpeDbStorePred::PMatch(_)
        | IpeDbStorePred::POwner(_)
        | IpeDbStorePred::PExists(_) => IpeDbStorePred::PNotP(Box::new(inner)),
    }
}
pub(crate) fn user_ipe_db_store_pred_is_always(pred: IpeDbStorePred) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    match pred {
        IpeDbStorePred::PAlways => true,
        IpeDbStorePred::PNever
        | IpeDbStorePred::PNotP(_)
        | IpeDbStorePred::PAll(_)
        | IpeDbStorePred::PAny(_)
        | IpeDbStorePred::PMatch(_)
        | IpeDbStorePred::POwner(_)
        | IpeDbStorePred::PExists(_) => false,
    }
}
pub(crate) fn user_ipe_db_store_pred_is_never(pred: IpeDbStorePred) -> bool {
    let _ipe_recursion_guard = crate::recursion_guard();
    match pred {
        IpeDbStorePred::PNever => true,
        IpeDbStorePred::PAlways
        | IpeDbStorePred::PNotP(_)
        | IpeDbStorePred::PAll(_)
        | IpeDbStorePred::PAny(_)
        | IpeDbStorePred::PMatch(_)
        | IpeDbStorePred::POwner(_)
        | IpeDbStorePred::PExists(_) => false,
    }
}
pub(crate) fn user_ipe_db_store_dedupe_preds(preds: Vec<IpeDbStorePred>) -> Vec<IpeDbStorePred> {
    let _ipe_recursion_guard = crate::recursion_guard();
    list_foldr(
        {
            let __ipe_fn: Box<
                dyn Fn(IpeDbStorePred, Vec<IpeDbStorePred>) -> Vec<IpeDbStorePred>
                    + Send
                    + Sync
                    + 'static,
            > = Box::new(move |p: IpeDbStorePred, acc: Vec<IpeDbStorePred>| -> Vec<IpeDbStorePred> {
                (if list_member(p.clone(), acc.clone()) {
                    acc
                } else {
                    ipe_runtime::list::ipe_list_cons(p, acc)
                })
            });
            __ipe_fn
        },
        Vec::<IpeDbStorePred>::new(),
        preds,
    )
}
pub(crate) fn user_ipe_db_store_pred_fragment_in(
    principal: ipe_runtime::principal::Principal,
    outerTable: String,
    columns: Vec<IpeDbStoreColumn>,
    pred: IpeDbStorePred,
) -> IpeResult<ipe_runtime::error::IpeError, ipe_runtime::db::SqlFragment> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match pred {
        IpeDbStorePred::PAlways => IpeResult::Ok(crate::user_ipe_db_store_true_fragment()),
        IpeDbStorePred::PNever => IpeResult::Ok(crate::user_ipe_db_store_false_fragment()),
        IpeDbStorePred::PMatch(cond) => crate::user_ipe_db_store_cond_fragment_in(columns, cond),
        IpeDbStorePred::POwner(col) => crate::user_ipe_db_store_check_column(
            columns,
            col.clone(),
            sql_eq(
                sql_column(col),
                sql_param(MainSqlValue::SqlString(principal_subject(principal))),
            ),
        ),
        IpeDbStorePred::PNotP(inner) => {
            let inner = *inner;
            ipe_result_map(
                crate::user_ipe_db_store_pred_fragment_in(principal, outerTable, columns, inner),
                {
                    let __ipe_fn: Box<
                        dyn Fn(ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                            + Send
                            + Sync
                            + 'static,
                    > = Box::new(sql_not);
                    __ipe_fn
                },
            )
        }
        IpeDbStorePred::PAll(preds) => {
            let preds = *preds;
            crate::user_ipe_db_store_fold_preds(
                principal,
                outerTable,
                columns,
                {
                    let __ipe_fn: Box<
                        dyn Fn(ipe_runtime::db::SqlFragment, ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                            + Send
                            + Sync
                            + 'static,
                    > = Box::new(sql_and);
                    __ipe_fn
                },
                crate::user_ipe_db_store_true_fragment(),
                preds,
            )
        }
        IpeDbStorePred::PAny(preds) => {
            let preds = *preds;
            crate::user_ipe_db_store_fold_preds(
                principal,
                outerTable,
                columns,
                {
                    let __ipe_fn: Box<
                        dyn Fn(ipe_runtime::db::SqlFragment, ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                            + Send
                            + Sync
                            + 'static,
                    > = Box::new(sql_or);
                    __ipe_fn
                },
                crate::user_ipe_db_store_false_fragment(),
                preds,
            )
        }
        IpeDbStorePred::PExists(ref_) => {
            let ref_ = *ref_;
            crate::user_ipe_db_store_exists_fragment(principal, outerTable, ref_)
        }
    }
}
pub(crate) fn user_ipe_db_store_exists_fragment(
    principal: ipe_runtime::principal::Principal,
    outerTable: String,
    ref_: IpeDbStoreExistsRef,
) -> IpeResult<ipe_runtime::error::IpeError, ipe_runtime::db::SqlFragment> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match ref_ {
        IpeDbStoreExistsRef::ExistsRef(r) => {
            let r = *r;
            (if basics_not(crate::user_ipe_db_store_has_column(
                (r.clone()).shareColumns.clone(),
                (r.clone()).shareCol.clone(),
            )) {
                IpeResult::Err(
                    crate::user_ipe_db_store_unknown_column_error((r).shareCol.clone()),
                )
            } else {
                ({
                    let qualifiedShareCol = string_concat(vec![
                        (r.clone()).shareTable.clone(),
                        ".".to_string(),
                        (r.clone()).shareCol.clone(),
                    ]);
                    ({
                        let qualifiedOuterCol = string_concat(vec![
                            outerTable,
                            ".".to_string(),
                            (r.clone()).outerCol.clone(),
                        ]);
                        ({
                            let correlation = sql_eq(
                                sql_column(qualifiedShareCol),
                                sql_column(qualifiedOuterCol),
                            );
                            match crate::user_ipe_db_store_pred_fragment_in(principal, (r.clone()).shareTable.clone(), (r.clone()).shareColumns.clone(), crate::user_ipe_db_store_simplify((r.clone()).shareRead.clone()))
                            {
                                IpeResult::Err(e) => IpeResult::Err(e),
                                IpeResult::Ok(shareReadFrag) => IpeResult::Ok(
                                    sql_exists(
                                        (r).shareTable.clone(),
                                        sql_and(correlation, shareReadFrag),
                                    ),
                                ),
                            }
                        })
                    })
                })
            })
        }
    }
}
pub(crate) fn user_ipe_db_store_fold_preds(
    principal: ipe_runtime::principal::Principal,
    outerTable: String,
    columns: Vec<IpeDbStoreColumn>,
    join: Box<dyn Fn(ipe_runtime::db::SqlFragment, ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment + Send + Sync + 'static>,
    emptyFragment: ipe_runtime::db::SqlFragment,
    preds: Vec<IpeDbStorePred>,
) -> IpeResult<ipe_runtime::error::IpeError, ipe_runtime::db::SqlFragment> {
    let _ipe_recursion_guard = crate::recursion_guard();
    ({
        let join = {
            let __ipe_fn: ::std::sync::Arc<
                dyn Fn(ipe_runtime::db::SqlFragment, ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                    + Send
                    + Sync
                    + 'static,
            > = ::std::sync::Arc::new(
                move |eta_0: ipe_runtime::db::SqlFragment, eta_1: ipe_runtime::db::SqlFragment| -> ipe_runtime::db::SqlFragment {
                    (join)(eta_0, eta_1)
                },
            );
            __ipe_fn
        };
        match (preds).as_slice() {
            [] => IpeResult::Ok(emptyFragment),
            [single] => {
                let single = single.clone();
                crate::user_ipe_db_store_pred_fragment_in(principal, outerTable, columns, single)
            }
            [first, rest @ ..] => {
                let first = first.clone();
                let rest = rest.to_vec();
                match crate::user_ipe_db_store_pred_fragment_in(principal.clone(), outerTable.clone(), columns.clone(), first)
                {
                    IpeResult::Err(e) => IpeResult::Err(e),
                    IpeResult::Ok(head) => {
                        ipe_result_map(
                            crate::user_ipe_db_store_fold_preds(
                                principal,
                                outerTable,
                                columns,
                                ({
                                    let join = join.clone();
                                    {
                                        let __ipe_fn: Box<
                                            dyn Fn(ipe_runtime::db::SqlFragment, ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                                                + Send
                                                + Sync
                                                + 'static,
                                        > = Box::new(
                                            move |eta_0: ipe_runtime::db::SqlFragment, eta_1: ipe_runtime::db::SqlFragment| -> ipe_runtime::db::SqlFragment {
                                                (join.clone())(eta_0, eta_1)
                                            },
                                        );
                                        __ipe_fn
                                    }
                                }),
                                emptyFragment,
                                rest,
                            ),
                            ({
                                let join = join.clone();
                                {
                                    let __ipe_fn: Box<
                                        dyn Fn(ipe_runtime::db::SqlFragment) -> ipe_runtime::db::SqlFragment
                                            + Send
                                            + Sync
                                            + 'static,
                                    > = Box::new(
                                        move |tail: ipe_runtime::db::SqlFragment| -> ipe_runtime::db::SqlFragment {
                                            (join)(head.clone(), tail)
                                        },
                                    );
                                    __ipe_fn
                                }
                            }),
                        )
                    }
                }
            }
        }
    })
}
pub(crate) fn user_ipe_db_store_deny_all() -> IpeDbStorePolicy {
    let _ipe_recursion_guard = crate::recursion_guard();
    IpeDbStorePolicy::Policy(
        RecDeleteImmutablesInsertOwnersReadUpdate {
            delete: IpeDbStorePred::PNever,
            immutables: Vec::<String>::new(),
            insert: IpeDbStorePred::PNever,
            owners: Vec::<String>::new(),
            read: IpeDbStorePred::PNever,
            update: IpeDbStorePred::PNever,
        },
    )
}
pub(crate) fn user_ipe_db_store_read_only(p: IpeDbStorePred) -> IpeDbStorePolicy {
    let _ipe_recursion_guard = crate::recursion_guard();
    match crate::user_ipe_db_store_deny_all() {
        IpeDbStorePolicy::Policy(r) => {
            IpeDbStorePolicy::Policy(
                RecDeleteImmutablesInsertOwnersReadUpdate {
                    delete: (r.clone()).delete.clone(),
                    immutables: (r.clone()).immutables.clone(),
                    insert: (r.clone()).insert.clone(),
                    owners: (r.clone()).owners.clone(),
                    read: p,
                    update: (r).update.clone(),
                },
            )
        }
    }
}
pub(crate) fn user_ipe_db_store_owner_column_named(col: String) -> IpeDbStorePolicy {
    let _ipe_recursion_guard = crate::recursion_guard();
    match crate::user_ipe_db_store_deny_all() {
        IpeDbStorePolicy::Policy(r) => {
            IpeDbStorePolicy::Policy(
                RecDeleteImmutablesInsertOwnersReadUpdate {
                    delete: IpeDbStorePred::POwner(col.clone()),
                    immutables: (r).immutables.clone(),
                    insert: IpeDbStorePred::POwner(col.clone()),
                    owners: vec![col.clone()],
                    read: IpeDbStorePred::POwner(col.clone()),
                    update: IpeDbStorePred::POwner(col),
                },
            )
        }
    }
}
pub(crate) fn user_ipe_db_store_secured<T1: Clone>(
    policy: IpeDbStorePolicy,
    draft: IpeDbStoreDraft<T1>,
) -> IpeResult<ipe_runtime::error::IpeError, IpeDbStoreSecured<T1>> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match draft.clone() {
        IpeDbStoreDraft::Draft(r) => match crate::user_ipe_db_store_first_unknown_policy_column((r).currentColumns.clone(), crate::user_ipe_db_store_policy_columns(policy.clone()))
        {
            IpeMaybe::Just(bad) => {
                IpeResult::Err(crate::user_ipe_db_store_unknown_column_error(bad))
            }
            IpeMaybe::Nothing => match crate::user_ipe_db_store_first_unknown_policy_exists_share_column(policy.clone())
            {
                IpeMaybe::Just(bad) => {
                    IpeResult::Err(crate::user_ipe_db_store_unknown_column_error(bad))
                }
                IpeMaybe::Nothing => IpeResult::Ok(IpeDbStoreSecured::Secured(
                    crate::user_ipe_db_store_public(draft),
                    policy,
                )),
            },
        },
    }
}
pub(crate) fn user_ipe_db_store_first_unknown_policy_exists_share_column(
    policy: IpeDbStorePolicy,
) -> IpeMaybe<String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match policy {
        IpeDbStorePolicy::Policy(r) => crate::user_ipe_db_store_first_unknown_exists_share_column_in(
            vec![
                (r.clone()).read.clone(),
                (r.clone()).insert.clone(),
                (r.clone()).update.clone(),
                (r).delete.clone(),
            ],
        ),
    }
}
pub(crate) fn user_ipe_db_store_policy_columns(policy: IpeDbStorePolicy) -> Vec<String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match policy {
        IpeDbStorePolicy::Policy(r) => list_concat(vec![
            crate::user_ipe_db_store_pred_columns((r.clone()).read.clone()),
            crate::user_ipe_db_store_pred_columns((r.clone()).insert.clone()),
            crate::user_ipe_db_store_pred_columns((r.clone()).update.clone()),
            crate::user_ipe_db_store_pred_columns((r.clone()).delete.clone()),
            (r.clone()).owners.clone(),
            (r).immutables.clone(),
        ]),
    }
}
pub(crate) fn user_ipe_db_store_first_unknown_policy_column(
    columns: Vec<IpeDbStoreColumn>,
    names: Vec<String>,
) -> IpeMaybe<String> {
    let _ipe_recursion_guard = crate::recursion_guard();
    let mut columns = columns;
    let mut names = names;
    loop {
        match (names).as_slice() {
            [] => {
                return IpeMaybe::Nothing;
            }
            [name, rest @ ..] => {
                let name = name.clone();
                let rest = rest.to_vec();
                if crate::user_ipe_db_store_has_column(columns.clone(), name.clone()) {
                    let __tco_0 = columns;
                    let __tco_1 = rest;
                    columns = __tco_0;
                    names = __tco_1;
                    continue;
                } else {
                    return IpeMaybe::Just(name);
                }
            }
        }
    }
}
pub(crate) fn user_ipe_db_store_policy_fragment<
    T1: Clone,
    FN1: Fn(IpeDbStorePolicy) -> IpeDbStorePred + Send + Sync + 'static,
>(
    principal: ipe_runtime::principal::Principal,
    op: FN1,
    store: IpeDbStoreStore<T1>,
    policy: IpeDbStorePolicy,
) -> ipe_runtime::db::SqlFragment {
    let _ipe_recursion_guard = crate::recursion_guard();
    match store {
        IpeDbStoreStore::Store(s) => match crate::user_ipe_db_store_pred_fragment_in(principal, (s.clone()).table.clone(), (s).currentColumns.clone(), crate::user_ipe_db_store_simplify((op)(policy)))
        {
            IpeResult::Ok(fragment) => fragment,
            IpeResult::Err(_) => crate::user_ipe_db_store_false_fragment(),
        },
    }
}
pub(crate) fn user_ipe_db_store_read_pred(policy: IpeDbStorePolicy) -> IpeDbStorePred {
    let _ipe_recursion_guard = crate::recursion_guard();
    match policy {
        IpeDbStorePolicy::Policy(r) => (r).read.clone(),
    }
}
pub(crate) fn user_ipe_db_store_all_as<T1: 'static + Send + Sync + Clone>(
    principal: ipe_runtime::principal::Principal,
    conn: Db,
    secured: IpeDbStoreSecured<T1>,
) -> IpeTask<Vec<T1>> {
    let _ipe_recursion_guard = crate::recursion_guard();
    match secured {
        IpeDbStoreSecured::Secured(store, policy) => match store.clone() {
            IpeDbStoreStore::Store(r) => {
                task_and_then(
                    db_find_where(conn.clone(), (r.clone()).table.clone(), crate::user_ipe_db_store_policy_fragment(principal, { let __ipe_fn: Box<dyn Fn(IpeDbStorePolicy) -> IpeDbStorePred + Send + Sync + 'static> = Box::new(crate::user_ipe_db_store_read_pred); __ipe_fn }, store, policy)),
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
