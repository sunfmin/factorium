//! A worker's growth, judged by a second agent: the one question it is
//! asked — slimfit's goal for the reviewer and the change's diff, as
//! slimfit and git gave them — and what its answer must be to be taken:
//! `as-is` with one line of why, or one line naming what to cut. The rules
//! of judging are slimfit's (`slimfit help reviewer`), not restated here.

use serde::Deserialize;
use serde_json::Value;

use crate::pick::Taken;

/// What the reviewer answered, as taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reviewed {
    /// Why the change cannot be smaller.
    AsIs(String),
    /// What to cut: each place, and what to reuse, delete or simplify.
    Cut(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Said {
    as_is: Option<String>,
    cut: Option<String>,
}

/// The question the reviewer is asked: `goal` (slimfit's reviewer topic)
/// and the change's `diff`, each as given, then the one form its answer
/// takes.
pub fn question(goal: &str, diff: &str) -> String {
    format!("{goal}\n\n# The diff\n\n```diff\n{diff}\n```\n\n{}", crate::pick::how(FORM))
}

/// The one form the reviewer's answer takes.
pub const FORM: &str = "{\"as_is\": \"<one line of why the change cannot be smaller>\"}, or \
                        {\"cut\": \"<one line naming each place to cut, by file, and what to reuse, delete or simplify>\"}.";

/// The reviewer's answer `text`: its one JSON object (fenced or not), one
/// verdict of one line; or why it is not taken, in one line, without the
/// text.
pub fn read(text: &str) -> Result<Reviewed, String> {
    let said: Said = crate::pick::object(text, "reviewer")?;
    let one_line = |s: &String| !s.trim().is_empty() && !s.contains('\n');
    match (said.as_is, said.cut) {
        (Some(why), None) if one_line(&why) => Ok(Reviewed::AsIs(why.trim().into())),
        (None, Some(what)) if one_line(&what) => Ok(Reviewed::Cut(what.trim().into())),
        _ => Err("the reviewer answered neither one line of as_is nor one line of cut".into()),
    }
}

/// The worker's answer — the verdict, by whom, what it took, and the
/// command that writes the growth record of it for the issue it `serves`,
/// each word put in by `quote` — and the line its issue keeps of what
/// reviewing took.
pub fn answer(reviewed: &Reviewed, serves: &str, t: &Taken, quote: fn(&str) -> String) -> (Value, String) {
    let (field, verdict, first, flags) = match reviewed {
        Reviewed::AsIs(why) => ("as_is", why, "", format!("--as-is {}", quote(why))),
        Reviewed::Cut(what) => (
            "cut",
            what,
            "make the cuts, or answer each with why not; if the change still adds more than the allowance, ",
            format!("--cut {} --answer \"<what you did, or why not>\"", quote(what)),
        ),
    };
    let mut said = t.said("reviewer");
    said[field] = verdict.as_str().into();
    said["next"] = format!(
        "{first}factorium worker exclusive -- slimfit ratchet --serves {} --considered \"<the code you weighed reusing \
         or deleting>\" {flags} --reviewer {} --author {}; commit .slimfit/ with the change",
        quote(serves),
        quote(&t.id()),
        quote(&t.author)
    )
    .into();
    (said, t.cost("review-growth", "reviewer", "reviewed", &field.replace('_', "-")))
}
