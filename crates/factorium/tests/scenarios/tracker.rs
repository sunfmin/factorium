//! The checkout list and the tracker.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::fixture::{World, findings, refused};

fn numbers(list: &Value) -> Vec<(String, u64)> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|i| (i["checkout"].as_str().unwrap().to_string(), i["number"].as_u64().unwrap()))
        .collect()
}

#[tokio::test]
async fn a_checkout_is_listed_with_its_branches_and_real_path_and_gets_its_proving_branch() {
    let w = World::new();
    let web = w.repo("web");
    w.app.checkout_add("web", &web.join("..").join("web"), "main", "proving").await.unwrap();
    let list = w.app.checkout_list().await.unwrap();
    assert_eq!(
        list,
        json!([{ "name": "web", "path": std::fs::canonicalize(&web).unwrap(), "base": "main", "proving": "proving" }])
    );
    assert_eq!(w.remote_tip("web", "proving"), w.remote_tip("web", "main"));
}

#[tokio::test]
async fn a_checkout_is_refused_twice_without_a_path_an_origin_or_a_good_name_or_with_one_branch() {
    let w = World::new();
    w.checkout("web").await;
    refused(w.app.checkout_add("web", &w.repo("other"), "main", "proving").await, "listed");
    refused(w.app.checkout_add("gone", &w.dir.join("nowhere"), "main", "proving").await, "no_path");
    std::fs::create_dir_all(w.dir.join("plain")).unwrap();
    refused(w.app.checkout_add("plain", &w.dir.join("plain"), "main", "proving").await, "no_base");
    refused(w.app.checkout_add("a b", &w.repo("ab"), "main", "proving").await, "invalid_name");
    refused(w.app.checkout_add("one", &w.repo("one"), "main", "main").await, "proving_is_base");
    assert_eq!(w.app.checkout_list().await.unwrap().as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_removed_checkout_leaves_the_list_takes_no_issues_and_can_come_back() {
    let w = World::new();
    w.checkout("web").await;
    w.app.checkout_remove("web").await.unwrap();
    assert_eq!(w.app.checkout_list().await.unwrap(), json!([]));
    refused(w.app.checkout_remove("web").await, "not_listed");
    refused(w.app.issue_add("web", "t", "", false, &[]).await, "no_checkout");
    let again = w.repo("web2");
    w.app.checkout_add("web", &again, "main", "proving").await.unwrap();
    assert_eq!(w.app.checkout_list().await.unwrap()[0]["path"], json!(std::fs::canonicalize(again).unwrap()));
}

#[tokio::test]
async fn issues_are_numbered_per_checkout_from_one_and_the_next_run_reads_them() {
    let w = World::new();
    w.checkout("web").await;
    w.checkout("api").await;
    let first = w.app.issue_add("web", "  Fix the login  ", "body", false, &[]).await.unwrap();
    assert_eq!(
        (first["number"].clone(), first["title"].clone(), first["status"].clone()),
        (json!(1), json!("Fix the login"), json!("ready"))
    );
    w.app.issue_add("web", "second", "", false, &[]).await.unwrap();
    w.app.issue_add("api", "first of api", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "third", "", true, &[]).await.unwrap();
    assert_eq!(
        numbers(&w.app.issue_list(None, false).await.unwrap()),
        vec![("api".into(), 1), ("web".into(), 1), ("web".into(), 2), ("web".into(), 3)]
    );
    assert_eq!(w.app.issue_show("web", 3).await.unwrap()["status"], "waiting_for_human");
    // What one run records, the next run reads.
    let next = w.run();
    let read = numbers(&next.issue_list(None, false).await.unwrap());
    assert_eq!(
        read,
        vec![("api".into(), 1), ("web".into(), 1), ("web".into(), 2), ("web".into(), 3)],
        "the next run reads them"
    );
    assert_eq!(next.issue_add("web", "fourth", "", false, &[]).await.unwrap()["number"], 4, "and numbers on from them");
}

#[tokio::test]
async fn an_issue_needs_a_listed_checkout_and_a_title() {
    let w = World::new();
    w.checkout("web").await;
    refused(w.app.issue_add("nope", "t", "", false, &[]).await, "no_checkout");
    refused(w.app.issue_add("web", "   ", "", false, &[]).await, "no_title");
    refused(w.app.issue_show("web", 1).await, "no_issue");
    refused(w.app.issue_close("web", 1, "r").await, "no_issue");
    assert_eq!(w.app.issue_list(None, true).await.unwrap(), json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_runs_adding_at_once_take_different_numbers() {
    let w = World::new();
    w.checkout("web").await;
    let runs = [Arc::new(w.run()), Arc::new(w.run())];
    let adds: Vec<_> = (0..8)
        .map(|i| {
            let app = runs[i % 2].clone();
            tokio::spawn(async move { app.issue_add("web", "same time", "", false, &[]).await.unwrap() })
        })
        .collect();
    let mut got = Vec::new();
    for add in adds {
        got.push(add.await.unwrap()["number"].as_u64().unwrap());
    }
    got.sort();
    assert_eq!(got, (1..=8).collect::<Vec<_>>());
}

#[tokio::test]
async fn closed_issues_leave_the_list_unless_asked_for_and_their_numbers_are_not_reused() {
    let w = World::new();
    w.checkout("web").await;
    w.checkout("api").await;
    w.app.issue_add("web", "one", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "two", "", false, &[]).await.unwrap();
    w.app.issue_add("api", "three", "", false, &[]).await.unwrap();
    let closed = w.app.issue_close("web", 1, "done elsewhere").await.unwrap();
    assert_eq!((closed["status"].clone(), closed["reason"].clone()), (json!("closed"), json!("done elsewhere")));
    assert_eq!(numbers(&w.app.issue_list(None, false).await.unwrap()), vec![("api".into(), 1), ("web".into(), 2)]);
    assert_eq!(numbers(&w.app.issue_list(Some("web"), false).await.unwrap()), vec![("web".into(), 2)]);
    assert_eq!(
        numbers(&w.app.issue_list(Some("web"), true).await.unwrap()),
        vec![("web".into(), 1), ("web".into(), 2)]
    );
    assert_eq!(w.app.issue_add("web", "four", "", false, &[]).await.unwrap()["number"], 3);
}

#[tokio::test]
async fn an_edit_shows_in_the_list_and_keeps_what_it_does_not_name() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "old", "the body", false, &[]).await.unwrap();
    let e = w.app.issue_edit("web", 1, Some("new"), None).await.unwrap();
    assert_eq!((e["title"].clone(), e["body"].clone()), (json!("new"), json!("the body")));
    let e = w.app.issue_edit("web", 1, None, Some("another")).await.unwrap();
    assert_eq!((e["title"].clone(), e["body"].clone()), (json!("new"), json!("another")));
    refused(w.app.issue_edit("web", 1, Some(" "), None).await, "no_title");
    assert_eq!(w.app.issue_list(None, false).await.unwrap()[0]["title"], "new");
}

#[tokio::test]
async fn an_issue_waiting_for_a_human_is_listed_so_until_readied() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "one", "", false, &[]).await.unwrap();
    let p = w.app.issue_wait_for_human("web", 1, "needs a decision").await.unwrap();
    assert_eq!((p["status"].clone(), p["reason"].clone()), (json!("waiting_for_human"), json!("needs a decision")));
    assert_eq!(w.app.issue_list(None, false).await.unwrap()[0]["status"], "waiting_for_human");
    let r = w.app.issue_ready("web", 1).await.unwrap();
    assert_eq!((r["status"].clone(), r["reason"].clone()), (json!("ready"), json!(null)));
    assert_eq!(w.app.issue_list(None, false).await.unwrap()[0]["status"], "ready");
}

