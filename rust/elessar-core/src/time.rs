//! Timestamps and the undated policy (spec §1 time, §8, §9). Phase 1b.

use crate::pack::{Source, TimeConfig};
use serde_json::Value;

/// Parse a time value by unit into epoch milliseconds (UTC). Units: `ms`,
/// `s`, `iso` (`YYYY-MM-DDTHH:MM[:SS[.fff]]Z` or `YYYY-MM-DD`), `date_dmy`.
pub fn parse_time(value: &Value, unit: &str) -> Result<i64, String> {
    let _ = (value, unit);
    todo!("phase 1b: parse_time")
}

/// A record's time per a `field` config: the first listed path that is
/// present, parsed. `Ok(None)` when no listed field is present.
pub fn record_time(data: &Value, time: &TimeConfig) -> Result<Option<i64>, String> {
    let _ = (data, time);
    todo!("phase 1b: record_time")
}

/// A whole source's time per a `document` config, from the raw payload.
pub fn document_time(raw: &str, source: &Source, time: &TimeConfig) -> Result<i64, String> {
    let _ = (raw, source, time);
    todo!("phase 1b: document_time")
}

/// Epoch ms as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso(t: i64) -> String {
    let _ = t;
    todo!("phase 1b: iso")
}
