//! Grok, as a harness. Grok loads no hooks from a project file, so each start
//! gets a generated home whose config holds the start directory's hooks and
//! denies, the guard and the report. A session works only in its cwd, so it
//! starts in the worktree; the start directory's rules are told in that
//! home, and its skills are named there.

use serde::de::Error;
use serde::{Deserialize, Deserializer};

use super::session::{self, json_lines};
use super::{Link, SessionFile, SessionLayout};
use crate::harness::Call;
use crate::line::Line;
use crate::start::Start;
use crate::turn::{Marker, Note, Turn};

pub(super) const KEY: &str = "grok";

pub(super) const MODEL: &str = "--model";
/// On the launch line. Grok 1.0.70 accepts it and `--help` does not name it,
/// so it is not one of `FLAGS`: the flag check does not require `--help` to.
pub(super) const TRUST: &str = "--trust";

pub(super) const USER_DIR: &str = "GROK_HOME";
/// Nothing here is set to `=1`. The disables are `=0`, written in `environment`.
pub(super) const ENVIRONMENT: [&str; 0] = [];

pub(super) const KEPT_LINKS: &[Link] = &[Link { home: ".grok/auth.json", kept: "auth.json" }];

pub(super) const REFUSED: u8 = 2;

pub(super) fn environment(line: Line, start: &Start) -> Line {
    let line = line
        .fixed(" ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(start.home.dir())
        .fixed(" GROK_MEMORY=0")
        .fixed(" GROK_CLAUDE_HOOKS_ENABLED=0")
        .fixed(" GROK_CLAUDE_MCPS_ENABLED=0")
        .fixed(" GROK_CURSOR_HOOKS_ENABLED=0")
        .fixed(" GROK_CURSOR_MCPS_ENABLED=0");
    line.fixed(" GROK_CLAUDE_RULES_ENABLED=0")
        .fixed(" GROK_CLAUDE_AGENTS_ENABLED=0")
        .fixed(" GROK_CURSOR_RULES_ENABLED=0")
        .fixed(" GROK_CURSOR_AGENTS_ENABLED=0")
        .fixed(" GROK_CURSOR_SKILLS_ENABLED=0")
}

pub(super) fn added(line: Line, start: &Start) -> Line {
    // No `--add-dir`. Grok 1.0.60+ refuses `additionalDirectories` and the
    // session then never starts. The worktree is the cwd (`start_line`). Repo
    // and added rules are in the kept home, and so are the start directory's.
    let line = line.fixed(" ").fixed(TRUST);
    match start.model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    }
}

/// One tool call, as Grok hands it to a hook. The event names the tool in
/// camelCase and again in Claude's snake_case, and the input the same way
/// (`toolInput` and `tool_input`). Either spelling alone is the call. Both
/// spellings of one fact have to agree: an alias would reject the event as a
/// duplicate field, and judging one spelling while the tool runs the other
/// would let a command past the guard.
#[derive(Deserialize)]
struct Described {
    #[serde(rename = "toolName")]
    camel_name: Option<String>,
    #[serde(rename = "tool_name")]
    snake_name: Option<String>,
    #[serde(default, rename = "toolInput")]
    camel_input: Option<Input>,
    #[serde(default, rename = "tool_input")]
    snake_input: Option<Input>,
}

#[derive(Default, Deserialize)]
struct Input {
    #[serde(default)]
    command: Option<String>,
}

/// The one value the two spellings give. A spelling that is absent is the
/// other. Both present and different is not a value.
fn one_spelling<T: PartialEq, E: Error>(what: &str, camel: Option<T>, snake: Option<T>) -> Result<Option<T>, E> {
    match (camel, snake) {
        (Some(camel), Some(snake)) if camel != snake => {
            Err(E::custom(format!("`{what}` and its other spelling disagree")))
        }
        (Some(camel), _) => Ok(Some(camel)),
        (None, snake) => Ok(snake),
    }
}

pub(super) fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    let name = one_spelling("toolName", described.camel_name, described.snake_name)?
        .ok_or_else(|| D::Error::custom("a Grok tool call names no tool"))?;
    let command = one_spelling(
        "toolInput",
        described.camel_input.map(|input| input.command),
        described.snake_input.map(|input| input.command),
    )?
    .flatten();
    Ok(match command {
        Some(line) if name == "run_terminal_command" => Call::Shell(line),
        _ => Call::Other,
    })
}

const EVENTS: &str = "events.jsonl";

/// The same path → `%2FUsers%2Fsunfmin%2FDevelopments%2Fchessmirror`.
fn dir_name(cwd: &str) -> String {
    let mut out = String::new();
    for byte in cwd.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    session::stored_name(out, cwd)
}

/// The turn `text` records. The worktree is the directory the file was in.
/// A session that is not the primary one is a child.
pub(super) fn recorded_turn(_cwd: &str, text: &str, not_before: Option<u64>) -> Option<Turn> {
    let notes: Vec<Note> = json_lines(text).filter_map(|value| note(&value)).collect();
    session::turn_of(&notes, not_before)
}

fn note(value: &serde_json::Value) -> Option<Note> {
    let at = session::parse_iso(value["ts"].as_str()?)?;
    let child = value["session_relationship"].as_str().is_some_and(|kind| kind != "primary");
    match value["type"].as_str()? {
        "turn_started" => Some(Note { marker: Marker::Start, at, child }),
        "turn_ended" => Some(Note { marker: Marker::End, at, child }),
        _ => None,
    }
}

/// The events file a stand-in writes for this turn, and the summary beside
/// it. The summary is not the turn.
pub(super) fn session_files(cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
    let iso = session::format_iso(at);
    let dir = format!("sessions/{}/factorium-turn", dir_name(cwd));
    let mut body = format!(
        "{}\n",
        session::object(vec![
            ("ts", iso.clone().into()),
            ("type", "turn_started".into()),
            ("turn_number", 1_u64.into()),
            ("session_relationship", "primary".into()),
        ])
    );
    if !open {
        body.push_str(&format!(
            "{}\n",
            session::object(vec![
                ("ts", iso.clone().into()),
                ("type", "turn_ended".into()),
                ("outcome", "completed".into()),
            ])
        ));
    }
    vec![
        SessionFile { path: format!("{dir}/{EVENTS}"), body, compressed: false, replaces: None },
        SessionFile {
            path: format!("{dir}/summary.json"),
            body: session::object(vec![(
                "info",
                session::object(vec![("cwd", cwd.into()), ("created_at", iso.into())]),
            )])
            .to_string(),
            compressed: false,
            replaces: None,
        },
    ]
}

/// Where Grok keeps the session of `cwd`: one events file in each session
/// directory under the worktree's directory.
pub(super) fn session_layout(cwd: &str) -> SessionLayout {
    SessionLayout::Named { dir: format!("sessions/{}", dir_name(cwd)), name: EVENTS }
}