/// doctor's `asks`: every issue left to a person, oldest first, with its
/// question and who left it so, until `issue ready` or `issue close`.
#[tokio::test]
async fn doctor_asks_every_issue_left_to_a_person_oldest_first_until_readied_or_closed() {
    let w = World::new();
    w.checkout("web").await;
    let body = "Based on proving 1a2b.\n\nShip the new login now?\nIt is behind a flag.";
    w.app.issue_add("web", "Ship login", body, true, &[]).await.unwrap();
    w.advance(1);
    w.app.issue_add("web", "Pay for runners", "Buy two more?", false, &[]).await.unwrap();
    let watch = factorium::App::open(&w.db, factorium::Caller::Watch.by(), w.host.clone()).unwrap();
    watch.issue_wait_for_human("web", 2, "it costs money").await.unwrap();
    w.advance(1);
    w.app.issue_add("web", "Rename", "Call it guild?", false, &[]).await.unwrap();
    w.app.issue_wait_for_human("web", 3, "a name is the owner's").await.unwrap();
    let asks = w.doctor().await["asks"].clone();
    let shown: Vec<_> = (asks.as_array().unwrap().iter())
        .map(|a| (a["issue"].clone(), a["title"].clone(), a["question"].clone(), a["by"].clone(), a["why"].clone()))
        .collect();
    assert_eq!(
        shown,
        [
            (json!("web#1"), json!("Ship login"), json!("Ship the new login now?"), json!("added"), json!(null)),
            (json!("web#2"), json!("Pay for runners"), json!("Buy two more?"), json!("watch"), json!("it costs money")),
            (
                json!("web#3"),
                json!("Rename"),
                json!("Call it guild?"),
                json!("session"),
                json!("a name is the owner's")
            ),
        ],
        "{asks}"
    );
    let since: Vec<_> = asks.as_array().unwrap().iter().map(|a| a["since"].as_i64().unwrap()).collect();
    assert!(since[0] < since[1] && since[1] < since[2], "{asks}");
    assert!(asks[0]["utc"].as_str().is_some(), "{asks}");
    w.app.issue_ready("web", 1).await.unwrap();
    w.app.issue_close("web", 2, "not now").await.unwrap();
    let left: Vec<_> = w.doctor().await["asks"].as_array().unwrap().iter().map(|a| a["issue"].clone()).collect();
    assert_eq!(left, [json!("web#3")]);
}

