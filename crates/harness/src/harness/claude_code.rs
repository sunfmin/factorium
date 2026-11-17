//! Claude Code, as a harness. It loads a directory's rules, skills and hooks
//! from the directory it starts in, so a start adds only the rest: the
//! directories beside it, the settings it may load, and the hooks that ask
//! the guard and report its status. The host user's skills and rules are one
//! more directory, kept by the host and holding nothing else of the user's,
//! so leaving their settings out still leaves out their hooks.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::{Error, IgnoredAny};
use serde::{Deserialize, Deserializer};

use super::session::{self, json_lines};
use super::{Held, Kept, Link, SessionFile, SessionLayout};
use crate::harness::Call;
use crate::line::Line;
use crate::start::Start;
use crate::status::{self, Blocked, CallEnd, Reading, Report, Status};
use crate::steps::{self, Said};
use crate::tokens::{Counts, Keys, Usage};
use crate::turn::{Marker, Note, Turn};
use crate::window::{self, Limit, Limited, StatusLine, Window};

pub(super) const NAME: &str = "Claude Code";
pub(super) const KEY: &str = "claude-code";
pub(super) const PROGRAM: &str = "claude";
/// The line that installs it: its native installer, which keeps it up to date.
pub(super) const INSTALL: &str = "curl -fsSL https://claude.ai/install.sh | bash";
/// How it is asked whether it is logged in, and logged in.
pub(super) const LOGIN: Option<super::Login> =
    Some(super::Login { question: "claude auth status", logged_out: "is not logged in", line: "claude auth login" });

/// The variable that points Claude Code at a user configuration directory
/// other than `~/.claude`: its login, settings, skills and rules, and the
/// `.claude.json` it keeps in the home itself while the variable is unset.
pub(super) const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
/// What a fresh user configuration directory of Claude Code is given so that
/// it asks no first-run question: without it, it opens with its theme
/// onboarding, which nobody at its terminal answers. Merged into the file,
/// whose login and every other member are kept.
pub(super) const FRESH: [super::Fresh; 1] =
    [super::Fresh { file: ".claude.json", given: super::Given::Members("{\"hasCompletedOnboarding\":true}") }];

/// Whether the answer to its login question says it is logged in: by the
/// `loggedIn` field of the JSON it prints — whatever else the shell printed
/// around it — not by the exit status.
pub(super) fn logged_in(_succeeded: bool, printed: &str) -> bool {
    let dense: String = printed.split_whitespace().collect();
    dense.contains("\"loggedIn\":true")
}
/// Where it reads its user's global instructions, under the user's home.
pub(super) const INSTRUCTIONS: &str = ".claude/CLAUDE.md";
/// The agent `npx skills` installs its user's skills for.
pub(super) const SKILLS_AGENT: Option<&str> = Some("claude-code");
/// The setting that answers the warning Claude Code opens with when it is
/// started to work with nobody at its terminal: a session has nobody to accept
/// it, and stops there. On a host whose user accepted it once, their settings
/// say the same. Written as a member of a JSON object.
macro_rules! warning_accepted {
    () => {
        "\"skipDangerousModePermissionPrompt\":true"
    };
}
/// Where Claude Code reads its user's hooks: their settings, which a
/// session they start by hand, and a session outside a start directory, loads. Its tools
/// that edit a file are each named.
pub(super) const USER_HOOKS: super::UserHooks = super::UserHooks {
    settings: ".claude/settings.json",
    table: "hooks",
    event: "PreToolUse",
    shell: "Bash",
    edits: "Edit|Write|MultiEdit|NotebookEdit",
};
/// Claude Code with the flag that lets it work with nobody at its terminal,
/// and the warning it opens with under that flag answered. Each session adds
/// native session trust as well. Of two `--settings` Claude Code takes
/// the last, whole: a session in its start directory is started on the tool's (`added`).
pub(super) const STOCK: &str =
    concat!("claude --dangerously-skip-permissions --settings '{", warning_accepted!(), "}'");
/// The variable Claude Code reads a custom provider's address from.
pub(super) const PROVIDER: &str = "ANTHROPIC_BASE_URL";
/// Claude Code's native session trust. Permission bypass and its warning
/// setting do not accept workspace trust, even when both settings are given.
/// This does not write a trust entry in the host user's config.
pub(super) const TRUST: &str = "CLAUDE_CODE_SANDBOXED=1";
/// Turns off Claude Code's prompt suggestions: the grey ghost text it puts in
/// the prompt box once a turn ends. A session, an interactive session and the asking session
/// have nobody at that prompt, and whoever reads their screen could take the
/// suggestion for a person's word. The variable wins
/// over every settings file, and is set in the launch, so the host user's own
/// settings are left as they are.
pub(super) const NO_SUGGESTIONS: &str = "CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=false";
/// Its worktree trust outside a start directory is the permission-bypass and settings
/// flags `in_worktree` adds: a launch checks the command takes `FLAGS`.
pub(super) const TRUST_IS_FLAGS: bool = true;
const BYPASS: &str = "--dangerously-skip-permissions";

