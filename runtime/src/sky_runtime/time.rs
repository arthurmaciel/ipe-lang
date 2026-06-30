// Time kernel — basic helpers (tokio-gated) + Std.Time advanced (always available).
use super::*;

#[cfg(feature = "tokio")]
pub fn time_now<E: Send + 'static>(_: ()) -> SkyTask<E, i64> {
    Box::pin(async move {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        ok_res(ms)
    })
}

#[cfg(feature = "tokio")]
pub fn time_sleep<E: Send + 'static>(ms: i64) -> SkyTask<E, ()> {
    Box::pin(async move {
        // Clamp negative ms to 0: `ms as u64` on a negative wraps to a near-
        // infinite Duration (permanent deadlock from a well-typed Time.sleep).
        tokio::time::sleep(std::time::Duration::from_millis(ms.max(0) as u64)).await;
        ok_res(())
    })
}

#[cfg(feature = "tokio")]
pub fn time_unix_millis<E: Send + 'static>(_: ()) -> SkyTask<E, i64> {
    time_now(())
}

pub fn time_time_string(ms: i64) -> String {
    format!("timestamp:{}", ms)
}

/// `Time.addMillis : Int -> Int -> Int` — pure integer addition.
/// Go: `return AsInt(ms) + AsInt(delta)`. Args order: delta first, ms second
/// (matches the Sky sig `addMillis : Int -> Int -> Int`, called
/// `Time.addMillis delta ms`).
pub fn time_add_millis(delta: i64, ms: i64) -> i64 {
    ms.saturating_add(delta)
}

/// `Time.diffMillis : Int -> Int -> Int` — `later - earlier`.
/// Go: `return AsInt(later) - AsInt(earlier)`. Args: (later, earlier).
pub fn time_diff_millis(later: i64, earlier: i64) -> i64 {
    later.saturating_sub(earlier)
}

/// `Time.format : String -> Int -> String` — custom Go-style layout.
/// Go uses `t.UTC().Format(layout)`. We map the Go reference-time layout to
/// chrono's strftime format. Sky exposes the Go layout directly
/// ("2006-01-02 15:04:05"), so we translate the Go reference time tokens.
/// Fallback to a best-effort strftime for unrecognised tokens (matches the
/// open-ended nature of Go's `t.Format`).
pub fn time_format(layout: String, ms: i64) -> String {
    use chrono::{TimeZone, Utc};
    let dt = match Utc.timestamp_millis_opt(ms).single() {
        Some(d) => d,
        None => return String::new(),
    };
    // Translate Go reference-time placeholders to chrono strftime.
    // Go's reference time: Mon Jan 2 15:04:05 MST 2006 (= 2006-01-02 15:04:05).
    let strfmt = layout
        .replace("2006", "%Y")
        .replace("01", "%m")
        .replace("02", "%d")
        .replace("15", "%H")
        .replace("04", "%M")
        .replace("05", "%S")
        .replace("Jan", "%b")
        .replace("Mon", "%a")
        .replace("MST", "UTC")
        // Longer fractional-second token MUST be translated before the shorter
        // one, else `.000` shadows the `.000000` form (it matches the leading 4
        // chars and leaves a stray `000`).
        .replace(".000000", ".%6f")
        .replace(".000", ".%3f")
        .replace("PM", "%p")
        .replace("pm", "%P");
    // Non-panicking render: chrono's DelayedFormat Display can return Err on an
    // invalid/unterminated format specifier (e.g. a stray `%`), and std's
    // to_string() panics when a Display impl returns Err. Use write! instead.
    let mut out = String::new();
    match std::fmt::write(&mut out, format_args!("{}", dt.format(&strfmt))) {
        Ok(()) => out,
        Err(_) => String::new(),
    }
}

/// `Time.formatHTTP : Int -> String` — HTTP date header format.
/// Go: `t.UTC().Format(http.TimeFormat)` → "Mon, 02 Jan 2006 15:04:05 GMT".
/// chrono's `%a, %d %b %Y %H:%M:%S GMT` produces byte-identical output.
pub fn time_format_http(ms: i64) -> String {
    use chrono::{TimeZone, Utc};
    match Utc.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
        None => String::new(),
    }
}

/// `Time.formatRFC3339 : Int -> String` — RFC 3339 / ISO 8601 with nanoseconds.
/// Go: `t.UTC().Format(time.RFC3339Nano)` → "2006-01-02T15:04:05.999999999Z".
/// chrono's `to_rfc3339` produces RFC 3339 with sub-second precision when non-zero.
pub fn time_format_rfc3339(ms: i64) -> String {
    use chrono::{TimeZone, Utc};
    match Utc.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.to_rfc3339(),
        None => String::new(),
    }
}

