//! Antigravity CLI (`agy`), as a harness. agy reads its configuration, its
//! hooks, its rules and its login from `~/.gemini`, and no variable but
//! `HOME` points it elsewhere (agy 1.5.14: `ANTIGRAVITY_GEMINI_DIR`,
//! `ANTIGRAVITY_APP_DATA_DIR` and `JETSKI_APP_DATA_DIR` are not read), so a
//! session runs with `HOME` its kept home: there the host user's login is
//! linked, and the guard, Orca's report, where its rules are and onboarding
//! done are generated at each start; none of the host user's hooks, settings,
//! rules or skills is there. The shell its commands run in is given the host
//! user's `HOME` back by the kept `.zshenv`, so git, cargo and factorium find
//! theirs, its session's `PATH` still first. A session works in the
//! worktree, its start directory not added: agy runs every command in a
//! directory added with `--add-dir`, in either order (agy 1.5.14), and the
//! session reads its rules there by their path.

use serde::de::Error;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::session::{self, json_lines};
use super::store::Layout;
use super::{Dialog, Kind, Link, Login, Stop};
use crate::ask::Asked;
use crate::harness::{Call, Harness};
use crate::line::{Line, quoted};
use crate::start::{Start, TOLD};
use crate::turn::{Marker, Note, Turn};

const MODEL: &str = "--model";
/// Settings for this session only, never saved (`agy --help`).
const SETTINGS: &str = "--settings";
/// Runs its first prompt and goes on interactively.
const PROMPT_INTERACTIVE: &str = "-i";

/// The only variable that moves where agy keeps its files.
const USER_DIR: &str = "HOME";
const ENVIRONMENT: [&str; 1] = ["AGY_CLI_DISABLE_AUTO_UPDATE"];

/// Its login: the token it refreshes in its own directory.
const KEPT_LINKS: &[Link] = &[Link {
    home: ".gemini/antigravity-cli/antigravity-oauth-token",
    kept: ".gemini/antigravity-cli/antigravity-oauth-token",
}];

/// Its global hooks, the rules it reads from its home, and what says its
/// first-run screens (a color scheme, a sign-in) were gone through.
const HOOKS: &str = ".gemini/config/hooks.json";
const RULES_FILE: &str = ".gemini/GEMINI.md";
const ONBOARDING: &str = ".gemini/antigravity-cli/cache/onboarding.json";
/// What the shell its commands run in reads first, as `HOME` stands.
const ZSHENV: &str = ".zshenv";

/// The event of agy's before each tool call: where the guard is asked.
const BEFORE_A_CALL: &str = "PreToolUse";
/// Seconds agy waits on one hook; one that runs out fails the call.
const HOOK_TIMEOUT: u64 = 10;
/// What agy blocks a call on: printed by its `PreToolUse` hook, `%s` the
/// reason. A hook that prints nothing lets it by; `{}` blocks it too, with
/// no reason (agy 1.5.14).
const DENY: &str = r#"{"decision":"deny","reason":"%s"}"#;

