//! What every harness's session shares: a clock stamp, a worktree path too
//! long for one directory component, and a line of JSON. Which lines start
//! or end a turn stays in that harness's module.

use serde_json::Value;

use crate::turn::{Note, Turn, latest};

/// Epoch seconds of an ISO-8601 stamp in UTC, or `None` when it is not one.
/// A fractional second is dropped. The offset is `Z` or zero.
pub fn parse_iso(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year: i64 = text[0..4].parse().ok()?;
    let month: u32 = text[5..7].parse().ok()?;
    let day: u32 = text[8..10].parse().ok()?;
    let hour: u64 = text[11..13].parse().ok()?;
    let minute: u64 = text[14..16].parse().ok()?;
    let second: u64 = text[17..19].parse().ok()?;
    let mut rest = &text[19..];
    if let Some(stripped) = rest.strip_prefix('.') {
        let digits = stripped.find(|ch: char| !ch.is_ascii_digit()).unwrap_or(stripped.len());
        rest = &stripped[digits..];
    }
    if rest != "Z" && rest != "+00:00" && rest != "-00:00" {
        return None;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    if second > 60 {
        return None;
    }
    let days = u64::try_from(days_from_civil(year, month, day)).ok()?;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// `at` as an ISO-8601 stamp in UTC, whole seconds.
pub(super) fn format_iso(at: u64) -> String {
    let (year, month, day) = civil_from_days((at / 86_400) as i64);
    let rest = at % 86_400;
    let hour = rest / 3600;
    let minute = (rest % 3600) / 60;
    let second = rest % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Epoch seconds. A value past year 2286 is milliseconds.
pub fn epoch_number(value: u64) -> u64 {
    if value > 10_000_000_000 { value / 1000 } else { value }
}

/// `seconds` when the line carries them, otherwise an ISO `timestamp`.
pub(super) fn number_at(seconds: Option<u64>, timestamp: Option<&str>) -> Option<u64> {
    seconds.map(epoch_number).or_else(|| timestamp.and_then(parse_iso))
}

/// The year, month and day of an epoch day count (Howard Hinnant's
/// civil-from-days), for a session laid out by date.
pub(super) fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    i32::try_from(year).map_or((1970, 1, 1), |year| (year, month as u32, day as u32))
}

/// The epoch day count of a civil date (Howard Hinnant's days-from-civil).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let day_of_year = (153 * month_index + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// One JSON object, field by field.
pub(super) fn object(fields: Vec<(&str, Value)>) -> Value {
    Value::Object(fields.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
}

/// The same worktree, ignoring a trailing slash.
pub(super) fn same_cwd(recorded: &str, cwd: &str) -> bool {
    recorded.trim_end_matches('/') == cwd.trim_end_matches('/')
}

/// Whether `text` names the absolute `path`: an occurrence not preceded by a
/// character a path holds, so `/a/b` does not name `/x/a/b`.
pub(super) fn names_path(text: &str, path: &str) -> bool {
    !path.is_empty()
        && text.match_indices(path).any(|(at, _)| {
            text.get(..at)
                .and_then(|before| before.chars().next_back())
                .is_none_or(|ch| !(ch.is_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '~' | '%' | '+' | '@')))
        })
}

/// One path component holds at most this many bytes. A worktree path longer
/// than that cannot be the harness's own directory name, so the record and
/// the reader share a short name for it. A path that fits keeps the name the
/// harness writes.
pub(super) fn stored_name(natural: String, cwd: &str) -> String {
    if natural.len() <= 255 { natural } else { format!("factorium-{}", mark(cwd)) }
}

/// A short name for `cwd`, shared by the record and the reader.
pub(super) fn mark(cwd: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The directory name pi and DeepSeek Harness write for `cwd`: the path with
/// a trailing slash, every character that is not a letter or a digit turned
/// into `-`, and a `-` at each end.
pub(super) fn wrapped_dir(cwd: &str) -> String {
    let mut with_slash = cwd.to_string();
    if !with_slash.ends_with('/') {
        with_slash.push('/');
    }
    let mut body = String::new();
    for ch in with_slash.chars() {
        if ch.is_ascii_alphanumeric() {
            body.push(ch);
        } else {
            body.push('-');
        }
    }
    stored_name(format!("-{body}-"), cwd)
}

/// The hook output Claude Code and Codex both read as ending the session's
/// turn: `continue: false`, with `reason` the `stopReason` shown.
pub(super) fn stopped(reason: &str) -> String {
    let reason = serde_json::Value::String(reason.to_string());
    format!("{{\"continue\":false,\"stopReason\":{reason}}}")
}

/// Each JSON object `text` holds, one a line. A blank line and a line that
/// is not JSON are not events.
pub(super) fn json_lines(text: &str) -> impl Iterator<Item = Value> + '_ {
    text.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() { None } else { serde_json::from_str(line).ok() }
    })
}

