//! Codex, as a harness. It is started as an interactive session, never as
//! one run of a prompt. It cannot be told to leave the host user's
//! configuration out, so it gets a private user directory per session, and
//! everything it is told — hooks, trust, rules from outside its start
//! directory — goes on its start line. It finds skills in directories of its
//! own; it also always loads the host user's home skills, and of two skills
//! of one name it shows both, so a shadowed host skill is turned off.

use serde::{Deserialize, Deserializer};
use serde_json::json;

use super::{Dialog, Kind, Link, Login, Stop};
use super::session::{self, json_lines};
use super::store::Layout;
use crate::harness::Call;
use crate::line::Line;
use crate::start::RULES;
use crate::start::Start;
use crate::turn::{Marker, Note, Turn};

/// The screen with its whitespace collapsed, so wrapped words still match.
fn joined(output: &[String]) -> String {
    output.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Codex's task-refusal screen, seen 2026-10-26.
fn task_refusal(output: &[String]) -> Option<String> {
    let words = "We take extra care with some cybersecurity requests. If you're doing authorized security work, apply for Daybreak to get broader access.";
    joined(output).contains(words).then(|| words.to_string())
}

/// Codex's capacity words, seen 2026-10-26.
fn capacity_stop(output: &[String]) -> Option<String> {
    let words = "Selected model is at capacity. Please try a different model.";
    joined(output).contains(words).then(|| words.to_string())
}

/// Codex's usage-limit screen, an account quota: both clauses, in any case,
/// possibly wrapped. A screen that only says a usage limit was reached is
/// not this one.
fn account_quota(output: &[String]) -> Option<String> {
    let lower = joined(output).to_lowercase();
    if !lower.contains("usage limit reached") || !lower.contains("increase your limits to continue using codex") {
        return None;
    }
    Some(
        output
            .iter()
            .find(|line| line.to_lowercase().contains("usage limit reached"))
            .cloned()
            .unwrap_or_else(|| output.join("\n")),
    )
}

const KEY: &str = "codex";
/// What a bare program is started with: no approval prompts, no sandbox.
const STOCK_ARGUMENTS: &str = "--dangerously-bypass-approvals-and-sandbox";

const ADD_DIR: &str = "--add-dir";
/// One configuration key, `key=value` with the value in TOML.
const CONFIG: &str = "--config";
/// Answers, for one run, the review Codex asks of every unreviewed hook;
/// asked at the terminal it would take the brief for its answer.
const HOOKS_REVIEWED: &str = "--dangerously-bypass-hook-trust";
const MODEL: &str = "--model";

const USER_DIR: &str = "CODEX_HOME";

/// Codex's kept user directory links the host user's login and nothing else
/// (so no hook, configuration or custom provider of theirs), and their
/// skills and rules for a session given them. A link, so a renewed login
/// stays fresh in both places.
const KEPT_LINKS: &[Link] = &[Link { home: ".codex/auth.json", kept: "auth.json" }];

/// The key that says, per directory, whether Codex loads what it holds of
/// Codex's own; untold, Codex asks at its terminal. Given as one table: a
/// dotted key is cut at every dot, and paths hold dots.
const DIRECTORIES: &str = "projects";
const HOOKS: &str = "hooks";
const BEFORE_A_CALL: &str = "PreToolUse";
/// The events at which a session's state changes, besides `BEFORE_A_CALL`:
/// each runs `REPORT`.
const STATE_EVENTS: [&str; 7] =
    ["SessionStart", "UserPromptSubmit", "PermissionRequest", "PostToolUse", "SubagentStart", "SubagentStop", "Stop"];
/// Seconds Codex waits on one report.
const REPORT_TIMEOUT: u64 = 10;
/// Nothing one session learned is carried to the next on this host.
const NO_MEMORY: &str = "features.memories=false";
/// Leaves out the skills Codex carries itself.
const NO_CARRIED_SKILLS: &str = "skills.bundled.enabled=false";

/// The key naming the fallback rules files: the one every harness loads.
const RULES_FILES: &str = "project_doc_fallback_filenames";
/// No root markers: Codex loads only the start directory's rules, not those
/// of every directory above it.
const NO_ROOT: &str = "project_root_markers=[]";
/// Text given to the model as a developer's: how Codex is told rules from
/// outside its start directory.
const TOLD: &str = "developer_instructions";
const TOLD_OPENS: &str = "factorium started you, and gives you the rules below. You work under \
     them as you do under the rules of the directory you started in.";
const REPO_S_OWN: &str = "The repo's own rules";

/// Wraps the guard line. Codex refuses a call only when a hook exits
/// `REFUSED` having said why, and makes it on any other failure, so a
/// refusal, a missing guard and a silent failure all end the same way here.
const GUARD_ASKED: (&str, &str) = (
    "said=$(",
    " 2>&1 >/dev/null) && exit 0; \
     printf '%s\\n' \"${said:-factorium guard: it could not be asked}\" >&2; exit 2",
);

/// Tells orca of the session's state, through the hook orca installs for
/// Codex, where it did. It prints nothing and succeeds: Codex takes a hook's
/// output for an answer, and only the guard answers.
const REPORT: &str = "hook=\"${HOME-}/.orca/agent-hooks/codex-hook.sh\"; if [ -x \"$hook\" ]; \
     then /bin/sh \"$hook\"; else cat; fi >/dev/null 2>&1; exit 0";

/// The shell tool. `apply_patch` is handed a `command` too, and is another.
const SHELL_TOOL: &str = "Bash";
const REFUSED: u8 = 2;

/// `text` as a TOML string, quotes and all.
fn toml_text(text: &str) -> String {
    let mut toml = String::from("\"");
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                toml.push('\\');
                toml.push(c);
            }
            c if c.is_control() => toml.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => toml.push(c),
        }
    }
    toml.push('"');
    toml
}

