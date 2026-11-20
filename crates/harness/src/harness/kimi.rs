//! Kimi Code, as a harness: its isolated data home carries only the host's
//! providers and models, the guard and the report, and where its rules are.
//! Kimi has no project-level hook configuration, so its home's configuration
//! is generated at each start, and is the only way it gets the guard.

use serde::Deserializer;

use super::{Dialog, Kind, Link, Login, Stop};
use super::session::{self, json_lines};
use super::store::Layout;
use crate::harness::{Call, Harness};
use crate::line::Line;
use crate::start::{Start, TOLD};
use crate::turn::{Marker, Note, Turn};
use toml::Value as Toml;

const ADD_DIR: &str = "--add-dir";
const SKILLS_DIR: &str = "--skills-dir";
const MODEL: &str = "--model";

const USER_DIR: &str = "KIMI_CODE_HOME";
const ENVIRONMENT: [&str; 3] =
    ["KIMI_CODE_TRUST_WORKSPACE", "KIMI_CODE_NO_AUTO_UPDATE", "KIMI_DISABLE_TELEMETRY"];

const KEPT_LINKS: &[Link] = &[Link { home: ".kimi-code/credentials", kept: "credentials" }];

/// The file whose being in its user directory answers the question Kimi
/// Code opens a fresh one with: whether to migrate kimi-cli's configuration.
const SKIP_MIGRATION: &str = ".skip-migration-from-kimi-cli";
/// The host user's Kimi config, of which a session keeps only model routing.
const USER_CONFIG: &str = ".kimi-code/config.toml";
/// Its configuration, and the system prompt it is told, in its user directory.
const CONFIG: &str = "config.toml";
const SYSTEM: &str = "SYSTEM.md";

/// The configuration a session gets: of the host user's `user` only model
/// routing, then the tool's settings, the guard `guard` and the report.
fn configuration(user: Option<&str>, guard: &str) -> Result<String, String> {
    let mut config = match user {
        Some(text) if !text.trim().is_empty() => {
            text.parse::<Toml>().map_err(|error| format!("the host user's Kimi config: {error}"))?
        }
        _ => Toml::Table(Default::default()),
    };
    let table = config.as_table_mut().ok_or("the host user's Kimi config is not a table")?;
    table.retain(|key, _| matches!(key, "default_model" | "providers" | "models"));
    table.insert("default_permission_mode".into(), Toml::from("auto"));
    table.insert("default_plan_mode".into(), Toml::from(false));
    table.insert("auto_session_title".into(), Toml::from(false));
    table.insert("builtin_product_skills".into(), Toml::from(false));
    let mut hooks = vec![hook(BEFORE_A_CALL, guard, None)];
    for event in STATE_EVENTS {
        hooks.push(hook(event, REPORT, Some(10)));
    }
    table.insert("hooks".into(), Toml::Array(hooks));
    toml::to_string_pretty(&config).map_err(|error| error.to_string())
}

/// The event of Kimi's before each tool call: where the guard is asked.
const BEFORE_A_CALL: &str = "PreToolUse";

fn hook(event: &str, command: &str, timeout: Option<i64>) -> Toml {
    let mut hook = toml::map::Map::new();
    hook.insert("event".into(), Toml::from(event));
    hook.insert("command".into(), Toml::from(command));
    if let Some(timeout) = timeout {
        hook.insert("timeout".into(), Toml::from(timeout));
    }
    Toml::Table(hook)
}

/// The events at which a session's state changes, besides `PreToolUse`:
/// each runs `REPORT`.
const STATE_EVENTS: [&str; 17] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "TurnStarted",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionResult",
    "SubagentStart",
    "SubagentStop",
    "TaskStarted",
    "Stop",
    "StopFailure",
    "Interrupt",
    "PreCompact",
    "PostCompact",
    "Notification",
];

