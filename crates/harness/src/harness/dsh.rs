//! DeepSeek Harness: an interactive TUI on the profile its start directory chooses. The only
//! added plugin asks the guard and reports running/idle to the session backend.
use serde::{Deserialize, Deserializer};

use super::{Dialog, Kind, Link, Login, Stop};
use super::session::{self, json_lines};
use super::store::Layout;
use crate::harness::{Call, Harness};
use crate::line::Line;
use crate::start::{START_DIR_ENV, Start, WORKTREE_ENV};
use crate::turn::{Marker, Note, Turn};

const KEY: &str = "dsh";
/// What a bare program is started with: nothing.
const STOCK_ARGUMENTS: &str = "";
/// The variable that names DSH's user directory.
const USER_DIR: &str = "DSH_HOME";

const REFUSED: u8 = 2;
/// The question dsh-tui asks before its screen is up (seen 2026-11, dsh's
/// program having exited and been started again): typed text answers it.
const ASKS_FIRST: &str = "进入安全模式";
const KEPT_LINKS: &[Link] = &[
    Link { home: ".dsh/.credentials.yaml", kept: ".credentials.yaml" },
    // Only dependency resolution is shared; neither the user's profile
    // manifest nor any patch is loaded by the tool's profile.
    Link {
        home: ".dsh/profiles/dsh-tui/node_modules/@deepseek-harness-tui/dsh-tui",
        kept: "profiles/dsh-tui/node_modules/@deepseek-harness-tui/dsh-tui",
    },
];

/// The plugin that asks the guard, as the overlay names it.
const PLUGIN: &str = "factorium.mjs";
/// The overlay that mounts it, and the script the line starts DSH through.
const OVERLAY: &str = "factorium.patch.yml";
const START: &str = "start.sh";
/// What the overlay injects the plugin as.
const INJECTED: &str = "factoriumGuard";
/// The variable the plugin reads the line that runs the guard from.
const GUARD_ENV: &str = "FACTORIUM_GUARD";

/// What the host keeps for DSH beside the links: the tool's profile, which
/// loads none of the host user's patches; the overlay that mounts nothing
/// before the guard; the guard's plugin; and the script the line starts it
/// through, which refuses a session whose guard files are not there.
fn kept(start: &Start) -> Vec<(&'static str, String)> {
    let plugin = format!("{}/{PLUGIN}", start.home.dir()).replace('\'', "''");
    vec![
        ("profiles/dsh-tui/package.json", include_str!("dsh/profile.json").into()),
        ("profiles/dsh-tui/cordis.yml", "[]\n".into()),
        (OVERLAY, include_str!("dsh/overlay.yml").replace("@FACTORIUM_DSH_PLUGIN@", &plugin)),
        ("sandbox.patch.yml", include_str!("dsh/sandbox.yml").into()),
        (PLUGIN, include_str!("dsh/plugin.mjs").into()),
        (START, include_str!("dsh/start.sh").into()),
    ]
}

/// The guard line the kept plugin runs, its variable's: where the start
/// script starts DSH on the overlay, the overlay injects the plugin by its
/// kept path, and the plugin reads the variable.
fn guards(home: &std::path::Path) -> Vec<String> {
    let read = |file: &str| std::fs::read_to_string(home.join(file)).unwrap_or_default();
    let (overlay, plugin) = (read(OVERLAY), format!("{}/{PLUGIN}", home.display()).replace('\'', "''"));
    let mounted = overlay.contains(INJECTED) && overlay.contains(&plugin) && read(START).contains(OVERLAY);
    (mounted && read(PLUGIN).contains(GUARD_ENV)).then(|| format!("${GUARD_ENV}")).into_iter().collect()
}

fn environment(line: Line, start: &Start) -> Line {
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
        .fixed(" ")
        .fixed(GUARD_ENV)
        .fixed("=")
        .word(&start.hook(Harness::Dsh))
        .fixed(" ")
        .fixed(WORKTREE_ENV)
        .fixed("=")
        .word(start.worktree)
        .fixed(" ")
        .fixed(START_DIR_ENV)
        .fixed("=")
        .word(start.dir);
    match start.model {
        Some(model) => line.fixed(" FACTORIUM_DSH_MODEL=").word(model),
        None => line,
    }
}

fn added(line: Line, start: &Start) -> Line {
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
        .fixed(" ")
        .word(&crate::brief::brief_pointer(start.brief))
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

fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.arguments.command {
        Some(line) if described.name == "bash" || described.name == "bash_persistent" => Call::Shell(line),
        _ => Call::Other,
    })
}

const SESSION_FILE: &str = "session.v4.jsonl.zstd";

/// The turn `text` records. The worktree is the directory the file was in.
/// DeepSeek Harness records no child session.
fn recorded_turn(_cwd: &str, text: &str) -> Option<Turn> {
    let notes: Vec<Note> = json_lines(text).filter_map(|value| note(&value)).collect();
    session::turn_of(&notes)
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
fn stand_in(open: bool, at: u64) -> String {
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
fn layout(cwd: &str) -> Layout {
    Layout::Named { dir: format!("sessions/{}", session::wrapped_dir(cwd)), name: SESSION_FILE }
}

/// DSH, as a harness: what its module says of it.
pub(super) struct Dsh;

impl Kind for Dsh {
    const KEY: &'static str = KEY;
    const LOGIN: Login = Login::Unused;
    const USER_HOME: Option<&'static str> = None;
    const KEPT_LINKS: &'static [Link] = KEPT_LINKS;
    const STOCK_ARGUMENTS: &'static str = STOCK_ARGUMENTS;
    const ASKS_FIRST: Option<&'static str> = Some(ASKS_FIRST);
    const SHOWS_UP: Option<&'static str> = None;
    const ASKS_MIDWAY: &'static [Dialog] = &[];
    const ENVIRONMENT: &'static [&'static str] = &[];
    const REFUSED: u8 = REFUSED;
    /// Escape.
    const INTERRUPT: &'static str = "\x1b";

    fn kept(start: &Start) -> Result<Vec<(&'static str, String)>, String> {
        Ok(kept(start))
    }

    const TOLD_IN: Option<&'static str> = None;

    fn guards(home: &std::path::Path) -> Option<Vec<String>> {
        Some(guards(home))
    }

    fn start_dir<'a>(start: &Start<'a>) -> &'a str {
        start.dir
    }

    fn environment(line: Line, start: &Start) -> Line {
        environment(line, start)
    }

    fn added(line: Line, start: &Start) -> Line {
        added(line, start)
    }

    /// Its start script hands it the brief as its first prompt.
    fn briefed(line: String, _brief: &str) -> (String, Option<String>) {
        (line, None)
    }

    fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
        call(described)
    }

    fn stopped(_screen: &[String]) -> Option<Stop> {
        None
    }

    fn layout(cwd: &str) -> Layout {
        layout(cwd)
    }

    fn could_record(_cwd: &str, _text: &str, _brief: &str) -> bool {
        true
    }

    fn recorded_turn(cwd: &str, text: &str, _brief: &str) -> Option<Turn> {
        recorded_turn(cwd, text)
    }

    fn stand_in(_cwd: &str, _brief: &str, open: bool, at: u64) -> String {
        stand_in(open, at)
    }

    fn prompted(_text: &str, _said: &str, _since: u64) -> Option<bool> {
        None
    }
}