/// The key that has Codex load what `dir` holds of its own.
fn trusted(dir: &str) -> String {
    format!("{DIRECTORIES}={{{}={{trust_level=\"trusted\"}}}}", toml_text(dir))
}

/// The guard before every tool call, and `report` at every state change, as
/// one configuration key. Codex adds them to the start directory's own hooks.
fn added_hooks(guard: &str, report: &str) -> String {
    let (before, after) = GUARD_ASKED;
    let guard = format!("{{type=\"command\",command={}}}", toml_text(&format!("{before}{guard}{after}")));
    let report = format!("{{type=\"command\",command={},timeout={REPORT_TIMEOUT}}}", toml_text(report));
    let mut hooks = format!("{HOOKS}={{{BEFORE_A_CALL}=[{{hooks=[{guard},{report}]}}]");
    for event in STATE_EVENTS {
        hooks.push_str(&format!(",{event}=[{{hooks=[{report}]}}]"));
    }
    hooks.push('}');
    hooks
}

fn rules_files() -> String {
    format!("{RULES_FILES}=[{}]", toml_text(RULES))
}

/// The repo's own rules, read from `file`, as one configuration key.
fn told(file: &str, text: &str) -> String {
    let repo = format!("\n\n# {REPO_S_OWN}: {file}\n\n<rules>\n{}\n</rules>", text.trim_end());
    format!("{TOLD}={}", toml_text(&format!("{TOLD_OPENS}{repo}")))
}

fn environment(line: Line, start: &Start) -> Line {
    line.fixed(" ").fixed(USER_DIR).fixed("=").word(start.home.dir())
}

/// Sets a session's goal and starts its first turn (codex-cli 0.166.0).
/// While it is active Codex begins another turn each time one ends short.
const GOAL: &str = "/goal ";

/// The line that hands a session `brief`: set as its goal.
fn brief_line(brief: &str) -> String {
    format!("{GOAL}{}", crate::brief::goal_objective(brief))
}

/// Answers both questions Codex asks before loading a directory's own
/// configuration.
fn trust(line: Line, dir: &str) -> Line {
    line.fixed(" ").fixed(HOOKS_REVIEWED).fixed(" ").fixed(CONFIG).fixed(" ").word(&trusted(dir))
}

/// No memories carried between sessions, no bundled skills.
fn isolated(line: Line) -> Line {
    line.fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .fixed(NO_MEMORY)
        .fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .fixed(NO_CARRIED_SKILLS)
}

/// `line` with the worktree added, hooks reviewed and the start directory
/// trusted, its rules and none above them, the told rules, the guard and
/// the report, isolation, shadowed skills off, and the model.
fn added(line: Line, start: &Start) -> Line {
    let line = line.fixed(" ").fixed(ADD_DIR).fixed(" ").word(start.worktree);
    let line = trust(line, start.dir)
        .fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .word(&rules_files())
        .fixed(" ")
        .fixed(CONFIG)
        .fixed(" ")
        .word(NO_ROOT);
    let repo = std::path::Path::new(start.worktree).join(RULES);
    let line = match std::fs::read_to_string(&repo).ok().map(|text| told(&repo.to_string_lossy(), &text)) {
        Some(told) => line.fixed(" ").fixed(CONFIG).fixed(" ").word(&told),
        None => line,
    };
    let line = isolated(line.fixed(" ").fixed(CONFIG).fixed(" ").word(&added_hooks(start.hook, REPORT)));
    match start.model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    }
}

/// A session file is under `SESSIONS`, named to end with its thread id and
/// `SESSION_ENDS`.
const SESSIONS: &str = "sessions";

/// One tool call, as Codex describes it to a hook before making it.
#[derive(Deserialize)]
struct Described {
    tool_name: String,
    #[serde(default)]
    tool_input: Input,
}

