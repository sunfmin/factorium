//! pi, as a harness: everything of a session's start, and of the guard's
//! asking, that is pi's own. It loads a start directory's rules from the directory it
//! starts in, and little else of what a session is to have, so
//! the tool adds the rest: the start directory's skills, named; the answer to the
//! question pi asks before it loads a start directory's own files; a user directory of
//! its own, since pi has no way to be told to leave the host user's
//! out; and an extension the tool carries, which asks the guard and loads the
//! rules of the worktree and the added rules.

use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::session::{self, json_lines};
use super::{Held, Kept, Link, SessionFile, SessionLayout};
use crate::harness::Call;
use crate::line::Line;
use crate::start::SKILLS;
use crate::start::Start;
use crate::steps::{self, Said};
use crate::tokens::{Counts, Keys, Usage};
use crate::turn::{Marker, Note, Turn};

pub(super) const NAME: &str = "pi";
pub(super) const KEY: &str = "pi";
pub(super) const PROGRAM: &str = "pi";
/// The line that installs it: its native installer, which keeps it up to date.
pub(super) const INSTALL: &str = "curl -fsSL https://pi.dev/install.sh | sh";
/// pi logs in to providers, and lists the models of those it holds
/// credentials for: which one a session uses is its caller's to say. It logs
/// in to a provider at its own prompt: its words say how, since no line does.
pub(super) const LOGIN: Option<super::Login> = Some(super::Login {
    question: "pi --list-models",
    logged_out: "holds credentials for no provider (`/login` at its prompt logs it in)",
    line: "pi",
});

/// What pi lists in place of its models when it holds credentials for no
/// provider.
const NO_MODELS: &str = "No models available";

/// Whether the answer to its login question says it holds credentials: it
/// listed a model at all.
pub(super) fn logged_in(succeeded: bool, printed: &str) -> bool {
    succeeded && !printed.trim().is_empty() && !printed.contains(NO_MODELS)
}
/// Where it reads its user's global instructions, under the user's home.
pub(super) const INSTRUCTIONS: &str = ".pi/agent/AGENTS.md";
/// The agent `npx skills` installs its user's skills for.
pub(super) const SKILLS_AGENT: Option<&str> = Some("pi");
/// pi asks nothing before a tool call: it takes no flag to work with nobody
/// at its terminal.
pub(super) const STOCK: &str = "pi";

/// pi's flag that answers, for one run, the question it asks before it loads
/// what the directory it starts in holds of pi's own: a start directory's limits in pi's
/// form among it. Asked at the terminal, the question would take the
/// session's brief for its answer.
const APPROVE: &str = "--approve";
pub(super) const TRUST: &str = APPROVE;
/// Its worktree trust is a flag (`APPROVE`): a launch checks the command
/// takes `FLAGS`.
pub(super) const TRUST_IS_FLAGS: bool = true;

/// Answers pi's worktree approval question on the launch line.
pub(super) fn in_worktree(command: &str) -> String {
    if command.contains(APPROVE) { command.to_string() } else { format!("{command} {APPROVE}") }
}

/// Starts one interactive session interactively, with pi's login and approval setup.
pub(super) fn interactive(command: &str, kept: &str, model: Option<&str>) -> String {
    let line =
        Line::new().fixed("export ").fixed(USER_DIR).fixed("=").word(kept).fixed(" && ").launch(&in_worktree(command));
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.done()
}

/// Loads the same extension as `start_line`, without adding rules.
pub(super) fn guarded(command: &str, worktree: &str, kept: &str, guard: &str) -> String {
    Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" FACTORIUM_GUARD=")
        .word(guard)
        .fixed(" FACTORIUM_WORKTREE=")
        .word(worktree)
        .fixed(" FACTORIUM_ADDED_DIR=")
        .word(worktree)
        .fixed(" && ")
        .launch(command)
        .fixed(" ")
        .fixed(EXTENSION)
        .fixed(" ")
        .word(&format!("{kept}/{EXTENSION_FILE}"))
        .done()
}