/// doctor's `decided`: every Decided record in an issue's body (at `issue
/// add`) or comment, newest first, with every field, until `decided --read`;
/// a comment not in the shape is none.
#[tokio::test]
async fn doctor_lists_every_decided_record_newest_first_until_read() {
    let w = World::new();
    w.checkout("web").await;
    let body = "Based on proving 1a2b.\n\nRename the flag.\n\n\
        Decided (session, for Felix): call it guild\n\
        Options: A) guild  B) hall\n\
        Chose: A, because the repo is guild\n\
        Undo: issue edit the title back";
    w.app.issue_add("web", "Rename", body, false, &[]).await.unwrap();
    w.advance(1);
    w.app.issue_add("web", "Other", "No record here.", false, &[]).await.unwrap();
    w.app.issue_comment("web", 2, "measured 4.1s\nDecided (x, for Felix): not first").await.unwrap();
    w.advance(1);
    let record = "Decided (watch, for Felix): park web#2 behind web#1\n\
        Options: A) restart now  B) park it\n\
        Chose: B, because web#1 changes the same code\n\
        Undo: issue block web 2 web#1 --undo";
    w.app.issue_comment("web", 2, record).await.unwrap();
    let decided = w.doctor().await["decided"].clone();
    let at = 1_800_000_000_000_i64;
    assert_eq!(
        decided,
        json!([
            {
                "id": "D2", "issue": "web#2", "who": "watch", "what": "park web#2 behind web#1",
                "options": "A) restart now  B) park it", "chose": "B, because web#1 changes the same code",
                "undo": "issue block web 2 web#1 --undo", "at": at + 120_000,
                "utc": factorium_domain::when::utc(at + 120_000),
            },
            {
                "id": "D1", "issue": "web#1", "who": "session", "what": "call it guild",
                "options": "A) guild  B) hall", "chose": "A, because the repo is guild",
                "undo": "issue edit the title back", "at": at, "utc": factorium_domain::when::utc(at),
            },
        ]),
        "{decided}"
    );
    let week = w.app.decided_view(false, None).await.unwrap();
    assert_eq!(week["decided"], decided, "the last 7 days, read or not");
    w.app.decided_view(true, None).await.unwrap();
    w.app.issue_comment("web", 1, "Decided (session, for Felix): made as it was read").await.unwrap();
    assert_eq!(w.doctor().await["decided"], json!([]), "read, up to and at its time");
    let week = w.app.decided_view(false, None).await.unwrap()["decided"].as_array().unwrap().clone();
    assert_eq!(week[0]["what"], "made as it was read", "still in the last 7 days");
    assert_eq!(week[1..], decided.as_array().unwrap()[..], "still in the last 7 days");
    w.advance(1);
    w.app.issue_comment("web", 1, "Decided (session, for Felix): keep hall too").await.unwrap();
    let after: Vec<_> = w.doctor().await["decided"].as_array().unwrap().iter().map(|d| d["what"].clone()).collect();
    assert_eq!(after, [json!("keep hall too")], "only what came after the read");
    w.advance(7 * 24 * 60 - 1);
    let week: Vec<_> = (w.app.decided_view(false, None).await.unwrap()["decided"].as_array().unwrap().iter())
        .map(|d| d["what"].clone())
        .collect();
    assert_eq!(week, [json!("keep hall too")], "7 days back, no more: the rest are 7 days and 1 minute old");
}