/// Starts Claude Code with its worktree trusted, its unattended warning
/// answered and its prompt suggestions off, without replacing a setting the
/// stock command already carries.
pub(super) fn in_worktree(command: &str) -> String {
    let mut command = command.to_string();
    if !command.contains(BYPASS) {
        command.push_str(&format!(" {BYPASS}"));
    }
    if !command.contains("skipDangerousModePermissionPrompt") {
        command.push_str(&format!(" --settings '{{{}}}'", warning_accepted!()));
    }
    let missing: Vec<&str> = [TRUST, NO_SUGGESTIONS].into_iter().filter(|set| !command.contains(set)).collect();
    if missing.is_empty() {
        return command;
    }
    let mut line = Line::new().fixed("export");
    for set in missing {
        line = line.fixed(" ").fixed(set);
    }
    line.fixed(" && ").launch(&command).done()
}

/// The environment a session in its start directory sets for Claude Code beyond the
/// switches `ENVIRONMENT` names: its prompt suggestions off.
/// And, where the start says the directory kept for it is its user
/// configuration (`Start::kept_config`), its variable pointed there.
pub(super) fn environment(line: Line, start: &Start) -> Line {
    let line = line.fixed(" ").fixed(NO_SUGGESTIONS);
    if start.kept_config { line.fixed(" ").fixed(CONFIG_DIR).fixed("=").word(start.kept) } else { line }
}

/// Starts one interactive session interactively, with the same unattended trust setup.
pub(super) fn interactive(command: &str, model: Option<&str>) -> String {
    let line = Line::new().launch(&in_worktree(command));
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.done()
}

/// Adds the tool's hooks without changing project or user rule discovery.
pub(super) fn guarded(command: &str, guard: &str) -> String {
    Line::new().launch(command).fixed(" --settings ").word(&added_settings(guard, REPORT, &[])).done()
}

/// Claude Code's flag for a directory added to the one it was started in.
const ADD_DIR: &str = "--add-dir";
/// Claude Code's flag for which settings it loads.
const SETTING_SOURCES: &str = "--setting-sources";
/// Claude Code's flag for the model of the session.
pub(super) const MODEL: &str = "--model";
/// Claude Code's flag for settings added to those it loads.
const SETTINGS: &str = "--settings";
/// Every flag the tool adds: what a launch command that starts
/// Claude Code must take.
pub(super) const FLAGS: [&str; 4] = [ADD_DIR, SETTING_SOURCES, SETTINGS, MODEL];
/// What Claude Code is asked for the flags it takes.
const HELP: &str = " --help";

/// The line that sends `model` the one-line `prompt` through `command`, a
/// launch command of Claude Code's, and prints its answer. Claude
/// Code keeps its logins with the host's user, as a session finds them.
pub(super) fn prompt_question(command: &str, model: Option<&str>, prompt: &str) -> String {
    let line = Line::new().launch(command);
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(" --print ").word(prompt).done()
}

/// The control request Claude Code answers with, among the rest of what it
/// offers, the models its login runs: sent before any prompt, it is the only
/// thing sent, and no prompt follows.
const INITIALIZE: &str = r#"{"type":"control_request","request_id":"models","request":{"subtype":"initialize"}}"#;

/// How Claude Code is started to read control requests from its input and
/// answer them on its output, a JSON object a line.
const STREAMED: &str = " -p --input-format stream-json --output-format stream-json --verbose";

/// What comes before the list of models in Claude Code's answer to
/// `INITIALIZE`, with no whitespace: a `"` inside a JSON string is written
/// `\"`, so this is in the answer only where the key is.
const MODELS: &str = "\"models\":[";

/// What comes before each model's name in that list: the key of it.
const NAMED: &str = "\"value\":\"";

/// The line that asks `command`, a launch command of Claude Code's,
/// which models its login runs: `INITIALIZE`, on its input, and nothing else.
/// Claude Code keeps its logins with the host's user, as a session finds them.
pub(super) fn models_question(command: &str) -> String {
    Line::new().fixed("printf '%s\\n' ").word(INITIALIZE).fixed(" | ").launch(command).fixed(STREAMED).done()
}

