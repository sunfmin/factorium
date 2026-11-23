//! What a role directory needs on this host before a worker starts with it
//! (factorium#277): `rolesmith doctor --project <dir>` says, from the roles
//! its `rolesmith.toml` records. A program it lacks is installed first
//! (`rolesmith tool install --project <dir>`); what is still lacking after,
//! a mytokens entry among it, holds the role's issues and is the watch's,
//! each thing named with the fix rolesmith gives.

use serde::{Deserialize, Serialize};

/// The file that makes a role directory one rolesmith checks: without it
/// the role needs nothing of the host.
pub const RECORD: &str = "rolesmith.toml";

/// One thing a role needs that this host lacks, as rolesmith names it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lack {
    /// `program` or `entry` (a mytokens entry).
    pub kind: String,
    pub name: String,
    pub reason: String,
    /// The command that fixes it.
    pub fix: String,
}

impl Lack {
    /// Whether `rolesmith tool install` can give it: a program.
    pub fn installable(&self) -> bool {
        self.kind == "program"
    }
}

/// What a role directory lacks, as its last check found: each thing
/// rolesmith names, and why the check or the install failed, where one did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lacking {
    pub lacks: Vec<Lack>,
    /// What rolesmith said where it named nothing: its install failed, or
    /// it could not check at all.
    #[serde(default)]
    pub failed: Option<String>,
}

impl Lacking {
    /// Whether an install may give what it lacks: a program, never an
    /// entry, and not the same lack an install already left so.
    pub fn to_install(&self, before: Option<&Lacking>) -> bool {
        self.lacks.iter().any(Lack::installable) && before.is_none_or(|b| b.lacks != self.lacks)
    }

    /// What it lacks, each thing with why: one line for the watch.
    pub fn what(&self) -> String {
        let mut said: Vec<String> =
            self.lacks.iter().map(|l| format!("{} `{}` ({})", l.kind, l.name, l.reason)).collect();
        said.extend(self.failed.iter().map(|f| format!("rolesmith: {f}")));
        said.join("; ")
    }

    /// The fixes rolesmith gives, each once, in its order.
    pub fn fixes(&self) -> Vec<String> {
        let mut fixes: Vec<String> = Vec::new();
        for l in &self.lacks {
            if !fixes.contains(&l.fix) {
                fixes.push(l.fix.clone());
            }
        }
        fixes
    }
}

/// What `rolesmith doctor --project <dir> --json` answered: `None`, nothing
/// lacking; else what is. `ok` is whether it exited 0, `out` its stdout,
/// `err` its stderr. A refusal that names nothing (no rolesmith, a record
/// it cannot read) lacks rolesmith's message itself.
pub fn read_doctor(ok: bool, out: &str, err: &str) -> Option<Lacking> {
    if ok {
        return None;
    }
    let answer: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_default();
    let error = &answer["error"];
    let lacks: Vec<Lack> = serde_json::from_value(error["missing"].clone()).unwrap_or_default();
    let failed = lacks.is_empty().then(|| refusal(error, err));
    Some(Lacking { lacks, failed })
}

/// Why rolesmith refused: its JSON error's message, else its stderr.
pub fn refusal(error: &serde_json::Value, err: &str) -> String {
    let said = error["message"].as_str().map_or_else(|| err.trim().to_string(), str::to_string);
    let said = said.strip_prefix("rolesmith: ").unwrap_or(&said).to_string();
    if said.is_empty() { "it failed and said nothing".into() } else { said }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MISSING: &str = r#"{"error":{"kind":"failed","message":"the project lacks 2 thing(s), on this machine:","missing":[
        {"kind":"program","name":"metal","reason":"check `xcrun metal` exits 1","fix":"rolesmith tool install --project /r","from":["felix/ios"]},
        {"kind":"entry","name":"apple","reason":"no mytokens entry","fix":"mytokens put apple --fields KEY"},
        {"kind":"program","name":"swift","reason":"check `swift` exits 127","fix":"rolesmith tool install --project /r"}]}}"#;

    #[test]
    fn a_doctor_that_passes_lacks_nothing_and_one_that_refuses_lacks_what_it_names() {
        assert_eq!(read_doctor(true, "{\"programs\":[]}", ""), None);
        let l = read_doctor(false, MISSING, "rolesmith: the project lacks").unwrap();
        let names: Vec<&str> = l.lacks.iter().map(|l| l.name.as_str()).collect();
        assert_eq!((names, l.failed.clone()), (vec!["metal", "apple", "swift"], None));
        assert_eq!(l.fixes(), ["rolesmith tool install --project /r", "mytokens put apple --fields KEY"]);
        assert_eq!(
            l.what(),
            "program `metal` (check `xcrun metal` exits 1); entry `apple` (no mytokens entry); program `swift` (check `swift` exits 127)"
        );
    }

    #[test]
    fn a_refusal_naming_nothing_lacks_rolesmiths_own_message() {
        let said = r#"{"error":{"kind":"failed","message":"/r has no rolesmith.toml"}}"#;
        let l = read_doctor(false, said, "rolesmith: /r has no rolesmith.toml\n").unwrap();
        assert_eq!((l.lacks.len(), l.failed.as_deref()), (0, Some("/r has no rolesmith.toml")));
        let gone = read_doctor(false, "", "rolesmith: No such file or directory\n").unwrap();
        assert_eq!(gone.failed.as_deref(), Some("No such file or directory"));
        assert_eq!(read_doctor(false, "", " ").unwrap().failed.as_deref(), Some("it failed and said nothing"));
        assert_eq!(gone.what(), "rolesmith: No such file or directory");
    }

    #[test]
    fn only_a_program_is_installed_and_never_twice_for_the_same_lack() {
        let program = Lack { kind: "program".into(), name: "metal".into(), ..Lack::default() };
        let entry = Lack { kind: "entry".into(), name: "apple".into(), ..Lack::default() };
        let entries = Lacking { lacks: vec![entry.clone()], failed: None };
        assert!(!entries.to_install(None), "an entry is never installed");
        let both = Lacking { lacks: vec![program.clone(), entry], failed: None };
        assert!(both.to_install(None));
        assert!(both.to_install(Some(&entries)), "a lack an install did not see yet");
        assert!(!both.to_install(Some(&both)), "an install already left it so");
    }
}