/// A body that quotes the record's shape in a fenced code block is not a
/// record there; a real one in the same body, outside the fence, is.
#[tokio::test]
async fn doctor_does_not_list_a_decided_shape_quoted_in_a_fence() {
    let w = World::new();
    w.checkout("web").await;
    let body = "Write records in this shape:\n\n```\n```text, a fence with text, closes none\n\
        Decided (<who>, for Felix): <what, one line>\n\
        Options: A) …  B) …\n\
        Undo: <how to take it back>\n```\n\n\
        Decided (session, for Felix): call it guild\nUndo: rename it back";
    w.app.issue_add("web", "Shape", body, false, &[]).await.unwrap();
    let what: Vec<_> = w.doctor().await["decided"].as_array().unwrap().iter().map(|d| d["what"].clone()).collect();
    assert_eq!(what, [json!("call it guild")]);
}

/// A body line that starts with inline code (one backtick) opens no fence,
/// and doctor answers `decided` from that body without failing (factorium#239).
#[tokio::test]
async fn doctor_reads_a_body_line_starting_with_inline_code() {
    let w = World::new();
    w.checkout("web").await;
    let body = "`factorium doctor` answers `decided`: the records.\n\n\
        Decided (session, for Felix): call it guild\nUndo: rename it back";
    w.app.issue_add("web", "Inline", body, false, &[]).await.unwrap();
    let what: Vec<_> = w.doctor().await["decided"].as_array().unwrap().iter().map(|d| d["what"].clone()).collect();
    assert_eq!(what, [json!("call it guild")]);
}

#[tokio::test]
async fn an_issue_waits_for_others_of_its_checkout() {
    let w = World::new();
    w.checkout("web").await;
    for t in ["one", "two", "three"] {
        w.app.issue_add("web", t, "", false, &[]).await.unwrap();
    }
    w.app.issue_block("web", 3, 1, false).await.unwrap();
    w.app.issue_block("web", 3, 1, false).await.unwrap();
    assert_eq!(w.app.issue_block("web", 3, 2, false).await.unwrap()["blocked_by"], json!([1, 2]));
    refused(w.app.issue_block("web", 3, 3, false).await, "blocks_itself");
    refused(w.app.issue_block("web", 3, 9, false).await, "no_issue");
    refused(w.app.issue_block("web", 9, 1, false).await, "no_issue");
    assert_eq!(w.app.issue_block("web", 3, 1, true).await.unwrap()["blocked_by"], json!([2]));
    assert_eq!(w.app.issue_block("web", 3, 1, true).await.unwrap()["blocked_by"], json!([2]));
}