/// The models Claude Code lists in `answer`, what it printed for
/// `models_question`, each by the name its `MODEL` flag takes, in the order
/// listed: the `value` of each model there, whatever a shell printed around
/// it or wherever a terminal broke its lines. `None` when it lists none: it
/// said something else.
pub(super) fn models(answer: &str) -> Option<Vec<String>> {
    let dense: String = answer.split_whitespace().collect();
    let (_, list) = dense.split_once(MODELS)?;
    // The models are the objects one level into the list, which ends at the
    // `]` that closes it; a `[`, `]`, `{` or `}` inside a string is none.
    let mut listed = vec![];
    let (mut depth, mut quoted, mut escaped) = (1_usize, false, false);
    for (at, character) in list.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => {
                if !quoted && depth == 2 {
                    let named = list[at..].strip_prefix(NAMED).and_then(|after| after.split_once('"'));
                    listed.extend(named.map(|(name, _)| name.to_string()));
                }
                quoted = !quoted;
            }
            _ if quoted => {}
            '[' | '{' => depth = depth.saturating_add(1),
            ']' | '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    listed.retain(|name| !name.is_empty());
    if listed.is_empty() { None } else { Some(listed) }
}

/// What Claude Code's probe adds: its answer as one JSON object, whose
/// `modelUsage` is keyed by the model that answered.
const AS_JSON: &str = " --output-format json";

/// What comes before the first model's id in that answer, with no
/// whitespace.
const USAGE: &str = "\"modelUsage\":{\"";

/// The line that probes `model` with `prompt` through `command`, as
/// `prompt_question` sends it, answering in JSON (`answered_as`).
pub(super) fn probe_question(command: &str, model: Option<&str>, prompt: &str) -> String {
    let line = Line::new().launch(command);
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(" --print ").word(prompt).fixed(AS_JSON).done()
}

/// The model that answered `probe_question`, by `answer`: the first key of
/// its `modelUsage`, whatever a terminal broke its lines at.
pub(super) fn answered_as(answer: &str) -> Option<String> {
    let dense: String = answer.split_whitespace().collect();
    let (_, after) = dense.split_once(USAGE)?;
    let (id, _) = after.split_once('"')?;
    (!id.is_empty()).then(|| id.to_string())
}

/// What comes before a model's display name in Claude Code's answer to
/// `INITIALIZE`.
const SHOWN: &str = "\"displayName\":\"";

