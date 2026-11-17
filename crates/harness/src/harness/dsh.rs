//! DeepSeek Harness: an interactive TUI on the profile its start directory chooses. The only
//! added plugin asks the guard and reports running/idle to the session backend.
use serde::{Deserialize, Deserializer};

use super::Link;
use super::session::{self, json_lines};
use super::store::Layout;
use crate::harness::Call;
use crate::line::Line;
use crate::start::Start;
use crate::turn::{Marker, Note, Turn};

pub(super) const KEY: &str = "dsh";
/// What a bare program is started with: nothing.
pub(super) const STOCK_ARGUMENTS: &str = "";
/// The variable that names DSH's user directory.
pub(super) const USER_DIR: &str = "DSH_HOME";

pub(super) const REFUSED: u8 = 2;
pub(super) const KEPT_LINKS: &[Link] = &[
    Link { home: ".dsh/.credentials.yaml", kept: ".credentials.yaml" },
    // Only dependency resolution is shared; neither the user's profile
    // manifest nor any patch is loaded by the tool's profile.
    Link {
        home: ".dsh/profiles/dsh-tui/node_modules/@deepseek-harness-tui/dsh-tui",
        kept: "profiles/dsh-tui/node_modules/@deepseek-harness-tui/dsh-tui",
    },
];

pub(super) fn environment(line: Line, start: &Start) -> Line {
    let line = line
        .fixed(" ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(start.home.dir())
        .fixed(" DSH_AGENTS_HOME=")
        .word(&format!("{}/agents", start.home.dir()))
        .fixed(" DSH_PERMISSION_MODE=danger-full-access");
    let line = line
        .fixed(" FACTORIUM_DSH_KEPT=")
        .word(start.home.dir())
        .fixed(" DSH_TUI_PROFILE=dsh-tui")
        .fixed(" FACTORIUM_GUARD=")
        .word(start.hook)
        .fixed(" FACTORIUM_WORKTREE=")
        .word(start.worktree)
        .fixed(" FACTORIUM_ADDED_DIR=")
        .word(start.dir);
    match start.model {
        Some(model) => line.fixed(" FACTORIUM_DSH_MODEL=").word(model),
        None => line,
    }
}

pub(super) fn added(line: Line, start: &Start) -> Line {
    let launch = line.done();
    Line::new()
        .launch("sh ")
        .word(&format!("{}/start.sh", start.home.dir()))
        .fixed(" ")
        .word(&launch)
        .fixed(" ")
        .word(start.home.dir())
        .fixed(" ")
        .word(start.dir)
        .fixed(" ")
        .word(start.worktree)
        .fixed(" ")
        .word("kept")
}

#[derive(Deserialize)]
struct Described {
    name: String,
    #[serde(default)]
    arguments: Arguments,
}
#[derive(Default, Deserialize)]
struct Arguments {
    command: Option<String>,
}

pub(super) fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.arguments.command {
        Some(line) if described.name == "bash" || described.name == "bash_persistent" => Call::Shell(line),
        _ => Call::Other,
    })
}

const SESSION_FILE: &str = "session.v4.jsonl.zstd";

/// The turn `text` records. The worktree is the directory the file was in.
/// DeepSeek Harness records no child session.
pub(super) fn recorded_turn(_cwd: &str, text: &str, not_before: Option<u64>) -> Option<Turn> {
    let notes: Vec<Note> = json_lines(text).filter_map(|value| note(&value)).collect();
    session::turn_of(&notes, not_before)
}

fn note(value: &serde_json::Value) -> Option<Note> {
    let at = session::epoch_number(value["time"].as_u64()?);
    match value["type"].as_str()? {
        "turn/start" => Some(Note { marker: Marker::Start, at, child: false }),
        "turn/end" => Some(Note { marker: Marker::End, at, child: false }),
        _ => None,
    }
}

/// The session a stand-in writes for this turn, before it is compressed.
pub(super) fn stand_in(open: bool, at: u64) -> String {
    let ms = at.saturating_mul(1000);
    let mut body = format!(
        "{}\n",
        session::object(vec![
            ("type", "turn/start".into()),
            ("time", ms.into()),
            ("data", session::object(vec![("turn", 1_u64.into())])),
        ])
    );
    if !open {
        body.push_str(&format!(
            "{}\n",
            session::object(vec![
                ("type", "turn/end".into()),
                ("time", ms.into()),
                (
                    "data",
                    session::object(vec![
                        ("turn", 1_u64.into()),
                        ("reason", session::object(vec![("kind", "completed".into())])),
                    ]),
                ),
            ])
        ));
    }
    body
}

/// Where DeepSeek Harness keeps the session of `cwd`: one compressed file
/// in each session directory under the worktree's directory.
pub(super) fn layout(cwd: &str) -> Layout {
    Layout::Named { dir: format!("sessions/{}", session::wrapped_dir(cwd)), name: SESSION_FILE }
}
