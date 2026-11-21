//! Claude Code, as a harness. It loads a directory's rules, skills and hooks
//! from the directory it starts in, so a start adds only the rest: the
//! directories beside it, the settings it may load, and the hooks that ask
//! the guard and report its status. The host user's skills and rules are one
//! more directory, kept by the host and holding nothing else of the user's,
//! so leaving their settings out still leaves out their hooks.

use serde::{Deserialize, Deserializer};

use super::session::{self, json_lines};
use super::store::Layout;
use super::{Dialog, Kind, Link, Login, Stop};
use crate::ask::{Asked, Headless};
use crate::harness::{Call, Harness};
use crate::line::Line;
use crate::start::Start;
use crate::turn::{Marker, Note, Turn};

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
/// The setting that keeps Claude Code from asking, mid-session, to "Teach auto
/// mode about your environment?": it opens that dialog only while its
/// `auto-mode-setup` skill is not off (read in Claude Code 2.1.326). Written
/// as a member of a JSON object.
macro_rules! auto_mode_setup_off {
    () => {
        "\"skillOverrides\":{\"auto-mode-setup\":\"off\"}"
    };
}
/// The dialogs Claude Code opens on its own once a turn ended, which eat a
/// line typed at it (seen 2026-11-18 on two workers, whose landing's red
/// never reached them): each dismissed by Escape.
const ASKS_MIDWAY: &[Dialog] =
    &[Dialog { asks: "Teach auto mode about your environment?", keys: "Esc to cancel", dismiss: "\x1b" }];
/// Turns off Claude Code's prompt suggestions: the grey ghost text it puts in
/// the prompt box once a turn ends. A session, an interactive session and the asking session
/// have nobody at that prompt, and whoever reads their screen could take the
/// suggestion for a person's word. The variable wins
/// over every settings file, and is set in the launch, so the host user's own
/// settings are left as they are.
const NO_SUGGESTIONS: &str = "CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=false";

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

/// `text` as a JSON string, quotes and all.
fn json_text(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// The settings added to a session's: the guard, a hook Claude Code runs
/// before every tool call with the line `guard`, the report, a hook it
/// runs with the line `report` wherever the session's state changes, the
/// answer to the warning it would stop at, whatever command starts it, and
/// its auto-mode setup off, which would open a dialog mid-session. They
/// are added to the start directory's own hooks, which run beside them.
fn added_settings(guard: &str, report: &str) -> String {
    let guard = format!("{{\"type\":\"command\",\"command\":{}}}", json_text(guard));
    let report = format!("{{\"type\":\"command\",\"command\":{},\"timeout\":{REPORT_TIMEOUT}}}", json_text(report));
    let mut settings = format!("{{\"hooks\":{{\"{BEFORE_A_CALL}\":[{{\"hooks\":[{guard},{report}]}}]");
    for event in STATE_EVENTS {
        settings.push_str(&format!(",\"{event}\":[{{\"hooks\":[{report}]}}]"));
    }
    settings.push_str(concat!("},", warning_accepted!(), ",", auto_mode_setup_off!()));
    settings.push('}');
    settings
}

/// The turn for `cwd`, or for the session its initial brief pointer names
/// when Claude Code starts in a start directory. A sidechain is a child session.
fn recorded_turn(cwd: &str, text: &str, brief: &str) -> Option<Turn> {
    let named = names_brief(text, brief);
    // Each line is read once, into only what a turn needs: a long session
    // is tens of megabytes of tool output nobody looks at here.
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
    crate::turn::latest(&notes)
}

/// What a turn reads of one session line. Every other field is skipped
/// unparsed; a field of another shape reads as absent, as a JSON value's
/// lookup would, but a `message` that is no object leaves the line unread.
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
    #[serde(default)]
    message: Option<Message>,
}

/// What a turn reads of a line's message.
#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    stop_reason: Option<serde_json::Value>,
}

impl Entry {
    fn sidechain(&self) -> bool {
        self.sidechain == Some(serde_json::Value::Bool(true))
    }

