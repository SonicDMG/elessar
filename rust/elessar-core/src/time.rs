//! Timestamps and the undated policy (spec §1 time, §8, §9). Phase 1b.
//!
//! Internally a time is microseconds since the epoch, as Python's `datetime`
//! holds it. The epoch milliseconds handed out follow the Python engine's
//! `int(dt.timestamp() * 1000)` float path, so any rounding it does is shared
//! with the Python events' `t`.

use crate::pack::{Source, TimeConfig};
use crate::pipeline::{document_path_value, py};
use crate::store::{get_path, present};
use chrono::{DateTime, NaiveDate};
use serde_json::Value;

const MIN_SECS: i128 = -62_135_596_800; // 0001-01-01T00:00:00Z
const MAX_SECS: i128 = 253_402_300_799; // 9999-12-31T23:59:59Z

fn check_range(secs: i128) -> Result<i64, String> {
    if (MIN_SECS..=MAX_SECS).contains(&secs) {
        return Ok(secs as i64);
    }
    if secs.unsigned_abs() >= 1 << 62 {
        return Err("timestamp out of range for platform time_t".into());
    }
    Err(format!("year {} is out of range", civil_year(secs)))
}

/// The proleptic Gregorian year of an epoch second, for any magnitude.
fn civil_year(secs: i128) -> i128 {
    const DAYS_PER_400Y: i128 = 146_097;
    let z = secs.div_euclid(86_400) + 719_468; // days since 0000-03-01
    let era = z.div_euclid(DAYS_PER_400Y);
    let doe = z.rem_euclid(DAYS_PER_400Y);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    yoe + era * 400 + i128::from(doy >= 306) // March-based years roll over at January
}

/// `datetime.fromtimestamp(ms / 1000, utc)`: true division, then microseconds
/// rounded half to even.
pub(crate) fn micros_from_ms(ms: i128) -> Result<i64, String> {
    let d = ms as f64 / 1000.0;
    let mut whole = d.trunc();
    let mut micros = ((d - whole) * 1e6).round_ties_even();
    if micros >= 1e6 {
        micros -= 1e6;
        whole += 1.0;
    } else if micros < 0.0 {
        micros += 1e6;
        whole -= 1.0;
    }
    Ok(check_range(whole as i128)? * 1_000_000 + micros as i64)
}

/// `datetime.fromtimestamp(s, utc)` for an int.
pub(crate) fn micros_from_s(s: i128) -> Result<i64, String> {
    Ok(check_range(s)? * 1_000_000)
}

/// `int(dt.timestamp() * 1000)`.
fn ms_of_micros(micros: i64) -> i64 {
    ((micros as f64 / 1e6) * 1000.0) as i64
}

fn format_secs(secs: i64) -> String {
    DateTime::from_timestamp_secs(secs).map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string()).unwrap_or_default()
}

/// `strftime("%Y-%m-%dT%H:%M:%SZ")` of a parsed time.
pub(crate) fn iso_of_micros(micros: i64) -> String {
    format_secs(micros.div_euclid(1_000_000))
}

// `strptime` directives. CPython compiles a format to a regex whose literals
// are never digits, so taking the first alternative that fits never needs to
// backtrack and these scans accept exactly what the regex does.

fn digit(b: &[u8], i: usize) -> Option<u32> {
    b.get(i).filter(|c| c.is_ascii_digit()).map(|c| (*c - b'0') as u32)
}

fn two(b: &[u8], i: usize) -> Option<(u32, usize)> {
    Some((digit(b, i)? * 10 + digit(b, i + 1)?, i + 2))
}

/// `%Y`: exactly four digits.
fn take_year(b: &[u8], i: usize) -> Option<(i32, usize)> {
    let (hi, _) = two(b, i)?;
    let (lo, end) = two(b, i + 2)?;
    Some(((hi * 100 + lo) as i32, end))
}

/// `%m`: `1[0-2]|0[1-9]|[1-9]`.
fn take_month(b: &[u8], i: usize) -> Option<(u32, usize)> {
    match two(b, i) {
        Some((v @ (1..=12), end)) => Some((v, end)),
        _ => digit(b, i).filter(|d| *d > 0).map(|d| (d, i + 1)),
    }
}