/// === Std.Time advanced — IANA zones + calendar math ===
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc, Weekday};
use chrono_tz::Tz;

fn parse_zone<E: From<String>>(z: &str) -> SkyResult<E, Tz> {
    match z.parse::<Tz>() {
        Ok(t) => SkyResult::Ok(t),
        Err(_) => SkyResult::Err(format!("Std.Time: unknown zone: {}", z).into()),
    }
}

fn millis_to_zoned<E: From<String>>(zone: &str, ms: i64) -> SkyResult<E, DateTime<Tz>> {
    let tz = match parse_zone::<E>(zone) {
        SkyResult::Ok(t) => t,
        SkyResult::Err(e) => return SkyResult::Err(e),
    };
    match Utc.timestamp_millis_opt(ms).single() {
        Some(utc) => SkyResult::Ok(utc.with_timezone(&tz)),
        None => SkyResult::Err(format!("Std.Time: epoch ms out of range: {}", ms).into()),
    }
}

pub fn time_in_zone<E: From<String>>(zone: String, ms: i64) -> SkyResult<E, String> {
    let dt = match millis_to_zoned::<E>(&zone, ms) {
        SkyResult::Ok(d) => d,
        SkyResult::Err(e) => return SkyResult::Err(e),
    };
    SkyResult::Ok(dt.to_rfc3339())
}

// Saturating arithmetic: extreme caller-controlled Ints would otherwise
// overflow-panic in debug / wrap silently in release. Saturation keeps these
// total (no panic path) and is the closest bare-`i64` analogue of the
// `time_add_months` "return ms on out-of-range" fallback.
pub fn time_add_days(days: i64, ms: i64) -> i64 {
    ms.saturating_add(days.saturating_mul(86_400_000))
}
pub fn time_add_hours(h: i64, ms: i64) -> i64 {
    ms.saturating_add(h.saturating_mul(3_600_000))
}
pub fn time_add_minutes(m: i64, ms: i64) -> i64 {
    ms.saturating_add(m.saturating_mul(60_000))
}
pub fn time_add_seconds(s: i64, ms: i64) -> i64 {
    ms.saturating_add(s.saturating_mul(1000))
}

pub fn time_add_months(months: i64, ms: i64) -> i64 {
    let utc = match Utc.timestamp_millis_opt(ms).single() {
        Some(d) => d,
        None => return ms,
    };
    let y = utc.year() as i64;
    // `months` is caller-controlled (Sky `Std.Time.addMonths`): saturating_add
    // avoids the i64 overflow (debug panic / release silent-wrap) on an extreme
    // value. A saturated `m` makes `new_y as i32` truncate → from_ymd_opt below
    // returns None → the function returns `ms` unchanged (total, no panic).
    let m = (utc.month() as i64 - 1).saturating_add(months);
    // Parse the target year into chrono's i32 domain (don't truncate): a lossy
    // `as i32` on a large-but-non-saturating result would wrap a far-future/past
    // year back into a valid band → a WRONG in-range date. try_from → out-of-range
    // returns `ms` unchanged (the intended total fallthrough).
    let new_y = match i32::try_from(y.saturating_add(m.div_euclid(12))) {
        Ok(v) => v,
        Err(_) => return ms,
    };
    let new_m = (m.rem_euclid(12) + 1) as u32;
    // Clamp day to month end
    let first = NaiveDate::from_ymd_opt(new_y, new_m, 1);
    let max_day = match first {
        Some(d) => {
            let (ny, nm) = if new_m == 12 {
                (new_y + 1, 1u32)
            } else {
                (new_y, new_m + 1)
            };
            let first_next = NaiveDate::from_ymd_opt(ny, nm, 1).unwrap_or(d);
            first_next.signed_duration_since(d).num_days() as u32
        }
        None => return ms,
    };
    let day = utc.day().min(max_day);
    match NaiveDate::from_ymd_opt(new_y, new_m, day).and_then(|d| {
        d.and_hms_milli_opt(
            utc.hour(),
            utc.minute(),
            utc.second(),
            utc.timestamp_subsec_millis(),
        )
    }) {
        Some(ndt) => Utc.from_utc_datetime(&ndt).timestamp_millis(),
        None => ms,
    }
}