/// A db written before the state was named for who resumes it is rewritten
/// by `packaging/migrate-wait-for-human.sql`, and then reads as before.
#[tokio::test]
async fn the_sql_rewrites_a_db_kept_with_parked_issues_to_the_new_names() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "one", "", true, &[]).await.unwrap();
    w.app.issue_add("web", "two", "", false, &[]).await.unwrap();
    w.app.issue_wait_for_human("web", 2, "needs a decision").await.unwrap();
    w.app.issue_add("web", "three", "", false, &[]).await.unwrap();
    w.issues_as_code_before_95_wrote_them();
    assert_ne!(w.names_before_95(), 0, "the old names are in");
    let migration = include_str!("../../../../packaging/migrate-wait-for-human.sql");
    w.migrate(migration);
    w.migrate(migration);
    assert_eq!(w.names_before_95(), 0, "no old name is left");
    let app = w.run();
    for (n, status, reason) in [
        (1, "waiting_for_human", json!(null)),
        (2, "waiting_for_human", json!("needs a decision")),
        (3, "ready", json!(null)),
    ] {
        let shown = app.issue_show("web", n).await.unwrap();
        assert_eq!((shown["status"].clone(), shown["reason"].clone()), (json!(status), reason), "#{n}");
    }
    let listed = app.issue_list(None, false).await.unwrap();
    let statuses: Vec<_> = listed.as_array().unwrap().iter().map(|i| i["status"].clone()).collect();
    assert_eq!(statuses, [json!("waiting_for_human"), json!("waiting_for_human"), json!("ready")]);
    assert_eq!(app.issue_ready("web", 2).await.unwrap()["status"], "ready");
}

/// An event the tracker cannot read (a binary's bug, or one the running
/// binary does not know) is set aside as a dead letter, and doctor names it.
#[tokio::test]
async fn an_event_the_tracker_cannot_read_is_a_dead_letter_doctor_names() {
    let w = World::new();
    w.kit().await;
    w.a_dead_letter_for_the_tracker();
    let found = w.app.doctor().await.unwrap();
    let dead = findings(&found).iter().find(|f| f["key"] == "dead:tracker");
    assert_eq!(dead.expect("named")["what"], "projection tracker has 1 dead letters", "{found}");
}

