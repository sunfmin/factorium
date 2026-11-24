//! Decided records by the domain's rule alone: which text is a record, a
//! fence quoting the shape, the numbers they keep, and their names
//! (factorium#304: once a never-built unit test of `decided.rs`).

use factorium_domain::decided::{
    Given, Mark, Numbers, NumbersEvent, Record, in_body, in_comment, name, numbers, numbers_id, parse, remarked,
};
use factum::Aggregate;

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
    let body = format!("Based on proving 1a2b.\n\nWhy it matters.\n\n{RECORD}\nDecided (watch, for Felix): park it\n");
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

/// The sequence's stream is the one every binary gives from: another
/// would give each number again.
#[test]
fn the_sequence_s_stream_stays_the_same() {
    assert_eq!(numbers_id().to_string(), "b5346f80-045a-50f9-8d8f-78026838caa6");
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