#[derive(Default, Deserialize)]
struct Input {
    /// The shell line, or of `apply_patch` the patch.
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

/// Every JSONL file under [`SESSIONS`]; each file says its worktree.
fn layout() -> Layout {
    Layout::Tree { dir: SESSIONS.into(), extension: "jsonl" }
}

/// The turn `text` records for `cwd`, or for the session handed `brief`
/// when it started elsewhere. A header is required.
fn recorded_turn(cwd: &str, text: &str, brief: Option<&str>) -> Option<Turn> {
    let mut saw_header = false;
    let mut notes = Vec::new();
    for value in json_lines(text) {
        if value["type"] == "session_meta" {
            let recorded = value["payload"]["cwd"].as_str().unwrap_or("");
            if !session::same_cwd(recorded, cwd) && !brief.is_some_and(|brief| names_brief(text, brief)) {
                return None;
            }
            saw_header = true;
        }
        if let Some(note) = note(&value) {
            notes.push(note);
        }
    }
    if !saw_header {
        return None;
    }
    session::turn_of(&notes)
}

/// Whether the first brief pointer in the opening user messages (after any
/// AGENTS.md and environment messages) is `brief`'s. A later mention, tool
/// output or reply does not count.
fn names_brief(text: &str, brief: &str) -> bool {
    for value in json_lines(text) {
        let payload = &value["payload"];
        if value["type"] == "response_item"
            && payload["role"] != "developer"
            && payload["role"] != "system"
            && (payload["role"] == "assistant" || payload["type"] != "message")
        {
            return false;
        }
        // A brief handed as the goal is its objective, exactly.
        if value["type"] == "event_msg" && payload["type"] == "thread_goal_updated" {
            let objective = payload["goal"]["objective"].as_str();
            return objective == Some(crate::brief::goal_objective(brief).as_str());
        }
        if let Some(matches) = session::brief_match(&value, brief) {
            return matches;
        }
    }
    false
}

fn note(value: &serde_json::Value) -> Option<Note> {
    let payload = &value["payload"];
    match payload["type"].as_str()? {
        "task_started" => {
            let at = session::number_at(payload["started_at"].as_u64(), value["timestamp"].as_str())?;
            Some(Note { marker: Marker::Start, at, child: false })
        }
        "task_complete" => {
            let at = session::number_at(payload["completed_at"].as_u64(), value["timestamp"].as_str())?;
            Some(Note { marker: Marker::End, at, child: false })
        }
        "turn_aborted" => {
            let at = session::parse_iso(value["timestamp"].as_str()?)?;
            Some(Note { marker: Marker::End, at, child: false })
        }
        _ => None,
    }
}

/// The session a stand-in writes for this turn.
fn stand_in(cwd: &str, brief: &str, open: bool, at: u64) -> String {
    let iso = session::format_iso(at);
    let meta = json!({"cwd": cwd, "id": "factorium"});
    let mut body = format!("{}\n", json!({"timestamp": iso, "type": "session_meta", "payload": meta}));
    let pointer = json!({"type": "input_text", "text": crate::brief::brief_pointer(brief)});
    let said = json!({"type": "message", "role": "user", "content": [pointer]});
    body += &format!("{}\n", json!({"timestamp": iso, "type": "response_item", "payload": said}));
    let started = json!({"type": "task_started", "started_at": at, "turn_id": "factorium"});
    body += &format!("{}\n", json!({"timestamp": iso, "type": "event_msg", "payload": started}));
    if !open {
        let done = json!({"type": "task_complete", "completed_at": at, "turn_id": "factorium"});
        body += &format!("{}\n", json!({"timestamp": iso, "type": "event_msg", "payload": done}));
    }
    body
}

/// Codex, as a harness: what its module says of it.
pub(super) struct Codex;

impl Kind for Codex {
    const KEY: &'static str = KEY;
    const LOGIN: Login = Login::Machine { user_dir: USER_DIR };
    const USER_HOME: Option<&'static str> = None;
    const KEPT_LINKS: &'static [Link] = KEPT_LINKS;
    const STOCK_ARGUMENTS: &'static str = STOCK_ARGUMENTS;
    const ASKS_FIRST: Option<&'static str> = None;
    const ASKS_MIDWAY: &'static [Dialog] = &[];
    const ENVIRONMENT: &'static [&'static str] = &[];
    const REFUSED: u8 = REFUSED;
    /// Escape.
    const INTERRUPT: &'static str = "\x1b";

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
        (line, Some(brief_line(brief)))
    }

    fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
        call(described)
    }

    fn stopped(screen: &[String]) -> Option<Stop> {
        account_quota(screen).map(Stop::Quota).or_else(|| task_refusal(screen).or_else(|| capacity_stop(screen)).map(Stop::Refused))
    }

    fn layout(_cwd: &str) -> Layout {
        layout()
    }

    fn could_record(cwd: &str, text: &str, brief: &str) -> bool {
        session::names_cwd_or_brief(cwd, text, brief)
    }

    fn recorded_turn(cwd: &str, text: &str, brief: &str) -> Option<Turn> {
        recorded_turn(cwd, text, Some(brief))
    }

    fn stand_in(cwd: &str, brief: &str, open: bool, at: u64) -> String {
        stand_in(cwd, brief, open, at)
    }

    fn prompted(_text: &str, _said: &str, _since: u64) -> Option<bool> {
        None
    }
}