/// The events Orca reports a session's state at, each with what agy is to
/// read from a hook that has nothing to say, and whether it is one of a
/// tool's (matched to every tool). As Orca writes them into the host user's
/// own `hooks.json`.
const STATE_EVENTS: [(&str, &str, bool); 5] = [
    ("PreInvocation", "{}", false),
    ("PostInvocation", "{}", false),
    ("Stop", r#"{"decision":""}"#, false),
    (BEFORE_A_CALL, "", true),
    ("PostToolUse", "{}", true),
];

/// Tells Orca of the session's state at `event` through the hook Orca
/// installs for agy under the host user's home `user_home`, where it did and
/// Orca started the session, run with that home; else drains the payload
/// and prints `quiet`. It succeeds either way.
fn report(user_home: &str, event: &str, quiet: &str) -> String {
    let hook = quoted(&format!("{user_home}/.orca/agent-hooks/antigravity-hook.sh"));
    let said = if quiet.is_empty() { String::new() } else { format!(" printf '%s\\n' {};", quoted(quiet)) };
    format!(
        "hook={hook}; if [ -n \"${{ORCA_PANE_KEY-}}${{ORCA_AGENT_PANE-}}\" ] && [ -x \"$hook\" ]; \
         then HOME={home} ORCA_HOOK_AGENT_PID=\"$PPID\" ORCA_ANTIGRAVITY_EVENT={event} /bin/sh \"$hook\"; \
         else cat >/dev/null;{said} fi; exit 0",
        home = quoted(user_home),
    )
}

/// The hooks a session gets: the guard `guard` before each tool call, and
/// Orca's report at each of its events.
fn hooks(user_home: &str, guard: &str) -> String {
    let command = |command: &str| json!({ "type": "command", "command": command, "timeout": HOOK_TIMEOUT });
    let mut events = serde_json::Map::new();
    for (event, quiet, of_a_tool) in STATE_EVENTS {
        let mut commands = Vec::new();
        if event == BEFORE_A_CALL {
            commands.push(command(guard));
        }
        commands.push(command(&report(user_home, event, quiet)));
        let entries = if of_a_tool { json!([{ "matcher": "*", "hooks": commands }]) } else { json!(commands) };
        events.insert(event.into(), entries);
    }
    let all = json!({ "factorium": events });
    serde_json::to_string_pretty(&all).unwrap_or_default() + "\n"
}

/// The shell's first file: the host user's home back, then theirs, then
/// the session's `PATH` before whatever theirs put first, so the factorium
/// it runs by name is the one that started it (a `.zshenv` that puts
/// Homebrew first would find an installed one).
fn zshenv(user_home: &str) -> String {
    format!(
        "factorium_path=$PATH\nexport HOME={}\n[ -f \"$HOME/.zshenv\" ] && . \"$HOME/.zshenv\"\n\
         export PATH=\"$factorium_path:$PATH\"\nunset factorium_path\n",
        quoted(user_home)
    )
}

/// One tool call, as agy hands it to its `PreToolUse` hook (agy 1.5.14):
/// the call under `toolCall`, beside the session's own fields.
#[derive(Deserialize)]
struct Described {
    #[serde(rename = "toolCall")]
    tool_call: ToolCall,
}

#[derive(Deserialize)]
struct ToolCall {
    name: String,
    #[serde(default)]
    args: Value,
}

/// The tool agy runs shell lines with, and its argument holding the line.
const SHELL_TOOL: &str = "run_command";
const SHELL_LINE: &str = "CommandLine";

/// What a user's input says, `content` being the whole of it as the
/// transcript writes it: agy wraps the words typed in `<USER_REQUEST>`.
fn request(content: &str) -> &str {
    let inner = content.split_once("<USER_REQUEST>").map_or(content, |(_, rest)| rest);
    let inner = inner.split_once("</USER_REQUEST>").map_or(inner, |(said, _)| said);
    inner.trim()
}

/// Whether the transcript line `value` is a user's input.
fn user_input(value: &Value) -> bool {
    value["type"] == "USER_INPUT"
}

/// Whether the transcript line `value` is the model's answer.
fn planner(value: &Value) -> bool {
    value["type"] == "PLANNER_RESPONSE"
}

fn note(value: &Value) -> Option<Note> {
    let at = session::parse_iso(value["created_at"].as_str()?)?;
    if user_input(value) {
        return Some(Note { marker: Marker::Start, at, child: false });
    }
    let calls = value["tool_calls"].as_array().is_some_and(|calls| !calls.is_empty());
    (planner(value) && !calls && value["status"] == "DONE").then_some(Note { marker: Marker::End, at, child: false })
}

/// The agent a session asked headless runs as: none of agy's tools, none
/// of its default prompt's sections; its user directory is a fresh one
/// (`ask_home`) holding only its login and this agent, so no hook, rule,
/// skill, plugin or MCP server of the host user's.
const ASK_AGENT_FILE: &str = ".gemini/config/agents/factorium-ask.md";
const ASK_AGENT: &str = "---\nname: factorium-ask\ndescription: Answers one question, with no tools.\n\
                         tools: []\nexcludeDefaultComponents: true\n---\n\
                         You have no tools. Answer the question in this one reply, as it asks, and nothing else.\n";

/// agy asked one question headless (read in agy 1.5.14): with stdin not a
/// terminal and no `-p`, it reads its prompt there (`-p` takes the prompt
/// as its value, and an empty one is refused), answers one JSON object, and
/// expands no slash command. It runs as [`ASK_AGENT`], which has no tools,
/// and every kind of call its permission rules name is denied besides.
const HEADLESS: crate::ask::Headless = crate::ask::Headless {
    flags: "--output-format json --disable-slash-commands --agent factorium-ask \
            --settings '{\"permissions\":{\"deny\":[\"command(*)\",\"read_file(*)\",\"write_file(*)\",\
            \"read_url(*)\",\"execute_url(*)\",\"mcp(*)\"]}}'",
    model: MODEL,
    again: Some("--conversation"),
    files: asked_files,
    read: asked,
    stand_in: asked_stand_in,
};

fn asked_files() -> Result<crate::ask::Files, String> {
    Ok(vec![(ASK_AGENT_FILE, ASK_AGENT.into())])
}

/// What `agy --output-format json` printed: its answer in `response`,
/// unless `status` is not `SUCCESS` (then `error` says why); the
/// conversation in `conversation_id`; the tokens of every turn in `usage`,
/// thinking among the output. It names no model: the kit's, else
/// `default`, as the worker's id says one.
fn asked(out: &str, model: Option<&str>) -> Result<Asked, String> {
    let said: Value = serde_json::from_str(out.trim()).map_err(|e| format!("agy answered no JSON ({e}): {out}"))?;
    if said["status"] != "SUCCESS" {
        return Err(format!("agy failed: {}", said["error"].as_str().unwrap_or(out.trim())));
    }
    let text = said["response"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| format!("agy answered nothing: {out}"))?;
    let session = said["conversation_id"].as_str().filter(|s| !s.is_empty()).ok_or("agy named no conversation_id")?;
    let n = |field: &str| said["usage"][field].as_u64().unwrap_or(0);
    let (tokens_in, tokens_out) = (n("input_tokens") + n("cache_read_tokens"), n("output_tokens"));
    let model = model.unwrap_or("default").to_string();
    Ok(Asked { session: session.into(), model, text: text.into(), tokens_in, tokens_out })
}

fn asked_stand_in(a: &Asked) -> String {
    let usage = json!({ "input_tokens": a.tokens_in, "output_tokens": a.tokens_out, "thinking_tokens": 0,
                        "cache_read_tokens": 0, "total_tokens": a.tokens_in + a.tokens_out });
    let said = json!({ "conversation_id": a.session, "status": "SUCCESS", "response": a.text,
                       "duration_seconds": 1.0, "num_turns": 1, "usage": usage });
    format!("{said}\n")
}

/// Antigravity CLI, as a harness: what its module says of it.
pub(super) struct Agy;

impl Kind for Agy {
    const KEY: &'static str = "agy";
    const LOGIN: Login = Login::Machine { user_dir: USER_DIR };
    const USER_HOME: Option<&'static str> = None;
    const KEPT_LINKS: &'static [Link] = KEPT_LINKS;
    /// No permission prompts: every tool call is approved, the guard aside.
    const PROGRAM: &'static str = "agy";
    const STOCK_ARGUMENTS: &'static str = "--dangerously-skip-permissions";
    const ASKS_FIRST: Option<&'static str> = None;
    /// Its brief is on its line (`-i`): nothing is typed before it is up.
    const SHOWS_UP: Option<&'static str> = None;
    const ASKS_MIDWAY: &'static [Dialog] = &[];
    const ENVIRONMENT: &'static [&'static str] = &ENVIRONMENT;
    /// What the guard exits with; the hook prints agy's refusal (`DENY`).
    const REFUSED: u8 = 2;
    /// Escape.
    const INTERRUPT: &'static str = "\x1b";
    const HEADLESS: Option<crate::ask::Headless> = Some(HEADLESS);

    /// What the host keeps for agy beside its login: the hooks, where its
    /// rules are, its onboarding done, and the shell's way back to the host
    /// user's home.
    fn kept(start: &Start) -> Result<Vec<(&'static str, String)>, String> {
        let user_home = start.user_home.to_string_lossy();
        let guard = crate::line::guard_printing(start.guard, Harness::Agy.key(), DENY);
        let onboarded = json!({ "consumerOnboardingComplete": true, "enterpriseOnboardingComplete": false,
                                "onboardingComplete": true });
        Ok(vec![
            (HOOKS, hooks(&user_home, &guard)),
            (RULES_FILE, TOLD.into()),
            (ONBOARDING, format!("{onboarded}\n")),
            (ZSHENV, zshenv(&user_home)),
        ])
    }

    const TOLD_IN: Option<&'static str> = Some(RULES_FILE);

    /// The guard lines its kept hooks under `home` run before a call.
    fn guards(home: &std::path::Path) -> Option<Vec<String>> {
        let text = std::fs::read_to_string(home.join(HOOKS)).unwrap_or_default();
        let hooks: Value = serde_json::from_str(&text).unwrap_or_default();
        let groups = hooks.as_object().into_iter().flat_map(|names| names.values());
        let before = groups.filter_map(|group| group[BEFORE_A_CALL].as_array()).flatten();
        let commands = before.filter_map(|entry| entry["hooks"].as_array()).flatten();
        Some(commands.filter_map(|hook| hook["command"].as_str().map(String::from)).collect())
    }

    /// Its commands run where it starts: the worktree.
    fn start_dir<'a>(start: &Start<'a>) -> &'a str {
        start.worktree
    }

    /// Its home, and where its rules are (`TOLD`).
    fn environment(line: Line, start: &Start) -> Line {
        start.rules_env(line.set(USER_DIR, start.home.dir()))
    }

    /// Both directories trusted for this session alone (else it asks
    /// whether to trust the worktree before it starts), and the caller's
    /// model. No directory added: agy would run its commands there.
    fn added(line: Line, start: &Start) -> Line {
        let trusted = json!({ "trustedWorkspaces": [start.worktree, start.dir] }).to_string();
        line.flag(SETTINGS, &trusted).flag_if(MODEL, start.model)
    }

    /// Its brief is its first prompt, on its line.
    fn briefed(line: String, brief: &str) -> (String, Option<String>) {
        let pointer = crate::brief::brief_pointer(brief);
        (Line::new().launch(&line).flag(PROMPT_INTERACTIVE, &pointer).done(), None)
    }

    /// A shell call is `run_command` with its `CommandLine`; one without is
    /// not read.
    fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
        let call = Described::deserialize(described)?.tool_call;
        if call.name != SHELL_TOOL {
            return Ok(Call::NotShell);
        }
        match &call.args[SHELL_LINE] {
            Value::String(line) => Ok(Call::Shell(line.clone())),
            _ => Err(D::Error::custom(format!("a `{SHELL_TOOL}` call with no `{SHELL_LINE}`"))),
        }
    }

    fn stopped(screen: &[String]) -> Option<Stop> {
        quota(screen)
    }

    /// Where agy keeps every session, of every worktree: one directory a
    /// conversation, its transcript in it (`transcript_full.jsonl` beside it
    /// says the same at length, and is not read).
    fn layout(_cwd: &str) -> Layout {
        Layout::Named { dir: ".gemini/antigravity-cli/brain".into(), name: ".system_generated/logs/transcript.jsonl" }
    }

    fn could_record(_cwd: &str, _text: &str, _brief: &str) -> bool {
        true
    }

    /// The turn `text`, one conversation's transcript, records for the
    /// session handed `brief`. A transcript names no worktree: it is this
    /// session's when its first brief pointer is this brief's. A user's input
    /// starts a turn; the model's answer that calls no tool ends it.
    fn recorded_turn(_cwd: &str, text: &str, brief: &str) -> Option<Turn> {
        let pointer = |value: Value| {
            let said = request(value["content"].as_str()?);
            said.starts_with(crate::brief::BRIEF_POINTER_PREFIX).then(|| said == crate::brief::brief_pointer(brief))
        };
        if !json_lines(text).filter(user_input).find_map(pointer)? {
            return None;
        }
        let notes: Vec<Note> = json_lines(text).filter_map(|value| note(&value)).collect();
        crate::turn::latest(&notes)
    }

    /// The transcript a stand-in writes for this turn: the brief typed, then
    /// a shell call (open) or an answer (closed) that spent 15 tokens given
    /// and 11 written.
    fn stand_in(_cwd: &str, brief: &str, open: bool, at: u64) -> String {
        let iso = session::format_iso(at);
        let content = format!(
            "<USER_REQUEST>\n{}\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: {iso}.\n</ADDITIONAL_METADATA>",
            crate::brief::brief_pointer(brief)
        );
        let typed = json!({ "step_index": 0, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE",
                            "created_at": iso, "content": content });
        let mut answer = json!({ "step_index": 1, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE",
                                 "created_at": iso, "model": "gemini-4-neon-high", "input_tokens": 3,
                                 "cache_read_tokens": 12, "output_tokens": 11 });
        if open {
            answer["tool_calls"] = json!([{ "name": SHELL_TOOL, "args": "{'CommandLine': '\"ls\"'}" }]);
        } else {
            answer["content"] = json!("Done.");
        }
        format!("{typed}\n{answer}\n")
    }

    const SPENT: Option<super::ReadSpent> = Some(spent);

    fn prompted(_text: &str, _said: &str, _since: u64) -> Option<bool> {
        None
    }
}

