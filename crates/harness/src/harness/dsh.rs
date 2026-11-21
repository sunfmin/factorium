//! DeepSeek Harness: an interactive TUI on the profile its start directory chooses. The only
//! added plugin asks the guard and reports running/idle to the session backend.
use serde::{Deserialize, Deserializer};

use super::session::{self, json_lines};
use super::store::Layout;
use super::{Dialog, Kind, Link, Login, Stop};
use crate::harness::{Call, Harness};
use crate::line::Line;
use crate::start::Start;
use crate::turn::{Marker, Note, Turn};

/// The variable that names DSH's user directory.
const USER_DIR: &str = "DSH_HOME";

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

const SESSION_FILE: &str = "session.v4.jsonl.zstd";

fn note(value: &serde_json::Value) -> Option<Note> {
    let at = session::epoch_number(value["time"].as_u64()?);
    match value["type"].as_str()? {
        "turn/start" => Some(Note { marker: Marker::Start, at, child: false }),
        "turn/end" => Some(Note { marker: Marker::End, at, child: false }),
        _ => None,
    }
}

/// DSH, as a harness: what its module says of it.
pub(super) struct Dsh;

impl Kind for Dsh {
    const KEY: &'static str = "dsh";
    const LOGIN: Login = Login::Unused;
    const USER_HOME: Option<&'static str> = None;
    const KEPT_LINKS: &'static [Link] = KEPT_LINKS;
    /// Nothing.
    const STOCK_ARGUMENTS: &'static str = "";
    /// The question dsh-tui asks before its screen is up (seen 2026-11, dsh's
    /// program having exited and been started again): typed text answers it.
    const ASKS_FIRST: Option<&'static str> = Some("进入安全模式");
    const SHOWS_UP: Option<&'static str> = None;
    const ASKS_MIDWAY: &'static [Dialog] = &[];
    const ENVIRONMENT: &'static [&'static str] = &[];
    const REFUSED: u8 = 2;
    /// Escape.
    const INTERRUPT: &'static str = "\x1b";
    /// Not asked headless: no run of it so has been read here.
    const HEADLESS: Option<crate::ask::Headless> = None;

    /// What the host keeps for DSH beside the links: the tool's profile, which
    /// loads none of the host user's patches; the overlay that mounts nothing
    /// before the guard; the guard's plugin; and the script the line starts it
    /// through, which refuses a session whose guard files are not there.
    fn kept(start: &Start) -> Result<Vec<(&'static str, String)>, String> {
        let plugin = format!("{}/{PLUGIN}", start.home.dir()).replace('\'', "''");
        Ok(vec![
            ("profiles/dsh-tui/package.json", include_str!("dsh/profile.json").into()),
            ("profiles/dsh-tui/cordis.yml", "[]\n".into()),
            (OVERLAY, include_str!("dsh/overlay.yml").replace("@FACTORIUM_DSH_PLUGIN@", &plugin)),
            ("sandbox.patch.yml", include_str!("dsh/sandbox.yml").into()),
            (PLUGIN, include_str!("dsh/plugin.mjs").into()),
            (START, include_str!("dsh/start.sh").into()),
        ])
    }

    const TOLD_IN: Option<&'static str> = None;

    /// The guard line the kept plugin runs, its variable's: where the start
    /// script starts DSH on the overlay, the overlay injects the plugin by its
    /// kept path, and the plugin reads the variable.
    fn guards(home: &std::path::Path) -> Option<Vec<String>> {
        let read = |file: &str| std::fs::read_to_string(home.join(file)).unwrap_or_default();
        let (overlay, plugin) = (read(OVERLAY), format!("{}/{PLUGIN}", home.display()).replace('\'', "''"));
        let mounted = overlay.contains(INJECTED) && overlay.contains(&plugin) && read(START).contains(OVERLAY);
        Some((mounted && read(PLUGIN).contains(GUARD_ENV)).then(|| format!("${GUARD_ENV}")).into_iter().collect())
    }

    fn start_dir<'a>(start: &Start<'a>) -> &'a str {
        start.dir
    }

    fn environment(line: Line, start: &Start) -> Line {
        let home = start.home.dir();
        let line = line.set(USER_DIR, home).fixed(" DSH_AGENTS_HOME=").word(&format!("{home}/agents"));
        let line = line.arg("DSH_PERMISSION_MODE=danger-full-access").fixed(" FACTORIUM_DSH_KEPT=").word(home);
        let line = line.arg("DSH_TUI_PROFILE=dsh-tui").set(GUARD_ENV, &start.hook(Harness::Dsh));
        let line = start.rules_env(line);
        match start.model {
            Some(model) => line.fixed(" FACTORIUM_DSH_MODEL=").word(model),
            None => line,
        }
    }

    fn added(line: Line, start: &Start) -> Line {
        let (launch, home, pointer) = (line.done(), start.home.dir(), crate::brief::brief_pointer(start.brief));
        let mut line = Line::new().launch("sh ").word(&format!("{home}/{START}"));
        for word in [&launch, home, start.dir, start.worktree, "kept", &pointer] {
            line = line.fixed(" ").word(word);
        }
        line
    }

    /// Its start script hands it the brief as its first prompt.
    fn briefed(line: String, _brief: &str) -> (String, Option<String>) {
        (line, None)
    }

    fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
        let described = Described::deserialize(described)?;
        Ok(match described.arguments.command {
            Some(line) if described.name == "bash" || described.name == "bash_persistent" => Call::Shell(line),
            _ => Call::NotShell,
        })
    }

    fn stopped(_screen: &[String]) -> Option<Stop> {
        None
    }

    /// Where DeepSeek Harness keeps the session of `cwd`: one compressed file
    /// in each session directory under the worktree's directory.
    fn layout(cwd: &str) -> Layout {
        Layout::Named { dir: format!("sessions/{}", session::wrapped_dir(cwd)), name: SESSION_FILE }
    }

    fn could_record(_cwd: &str, _text: &str, _brief: &str) -> bool {
        true
    }

    /// The turn `text` records. The worktree is the directory the file was in.
    /// DeepSeek Harness records no child session.
    fn recorded_turn(_cwd: &str, text: &str, _brief: &str) -> Option<Turn> {
        let notes: Vec<Note> = json_lines(text).filter_map(|value| note(&value)).collect();
        crate::turn::latest(&notes)
    }

    /// The session a stand-in writes for this turn, before it is compressed.
    fn stand_in(_cwd: &str, _brief: &str, open: bool, at: u64) -> String {
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

    fn prompted(_text: &str, _said: &str, _since: u64) -> Option<bool> {
        None
    }
}