/// The turn `notes` leave, in the order the session wrote them.
pub(super) fn turn_of(notes: &[Note], not_before: Option<u64>) -> Option<Turn> {
    latest(notes, not_before)
}

/// Whether a session's `text` holds `mark` in a message the harness gave its
/// model as the user's: a line typed at its prompt, or one typed while it
/// worked that it handed over mid-turn. A line the harness only queued, a
/// tool's output and the model's own words are not.
pub(super) fn holds_note(text: &str, mark: &str) -> bool {
    !mark.is_empty() && json_lines(text).any(|line| user_texts(&line).iter().any(|said| said.contains(mark)))
}

/// When the first line of `text` that holds `mark` as the user's (as
/// `holds_note`) was recorded, by its `timestamp`: epoch seconds or
/// milliseconds, or an ISO stamp. A line with no readable stamp is passed.
pub(super) fn noted_at(text: &str, mark: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        if !holds_note(line, mark) {
            return None;
        }
        let value = serde_json::from_str::<Value>(line).ok()?;
        number_at(value["timestamp"].as_u64(), value["timestamp"].as_str())
    })
}

/// The first brief pointer in a user message: whether it names exactly
/// `brief`. `None` means this message has no pointer. Harnesses choose which
/// messages may associate their primary session with a session; this match
/// is shared by readers whose sessions start in a start directory.
pub(super) fn brief_match(line: &Value, brief: &str) -> Option<bool> {
    let expected = crate::brief::brief_pointer(brief);
    user_texts(line)
        .into_iter()
        .find(|said| said.starts_with(crate::brief::BRIEF_POINTER_PREFIX))
        .map(|said| said == expected)
}

/// What one session line gives the model as the user's words. Claude Code
/// writes a line typed mid-turn as a `queued_command` attachment once its
/// model has it, and a `queue-operation` line while it only waits in the
/// queue. Every harness here writes a message at its prompt with the start directory
/// or the type `user`, at the top of the line, under `message` or under
/// `payload`; Codex also as a `user_message` event.
fn user_texts(line: &Value) -> Vec<&str> {
    let attachment = &line["attachment"];
    if line["type"] == "attachment" && attachment["type"] == "queued_command" {
        return attachment["prompt"].as_str().into_iter().collect();
    }
    let mut said = Vec::new();
    for message in [line, &line["message"], &line["payload"]] {
        let user = message["role"] == "user" || message["type"] == "user" || message["type"] == "user_message";
        if !user {
            continue;
        }
        said.extend(message["message"].as_str());
        said.extend(message["text"].as_str());
        match &message["content"] {
            Value::String(text) => said.push(text.as_str()),
            Value::Array(parts) => said.extend(parts.iter().filter_map(text_part)),
            _ => {}
        }
    }
    said
}

/// A part of a message's content that is text, not a tool's result.
fn text_part(part: &Value) -> Option<&str> {
    match part["type"].as_str() {
        None | Some("text" | "input_text") => part["text"].as_str().or(part.as_str()),
        Some(_) => None,
    }
}