/// pi's flag that leaves out every skill it would find by itself: those of
/// the host user's home among them, which it reads wherever its user
/// directory is. The host user's own are named, where a session is given
/// them, from the user directory the host keeps.
const NO_SKILLS: &str = "--no-skills";
/// pi's flag for a directory of skills to load all the same. Of two skills
/// of one name pi keeps the one named first.
const SKILL: &str = "--skill";
/// pi's flag for an extension to load.
const EXTENSION: &str = "--extension";
/// pi's flag for the model of the session.
pub(super) const MODEL: &str = "--model";
/// Every flag the tool adds: what a launch command that starts pi
/// must take.
pub(super) const FLAGS: [&str; 5] = [APPROVE, NO_SKILLS, SKILL, EXTENSION, MODEL];
/// What pi is asked for the flags it takes.
const HELP: &str = " --help";

/// What pi is asked for the models it knows.
const LIST_MODELS: &str = " --list-models";
/// What may follow a model's name and a `:`, where pi is given one: how hard
/// the model is to think.
const THINKING: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Where a start directory's limits for pi are written, in its directory: the
/// extensions pi loads from the directory it starts in, once its question is
/// answered (`APPROVE`).
pub(super) const LIMITS: &str = ".pi/extensions";
/// The files pi loads the rules of a directory from before the one every
/// harness loads (`start::RULES`), and in its place: it loads the first of
/// them a directory holds, and no other.
pub(super) const BEFORE_THE_RULES: [&str; 3] = ["AGENTS.override.md", "AGENTS.md", "AGENTS.MD"];

/// The variable that names pi's user directory: its own, in place of
/// the host user's.
pub(super) const USER_DIR: &str = "PI_CODING_AGENT_DIR";
/// The variables the tool's extension reads the line that runs the guard,
/// the worktree, and the tool's copy of the added directory, from.
const GUARD_ENV: &str = "FACTORIUM_GUARD";
const WORKTREE_ENV: &str = "FACTORIUM_WORKTREE";
const ADDED_ENV: &str = "FACTORIUM_ADDED_DIR";

/// The tool's extension, as the directory the host keeps for pi holds it.
/// It is named on the line that starts a session and is not among those pi
/// finds by itself: a session whose guard is not there does not start.
const EXTENSION_FILE: &str = "factorium.js";

/// What a host keeps for pi: its user directory, of its own. Of the
/// host user's it holds what the host knows of providers — the logins, and
/// the models the user defined — and the extension the session backend
/// installed to hear of a pi session's state; pi finds that one by itself,
/// so on a host where the backend installed none, nothing is told. They are
/// links: a login pi renews is renewed for the host user too, and neither
/// goes stale. No setting or other extension of the user's is there, and
/// their skills and rules only in the directory kept for a session given them.
pub(super) const KEPT: Kept = Kept {
    links: &[
        Link { home: ".pi/agent/auth.json", kept: "auth.json" },
        Link { home: ".pi/agent/models.json", kept: "models.json" },
        Link { home: ".pi/agent/extensions/orca-agent-status.ts", kept: "extensions/orca-agent-status.ts" },
    ],
    host_user: &[
        Link { home: ".pi/agent/skills", kept: HOST_SKILLS },
        Link { home: ".pi/agent/AGENTS.override.md", kept: "AGENTS.override.md" },
        Link { home: ".pi/agent/AGENTS.md", kept: "AGENTS.md" },
        Link { home: ".pi/agent/CLAUDE.md", kept: "CLAUDE.md" },
    ],
    files: &[(EXTENSION_FILE, include_str!("pi/extension.js"))],
};
/// Where the user directory the host keeps for a session given the host
/// user's skills and rules links to their skills. Their rules are linked to
/// where pi loads them from in its user directory, the first it finds — of
/// the names that differ in more than case: on a host that does not tell
/// case apart, one link would stand in for another.
const HOST_SKILLS: &str = "skills";

/// The tool pi runs shell lines with.
const SHELL_TOOL: &str = "bash";
/// The exit code with which the guard refuses a call of pi's: the tool's
/// extension takes any but 0 for a refusal.
pub(super) const REFUSED: u8 = 2;

