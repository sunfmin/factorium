//! The checkout list and the tracker.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::{World, refused};

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
async fn issues_are_numbered_per_checkout_from_one() {
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

#[tokio::test]
async fn what_one_run_records_the_next_run_reads() {
    let w = World::new();
    w.checkout("web").await;
    w.app.issue_add("web", "one", "", false, &[]).await.unwrap();
    let next = w.run();
    assert_eq!(numbers(&next.issue_list(None, false).await.unwrap()), vec![("web".into(), 1)]);
    assert_eq!(next.issue_add("web", "two", "", false, &[]).await.unwrap()["number"], 2);
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
    let sql = |text: &str| {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new("sqlite3")
            .arg(&w.db)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        let ran = child.wait_with_output().unwrap();
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
        String::from_utf8(ran.stdout).unwrap()
    };
    // Rewritten as the code before #95 wrote it.
    let old = r#"
        UPDATE events SET kind = 'IssueParked' WHERE kind = 'IssueWaitingForHuman';
        UPDATE events SET data = json_remove(json_set(data, '$.parked', json(CASE WHEN
            json_extract(data, '$.waiting_for_human') THEN 'true' ELSE 'false' END)), '$.waiting_for_human')
            WHERE kind = 'IssueOpened';
        UPDATE events SET meta = json_set(meta, '$.command', 'issue.park')
            WHERE json_extract(meta, '$.command') = 'issue.wait_for_human';
        UPDATE "rows:tracker:issues" SET row = json_set(row, '$.status', 'parked')
            WHERE json_extract(row, '$.status') = 'waiting_for_human';
        UPDATE snapshots SET state = replace(state, '"waiting_for_human"', '"parked"');
        UPDATE answers SET answer = replace(replace(answer, '"IssueWaitingForHuman"', '"IssueParked"'),
            '"waiting_for_human":', '"parked":');"#;
    sql(old);
    let count = "SELECT (SELECT count(*) FROM events WHERE kind = 'IssueParked' OR data LIKE '%\"parked\":%' \
                 OR meta LIKE '%issue.park\"%') + (SELECT count(*) FROM \"rows:tracker:issues\" WHERE status = \
                 'parked') + (SELECT count(*) FROM answers WHERE answer LIKE '%parked%');";
    assert_ne!(sql(count).trim(), "0", "the old names are in");
    let migration = include_str!("../../../../packaging/migrate-wait-for-human.sql");
    sql(migration);
    sql(migration);
    assert_eq!(sql(count).trim(), "0", "no old name is left");
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
    let db = rusqlite::Connection::open(&w.db).unwrap();
    let (event, stream) = (uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
    db.execute(
        "INSERT INTO events (id, stream, version, kind, type_version, data, meta) \
         SELECT ?1, ?2, 1, 'CheckoutAdded', 1, '{}', meta FROM events LIMIT 1",
        [event.to_string(), format!("checkout/{stream}")],
    )
    .unwrap();
    let found = w.app.doctor().await.unwrap();
    let dead = found["findings"].as_array().unwrap().iter().find(|f| f["key"] == "dead:tracker");
    assert_eq!(dead.expect("named")["what"], "projection tracker has 1 dead letters", "{found}");
}
