//! A time as the owner names one (RFC 3339, `2026-11-23T14:34:37+09:00`,
//! or ms since the epoch) and as factorium shows one (UTC, to the second).

use jiff::Timestamp;

/// `s` as ms since the epoch: RFC 3339 with its offset (`Z` or `±HH:MM`;
/// a space may stand for the `T`, and a fraction finer than a ms is
/// dropped), or a bare count of ms; either within years -9999 to 9999.
pub fn parse(s: &str) -> Result<i64, String> {
    let t = s.trim();
    let at = match t.parse::<i64>() {
        Ok(ms) => Timestamp::from_millisecond(ms),
        Err(_) => t.parse::<Timestamp>(),
    };
    at.map(|at| at.as_millisecond())
        .map_err(|e| format!("`{s}` is no time ({e}): name one as 2026-11-23T14:34:37+09:00 (or Z), or in ms"))
}

/// `ms` since the epoch as UTC, to the second: `2026-11-23T05:34:37Z`;
/// one past the years [`parse`] reads, as the nearest it reads.
pub fn utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000).clamp(Timestamp::MIN.as_second(), Timestamp::MAX.as_second());
    Timestamp::from_second(secs).expect("clamped to the range").to_string()
}
