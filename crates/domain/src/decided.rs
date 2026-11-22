//! A decision a decider (a session, a worker, the watch) made for the owner,
//! as it recorded it in the issue it belongs to: the body when the issue is
//! filed, a comment afterwards. The issue is its one home; doctor's
//! `decided` reads it from there. Its shape:
//!
//! ```text
//! Decided (<who>, for Felix): <what, one line>
//! Options: A) …  B) …
//! Chose: B, because …
//! Undo: <how to take it back>
//! ```

use serde::Serialize;

/// The line a record starts with.
const HEAD: &str = "Decided (";

/// One record, its fields as written; a field it lacks is none.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Record {
    pub who: String,
    pub what: String,
    pub options: Option<String>,
    pub chose: Option<String>,
    pub undo: Option<String>,
}

/// The record a comment is: one whose first line is `Decided (…`.
pub fn in_comment(text: &str) -> Option<Record> {
    let mut lines = text.lines().map(str::trim).skip_while(|l| l.is_empty());
    let head = lines.next()?;
    read(head, lines)
}

/// Every record a body holds, each starting on a `Decided (…` line outside
/// a fenced code block: one inside quotes the shape, it is not a record.
pub fn in_body(body: &str) -> Vec<Record> {
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let mut open: Option<&str> = None;
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        match (open, fence(line)) {
            (None, Some(f)) => open = Some(f),
            (Some(o), Some(f)) if f == o && line.trim_start_matches(&o[..1]).trim().is_empty() => open = None,
            (None, None) => found.extend(read(line, lines[i + 1..].iter().copied())),
            _ => {}
        }
    }
    found
}

/// The fence a line opens or closes: its run of ``` or ~~~, three or more.
fn fence(line: &str) -> Option<&str> {
    let c = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = &line[..line.len() - line.trim_start_matches(c).len()];
    (run.len() >= 3).then_some(&run[..3])
}

/// The record headed by `head`, each field from its own line after it, up to
/// the next record, a fence or a blank line; none unless `head` is a record's.
fn read<'a>(head: &str, rest: impl Iterator<Item = &'a str>) -> Option<Record> {
    let (who, what) = head.strip_prefix(HEAD)?.split_once("):")?;
    let who = who.split_once(", for").map_or(who, |(w, _)| w).trim();
    let end = |l: &&str| l.is_empty() || l.starts_with(HEAD) || fence(l).is_some();
    let lines: Vec<&str> = rest.take_while(|l| !end(l)).collect();
    let field = |p: &str| lines.iter().find_map(|l| Some(l.strip_prefix(p)?.trim().to_string()));
    let (options, chose, undo) = (field("Options:"), field("Chose:"), field("Undo:"));
    Some(Record { who: who.into(), what: what.trim().into(), options, chose, undo })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORD: &str = "Decided (worker, for Felix): KEEP applies only to sampled survivors\n\
        Options: A) every survivor  B) sampled survivors only\n\
        Chose: B, because the issue is stricter than it says\n\
        Undo: revert the commit";

    fn kept() -> Record {
        Record {
            who: "worker".into(),
            what: "KEEP applies only to sampled survivors".into(),
            options: Some("A) every survivor  B) sampled survivors only".into()),
            chose: Some("B, because the issue is stricter than it says".into()),
            undo: Some("revert the commit".into()),
        }
    }

    #[test]
    fn a_comment_is_a_record_only_when_its_first_line_is_one() {
        assert_eq!(in_comment(RECORD), Some(kept()));
        assert_eq!(in_comment(&format!("\n{RECORD}")), Some(kept()));
        assert_eq!(in_comment(&format!("measured 4.1s\n{RECORD}")), None);
        assert_eq!(in_comment("Decided: no parens"), None);
        let bare = in_comment("Decided (watch): restart it").unwrap();
        assert_eq!((bare.who.as_str(), bare.what.as_str(), bare.undo), ("watch", "restart it", None));
    }

    #[test]
    fn a_body_holds_each_record_on_its_own_lines() {
        let body =
            format!("Based on proving 1a2b.\n\nWhy it matters.\n\n{RECORD}\nDecided (watch, for Felix): park it\n");
        let found = in_body(&body);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], kept());
        assert_eq!((found[1].who.as_str(), found[1].what.as_str()), ("watch", "park it"));
        assert!(in_body("no record here").is_empty());
        let bare = in_body("Decided (a): x\nDecided (b): y\nUndo: z\n\nUndo: not b's");
        assert_eq!((bare[0].undo.as_deref(), bare[1].undo.as_deref()), (None, Some("z")));
    }

    #[test]
    fn a_record_in_a_fenced_block_quotes_the_shape_and_is_none() {
        let quoted = "```\nDecided (<who>, for Felix): <what>\nUndo: <how>\n```";
        let tilde = "~~~~text\nDecided (a): x\n```\nDecided (b): still in\n~~~~";
        let body = format!("{quoted}\n{tilde}\n{RECORD}\n```\nUndo: quoted\n```\nDecided (c): after");
        let found = in_body(&body);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0], kept());
        assert_eq!(found[1].what, "after");
        assert_eq!(in_comment(quoted), None);
    }
}