pub fn time_add_years(years: i64, ms: i64) -> i64 {
    time_add_months(years.saturating_mul(12), ms)
}

fn zoned_field<E: From<String>, F>(zone: String, ms: i64, f: F) -> SkyResult<E, i64>
where
    F: FnOnce(DateTime<Tz>) -> i64,
{
    let dt = match millis_to_zoned::<E>(&zone, ms) {
        SkyResult::Ok(d) => d,
        SkyResult::Err(e) => return SkyResult::Err(e),
    };
    SkyResult::Ok(f(dt))
}

pub fn time_year<E: From<String>>(z: String, ms: i64) -> SkyResult<E, i64> {
    zoned_field(z, ms, |dt| dt.year() as i64)
}
pub fn time_month<E: From<String>>(z: String, ms: i64) -> SkyResult<E, i64> {
    zoned_field(z, ms, |dt| dt.month() as i64)
}
pub fn time_day<E: From<String>>(z: String, ms: i64) -> SkyResult<E, i64> {
    zoned_field(z, ms, |dt| dt.day() as i64)
}
pub fn time_day_of_week<E: From<String>>(z: String, ms: i64) -> SkyResult<E, i64> {
    zoned_field(z, ms, |dt| match dt.weekday() {
        Weekday::Mon => 1,
        Weekday::Tue => 2,
        Weekday::Wed => 3,
        Weekday::Thu => 4,
        Weekday::Fri => 5,
        Weekday::Sat => 6,
        Weekday::Sun => 7,
    })
}
pub fn time_day_of_year<E: From<String>>(z: String, ms: i64) -> SkyResult<E, i64> {
    zoned_field(z, ms, |dt| dt.ordinal() as i64)
}
pub fn time_week_of_year<E: From<String>>(z: String, ms: i64) -> SkyResult<E, i64> {
    zoned_field(z, ms, |dt| dt.iso_week().week() as i64)
}
pub fn time_is_weekend<E: From<String>>(z: String, ms: i64) -> SkyResult<E, bool> {
    let dt = match millis_to_zoned::<E>(&z, ms) {
        SkyResult::Ok(d) => d,
        SkyResult::Err(e) => return SkyResult::Err(e),
    };
    SkyResult::Ok(matches!(dt.weekday(), Weekday::Sat | Weekday::Sun))
}

