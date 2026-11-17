//! Claude Code, as a harness. It loads a directory's rules, skills and hooks
//! from the directory it starts in, so a start adds only the rest: the
//! directories beside it, the settings it may load, and the hooks that ask
//! the guard and report its status. The host user's skills and rules are one
//! more directory, kept by the host and holding nothing else of the user's,
//! so leaving their settings out still leaves out their hooks.

use serde::de::{Error, IgnoredAny};
use serde::{Deserialize, Deserializer};

use super::{Kind, Link, Login, Stop};
use super::session::{self, json_lines};
use super::store::Layout;
use crate::harness::Call;
use crate::line::Line;
use crate::start::Start;
use crate::turn::{Marker, Note, Turn};

const KEY: &str = "claude-code";
/// What a bare program is started with: no permission prompts.
const STOCK_ARGUMENTS: &str = "--dangerously-skip-permissions";

/// The field holding an account's token, which `claude setup-token` made:
/// also the variable a session is given it in.
const TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// The setting that answers the warning Claude Code opens with when it is
/// started to work with nobody at its terminal: a session has nobody to accept
/// it, and stops there. On a host whose user accepted it once, their settings
/// say the same. Written as a member of a JSON object.
macro_rules! warning_accepted {
    () => {
        "\"skipDangerousModePermissionPrompt\":true"
    };
}
/// Turns off Claude Code's prompt suggestions: the grey ghost text it puts in
/// the prompt box once a turn ends. A session, an interactive session and the asking session
/// have nobody at that prompt, and whoever reads their screen could take the
/// suggestion for a person's word. The variable wins
/// over every settings file, and is set in the launch, so the host user's own
/// settings are left as they are.
const NO_SUGGESTIONS: &str = "CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=false";

/// The environment a session in its start directory sets for Claude Code beyond the
/// switches `ENVIRONMENT` names: its prompt suggestions off. Its home is
/// the host user's own (`Harness::home`), so nothing points it elsewhere.
fn environment(line: Line, _start: &Start) -> Line {
    line.fixed(" ").fixed(NO_SUGGESTIONS)
}

/// Claude Code's flag for a directory added to the one it was started in.
const ADD_DIR: &str = "--add-dir";
/// Claude Code's flag for which settings it loads.
const SETTING_SOURCES: &str = "--setting-sources";
/// Claude Code's flag for the model of the session.
const MODEL: &str = "--model";
/// Claude Code's flag for settings added to those it loads.
const SETTINGS: &str = "--settings";

/// The settings a session loads: those of the directory it starts in, and
/// nothing of the host user's: no hook, no MCP server, no permission.
const SOURCES: &str = "project";

/// The event of Claude Code before each tool call: where the guard is asked.
const BEFORE_A_CALL: &str = "PreToolUse";
/// The events of Claude Code at which a session's state changes: what the
/// session backend hears of, to report it (`REPORT`). With the user's
/// settings left out, nothing else tells it.
const STATE_EVENTS: [&str; 12] = [
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "SubagentStart",
    "SubagentStop",
    "TeammateIdle",
    "PostCompact",
    "Stop",
    "StopFailure",
    "SessionEnd",
];
/// How long Claude Code waits on one report, in seconds: a session backend
/// that does not answer holds no tool call up.
const REPORT_TIMEOUT: u64 = 10;
/// Has Claude Code load the rules of the directories added to it: the repo's
/// own, in the worktree, and the added rules, in the added directory's copy. Their hooks
/// are not loaded: what a repo must enforce belongs in its gate.
const ADDED_RULES: &str = "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD";
/// Keeps Claude Code from carrying what one session learned to the next on
/// this host: a start directory is what its repo says, the same on every host.
const NO_MEMORY: &str = "CLAUDE_CODE_DISABLE_AUTO_MEMORY";
/// The variables a session's environment sets for Claude Code.
const ENVIRONMENT: [&str; 3] = [ADDED_RULES, NO_MEMORY, "CLAUDE_CODE_SANDBOXED"];

/// What a host keeps for Claude Code: the host user's skills and rules, as
/// a directory added to a session's holds them, and nothing else. Claude Code
/// loads the skills and the rules of an added directory, and not its
/// settings; a skill of the start directory's wins over one of the same name there.
const KEPT_LINKS: &[Link] = &[];

/// The shell line that tells orca of a session's state: the hook orca installs
/// on its host for Claude Code, which reads the event on standard input and
/// knows its terminal from the environment orca gave it. orca puts the same
/// line in the user's settings, which a session in its start directory does not load. On
/// a host where orca installed none, nothing is told.
const REPORT: &str = "hook=\"${HOME-}/.orca/agent-hooks/claude-hook.sh\"; if [ -x \"$hook\" ]; \
     then ORCA_HOOK_AGENT_PID=\"$PPID\" /bin/sh \"$hook\"; else cat >/dev/null; fi";

