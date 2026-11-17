//! Kimi Code, as a harness: its isolated data home carries only the host's
//! providers and models, the start directory's limits and the added hooks.
//! Kimi has no project-level hook configuration, so each start gets a
//! generated home of its own.

use serde::{Deserialize, Deserializer};

use super::session::{self, json_lines};
use super::{Link, SessionFile, SessionLayout};
use crate::harness::Call;
use crate::line::Line;
use crate::start::Start;
use crate::turn::{Marker, Note, Turn};

/// One session file the index names.
pub(super) struct Indexed {
    /// Absolute, or relative to the harness directory.
    pub(super) file: String,
}

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
    line.fixed(" ").fixed(USER_DIR).fixed("=").word(start.kept)
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

pub(super) const INDEX: &str = "session_index.jsonl";
const WIRE: &str = "agents/main/wire.jsonl";

/// Where Kimi keeps the sessions of every worktree: one index. The index
/// names each session file.
pub(super) fn session_layout(_cwd: &str) -> SessionLayout {
    SessionLayout::Index { file: INDEX.into() }
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

/// `existing` with this worktree's line replaced. Lines for other worktrees,
/// and a line that is not JSON, stay.
pub(super) fn session_index(cwd: &str, existing: &str) -> String {
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
    let session = format!("sessions/factorium-{}", session::mark(cwd));
    kept.push_str(
        &session::object(vec![
            ("sessionId", session::mark(cwd).into()),
            ("sessionDir", session.into()),
            ("workDir", cwd.into()),
        ])
        .to_string(),
    );
    kept.push('\n');
    kept
}

/// The wire a stand-in writes for this turn. The index is `session_index`.
pub(super) fn session_files(cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
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
    vec![SessionFile {
        path: format!("sessions/factorium-{}/{WIRE}", session::mark(cwd)),
        body,
        compressed: false,
        replaces: None,
    }]
}

/// The session files the index names.
pub(super) fn indexed_sessions(index: &str) -> Vec<Indexed> {
    let mut out = Vec::new();
    for value in json_lines(index) {
        let Some(session) = value["sessionDir"].as_str() else { continue };
        if session.is_empty() {
            continue;
        }
        let file = std::path::Path::new(session).join(WIRE);
        out.push(Indexed { file: file.to_string_lossy().into_owned() });
    }
    out
}