/// Tells Orca of the session's state through the hook Orca installs for
/// Kimi, where it did; prints nothing and succeeds.
const REPORT: &str = "hook=\"${HOME-}/.orca/agent-hooks/kimi-hook.sh\"; if [ -x \"$hook\" ]; \
     then /bin/sh \"$hook\"; else cat; fi >/dev/null 2>&1; exit 0";

/// Kimi Code's own usage-limit error.
/// The provider prefix is part of the words: a bare "5-hour usage limit"
/// on a screen is not this quota.
const USAGE_LIMIT: &str = "[provider.api_error] 429 You've reached your 5-hour usage limit";

/// Kimi Code's account-quota screen. The error may wrap; a terminal's
/// line break is whitespace. The same words on another harness are not it.
fn account_quota(output: &[String]) -> Option<String> {
    let flat = output.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    if !flat.contains(USAGE_LIMIT) {
        return None;
    }
    Some(
        output
            .iter()
            .find(|line| line.split_whitespace().collect::<Vec<_>>().join(" ").contains("5-hour usage limit"))
            .cloned()
            .unwrap_or_else(|| USAGE_LIMIT.to_string()),
    )
}

fn note(value: &serde_json::Value) -> Option<Note> {
    let at = session::epoch_number(value["time"].as_u64()?);
    match value["type"].as_str()? {
        "agent.turn.started" => Some(Note { marker: Marker::Start, at, child: false }),
        "agent.turn.ended" | "turn.ended" => Some(Note { marker: Marker::End, at, child: false }),
        _ => None,
    }
}

/// The index `existing` with this worktree's line replaced by one naming
/// its session directory `dir`. Lines for other worktrees, and a line that
/// is not JSON, stay.
fn session_index(cwd: &str, existing: &str, dir: &str) -> String {
    let mut kept = String::new();
    for line in existing.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            kept.push_str(line);
            kept.push('\n');
            continue;
        };
        if value["workDir"].as_str().is_some_and(|dir| session::same_cwd(dir, cwd)) {
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept.push_str(
        &session::object(vec![
            ("sessionId", session::mark(cwd).into()),
            ("sessionDir", dir.into()),
            ("workDir", cwd.into()),
        ])
        .to_string(),
    );
    kept.push('\n');
    kept
}

/// The session directories `index` names for the worktree `cwd`: absolute,
/// or relative to the harness directory.
fn session_dirs(index: &str, cwd: &str) -> Vec<String> {
    json_lines(index)
        .filter(|value| value["workDir"].as_str().is_some_and(|dir| session::same_cwd(dir, cwd)))
        .filter_map(|value| value["sessionDir"].as_str().filter(|dir| !dir.is_empty()).map(str::to_string))
        .collect()
}

/// Kimi Code, as a harness: what its module says of it.
pub(super) struct Kimi;