/// `%d`: `3[0-1]|[1-2]\d|0[1-9]|[1-9]| [1-9]`.
fn take_day(b: &[u8], i: usize) -> Option<(u32, usize)> {
    if b.get(i) == Some(&b' ') {
        return digit(b, i + 1).filter(|d| *d > 0).map(|d| (d, i + 2));
    }
    match two(b, i) {
        Some((v @ (1..=31), end)) => Some((v, end)),
        _ => digit(b, i).filter(|d| *d > 0).map(|d| (d, i + 1)),
    }
}

/// A two-digit directive up to `max`, or one digit: `%H`, `%M`, `%S`.
fn take_upto(b: &[u8], i: usize, max: u32) -> Option<(u32, usize)> {
    match two(b, i) {
        Some((v, end)) if v <= max => Some((v, end)),
        _ => digit(b, i).map(|d| (d, i + 1)),
    }
}

fn take_lit(b: &[u8], i: usize, lit: u8) -> Option<usize> {
    b.get(i).filter(|c| c.eq_ignore_ascii_case(&lit)).map(|_| i + 1)
}

fn take_date(b: &[u8]) -> Option<((i32, u32, u32), usize)> {
    let (y, i) = take_year(b, 0)?;
    let i = take_lit(b, i, b'-')?;
    let (m, i) = take_month(b, i)?;
    let i = take_lit(b, i, b'-')?;
    let (d, i) = take_day(b, i)?;
    Some(((y, m, d), i))
}

fn date_micros((y, m, d): (i32, u32, u32)) -> Result<i64, String> {
    if y < 1 {
        return Err(format!("year {y} is out of range"));
    }
    let date = NaiveDate::from_ymd_opt(y, m, d).ok_or("day is out of range for month")?;
    let midnight = date.and_hms_opt(0, 0, 0).ok_or("day is out of range for month")?;
    Ok(midnight.and_utc().timestamp() * 1_000_000)
}

/// `%Y-%m-%dT%H:%M[:%S[.%f]]Z` or `%Y-%m-%d`, whichever fits; the whole text must be consumed.
fn iso_micros(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    let (date, i) = take_date(b)?;
    if i == b.len() {
        return date_micros(date).ok();
    }
    let i = take_lit(b, i, b'T')?;
    let (hour, i) = take_upto(b, i, 23)?;
    let i = take_lit(b, i, b':')?;
    let (minute, i) = take_upto(b, i, 59)?;
    let (mut second, mut frac, mut i) = (0, 0, i);
    if b.get(i) == Some(&b':') {
        (second, i) = take_upto(b, i + 1, 61)?;
        if second > 59 {
            return None;
        }
        if b.get(i) == Some(&b'.') {
            i += 1;
            let start = i;
            let mut scale = 100_000;
            while let (Some(d), true) = (digit(b, i), i - start < 6) {
                frac += d as i64 * scale;
                scale /= 10;
                i += 1;
            }
            if i == start {
                return None;
            }
        }
    }
    if take_lit(b, i, b'Z')? != b.len() {
        return None;
    }
    let secs = (hour * 3600 + minute * 60 + second) as i64;
    Some(date_micros(date).ok()? + secs * 1_000_000 + frac)
}

/// `strptime(text, "%Y-%m-%d")` with its error messages.
fn ymd_micros(text: &str) -> Result<i64, String> {
    let Some((date, end)) = take_date(text.as_bytes()) else {
        return Err(format!("time data {} does not match format '%Y-%m-%d'", py::repr_str(text)));
    };
    if end != text.len() {
        return Err(format!("unconverted data remains: {}", &text[end..]));
    }
    date_micros(date)
}

fn parse_micros(value: &Value, unit: &str) -> Result<i64, String> {
    match unit {
        "ms" => micros_from_ms(py::to_int(value)?),
        "s" => micros_from_s(py::to_int(value)?),
        "date_dmy" => ymd_micros(&py::date_dmy(&py::str_of(value))?),
        _ => {
            let form = py::str_of(value);
            let text = py::strip(&form);
            iso_micros(text).ok_or_else(|| format!("not an ISO UTC timestamp: {}", py::repr_str(text)))
        }
    }
}

/// Parse a time value by unit into epoch milliseconds (UTC). Units: `ms`,
/// `s`, `iso` (`YYYY-MM-DDTHH:MM[:SS[.fff]]Z` or `YYYY-MM-DD`), `date_dmy`.
pub fn parse_time(value: &Value, unit: &str) -> Result<i64, String> {
    parse_micros(value, unit).map(ms_of_micros)
}

