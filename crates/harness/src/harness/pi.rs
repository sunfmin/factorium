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

use super::{Dialog, Kind, Link, Login, Stop};
use super::session::{self, json_lines};
use super::store::Layout;
use crate::harness::{Call, Harness};
use crate::line::Line;
use crate::start::{SKILLS, Start};
use crate::turn::{Marker, Note, Turn};

/// pi's flag that answers, for one run, the question it asks before it loads
/// what the directory it starts in holds of pi's own: a start directory's limits in pi's
/// form among it. Asked at the terminal, the question would take the
/// session's brief for its answer.
const APPROVE: &str = "--approve";

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
const MODEL: &str = "--model";

/// The variable that names pi's user directory: its own, in place of
/// the host user's.
const USER_DIR: &str = "PI_CODING_AGENT_DIR";
/// The variable the tool's extension reads the line that runs the guard
/// from; the worktree and the start directory it reads from theirs.
const GUARD_ENV: &str = "FACTORIUM_GUARD";

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
const KEPT_LINKS: &[Link] = &[
    Link { home: ".pi/agent/auth.json", kept: "auth.json" },
    Link { home: ".pi/agent/models.json", kept: "models.json" },
    Link { home: ".pi/agent/extensions/orca-agent-status.ts", kept: "extensions/orca-agent-status.ts" },
];

/// The tool pi runs shell lines with.
const SHELL_TOOL: &str = "bash";

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
/// reached. The terminal may wrap the JSON across lines, at a word's end or
/// within a word, so the screen is also read with its whitespace removed.
/// The same body on another harness's screen is not this quota.
fn account_quota(output: &[String]) -> Option<String> {
    if let Some(line) = output.iter().find(|line| zai_limit(line)) {
        return Some(line.clone());
    }
    let joined: String = output.concat().split_whitespace().collect();
    zai_limit(&joined).then(|| output.join("\n"))
}

