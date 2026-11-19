//! A checkout that pins another checkout by git rev: once that one's base
//! moves past the pin, the dependent gets one issue to bump it
//! (factorium#127). What is read here is the dependent's `Cargo.toml`; what
//! is decided is which pins name which repo, and the issue's words.

use serde::Serialize;

/// A git dependency pinned to a revision: its repo's URL and the rev.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pin {
    pub url: String,
    pub rev: String,
}

/// The git dependencies `Cargo.toml` pins by `rev`, wherever they are
/// declared (a crate's or the workspace's, a target's), each once.
pub fn pins(cargo_toml: &str) -> Vec<Pin> {
    fn walk(v: &toml::Value, into: &mut Vec<Pin>) {
        let Some(t) = v.as_table() else { return };
        if let (Some(url), Some(rev)) = (t.get("git").and_then(|g| g.as_str()), t.get("rev").and_then(|r| r.as_str())) {
            let pin = Pin { url: url.into(), rev: rev.into() };
            if !into.contains(&pin) {
                into.push(pin);
            }
        }
        t.values().for_each(|v| walk(v, into));
    }
    let mut all = Vec::new();
    walk(&cargo_toml.parse().unwrap_or(toml::Value::Boolean(false)), &mut all);
    all
}

/// Whether two remote URLs name the same repo: the scheme, a user, a
/// trailing `.git` or `/` and the case aside, and `host:path` read as
/// `host/path`, so `git@github.com:o/r.git` is `https://github.com/o/r`.
pub fn same_repo(a: &str, b: &str) -> bool {
    fn plain(u: &str) -> String {
        let u = u.split_once("://").map_or(u, |(_, rest)| rest);
        let u = u.split_once('@').map_or(u, |(_, rest)| rest);
        let u = u.trim_end_matches('/');
        u.strip_suffix(".git").unwrap_or(u).replacen(':', "/", 1).to_lowercase()
    }
    plain(a) == plain(b)
}

/// What every bump issue of `dep` is titled, before its tip.
fn titled(dep: &str) -> String {
    format!("Bump {dep} to its base tip")
}

/// Whether one of the `open` issues' titles is a bump of `dep`.
pub fn open(dep: &str, open: &[String]) -> bool {
    open.iter().any(|t| t.starts_with(&titled(dep)))
}

/// The bump issue of `dep`, pinned at `rev`, to its base tip `tip`, with
/// its landings since the pin (newest first): its title and body.
pub fn issue(dep: &str, rev: &str, tip: &str, landings: &[String]) -> (String, String) {
    let title = format!("{} ({tip})", titled(dep));
    let body = format!(
        "This checkout pins {dep} at {rev}; {dep}'s base is at {tip}, {} landing(s) on:\n\n{}\n\n\
         Move the pin to {tip}, adapt, and file an issue in {dep} for anything it should fix itself.",
        landings.len(),
        landings.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"),
    );
    (title, body)
}
