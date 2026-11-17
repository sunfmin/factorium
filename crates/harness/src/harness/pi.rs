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
use super::{Link, SessionFile, SessionLayout};
use crate::harness::Call;
use crate::line::Line;
use crate::start::SKILLS;
use crate::start::{START_DIR_ENV, Start, WORKTREE_ENV};
use crate::turn::{Marker, Note, Turn};

pub(super) const KEY: &str = "pi";

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
pub(super) const MODEL: &str = "--model";

/// The variable that names pi's user directory: its own, in place of
/// the host user's.
pub(super) const USER_DIR: &str = "PI_CODING_AGENT_DIR";
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
pub(super) const KEPT_LINKS: &[Link] = &[
    Link { home: ".pi/agent/auth.json", kept: "auth.json" },
    Link { home: ".pi/agent/models.json", kept: "models.json" },
    Link { home: ".pi/agent/extensions/orca-agent-status.ts", kept: "extensions/orca-agent-status.ts" },
];

/// What the host keeps for pi beside the links: the tool's extension.
pub(super) fn kept(_start: &Start) -> Vec<(&'static str, String)> {
    vec![(EXTENSION_FILE, include_str!("pi/extension.js").to_string())]
}

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
        .word(start.home.dir())
        .fixed(" ")
        .fixed(GUARD_ENV)
        .fixed("=")
        .word(start.hook)
        .fixed(" ")
        .fixed(WORKTREE_ENV)
        .fixed("=")
        .word(start.worktree)
        .fixed(" ")
        .fixed(START_DIR_ENV)
        .fixed("=")
        .word(start.dir)
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
    let line = line.fixed(" ").fixed(EXTENSION).fixed(" ").word(&format!("{}/{EXTENSION_FILE}", start.home.dir()));
    match start.model {
        Some(model) => line.fixed(" ").fixed(MODEL).fixed(" ").word(model),
        None => line,
    }
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