/// The display name of each model Claude Code lists in `answer` (`models`),
/// by its name: what its menu shows it as. A terminal's line breaks are
/// taken out, and the spaces a display name holds kept.
pub(super) fn shown(answer: &str) -> Vec<(String, String)> {
    let joined: String = answer.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    let Some((_, list)) = joined.split_once("\"models\":") else { return vec![] };
    // Each model is an object one level into the list, whichever order its
    // keys are in; a bracket inside a string is none.
    let mut shown = vec![];
    let (mut depth, mut quoted, mut escaped, mut opened) = (0_usize, false, false, 0);
    for (at, character) in list.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            _ if quoted => {}
            '[' | '{' => {
                depth = depth.saturating_add(1);
                if depth == 2 {
                    opened = at;
                }
            }
            ']' | '}' => {
                if depth == 2 {
                    let model = &list[opened..at];
                    let value = |key: &str| {
                        let (_, after) = model.split_once(key)?;
                        after.split_once('"').map(|(value, _)| value.to_string())
                    };
                    if let (Some(name), Some(display)) = (value(NAMED), value(SHOWN))
                        && !name.is_empty()
                        && !display.is_empty()
                    {
                        shown.push((name, display));
                    }
                }
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    shown
}

/// The settings a session loads: those of the directory it starts in, and
/// nothing of the host user's: no hook, no MCP server, no permission.
const SOURCES: &str = "project";

/// Claude Code also walks `.claude` directories above its start directory as project
/// rules: `--setting-sources project` alone does not exclude the host user's
/// rules when the start directory is under their home.
pub(super) fn host_rule_excludes(home: &str) -> Vec<String> {
    let home = home.trim_end_matches('/');
    vec![format!("{home}/.claude/CLAUDE.md"), format!("{home}/.claude/rules/**")]
}
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
pub(super) const ENVIRONMENT: [&str; 3] = [ADDED_RULES, NO_MEMORY, "CLAUDE_CODE_SANDBOXED"];

/// What a host keeps for Claude Code: the host user's skills and rules, as
/// a directory added to a session's holds them, and nothing else. Claude Code
/// loads the skills and the rules of an added directory, and not its
/// settings; a skill of the start directory's wins over one of the same name there.
pub(super) const KEPT: Kept = Kept {
    links: &[],
    host_user: &[
        Link { home: ".claude/skills", kept: ".claude/skills" },
        Link { home: ".claude/CLAUDE.md", kept: ".claude/CLAUDE.md" },
    ],
    files: &[],
};

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
pub(super) const REFUSED: u8 = 2;

/// What a hook answers to stop the session before the call it is asked of,
/// keeping the session: the common fields of Claude Code's hook output
/// (https://code.claude.com/docs/en/hooks, "Common JSON fields") —
/// `continue: false` ends the session's turn, and `stopReason` is shown to
/// its user, not to the model. Claude Code reads them only from a hook that
/// exits 0.
pub(super) fn stop_answer(reason: &str) -> super::Answer {
    super::Answer { printed: session::stopped(reason), code: 0 }
}
/// Claude Code honours `continue: false` from any hook, a call's included.
pub(super) const STOP_RELIABLE: bool = true;

/// The flag that continues the session it names, and the one that continues
/// the latest session of the directory Claude Code starts in.
const RESUME: &str = "--resume";
const CONTINUE: &str = "--continue";

/// `line`, which starts a session in its start directory, continuing the session
/// `session` — the latest of its directory where it names none — with
/// `instruction` as the prompt.
pub(super) fn resumed(line: Line, session: Option<&str>, instruction: &str) -> Line {
    let line = match session {
        Some(session) => line.fixed(" ").fixed(RESUME).fixed(" ").word(session),
        None => line.fixed(" ").fixed(CONTINUE),
    };
    line.fixed(" ").word(instruction)
}

/// The session id the session file `text` is of: the `sessionId` each of
/// its entries carries, the file's name before `.jsonl`.
pub(super) fn session_id(text: &str) -> Option<String> {
    json_lines(text).find_map(|value| value["sessionId"].as_str().filter(|id| !id.is_empty()).map(str::to_string))
}

/// Where a start directory's limits for Claude Code are written, in its directory: the
/// settings Claude Code loads from the directory it starts in.
pub(super) const LIMITS: &str = ".claude/settings.json";

/// A start directory's settings, as far as they say whether they limit a session.
#[derive(Deserialize)]
struct Settings {
    /// Each event of Claude Code's, with the hooks it runs at it.
    #[serde(default)]
    hooks: BTreeMap<String, Vec<IgnoredAny>>,
    #[serde(default)]
    permissions: Permissions,
}

#[derive(Default, Deserialize)]
struct Permissions {
    /// What a session may never do, whatever it is started with.
    #[serde(default)]
    deny: Vec<IgnoredAny>,
}

/// Whether what a start directory holds at `LIMITS` limits a session: a file
/// of settings that names a hook at some event, or denies a permission. One
/// that names neither — a skeleton's — holds no limit.
pub(super) fn limited<'de, D: Deserializer<'de>>(held: Held<D>) -> Result<bool, D::Error> {
    match held {
        Held::Nothing => Ok(false),
        Held::Entries(_) => Err(D::Error::custom("it is a directory, not a file of settings")),
        Held::File(settings) => {
            let settings = Settings::deserialize(settings)?;
            let hooked = settings.hooks.values().any(|hooks| !hooks.is_empty());
            Ok(hooked || !settings.permissions.deny.is_empty())
        }
    }
}

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
/// `excluded` keeps host-user rule paths out of native ancestor discovery
/// when the start directory has turned host-user rules off.
fn added_settings(guard: &str, report: &str, excluded: &[String]) -> String {
    let guard = format!("{{\"type\":\"command\",\"command\":{}}}", json_text(guard));
    let report = format!("{{\"type\":\"command\",\"command\":{},\"timeout\":{REPORT_TIMEOUT}}}", json_text(report));
    let mut settings = format!("{{\"hooks\":{{\"{BEFORE_A_CALL}\":[{{\"hooks\":[{guard},{report}]}}]");
    for event in STATE_EVENTS {
        settings.push_str(&format!(",\"{event}\":[{{\"hooks\":[{report}]}}]"));
    }
    settings.push_str(concat!("},", warning_accepted!()));
    if !excluded.is_empty() {
        let paths = excluded.iter().map(|path| json_text(path)).collect::<Vec<_>>().join(",");
        settings.push_str(&format!(",\"claudeMdExcludes\":[{paths}]"));
    }
    settings.push('}');
    settings
}

/// `line`, which starts the launch command, with what Claude Code is
/// given to put a session in its start directory: the worktree and the added directory's copy added,
/// and the host user's skills and rules where it is given them; the user's
/// settings left out, the guard and the report added, and the caller's model.
pub(super) fn added(line: Line, start: &Start) -> Line {
    let excluded = if start.host_user { &[][..] } else { start.host_rules };
    let line = line
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.worktree)
        .fixed(" ")
        .fixed(ADD_DIR)
        .fixed(" ")
        .word(start.added);
    let line = if start.host_user { line.fixed(" ").fixed(ADD_DIR).fixed(" ").word(start.kept) } else { line };
    let line = line
        .fixed(" ")
        .fixed(SETTING_SOURCES)
        .fixed(" ")
        .fixed(SOURCES)
        .fixed(" ")
        .fixed(SETTINGS)
        .fixed(" ")
        .word(&added_settings(start.hook, REPORT, excluded));
    match start.model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    }
}

