//! Grok, as a harness. Grok loads no hooks from a project file, so each start
//! gets a generated home whose config holds the start directory's hooks and
//! denies, the guard and the report. A session works only in its cwd, so it
//! starts in the worktree; the start directory's rules are told in that
//! home, and its skills are named there.

use std::collections::BTreeMap;

use serde::de::{Error, IgnoredAny};
use serde::{Deserialize, Deserializer};
use toml::Value as Toml;

use super::session::{self, json_lines};
use super::{Held, Kept, Keystroke, Link, SessionFile, SessionLayout};
use crate::harness::Call;
use crate::line::Line;
use crate::start::RULES;
use crate::start::{Rules, Start, Told};
use crate::steps::{self, Said};
use crate::tokens::{Counts, Keys, Usage};
use crate::turn::{Marker, Note, Turn};

pub(super) const NAME: &str = "Grok";
pub(super) const KEY: &str = "grok";
pub(super) const PROGRAM: &str = "grok";
/// The line that installs it.
pub(super) const INSTALL: &str = "curl -fsSL https://x.ai/cli/install.sh | bash";
/// A session of it outside a start directory gets a home of its own.
pub(super) const OWN_HOME_OUTSIDE_START: bool = true;
/// The line that logs it in with a device code, opened on any other device:
/// what logs in an account's home on its machine (github.com/xai-org/grok-build user guide 02-authentication, 2026-11-16).
pub(super) const DEVICE_LOGIN: Option<&str> = Some("grok login --device-auth");
/// How it is asked whether it is logged in, and logged in.
pub(super) const LOGIN: Option<super::Login> = Some(super::Login {
    question: "grok models",
    logged_out: "is not logged in or listed no model",
    line: "grok login",
});

/// Whether the answer to its login question says it is logged in: it does
/// not say it is not, and names a model.
pub(super) fn logged_in(succeeded: bool, printed: &str) -> bool {
    succeeded && !not_authenticated(printed) && models(printed).is_some()
}
/// Where it reads its user's global instructions, under the user's home.
pub(super) const INSTRUCTIONS: &str = ".grok/rules/AGENTS.md";
/// The agent `npx skills` installs its user's skills for.
pub(super) const SKILLS_AGENT: Option<&str> = Some("grok");
/// Lets an unattended interactive session run. Deny rules and hooks still apply.
pub(super) const STOCK: &str = "grok --always-approve";

pub(super) const MODEL: &str = "--model";
/// On the launch line. Grok 1.0.70 accepts it and `--help` does not name it,
/// so it is not one of `FLAGS`: the flag check does not require `--help` to.
pub(super) const TRUST: &str = "--trust";
/// `TRUST` is a flag `--help` does not name, and not one of `FLAGS`: outside
/// a start directory a launch adds no flag it checks the command takes.
pub(super) const TRUST_IS_FLAGS: bool = false;
/// `--add-dir` is not one of these. Grok 1.0.60 and later reject
/// `additionalDirectories` on session create, so the tool never passes it
///. A command must take `--model`.
pub(super) const FLAGS: [&str; 1] = [MODEL];
const HELP: &str = " --help";
const MODELS: &str = " models";
const PROMPT: &str = " -p";
const AS_JSON: &str = " --output-format json";

pub(super) const USER_DIR: &str = "GROK_HOME";
/// Nothing here is set to `=1`. The disables are `=0`, written in `environment`.
pub(super) const ENVIRONMENT: [&str; 0] = [];

pub(super) const KEPT: Kept = Kept {
    links: &[Link { home: ".grok/auth.json", kept: "auth.json" }],
    host_user: &[Link { home: ".grok/skills", kept: "skills" }, Link { home: ".grok/commands", kept: "commands" }],
    files: &[],
};

