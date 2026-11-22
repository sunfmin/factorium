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
//!
//! Each record has a short number, `D<n>`, one sequence for the host, given
//! once when it enters its issue ([`Numbers`]) and kept on the issue's own
//! events: a comment's on its event, a body's as [`Mark`]s.

use factum_core::{Aggregate, Uuid};
use serde::{Deserialize, Serialize};

use crate::command;

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

impl Record {
    /// Whether `m` marks it: a body's record keeps its number across edits
    /// while its head line (who and what) stays as it was.
    pub fn marked_by(&self, m: &Mark) -> bool {
        (self.who.as_str(), self.what.as_str()) == (m.who.as_str(), m.what.as_str())
    }
}

/// A body record's number, with the head it was given to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub id: u64,
    pub who: String,
    pub what: String,
}

impl Mark {
    pub fn of(r: &Record, id: u64) -> Mark {
        Mark { id, who: r.who.clone(), what: r.what.clone() }
    }
}

/// The number each of `records` (a body's) has among `marks`, in order,
/// each mark used once; none for one no mark names.
pub fn numbers(records: &[Record], marks: &[Mark]) -> Vec<Option<u64>> {
    let mut used = vec![false; marks.len()];
    (records.iter())
        .map(|r| {
            let i = (0..marks.len()).find(|&i| !used[i] && r.marked_by(&marks[i]))?;
            used[i] = true;
            Some(marks[i].id)
        })
        .collect()
}

/// The marks of `records` after an edit: each keeps the number `marks`
/// gave it, the rest take `fresh` in order; none if `fresh` runs short.
pub fn remarked(records: &[Record], marks: &[Mark], fresh: &[u64]) -> Option<Vec<Mark>> {
    let mut fresh = fresh.iter().copied();
    let ids = numbers(records, marks).into_iter().map(|id| id.or_else(|| fresh.next()));
    records.iter().zip(ids).map(|(r, id)| Some(Mark::of(r, id?))).collect()
}

/// A record's name, as the owner says it.
pub fn name(id: u64) -> String {
    format!("D{id}")
}

/// The number a name says: `D17`, `d17` or `17`.
pub fn parse(name: &str) -> Option<u64> {
    let n = name.trim();
    n.strip_prefix(['D', 'd']).unwrap_or(n).parse().ok().filter(|&n| n > 0)
}

/// Numbers for the Decided records of issue `number` of `checkout`: its
/// body's, by head, and its comments', by index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Numbering {
    pub checkout: String,
    pub number: u64,
    pub body: Vec<Mark>,
    pub comments: Vec<(usize, u64)>,
}

/// The host's one sequence of record numbers: the last given, whether the
/// records made before numbers were given have theirs, and those of them
/// in an issue that had closed, which takes no event: placed here, named by
/// issue and place, never copied.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Given {
    pub last: u64,
    pub swept: bool,
    #[serde(default)]
    pub placed: Vec<Numbering>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum NumbersEvent {
    /// Numbers `from` to `from + count - 1` are given; with `sweep`, to the
    /// records that had none.
    NumbersGiven { from: u64, count: u64, sweep: bool },
    /// Numbers given in a sweep to records of issues that had closed.
    NumbersPlaced { placed: Vec<Numbering> },
}

pub struct Numbers;

impl Aggregate for Numbers {
    const NAME: &'static str = "decided";
    type State = Given;
    type Event = NumbersEvent;

    fn evolve(s: &mut Given, event: NumbersEvent) {
        match event {
            NumbersEvent::NumbersGiven { from, count, sweep } => {
                s.last = s.last.max((from + count).saturating_sub(1));
                s.swept |= sweep;
            }
            NumbersEvent::NumbersPlaced { placed } => s.placed.extend(placed),
        }
    }
}

/// The id of the host's one sequence of record numbers.
pub fn numbers_id() -> Uuid {
    crate::id("decided:numbers")
}

/// `count` numbers wanted; with `sweep`, for the records that had none.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Give {
    pub count: u64,
    #[serde(default)]
    pub sweep: bool,
}

command!(GiveNumbers, "decided.give", Numbers, Give, |_p| numbers_id(), |s, p, now| {
    Ok(if p.count == 0 && !(p.sweep && !s.swept) {
        Vec::new()
    } else {
        vec![NumbersEvent::NumbersGiven { from: s.last + 1, count: p.count, sweep: p.sweep }]
    })
});

/// Numbers a sweep gave to records of issues that had closed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Place {
    pub placed: Vec<Numbering>,
}

command!(PlaceNumbers, "decided.place", Numbers, Place, |_p| numbers_id(), |s, p, now| {
    Ok(if p.placed.is_empty() { Vec::new() } else { vec![NumbersEvent::NumbersPlaced { placed: p.placed.clone() }] })
});

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
    (run.len() >= 3).then(|| &run[..3])
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
        let tilde = "~~~~text\nDecided (a): x\n```\n~~~ not a closer\nDecided (b): still in\n~~~~";
        let body = format!("{quoted}\n{tilde}\n{RECORD}\n```\nUndo: quoted\n```\nDecided (c): after");
        let found = in_body(&body);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0], kept());
        assert_eq!(found[1].what, "after");
        assert_eq!(in_comment(quoted), None);
    }

    #[test]
    fn a_body_record_keeps_its_number_while_its_head_stays() {
        let rec = |who: &str, what: &str| Record { who: who.into(), what: what.into(), ..Record::default() };
        let marks = [Mark::of(&rec("a", "x"), 3), Mark::of(&rec("b", "y"), 5)];
        let now = [rec("b", "y"), rec("c", "new"), rec("a", "x")];
        assert_eq!(numbers(&now, &marks), [Some(5), None, Some(3)]);
        let after = remarked(&now, &marks, &[9]).unwrap();
        assert_eq!(after.iter().map(|m| m.id).collect::<Vec<_>>(), [5, 9, 3]);
        assert_eq!(remarked(&now, &marks, &[]), None, "too few fresh numbers");
        assert_eq!(numbers(&[rec("a", "x"), rec("a", "x")], &marks[..1]), [Some(3), None], "a mark serves once");
    }

    #[test]
    fn the_sequence_gives_on_from_its_last_and_stays_swept() {
        let mut s = Given::default();
        let give = |s: &mut Given, count, sweep| {
            let from = s.last + 1;
            Numbers::evolve(s, NumbersEvent::NumbersGiven { from, count, sweep });
        };
        give(&mut s, 0, true);
        assert_eq!((s.last, s.swept), (0, true), "a sweep that found nothing");
        give(&mut s, 3, false);
        assert_eq!((s.last, s.swept), (3, true), "swept stays");
    }

    #[test]
    fn a_name_is_d_and_its_number() {
        assert_eq!(name(17), "D17");
        assert_eq!((parse("D17"), parse("d17"), parse("17"), parse(" D3 ")), (Some(17), Some(17), Some(17), Some(3)));
        assert_eq!((parse("D0"), parse("Dx"), parse("rolesmith#4")), (None, None, None));
    }

    #[test]
    fn a_line_starting_with_a_short_run_of_backticks_or_tildes_is_no_fence() {
        let body = format!("`x` answers it\n``x`` too\n~x\n{RECORD}");
        assert_eq!(in_body(&body), vec![kept()]);
        assert!(in_body("`x`\n``x``\n~x").is_empty());
    }
}