/// `line`, which exports a session's environment, with what pi and the
/// the tool's extension read from it: the user directory the host keeps, the
/// line that runs the guard, the worktree, and the added directory,
/// whose rules a pi session is given as it is its worktree's.
pub(super) fn environment(line: Line, start: &Start) -> Line {
    line.fixed(" ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(start.kept)
        .fixed(" ")
        .fixed(GUARD_ENV)
        .fixed("=")
        .word(start.hook)
        .fixed(" ")
        .fixed(WORKTREE_ENV)
        .fixed("=")
        .word(start.worktree)
        .fixed(" ")
        .fixed(ADDED_ENV)
        .fixed("=")
        .word(start.added)
}

/// `line`, which starts the launch command, with what pi is given to
/// put a session in its start directory: its question answered, the start directory's skills, then
/// the host user's where it is given them, and no others, the tool's
/// extension, and the caller's model.
pub(super) fn added(line: Line, start: &Start) -> Line {
    let line = line
        .fixed(" ")
        .fixed(APPROVE)
        .fixed(" ")
        .fixed(NO_SKILLS)
        .fixed(" ")
        .fixed(SKILL)
        .fixed(" ")
        .word(&format!("{}/{SKILLS}", start.dir));
    let line = if start.host_user {
        line.fixed(" ").fixed(SKILL).fixed(" ").word(&format!("{}/{HOST_SKILLS}", start.kept))
    } else {
        line
    };
    let line = line.fixed(" ").fixed(EXTENSION).fixed(" ").word(&format!("{}/{EXTENSION_FILE}", start.kept));
    match start.model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    }
}

/// `line`, which starts the launch command, asking it which flags it
/// takes instead of starting a session.
pub(super) fn asked_its_flags(line: Line) -> Line {
    line.fixed(HELP)
}

/// Whether what a start directory holds at `LIMITS` limits a session: a
/// directory with an entry. What an extension refuses is not read.
pub(super) fn limited<D>(held: Held<D>) -> bool {
    match held {
        Held::Entries(entries) => entries > 0,
        Held::Nothing | Held::File(_) => false,
    }
}

/// The line that asks `command`, a launch command of pi's, which
/// models it knows, under the user directory `kept` a session of it is given:
/// the logins and the models there are the ones a session would have.
pub(super) fn models_question(command: &str, kept: &str) -> String {
    Line::new()
        .fixed("export ")
        .fixed(USER_DIR)
        .fixed("=")
        .word(kept)
        .fixed(" && ")
        .launch(command)
        .fixed(LIST_MODELS)
        .done()
}

/// The line that sends `model` of pi's the one-line `prompt` through
/// `command`, under the user directory `kept` a session of it is given, and
/// prints its answer.
pub(super) fn prompt_question(command: &str, kept: &str, model: Option<&str>, prompt: &str) -> String {
    let line = Line::new().fixed("export ").fixed(USER_DIR).fixed("=").word(kept).fixed(" && ").launch(command);
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(" --print ").word(prompt).done()
}

/// What pi's probe adds: its events as JSON, a line each, of which the
/// answer's names the provider and the model that answered.
const AS_JSON: &str = " --mode json";

/// The line that probes `model` with `prompt` through `command`, as
/// `prompt_question` sends it, printing its events in JSON (`answered_as`).
pub(super) fn probe_question(command: &str, kept: &str, model: Option<&str>, prompt: &str) -> String {
    let line = Line::new().fixed("export ").fixed(USER_DIR).fixed("=").word(kept).fixed(" && ").launch(command);
    let line = match model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    };
    line.fixed(AS_JSON).fixed(" --print ").word(prompt).done()
}

/// What comes before the provider of a message, and between it and its
/// model, in pi's events, with no whitespace.
const PROVIDER: &str = "\"provider\":\"";
const THEN_MODEL: &str = "\",\"model\":\"";

/// The model that answered `probe_question`, by `answer`: as `provider/id`,
/// the last message's — the answer's — whatever a terminal broke its lines
/// at.
pub(super) fn answered_as(answer: &str) -> Option<String> {
    let dense: String = answer.split_whitespace().collect();
    dense
        .split(PROVIDER)
        .skip(1)
        .filter_map(|after| {
            let (provider, rest) = after.split_once(THEN_MODEL)?;
            let (id, _) = rest.split_once('"')?;
            let named = !provider.is_empty() && !provider.contains('"') && !id.is_empty();
            named.then(|| format!("{provider}/{id}"))
        })
        .last()
}