/// The tool Claude Code runs shell lines with.
const SHELL_TOOL: &str = "Bash";
/// The exit code that tells Claude Code a hook refuses the call.
const REFUSED: u8 = 2;

/// `text` as a JSON string, quotes and all.
fn json_text(text: &str) -> String {
    let mut json = String::from("\"");
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                json.push('\\');
                json.push(c);
            }
            // A shell line holds no control character worth carrying.
            c if c.is_control() => json.push(' '),
            c => json.push(c),
        }
    }
    json.push('"');
    json
}

/// The settings added to a session's: the guard, a hook Claude Code runs
/// before every tool call with the line `guard`, the report, a hook it
/// runs with the line `report` wherever the session's state changes, and the
/// answer to the warning it would stop at, whatever command starts it. They
/// are added to the start directory's own hooks, which run beside them.
fn added_settings(guard: &str, report: &str) -> String {
    let guard = format!("{{\"type\":\"command\",\"command\":{}}}", json_text(guard));
    let report = format!("{{\"type\":\"command\",\"command\":{},\"timeout\":{REPORT_TIMEOUT}}}", json_text(report));
    let mut settings = format!("{{\"hooks\":{{\"{BEFORE_A_CALL}\":[{{\"hooks\":[{guard},{report}]}}]");
    for event in STATE_EVENTS {
        settings.push_str(&format!(",\"{event}\":[{{\"hooks\":[{report}]}}]"));
    }
    settings.push_str(concat!("},", warning_accepted!()));
    settings.push('}');
    settings
}

/// `line`, which starts the launch command, with what Claude Code is
/// given to put a session in its start directory: the worktree and the added directory's copy added,
/// and the host user's skills and rules where it is given them; the user's
/// settings left out, the guard and the report added, and the caller's model.
fn added(line: Line, start: &Start) -> Line {
    let line = line
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.worktree)
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.dir);
    let line = line
        .fixed(" ")
        .fixed(SETTING_SOURCES)
        .fixed(" ")
        .fixed(SOURCES)
        .fixed(" ")
        .fixed(SETTINGS)
        .fixed(" ")
        .word(&added_settings(start.hook, REPORT));
    match start.model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    }
}

/// The start `line` with the brief pointer to `brief` as the session's first
/// prompt. `--` ends its flags first: `--add-dir` takes every word after it,
/// so a prompt after it alone would be read as a directory.
fn brief_at_start(line: &str, brief: &str) -> String {
    Line::new().launch(line).fixed(" -- ").word(&crate::brief::brief_pointer(brief)).done()
}

/// One tool call, as Claude Code describes it to a hook before making it.
#[derive(Deserialize)]
struct Described {
    tool_name: String,
    #[serde(default)]
    tool_input: Input,
}

#[derive(Default, Deserialize)]
struct Input {
    /// The shell line, when the tool runs one.
    #[serde(default)]
    command: Option<String>,
}

/// The tool call `described` to a hook, as the guard judges one.
fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.tool_input.command {
        Some(line) if described.tool_name == SHELL_TOOL => Call::Shell(line),
        _ => Call::Other,
    })
}

/// The harness directory under a user's home.
const HOME: &str = ".claude";

/// Sessions are filed under the directory Claude Code starts in, which is
/// the start directory for a bound session. The session identifies its worktree.
fn layout() -> Layout {
    Layout::Tree { dir: "projects".into(), extension: "jsonl" }
}

/// The turn for `cwd`, or for the session its initial brief pointer names
/// when Claude Code starts in a start directory. A sidechain is a child session.
fn recorded_turn(cwd: &str, text: &str, not_before: Option<u64>, brief: Option<&str>) -> Option<Turn> {
    let named = brief.is_some_and(|brief| names_brief(text, brief));
    // Each line is read once, into only what a turn needs: a long session
    // is tens of megabytes of tool output nobody looks at here
    //.
    let mut entries: Vec<Entry> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str(line) {
            entries.push(entry);
        }
    }
    let here = entries
        .iter()
        .any(|entry| !entry.sidechain() && entry.cwd().is_some_and(|recorded| session::same_cwd(recorded, cwd)));
    if !here && !named {
        return None;
    }
    let notes: Vec<Note> = entries.iter().filter_map(|entry| note(cwd, entry, named)).collect();
    session::turn_of(&notes, not_before)
}

/// What a turn reads of one session line. Every other field is skipped
/// unparsed; a field of another shape reads as absent, as a JSON value's
/// lookup would.
#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "isSidechain", default)]
    sidechain: Option<serde_json::Value>,
    #[serde(default)]
    cwd: Option<serde_json::Value>,
    #[serde(default)]
    timestamp: Option<serde_json::Value>,
    #[serde(rename = "type", default)]
    kind: Option<serde_json::Value>,
    #[serde(rename = "isCompactSummary", default)]
    compact: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "stop_reason")]
    message: Option<String>,
}