/// Each Decided record has a host-wide number given once, in the order
/// records come: a comment's and a body's alike. An edit of the body keeps
/// the numbers of the records it keeps; `decided D<n>` answers one, old or
/// read; one that names no record is refused.
#[tokio::test]
async fn each_decided_record_has_a_host_wide_number_given_once() {
    let w = World::new();
    w.checkout("web").await;
    w.checkout("api").await;
    let body =
        "Rename.\n\nDecided (session, for Felix): call it guild\nUndo: x\n\nDecided (session, for Felix): keep hall\n";
    w.app.issue_add("web", "Rename", body, false, &[]).await.unwrap();
    w.advance(1);
    w.app.issue_comment("api", 1, "Decided (watch, for Felix): park it").await.unwrap_err();
    w.app.issue_add("api", "Other", "none", false, &[]).await.unwrap();
    let shown = w.app.issue_comment("api", 1, "Decided (watch, for Felix): park it").await.unwrap();
    assert_eq!(shown["comments"][0]["decided"], "D3", "{shown}");
    w.app.issue_comment("api", 1, "measured 4.1s").await.unwrap();
    let shown = w.app.issue_show("api", 1).await.unwrap();
    assert_eq!(shown["comments"][1].get("decided"), None, "not a record: {shown}");
    let ids =
        |v: &Value| v.as_array().unwrap().iter().map(|d| (d["id"].clone(), d["what"].clone())).collect::<Vec<_>>();
    let all = w.app.decided_view(false, None).await.unwrap()["decided"].clone();
    assert_eq!(
        ids(&all),
        [(json!("D3"), json!("park it")), (json!("D2"), json!("keep hall")), (json!("D1"), json!("call it guild"))],
        "{all}"
    );
    // The edit drops one record, keeps another (its Undo changed), and adds one.
    let edited = "Rename.\n\nDecided (session, for Felix): name it lore\n\nDecided (session, for Felix): call it guild\nUndo: y\n";
    let shown = w.app.issue_edit("web", 1, None, Some(edited)).await.unwrap();
    assert_eq!(shown["decided"], json!(["D4", "D1"]), "kept as it was, the new one next: {shown}");
    w.app.issue_edit("web", 1, Some("Renamed"), None).await.unwrap();
    assert_eq!(w.app.issue_show("web", 1).await.unwrap()["decided"], json!(["D4", "D1"]), "a title edit keeps them");
    w.app.decided_view(true, None).await.unwrap();
    w.advance(8 * 24 * 60);
    assert_eq!(w.app.decided_view(false, None).await.unwrap()["decided"], json!([]), "older than 7 days");
    let one = w.app.decided_view(false, Some("D1")).await.unwrap();
    assert_eq!(
        (one["id"].clone(), one["what"].clone(), one["undo"].clone()),
        (json!("D1"), json!("call it guild"), json!("y"))
    );
    assert_eq!(w.app.decided_view(false, Some("3")).await.unwrap()["issue"], "api#1", "read and old, named bare");
    let none = w.app.decided_view(false, Some("D99999")).await.unwrap_err();
    assert_eq!(none.code, "no_record", "{none:?}");
    assert_eq!(w.app.decided_view(false, Some("rolesmith#4")).await.unwrap_err().code, "bad_id");
}

/// Records made before numbers were given take theirs once, oldest first by
/// when they were made (a body's, when its issue was filed), a closed
/// issue's among them; a record made after takes the next.
#[tokio::test]
async fn records_made_before_numbers_take_theirs_oldest_first() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "One", "Decided (a, for Felix): first\n", false, &[]).await.unwrap();
    w.advance(1);
    w.app.issue_add("web", "Two", "plain", false, &[]).await.unwrap();
    w.app.issue_comment("web", 2, "Decided (b, for Felix): second").await.unwrap();
    w.advance(1);
    w.app.issue_comment("web", 1, "Decided (c, for Felix): third").await.unwrap();
    w.app.issue_close("web", 1, "done").await.unwrap();
    w.records_as_code_before_243_wrote_them();
    let shown = w.app.issue_show("web", 2).await.unwrap();
    assert_eq!(shown["comments"][0].get("decided"), None, "none yet: {shown}");
    w.app.issue_comment("web", 2, "Decided (d, for Felix): after").await.unwrap();
    let all = w.app.decided_view(false, None).await.unwrap()["decided"].clone();
    let ids: Vec<_> = all.as_array().unwrap().iter().map(|d| (d["id"].clone(), d["what"].clone())).collect();
    assert_eq!(
        ids,
        [
            (json!("D4"), json!("after")),
            (json!("D3"), json!("third")),
            (json!("D2"), json!("second")),
            (json!("D1"), json!("first"))
        ],
        "{all}"
    );
    let closed = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!(
        (closed["decided"].clone(), closed["comments"][0]["decided"].clone()),
        (json!(["D1"]), json!("D3")),
        "{closed}"
    );
    w.app.decided_view(false, None).await.unwrap();
    assert_eq!(w.app.decided_view(false, None).await.unwrap()["decided"], all, "given once");
}
