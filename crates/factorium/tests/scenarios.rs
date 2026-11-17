//! Scenarios: each drives `App` as the CLI does and reads only what the
//! CLI would print.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use factorium::{Answer, App};
use factum::{Instant, ManualClock, Uuid};
use serde_json::{Value, json};

/// A fresh directory for one scenario: its database and its repos.
struct Host {
    dir: PathBuf,
    clock: Arc<ManualClock>,
}

impl Host {
    fn new() -> Host {
        let dir = std::env::temp_dir()
            .join("factorium-scenarios")
            .join(Uuid::now_v7().to_string());
        std::fs::create_dir_all(dir.join("repos")).unwrap();
        Host {
            dir,
            clock: Arc::new(ManualClock::new(Instant(1_800_000_000_000))),
        }
    }

    fn db(&self) -> PathBuf {
        self.dir.join("data").join("factorium.db")
    }

    /// The app as one CLI run opens it.
    fn app(&self) -> App {
        App::open(&self.db(), self.clock.clone(), "owner").unwrap()
    }

    fn repo(&self, name: &str) -> PathBuf {
        let p = self.dir.join("repos").join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn refused(answer: Answer, code: &str) {
    match answer {
        Err(r) => assert_eq!(r.code, code, "{r}"),
        Ok(v) => panic!("expected {code}, got {v}"),
    }
}

async fn with_checkout(host: &Host, name: &str) -> App {
    let app = host.app();
    app.checkout_add(name, &host.repo(name), "main", "proving")
        .await
        .unwrap();
    app
}

fn numbers(list: &Value) -> Vec<(String, u64)> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["checkout"].as_str().unwrap().to_string(),
                i["number"].as_u64().unwrap(),
            )
        })
        .collect()
}

// ---- checkouts ----

#[tokio::test]
async fn a_checkout_is_listed_with_its_branches_and_real_path() {
    let host = Host::new();
    let app = host.app();
    let repo = host.repo("web");
    app.checkout_add("web", &repo.join("..").join("web"), "main", "proving")
        .await
        .unwrap();
    app.checkout_add("api", &host.repo("api"), "trunk", "staging")
        .await
        .unwrap();

    let real = std::fs::canonicalize(&repo).unwrap();
    assert_eq!(
        app.checkout_list().await.unwrap(),
        json!([
            { "name": "api", "path": std::fs::canonicalize(host.repo("api")).unwrap(), "base": "trunk", "proving": "staging" },
            { "name": "web", "path": real, "base": "main", "proving": "proving" },
        ])
    );
}

#[tokio::test]
async fn a_checkout_is_refused_twice_without_a_path_or_with_a_bad_name_or_one_branch() {
    let host = Host::new();
    let app = with_checkout(&host, "web").await;
    refused(
        app.checkout_add("web", &host.repo("other"), "main", "proving")
            .await,
        "listed",
    );
    refused(
        app.checkout_add("gone", &host.dir.join("nowhere"), "main", "proving")
            .await,
        "no_path",
    );
    refused(
        app.checkout_add("a b", &host.repo("ab"), "main", "proving")
            .await,
        "invalid_name",
    );
    refused(
        app.checkout_add("", &host.repo("e"), "main", "proving")
            .await,
        "invalid_name",
    );
    refused(
        app.checkout_add("one", &host.repo("one"), "main", "main")
            .await,
        "proving_is_base",
    );
    assert_eq!(
        app.checkout_list().await.unwrap().as_array().unwrap().len(),
        1
    );
}

#[tokio::test]
async fn a_removed_checkout_leaves_the_list_and_can_come_back() {
    let host = Host::new();
    let app = with_checkout(&host, "web").await;
    app.checkout_remove("web").await.unwrap();
    assert_eq!(app.checkout_list().await.unwrap(), json!([]));
    refused(app.checkout_remove("web").await, "not_listed");
    refused(app.issue_add("web", "t", "", false).await, "no_checkout");

    app.checkout_add("web", &host.repo("web2"), "main", "proving")
        .await
        .unwrap();
    let list = app.checkout_list().await.unwrap();
    assert_eq!(
        list[0]["path"],
        json!(std::fs::canonicalize(host.repo("web2")).unwrap())
    );
}

// ---- issues ----

#[tokio::test]
async fn issues_are_numbered_per_checkout_from_one() {
    let host = Host::new();
    let app = with_checkout(&host, "web").await;
    app.checkout_add("api", &host.repo("api"), "main", "proving")
        .await
        .unwrap();

    let first = app
        .issue_add("web", "  Fix the login  ", "body", false)
        .await
        .unwrap();
    assert_eq!(
        first,
        json!({ "checkout": "web", "number": 1, "title": "Fix the login", "body": "body",
                "status": "ready", "blocked_by": [], "reason": null })
    );
    app.issue_add("web", "second", "", false).await.unwrap();
    app.issue_add("api", "first of api", "", false)
        .await
        .unwrap();
    app.issue_add("web", "third", "", true).await.unwrap();

    assert_eq!(
        numbers(&app.issue_list(None, false).await.unwrap()),
        vec![
            ("api".into(), 1),
            ("web".into(), 1),
            ("web".into(), 2),
            ("web".into(), 3)
        ]
    );
    assert_eq!(app.issue_show("web", 3).await.unwrap()["status"], "parked");
}