/// The start `line` with the brief pointer to `brief` as the session's first
/// prompt. `--` ends its flags first: `--add-dir` takes every word after it,
/// so a prompt after it alone would be read as a directory.
pub(super) fn brief_at_start(line: &str, brief: &str) -> String {
    Line::new().launch(line).fixed(" -- ").word(&crate::brief::brief_pointer(brief)).done()
}

/// `line`, which starts the launch command, asking it which flags it
/// takes instead of starting a session.
pub(super) fn asked_its_flags(line: Line) -> Line {
    line.fixed(HELP)
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
pub(super) fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.tool_input.command {
        Some(line) if described.tool_name == SHELL_TOOL => Call::Shell(line),
        _ => Call::Other,
    })
}

/// The line Claude Code waits for the next prompt at: its footer is below it.
const PROMPT: char = '❯';
/// What Claude Code's footer counts of the work a session runs in the
/// background, as `<n> <what>` between its `·`s: `1 shell, 1 monitor`.
const BACKGROUND: [&str; 6] = ["shell", "shells", "monitor", "monitors", "background task", "background tasks"];

/// Whether the end of a Claude Code session's screen shows work it started
/// itself still running: its footer — the lines below the last prompt, or
/// the last line where no prompt shows — counts a shell or a monitor. What
/// a turn's end said ran, above the prompt, is not read: that work may have
/// ended since.
pub(super) fn shows_background(screen: &[String]) -> bool {
    let mut below = screen.iter();
    match screen.iter().rposition(|line| line.trim_start().starts_with(PROMPT)) {
        Some(prompt) => {
            below.nth(prompt);
        }
        None => {
            let last = screen.iter().rposition(|line| !line.trim().is_empty());
            if let Some(last) = last.and_then(|last| last.checked_sub(1)) {
                below.nth(last);
            }
        }
    }
    below.any(|line| {
        line.split(['·', ',']).any(|part| match part.trim().split_once(' ') {
            Some((count, what)) => {
                !count.is_empty()
                    && count.bytes().all(|b| b.is_ascii_digit())
                    && count != "0"
                    && BACKGROUND.contains(&what.trim())
            }
            None => false,
        })
    })
}

/// How a tool result begins when Claude Code started the call's work in the
/// background, and what comes right before the ID that work's notification
/// names as its `<task-id>`: a background agent, then a background shell.
const LAUNCHED: [(&str, &str); 2] = [
    ("Async agent launched successfully.", "agentId: "),
    ("Command running in background with ID: ", "Command running in background with ID: "),
];
/// What a notification that background work ended names that work by.
const TASK_ID: (&str, &str) = ("<task-id>", "</task-id>");

/// Whether the session `text` records background work its own process runs
/// launched and not yet notified as ended: an agent (the Agent tool run in
/// the background) or a shell. A launch is a tool result [`LAUNCHED`] names;
/// its end is a `<task-notification>` naming its ID, recorded as a queued
/// prompt (a `queue-operation` enqueue or a `queued_command` attachment) or
/// as a user message. A sidechain's lines are a child session's, and a
/// request kept beside the session (`api-request-blob`) repeats a message:
/// neither counts.
pub(super) fn background_pending(text: &str) -> bool {
    let mut launched = BTreeSet::new();
    let mut notified = BTreeSet::new();
    for value in json_lines(text) {
        if value["isSidechain"] == true {
            continue;
        }
        let queued = match value["type"].as_str() {
            Some("queue-operation") if value["operation"] == "enqueue" => &value["content"],
            Some("attachment") if value["attachment"]["type"] == "queued_command" => &value["attachment"]["prompt"],
            Some("user") => &value["message"]["content"],
            _ => continue,
        };
        if let Some(prompt) = queued.as_str() {
            notified.extend(task_ids(prompt));
            continue;
        }
        let Some(parts) = queued.as_array() else { continue };
        for part in parts {
            match part["type"].as_str() {
                Some("tool_result") => launched.extend(launch(&part["content"])),
                Some("text") => notified.extend(part["text"].as_str().map(task_ids).into_iter().flatten()),
                _ => {}
            }
        }
    }
    launched.iter().any(|id| !notified.contains(id))
}

/// The ID of the background work a tool result's `content` says was launched.
fn launch(content: &serde_json::Value) -> Option<String> {
    let text = match content {
        serde_json::Value::String(text) => text.as_str(),
        serde_json::Value::Array(parts) => parts.first()?["text"].as_str()?,
        _ => return None,
    };
    let text = text.trim_start();
    let (_, before) = LAUNCHED.iter().find(|(begins, _)| text.starts_with(begins))?;
    let (_, rest) = text.split_once(before)?;
    let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    (!id.is_empty()).then_some(id)
}