impl Entry {
    fn sidechain(&self) -> bool {
        self.sidechain == Some(serde_json::Value::Bool(true))
    }

    fn cwd(&self) -> Option<&str> {
        self.cwd.as_ref()?.as_str()
    }
}

/// A message's `stop_reason` when it is a string; anything else is skipped.
fn stop_reason<'de, D: Deserializer<'de>>(message: D) -> Result<Option<String>, D::Error> {
    struct Reason;
    impl<'de> serde::de::Visitor<'de> for Reason {
        type Value = Option<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a message")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut reason = None;
            while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
                if key == "stop_reason" {
                    reason = map.next_value::<serde_json::Value>()?.as_str().map(str::to_owned);
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(reason)
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            Ok(None)
        }
        fn visit_str<E: Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_bool<E: Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_i64<E: Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_u64<E: Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_f64<E: Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
    }
    message.deserialize_any(Reason)
}

/// The first primary user message must be this session's brief pointer.
/// Tool results, queued input, replies and later mentions cannot associate
/// another session's session. The pointer match is shared with Codex.
fn names_brief(text: &str, brief: &str) -> bool {
    for value in json_lines(text) {
        if value["isSidechain"] == true {
            continue;
        }
        match value["type"].as_str() {
            Some("user") => return session::brief_match(&value, brief).unwrap_or(false),
            Some("assistant") => return false,
            _ => {}
        }
    }
    false
}

fn note(cwd: &str, entry: &Entry, named: bool) -> Option<Note> {
    if !named && entry.cwd().is_some_and(|recorded| !session::same_cwd(recorded, cwd)) {
        return None;
    }
    let child = entry.sidechain();
    let at = session::parse_iso(entry.timestamp.as_ref()?.as_str()?)?;
    match entry.kind.as_ref()?.as_str()? {
        // A compaction's summary is written as the user's, but nobody asked
        // anything: compacted while idle, the session stays at its prompt.
        "user" if entry.compact == Some(serde_json::Value::Bool(true)) => None,
        "user" => Some(Note { marker: Marker::Start, at, child }),
        "assistant" if entry.message.as_deref() == Some("end_turn") => Some(Note { marker: Marker::End, at, child }),
        _ => None,
    }
}

/// The session a stand-in writes for this turn.
fn stand_in(cwd: &str, open: bool, at: u64) -> String {
    let iso = session::format_iso(at);
    let user = session::object(vec![
        ("type", "user".into()),
        ("timestamp", iso.clone().into()),
        ("cwd", cwd.into()),
        ("isSidechain", false.into()),
        ("message", session::object(vec![("role", "user".into()), ("content", "brief".into())])),
    ]);
    let mut body = format!("{user}\n");
    if !open {
        let end = session::object(vec![
            ("type", "assistant".into()),
            ("timestamp", iso.into()),
            ("cwd", cwd.into()),
            ("isSidechain", false.into()),
            ("message", session::object(vec![("role", "assistant".into()), ("stop_reason", "end_turn".into())])),
        ]);
        body.push_str(&format!("{end}\n"));
    }
    body
}

/// Claude Code, as a harness: what its module says of it.
pub(super) struct ClaudeCode;

impl Kind for ClaudeCode {
    const KEY: &'static str = KEY;
    const LOGIN: Login = Login::Token(TOKEN);
    const USER_HOME: Option<&'static str> = Some(HOME);
    const KEPT_LINKS: &'static [Link] = KEPT_LINKS;
    const STOCK_ARGUMENTS: &'static str = STOCK_ARGUMENTS;
    const ASKS_FIRST: Option<&'static str> = None;
    const ENVIRONMENT: &'static [&'static str] = &ENVIRONMENT;
    const REFUSED: u8 = REFUSED;

    fn kept(_start: &Start) -> Result<Vec<(&'static str, String)>, String> {
        Ok(Vec::new())
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

    fn briefed(line: String, brief: &str) -> (String, Option<String>) {
        (brief_at_start(&line, brief), None)
    }

    fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
        call(described)
    }

    fn stopped(_screen: &[String]) -> Option<Stop> {
        None
    }

    fn layout(_cwd: &str) -> Layout {
        layout()
    }

    fn could_record(cwd: &str, text: &str, brief: &str) -> bool {
        session::names_cwd_or_brief(cwd, text, brief)
    }

    fn recorded_turn(cwd: &str, text: &str, brief: &str) -> Option<Turn> {
        recorded_turn(cwd, text, None, Some(brief))
    }

    fn stand_in(cwd: &str, open: bool, at: u64) -> String {
        stand_in(cwd, open, at)
    }
}
