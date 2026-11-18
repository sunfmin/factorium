//! Grok, as a harness. Grok loads no hooks from a project file, so its home's
//! config is generated at each start and holds the guard and the report: the
//! only way it gets the guard. A session works only in its cwd, so it starts
//! in the worktree; where the start directory's rules and skills are, and
//! the repo's rules, which Grok is kept from reading as Claude's, is told in
//! that home.

use serde::de::Error;
use serde::{Deserialize, Deserializer};

use super::{Dialog, Kind, Link, Login, Stop};
use super::session::{self, json_lines};
use super::store::Layout;
use crate::harness::{Call, Harness};
use crate::line::Line;
use crate::start::{Start, TOLD};
use crate::turn::{Marker, Note, Turn};
use toml::Value as Toml;

const MODEL: &str = "--model";
/// On the launch line. Grok 1.0.70 accepts it and `--help` does not name it,
/// so it is not one of `FLAGS`: the flag check does not require `--help` to.
const TRUST: &str = "--trust";

const USER_DIR: &str = "GROK_HOME";
/// Nothing here is set to `=1`. The disables are `=0`, written in `environment`.
const ENVIRONMENT: [&str; 0] = [];

const KEPT_LINKS: &[Link] = &[Link { home: ".grok/auth.json", kept: "auth.json" }];

/// Its configuration, and the rules it is told, in its user directory.
const CONFIG: &str = "config.toml";
const TOLD_FILE: &str = "rules/told.md";
/// Seconds Grok waits on one hook.
const HOOK_TIMEOUT: i64 = 10;

/// The events at which a session's state changes, besides `PreToolUse`,
/// which also asks the guard: each runs `REPORT`.
const STATE_EVENTS: [&str; 12] = [
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "PostToolUseFailure",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "StopFailure",
    "PreCompact",
    "PostCompact",
    "Notification",
    "SessionEnd",
];

/// The host user's skill and command directories, none of which a session
/// is given. `~` is the host user's home, which Grok expands in its config.
const HOME_SKILLS: [&str; 7] = [
    "~/.claude/skills",
    "~/.claude/commands",
    "~/.agents/skills",
    "~/.agents/commands",
    "~/.cursor/skills",
    "~/.grok/skills",
    "~/.grok/commands",
];

/// Tells Orca of the session's state through the hook Orca installs for
/// Grok, where it did; prints nothing and succeeds.
const REPORT: &str = "hook=\"${HOME-}/.orca/agent-hooks/grok-hook.sh\"; \
     if [ -x \"$hook\" ]; then ORCA_HOOK_AGENT_PID=\"$PPID\" /bin/sh \"$hook\"; \
     else cat >/dev/null; fi; exit 0";

/// The configuration a session gets. The host user's Grok config is not
/// read: its hooks, MCP servers, plugins, permission mode and memories stay
/// out, and so do Claude's and Cursor's of theirs, and every skill of the
/// host user's.
fn configuration(guard: &str) -> Result<String, String> {
    let off = |cells: &[&str]| Toml::Table(cells.iter().map(|cell| (cell.to_string(), Toml::from(false))).collect());
    let mut compat = toml::map::Map::new();
    compat.insert("claude".into(), off(&["hooks", "mcps", "rules", "agents"]));
    compat.insert("cursor".into(), off(&["hooks", "mcps", "skills", "rules", "agents"]));
    let mut skills = toml::map::Map::new();
    skills.insert("ignore".into(), Toml::Array(HOME_SKILLS.into_iter().map(Toml::from).collect()));
    let command = |command: &str| {
        let mut hook = toml::map::Map::new();
        hook.insert("type".into(), Toml::from("command"));
        hook.insert("command".into(), Toml::from(command));
        hook.insert("timeout".into(), Toml::from(HOOK_TIMEOUT));
        Toml::Table(hook)
    };
    let group = |hooks: Vec<Toml>| {
        Toml::Array(vec![Toml::Table([("hooks".to_string(), Toml::Array(hooks))].into_iter().collect())])
    };
    let mut hooks = toml::map::Map::new();
    hooks.insert(BEFORE_A_CALL.into(), group(vec![command(guard), command(REPORT)]));

    for event in STATE_EVENTS {
        hooks.insert(event.into(), group(vec![command(REPORT)]));
    }
    let mut table = toml::map::Map::new();
    table.insert("compat".into(), Toml::Table(compat));
    table.insert("skills".into(), Toml::Table(skills));
    table.insert("hooks".into(), Toml::Table(hooks));
    toml::to_string_pretty(&Toml::Table(table)).map_err(|error| error.to_string())
}

/// The event of Grok's before each tool call: where the guard is asked.
const BEFORE_A_CALL: &str = "PreToolUse";

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

fn note(value: &serde_json::Value) -> Option<Note> {
    let at = session::parse_iso(value["ts"].as_str()?)?;
    let child = value["session_relationship"].as_str().is_some_and(|kind| kind != "primary");
    match value["type"].as_str()? {
        "turn_started" => Some(Note { marker: Marker::Start, at, child }),
        "turn_ended" => Some(Note { marker: Marker::End, at, child }),
        _ => None,
    }
}

