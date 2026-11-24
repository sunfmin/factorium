//! An issue's rules read by the domain alone: the question it asks, and
//! who asked last (factorium#304: once a never-built unit test of
//! `issue.rs`; its block cases `tracker.rs` holds through `App`).

use factorium_domain::issue::{Asker, Author, IssueEvent, On, asked, question};

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
