//! Kimi Code, as a harness: its isolated data home carries only the host's
//! providers and models, the start directory's limits and the added hooks.
//! Kimi has no project-level hook configuration, so each start gets a
//! generated home of its own.

use serde::{Deserialize, Deserializer};

use super::Link;
use super::session::{self, json_lines};
use super::store::Layout;
use crate::harness::Call;
use crate::line::Line;
use crate::start::Start;
use crate::turn::{Marker, Note, Turn};

pub(super) const KEY: &str = "kimi";

const ADD_DIR: &str = "--add-dir";
const SKILLS_DIR: &str = "--skills-dir";
pub(super) const MODEL: &str = "--model";

pub(super) const USER_DIR: &str = "KIMI_CODE_HOME";
pub(super) const ENVIRONMENT: [&str; 3] =
    ["KIMI_CODE_TRUST_WORKSPACE", "KIMI_CODE_NO_AUTO_UPDATE", "KIMI_DISABLE_TELEMETRY"];

pub(super) const KEPT_LINKS: &[Link] = &[Link { home: ".kimi-code/credentials", kept: "credentials" }];

pub(super) const REFUSED: u8 = 2;

pub(super) fn environment(line: Line, start: &Start) -> Line {
    line.fixed(" ").fixed(USER_DIR).fixed("=").word(start.home.dir())
}

pub(super) fn added(line: Line, start: &Start) -> Line {
    let line = line
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.worktree)
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.dir)
        .fixed(" ")
        .fixed(SKILLS_DIR)
        .fixed(" ")
        .word(&format!("{}/{}", start.dir, crate::start::SKILLS));
    match start.model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    }
}

#[derive(Deserialize)]
struct Described {
    tool_name: String,
    #[serde(default)]
    tool_input: Input,
}

#[derive(Default, Deserialize)]
struct Input {
    #[serde(default)]
    command: Option<String>,
}

pub(super) fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.tool_input.command {
        Some(line) if described.tool_name == "Bash" => Call::Shell(line),
        _ => Call::Other,
    })
}

/// Kimi Code's own usage-limit error.
/// The provider prefix is part of the words: a bare "5-hour usage limit"
/// on a screen is not this quota.
const USAGE_LIMIT: &str = "[provider.api_error] 429 You've reached your 5-hour usage limit";

/// Kimi Code's account-quota screen. The error may wrap; a terminal's
/// line break is whitespace. The same words on another harness are not it.
pub(super) fn account_quota(output: &[String]) -> Option<String> {
    let flat = output.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    if !flat.contains(USAGE_LIMIT) {
        return None;
    }
    Some(
        output
            .iter()
            .find(|line| line.split_whitespace().collect::<Vec<_>>().join(" ").contains("5-hour usage limit"))
            .cloned()
            .unwrap_or_else(|| USAGE_LIMIT.to_string()),
    )
}

/// Where Kimi keeps the sessions of every worktree: one index names each
/// worktree's session directories, and each holds its wire.
pub(super) fn layout() -> Layout {
    Layout::Index { index: "session_index.jsonl", name: "agents/main/wire.jsonl" }
}

/// The turn `text`, one session's wire, records. The worktree was named by
/// the index that pointed here. Kimi records no child session. The wire's
/// other lines are huge and are not a turn: the type is the whole match, so
/// a prompt that mentions a turn is not one.
pub(super) fn recorded_turn(_cwd: &str, text: &str, not_before: Option<u64>) -> Option<Turn> {
    let notes: Vec<Note> = json_lines(text).filter_map(|value| note(&value)).collect();
    session::turn_of(&notes, not_before)
}

fn note(value: &serde_json::Value) -> Option<Note> {
    let at = session::epoch_number(value["time"].as_u64()?);
    match value["type"].as_str()? {
        "agent.turn.started" => Some(Note { marker: Marker::Start, at, child: false }),
        "agent.turn.ended" | "turn.ended" => Some(Note { marker: Marker::End, at, child: false }),
        _ => None,
    }
}

/// The index `existing` with this worktree's line replaced by one naming
/// its session directory `dir`. Lines for other worktrees, and a line that
/// is not JSON, stay.
pub(super) fn session_index(cwd: &str, existing: &str, dir: &str) -> String {
    let mut kept = String::new();
    for line in existing.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            kept.push_str(line);
            kept.push('\n');
            continue;
        };
        if value["workDir"].as_str().is_some_and(|dir| session::same_cwd(dir, cwd)) {
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept.push_str(
        &session::object(vec![
            ("sessionId", session::mark(cwd).into()),
            ("sessionDir", dir.into()),
            ("workDir", cwd.into()),
        ])
        .to_string(),
    );
    kept.push('\n');
    kept
}

/// The wire a stand-in writes for this turn.
pub(super) fn stand_in(open: bool, at: u64) -> String {
    let ms = at.saturating_mul(1000);
    let mut body = format!(
        "{}\n",
        session::object(vec![
            ("type", "agent.turn.started".into()),
            ("time", ms.into()),
            ("turnId", 0_u64.into()),
            ("kind", "event".into()),
        ])
    );
    if !open {
        body.push_str(&format!(
            "{}\n",
            session::object(vec![
                ("type", "agent.turn.ended".into()),
                ("time", ms.into()),
                ("turnId", 0_u64.into()),
                ("outcome", "completed".into()),
            ])
        ));
    }
    body
}

/// The session directories `index` names for the worktree `cwd`: absolute,
/// or relative to the harness directory.
pub(super) fn session_dirs(index: &str, cwd: &str) -> Vec<String> {
    json_lines(index)
        .filter(|value| value["workDir"].as_str().is_some_and(|dir| session::same_cwd(dir, cwd)))
        .filter_map(|value| value["sessionDir"].as_str().filter(|dir| !dir.is_empty()).map(str::to_string))
        .collect()
}