/// Grok, as a harness: what its module says of it.
pub(super) struct Grok;

impl Kind for Grok {
    const KEY: &'static str = "grok";
    const LOGIN: Login = Login::Machine { user_dir: USER_DIR };
    const USER_HOME: Option<&'static str> = None;
    const KEPT_LINKS: &'static [Link] = KEPT_LINKS;
    /// No approval prompts (its `auto-review` mode denies `factorium worker
    /// done`).
    const STOCK_ARGUMENTS: &'static str = "--always-approve";
    const ASKS_FIRST: Option<&'static str> = None;
    const SHOWS_UP: Option<&'static str> = None;
    const ASKS_MIDWAY: &'static [Dialog] = &[];
    const ENVIRONMENT: &'static [&'static str] = &ENVIRONMENT;
    const REFUSED: u8 = 2;
    /// Escape.
    const INTERRUPT: &'static str = "\x1b";

    /// What the host keeps for Grok beside the links: the configuration, and
    /// where its rules are.
    fn kept(start: &Start) -> Result<Vec<(&'static str, String)>, String> {
        Ok(vec![(CONFIG, configuration(&start.hook_asked(Harness::Grok))?), (TOLD_FILE, TOLD.into())])
    }

    const TOLD_IN: Option<&'static str> = Some(TOLD_FILE);

    /// The guard lines its kept configuration under `home` runs before a call.
    fn guards(home: &std::path::Path) -> Option<Vec<String>> {
        let config = std::fs::read_to_string(home.join(CONFIG)).ok().and_then(|text| text.parse::<Toml>().ok());
        let groups = config.as_ref().and_then(|config| config.get("hooks")?.get(BEFORE_A_CALL)?.as_array());
        let hooks = groups.into_iter().flatten().filter_map(|group| group.get("hooks")?.as_array()).flatten();
        Some(hooks.filter_map(|hook| hook.get("command")?.as_str().map(String::from)).collect())
    }

    /// Grok works only in its cwd, so it starts in the worktree.
    fn start_dir<'a>(start: &Start<'a>) -> &'a str {
        start.worktree
    }

    fn environment(line: Line, start: &Start) -> Line {
        let line = start.rules_env(line.set(USER_DIR, start.home.dir()));
        let line = line.arg("GROK_MEMORY=0").arg("GROK_CLAUDE_HOOKS_ENABLED=0").arg("GROK_CLAUDE_MCPS_ENABLED=0");
        let line = line.arg("GROK_CURSOR_HOOKS_ENABLED=0").arg("GROK_CURSOR_MCPS_ENABLED=0");
        let line = line.arg("GROK_CLAUDE_RULES_ENABLED=0").arg("GROK_CLAUDE_AGENTS_ENABLED=0");
        let line = line.arg("GROK_CURSOR_RULES_ENABLED=0").arg("GROK_CURSOR_AGENTS_ENABLED=0");
        line.arg("GROK_CURSOR_SKILLS_ENABLED=0")
    }

    /// No `--add-dir`. Grok 1.0.60+ refuses `additionalDirectories` and the
    /// session then never starts. The worktree is the cwd (`start_dir`).
    /// Repo and added rules are in the kept home, and so are the start
    /// directory's.
    fn added(line: Line, start: &Start) -> Line {
        line.arg(TRUST).flag_if(MODEL, start.model)
    }

    fn briefed(line: String, brief: &str) -> (String, Option<String>) {
        (line, Some(crate::brief::brief_pointer(brief)))
    }

    fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
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
            _ => Call::NotShell,
        })
    }

    fn stopped(_screen: &[String]) -> Option<Stop> {
        None
    }

    /// Where Grok keeps the session of `cwd`: one events file in each session
    /// directory under the worktree's directory.
    fn layout(cwd: &str) -> Layout {
        Layout::Named { dir: format!("sessions/{}", dir_name(cwd)), name: EVENTS }
    }

    fn could_record(_cwd: &str, _text: &str, _brief: &str) -> bool {
        true
    }

    /// The turn `text` records. The worktree is the directory the file was in.
    /// A session that is not the primary one is a child.
    fn recorded_turn(_cwd: &str, text: &str, _brief: &str) -> Option<Turn> {
        let notes: Vec<Note> = json_lines(text).filter_map(|value| note(&value)).collect();
        crate::turn::latest(&notes)
    }

    /// The events a stand-in writes for this turn.
    fn stand_in(_cwd: &str, _brief: &str, open: bool, at: u64) -> String {
        let iso = session::format_iso(at);
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
                session::object(vec![("ts", iso.into()), ("type", "turn_ended".into()), ("outcome", "completed".into())])
            ));
        }
        body
    }

    fn prompted(_text: &str, _said: &str, _since: u64) -> Option<bool> {
        None
    }
}