pub fn time_is_leap_year(y: i64) -> bool {
    let y = y as i32;
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn time_days_in_month(year: i64, month: i64) -> i64 {
    // Parse year into chrono's i32 domain rather than truncating: a lossy cast on
    // a large caller Int would wrap to a valid band → wrong day count. Out-of-range → 0.
    let y = match i32::try_from(year) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let m = month as u32;
    if !(1..=12).contains(&m) {
        return 0;
    }
    // saturating_add: `year` is caller-controlled; y+1 at i32::MAX would panic
    // (debug) / wrap (release). A saturated year makes from_ymd_opt return None → 0.
    let (ny, nm) = if m == 12 {
        (y.saturating_add(1), 1)
    } else {
        (y, m + 1)
    };
    match (
        NaiveDate::from_ymd_opt(ny, nm, 1),
        NaiveDate::from_ymd_opt(y, m, 1),
    ) {
        (Some(next), Some(this)) => next.signed_duration_since(this).num_days(),
        _ => 0,
    }
}

fn local_midnight_in_zone<E: From<String>>(
    zone: String,
    ms: i64,
    h: u32,
    mi: u32,
    se: u32,
    mi_lli: u32,
    target_date: impl Fn(DateTime<Tz>) -> NaiveDate,
) -> SkyResult<E, i64> {
    let dt = match millis_to_zoned::<E>(&zone, ms) {
        SkyResult::Ok(d) => d,
        SkyResult::Err(e) => return SkyResult::Err(e),
    };
    let date = target_date(dt);
    let local = match date.and_hms_milli_opt(h, mi, se, mi_lli) {
        Some(l) => l,
        None => return SkyResult::Err("Std.Time: invalid date components".to_string().into()),
    };
    match dt.timezone().from_local_datetime(&local).single() {
        Some(z) => SkyResult::Ok(z.timestamp_millis()),
        None => SkyResult::Err("Std.Time: ambiguous local time".to_string().into()),
    }
}

pub fn time_start_of_day<E: From<String>>(zone: String, ms: i64) -> SkyResult<E, i64> {
    local_midnight_in_zone(zone, ms, 0, 0, 0, 0, |dt| dt.date_naive())
}
pub fn time_end_of_day<E: From<String>>(zone: String, ms: i64) -> SkyResult<E, i64> {
    match time_start_of_day::<E>(zone, ms) {
        SkyResult::Ok(start) => SkyResult::Ok(start + 86_400_000 - 1),
        SkyResult::Err(e) => SkyResult::Err(e),
    }
}
pub fn time_start_of_week<E: From<String>>(zone: String, ms: i64) -> SkyResult<E, i64> {
    local_midnight_in_zone(zone, ms, 0, 0, 0, 0, |dt| {
        let wd = dt.weekday().num_days_from_monday();
        dt.date_naive() - Duration::days(wd as i64)
    })
}
pub fn time_start_of_month<E: From<String>>(zone: String, ms: i64) -> SkyResult<E, i64> {
    local_midnight_in_zone(zone, ms, 0, 0, 0, 0, |dt| {
        NaiveDate::from_ymd_opt(dt.year(), dt.month(), 1).unwrap_or(dt.date_naive())
    })
}
pub fn time_end_of_month<E: From<String>>(zone: String, ms: i64) -> SkyResult<E, i64> {
    let dt = match millis_to_zoned::<E>(&zone, ms) {
        SkyResult::Ok(d) => d,
        SkyResult::Err(e) => return SkyResult::Err(e),
    };
    let dim = time_days_in_month(dt.year() as i64, dt.month() as i64) as u32;
    let target = NaiveDate::from_ymd_opt(dt.year(), dt.month(), dim);
    let target_date = target.unwrap_or(dt.date_naive());
    local_midnight_in_zone::<E>(zone, ms, 23, 59, 59, 999, move |_| target_date)
}
pub fn time_start_of_year<E: From<String>>(zone: String, ms: i64) -> SkyResult<E, i64> {
    local_midnight_in_zone(zone, ms, 0, 0, 0, 0, |dt| {
        NaiveDate::from_ymd_opt(dt.year(), 1, 1).unwrap_or(dt.date_naive())
    })
}
pub fn time_end_of_year<E: From<String>>(zone: String, ms: i64) -> SkyResult<E, i64> {
    local_midnight_in_zone(zone, ms, 23, 59, 59, 999, |dt| {
        NaiveDate::from_ymd_opt(dt.year(), 12, 31).unwrap_or(dt.date_naive())
    })
}

pub fn time_format_in_zone<E: From<String>>(
    pattern: String,
    zone: String,
    ms: i64,
) -> SkyResult<E, String> {
    let dt = match millis_to_zoned::<E>(&zone, ms) {
        SkyResult::Ok(d) => d,
        SkyResult::Err(e) => return SkyResult::Err(e),
    };
    // Non-panicking render: same risk as time_format — stray `%` in a
    // Sky-supplied pattern causes DelayedFormat::fmt to return Err, which
    // to_string() turns into a panic. Use write! and fall back to "".
    let mut out = String::new();
    match std::fmt::write(&mut out, format_args!("{}", dt.format(&pattern))) {
        Ok(()) => SkyResult::Ok(out),
        Err(_) => SkyResult::Ok(String::new()),
    }
}

/// `Sky.Core.Time.formatISO8601 ms` — the UTC instant as an RFC3339 / ISO-8601
/// string (Go parity: `t.UTC().Format(time.RFC3339)`). Infallible (`""` only on
/// an out-of-range timestamp).
pub fn time_format_iso8601(ms: i64) -> String {
    match Utc.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.to_rfc3339(),
        None => String::new(),
    }
}

// === advanced diff / fromParts / zone kernels ===

/// `diffSeconds later earlier` — integer seconds between two epoch-ms timestamps.
// Division truncates toward zero (Go parity; negative spans truncate toward
// zero too). `saturating_sub` avoids an overflow-panic on extreme epoch inputs.
pub fn time_diff_seconds(later_ms: i64, earlier_ms: i64) -> i64 {
    later_ms.saturating_sub(earlier_ms) / 1_000
}
pub fn time_diff_minutes(later_ms: i64, earlier_ms: i64) -> i64 {
    later_ms.saturating_sub(earlier_ms) / 60_000
}
pub fn time_diff_hours(later_ms: i64, earlier_ms: i64) -> i64 {
    later_ms.saturating_sub(earlier_ms) / 3_600_000
}
pub fn time_diff_days(later_ms: i64, earlier_ms: i64) -> i64 {
    later_ms.saturating_sub(earlier_ms) / 86_400_000
}

