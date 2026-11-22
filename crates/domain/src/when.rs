//! A time as the owner names one (RFC 3339, `2026-11-23T14:34:37+09:00`,
//! or ms since the epoch) and as factorium shows one (UTC, to the second).

/// The days from 1970-01-01 to `y-m-d` of the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `s` as ms since the epoch: RFC 3339 with its offset (`Z` or `±HH:MM`;
/// a space may stand for the `T`, and fractions of a second are dropped),
/// or a bare count of ms.
pub fn parse(s: &str) -> Result<i64, String> {
    let bad = || format!("`{s}` is no time: name one as 2026-11-23T14:34:37+09:00 (or Z), or in ms");
    let t = s.trim();
    if let Ok(ms) = t.parse() {
        return Ok(ms);
    }
    let (stamp, rest) = t.split_at_checked(19).ok_or_else(bad)?;
    let fields: Result<Vec<i64>, _> = stamp.split(['-', 'T', 't', ' ', ':']).map(str::parse).collect();
    let [y, mo, d, h, mi, se] = fields.map_err(|_| bad())?[..] else { return Err(bad()) };
    let rest = rest.strip_prefix('.').map_or(rest, |f| f.trim_start_matches(|c: char| c.is_ascii_digit()));
    let offset = match rest.split_at_checked(1) {
        Some(("Z" | "z", "")) => 0,
        Some((sign @ ("+" | "-"), hm)) => {
            let (oh, om) = hm.split_once(':').ok_or_else(bad)?;
            let (oh, om): (i64, i64) = (oh.parse().map_err(|_| bad())?, om.parse().map_err(|_| bad())?);
            (oh * 3600 + om * 60) * if sign == "-" { -1 } else { 1 }
        }
        _ => return Err(bad()),
    };
    // A date that is none (the 30th of February) comes back as another.
    let days = days_from_civil(y, mo, d);
    if !(1..=12).contains(&mo) || civil_from_days(days) != (y, mo, d) || h > 23 || mi > 59 || se > 59 {
        return Err(bad());
    }
    Ok((days * 86_400 + h * 3600 + mi * 60 + se - offset) * 1000)
}

/// `ms` since the epoch as UTC, to the second: `2026-11-23T05:34:37Z`.
pub fn utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s % 3600 / 60, s % 60)
}