/// Whether `text` is Z.ai's usage-limit error: status 429, then one JSON
/// body with code 1308 and a usage-limit message, the message read without
/// its whitespace. A line the terminal wrapped is not that body until the
/// screen is joined.
fn zai_limit(text: &str) -> bool {
    let Some(start) = text.find('{') else { return false };
    let status = text[..start].split(|character: char| !character.is_ascii_digit()).any(|digits| digits == "429");
    if !status {
        return false;
    }
    let Some(Ok(body)) = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>().next() else {
        return false;
    };
    let known_code = body.get("code").is_some_and(|code| code.as_str() == Some("1308") || code.as_u64() == Some(1308));
    let explicit_limit = body.get("message").and_then(Value::as_str).is_some_and(|message| {
        message.split_whitespace().collect::<String>().to_lowercase().contains("usagelimitreached")
    });
    known_code && explicit_limit
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

/// pi, as a harness: what its module says of it.
pub(super) struct Pi;

impl Kind for Pi {
    const KEY: &'static str = "pi";
    const LOGIN: Login = Login::Unused;
    const USER_HOME: Option<&'static str> = None;
    const KEPT_LINKS: &'static [Link] = KEPT_LINKS;
    /// Nothing; `APPROVE` is always added.
    const STOCK_ARGUMENTS: &'static str = "";
    const ASKS_FIRST: Option<&'static str> = None;
    const SHOWS_UP: Option<&'static str> = None;
    const ASKS_MIDWAY: &'static [Dialog] = &[];
    const ENVIRONMENT: &'static [&'static str] = &[];
    /// The tool's extension takes any exit but 0 for a refusal.
    const REFUSED: u8 = 2;
    /// Escape.
    const INTERRUPT: &'static str = "\x1b";
    /// Not asked headless: no run of it so has been read here.
    const HEADLESS: Option<crate::ask::Headless> = None;

    /// What the host keeps for pi beside the links: the tool's extension.
    fn kept(_start: &Start) -> Result<Vec<(&'static str, String)>, String> {
        Ok(vec![(EXTENSION_FILE, include_str!("pi/extension.js").to_string())])
    }

    const TOLD_IN: Option<&'static str> = None;

    /// The guard line the kept extension runs: its variable's, where it reads it.
    fn guards(home: &std::path::Path) -> Option<Vec<String>> {
        let extension = std::fs::read_to_string(home.join(EXTENSION_FILE)).unwrap_or_default();
        Some(extension.contains(GUARD_ENV).then(|| format!("${GUARD_ENV}")).into_iter().collect())
    }

    fn start_dir<'a>(start: &Start<'a>) -> &'a str {
        start.dir
    }

    /// What pi and the tool's extension read from its environment: the user
    /// directory the host keeps, the line that runs the guard, the worktree, and
    /// the added directory, whose rules a pi session is given as it is its
    /// worktree's.
    fn environment(line: Line, start: &Start) -> Line {
        start.rules_env(line.set(USER_DIR, start.home.dir()).set(GUARD_ENV, &start.hook(Harness::Pi)))
    }

    /// What pi is given to put a session in its start directory: its question
    /// answered, the start directory's skills and no others, the tool's
    /// extension, and the caller's model.
    fn added(line: Line, start: &Start) -> Line {
        let line = line.arg(APPROVE).arg(NO_SKILLS).flag(SKILL, &format!("{}/{SKILLS}", start.dir));
        line.flag(EXTENSION, &format!("{}/{EXTENSION_FILE}", start.home.dir())).flag_if(MODEL, start.model)
    }

    fn briefed(line: String, brief: &str) -> (String, Option<String>) {
        (line, Some(crate::brief::brief_pointer(brief)))
    }

    /// The tool call `described` to the tool's extension, as the guard judges
    /// one.
    fn call<'de, D: Deserializer<'de>>(described: D) -> Result<Call, D::Error> {
        let described = Described::deserialize(described)?;
        Ok(match described.input.command {
            Some(line) if described.tool_name == SHELL_TOOL => Call::Shell(line),
            _ => Call::NotShell,
        })
    }

    fn stopped(screen: &[String]) -> Option<Stop> {
        account_quota(screen).map(Stop::Quota)
    }

    /// Where pi keeps the session of `cwd`: one JSONL file for each session, in
    /// the directory it names from the worktree path.
    fn layout(cwd: &str) -> Layout {
        Layout::Files { dir: format!("sessions/{}", session::wrapped_dir(cwd)), extension: "jsonl" }
    }

    fn could_record(_cwd: &str, _text: &str, _brief: &str) -> bool {
        true
    }

    /// The turn `text` records for `cwd`. A session header for another worktree
    /// is not this file. pi records no child session.
    fn recorded_turn(cwd: &str, text: &str, _brief: &str) -> Option<Turn> {
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
        crate::turn::latest(&notes)
    }

    /// The session a stand-in writes for this turn.
    fn stand_in(cwd: &str, _brief: &str, open: bool, at: u64) -> String {
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
                    ("message", serde_json::json!({ "role": "assistant", "stopReason": "stop", "usage": { "input": 3, "cacheRead": 7, "cacheWrite": 5, "output": 11 } })),
                ])
            ));
        }
        body
    }

    const SPENT: Option<super::ReadSpent> = Some(spent);

    fn prompted(_text: &str, _said: &str, _since: u64) -> Option<bool> {
        None
    }
}

/// The tokens a session file spent: each assistant message's `usage`, its
/// cache reads and writes counted as input.
fn spent(text: &str) -> (u64, u64) {
    let usages = json_lines(text)
        .filter(|v| v["type"] == "message" && v["message"]["role"] == "assistant")
        .map(|v| v["message"]["usage"].clone());
    usages.fold((0, 0), |(i, o), u| {
        let n = |field: &str| u[field].as_u64().unwrap_or(0);
        (i + n("input") + n("cacheRead") + n("cacheWrite"), o + n("output"))
    })
}