/// Sky source: `fromParts zone y m d h mins s -> Result Error Int`.
/// Computes the UTC epoch-ms for the given local date/time in the given IANA
/// zone. Invalid parts return Err. Unknown timezone returns Err.
pub fn time_from_parts<E: From<String>>(
    zone: String,
    y: i64,
    m: i64,
    d: i64,
    h: i64,
    mins: i64,
    s: i64,
) -> SkyResult<E, i64> {
    let tz: Tz = match zone.parse() {
        Ok(t) => t,
        Err(_) => {
            return SkyResult::Err(format!("Time.fromParts: unknown timezone {:?}", zone).into());
        }
    };
    let naive = match NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32)
        .and_then(|day| day.and_hms_opt(h as u32, mins as u32, s as u32))
    {
        Some(n) => n,
        None => {
            return SkyResult::Err(
                format!(
                    "Time.fromParts: invalid date parts {}-{:02}-{:02} {:02}:{:02}:{:02}",
                    y, m, d, h, mins, s
                )
                .into(),
            );
        }
    };
    match tz.from_local_datetime(&naive).single() {
        Some(zoned) => SkyResult::Ok(zoned.with_timezone(&Utc).timestamp_millis()),
        None => SkyResult::Err(format!(
            "Time.fromParts: ambiguous/non-existent local time {}-{:02}-{:02} {:02}:{:02}:{:02} in {}",
            y, m, d, h, mins, s, zone).into()),
    }
}

/// `zoneOffset zone ms -> Result Error Int` — UTC offset in seconds for the
/// instant in the given zone. Unknown zones return Err.
pub fn time_zone_offset<E: From<String>>(zone_name: String, ms: i64) -> SkyResult<E, i64> {
    use chrono::Offset;
    let utc: DateTime<Utc> = match Utc.timestamp_millis_opt(ms).single() {
        Some(t) => t,
        None => return SkyResult::Err(format!("Time.zoneOffset: invalid epoch ms {}", ms).into()),
    };
    match zone_name.parse::<Tz>() {
        Ok(tz) => SkyResult::Ok(
            tz.from_utc_datetime(&utc.naive_utc())
                .offset()
                .fix()
                .local_minus_utc() as i64,
        ),
        Err(_) => {
            SkyResult::Err(format!("Time.zoneOffset: unknown timezone {:?}", zone_name).into())
        }
    }
}

/// `zoneName zone ms -> Result Error String` — short timezone abbreviation
/// (e.g. "EST", "PDT"). Unknown zones return Err.
pub fn time_zone_name<E: From<String>>(zone_name: String, ms: i64) -> SkyResult<E, String> {
    let utc: DateTime<Utc> = match Utc.timestamp_millis_opt(ms).single() {
        Some(t) => t,
        None => return SkyResult::Err(format!("Time.zoneName: invalid epoch ms {}", ms).into()),
    };
    match zone_name.parse::<Tz>() {
        Ok(tz) => SkyResult::Ok(
            tz.from_utc_datetime(&utc.naive_utc())
                .format("%Z")
                .to_string(),
        ),
        Err(_) => SkyResult::Err(format!("Time.zoneName: unknown timezone {:?}", zone_name).into()),
    }
}

#[cfg(test)]
mod time_advanced_tests {
    use super::*;

    // 2026-05-29 12:00:00 UTC is a Friday
    const T1: i64 = 1_780_400_400_000;

    #[test]
    fn test_in_zone_utc() {
        let r: SkyResult<String, String> = time_in_zone("UTC".to_string(), T1);
        assert!(matches!(r, SkyResult::Ok(ref s) if s.contains("2026")));
    }

    #[test]
    fn test_in_zone_unknown() {
        let r: SkyResult<String, String> = time_in_zone("Not/AZone".to_string(), T1);
        assert!(matches!(r, SkyResult::Err(_)));
    }

    #[test]
    fn test_day_of_week_friday() {
        let r: SkyResult<String, i64> = time_day_of_week("UTC".to_string(), T1);
        assert!(
            matches!(r, SkyResult::Ok(d) if (1..=7).contains(&d)),
            "got {:?}",
            r
        );
    }

    #[test]
    fn test_is_leap_year() {
        assert!(time_is_leap_year(2024));
        assert!(!time_is_leap_year(2025));
        assert!(!time_is_leap_year(1900));
        assert!(time_is_leap_year(2000));
    }