/// The IDs each `<task-id>` in `prompt` names.
fn task_ids(prompt: &str) -> Vec<String> {
    prompt
        .split(TASK_ID.0)
        .skip(1)
        .filter_map(|rest| Some(rest.split_once(TASK_ID.1)?.0.trim().to_string()))
        .filter(|id| !id.is_empty())
        .collect()
}

/// The harness directory under a user's home.
pub(super) const HOME: &str = ".claude";

/// Sessions are filed under the directory Claude Code starts in, which is
/// the start directory for a bound session. The session identifies its worktree.
pub(super) fn session_layout(_cwd: &str) -> SessionLayout {
    SessionLayout::Tree { dir: "projects".into(), extension: "jsonl" }
}

/// Notes typed at the prompt of a session started in `cwd` stay in its project directory.
pub(super) fn note_layout(cwd: &str) -> SessionLayout {
    SessionLayout::Files { dir: format!("projects/{}", dir_name(cwd)), extension: "jsonl" }
}

/// The directory Claude Code files `cwd`'s sessions under: every character
/// that is not an ASCII letter or digit turned into `-` (one per UTF-16 unit,
/// as Claude Code's JavaScript replaces), so
/// `/Users/felix/.guild/instances/x_y` → `-Users-felix--guild-instances-x-y`.
fn dir_name(cwd: &str) -> String {
    let natural = cwd
        .chars()
        .flat_map(|ch| match ch.is_ascii_alphanumeric() {
            true => std::iter::repeat_n(ch, 1),
            false => std::iter::repeat_n('-', ch.len_utf16()),
        })
        .collect();
    session::stored_name(natural, cwd)
}

