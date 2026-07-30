use crate::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeMoneyCurrency {
    USD,
    EUR,
    GBP,
    JPY,
    CHF,
    AUD,
    CAD,
    NZD,
    SEK,
    NOK,
    DKK,
    CNY,
    HKD,
    SGD,
    KRW,
    TWD,
    INR,
    THB,
    MYR,
    IDR,
    PHP,
    VND,
    BRL,
    MXN,
    ARS,
    CLP,
    ZAR,
    TRY,
    RUB,
    UAH,
    PLN,
    CZK,
    HUF,
    RON,
    BGN,
    AED,
    SAR,
    QAR,
    KWD,
    BHD,
    OMR,
    JOD,
    ILS,
    EGP,
    NGN,
    KES,
    GHS,
    MAD,
    TND,
    DZD,
    PKR,
    BDT,
    LKR,
    NPR,
    BTC,
    ETH,
    USDT,
    USDC,
    CurrencyRaw(String),
}
impl IpeStringify for IpeMoneyCurrency {
    fn ipe_show(&self) -> String {
        match self {
            IpeMoneyCurrency::USD => "USD".to_string(),
            IpeMoneyCurrency::EUR => "EUR".to_string(),
            IpeMoneyCurrency::GBP => "GBP".to_string(),
            IpeMoneyCurrency::JPY => "JPY".to_string(),
            IpeMoneyCurrency::CHF => "CHF".to_string(),
            IpeMoneyCurrency::AUD => "AUD".to_string(),
            IpeMoneyCurrency::CAD => "CAD".to_string(),
            IpeMoneyCurrency::NZD => "NZD".to_string(),
            IpeMoneyCurrency::SEK => "SEK".to_string(),
            IpeMoneyCurrency::NOK => "NOK".to_string(),
            IpeMoneyCurrency::DKK => "DKK".to_string(),
            IpeMoneyCurrency::CNY => "CNY".to_string(),
            IpeMoneyCurrency::HKD => "HKD".to_string(),
            IpeMoneyCurrency::SGD => "SGD".to_string(),
            IpeMoneyCurrency::KRW => "KRW".to_string(),
            IpeMoneyCurrency::TWD => "TWD".to_string(),
            IpeMoneyCurrency::INR => "INR".to_string(),
            IpeMoneyCurrency::THB => "THB".to_string(),
            IpeMoneyCurrency::MYR => "MYR".to_string(),
            IpeMoneyCurrency::IDR => "IDR".to_string(),
            IpeMoneyCurrency::PHP => "PHP".to_string(),
            IpeMoneyCurrency::VND => "VND".to_string(),
            IpeMoneyCurrency::BRL => "BRL".to_string(),
            IpeMoneyCurrency::MXN => "MXN".to_string(),
            IpeMoneyCurrency::ARS => "ARS".to_string(),
            IpeMoneyCurrency::CLP => "CLP".to_string(),
            IpeMoneyCurrency::ZAR => "ZAR".to_string(),
            IpeMoneyCurrency::TRY => "TRY".to_string(),
            IpeMoneyCurrency::RUB => "RUB".to_string(),
            IpeMoneyCurrency::UAH => "UAH".to_string(),
            IpeMoneyCurrency::PLN => "PLN".to_string(),
            IpeMoneyCurrency::CZK => "CZK".to_string(),
            IpeMoneyCurrency::HUF => "HUF".to_string(),
            IpeMoneyCurrency::RON => "RON".to_string(),
            IpeMoneyCurrency::BGN => "BGN".to_string(),
            IpeMoneyCurrency::AED => "AED".to_string(),
            IpeMoneyCurrency::SAR => "SAR".to_string(),
            IpeMoneyCurrency::QAR => "QAR".to_string(),
            IpeMoneyCurrency::KWD => "KWD".to_string(),
            IpeMoneyCurrency::BHD => "BHD".to_string(),
            IpeMoneyCurrency::OMR => "OMR".to_string(),
            IpeMoneyCurrency::JOD => "JOD".to_string(),
            IpeMoneyCurrency::ILS => "ILS".to_string(),
            IpeMoneyCurrency::EGP => "EGP".to_string(),
            IpeMoneyCurrency::NGN => "NGN".to_string(),
            IpeMoneyCurrency::KES => "KES".to_string(),
            IpeMoneyCurrency::GHS => "GHS".to_string(),
            IpeMoneyCurrency::MAD => "MAD".to_string(),
            IpeMoneyCurrency::TND => "TND".to_string(),
            IpeMoneyCurrency::DZD => "DZD".to_string(),
            IpeMoneyCurrency::PKR => "PKR".to_string(),
            IpeMoneyCurrency::BDT => "BDT".to_string(),
            IpeMoneyCurrency::LKR => "LKR".to_string(),
            IpeMoneyCurrency::NPR => "NPR".to_string(),
            IpeMoneyCurrency::BTC => "BTC".to_string(),
            IpeMoneyCurrency::ETH => "ETH".to_string(),
            IpeMoneyCurrency::USDT => "USDT".to_string(),
            IpeMoneyCurrency::USDC => "USDC".to_string(),
            IpeMoneyCurrency::CurrencyRaw(p0) => format!(
                "CurrencyRaw {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeMoneyMoney {
    Money(ipe_runtime::decimal::Decimal, IpeMoneyCurrency),
}
impl IpeStringify for IpeMoneyMoney {
    fn ipe_show(&self) -> String {
        match self {
            IpeMoneyMoney::Money(p0, p1) => format!(
                "Money {} {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch(),
                (&ipe_runtime::stringify::Wrap(p1)).dispatch()
            ),
        }
    }
}
pub(crate) fn user_ipe_money_from_minor(c: IpeMoneyCurrency, minor: i64) -> IpeMoneyMoney {
    IpeMoneyMoney::Money(
        decimal_from_minor(crate::user_ipe_money_minor_units(c.clone()), minor),
        c,
    )
}
pub(crate) fn user_ipe_money_from_major(c: IpeMoneyCurrency, n: i64) -> IpeMoneyMoney {
    IpeMoneyMoney::Money(decimal_from_int(n), c)
}
pub(crate) fn user_ipe_money_from_string(
    c: IpeMoneyCurrency,
    s: String,
) -> IpeResult<ipe_runtime::error::IpeError, IpeMoneyMoney> {
    match decimal_from_string::<IpeError>(s) {
        IpeResult::Ok(d) => IpeResult::Ok(IpeMoneyMoney::Money(d, c)),
        IpeResult::Err(e) => IpeResult::Err(e),
    }
}
pub(crate) fn user_ipe_money_zero(c: IpeMoneyCurrency) -> IpeMoneyMoney {
    IpeMoneyMoney::Money(decimal_zero(), c)
}
pub(crate) fn user_ipe_money_zero_of(m: IpeMoneyMoney) -> IpeMoneyMoney {
    IpeMoneyMoney::Money(decimal_zero(), crate::user_ipe_money_currency(m))
}
pub(crate) fn user_ipe_money_amount(m: IpeMoneyMoney) -> ipe_runtime::decimal::Decimal {
    match m {
        IpeMoneyMoney::Money(d, _) => d,
    }
}
pub(crate) fn user_ipe_money_currency(m: IpeMoneyMoney) -> IpeMoneyCurrency {
    match m {
        IpeMoneyMoney::Money(_, c) => c,
    }
}
pub(crate) fn user_ipe_money_currency_code(c: IpeMoneyCurrency) -> String {
    match c {
        IpeMoneyCurrency::USD => "USD".to_string(),
        IpeMoneyCurrency::EUR => "EUR".to_string(),
        IpeMoneyCurrency::GBP => "GBP".to_string(),
        IpeMoneyCurrency::JPY => "JPY".to_string(),
        IpeMoneyCurrency::CHF => "CHF".to_string(),
        IpeMoneyCurrency::AUD => "AUD".to_string(),
        IpeMoneyCurrency::CAD => "CAD".to_string(),
        IpeMoneyCurrency::NZD => "NZD".to_string(),
        IpeMoneyCurrency::SEK => "SEK".to_string(),
        IpeMoneyCurrency::NOK => "NOK".to_string(),
        IpeMoneyCurrency::DKK => "DKK".to_string(),
        IpeMoneyCurrency::CNY => "CNY".to_string(),
        IpeMoneyCurrency::HKD => "HKD".to_string(),
        IpeMoneyCurrency::SGD => "SGD".to_string(),
        IpeMoneyCurrency::KRW => "KRW".to_string(),
        IpeMoneyCurrency::TWD => "TWD".to_string(),
        IpeMoneyCurrency::INR => "INR".to_string(),
        IpeMoneyCurrency::THB => "THB".to_string(),
        IpeMoneyCurrency::MYR => "MYR".to_string(),
        IpeMoneyCurrency::IDR => "IDR".to_string(),
        IpeMoneyCurrency::PHP => "PHP".to_string(),
        IpeMoneyCurrency::VND => "VND".to_string(),
        IpeMoneyCurrency::BRL => "BRL".to_string(),
        IpeMoneyCurrency::MXN => "MXN".to_string(),
        IpeMoneyCurrency::ARS => "ARS".to_string(),
        IpeMoneyCurrency::CLP => "CLP".to_string(),
        IpeMoneyCurrency::ZAR => "ZAR".to_string(),
        IpeMoneyCurrency::TRY => "TRY".to_string(),
        IpeMoneyCurrency::RUB => "RUB".to_string(),
        IpeMoneyCurrency::UAH => "UAH".to_string(),
        IpeMoneyCurrency::PLN => "PLN".to_string(),
        IpeMoneyCurrency::CZK => "CZK".to_string(),
        IpeMoneyCurrency::HUF => "HUF".to_string(),
        IpeMoneyCurrency::RON => "RON".to_string(),
        IpeMoneyCurrency::BGN => "BGN".to_string(),
        IpeMoneyCurrency::AED => "AED".to_string(),
        IpeMoneyCurrency::SAR => "SAR".to_string(),
        IpeMoneyCurrency::QAR => "QAR".to_string(),
        IpeMoneyCurrency::KWD => "KWD".to_string(),
        IpeMoneyCurrency::BHD => "BHD".to_string(),
        IpeMoneyCurrency::OMR => "OMR".to_string(),
        IpeMoneyCurrency::JOD => "JOD".to_string(),
        IpeMoneyCurrency::ILS => "ILS".to_string(),
        IpeMoneyCurrency::EGP => "EGP".to_string(),
        IpeMoneyCurrency::NGN => "NGN".to_string(),
        IpeMoneyCurrency::KES => "KES".to_string(),
        IpeMoneyCurrency::GHS => "GHS".to_string(),
        IpeMoneyCurrency::MAD => "MAD".to_string(),
        IpeMoneyCurrency::TND => "TND".to_string(),
        IpeMoneyCurrency::DZD => "DZD".to_string(),
        IpeMoneyCurrency::PKR => "PKR".to_string(),
        IpeMoneyCurrency::BDT => "BDT".to_string(),
        IpeMoneyCurrency::LKR => "LKR".to_string(),
        IpeMoneyCurrency::NPR => "NPR".to_string(),
        IpeMoneyCurrency::BTC => "BTC".to_string(),
        IpeMoneyCurrency::ETH => "ETH".to_string(),
        IpeMoneyCurrency::USDT => "USDT".to_string(),
        IpeMoneyCurrency::USDC => "USDC".to_string(),
        IpeMoneyCurrency::CurrencyRaw(s) => s,
    }
}
pub(crate) fn user_ipe_money_minor_units(c: IpeMoneyCurrency) -> i64 {
    money_minor_units(crate::user_ipe_money_currency_code(c))
}
pub(crate) fn user_ipe_money_symbol(c: IpeMoneyCurrency) -> String {
    money_symbol(crate::user_ipe_money_currency_code(c))
}
pub(crate) fn user_ipe_money_currency_name(c: IpeMoneyCurrency) -> String {
    money_currency_name(crate::user_ipe_money_currency_code(c))
}
pub(crate) fn user_ipe_money_known_currency(c: IpeMoneyCurrency) -> bool {
    match c {
        IpeMoneyCurrency::CurrencyRaw(_) => false,
        _ => true,
    }
}
pub(crate) fn user_ipe_money_is_known_code(code: String) -> bool {
    money_is_known_currency(code)
}
pub(crate) fn user_ipe_money_parse_currency(code: String) -> IpeMaybe<IpeMoneyCurrency> {
    match (string_to_upper(string_trim(code))).as_str() {
        "USD" => IpeMaybe::Just(IpeMoneyCurrency::USD),
        "EUR" => IpeMaybe::Just(IpeMoneyCurrency::EUR),
        "GBP" => IpeMaybe::Just(IpeMoneyCurrency::GBP),
        "JPY" => IpeMaybe::Just(IpeMoneyCurrency::JPY),
        "CHF" => IpeMaybe::Just(IpeMoneyCurrency::CHF),
        "AUD" => IpeMaybe::Just(IpeMoneyCurrency::AUD),
        "CAD" => IpeMaybe::Just(IpeMoneyCurrency::CAD),
        "NZD" => IpeMaybe::Just(IpeMoneyCurrency::NZD),
        "SEK" => IpeMaybe::Just(IpeMoneyCurrency::SEK),
        "NOK" => IpeMaybe::Just(IpeMoneyCurrency::NOK),
        "DKK" => IpeMaybe::Just(IpeMoneyCurrency::DKK),
        "CNY" => IpeMaybe::Just(IpeMoneyCurrency::CNY),
        "HKD" => IpeMaybe::Just(IpeMoneyCurrency::HKD),
        "SGD" => IpeMaybe::Just(IpeMoneyCurrency::SGD),
        "KRW" => IpeMaybe::Just(IpeMoneyCurrency::KRW),
        "TWD" => IpeMaybe::Just(IpeMoneyCurrency::TWD),
        "INR" => IpeMaybe::Just(IpeMoneyCurrency::INR),
        "THB" => IpeMaybe::Just(IpeMoneyCurrency::THB),
        "MYR" => IpeMaybe::Just(IpeMoneyCurrency::MYR),
        "IDR" => IpeMaybe::Just(IpeMoneyCurrency::IDR),
        "PHP" => IpeMaybe::Just(IpeMoneyCurrency::PHP),
        "VND" => IpeMaybe::Just(IpeMoneyCurrency::VND),
        "BRL" => IpeMaybe::Just(IpeMoneyCurrency::BRL),
        "MXN" => IpeMaybe::Just(IpeMoneyCurrency::MXN),
        "ARS" => IpeMaybe::Just(IpeMoneyCurrency::ARS),
        "CLP" => IpeMaybe::Just(IpeMoneyCurrency::CLP),
        "ZAR" => IpeMaybe::Just(IpeMoneyCurrency::ZAR),
        "TRY" => IpeMaybe::Just(IpeMoneyCurrency::TRY),
        "RUB" => IpeMaybe::Just(IpeMoneyCurrency::RUB),
        "UAH" => IpeMaybe::Just(IpeMoneyCurrency::UAH),
        "PLN" => IpeMaybe::Just(IpeMoneyCurrency::PLN),
        "CZK" => IpeMaybe::Just(IpeMoneyCurrency::CZK),
        "HUF" => IpeMaybe::Just(IpeMoneyCurrency::HUF),
        "RON" => IpeMaybe::Just(IpeMoneyCurrency::RON),
        "BGN" => IpeMaybe::Just(IpeMoneyCurrency::BGN),
        "AED" => IpeMaybe::Just(IpeMoneyCurrency::AED),
        "SAR" => IpeMaybe::Just(IpeMoneyCurrency::SAR),
        "QAR" => IpeMaybe::Just(IpeMoneyCurrency::QAR),
        "KWD" => IpeMaybe::Just(IpeMoneyCurrency::KWD),
        "BHD" => IpeMaybe::Just(IpeMoneyCurrency::BHD),
        "OMR" => IpeMaybe::Just(IpeMoneyCurrency::OMR),
        "JOD" => IpeMaybe::Just(IpeMoneyCurrency::JOD),
        "ILS" => IpeMaybe::Just(IpeMoneyCurrency::ILS),
        "EGP" => IpeMaybe::Just(IpeMoneyCurrency::EGP),
        "NGN" => IpeMaybe::Just(IpeMoneyCurrency::NGN),
        "KES" => IpeMaybe::Just(IpeMoneyCurrency::KES),
        "GHS" => IpeMaybe::Just(IpeMoneyCurrency::GHS),
        "MAD" => IpeMaybe::Just(IpeMoneyCurrency::MAD),
        "TND" => IpeMaybe::Just(IpeMoneyCurrency::TND),
        "DZD" => IpeMaybe::Just(IpeMoneyCurrency::DZD),
        "PKR" => IpeMaybe::Just(IpeMoneyCurrency::PKR),
        "BDT" => IpeMaybe::Just(IpeMoneyCurrency::BDT),
        "LKR" => IpeMaybe::Just(IpeMoneyCurrency::LKR),
        "NPR" => IpeMaybe::Just(IpeMoneyCurrency::NPR),
        "BTC" => IpeMaybe::Just(IpeMoneyCurrency::BTC),
        "ETH" => IpeMaybe::Just(IpeMoneyCurrency::ETH),
        "USDT" => IpeMaybe::Just(IpeMoneyCurrency::USDT),
        "USDC" => IpeMaybe::Just(IpeMoneyCurrency::USDC),
        _ => IpeMaybe::Nothing,
    }
}
pub(crate) fn user_ipe_money_add(
    a: IpeMoneyMoney,
    b: IpeMoneyMoney,
) -> IpeResult<ipe_runtime::error::IpeError, IpeMoneyMoney> {
    (if crate::user_ipe_money_eq_currency(
        crate::user_ipe_money_currency(a.clone()),
        crate::user_ipe_money_currency(b.clone()),
    ) {
        IpeResult::Ok(IpeMoneyMoney::Money(
            decimal_add(
                crate::user_ipe_money_amount(a.clone()),
                crate::user_ipe_money_amount(b),
            ),
            crate::user_ipe_money_currency(a),
        ))
    } else {
        IpeResult::Err(ipe_error_unexpected(format!(
            "{}{}",
            "Money.add: currency mismatch: ".to_string(),
            format!(
                "{}{}",
                crate::user_ipe_money_currency_code(crate::user_ipe_money_currency(a)),
                format!(
                    "{}{}",
                    " vs ".to_string(),
                    crate::user_ipe_money_currency_code(crate::user_ipe_money_currency(b))
                )
            )
        )))
    })
}
pub(crate) fn user_ipe_money_sub(
    a: IpeMoneyMoney,
    b: IpeMoneyMoney,
) -> IpeResult<ipe_runtime::error::IpeError, IpeMoneyMoney> {
    (if crate::user_ipe_money_eq_currency(
        crate::user_ipe_money_currency(a.clone()),
        crate::user_ipe_money_currency(b.clone()),
    ) {
        IpeResult::Ok(IpeMoneyMoney::Money(
            decimal_sub(
                crate::user_ipe_money_amount(a.clone()),
                crate::user_ipe_money_amount(b),
            ),
            crate::user_ipe_money_currency(a),
        ))
    } else {
        IpeResult::Err(ipe_error_unexpected(format!(
            "{}{}",
            "Money.sub: currency mismatch: ".to_string(),
            format!(
                "{}{}",
                crate::user_ipe_money_currency_code(crate::user_ipe_money_currency(a)),
                format!(
                    "{}{}",
                    " vs ".to_string(),
                    crate::user_ipe_money_currency_code(crate::user_ipe_money_currency(b))
                )
            )
        )))
    })
}
pub(crate) fn user_ipe_money_mul(
    k: ipe_runtime::decimal::Decimal,
    m: IpeMoneyMoney,
) -> IpeMoneyMoney {
    IpeMoneyMoney::Money(
        decimal_mul(k, crate::user_ipe_money_amount(m.clone())),
        crate::user_ipe_money_currency(m),
    )
}
pub(crate) fn user_ipe_money_neg(m: IpeMoneyMoney) -> IpeMoneyMoney {
    IpeMoneyMoney::Money(
        decimal_neg(crate::user_ipe_money_amount(m.clone())),
        crate::user_ipe_money_currency(m),
    )
}
pub(crate) fn user_ipe_money_abs(m: IpeMoneyMoney) -> IpeMoneyMoney {
    IpeMoneyMoney::Money(
        decimal_abs(crate::user_ipe_money_amount(m.clone())),
        crate::user_ipe_money_currency(m),
    )
}
pub(crate) fn user_ipe_money_allocate(parts: i64, m: IpeMoneyMoney) -> Vec<IpeMoneyMoney> {
    ({
        let c = crate::user_ipe_money_currency(m.clone());
        ({
            let decimals = money_allocate(
                crate::user_ipe_money_minor_units(c.clone()),
                parts,
                crate::user_ipe_money_amount(m),
            );
            list_map_consume(
                {
                    let __ipe_fn: Box<
                        dyn Fn(ipe_runtime::decimal::Decimal) -> IpeMoneyMoney
                            + Send
                            + Sync
                            + 'static,
                    > = Box::new(move |d: ipe_runtime::decimal::Decimal| -> IpeMoneyMoney {
                        IpeMoneyMoney::Money(d, c.clone())
                    });
                    __ipe_fn
                },
                decimals,
            )
        })
    })
}
pub(crate) fn user_ipe_money_sum_of(
    c: IpeMoneyCurrency,
    xs: Vec<IpeMoneyMoney>,
) -> IpeResult<ipe_runtime::error::IpeError, IpeMoneyMoney> {
    crate::user_ipe_money_sum_of_help(xs, IpeResult::Ok(crate::user_ipe_money_zero(c)))
}
pub(crate) fn user_ipe_money_sum_of_help(
    xs: Vec<IpeMoneyMoney>,
    acc: IpeResult<ipe_runtime::error::IpeError, IpeMoneyMoney>,
) -> IpeResult<ipe_runtime::error::IpeError, IpeMoneyMoney> {
    let mut xs = xs;
    let mut acc = acc;
    loop {
        match (xs).as_slice() {
            [] => {
                return acc;
            }
            [first, rest @ ..] => {
                let first = first.clone();
                let rest = rest.to_vec();
                match acc {
                    IpeResult::Err(e) => {
                        return IpeResult::Err(e);
                    }
                    IpeResult::Ok(m) => {
                        let __tco_0 = rest;
                        let __tco_1 = crate::user_ipe_money_add(m, first);
                        xs = __tco_0;
                        acc = __tco_1;
                        continue;
                    }
                }
            }
        }
    }
}
pub(crate) fn user_ipe_money_compare(a: IpeMoneyMoney, b: IpeMoneyMoney) -> i64 {
    decimal_compare(
        crate::user_ipe_money_amount(a),
        crate::user_ipe_money_amount(b),
    )
}
pub(crate) fn user_ipe_money_eq(a: IpeMoneyMoney, b: IpeMoneyMoney) -> bool {
    (crate::user_ipe_money_eq_currency(
        crate::user_ipe_money_currency(a.clone()),
        crate::user_ipe_money_currency(b.clone()),
    ) && decimal_eq(
        crate::user_ipe_money_amount(a),
        crate::user_ipe_money_amount(b),
    ))
}
pub(crate) fn user_ipe_money_neq(a: IpeMoneyMoney, b: IpeMoneyMoney) -> bool {
    basics_not(crate::user_ipe_money_eq(a, b))
}
pub(crate) fn user_ipe_money_lt(a: IpeMoneyMoney, b: IpeMoneyMoney) -> bool {
    decimal_lt(
        crate::user_ipe_money_amount(a),
        crate::user_ipe_money_amount(b),
    )
}
pub(crate) fn user_ipe_money_lte(a: IpeMoneyMoney, b: IpeMoneyMoney) -> bool {
    decimal_lte(
        crate::user_ipe_money_amount(a),
        crate::user_ipe_money_amount(b),
    )
}
pub(crate) fn user_ipe_money_gt(a: IpeMoneyMoney, b: IpeMoneyMoney) -> bool {
    decimal_gt(
        crate::user_ipe_money_amount(a),
        crate::user_ipe_money_amount(b),
    )
}
pub(crate) fn user_ipe_money_gte(a: IpeMoneyMoney, b: IpeMoneyMoney) -> bool {
    decimal_gte(
        crate::user_ipe_money_amount(a),
        crate::user_ipe_money_amount(b),
    )
}
pub(crate) fn user_ipe_money_eq_currency(a: IpeMoneyCurrency, b: IpeMoneyCurrency) -> bool {
    (crate::user_ipe_money_currency_code(a) == crate::user_ipe_money_currency_code(b))
}
pub(crate) fn user_ipe_money_is_zero(m: IpeMoneyMoney) -> bool {
    decimal_is_zero(crate::user_ipe_money_amount(m))
}
pub(crate) fn user_ipe_money_is_positive(m: IpeMoneyMoney) -> bool {
    decimal_is_positive(crate::user_ipe_money_amount(m))
}
pub(crate) fn user_ipe_money_is_negative(m: IpeMoneyMoney) -> bool {
    decimal_is_negative(crate::user_ipe_money_amount(m))
}
pub(crate) fn user_ipe_money_percent_of(
    pct: ipe_runtime::decimal::Decimal,
    m: IpeMoneyMoney,
) -> IpeMoneyMoney {
    IpeMoneyMoney::Money(
        decimal_percent_of(pct, crate::user_ipe_money_amount(m.clone())),
        crate::user_ipe_money_currency(m),
    )
}
pub(crate) fn user_ipe_money_add_percent(
    pct: ipe_runtime::decimal::Decimal,
    m: IpeMoneyMoney,
) -> IpeMoneyMoney {
    IpeMoneyMoney::Money(
        decimal_add_percent(pct, crate::user_ipe_money_amount(m.clone())),
        crate::user_ipe_money_currency(m),
    )
}
pub(crate) fn user_ipe_money_sub_percent(
    pct: ipe_runtime::decimal::Decimal,
    m: IpeMoneyMoney,
) -> IpeMoneyMoney {
    IpeMoneyMoney::Money(
        decimal_sub_percent(pct, crate::user_ipe_money_amount(m.clone())),
        crate::user_ipe_money_currency(m),
    )
}
pub(crate) fn user_ipe_money_format(m: IpeMoneyMoney) -> String {
    money_format(
        crate::user_ipe_money_currency_code(crate::user_ipe_money_currency(m.clone())),
        crate::user_ipe_money_amount(m),
    )
}
pub(crate) fn user_ipe_money_format_with_code(m: IpeMoneyMoney) -> String {
    money_format_with_code(
        crate::user_ipe_money_currency_code(crate::user_ipe_money_currency(m.clone())),
        crate::user_ipe_money_amount(m),
    )
}
pub(crate) fn user_ipe_money_to_minor(m: IpeMoneyMoney) -> i64 {
    decimal_to_minor(
        crate::user_ipe_money_minor_units(crate::user_ipe_money_currency(m.clone())),
        crate::user_ipe_money_amount(m),
    )
}
pub(crate) fn user_ipe_money_set_rate(
    from: IpeMoneyCurrency,
    to: IpeMoneyCurrency,
    rate: ipe_runtime::decimal::Decimal,
) -> IpeResult<ipe_runtime::error::IpeError, ()> {
    money_set_rate(
        crate::user_ipe_money_currency_code(from),
        crate::user_ipe_money_currency_code(to),
        rate,
    )
}
pub(crate) fn user_ipe_money_get_rate(
    from: IpeMoneyCurrency,
    to: IpeMoneyCurrency,
) -> IpeResult<ipe_runtime::error::IpeError, ipe_runtime::decimal::Decimal> {
    money_get_rate(
        crate::user_ipe_money_currency_code(from),
        crate::user_ipe_money_currency_code(to),
    )
}
pub(crate) fn user_ipe_money_has_rate(from: IpeMoneyCurrency, to: IpeMoneyCurrency) -> bool {
    money_has_rate(
        crate::user_ipe_money_currency_code(from),
        crate::user_ipe_money_currency_code(to),
    )
}
pub(crate) fn user_ipe_money_convert(
    to: IpeMoneyCurrency,
    m: IpeMoneyMoney,
) -> IpeResult<ipe_runtime::error::IpeError, IpeMoneyMoney> {
    (if crate::user_ipe_money_eq_currency(crate::user_ipe_money_currency(m.clone()), to.clone()) {
        IpeResult::Ok(m)
    } else {
        match crate::user_ipe_money_get_rate(crate::user_ipe_money_currency(m.clone()), to.clone())
        {
            IpeResult::Ok(rate) => IpeResult::Ok(IpeMoneyMoney::Money(
                decimal_mul(rate, crate::user_ipe_money_amount(m)),
                to,
            )),
            IpeResult::Err(e) => IpeResult::Err(e),
        }
    })
}