#[tokio::test]
async fn an_issue_needs_a_listed_checkout_and_a_title() {
    let host = Host::new();
    let app = with_checkout(&host, "web").await;
    refused(app.issue_add("nope", "t", "", false).await, "no_checkout");
    refused(app.issue_add("web", "   ", "", false).await, "no_title");
    refused(app.issue_show("web", 1).await, "no_issue");
    refused(app.issue_close("web", 1, "r").await, "no_issue");
    assert_eq!(app.issue_list(None, true).await.unwrap(), json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_runs_adding_at_once_take_different_numbers() {
    let host = Host::new();
    with_checkout(&host, "web").await;
    let runs = [Arc::new(host.app()), Arc::new(host.app())];
    let adds: Vec<_> = (0..8)
        .map(|i| {
            let app = runs[i % 2].clone();
            tokio::spawn(async move { app.issue_add("web", "same time", "", false).await.unwrap() })
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
async fn closed_issues_leave_the_list_unless_asked_for() {
    let host = Host::new();
    let app = with_checkout(&host, "web").await;
    app.checkout_add("api", &host.repo("api"), "main", "proving")
        .await
        .unwrap();
    app.issue_add("web", "one", "", false).await.unwrap();
    app.issue_add("web", "two", "", false).await.unwrap();
    app.issue_add("api", "three", "", false).await.unwrap();

    let closed = app.issue_close("web", 1, "done elsewhere").await.unwrap();
    assert_eq!(
        (closed["status"].clone(), closed["reason"].clone()),
        (json!("closed"), json!("done elsewhere"))
    );
    assert_eq!(
        numbers(&app.issue_list(None, false).await.unwrap()),
        vec![("api".into(), 1), ("web".into(), 2)]
    );
    assert_eq!(
        numbers(&app.issue_list(Some("web"), false).await.unwrap()),
        vec![("web".into(), 2)]
    );
    assert_eq!(
        numbers(&app.issue_list(Some("web"), true).await.unwrap()),
        vec![("web".into(), 1), ("web".into(), 2)]
    );
    // A closed number is never handed out again.
    assert_eq!(
        app.issue_add("web", "four", "", false).await.unwrap()["number"],
        3
    );
}

#[tokio::test]
async fn an_edit_shows_in_the_list_and_keeps_what_it_does_not_name() {
    let host = Host::new();
    let app = with_checkout(&host, "web").await;
    app.issue_add("web", "old", "the body", false)
        .await
        .unwrap();
    let edited = app.issue_edit("web", 1, Some("new"), None).await.unwrap();
    assert_eq!(
        (edited["title"].clone(), edited["body"].clone()),
        (json!("new"), json!("the body"))
    );
    let edited = app
        .issue_edit("web", 1, None, Some("another"))
        .await
        .unwrap();
    assert_eq!(
        (edited["title"].clone(), edited["body"].clone()),
        (json!("new"), json!("another"))
    );
    refused(app.issue_edit("web", 1, Some(" "), None).await, "no_title");
    assert_eq!(
        app.issue_list(None, false).await.unwrap()[0]["title"],
        "new"
    );
}

#[tokio::test]
async fn a_parked_issue_is_parked_in_the_list_until_readied() {
    let host = Host::new();
    let app = with_checkout(&host, "web").await;
    app.issue_add("web", "one", "", false).await.unwrap();
    let parked = app.issue_park("web", 1, "needs a decision").await.unwrap();
    assert_eq!(
        (parked["status"].clone(), parked["reason"].clone()),
        (json!("parked"), json!("needs a decision"))
    );
    assert_eq!(
        app.issue_list(None, false).await.unwrap()[0]["status"],
        "parked"
    );

    let ready = app.issue_ready("web", 1).await.unwrap();
    assert_eq!(
        (ready["status"].clone(), ready["reason"].clone()),
        (json!("ready"), json!(null))
    );
    assert_eq!(
        app.issue_list(None, false).await.unwrap()[0]["status"],
        "ready"
    );
}

#[tokio::test]
async fn an_issue_waits_for_others_of_its_checkout() {
    let host = Host::new();
    let app = with_checkout(&host, "web").await;
    for t in ["one", "two", "three"] {
        app.issue_add("web", t, "", false).await.unwrap();
    }
    app.issue_block("web", 3, 1, false).await.unwrap();
    app.issue_block("web", 3, 1, false).await.unwrap();
    assert_eq!(
        app.issue_block("web", 3, 2, false).await.unwrap()["blocked_by"],
        json!([1, 2])
    );
    refused(app.issue_block("web", 3, 3, false).await, "blocks_itself");
    refused(app.issue_block("web", 3, 9, false).await, "no_issue");
    refused(app.issue_block("web", 9, 1, false).await, "no_issue");

    assert_eq!(
        app.issue_block("web", 3, 1, true).await.unwrap()["blocked_by"],
        json!([2])
    );
    assert_eq!(
        app.issue_block("web", 3, 1, true).await.unwrap()["blocked_by"],
        json!([2])
    );
}

#[tokio::test]
async fn what_one_run_records_the_next_run_reads() {
    let host = Host::new();
    {
        let app = with_checkout(&host, "web").await;
        app.issue_add("web", "one", "", false).await.unwrap();
    }
    let app = host.app();
    assert_eq!(
        numbers(&app.issue_list(None, false).await.unwrap()),
        vec![("web".into(), 1)]
    );
    assert_eq!(
        app.issue_add("web", "two", "", false).await.unwrap()["number"],
        2
    );
    assert!(Path::new(&host.db()).exists());
}