/// A record's time per a `field` config: the first listed path that is
/// present, parsed. `Ok(None)` when no listed field is present.
pub fn record_time(data: &Value, time: &TimeConfig) -> Result<Option<i64>, String> {
    let Some(fields) = &time.field else {
        return Ok(None);
    };
    for field in fields.list() {
        let v = get_path(data, field);
        if present(v) {
            return v.map(|v| parse_time(v, &time.unit)).transpose();
        }
    }
    Ok(None)
}

/// A whole source's time per a `document` config, from the raw payload.
pub fn document_time(raw: &str, source: &Source, time: &TimeConfig) -> Result<i64, String> {
    let Some(doc) = &time.document else {
        return Err("no document time config".into());
    };
    if let Some(constant) = &doc.constant {
        return parse_time(&Value::String(constant.clone()), &time.unit);
    }
    if let Some(pattern) = &doc.regex {
        let re = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
        let found = re.captures(raw).filter(|_| re.captures_len() > 1);
        let Some(caps) = found else {
            return Err(format!("document regex {} found no group in the source text", py::repr_str(pattern)));
        };
        let group = caps.get(1).map_or(Value::Null, |m| Value::String(m.as_str().to_string()));
        return parse_time(&group, &time.unit);
    }
    let Some(path) = &doc.path else {
        return Err("no document time config".into());
    };
    let value = document_path_value(raw, &source.format, path)?;
    if !present(value.as_ref()) {
        return Err(format!("document path {} is empty", py::repr_str(path)));
    }
    parse_time(value.as_ref().unwrap_or(&Value::Null), &time.unit)
}

/// Epoch ms as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso(t: i64) -> String {
    format_secs(t.div_euclid(1000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn iso_accepts_the_python_formats() {
        let t = |s: &str| parse_time(&json!(s), "iso");
        assert_eq!(t("2026-10-15T00:00:00Z"), Ok(1_792_022_400_000));
        assert_eq!(t("2026-10-15T00:00Z"), Ok(1_792_022_400_000));
        assert_eq!(t("2026-10-15"), Ok(1_792_022_400_000));
        assert_eq!(t(" 2026-10-15T00:00:00.500Z "), Ok(1_792_022_400_500));
        assert_eq!(t("2026-1-5t3:4z"), parse_time(&json!("2026-01-05T03:04:00Z"), "iso"));
        for bad in ["2026-10-15T00:00:00", "2026-13-01", "2026-02-30", "2026-10-15T24:00Z", "2026-10-15T00:00:60Z", "0000-01-01", "26-10-15", "2026-10-15T00:00:00.1234567Z", "soon"] {
            assert!(t(bad).is_err(), "{bad}");
        }
        assert_eq!(t("soon").unwrap_err(), "not an ISO UTC timestamp: 'soon'");
    }

    #[test]
    fn numeric_and_dmy_units() {
        assert_eq!(parse_time(&json!(1_791_245_789_000i64), "ms"), Ok(1_791_245_789_000));
        assert_eq!(parse_time(&json!("1791245789"), "s"), Ok(1_791_245_789_000));
        assert_eq!(parse_time(&json!(1791245789.9), "s"), Ok(1_791_245_789_000));
        assert_eq!(parse_time(&json!("15/10/2026"), "date_dmy"), Ok(1_792_022_400_000));
        assert_eq!(parse_time(&json!("15.10-2026"), "date_dmy"), Ok(1_792_022_400_000));
        assert_eq!(parse_time(&json!("abc"), "ms").unwrap_err(), "invalid literal for int() with base 10: 'abc'");
        assert_eq!(parse_time(&json!("2026-10"), "date_dmy").unwrap_err(), "not enough values to unpack (expected 3, got 2)");
        assert_eq!(parse_time(&json!("31/02/2026"), "date_dmy").unwrap_err(), "day is out of range for month");
        assert_eq!(parse_time(&json!("01/02/26"), "date_dmy").unwrap_err(), "time data '26-02-01' does not match format '%Y-%m-%d'");
    }

    #[test]
    fn iso_floors_to_the_second() {
        assert_eq!(iso(1_791_245_789_999), "2026-10-06T00:16:29Z");
        assert_eq!(iso(-1), "1969-12-31T23:59:59Z");
    }
}