/// The models pi lists in `answer`, what it printed for `models_question`,
/// each as `provider/id`: a table under a line that heads it `provider
/// model`, whatever a shell printed above. `None` when it lists none: pi has
/// no login on this host, or said something else.
pub(super) fn models(answer: &str) -> Option<Vec<String>> {
    let mut listed = vec![];
    let mut headed = false;
    for line in answer.lines() {
        let mut columns = line.split_whitespace();
        let (Some(provider), Some(id)) = (columns.next(), columns.next()) else { continue };
        if headed {
            listed.push(format!("{provider}/{id}"));
        } else {
            headed = provider == "provider" && id == "model";
        }
    }
    if listed.is_empty() { None } else { Some(listed) }
}

/// Whether `model`, as a caller writes it, is one of `listed`, the models pi
/// lists on a host, as pi reads a model's name: a `provider/id`, an id, or a
/// part of an id — in one provider, where it names one — whatever the case,
/// with how hard to think after a `:` or not. pi takes more than this: a
/// name it has no login for, and any name at all after a provider it knows.
/// Neither would run a session here, and neither is known.
pub(super) fn knows(listed: &[String], model: &str) -> bool {
    let thinks = model.rsplit_once(':').filter(|(_, level)| THINKING.contains(level));
    named(listed, model) || thinks.is_some_and(|(model, _)| named(listed, model))
}

/// Whether `model` is one of `listed` by its whole name, a `provider/id`,
/// with how hard to think after a `:` or not.
pub(super) fn names(listed: &[String], model: &str) -> bool {
    let whole = |name: &str| listed.iter().any(|full| full.eq_ignore_ascii_case(name.trim()));
    let thinks = model.rsplit_once(':').filter(|(_, level)| THINKING.contains(level));
    whole(model) || thinks.is_some_and(|(model, _)| whole(model))
}

/// Whether `name` names one of `listed`, each a `provider/id`.
fn named(listed: &[String], name: &str) -> bool {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return false;
    }
    let within = name.split_once('/').filter(|(_, part)| !part.is_empty());
    listed.iter().any(|full| {
        let full = full.to_lowercase();
        let (provider, id) = full.split_once('/').unwrap_or(("", &full));
        full == name || id.contains(&name) || within.is_some_and(|(of, part)| of == provider && id.contains(part))
    })
}

/// One tool call, as the tool's extension passes on what pi describes to it
/// before making the call.
#[derive(Deserialize)]
struct Described {
    #[serde(rename = "toolName")]
    tool_name: String,
    #[serde(default)]
    input: Input,
}

#[derive(Default, Deserialize)]
struct Input {
    /// The shell line, when the tool runs one.
    #[serde(default)]
    command: Option<String>,
}

/// Z.ai's usage limit on pi's screen.
/// An HTTP 429 whose body carries code 1308 and says the usage limit was
/// reached. The JSON may wrap across lines. The same body on another
/// harness's screen is not this quota.
pub(super) fn account_quota(output: &[String]) -> Option<String> {
    if let Some(line) = output.iter().find(|line| zai_limit(line)) {
        return Some(line.clone());
    }
    let flat = output.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    zai_limit(&flat).then(|| output.join("\n"))
}

/// Whether `text` is Z.ai's usage-limit error: status 429, then one JSON
/// body with code 1308 and a usage-limit message. A line the terminal
/// wrapped is not that body until the screen is joined.
fn zai_limit(text: &str) -> bool {
    let Some(start) = text.find('{') else { return false };
    let status = text[..start].split(|character: char| !character.is_ascii_digit()).any(|digits| digits == "429");
    if !status {
        return false;
    }
    let Ok(body) = serde_json::from_str::<Value>(&text[start..]) else { return false };
    let known_code = body.get("code").is_some_and(|code| code.as_str() == Some("1308") || code.as_u64() == Some(1308));
    let explicit_limit = body
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| message.to_lowercase().contains("usage limit reached"));
    known_code && explicit_limit
}

/// The tool call `described` to the tool's extension, as the guard judges
/// one.
pub(super) fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
    let described = Described::deserialize(described)?;
    Ok(match described.input.command {
        Some(line) if described.tool_name == SHELL_TOOL => Call::Shell(line),
        _ => Call::Other,
    })
}

/// The harness directory under a user's home.
pub(super) const HOME: &str = ".pi/agent";

/// Where pi keeps the session of `cwd`: one JSONL file for each session, in
/// the directory it names from the worktree path.
pub(super) fn session_layout(cwd: &str) -> SessionLayout {
    SessionLayout::Files { dir: format!("sessions/{}", session::wrapped_dir(cwd)), extension: "jsonl" }
}