/// The turn for `cwd`, or for the session its initial brief pointer names
/// when Claude Code starts in a start directory. A sidechain is a child session.
pub(super) fn recorded_turn(cwd: &str, text: &str, not_before: Option<u64>, brief: Option<&str>) -> Option<Turn> {
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
pub(super) fn names_brief(text: &str, brief: &str) -> bool {
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

/// The line a stand-in writes for `line`, typed while the session worked: a
/// `queued_command` attachment once its model has it, else only the
/// `queue-operation` that holds it in the input queue.
pub(super) fn note_files(cwd: &str, line: &str, taken: bool, at: u64) -> Vec<SessionFile> {
    let iso = session::format_iso(at);
    let record = if taken {
        session::object(vec![
            ("type", "attachment".into()),
            ("timestamp", iso.clone().into()),
            ("cwd", cwd.into()),
            ("isSidechain", false.into()),
            (
                "attachment",
                session::object(vec![
                    ("type", "queued_command".into()),
                    ("prompt", line.into()),
                    ("commandMode", "prompt".into()),
                    ("timestamp", iso.into()),
                ]),
            ),
        ])
    } else {
        session::object(vec![
            ("type", "queue-operation".into()),
            ("operation", "enqueue".into()),
            ("timestamp", iso.into()),
            ("content", line.into()),
        ])
    };
    vec![SessionFile {
        path: format!("projects/{}/factorium-note.jsonl", dir_name(cwd)),
        body: format!("{record}\n"),
        compressed: false,
        replaces: None,
    }]
}

/// One session file a stand-in writes for this turn.
pub(super) fn session_files(cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
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
    vec![SessionFile {
        path: format!("projects/{}/factorium-turn.jsonl", dir_name(cwd)),
        body,
        compressed: false,
        replaces: None,
    }]
}

/// The names Claude Code gives the counts of a message's usage.
const USAGE_KEYS: Keys = Keys {
    input: "input_tokens",
    cache_read: "cache_read_input_tokens",
    cache_write: "cache_creation_input_tokens",
    output: "output_tokens",
    reasoning: "reasoning_tokens",
};

/// What one Claude Code session line says of usage: each assistant message,
/// keyed by its id, as a streamed message repeats it. Thinking, where stated,
/// is its reasoning.
pub(super) fn usage(value: &serde_json::Value, line: usize) -> Option<Usage> {
    let message = &value["message"];
    if value["type"] != "assistant" || message["role"] != "assistant" {
        return None;
    }
    let usage = &message["usage"];
    if !usage.is_object() {
        return None;
    }
    let Some(mut counts) = Counts::read(usage, &USAGE_KEYS, false) else {
        return Some(Usage::Uncounted);
    };
    counts.reasoning = usage["output_tokens_details"]["thinking_tokens"].as_u64().or(counts.reasoning);
    Some(Usage::Message {
        key: message_key(value, line),
        model: message["model"].as_str().unwrap_or("").to_string(),
        counts,
    })
}

/// What one Claude Code session line says of a tool call: an assistant
/// message's `tool_use` parts are calls, and a user message's `tool_result`
/// parts what came back, `is_error` where Claude Code marks it an error. A
/// sidechain is a child session's, and a request Claude Code keeps beside
/// the session (`api-request-blob`) repeats a message: neither is a step.
pub(super) fn said(value: &serde_json::Value) -> Vec<Said> {
    if value["isSidechain"] == true {
        return Vec::new();
    }
    let Some(parts) = value["message"]["content"].as_array() else { return Vec::new() };
    let at = value["timestamp"].as_str().and_then(session::parse_iso);
    let kind = value["type"].as_str();
    parts
        .iter()
        .filter_map(|part| match (kind, part["type"].as_str()) {
            (Some("assistant"), Some("tool_use")) => Some(Said::Call {
                id: part["id"].as_str()?.to_string(),
                action: steps::action(part["name"].as_str().unwrap_or(""), &part["input"]),
            }),
            (Some("user"), Some("tool_result")) => Some(Said::Result {
                id: part["tool_use_id"].as_str()?.to_string(),
                result: steps::content(&part["content"]),
                error: part["is_error"] == true,
                at,
            }),
            _ => None,
        })
        .collect()
}

/// Where Claude Code reads the command its status line runs: the user's
/// settings, under `statusLine`. It hands that command a JSON object on
/// stdin, whose `rate_limits` [`windows`] reads.
pub(super) const STATUS_LINE: StatusLine = StatusLine { settings: USER_HOOKS.settings, key: "statusLine" };

/// How Claude Code is asked for its account's windows now. Its account usage
/// (`/api/oauth/usage`) answers only a login with the `user:profile` scope,
/// which a `claude setup-token` token lacks: Claude Code 2.1.326 itself reads
/// such a token's windows from a one-token "quota" request. Its smallest such
/// request a person can make is one turn on its smallest model, with no
/// tools, settings, servers or kept session, whose stream says the windows
/// in its `rate_limit_event` (recorded on this host: 3 s, under $0.0002).
pub(super) const WINDOWS_QUESTION: super::WindowsQuestion = super::WindowsQuestion {
    line: "claude -p --model haiku --output-format stream-json --verbose --tools '' \
           --strict-mcp-config --no-session-persistence --setting-sources '' \
           --system-prompt 'Answer with one word.' quota",
    spends_turn: true,
};

/// The windows one record of Claude Code's says: the `rate_limits` of the
/// input it hands its status line (`used_percentage`, `resets_at`), or the
/// `rate_limit_info` of a `rate_limit_event` in its stream, whose
/// `unifiedWindows` each give a `utilization` from 0 to 1 and a `resetsAt`.
pub(super) fn windows(record: &serde_json::Value) -> Vec<Window> {
    if let Some(limits) = record["rate_limits"].as_object() {
        return limits
            .iter()
            .filter_map(|(name, window)| {
                Some(Window {
                    name: name.clone(),
                    used_percent: window::number(&window["used_percentage"])?,
                    minutes: None,
                    resets_at: window::seconds(&window["resets_at"]),
                })
            })
            .collect();
    }
    if record["type"] != "rate_limit_event" {
        return Vec::new();
    }
    let info = &record["rate_limit_info"];
    let unified = |name: &str, window: &serde_json::Value| {
        Some(Window {
            name: name.to_string(),
            used_percent: window::number(&window["utilization"])? * 100.0,
            minutes: None,
            resets_at: window::seconds(&window["resetsAt"]),
        })
    };
    if let Some(windows) = info["unifiedWindows"].as_object() {
        return windows.iter().filter_map(|(name, window)| unified(name, window)).collect();
    }
    info["rateLimitType"].as_str().and_then(|name| unified(name, info)).into_iter().collect()
}

/// The limit one record of Claude Code's says it met. Its stream says a
/// rejected request with a `rate_limit_event` whose status is `rejected`, and
/// its session and stream with the synthetic assistant message whose
/// `error` is `rate_limit`. That message is a usage limit when it says
/// `usage_limit_reached` or carries the rejected limit's info, and otherwise
/// the service's own throttle ("not your usage limit"). The stream's
/// `result` of a usage limit says it too, without its reset.
pub(super) fn limit_event(record: &serde_json::Value) -> Option<Limit> {
    let usage = |info: &serde_json::Value| Limit {
        kind: Limited::Usage,
        window: info["rateLimitType"].as_str().map(str::to_string),
        resets_at: window::seconds(&info["resetsAt"]),
    };
    let api_error = record["apiError"].as_str().or_else(|| record["api_error"].as_str());
    match record["type"].as_str()? {
        "rate_limit_event" => {
            let info = &record["rate_limit_info"];
            (info["status"] == "rejected").then(|| usage(info))
        }
        "assistant" if record["error"] == "rate_limit" => {
            let params = if record["apiErrorParams"].is_object() {
                &record["apiErrorParams"]
            } else {
                &record["api_error_params"]
            };
            let rejected = [&params["rate_limit_info"], &record["quotaLimits"]]
                .into_iter()
                .find(|info| info["status"] == "rejected");
            match rejected {
                Some(info) => Some(usage(info)),
                None if api_error == Some(USAGE_LIMIT) => Some(usage(&serde_json::Value::Null)),
                None => Some(Limit { kind: Limited::Throttle, window: None, resets_at: None }),
            }
        }
        "result" if api_error == Some(USAGE_LIMIT) => Some(usage(&serde_json::Value::Null)),
        _ => None,
    }
}

/// What Claude Code calls the API error of an account's usage limit.
const USAGE_LIMIT: &str = "usage_limit_reached";

/// The message's id, else the line's, else its line number.
pub(super) fn message_key(value: &serde_json::Value, line: usize) -> String {
    value["message"]["id"]
        .as_str()
        .or_else(|| value["id"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("line:{line}"))
}

/// The events of Claude Code that say its session's status
/// (`Harness::status_events`): one command is registered at every one of
/// them (`status_hooks`). `PermissionRequest` names the call a permission
/// prompt asks about, which the `Notification` after it does not; and
/// `StopFailure` is a turn that ended in an error.
pub(super) const STATUS_EVENTS: [&str; 10] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Notification",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

/// The tool by which Claude Code asks its user a question.
const QUESTION_TOOL: &str = "AskUserQuestion";
/// What of a tool's input names its call, in order: a shell line, a file,
/// a search, a page, a task.
const CALL_KEYS: [&str; 8] = ["command", "file_path", "path", "pattern", "url", "query", "description", "prompt"];
/// The error a `StopFailure` names where the session's login was refused.
const AUTH_FAILED: &str = "authentication_failed";

/// The question a call of `QUESTION_TOOL` asks: its first.
fn question(input: &serde_json::Value) -> Option<String> {
    input["questions"][0]["question"].as_str().and_then(status::one_line)
}

/// What Claude Code's hook event `payload` says of its session's status.
/// Recorded from Claude Code 2.1.322 (`tests/recorded/claude-code-hooks.jsonl`).
pub(super) fn hook_status(payload: &serde_json::Value) -> Reading {
    let text = |key: &str| payload[key].as_str().and_then(status::one_line);
    let tool = payload["tool_name"].as_str().map(str::to_string);
    let input = &payload["tool_input"];
    let asks = tool.as_deref() == Some(QUESTION_TOOL);
    let call = |status: Status, step: Option<CallEnd>| {
        let message = match tool.as_deref() {
            Some(QUESTION_TOOL) => question(input),
            Some(name) => Some(status::call_line(name, input, &CALL_KEYS)),
            None => None,
        };
        Reading::Is(Report { status, message, tool: tool.clone(), end: step })
    };
    match payload["hook_event_name"].as_str().unwrap_or_default() {
        "SessionStart" | "SessionEnd" => Reading::status(Status::Idle),
        "UserPromptSubmit" => Reading::told(Status::Working, text("prompt")),
        // A question is asked as a call is made, before any permission.
        "PreToolUse" if asks => call(Status::Blocked(Blocked::Question), Some(CallEnd::Start)),
        "PreToolUse" => call(Status::Working, Some(CallEnd::Start)),
        "PermissionRequest" if asks => call(Status::Blocked(Blocked::Question), None),
        "PermissionRequest" => call(Status::Blocked(Blocked::Permission), None),
        "PostToolUse" => call(Status::Working, Some(CallEnd::Result { error: None })),
        "PostToolUseFailure" => {
            let error = payload["error"].as_str().map(str::to_string);
            let message = error.as_deref().and_then(status::one_line);
            Reading::Is(Report { status: Status::Working, message, tool, end: Some(CallEnd::Result { error }) })
        }
        "Notification" => match payload["notification_type"].as_str() {
            Some("idle_prompt") => Reading::told(Status::Idle, text("message")),
            Some("elicitation_dialog") => Reading::told(Status::Blocked(Blocked::Question), text("message")),
            // A permission prompt's own `PermissionRequest` came before
            // it, naming its call; a login's success says nothing new.
            _ => Reading::Unchanged,
        },
        "Stop" => Reading::told(Status::Done, text("last_assistant_message")),
        "StopFailure" => {
            let status = match payload["error"].as_str() {
                Some(AUTH_FAILED) => Status::Blocked(Blocked::Auth),
                _ => Status::Error,
            };
            Reading::told(status, text("last_assistant_message").or_else(|| text("error")))
        }
        _ => Reading::Unchanged,
    }
}
