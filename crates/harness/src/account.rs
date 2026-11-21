//! The engine accounts a session may start on, read from mytokens, their one
//! home: several per harness, each one mytokens secret named
//! `agent-account-<name>`, whose fields say its harness and its credential —
//! a token, or the machine a refreshing login is bound to, as its harness
//! logs in (`Harness::login`).
//!
//! A credential's value goes only into the environment a session starts with
//! ([`Usable::env`]): nothing here prints it or writes it anywhere, and
//! reading it keeps nothing of what mytokens printed but what it parsed.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use crate::Harness;
use crate::harness::Login;

/// What the name of every account's mytokens secret starts with; the rest is
/// the account's name.
pub const PREFIX: &str = "agent-account-";
/// The directory under the user's home that holds each account's own home,
/// one directory per account named as the account is: the same on every
/// machine and for every program, so that a login a person made once is
/// found by all.
const HOMES: &str = ".agent-accounts";

/// The field that names the account's harness, by its key (`Harness::key`).
const HARNESS: &str = "HARNESS";
/// The field that says how many sessions may run on it at once; optional.
const SESSIONS: &str = "SESSIONS";
/// The field that names the machine a refreshing login is bound to.
const MACHINE: &str = "MACHINE";

/// mytokens run with these arguments: what it printed, or why it failed.
/// The why never holds what it printed.
pub type Mytokens<'a> = &'a dyn Fn(&[&str]) -> Result<String, String>;

/// The accounts mytokens holds, as this machine can use them now: those a
/// session can start on, and every other with why it cannot.
pub struct Here {
    pub usable: Vec<Usable>,
    pub skipped: Vec<Skipped>,
}

/// An account no session can start on here now, and why — what a person
/// does about it, when it is theirs to do. Its why never holds a credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// Its mytokens secret; `mytokens` when mytokens could list none.
    pub secret: String,
    pub why: String,
}