/// The turn `text` records for `cwd`. A session header for another worktree
/// is not this file. pi records no child session.
pub(super) fn recorded_turn(cwd: &str, text: &str, not_before: Option<u64>) -> Option<Turn> {
    let mut notes = Vec::new();
    for value in json_lines(text) {
        if value["type"] == "session" && value["cwd"].as_str().is_some_and(|recorded| !session::same_cwd(recorded, cwd))
        {
            return None;
        }
        if let Some(note) = note(&value) {
            notes.push(note);
        }
    }
    session::turn_of(&notes, not_before)
}

fn note(value: &Value) -> Option<Note> {
    if value["type"] != "message" {
        return None;
    }
    let at = session::parse_iso(value["timestamp"].as_str()?)?;
    match value["message"]["role"].as_str()? {
        "user" => Some(Note { marker: Marker::Start, at, child: false }),
        "assistant" => match value["message"]["stopReason"].as_str()? {
            "stop" | "error" => Some(Note { marker: Marker::End, at, child: false }),
            _ => None,
        },
        _ => None,
    }
}

/// One session file a stand-in writes for this turn.
pub(super) fn session_files(cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
    let iso = session::format_iso(at);
    let mut body = format!(
        "{}\n",
        session::object(vec![("type", "session".into()), ("cwd", cwd.into()), ("timestamp", iso.clone().into()),])
    );
    body.push_str(&format!(
        "{}\n",
        session::object(vec![
            ("type", "message".into()),
            ("timestamp", iso.clone().into()),
            ("message", session::object(vec![("role", "user".into())])),
        ])
    ));
    if !open {
        body.push_str(&format!(
            "{}\n",
            session::object(vec![
                ("type", "message".into()),
                ("timestamp", iso.into()),
                ("message", session::object(vec![("role", "assistant".into()), ("stopReason", "stop".into()),]),),
            ])
        ));
    }
    vec![SessionFile {
        path: format!("sessions/{}/factorium-turn.jsonl", session::wrapped_dir(cwd)),
        body,
        compressed: false,
        replaces: None,
    }]
}

/// The names pi gives the counts of a message's usage.
const USAGE_KEYS: Keys = Keys {
    input: "input",
    cache_read: "cacheRead",
    cache_write: "cacheWrite",
    output: "output",
    reasoning: "reasoning",
};

/// What one pi session line says of usage: each assistant message, keyed by
/// its id, as a repeated line repeats it.
pub(super) fn usage(value: &Value, line: usize) -> Option<Usage> {
    let message = &value["message"];
    if value["type"] != "message" || message["role"] != "assistant" {
        return None;
    }
    let usage = &message["usage"];
    if !usage.is_object() {
        return None;
    }
    let Some(counts) = Counts::read(usage, &USAGE_KEYS, false) else {
        return Some(Usage::Uncounted);
    };
    Some(Usage::Message {
        key: super::claude_code::message_key(value, line),
        model: message["model"].as_str().unwrap_or("").to_string(),
        counts,
    })
}

/// What one pi session line says of a tool call: an assistant message's
/// `toolCall` parts are calls, and a `toolResult` message what came back of
/// the one its `toolCallId` names, `isError` where pi marks it an error (a
/// command that exited other than 0 is one). pi starts no child session in
/// its own file: a fork is a file of its own.
pub(super) fn said(value: &Value) -> Vec<Said> {
    let message = &value["message"];
    if value["type"] != "message" {
        return Vec::new();
    }
    match message["role"].as_str() {
        Some("assistant") => {
            let mut calls = Vec::new();
            let Some(parts) = message["content"].as_array() else { return calls };
            for part in parts {
                if part["type"] != "toolCall" {
                    continue;
                }
                let Some(id) = part["id"].as_str() else { continue };
                let name = part["name"].as_str().unwrap_or("");
                calls.push(Said::Call { id: id.to_string(), action: steps::action(name, &part["arguments"]) });
            }
            calls
        }
        Some("toolResult") => {
            let Some(id) = message["toolCallId"].as_str() else { return Vec::new() };
            let at = session::number_at(message["timestamp"].as_u64(), value["timestamp"].as_str());
            vec![Said::Result {
                id: id.to_string(),
                result: steps::content(&message["content"]),
                error: message["isError"] == true,
                at,
            }]
        }
        _ => Vec::new(),
    }
}