/// The tokens a transcript spent: each answer's input and cache reads
/// given, its output written.
fn spent(text: &str) -> (u64, u64) {
    json_lines(text).filter(planner).fold((0, 0), |(given, written), v| {
        let n = |field: &str| v[field].as_u64().unwrap_or(0);
        (given + n("input_tokens") + n("cache_read_tokens"), written + n("output_tokens"))
    })
}

/// What agy prints when its account's quota is spent, and its reset after it.
const QUOTA: &str = "Individual quota reached.";
const RESETS: &str = "Resets in ";

/// agy's quota on its screen (agy 1.5.14, seen on five horizon-labs
/// workers' screens 2026-11-22: `⚠ Individual quota reached. Please upgrade
/// your subscription to increase your limits. Resets in 2h50m4s.`, wrapped
/// at the width, then `Error ID: …`): its words the lines up to the error's
/// own id, so the same screen read again after the reset is told from a new
/// limit. With its reset, the engine's limit until then; without, the
/// account is used up.
fn quota(screen: &[String]) -> Option<Stop> {
    let at = screen.iter().rposition(|line| line.contains(QUOTA))?;
    let rest: Vec<&str> = screen[at..].iter().take(3).map(|l| l.trim()).collect();
    let upto = rest.iter().position(|l| l.starts_with("Error ID")).map_or(1, |id| id + 1);
    let said = rest[..upto].join(" ");
    Some(match rest.join(" ").split(RESETS).nth(1).and_then(duration_ms) {
        Some(resets_in_ms) => Stop::Limit { said, resets_in_ms },
        None => Stop::Quota(said),
    })
}

/// The duration `text` opens with, as agy writes one (`2h50m4s`, `45m`,
/// `1d3h`), in ms; `None` if it opens with none.
fn duration_ms(text: &str) -> Option<i64> {
    let (mut total, mut n, mut any) = (0i64, None::<i64>, false);
    for c in text.chars() {
        if let Some(d) = c.to_digit(10) {
            n = Some(n.unwrap_or(0) * 10 + i64::from(d));
            continue;
        }
        let unit = match c {
            'd' => 86_400_000,
            'h' => 3_600_000,
            'm' => 60_000,
            's' => 1_000,
            _ => break,
        };
        let Some(v) = n.take() else { break };
        (total, any) = (total + v * unit, true);
    }
    any.then_some(total)
}