    fn cwd(&self) -> Option<&str> {
        self.cwd.as_ref()?.as_str()
    }
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
        "assistant" if entry.message.as_ref()?.stop_reason.as_ref()?.as_str() == Some("end_turn") => {
            Some(Note { marker: Marker::End, at, child })
        }
        _ => None,
    }
}

/// One session line, read only as far as a typed prompt needs: a prompt's
/// `message`, or the `attachment` text typed during a turn is taken as.
#[derive(Deserialize)]
struct Typed {
    #[serde(rename = "type")]
    kind: String,
    timestamp: String,
    message: Option<Said>,
    attachment: Option<Queued>,
}

impl Typed {
    /// The text this line says the session took from its user: a prompt of
    /// its own (a tool's result is a `user` line too, but its content is no
    /// string), or text queued into the running turn and read by it.
    fn took(&self) -> Option<&str> {
        match self.kind.as_str() {
            "user" => self.message.as_ref()?.content.as_str(),
            "attachment" => self.attachment.as_ref().filter(|a| a.kind == "queued_command")?.prompt.as_deref(),
            _ => None,
        }
    }
}

/// An attachment to a turn; a `queued_command` holds the text typed during it
/// that the turn read (Claude Code 2.1.330).
#[derive(Deserialize)]
struct Queued {
    #[serde(rename = "type")]
    kind: String,
    prompt: Option<String>,
}

/// Claude Code asked one question headless (`claude -p`, read in Claude Code
/// 2.1.329): no tools, no settings of any source (and so no CLAUDE.md
/// either), no MCP servers, its answer one JSON object.
const HEADLESS: Headless = Headless {
    flags: "-p --tools '' --setting-sources '' --strict-mcp-config --output-format json",
    model: MODEL,
    files: crate::ask::no_files,
    read: asked,
    stand_in: asked_stand_in,
};

/// What `claude -p --output-format json` printed: its answer in `result`,
/// unless `is_error`; the tokens of each model it used in `modelUsage`.
fn asked(out: &str, _model: Option<&str>) -> Result<Asked, String> {
    let said: serde_json::Value =
        serde_json::from_str(out.trim()).map_err(|e| format!("claude-code answered no JSON ({e}): {out}"))?;
    let text = said["result"].as_str().unwrap_or_default().to_string();
    if said["is_error"].as_bool() != Some(false) {
        return Err(format!("claude-code answered an error: {}", if text.is_empty() { out.trim() } else { &text }));
    }
    let session = said["session_id"].as_str().filter(|s| !s.is_empty()).ok_or("claude-code named no session_id")?;
    let models = said["modelUsage"].as_object().cloned().unwrap_or_default();
    let sum = |field: &str| models.values().map(|m| m[field].as_u64().unwrap_or(0)).sum::<u64>();
    let tokens_in = sum("inputTokens") + sum("cacheReadInputTokens") + sum("cacheCreationInputTokens");
    let model = models.keys().cloned().collect::<Vec<_>>().join("+");
    Ok(Asked { session: session.into(), model, text, tokens_in, tokens_out: sum("outputTokens") })
}

fn asked_stand_in(a: &Asked) -> String {
    let usage = serde_json::json!({ "inputTokens": a.tokens_in, "outputTokens": a.tokens_out });
    let said = serde_json::json!({ "type": "result", "is_error": false, "session_id": a.session, "result": a.text, "modelUsage": { a.model.clone(): usage } });
    said.to_string()
}

#[derive(Deserialize)]
struct Said {
    content: serde_json::Value,
}

/// Claude Code, as a harness: what its module says of it.
pub(super) struct ClaudeCode;

