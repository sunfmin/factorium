//! An issue's rules read by the domain alone: an old block's why, the
//! question it asks, and who asked last (factorium#304: once a never-built
//! unit test of `issue.rs`; its block cases `tracker.rs` holds through `App`).

use factorium_domain::issue::{Asker, Author, Issue, IssueEvent, Issues, NO_REASON, On, asked, question};
use factum::Aggregate;
use serde_json::json;

/// A block keeps its why; one kept before blocks had whys reads as no reason given.
#[test]
fn a_block_keeps_its_why_and_one_made_before_whys_reads_as_no_reason_given() {
    let mut i = Issue { checkout: "web".into(), number: 1, ..Issue::default() };
    Issues::evolve(&mut i, IssueEvent::BlockedBy { number: 2, why: Some("needs its parser".into()) });
    let old: IssueEvent = serde_json::from_value(json!({ "type": "BlockedBy", "data": { "number": 3 } })).unwrap();
    Issues::evolve(&mut i, old);
    let whys: Vec<_> = i.waits_for().into_iter().map(|(n, a)| (n, a.why().to_string())).collect();
    assert_eq!(whys, [("#2".into(), "needs its parser".into()), ("#3".into(), NO_REASON.to_string())]);
}

#[test]
fn the_question_is_the_first_line_after_any_based_on_line() {
    assert_eq!(question("Based on proving 1a2b.\n\nShip it now?\nmore"), "Ship it now?");
    assert_eq!(question("\n  Ship it now?  \n"), "Ship it now?");
    assert_eq!(question("Based on proving 1a2b."), "");
}

#[test]
fn the_last_wait_for_a_person_names_when_and_who() {
    let (watch, owner) = (true, false);
    let opened = |waiting_for_human| IssueEvent::IssueOpened {
        checkout: "web".into(),
        number: 1,
        title: "t".into(),
        body: String::new(),
        waiting_for_human,
        why: None,
        task: None,
        on: On::default(),
        decided: Vec::new(),
    };
    let wait = || IssueEvent::IssueWaitingForHuman { reason: "why".into() };
    assert_eq!(asked([(opened(true), 5, watch)]), Some((5, Asker::Added)));
    assert_eq!(asked([(opened(false), 5, owner), (wait(), 7, watch)]), Some((7, Asker::Watch)));
    let again = [(opened(true), 1, owner), (IssueEvent::IssueReadied, 2, owner), (wait(), 3, owner)];
    assert_eq!(asked(again), Some((3, Asker::Session)));
    let commented = IssueEvent::IssueCommented { text: "x".into(), by: Author::Owner, at: None, decided: None };
    assert_eq!(asked([(opened(true), 1, owner), (commented, 2, watch)]), Some((1, Asker::Added)));
    assert_eq!(asked([(opened(true), 1, owner), (IssueEvent::IssueReadied, 2, owner)]), None);
}
