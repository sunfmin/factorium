//! Codex, as a harness. It is started as an interactive session, never as
//! one run of a prompt. It cannot be told to leave the host user's
//! configuration out, so it gets a private user directory per session, and
//! everything it is told — hooks, trust, rules from outside its start
//! directory — goes on its start line. It finds skills in directories of its
//! own; it also always loads the host user's home skills, and of two skills
//! of one name it shows both, so a shadowed host skill is turned off.

use std::collections::BTreeMap;

use serde::de::{Error, IgnoredAny};
use serde::{Deserialize, Deserializer};
use serde_json::json;

use super::session::{self, json_lines};
use super::{Held, Kept, Link, SessionFile, SessionLayout};
use crate::harness::Call;
use crate::line::Line;
use crate::start::RULES;
use crate::start::{Rules, Start, Told};
use crate::status::{self, Blocked, CallEnd, Reading, Report, Status};
use crate::steps::{self, Said};
use crate::tokens::{Counts, Keys, Usage};
use crate::turn::{Marker, Note, Turn};
use crate::window::{self, Limit, Limited, Window};

/// The screen with its whitespace collapsed, so wrapped words still match.
fn joined(output: &[String]) -> String {
    output.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Codex's task-refusal screen, seen 2026-10-26.
pub(super) fn task_refusal(output: &[String]) -> Option<String> {
    let words = "We take extra care with some cybersecurity requests. If you're doing authorized security work, apply for Daybreak to get broader access.";
    joined(output).contains(words).then(|| words.to_string())
}

/// Codex's capacity words, seen 2026-10-26.
pub(super) fn capacity_stop(output: &[String]) -> Option<String> {
    let words = "Selected model is at capacity. Please try a different model.";
    joined(output).contains(words).then(|| words.to_string())
}

/// Codex's usage-limit screen, an account quota: both clauses, in any case,
/// possibly wrapped. A screen that only says a usage limit was reached is
/// not this one.
pub(super) fn account_quota(output: &[String]) -> Option<String> {
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

pub(super) const NAME: &str = "Codex";
pub(super) const KEY: &str = "codex";
pub(super) const PROGRAM: &str = "codex";
pub(super) const INSTALL: &str = "brew install --cask codex";
/// Device-code login (`codex login --help`, codex-cli 0.168.0).
pub(super) const DEVICE_LOGIN: Option<&str> = Some("codex login --device-auth");
/// A session reads its login from `auth.json`; a login kept in a keyring is
/// in no file, and no session can use it.
pub(super) const LOGIN: Option<super::Login> = Some(super::Login {
    question: "test -f \"$HOME/.codex/auth.json\" && codex login status",
    logged_out: "is not logged in where a worker reads its login, `~/.codex/auth.json` (a login \
                 kept in a keyring is in no file)",
    line: "codex login",
});

/// The login question asked of the directory `CODEX_HOME` names.
pub(super) const LOGIN_IN_CONFIG: &str = "test -f \"$CODEX_HOME/auth.json\" && codex login status";
pub(super) const LOGGED_OUT_IN_CONFIG: &str = "is not logged in where its user directory keeps its \
                                               login, `auth.json` (a login kept in a keyring is \
                                               in no file)";

/// Logged in when the question exits 0.
pub(super) fn logged_in(succeeded: bool, _printed: &str) -> bool {
    succeeded
}
pub(super) const INSTRUCTIONS: &str = ".codex/AGENTS.md";
pub(super) const SKILLS_AGENT: Option<&str> = Some("codex");
/// Codex edits files with one tool, `apply_patch`, handed the patch as
/// `tool_input.command`.
pub(super) const USER_HOOKS: super::UserHooks = super::UserHooks {
    settings: ".codex/hooks.json",
    table: "hooks",
    event: "PreToolUse",
    shell: "Bash",
    edits: "apply_patch",
};
/// No approvals and no sandbox: the guard hook is the limit.
pub(super) const STOCK: &str = "codex --dangerously-bypass-approvals-and-sandbox";
pub(super) const TRUST: &str = "trust_level=\"trusted\"";
/// Its trust is the `--config` flags `in_worktree` adds.
pub(super) const TRUST_IS_FLAGS: bool = true;

const ADD_DIR: &str = "--add-dir";
/// One configuration key, `key=value` with the value in TOML.
const CONFIG: &str = "--config";
/// Answers, for one run, the review Codex asks of every unreviewed hook;
/// asked at the terminal it would take the brief for its answer.
const HOOKS_REVIEWED: &str = "--dangerously-bypass-hook-trust";
pub(super) const MODEL: &str = "--model";
/// Every flag a start adds: what a launch command of Codex must take.
pub(super) const FLAGS: [&str; 4] = [ADD_DIR, HOOKS_REVIEWED, CONFIG, MODEL];
const HELP: &str = " --help";

/// The models its login runs, fetched from its server (not `--bundled`).
const LIST_MODELS: &str = " debug models";
/// The catalog carries hundreds of kilobytes of instructions: keep only the
/// names, so a bounded terminal scrollback keeps the start of the answer.
const MODEL_NAMES: &str = " | awk '{ while (match($0, /\"slug\"[[:space:]]*:[[:space:]]*\"[^\"]+\"/)) \
    { print substr($0, RSTART, RLENGTH); $0 = substr($0, RSTART + RLENGTH) } }'";
/// What precedes each model name in the JSON answer; an escaped `\"` inside
/// a string cannot match it.
const NAMED: &str = "\"slug\":\"";

pub(super) const USER_DIR: &str = "CODEX_HOME";

/// Codex's kept user directory links the host user's login and nothing else
/// (so no hook, configuration or custom provider of theirs), and their
/// skills and rules for a session given them. A link, so a renewed login
/// stays fresh in both places.
pub(super) const KEPT: Kept = Kept {
    links: &[Link { home: ".codex/auth.json", kept: "auth.json" }],
    host_user: &[
        Link { home: ".codex/skills", kept: "skills" },
        Link { home: ".codex/AGENTS.override.md", kept: "AGENTS.override.md" },
        Link { home: ".codex/AGENTS.md", kept: "AGENTS.md" },
    ],
    files: &[],
};
/// The host user's skill directories Codex reads beside the start
/// directory's.
pub(super) const SHADOWED_IN: [&str; 2] = [".agents/skills", ".codex/skills"];
/// The key that turns a skill off, by the path of its file.
const SKILLS_CONFIG: &str = "skills.config";

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
/// The directories of the start directory where Codex finds skills.
pub(super) const SKILLS: [&str; 2] = [".agents/skills", ".codex/skills"];

/// Codex loads the first of these a directory holds, before any other.
pub(super) const BEFORE_THE_RULES: [&str; 2] = ["AGENTS.override.md", "AGENTS.md"];
/// The key naming the fallback rules files: the one every harness loads.
const RULES_FILES: &str = "project_doc_fallback_filenames";
/// No root markers: Codex loads only the start directory's rules, not those
/// of every directory above it.
const NO_ROOT: &str = "project_root_markers=[]";
/// Text given to the model as a developer's: how Codex is told rules from
/// outside its start directory.
const TOLD: &str = "developer_instructions";
/// The repo's own rules: the first of these the worktree holds.
pub(super) const REPO_RULES: [&str; 2] = ["AGENTS.md", RULES];
const TOLD_OPENS: &str = "factorium started you, and gives you the rules below. You work under \
     them as you do under the rules of the directory you started in.";
const REPO_S_OWN: &str = "The repo's own rules";
const ADDED_S: &str = "The added rules";

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
pub(super) const REFUSED: u8 = 2;

/// `continue: false` with a `stopReason`, exit 0. Run as `Start::hook` it is
/// inside `GUARD_ASKED`, which drops what it prints.
pub(super) fn stop_answer(reason: &str) -> super::Answer {
    super::Answer { printed: session::stopped(reason), code: 0 }
}
/// A Codex version lets an answer it does not understand pass, and Codex does
/// not call its hooks a full enforcement boundary.
pub(super) const STOP_RELIABLE: bool = false;

const RESUME: &str = " resume";
const LAST: &str = "--last";

/// `line` continuing `session` (the latest where `None`) with `instruction`.
/// Options before the subcommand are the resumed session's.
pub(super) fn resumed(line: Line, session: Option<&str>, instruction: &str) -> Line {
    let line = line.fixed(RESUME).fixed(" ");
    let line = match session {
        Some(session) => line.word(session),
        None => line.fixed(LAST),
    };
    line.fixed(" ").word(instruction)
}

/// The id `session_meta` gives, which ends the file's name.
pub(super) fn session_id(text: &str) -> Option<String> {
    json_lines(text).find_map(|value| {
        (value["type"] == "session_meta")
            .then(|| value["payload"]["id"].as_str().filter(|id| !id.is_empty()))
            .flatten()
            .map(str::to_string)
    })
}

/// The hooks Codex loads from its start directory once told to.
pub(super) const LIMITS: &str = ".codex/hooks.json";

#[derive(Deserialize)]
struct Hooks {
    #[serde(default)]
    hooks: BTreeMap<String, Vec<IgnoredAny>>,
}

/// A hooks file limits a session when it names a hook at some event.
pub(super) fn limited<'de, D: Deserializer<'de>>(held: Held<D>) -> Result<bool, D::Error> {
    match held {
        Held::Nothing => Ok(false),
        Held::Entries(_) => Err(D::Error::custom("it is a directory, not a file of hooks")),
        Held::File(hooks) => {
            let hooks = Hooks::deserialize(hooks)?;
            Ok(hooks.hooks.values().any(|hooks| !hooks.is_empty()))
        }
    }
}

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

/// An interactive command with the guard and report hooks added.
pub(super) fn guarded(command: &str, guard: &str) -> String {
    Line::new().launch(command).fixed(" ").fixed(CONFIG).fixed(" ").word(&added_hooks(guard, REPORT)).done()
}

fn rules_files() -> String {
    format!("{RULES_FILES}=[{}]", toml_text(RULES))
}

fn told_rules(whose: &str, rules: Rules) -> String {
    format!("\n\n# {whose}: {}\n\n<rules>\n{}\n</rules>", rules.file, rules.text.trim_end())
}

/// The rules `told` as one configuration key; `None` when there are none.
fn told(told: &Told) -> Option<String> {
    let repo = told.repo.map(|rules| told_rules(REPO_S_OWN, rules));
    let added = told.added.map(|rules| told_rules(ADDED_S, rules));
    if repo.is_none() && added.is_none() {
        return None;
    }
    let text = format!("{TOLD_OPENS}{}{}", repo.unwrap_or_default(), added.unwrap_or_default());
    Some(format!("{TOLD}={}", toml_text(&text)))
}

/// The key that turns off the skills `shadowed`; `None` when there are none.
fn turned_off(shadowed: &[String]) -> Option<String> {
    if shadowed.is_empty() {
        return None;
    }
    let off: Vec<String> = shadowed.iter().map(|path| format!("{{path={},enabled=false}}", toml_text(path))).collect();
    Some(format!("{SKILLS_CONFIG}=[{}]", off.join(",")))
}

pub(super) fn environment(line: Line, start: &Start) -> Line {
    line.fixed(" ").fixed(USER_DIR).fixed("=").word(start.kept)
}

/// Codex in `worktree` with trust and a private user directory, and nothing
/// added: the repo's rules load from the directory itself.
pub(super) fn in_worktree(command: &str, worktree: &str, kept: &str) -> String {
    let line = Line::new().fixed("export ").fixed(USER_DIR).fixed("=").word(kept).fixed(" && ").launch(command);
    let line = trust(line, worktree).fixed(" ").fixed(CONFIG).fixed(" ").word(&rules_files());
    isolated(line).done()
}

/// An interactive session, as `in_worktree`, on `model`.
pub(super) fn interactive(command: &str, worktree: &str, kept: &str, model: Option<&str>) -> String {
    let line = Line::new().launch(&in_worktree(command, worktree, kept));
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.done()
}

/// Codex keeps each session apart from every other.
pub(super) const SESSION: super::Session = super::Session {
    fresh: "codex",
    note: HOME_NOTE,
    config: ".codex/config.toml",
    record: "factorium-codex-launch.json",
};

/// Keys of the user's config: model, its effort, the service tier.
const PREFERENCE_MODEL: &str = "model";
const EFFORT: &str = "model_reasoning_effort";
const SERVICE_TIER: &str = "service_tier";
const USER_SOURCE: &str = "user_selection";
const DEFAULT_SOURCE: &str = "codex_default";
const LABEL_SOURCE: &str = "model_label";

fn config_text(text: &str, file: &str) -> Result<toml::Table, String> {
    text.parse().map_err(|_| format!("cannot parse {file}; fix its TOML"))
}

/// The model an automatic start uses: `explicit`, else the user's, which
/// must be one of `listed`.
pub(super) fn automatic_model(
    explicit: Option<&str>,
    text: &str,
    file: &str,
    listed: &[String],
) -> Result<(Option<String>, &'static str), String> {
    if let Some(model) = explicit {
        return Ok((Some(model.to_string()), LABEL_SOURCE));
    }
    match config_text(text, file)?.get(PREFERENCE_MODEL) {
        Some(toml::Value::String(model)) if !model.trim().is_empty() => {
            if !listed.iter().any(|listed| listed == model) {
                return Err(format!(
                    "Codex user selection `{model}` from {file}:{PREFERENCE_MODEL} is not among \
                     the models listed for this host; confirm this Codex login offers it"
                ));
            }
            Ok((Some(model.clone()), USER_SOURCE))
        }
        None => Ok((None, DEFAULT_SOURCE)),
        _ => Err(format!(
            "{file}: {PREFERENCE_MODEL} must be a nonempty string; fix it or remove it for \
             Codex's default"
        )),
    }
}

fn preference(text: &str, file: &str, key: &str) -> Result<Option<String>, String> {
    match config_text(text, file)?.get(key) {
        Some(toml::Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.clone())),
        None => Ok(None),
        _ => Err(format!("{file}: {key} must be a nonempty string; fix it or remove it for Codex's default")),
    }
}

/// The effort of a start, from the same file, even with an explicit model.
pub(super) fn effort(text: &str, file: &str) -> Result<(Option<String>, String), String> {
    let effort = preference(text, file, EFFORT)?;
    let source = match &effort {
        Some(_) => format!("{file}:{EFFORT}"),
        None => DEFAULT_SOURCE.to_string(),
    };
    Ok((effort, source))
}

/// The service tier of a start. With none, Codex chooses its own.
pub(super) fn service_tier(text: &str, file: &str) -> Result<Option<String>, String> {
    preference(text, file, SERVICE_TIER)
}

/// What this session saved after it started; `None` when `text` is not
/// Codex's config.
pub(super) fn saved(file: &str, text: &str) -> Option<super::Saved> {
    let config: toml::Table = text.parse().ok()?;
    let model = config.get(PREFERENCE_MODEL).and_then(toml::Value::as_str).map(str::to_string);
    let effort = config.get(EFFORT).and_then(toml::Value::as_str).map(str::to_string);
    Some(super::Saved {
        model: model.clone(),
        model_source: model.map(|_| format!("session:{file}:{PREFERENCE_MODEL}")),
        effort: effort.clone(),
        effort_source: effort.map(|_| format!("session:{file}:{EFFORT}")),
    })
}

/// `command` naming `model`, unless it already does. The model is said once.
pub(super) fn ensure_model(command: &str, model: Option<&str>) -> String {
    match model {
        Some(model) if !command.contains(MODEL) => {
            Line::new().launch(command).fixed(" ").fixed(MODEL).fixed(" ").word(model).done()
        }
        _ => command.to_string(),
    }
}

fn with_preference(line: &str, key: &str, preference: Option<&str>) -> String {
    let line = Line::new().launch(line);
    match preference {
        Some(preference) => {
            let value = format!("{key}={}", toml::Value::String(preference.to_string()));
            line.fixed(" --config ").word(&value).done()
        }
        None => line.done(),
    }
}

/// `line` with the effort, said as Codex takes one key of its configuration.
pub(super) fn with_effort(line: &str, effort: Option<&str>) -> String {
    with_preference(line, EFFORT, effort)
}

/// `line` with the service tier; none leaves Codex's default.
pub(super) fn with_service_tier(line: &str, service_tier: Option<&str>) -> String {
    with_preference(line, SERVICE_TIER, service_tier)
}

/// Sets a session's goal and starts its first turn (codex-cli 0.166.0).
/// While it is active Codex begins another turn each time one ends short.
const GOAL: &str = "/goal ";
/// Clears the goal; typed mid-turn, that turn finishes and none follows.
const GOAL_CLEAR: &str = "/goal clear";
/// A goal's token budget (uncached input and output, from its setting).
/// Past it Codex stops and marks the goal `budget_limited`.
const GOAL_BUDGET: &str = "goals.max_goal_token_budget";
/// Five times what a Codex session used on a typical task in late 2026
/// (about 207,000): it bounds a session continued round at cost.
pub(super) const GOAL_TOKENS: u64 = 1_000_000;
/// The goal store under the user directory, and the query for the latest
/// goal's status; nothing when there is none or it was cleared.
pub(super) const GOAL_STORE: super::GoalStore = super::GoalStore {
    file: "goals_1.sqlite",
    query: "select status from thread_goals order by updated_at_ms desc limit 1",
};

/// The line that hands a session `brief`: set as its goal.
pub(super) fn brief_line(brief: &str) -> String {
    format!("{GOAL}{}", crate::brief::goal_objective(brief))
}

/// `line` with the goal's budget.
pub(super) fn with_goal_budget(line: &str) -> String {
    Line::new().launch(line).fixed(" --config ").word(&format!("{GOAL_BUDGET}={GOAL_TOKENS}")).done()
}

pub(super) const GOAL_END: super::Keystroke = super::Keystroke { text: Some(GOAL_CLEAR), enter: true };

/// The status of a goal, as the store says it.
pub(super) fn goal(answer: &str) -> Option<super::Goal> {
    use super::Goal;
    Some(match answer.trim() {
        "" => Goal::Unset,
        "active" => Goal::Active,
        "paused" => Goal::Paused,
        "blocked" => Goal::Blocked,
        "usage_limited" => Goal::UsageLimited,
        "budget_limited" => Goal::BudgetLimited,
        "complete" => Goal::Complete,
        _ => return None,
    })
}

/// A rollout's thread, the objective of its first `thread_goal_updated`, and
/// when every `turn_trigger: goal` turn started. The status after the
/// setting is in the store alone.
pub(super) fn goal_session(text: &str) -> super::GoalSession {
    let mut said = super::GoalSession::default();
    for value in json_lines(text) {
        let payload = &value["payload"];
        if value["type"] == "session_meta" && said.thread.is_none() {
            said.thread = payload["id"].as_str().map(str::to_string);
        }
        match payload["type"].as_str() {
            Some("thread_goal_updated") if said.objective.is_none() => {
                said.objective = payload["goal"]["objective"].as_str().map(str::to_string);
            }
            Some("task_started") if payload["turn_attribution"]["turn_trigger"] == "goal" => {
                let at = session::number_at(payload["started_at"].as_u64(), value["timestamp"].as_str());
                said.turns.extend(at);
            }
            _ => {}
        }
    }
    said
}

/// The query for `thread`'s status, tokens used and budget, `|`-separated.
/// A thread id is a UUID; any other is never quoted into the query.
pub(super) fn goal_row_query(thread: &str) -> Option<String> {
    let plain = !thread.is_empty() && thread.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    plain.then(|| format!("select status, tokens_used, token_budget from thread_goals where thread_id = '{thread}'"))
}

/// The row as `sqlite3 -readonly` prints it; nothing for a cleared goal.
pub(super) fn goal_row(answer: &str) -> Option<Option<super::GoalRow>> {
    let answer = answer.trim();
    if answer.is_empty() {
        return Some(None);
    }
    let mut fields = answer.split('|');
    let status = goal(fields.next()?).filter(|status| *status != super::Goal::Unset)?;
    let tokens_used = fields.next()?.trim().parse().ok()?;
    let token_budget = match fields.next()?.trim() {
        "" => None,
        budget => Some(budget.parse().ok()?),
    };
    if fields.next().is_some() {
        return None;
    }
    Some(Some(super::GoalRow { status, tokens_used, token_budget }))
}

/// Whether Codex began a goal turn by itself, after an earlier turn ended.
pub(super) fn goal_continued(text: &str) -> bool {
    let mut ended = false;
    for value in json_lines(text) {
        let payload = &value["payload"];
        match payload["type"].as_str() {
            Some("task_complete" | "turn_aborted") => ended = true,
            Some("task_started") if ended && payload["turn_attribution"]["turn_trigger"] == "goal" => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// An interactive session's line, with search, the model and the effort.
pub(super) fn session_interactive(
    command: &str,
    worktree: &str,
    kept: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> String {
    let command = format!("{command} --search");
    let command = match model {
        Some(model) => Line::new().launch(&command).fixed(" ").fixed(MODEL).fixed(" ").word(model).done(),
        None => command,
    };
    with_effort(&in_worktree(&command, worktree, kept), effort)
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
pub(super) fn added(line: Line, start: &Start) -> Line {
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
    let line = match told(&start.told) {
        Some(told) => line.fixed(" ").fixed(CONFIG).fixed(" ").word(&told),
        None => line,
    };
    let line = isolated(line.fixed(" ").fixed(CONFIG).fixed(" ").word(&added_hooks(start.hook, REPORT)));
    let line = match turned_off(start.shadowed) {
        Some(off) => line.fixed(" ").fixed(CONFIG).fixed(" ").word(&off),
        None => line,
    };
    match start.model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    }
}

/// `line` asking which flags it takes.
pub(super) fn asked_its_flags(line: Line) -> Line {
    line.fixed(HELP)
}

/// The line asking `command` which models it knows, under `kept`.
pub(super) fn models_question(command: &str, kept: &str) -> String {
    Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command)
        .fixed(LIST_MODELS)
        .fixed(MODEL_NAMES)
        .done()
}

/// The line sending `model` the one-line `prompt`, printing the answer:
/// outside any repo, kept nowhere.
pub(super) fn prompt_question(command: &str, kept: &str, model: Option<&str>, prompt: &str) -> String {
    let line = Line::new().fixed("export ").fixed(USER_DIR).fixed("=").word(kept).fixed(" && ").launch(command);
    let line = line.fixed(" exec --skip-git-repo-check --ephemeral");
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(" ").word(prompt).done()
}

/// Events as JSON lines. None names the model; the first names the thread,
/// whose session file does (`answered_as`).
const AS_JSON: &str = " --json";

/// A session file is under `SESSIONS`, named to end with its thread id and
/// `SESSION_ENDS`.
const SESSIONS: &str = "sessions";
const SESSION_ENDS: &str = ".jsonl";

/// As `prompt_question`, printing JSON events and keeping the session, which
/// names the model that answered; the prober deletes it.
pub(super) fn probe_question(command: &str, kept: &str, model: Option<&str>, prompt: &str) -> String {
    let line = Line::new().fixed("export ").fixed(USER_DIR).fixed("=").word(kept).fixed(" && ").launch(command);
    let line = line.fixed(" exec --skip-git-repo-check").fixed(AS_JSON);
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(" ").word(prompt).done()
}

/// What precedes the thread id in events, and the model in a turn context.
const THREAD: &str = "\"thread_id\":\"";
const TURN: &str = "\"type\":\"turn_context\"";
const TURN_MODEL: &str = "\"model\":\"";

/// The session file of the thread `answer` names.
pub(super) fn answered_as(answer: &str) -> super::AnsweredAs {
    let dense: String = answer.split_whitespace().collect();
    let thread = dense.split_once(THREAD).and_then(|(_, after)| after.split_once('"'));
    match thread {
        Some((thread, _)) if !thread.is_empty() => {
            super::AnsweredAs::InSession { under: SESSIONS, ends: format!("{thread}{SESSION_ENDS}") }
        }
        _ => super::AnsweredAs::Unsaid,
    }
}

/// The model of the first turn `session`, a session file of Codex's, holds.
pub(super) fn answered_in_session(session: &str) -> Option<String> {
    let dense: String = session.split_whitespace().collect();
    let (_, turn) = dense.split_once(TURN)?;
    let (_, after) = turn.split_once(TURN_MODEL)?;
    let (model, _) = after.split_once('"')?;
    (!model.is_empty()).then(|| model.to_string())
}

/// Each model's `slug` in `answer`, as `MODEL` takes it, hidden ones
/// included; `None` when there are none.
pub(super) fn models(answer: &str) -> Option<Vec<String>> {
    let dense: String = answer.split_whitespace().collect();
    let names = dense.split(NAMED).skip(1).filter_map(|after| after.split_once('"'));
    let listed: Vec<String> = names.map(|(name, _)| name.to_string()).filter(|name| !name.is_empty()).collect();
    if listed.is_empty() { None } else { Some(listed) }
}

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
pub(super) fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.tool_input.command {
        Some(line) if described.tool_name == SHELL_TOOL => Call::Shell(line),
        _ => Call::Other,
    })
}

/// The harness directory under a user's home.
pub(super) const HOME: &str = ".codex";

/// The note beside a worktree's git directory naming the home kept for it.
pub(super) const HOME_NOTE: &str = "factorium-codex-home";

/// Every JSONL file under [`SESSIONS`]; each file says its worktree.
pub(super) fn session_layout(_cwd: &str) -> SessionLayout {
    SessionLayout::Tree { dir: SESSIONS.into(), extension: "jsonl" }
}

/// The turn `text` records for `cwd`, or for the session handed `brief`
/// when it started elsewhere. A header is required.
pub(super) fn recorded_turn(cwd: &str, text: &str, not_before: Option<u64>, brief: Option<&str>) -> Option<Turn> {
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
    session::turn_of(&notes, not_before)
}

/// Whether the first brief pointer in the opening user messages (after any
/// AGENTS.md and environment messages) is `brief`'s. A later mention, tool
/// output or reply does not count.
pub(super) fn names_brief(text: &str, brief: &str) -> bool {
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

/// The stand-in session file; it replaces one of the same name.
pub(super) fn session_files(cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
    let iso = session::format_iso(at);
    let name = format!("rollout-factorium-{}.jsonl", session::mark(cwd));
    let (year, month, day) = session::civil_from_days((at / 86_400) as i64);
    let meta = json!({"cwd": cwd, "id": "factorium"});
    let mut body = format!("{}\n", json!({"timestamp": iso, "type": "session_meta", "payload": meta}));
    let started = json!({"type": "task_started", "started_at": at, "turn_id": "factorium"});
    body += &format!("{}\n", json!({"timestamp": iso, "type": "event_msg", "payload": started}));
    if !open {
        let done = json!({"type": "task_complete", "completed_at": at, "turn_id": "factorium"});
        body += &format!("{}\n", json!({"timestamp": iso, "type": "event_msg", "payload": done}));
    }
    vec![SessionFile {
        path: format!("{SESSIONS}/{year:04}/{month:02}/{day:02}/{name}"),
        body,
        compressed: false,
        replaces: Some(name),
    }]
}

/// Codex's token count keys. Its input includes the cached input.
const USAGE_KEYS: Keys = Keys {
    input: "input_tokens",
    cache_read: "cached_input_tokens",
    cache_write: "cache_write_input_tokens",
    output: "output_tokens",
    reasoning: "reasoning_output_tokens",
};

/// What one Codex session line says of usage: a turn's context names the
/// model, and a token count is the session's running total.
pub(super) fn usage(value: &serde_json::Value, _line: usize) -> Option<Usage> {
    if value["type"] == "turn_context" {
        return value["payload"]["model"].as_str().map(|model| Usage::Model(model.to_string()));
    }
    if value["type"] != "event_msg" || value["payload"]["type"] != "token_count" {
        return None;
    }
    let usage = &value["payload"]["info"]["total_token_usage"];
    if !usage.is_object() {
        return None;
    }
    Some(Counts::read(usage, &USAGE_KEYS, true).map_or(Usage::Uncounted, Usage::Running))
}

/// The `primary` and `secondary` windows of a `token_count` line's
/// `rate_limits`. Older Codex said `resets_in_seconds` from the `timestamp`.
pub(super) fn windows(value: &serde_json::Value) -> Vec<Window> {
    let Some(limits) = rate_limits(value) else { return Vec::new() };
    WINDOWS
        .into_iter()
        .filter_map(|name| {
            let window = &limits[name];
            Some(Window {
                name: name.to_string(),
                used_percent: window::number(&window["used_percent"])?,
                minutes: window["window_minutes"].as_u64(),
                resets_at: window::seconds(&window["resets_at"]).or_else(|| {
                    let after = window["resets_in_seconds"].as_u64()?;
                    Some(window::written_at(value)?.saturating_add(after))
                }),
            })
        })
        .collect()
}

/// The limit a line says it met: a `token_count` line naming
/// `rate_limit_reached_type` (the spent window and its reset), or a live
/// `error` event's `codex_error_info`, usage or throttle.
pub(super) fn limit_event(value: &serde_json::Value) -> Option<Limit> {
    if let Some(limits) = rate_limits(value) {
        limits["rate_limit_reached_type"].as_str()?;
        let spent = windows(value).into_iter().find(|window| window.used_percent >= 100.0);
        return Some(Limit {
            kind: Limited::Usage,
            resets_at: spent.as_ref().and_then(|window| window.resets_at),
            window: spent.map(|window| window.name),
        });
    }
    if value["type"] != "event_msg" || value["payload"]["type"] != "error" {
        return None;
    }
    let kind = match value["payload"]["codex_error_info"].as_str()? {
        "usage_limit_exceeded" => Limited::Usage,
        "rate_limit_exceeded" => Limited::Throttle,
        _ => return None,
    };
    Some(Limit { kind, window: None, resets_at: None })
}

/// The app server's `account/rateLimits/read` (codex-cli 0.169.0): no turn
/// spent, the login refreshed. The server quits at end of input before
/// answering, so a fifo holds its input open until answer 1 comes.
pub(super) const WINDOWS_QUESTION: super::WindowsQuestion = super::WindowsQuestion {
    line: r#"d=$(mktemp -d) && mkfifo "$d/in" && codex app-server <"$d/in" 2>/dev/null | (exec 3>"$d/in"; printf '%s\n' '{"id":0,"method":"initialize","params":{"clientInfo":{"name":"factorium","version":"0"}}}' '{"method":"initialized"}' '{"id":1,"method":"account/rateLimits/read","params":{"excludeResetCreditDetails":true}}' >&3; sed -n '/^{.*"id":1[,}]/{p;q;}'); rm -rf "$d""#,
    spends_turn: false,
};

/// The `primary` and `secondary` windows of the app server's `rateLimits`.
pub(super) fn windows_answer(printed: &str) -> Vec<Window> {
    let Some(limits) = printed
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|answer| answer["id"] == 1)
        .map(|answer| answer["result"]["rateLimits"].clone())
    else {
        return Vec::new();
    };
    WINDOWS
        .into_iter()
        .filter_map(|name| {
            let window = &limits[name];
            Some(Window {
                name: name.to_string(),
                used_percent: window::number(&window["usedPercent"])?,
                minutes: window["windowDurationMins"].as_u64(),
                resets_at: window::seconds(&window["resetsAt"]),
            })
        })
        .collect()
}

/// What Codex calls the windows of its `rate_limits`.
const WINDOWS: [&str; 2] = ["primary", "secondary"];

/// The `rate_limits` of a `token_count` line, the line [`usage`] reads.
fn rate_limits(value: &serde_json::Value) -> Option<&serde_json::Value> {
    if value["type"] != "event_msg" || value["payload"]["type"] != "token_count" {
        return None;
    }
    let limits = &value["payload"]["rate_limits"];
    limits.is_object().then_some(limits)
}

/// The hook events that say Codex's status. A turn ending in an error fires
/// no `Stop`, only `PostModelRequest`; an interrupted one `Interrupt`.
pub(super) const STATUS_EVENTS: [&str; 9] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "PostModelRequest",
    "Interrupt",
    "SessionEnd",
];

/// What of a tool's input names its call: the shell line, or the patch.
const CALL_KEYS: [&str; 2] = ["command", "description"];
/// The status `PostModelRequest` gives a request that failed.
const FAILED: &str = "failed";

/// What a hook `payload` says of the status. `PostToolUse` carries no exit
/// code, so every call ends with no error; a failed tool fires none.
pub(super) fn hook_status(payload: &serde_json::Value) -> Reading {
    let text = |key: &str| payload[key].as_str().and_then(status::one_line);
    let tool = payload["tool_name"].as_str().map(str::to_string);
    let call = |status: Status, step: Option<CallEnd>| {
        let message = tool.as_deref().map(|name| status::call_line(name, &payload["tool_input"], &CALL_KEYS));
        Reading::Is(Report { status, message, tool: tool.clone(), end: step })
    };
    match payload["hook_event_name"].as_str().unwrap_or_default() {
        "SessionStart" | "SessionEnd" | "Interrupt" => Reading::status(Status::Idle),
        "UserPromptSubmit" => Reading::told(Status::Working, text("prompt")),
        "PreToolUse" => call(Status::Working, Some(CallEnd::Start)),
        "PermissionRequest" => call(Status::Blocked(Blocked::Permission), None),
        "PostToolUse" => call(Status::Working, Some(CallEnd::Result { error: None })),
        "Stop" => Reading::told(Status::Done, text("last_assistant_message")),
        "PostModelRequest" if payload["status"].as_str() == Some(FAILED) => Reading::told(Status::Error, text("error")),
        _ => Reading::Unchanged,
    }
}

/// A rollout line's tool call or result, by `call_id`. An error is a nonzero
/// exit in the output text (or the older `metadata.exit_code`). A spawned
/// sub-agent's `session_meta` marks a child session.
pub(super) fn said(value: &serde_json::Value) -> Vec<Said> {
    let payload = &value["payload"];
    if value["type"] == "session_meta" {
        let spawned = payload["source"]["subagent"].is_object();
        return if spawned { vec![Said::Child] } else { Vec::new() };
    }
    if value["type"] != "response_item" {
        return Vec::new();
    }
    let Some(id) = payload["call_id"].as_str().filter(|id| !id.is_empty()) else {
        return Vec::new();
    };
    let id = id.to_string();
    let name = payload["name"].as_str().unwrap_or("");
    let said = match payload["type"].as_str() {
        Some("function_call") => Said::Call { id, action: steps::action(name, &payload["arguments"]) },
        Some("custom_tool_call") => Said::Call { id, action: steps::action(name, &payload["input"]) },
        Some("local_shell_call") => {
            Said::Call { id, action: steps::action("local_shell", &payload["action"]["command"]) }
        }
        Some("function_call_output" | "custom_tool_call_output") => {
            let (result, error) = call_output(&payload["output"]);
            let at = value["timestamp"].as_str().and_then(session::parse_iso);
            Said::Result { id, result, error, at }
        }
        _ => return Vec::new(),
    };
    vec![said]
}

/// A call's `output` on one line, and whether it exited nonzero.
fn call_output(output: &serde_json::Value) -> (String, bool) {
    if let Some(text) = output.as_str()
        && let Ok(object) = serde_json::from_str::<serde_json::Value>(text)
        && object["output"].is_string()
    {
        let code = object["metadata"]["exit_code"].as_i64();
        return (steps::content(&object["output"]), code.is_some_and(|code| code != 0));
    }
    let result = steps::content(output);
    let text = match output {
        serde_json::Value::String(text) => text.clone(),
        other => steps::content(other),
    };
    let code = steps::exit_code(&text, "Process exited with code ")
        .or_else(|| text.starts_with("Exit code: ").then(|| steps::exit_code(&text, "Exit code: "))?);
    (result, code.is_some_and(|code| code != 0))
}
