//! A time as the owner names one (RFC 3339, `2026-11-23T14:34:37+09:00`,
//! or ms since the epoch) and as factorium shows one (UTC, to the second),
//! and a span of time as the owner names one (`45m`).

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

/// A duration given as `90s`, `45m` or `2h` (a bare number is minutes), in
/// ms; one past an i64 of ms, the most it holds.
pub fn duration_ms(s: &str) -> Result<i64, String> {
    let t = s.trim();
    let units = [('s', 1_000), ('m', 60_000), ('h', 3_600_000)];
    let (n, per) = units.iter().find_map(|&(u, per)| Some((t.strip_suffix(u)?, per))).unwrap_or((t, 60_000));
    let n: i64 = n.trim().parse().map_err(|_| format!("`{s}` is not a duration such as `90s`, `45m` or `2h`"))?;
    Ok(n.saturating_mul(per))
}

/// `ms` since the epoch as UTC, to the second: `2026-11-23T05:34:37Z`;
/// one past the years [`parse`] reads, as the nearest it reads.
pub fn utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000).clamp(Timestamp::MIN.as_second(), Timestamp::MAX.as_second());
    Timestamp::from_second(secs).expect("clamped to the range").to_string()
}