impl Kind for Kimi {
    const KEY: &'static str = "kimi";
    const LOGIN: Login = Login::Machine { user_dir: USER_DIR };
    const USER_HOME: Option<&'static str> = None;
    const KEPT_LINKS: &'static [Link] = KEPT_LINKS;
    /// No approval prompts.
    const STOCK_ARGUMENTS: &'static str = "--auto";
    const ASKS_FIRST: Option<&'static str> = None;
    /// Its footer, drawn once its screen is up (Kimi Code 2.5.3): `context: 0%
    /// (0/1M)`. Typed before, while it starts, a line is lost (#39's first run:
    /// the brief, typed 1.2s after its start, never reached it).
    const SHOWS_UP: Option<&'static str> = Some("context: ");
    const ASKS_MIDWAY: &'static [Dialog] = &[];
    const ENVIRONMENT: &'static [&'static str] = &ENVIRONMENT;
    const REFUSED: u8 = 2;
    /// Escape.
    const INTERRUPT: &'static str = "\x1b";
    /// Not asked headless: no run of it so has been read here.
    const HEADLESS: Option<crate::ask::Headless> = None;

    /// What the host keeps for Kimi beside the links: the migration declined,
    /// the configuration, and its own prompt (`${base_prompt}`) then where its
    /// rules are: it starts in a directory whose rules file it does not read,
    /// and works in another.
    fn kept(start: &Start) -> Result<Vec<(&'static str, String)>, String> {
        let user = std::fs::read_to_string(start.user_home.join(USER_CONFIG)).ok();
        let config = configuration(user.as_deref(), &start.hook_asked(Harness::Kimi))?;
        Ok(vec![(SKIP_MIGRATION, String::new()), (CONFIG, config), (SYSTEM, format!("${{base_prompt}}\n\n{TOLD}"))])
    }

    const TOLD_IN: Option<&'static str> = Some(SYSTEM);

    /// The guard lines its kept configuration under `home` runs before a call.
    fn guards(home: &std::path::Path) -> Option<Vec<String>> {
        let config = std::fs::read_to_string(home.join(CONFIG)).ok().and_then(|text| text.parse::<Toml>().ok());
        let hooks = config.as_ref().and_then(|config| config.get("hooks")).and_then(Toml::as_array);
        let before = hooks.into_iter().flatten().filter(|hook| hook.get("event").and_then(Toml::as_str) == Some(BEFORE_A_CALL));
        Some(before.filter_map(|hook| hook.get("command")?.as_str().map(String::from)).collect())
    }

    fn start_dir<'a>(start: &Start<'a>) -> &'a str {
        start.dir
    }

    /// Kimi's home, and where its rules are (`TOLD`).
    fn environment(line: Line, start: &Start) -> Line {
        start.rules_env(line.set(USER_DIR, start.home.dir()))
    }

    fn added(line: Line, start: &Start) -> Line {
        let skills = format!("{}/{}", start.dir, crate::start::SKILLS);
        let line = line.flag(ADD_DIR, start.worktree).flag(ADD_DIR, start.dir).flag(SKILLS_DIR, &skills);
        line.flag_if(MODEL, start.model)
    }

    fn briefed(line: String, brief: &str) -> (String, Option<String>) {
        (line, Some(crate::brief::brief_pointer(brief)))
    }

    fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
        super::tool_call(described, "Bash")
    }

    fn stopped(screen: &[String]) -> Option<Stop> {
        account_quota(screen).map(Stop::Quota)
    }

    /// Where Kimi keeps the sessions of every worktree: one index names each
    /// worktree's session directories, and each holds its wire.
    fn layout(_cwd: &str) -> Layout {
        Layout::Index {
            index: "session_index.jsonl",
            name: "agents/main/wire.jsonl",
            dirs: session_dirs,
            with: session_index,
        }
    }

    fn could_record(_cwd: &str, _text: &str, _brief: &str) -> bool {
        true
    }

    /// The turn `text`, one session's wire, records. The worktree was named by
    /// the index that pointed here. Kimi records no child session. The wire's
    /// other lines are huge and are not a turn: the type is the whole match, so
    /// a prompt that mentions a turn is not one.
    fn recorded_turn(_cwd: &str, text: &str, _brief: &str) -> Option<Turn> {
        let notes: Vec<Note> = json_lines(text).filter_map(|value| note(&value)).collect();
        crate::turn::latest(&notes)
    }

    /// The wire a stand-in writes for this turn.
    fn stand_in(_cwd: &str, _brief: &str, open: bool, at: u64) -> String {
        let ms = at.saturating_mul(1000);
        let mut body = format!(
            "{}\n",
            session::object(vec![
                ("type", "agent.turn.started".into()),
                ("time", ms.into()),
                ("turnId", 0_u64.into()),
                ("kind", "event".into()),
            ])
        );
        if !open {
            body.push_str(&format!(
                "{}\n",
                session::object(vec![
                    ("type", "agent.turn.ended".into()),
                    ("time", ms.into()),
                    ("turnId", 0_u64.into()),
                    ("outcome", "completed".into()),
                ])
            ));
        }
        body
    }

    fn prompted(_text: &str, _said: &str, _since: u64) -> Option<bool> {
        None
    }
}
