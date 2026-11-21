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
    format!(
        "{goal}\n\n# The diff\n\n```diff\n{diff}\n```\n\n# How to answer\n\nAnswer one JSON object and nothing else: \
         {{\"as_is\": \"<one line of why the change cannot be smaller>\"}}, or \
         {{\"cut\": \"<one line naming each place to cut, by file, and what to reuse, delete or simplify>\"}}.\n"
    )
}

/// The reviewer's answer `text`: its one JSON object (fenced or not), one
/// verdict of one line; or why it is not taken.
pub fn read(text: &str) -> Result<Reviewed, String> {
    let object = match (text.find('{'), text.rfind('}')) {
        (Some(from), Some(to)) if from < to => &text[from..=to],
        _ => return Err(format!("the reviewer answered no JSON object: {text}")),
    };
    let said: Said =
        serde_json::from_str(object).map_err(|e| format!("the reviewer's answer is not as asked ({e}): {text}"))?;
    let one_line = |s: &String| !s.trim().is_empty() && !s.contains('\n');
    match (said.as_is, said.cut) {
        (Some(why), None) if one_line(&why) => Ok(Reviewed::AsIs(why.trim().into())),
        (None, Some(what)) if one_line(&what) => Ok(Reviewed::Cut(what.trim().into())),
        _ => Err(format!("the reviewer answered neither one line of as_is nor one line of cut: {text}")),
    }
}

/// The worker's answer — the verdict, by whom, what it took, and the
/// command that writes the growth record of it for the issue it `serves`,
/// each word put in by `quote` — and the line its issue keeps of what
/// reviewing took.
pub fn answer(reviewed: &Reviewed, serves: &str, t: &Taken, quote: fn(&str) -> String) -> (Value, String) {
    let ids = format!("--reviewer {} --author {}", quote(&t.id()), quote(&t.author));
    let ratchet = format!(
        "factorium worker exclusive -- slimfit ratchet --serves {} --considered \"<the code you weighed reusing or deleting>\"",
        quote(serves)
    );
    let mut said = t.said("reviewer");
    let what = match reviewed {
        Reviewed::AsIs(why) => {
            said["as_is"] = why.as_str().into();
            said["next"] = format!("{ratchet} --as-is {} {ids}; commit .slimfit/ with the change", quote(why)).into();
            "as-is"
        }
        Reviewed::Cut(what) => {
            said["cut"] = what.as_str().into();
            said["next"] = format!(
                "make the cuts, or answer each with why not; if the change still adds more than the allowance, \
                 {ratchet} --cut {} --answer \"<what you did, or why not>\" {ids}; commit .slimfit/ with the change",
                quote(what)
            )
            .into();
            "cut"
        }
    };
    (said, t.cost("review-growth", "reviewer", "reviewed", what))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reviewer_s_answer_is_taken_only_as_one_verdict_of_one_line() {
        assert_eq!(read("```json\n{\"as_is\": \" new code only \"}\n```"), Ok(Reviewed::AsIs("new code only".into())));
        assert_eq!(read(r#"{"cut": "a.rs: reuse b"}"#), Ok(Reviewed::Cut("a.rs: reuse b".into())));
        for off in
            [r#"{"as_is": "x", "cut": "y"}"#, r#"{"as_is": "two\nlines"}"#, r#"{"cut": " "}"#, "{}", "as-is", "} {"]
        {
            assert!(read(off).is_err(), "{off}");
        }
    }
}