impl Kind for ClaudeCode {
    const KEY: &'static str = "claude-code";
    const LOGIN: Login = Login::Token(TOKEN);
    /// The harness directory under a user's home.
    const USER_HOME: Option<&'static str> = Some(".claude");
    const KEPT_LINKS: &'static [Link] = KEPT_LINKS;
    /// No permission prompts.
    const STOCK_ARGUMENTS: &'static str = "--dangerously-skip-permissions";
    const ASKS_FIRST: Option<&'static str> = None;
    const SHOWS_UP: Option<&'static str> = None;
    const ASKS_MIDWAY: &'static [Dialog] = ASKS_MIDWAY;
    const ENVIRONMENT: &'static [&'static str] = &ENVIRONMENT;
    /// The exit code that tells Claude Code a hook refuses the call.
    const REFUSED: u8 = 2;
    /// Escape. Text queued behind the turn is then submitted as the next turn.
    const INTERRUPT: &'static str = "\x1b";
    const HEADLESS: Option<Headless> = Some(HEADLESS);

    fn kept(_start: &Start) -> Result<Vec<(&'static str, String)>, String> {
        Ok(Vec::new())
    }

    const TOLD_IN: Option<&'static str> = None;

    fn guards(_home: &std::path::Path) -> Option<Vec<String>> {
        None
    }

    fn start_dir<'a>(start: &Start<'a>) -> &'a str {
        start.dir
    }

    /// Beyond the switches `ENVIRONMENT` names: its prompt suggestions off.
    /// Its home is the host user's own (`Harness::home`), so nothing points
    /// it elsewhere.
    fn environment(line: Line, _start: &Start) -> Line {
        line.arg(NO_SUGGESTIONS)
    }

    /// What Claude Code is given to put a session in its start directory:
    /// the worktree and the added directory's copy added; the user's
    /// settings left out, the guard and the report added, and the caller's
    /// model.
    fn added(line: Line, start: &Start) -> Line {
        let settings = added_settings(&start.hook(Harness::ClaudeCode), REPORT);
        let line = line.flag(ADD_DIR, start.worktree).flag(ADD_DIR, start.dir);
        line.arg(SETTING_SOURCES).arg(SOURCES).flag(SETTINGS, &settings).flag_if(MODEL, start.model)
    }

    /// The brief pointer as the session's first prompt. `--` ends its flags
    /// first: `--add-dir` takes every word after it, so a prompt after it
    /// alone would be read as a directory.
    fn briefed(line: String, brief: &str) -> (String, Option<String>) {
        (Line::new().launch(&line).fixed(" -- ").word(&crate::brief::brief_pointer(brief)).done(), None)
    }

    fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
        super::tool_call(described, "Bash")
    }

    fn stopped(_screen: &[String]) -> Option<Stop> {
        None
    }

    /// Sessions are filed under the directory Claude Code starts in, which
    /// is the start directory for a bound session. The session identifies
    /// its worktree.
    fn layout(_cwd: &str) -> Layout {
        Layout::Tree { dir: "projects".into(), extension: "jsonl" }
    }

    fn could_record(cwd: &str, text: &str, brief: &str) -> bool {
        session::names_cwd_or_brief(cwd, text, brief)
    }

    fn recorded_turn(cwd: &str, text: &str, brief: &str) -> Option<Turn> {
        recorded_turn(cwd, text, brief)
    }

    /// The session a stand-in writes for this turn.
    fn stand_in(cwd: &str, brief: &str, open: bool, at: u64) -> String {
        let iso = session::format_iso(at);
        let user = session::object(vec![
            ("type", "user".into()),
            ("timestamp", iso.clone().into()),
            ("cwd", cwd.into()),
            ("isSidechain", false.into()),
            (
                "message",
                session::object(vec![("role", "user".into()), ("content", crate::brief::brief_pointer(brief).into())]),
            ),
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

    /// Whether the session file `text` took `said` from its user, at or after
    /// `since`. Text typed while a turn runs is written first as a
    /// `queue-operation`, held, and is taken once a turn reads it: the running
    /// turn, as a `queued_command` attachment of it, or the next, as its
    /// `user` prompt.
    fn prompted(text: &str, said: &str, since: u64) -> Option<bool> {
        Some(text.lines().filter_map(|line| serde_json::from_str::<Typed>(line).ok()).any(|t| {
            t.took().map(str::trim) == Some(said.trim())
                && session::parse_iso(&t.timestamp).is_some_and(|at| at >= since)
        }))
    }
}
