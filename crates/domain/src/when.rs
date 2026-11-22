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
    if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
        return t.parse().map_err(|_| bad());
    }
    let num = |a: usize, b: usize| t.get(a..b).filter(|p| p.bytes().all(|c| c.is_ascii_digit()))?.parse::<i64>().ok();
    let sep = |i: usize, c: &[u8]| t.as_bytes().get(i).is_some_and(|b| c.contains(b));
    if !(sep(4, b"-") && sep(7, b"-") && sep(10, b"Tt ") && sep(13, b":") && sep(16, b":")) {
        return Err(bad());
    }
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) =
        (num(0, 4), num(5, 7), num(8, 10), num(11, 13), num(14, 16), num(17, 19))
    else {
        return Err(bad());
    };
    let mut rest = t.get(19..).ok_or_else(bad)?;
    if let Some(frac) = rest.strip_prefix('.') {
        rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest {
        "Z" | "z" => 0,
        o if o.len() == 6 && (o.starts_with('+') || o.starts_with('-')) && &o[3..4] == ":" => {
            let (oh, om) = (o[1..3].parse::<i64>().map_err(|_| bad())?, o[4..6].parse::<i64>().map_err(|_| bad())?);
            let sign = if o.starts_with('-') { -1 } else { 1 };
            sign * (oh * 3600 + om * 60)
        }
        _ => return Err(bad()),
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days_in = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&mo) || d < 1 || d > days_in[(mo - 1) as usize] || h > 23 || mi > 59 || se > 59 {
        return Err(bad());
    }
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset;
    Ok(secs * 1000)
}

/// `ms` since the epoch as UTC, to the second: `2026-11-23T05:34:37Z`.
pub fn utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s % 3600 / 60, s % 60)
}
