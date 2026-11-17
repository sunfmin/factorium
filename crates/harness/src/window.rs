//! What an engine says of its account's usage windows and of a limit it
//! met: how much of each window is used and when it
//! resets, as the engine itself reports them, never predicted.
//!
//! Which record says it, and in which words, is each harness's
//! (`Harness::windows`, `Harness::limit_event`). This module holds only what
//! they are read into, and the small readings they share.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One usage window of an account, as its engine reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// The engine's own name for the window: Claude Code's `five_hour` or
    /// `seven_day`, Codex's `primary` or `secondary`, Kimi Code's `limit_5h`.
    pub name: String,
    /// How much of it is used, from 0 to 100.
    pub used_percent: f64,
    /// How long it is, in minutes, where the engine says (Codex does).
    pub minutes: Option<u64>,
    /// When it resets, in unix seconds, where the engine says.
    pub resets_at: Option<u64>,
}

/// What kind of limit an engine says it met.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Limited {
    /// The account's usage of a window ran out: it holds until that window
    /// resets.
    Usage,
    /// The service throttled requests for a while: not the account's usage.
    Throttle,
}

/// A limit an engine recorded meeting, read from its structured fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limit {
    pub kind: Limited,
    /// The engine's name for the window that ran out, where it says.
    pub window: Option<String>,
    /// When the limit lifts, in unix seconds, where the engine says.
    pub resets_at: Option<u64>,
}

/// Where a harness reads the command its status line runs, which it hands
/// what the status line shows on stdin: a key of a settings file of the
/// host's user. The value there is [`StatusLine::entry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusLine {
    /// The settings file, under the home of the host's user.
    pub settings: &'static str,
    /// The key of that file that holds the status line.
    pub key: &'static str,
}

impl StatusLine {
    /// The value of [`StatusLine::key`] that runs `command` as the status
    /// line. What `command` prints is shown; what it is handed on stdin is
    /// what `Harness::windows` reads.
    pub fn entry(&self, command: &str) -> Value {
        let mut entry = serde_json::Map::new();
        entry.insert("type".to_string(), Value::from("command"));
        entry.insert("command".to_string(), Value::from(command));
        Value::Object(entry)
    }
}

/// Unix seconds of a number or an ISO-8601 stamp in UTC.
pub(crate) fn seconds(value: &Value) -> Option<u64> {
    if let Some(n) = value.as_u64() {
        return Some(crate::harness::epoch_number(n));
    }
    if let Some(n) = value.as_f64() {
        return (n >= 0.0).then(|| crate::harness::epoch_number(n as u64));
    }
    value.as_str().and_then(crate::harness::parse_iso)
}

/// A number, or a number written as a string.
pub(crate) fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(text) => text.trim().parse().ok().filter(|n: &f64| n.is_finite()),
        _ => None,
    }
}

/// When the line was written, in unix seconds: its `timestamp`.
pub(crate) fn written_at(line: &Value) -> Option<u64> {
    seconds(&line["timestamp"])
}