pub(super) const LIMITS: &str = ".grok/config.toml";
pub(super) const REPO_RULES: [&str; 2] = ["AGENTS.md", RULES];
/// The start directory's rules. The session cwd is the worktree, so this file is how
/// they reach the session.
pub(super) const RULES_FILE: &str = RULES;
/// Where the generated home writes the repo and added rules. Not the user's
/// rules directory: that one is named, when the session is given it, and this
/// file has to stay the tool's.
pub(super) const SYSTEM_FILE: &str = "rules/told.md";
pub(super) const REFUSED: u8 = 2;

const HOOK_TIMEOUT: i64 = 10;

/// Events the report rides, besides `PreToolUse` which also asks the guard.
/// Documented for Grok; a denied call may not reach the report on that one
/// event, and busy and stopped still come from the others.
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

/// Home skill and command directories a session is not given when `host_user`
/// is false. `~` is the host user's home, which Grok expands in its own config.
const HOME_SKILLS: [&str; 7] = [
    "~/.claude/skills",
    "~/.claude/commands",
    "~/.agents/skills",
    "~/.agents/commands",
    "~/.cursor/skills",
    "~/.grok/skills",
    "~/.grok/commands",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Limits {
    #[serde(default)]
    hooks: BTreeMap<String, Vec<IgnoredAny>>,
    #[serde(default)]
    permission: Permissions,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Permissions {
    #[serde(default)]
    deny: Vec<String>,
    /// Accepted so a start directory can name them beside a deny. Neither is a limit.
    #[serde(default, rename = "allow")]
    _allow: Vec<IgnoredAny>,
    #[serde(default, rename = "ask")]
    _ask: Vec<IgnoredAny>,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    #[serde(default)]
    decision: String,
}

pub(super) fn limited<'de, D: Deserializer<'de>>(held: Held<D>) -> Result<bool, D::Error> {
    match held {
        Held::Nothing => Ok(false),
        Held::Entries(_) => Err(D::Error::custom("it is a directory, not a TOML file")),
        Held::File(limits) => {
            let limits = Limits::deserialize(limits)?;
            Ok(!limits.permission.deny.is_empty()
                || limits.permission.rules.iter().any(|rule| rule.decision == "deny")
                || limits.hooks.values().any(|hooks| !hooks.is_empty()))
        }
    }
}

/// Reads a start directory's TOML limits through the same decision as the other harnesses.
pub(super) fn limited_file(text: &str) -> Result<bool, String> {
    let deserializer = toml::de::Deserializer::new(text);
    limited(Held::File(deserializer)).map_err(|error| error.to_string())
}

/// The configuration a session or a shared home gets. The host user's Grok
/// config is not read: its hooks, MCP servers, plugins, permission mode and
/// memories stay out. A session also gets its start directory's limits and the tool's
/// hooks; a listing or a probe gets the compatibility switches and neither.
pub(super) fn configuration(
    role: Option<&str>,
    guard: Option<&str>,
    host_user: bool,
    start_skills: Option<&str>,
    skill_names: &[String],
) -> Result<String, String> {
    let mut table = toml::map::Map::new();
    table.insert("compat".into(), compat(host_user));
    if host_user {
        let mut paths = toml::map::Map::new();
        paths.insert("extra_rule_dirs".into(), Toml::Array(vec![Toml::from("~/.grok/rules")]));
        table.insert("paths".into(), Toml::Table(paths));
    }
    if let Some(skills) = skills_table(host_user, start_skills, skill_names) {
        table.insert("skills".into(), skills);
    }
    if let Some(role) = role.filter(|text| !text.trim().is_empty()) {
        let role = role.parse::<Toml>().map_err(|error| format!("the start directory's Grok limits: {error}"))?;
        let role = role.as_table().ok_or("the start directory's Grok limits are not a table")?;
        for (key, value) in role {
            if !matches!(key.as_str(), "hooks" | "permission") {
                return Err(format!("the start directory's Grok limits hold unsupported key `{key}`"));
            }
            if !value.is_table() {
                return Err(format!("the start directory's Grok {key} is not a table"));
            }
            table.insert(key.clone(), value.clone());
        }
    }
    if let Some(guard) = guard {
        let hooks = table.entry("hooks").or_insert_with(|| Toml::Table(Default::default()));
        let hooks = hooks.as_table_mut().ok_or("the start directory's Grok hooks are not a table")?;
        let fleet = group(vec![command_hook(&guard_asked(guard)), command_hook(REPORT)]);
        prepend(hooks, "PreToolUse", fleet)?;
        for event in STATE_EVENTS {
            prepend(hooks, event, group(vec![command_hook(REPORT)]))?;
        }
    }
    toml::to_string_pretty(&Toml::Table(table)).map_err(|error| error.to_string())
}

/// The start directory's skills, and the host skills ignored. With
/// `host_user` false all host skill directories are ignored; with it true,
/// only one named like a start directory skill, so that one wins.
fn skills_table(host_user: bool, start_skills: Option<&str>, names: &[String]) -> Option<Toml> {
    let mut skills = toml::map::Map::new();
    let start_skills = start_skills.filter(|dir| !dir.is_empty());
    if !host_user {
        let ignore = HOME_SKILLS.into_iter().map(Toml::from).collect();
        skills.insert("ignore".into(), Toml::Array(ignore));
    } else if start_skills.is_some() {
        let mut ignore = Vec::new();
        for name in names {
            if name.is_empty() || name.contains('/') || name.contains('\\') {
                continue;
            }
            for root in HOME_SKILLS {
                ignore.push(Toml::from(format!("{root}/{name}")));
            }
        }
        if !ignore.is_empty() {
            skills.insert("ignore".into(), Toml::Array(ignore));
        }
    }
    if let Some(dir) = start_skills {
        skills.insert("paths".into(), Toml::Array(vec![Toml::from(dir)]));
    }
    if skills.is_empty() { None } else { Some(Toml::Table(skills)) }
}

fn compat(host_user: bool) -> Toml {
    let mut claude = toml::map::Map::new();
    claude.insert("hooks".into(), Toml::from(false));
    claude.insert("mcps".into(), Toml::from(false));
    let mut cursor = toml::map::Map::new();
    cursor.insert("hooks".into(), Toml::from(false));
    cursor.insert("mcps".into(), Toml::from(false));
    if !host_user {
        for cell in ["rules", "agents"] {
            claude.insert(cell.into(), Toml::from(false));
        }
        for cell in ["skills", "rules", "agents"] {
            cursor.insert(cell.into(), Toml::from(false));
        }
    }
    let mut compat = toml::map::Map::new();
    compat.insert("claude".into(), Toml::Table(claude));
    compat.insert("cursor".into(), Toml::Table(cursor));
    Toml::Table(compat)
}

fn command_hook(command: &str) -> Toml {
    let mut hook = toml::map::Map::new();
    hook.insert("type".into(), Toml::from("command"));
    hook.insert("command".into(), Toml::from(command));
    hook.insert("timeout".into(), Toml::from(HOOK_TIMEOUT));
    Toml::Table(hook)
}

fn group(hooks: Vec<Toml>) -> Toml {
    let mut group = toml::map::Map::new();
    group.insert("hooks".into(), Toml::Array(hooks));
    Toml::Table(group)
}

fn prepend(hooks: &mut toml::map::Map<String, Toml>, event: &str, group: Toml) -> Result<(), String> {
    let entry = hooks.entry(event).or_insert_with(|| Toml::Array(vec![]));
    let array =
        entry.as_array_mut().ok_or_else(|| format!("the start directory's Grok hooks.{event} is not an array"))?;
    array.insert(0, group);
    Ok(())
}

/// Grok's hooks fail open on an unreadable or failed guard. Turn every such
/// outcome into the explicit refusal code, with the guard's reason on stderr.
fn guard_asked(guard: &str) -> String {
    format!(
        "said=$({guard} 2>&1 >/dev/null) && exit 0; \
         printf '%s\\n' \"${{said:-factorium guard: it could not be asked}}\" >&2; exit 2"
    )
}

const REPORT: &str = "hook=\"${HOME-}/.orca/agent-hooks/grok-hook.sh\"; \
     if [ -x \"$hook\" ]; then ORCA_HOOK_AGENT_PID=\"$PPID\" /bin/sh \"$hook\"; \
     else cat >/dev/null; fi; exit 0";

/// Start directory, repo and added rules, as markdown Grok loads from its home. The
/// session starts in the worktree, so the start directory's `CLAUDE.md` is told here
/// rather than loaded as the start directory's own rules.
pub(super) fn system(told: &Told) -> Option<String> {
    if told.own.is_none() && told.repo.is_none() && told.added.is_none() {
        return None;
    }
    let mut text = String::new();
    if let Some(rules) = told.own {
        told_rules(&mut text, "The start directory's own rules", rules);
    }
    if let Some(rules) = told.repo {
        told_rules(&mut text, "The repo's own rules", rules);
    }
    if let Some(rules) = told.added {
        told_rules(&mut text, "The added rules", rules);
    }
    Some(text)
}

fn told_rules(text: &mut String, whose: &str, rules: Rules) {
    if !text.is_empty() {
        text.push_str("\n\n");
    }
    text.push_str(&format!("# {whose}: {}\n\n{}\n", rules.file, rules.text.trim_end()));
}

/// Appends `--trust` when the command does not already carry that word.
/// A path is not that word.
pub(super) fn in_worktree(command: &str) -> String {
    if command.split_whitespace().any(|word| word == TRUST) {
        command.to_string()
    } else {
        format!("{command} {TRUST}")
    }
}

/// Starts one interactive session interactively: the stock command, trust, and the model.
pub(super) fn interactive(command: &str, model: Option<&str>) -> String {
    let line = Line::new().launch(&in_worktree(command));
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.done()
}

/// Uses the generated home containing the tool's hooks.
pub(super) fn guarded(command: &str, kept: &str) -> String {
    Line::new().fixed("export ").fixed(USER_DIR).fixed("=").word(kept).fixed(" && ").launch(command).done()
}

pub(super) fn environment(line: Line, start: &Start) -> Line {
    let line = line
        .fixed(" ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(start.kept)
        .fixed(" GROK_MEMORY=0")
        .fixed(" GROK_CLAUDE_HOOKS_ENABLED=0")
        .fixed(" GROK_CLAUDE_MCPS_ENABLED=0")
        .fixed(" GROK_CURSOR_HOOKS_ENABLED=0")
        .fixed(" GROK_CURSOR_MCPS_ENABLED=0");
    if start.host_user {
        line
    } else {
        line.fixed(" GROK_CLAUDE_RULES_ENABLED=0")
            .fixed(" GROK_CLAUDE_AGENTS_ENABLED=0")
            .fixed(" GROK_CURSOR_RULES_ENABLED=0")
            .fixed(" GROK_CURSOR_AGENTS_ENABLED=0")
            .fixed(" GROK_CURSOR_SKILLS_ENABLED=0")
    }
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

pub(super) fn asked_its_flags(line: Line) -> Line {
    line.fixed(HELP)
}

pub(super) fn models_question(command: &str, kept: &str) -> String {
    Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command)
        .fixed(MODELS)
        .done()
}

pub(super) fn prompt_question(command: &str, kept: &str, model: Option<&str>, prompt: &str) -> String {
    asked(command, kept, model, prompt, false)
}

pub(super) fn probe_question(command: &str, kept: &str, model: Option<&str>, prompt: &str) -> String {
    asked(command, kept, model, prompt, true)
}

fn asked(command: &str, kept: &str, model: Option<&str>, prompt: &str, json: bool) -> String {
    let line = Line::new().fixed("export ").fixed(USER_DIR).fixed("=").word(kept).fixed(" && ").launch(command);
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(PROMPT).fixed(" ").word(prompt).fixed(if json { AS_JSON } else { "" }).done()
}

/// What comes before the first model's id in a headless JSON answer.
const USAGE: &str = "\"modelUsage\":{\"";

pub(super) fn answered_as(answer: &str) -> Option<String> {
    let dense: String = answer.split_whitespace().collect();
    let (_, after) = dense.split_once(USAGE)?;
    let (id, _) = after.split_once('"')?;
    (!id.is_empty()).then(|| id.to_string())
}

/// The ids under `Available models:`. The one marked `*` or `(default)` is
/// first: it is the model sessions start on until research places them.
/// A line that is not an id ends the list. No section, or no id, is none.
pub(super) fn models(answer: &str) -> Option<Vec<String>> {
    let (_, rest) = answer.split_once("Available models:")?;
    let mut listed = vec![];
    let mut default = None;
    // The colon ends the header line, so the first line of what follows is empty.
    for line in rest.lines().skip_while(|line| line.trim().is_empty()) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        let marked = trimmed.starts_with('*') || trimmed.contains("(default)");
        let id = trimmed
            .trim_start_matches(|c: char| c == '*' || c == '-' || c.is_whitespace())
            .trim_end_matches("(default)")
            .trim();
        if id.is_empty() || id.split_whitespace().nth(1).is_some() {
            break;
        }
        if marked && default.is_none() {
            default = Some(id.to_string());
        } else if default.as_deref() != Some(id) {
            listed.push(id.to_string());
        }
    }
    let mut models = Vec::new();
    if let Some(default) = default {
        models.push(default);
    }
    models.extend(listed);
    if models.is_empty() { None } else { Some(models) }
}

pub(super) fn shown(_answer: &str) -> Vec<(String, String)> {
    vec![]
}

/// Whether a listing says the login is missing. A listing that is not logged
/// in still names models, so the models alone do not say it is logged in.
fn not_authenticated(answer: &str) -> bool {
    answer.to_ascii_lowercase().contains("not authenticated")
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

/// The harness directory under a user's home.
pub(super) const HOME: &str = ".grok";

const EVENTS: &str = "events.jsonl";

/// Where Grok keeps the session of `cwd`: one events file in each session
/// directory under the worktree's directory, its usage in the updates
/// beside them.
pub(super) fn session_layout(cwd: &str) -> SessionLayout {
    SessionLayout::Named {
        dir: format!("sessions/{}", dir_name(cwd)),
        name: EVENTS,
        compressed: false,
        usage: Some(UPDATES),
    }
}

/// What was said in each session: the user's lines and the model's, beside
/// its events.
const CHAT: &str = "chat_history.jsonl";

/// Where Grok keeps what was said in the session of `cwd`.
pub(super) fn note_layout(cwd: &str) -> SessionLayout {
    SessionLayout::Named { dir: format!("sessions/{}", dir_name(cwd)), name: CHAT, compressed: false, usage: None }
}

/// What a busy Grok prints when a typed line waits in its input queue, and
/// the key that hint names. Measured 2026-11-01: the
/// line stays queued until Enter, which the screen calls send now. A line
/// still only queued is not a message the model was given.
const QUEUE: &str = "Queued";
const SEND_NOW: &str = "Enter:send now";
/// The shortest visible piece of the typed line that counts as that line on
/// a truncated queue row. Shorter, and a header could match.
const SHOWN_MIN: usize = 8;

/// The key that hands `line` to the model now, when `screen` shows it
/// waiting in the input queue. `None` when the screen does not show that
/// queue, or does not show this line on it.
pub(super) fn send_queued(screen: &[String], line: &str) -> Option<Keystroke> {
    let line = squash(line);
    if line.chars().count() < SHOWN_MIN {
        return None;
    }
    let flat = squash(&screen.join(" "));
    if !flat.contains(QUEUE) || !flat.contains(SEND_NOW) {
        return None;
    }
    screen.iter().any(|raw| shows_item(raw, &line)).then_some(Keystroke { text: None, enter: true })
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `raw` is a queue row whose visible text is `line`, or the head of
/// it where the row is cut short.
fn shows_item(raw: &str, line: &str) -> bool {
    let shown = visible_item(raw);
    let n = shown.chars().count();
    n >= SHOWN_MIN && (line.starts_with(&shown) || shown.starts_with(line))
}

/// A queue row with its index and a trailing ellipsis taken off.
fn visible_item(raw: &str) -> String {
    let shown = squash(raw);
    let shown = match shown.strip_prefix('#') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => {
            rest.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start()
        }
        _ => shown.as_str(),
    };
    shown.trim_end_matches(['.', '…']).trim().to_string()
}

/// The chat entry a stand-in writes for `line`, typed while the session
/// worked, once Grok gave it to its model: as Grok words it. A line still in
/// its input queue is in no file.
pub(super) fn note_files(cwd: &str, line: &str, taken: bool, _at: u64) -> Vec<SessionFile> {
    if !taken {
        return Vec::new();
    }
    let said = format!("The user sent a message while you were working: <user_query>{line}</user_query>");
    let entry = session::object(vec![("role", "user".into()), ("content", said.into())]);
    vec![SessionFile {
        path: format!("sessions/{}/factorium-turn/{CHAT}", dir_name(cwd)),
        body: format!("{entry}\n"),
        compressed: false,
        replaces: None,
    }]
}

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

/// The file beside a session's events whose updates state its usage.
const UPDATES: &str = "updates.jsonl";

/// The names Grok gives the counts of one model's usage. Its input includes
/// the cached input.
const USAGE_KEYS: Keys = Keys {
    input: "inputTokens",
    cache_read: "cachedReadTokens",
    cache_write: "cacheCreationTokens",
    output: "outputTokens",
    reasoning: "reasoningTokens",
};

/// What one line of a Grok session's updates says of usage: every model's
/// running usage so far.
pub(super) fn usage(value: &serde_json::Value, _line: usize) -> Option<Usage> {
    let usage = &value["params"]["update"]["usage"];
    if !usage.is_object() {
        return None;
    }
    let Some(models) = usage["modelUsage"].as_object() else { return Some(Usage::Uncounted) };
    Some(Usage::Snapshot(
        models
            .iter()
            .filter_map(|(name, usage)| Some((name.clone(), Counts::read(usage, &USAGE_KEYS, true)?)))
            .collect(),
    ))
}

/// What one line of Grok's session updates (`updates.jsonl`, beside its
/// events) says of a tool call: a `tool_call` update is a call, and a
/// `tool_call_update` that is `completed` or `failed` what came back of the
/// one its `toolCallId` names. One that failed, or whose command exited
/// other than 0 (`rawOutput.exit_code`), is an error. A sub-agent is a
/// session directory of its own.
pub(super) fn said(value: &serde_json::Value) -> Vec<Said> {
    let update = &value["params"]["update"];
    let Some(id) = update["toolCallId"].as_str().filter(|id| !id.is_empty()) else {
        return Vec::new();
    };
    let id = id.to_string();
    let said = match update["sessionUpdate"].as_str() {
        Some("tool_call") => {
            let name = update["_meta"]["x.ai/tool"]["name"].as_str().or(update["title"].as_str()).unwrap_or("");
            Said::Call { id, action: steps::action(name, &update["rawInput"]) }
        }
        Some("tool_call_update") if update["status"] == "completed" || update["status"] == "failed" => {
            let parts: Vec<serde_json::Value> =
                update["content"].as_array().into_iter().flatten().map(|part| part["content"].clone()).collect();
            let code = update["rawOutput"]["exit_code"].as_i64();
            let at = session::number_at(
                value["ts"].as_u64().or(value["params"]["_meta"]["agentTimestampMs"].as_u64()),
                value["ts"].as_str(),
            );
            Said::Result {
                id,
                result: steps::content(&serde_json::Value::Array(parts)),
                error: update["status"] == "failed" || code.is_some_and(|code| code != 0),
                at,
            }
        }
        _ => return Vec::new(),
    };
    vec![said]
}