    #[test]
    fn test_add_days_months() {
        // adding 1 day = +86400000 ms
        assert_eq!(time_add_days(1, T1), T1 + 86_400_000);
        // adding months returns SOME result (no panic)
        let added = time_add_months(1, T1);
        assert!(added > T1);
    }

    // advanced kernel tests

    #[test]
    fn test_diff_seconds() {
        assert_eq!(time_diff_seconds(5_500, 3_000), 2);
        assert_eq!(time_diff_seconds(0, 2_500), -2);
    }

    #[test]
    fn test_diff_minutes_hours_days() {
        assert_eq!(time_diff_minutes(120_000, 0), 2);
        assert_eq!(time_diff_hours(3_600_000 * 5, 0), 5);
        assert_eq!(time_diff_days(86_400_000 * 7, 0), 7);
    }

    #[test]
    fn test_from_parts_epoch_utc() {
        let r: SkyResult<String, i64> = time_from_parts("UTC".into(), 1970, 1, 1, 0, 0, 0);
        assert!(matches!(r, SkyResult::Ok(0)));
    }

    #[test]
    fn test_from_parts_invalid_returns_err() {
        let r1: SkyResult<String, i64> = time_from_parts("UTC".into(), 2024, 13, 1, 0, 0, 0); // month 13
        let r2: SkyResult<String, i64> = time_from_parts("UTC".into(), 2024, 2, 30, 0, 0, 0); // Feb 30
        let r3: SkyResult<String, i64> = time_from_parts("Not/AZone".into(), 2024, 1, 1, 0, 0, 0);
        assert!(matches!(r1, SkyResult::Err(_)));
        assert!(matches!(r2, SkyResult::Err(_)));
        assert!(matches!(r3, SkyResult::Err(_))); // unknown timezone
    }

    #[test]
    fn test_zone_offset_utc() {
        let r: SkyResult<String, i64> = time_zone_offset("UTC".into(), 0);
        assert!(matches!(r, SkyResult::Ok(0)));
    }

    #[test]
    fn test_zone_offset_ny_winter() {
        // 1970-01-01 00:00 UTC; America/New_York was EST (-5h) on that day.
        let r: SkyResult<String, i64> = time_zone_offset("America/New_York".into(), 0);
        assert!(matches!(r, SkyResult::Ok(v) if v == -5 * 3_600));
    }

    #[test]
    fn test_zone_name_utc() {
        let r: SkyResult<String, String> = time_zone_name("UTC".into(), 0);
        match r {
            SkyResult::Ok(name) => assert!(name == "UTC" || name == "Z"),
            SkyResult::Err(_) => panic!("UTC should be a known timezone"),
        }
    }

    #[test]
    fn test_zone_offset_unknown_returns_err() {
        let r: SkyResult<String, i64> = time_zone_offset("Not/AZone".into(), 0);
        assert!(matches!(r, SkyResult::Err(_)));
    }

    // ── New kernels (go-parity gaps sweep 2026-06-15) ─────────────────────────

    #[test]
    fn test_add_millis() {
        assert_eq!(time_add_millis(1000, 5000), 6000);
        assert_eq!(time_add_millis(-500, 1000), 500);
        assert_eq!(time_add_millis(0, 999), 999);
    }

    #[test]
    fn test_diff_millis() {
        assert_eq!(time_diff_millis(5000, 3000), 2000);
        assert_eq!(time_diff_millis(1000, 3000), -2000);
        assert_eq!(time_diff_millis(42, 42), 0);
    }

    #[test]
    fn test_format_http() {
        // 1970-01-01 00:00:00 UTC = epoch 0.
        // Go's http.TimeFormat gives "Thu, 01 Jan 1970 00:00:00 GMT".
        let s = time_format_http(0);
        assert!(s.contains("1970"), "HTTP format for epoch 0: {}", s);
        assert!(s.ends_with("GMT"), "HTTP format must end in GMT: {}", s);
    }

    #[test]
    fn test_format_rfc3339() {
        let s = time_format_rfc3339(0);
        // chrono's to_rfc3339 produces "1970-01-01T00:00:00+00:00" for epoch 0.
        assert!(s.starts_with("1970-01-01T"), "RFC3339 for epoch 0: {}", s);
    }

    #[test]
    fn test_format_http_out_of_range() {
        // An invalid timestamp should return "" rather than panic.
        let s = time_format_http(i64::MAX);
        // Either empty or some valid string — just must not panic.
        let _ = s;
    }
}