/// An account a session can start on here now: its name, its harness, and
/// the environment a session on it starts with. No `Debug`: the
/// environment holds the credential.
#[derive(Clone, PartialEq, Eq)]
pub struct Usable {
    pub name: String,
    pub harness: Harness,
    pub env: Vec<(&'static str, String)>,
}

/// How an account's sessions are logged in.
enum Credential {
    /// A token, in the field and variable `field`, usable on several
    /// machines at once.
    Token { field: &'static str, token: String },
    /// A refreshing login in a file of the account's home on one machine,
    /// whose refresh token no other machine may share.
    Login { machine: String },
}

/// One engine account.
struct Account {
    /// Its name: its secret's, less [`PREFIX`].
    name: String,
    harness: Harness,
    credential: Credential,
}

/// The accounts `mytokens` holds, as this machine can use them now under
/// the user's home `user_home`: a token anywhere, a login on its
/// own machine once it is logged in there. Every secret named as an account
/// is one or the other.
pub fn here(mytokens: Mytokens, user_home: &Path) -> Here {
    let mut here = Here { usable: Vec::new(), skipped: Vec::new() };
    let listed = match mytokens(&["list"]) {
        Ok(listed) => listed,
        Err(e) => {
            here.skipped.push(Skipped { secret: "mytokens".into(), why: e });
            return here;
        }
    };
    let machine = this_machine();
    for service in services(&listed) {
        let usable =
            mytokens(&["get", &service, "--json"]).and_then(|json| parse(&service, &json)).and_then(|account| {
                let env = account.env(machine.as_deref().map_err(String::as_str), user_home)?;
                Ok(Usable { name: account.name, harness: account.harness, env })
            });
        match usable {
            Ok(usable) => here.usable.push(usable),
            Err(why) => here.skipped.push(Skipped { secret: service, why }),
        }
    }
    here
}

/// The account secrets `mytokens list` printed, each once, in its order: a
/// line's first column is `<service>/<account label>`.
fn services(listed: &str) -> Vec<String> {
    let mut services: Vec<String> = Vec::new();
    for line in listed.lines() {
        let first = line.split(['\t', ' ']).next().unwrap_or("");
        let service = first.rsplit_once('/').map_or(first, |(service, _)| service);
        if service.starts_with(PREFIX) && !services.iter().any(|s| s == service) {
            services.push(service.to_string());
        }
    }
    services
}

/// Whether `name` can name an account and its home: a single, plain
/// directory name.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The account the secret `service` is, from what `mytokens get --json`
/// printed of it. A why never holds a field's value.
fn parse(service: &str, json: &str) -> Result<Account, String> {
    let name = service.strip_prefix(PREFIX).unwrap_or(service);
    if !plain_name(name) {
        return Err(format!("`{name}` is no account name: letters, digits, `-`, `_` and `.`"));
    }
    let read: serde_json::Value = serde_json::from_str(json).map_err(|_| "mytokens printed no JSON".to_string())?;
    let Some(fields) = read.get("fields").and_then(|f| f.as_object()) else {
        return Err(format!("it holds no fields: an account is a profile with `{HARNESS}`"));
    };
    let mut harness = None;
    let mut machine = None;
    let mut others = Vec::new();
    for (field, value) in fields {
        let Some(text) = value.as_str() else {
            return Err(format!("its field `{field}` holds no text"));
        };
        let text = text.trim();
        match field.as_str() {
            HARNESS => harness = Some(Harness::read(text).ok_or_else(|| format!("`{text}` is no harness"))?),
            SESSIONS => {
                text.parse::<NonZeroU32>()
                    .map_err(|_| format!("its `{SESSIONS}` is no whole number of one or more"))?;
            }
            MACHINE if !text.is_empty() => machine = Some(text.to_string()),
            MACHINE => return Err(format!("its `{field}` is empty")),
            other => others.push((other, text)),
        }
    }
    let Some(harness) = harness else {
        return Err(format!("it names no `{HARNESS}`"));
    };
    let mut token = None;
    for (field, text) in others {
        match harness.login() {
            Login::Token(own) if own == field && !text.is_empty() => token = Some((own, text.to_string())),
            Login::Token(own) if own == field => return Err(format!("its `{field}` is empty")),
            _ => return Err(format!("`{field}` is no field of a {} account", harness.key())),
        }
    }
    let key = harness.key();
    let credential = match (harness.login(), token, machine) {
        (Login::Token(_), Some((field, token)), None) => Credential::Token { field, token },
        (Login::Token(field), None, _) => return Err(format!("a {key} account holds its token, `{field}`")),
        (Login::Token(_), Some(_), Some(_)) => {
            return Err(format!("a {key} account's token is bound to no machine: it names no `{MACHINE}`"));
        }
        (Login::Machine { .. }, None, Some(machine)) => Credential::Login { machine },
        (Login::Machine { .. }, ..) => {
            return Err(format!("a {key} account names the one `{MACHINE}` its login is on, and holds no token"));
        }
        (Login::Unused, ..) => return Err(format!("{key} takes no account")),
    };
    Ok(Account { name: name.to_string(), harness, credential })
}

impl Account {
    /// The environment a session on it starts with on the machine `here`
    /// (`Err`: this machine's name is unknown), the user's home being
    /// `user_home`; `Err`: why it cannot start here now. A token
    /// goes in alone; a login, on its machine, needs nothing: it is its home
    /// ([`login_home`]), found logged in by its login file there, which the
    /// session's home is (`Harness::home`).
    fn env(&self, here: Result<&str, &str>, user_home: &Path) -> Result<Vec<(&'static str, String)>, String> {
        let machine = match &self.credential {
            Credential::Token { field, token } => return Ok(vec![(*field, token.clone())]),
            Credential::Login { machine } => machine,
        };
        let here = here.map_err(|e| format!("its login is on `{machine}`, and {e}"))?;
        if !machine.eq_ignore_ascii_case(here) {
            return Err(format!("its login is on `{machine}`, not this machine `{here}`"));
        }
        let home = login_home(user_home, &self.name);
        let login = self.harness.login_file().unwrap_or_default();
        if home.join(login).exists() {
            return Ok(Vec::new());
        }
        Err(format!(
            "it is not logged in here: no `{login}` in {}; log in once with `{}`",
            home.display(),
            self.harness.login_line(&home)
        ))
    }
}

/// The home of the login account `name`, under the user's home `user_home`.
pub(crate) fn login_home(user_home: &Path, name: &str) -> PathBuf {
    user_home.join(HOMES).join(name)
}

/// This machine's name, as `hostname -s` says it: what an account's
/// `MACHINE` names. The system's name up to its first `.`.
fn this_machine() -> Result<String, String> {
    let system = gethostname::gethostname();
    let system = system.to_string_lossy();
    let name = short_name(&system);
    if name.is_empty() {
        return Err(format!("the system names no machine ({system:?})"));
    }
    Ok(name.to_string())
}

/// `name` as `hostname -s` prints it: trimmed, and cut at its first `.`.
fn short_name(name: &str) -> &str {
    let name = name.trim();
    name.split_once('.').map_or(name, |(short, _)| short)
}
