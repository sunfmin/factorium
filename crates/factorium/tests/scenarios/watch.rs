//! The watch: its session, its look, episodes and rounds, its report, and
//! what it does with its commands.

use factorium::host::Host;
use factorium_harness::RULES;
use serde_json::json;

use crate::fixture::{CLAUDE, World, id, worker};

/// A checkout, a kit, a worker that escalated, and the watch's session up.
async fn escalated(w: &World) -> serde_json::Value {
    let wk = escalated_alone(w).await;
    w.watch_session(false);
    wk
}

/// A checkout, a kit, a worker that escalated; the watch's terminal opened.
async fn escalated_alone(w: &World) -> serde_json::Value {
    let wk = w.working(CLAUDE, "t").await;
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    wk
}

#[tokio::test]
async fn no_round_is_typed_before_the_watch_s_session_is_up_and_idle() {
    let w = World::new();
    escalated_alone(&w).await;
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"], json!(null), "its session is not up yet");
    w.watch_session(true);
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"], json!(null), "its turn is open");
    assert!(w.terminal(&watch_terminal(&w)).sent.is_empty());
    w.watch_session(false);
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"]["n"], 1);
}

fn watch_terminal(w: &World) -> String {
    let t = w.host.orca.terminals.lock().unwrap();
    t.iter().find(|(_, t)| t.title == "factorium watch").map(|(h, _)| h.clone()).expect("the watch's terminal")
}

#[tokio::test]
async fn an_escalation_becomes_an_episode_and_one_round_sent_to_the_watch() {
    let w = World::new();
    let wk = escalated(&w).await;
    w.advance(5);
    let t = w.tick().await;
    assert_eq!(t.watch.as_ref().unwrap()["round"]["n"], 1);
    let term = w.terminal(&watch_terminal(&w));
    assert!(
        term.line.contains(&*w.data().watch_brief().to_string_lossy()),
        "the watch is handed its brief: {}",
        term.line
    );
    assert!(term.line.contains("FACTORIUM_WATCH"));
    let round = term.sent.last().unwrap();
    assert!(round.contains("round 1") && round.contains("which API version?"), "{round}");

    let next = w.app.watch_next().await.unwrap();
    assert!(next["episode"]["what"].as_str().unwrap().contains(wk["worker"].as_str().unwrap()));

    // The episode stands, but it already had its round.
    w.app.watch_report("Answered: v2.", None).await.unwrap();
    w.advance(5);
    w.tick().await;
    assert_eq!(w.app.watch_next().await.unwrap()["round"], json!(null));
    assert_eq!(w.terminal(&watch_terminal(&w)).sent.len(), 1);

    // Sent back to work, the worker's episode closes.
    w.app.watch_restart(id(&wk), "Use v2.", false).await.unwrap();
    assert_eq!(worker(&w.app).await["status"], "working");
    assert!(w.terminal(wk["terminal"].as_str().unwrap()).sent.last().unwrap().contains("Use v2."));
    w.advance(5);
    w.tick().await;
    assert_eq!(w.app.watch_next().await.unwrap()["episodes"], json!({}));
}

#[tokio::test]
async fn a_report_ends_only_a_running_round_and_its_owner_part_reaches_doctor_once() {
    let w = World::new();
    crate::fixture::refused(w.app.watch_report("nothing", None).await, "no_round");
    escalated(&w).await;
    w.advance(5);
    w.tick().await;
    crate::fixture::refused(w.app.watch_report(" ", None).await, "no_text");
    w.app.watch_report("The account ran out.", Some("buy more Codex credit")).await.unwrap();
    assert_eq!(w.app.doctor().await.unwrap()["human"], json!(["buy more Codex credit"]));
    assert_eq!(w.app.doctor().await.unwrap()["human"], json!([]), "seen once");
}

#[tokio::test]
async fn a_round_past_its_bound_ends_and_the_next_episode_gets_one() {
    let w = World::new();
    escalated(&w).await;
    w.advance(5);
    w.tick().await;
    w.app.issue_add("web", "u", "", false, &[]).await.unwrap();
    w.tick().await;
    let second = w.worker_on("web", 2).await;
    w.app.worker_escalate(id(&second), "stuck too").await.unwrap();
    w.advance(5);
    w.tick().await;
    assert_eq!(w.app.watch_next().await.unwrap()["round"]["n"], 1, "one round at a time");
    w.advance(61);
    w.tick().await;
    let next = w.app.watch_next().await.unwrap();
    assert_eq!(next["round"]["n"], 2);
    assert!(next["episode"]["what"].as_str().unwrap().contains("stuck too"));
}

#[tokio::test]
async fn restarting_a_worker_whose_session_is_gone_starts_its_issue_again() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    w.kill_terminal(wk["terminal"].as_str().unwrap());
    w.tick().await;
    assert!(!crate::fixture::closed(&w.app, wk["terminal"].as_str().unwrap()).await, "its tab stays until closed");
    let r = w.app.watch_restart(id(&wk), "go on", false).await.unwrap();
    assert_eq!(r["ended"], true);
    assert!(crate::fixture::closed(&w.app, wk["terminal"].as_str().unwrap()).await);
    let again = w.tick().await.started.unwrap();
    assert_eq!(again["issue"], "web#1");
    assert_ne!(again["worker"], wk["worker"]);
}

/// One read answers whether a worker is live: a worker's own command and
/// the watch's refuse an ended worker alike, and keep nothing on its issue.
#[tokio::test]
async fn an_ended_worker_is_refused_alike_by_its_own_commands_and_the_watch_s() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    w.kill_terminal(wk["terminal"].as_str().unwrap());
    assert_eq!(w.app.watch_restart(id(&wk), "go on", false).await.unwrap()["ended"], true);
    let kept = w.comments("web", 1).await.len();
    let refused = crate::fixture::refused;
    refused(w.app.worker_done(id(&wk), None).await, "ended");
    refused(w.app.watch_restart(id(&wk), "go on", false).await, "ended");
    refused(w.app.watch_tell(id(&wk), "hello", false).await, "ended");
    assert_eq!(w.comments("web", 1).await.len(), kept, "nothing is kept for an ended worker");
}

#[tokio::test]
async fn a_new_watch_session_gets_the_round_that_is_running() {
    let w = World::new();
    escalated(&w).await;
    w.advance(5);
    w.tick().await;
    let first = watch_terminal(&w);
    assert_eq!(w.terminal(&first).sent.len(), 1);
    w.kill_terminal(&first);
    w.advance(5);
    let t = w.tick().await.watch.unwrap();
    let second = t["watch"].as_str().unwrap().to_string();
    assert_ne!(second, first, "a new session");
    assert_eq!(t["round"]["n"], 1, "the same round");
    let sent = w.terminal(&second).sent;
    assert!(sent.len() == 1 && sent[0].contains("round 1"), "{sent:?}");
    w.advance(5);
    w.tick().await;
    assert_eq!(w.terminal(&second).sent.len(), 1, "once per session");
}

#[tokio::test]
async fn while_orca_gives_no_answer_the_watch_ends_no_worker_and_starts_no_second_session() {
    let w = World::new();
    let wk = escalated(&w).await;
    w.advance(5);
    w.tick().await;
    *w.host.orca.down.lock().unwrap() = Some("orca: the runtime is not running".into());
    crate::fixture::refused(w.app.watch_restart(id(&wk), "go on", false).await, "orca");
    w.advance(5);
    w.tick().await;
    *w.host.orca.down.lock().unwrap() = None;
    let sessions = w.host.orca.terminals.lock().unwrap().values().filter(|t| t.title == "factorium watch").count();
    assert_eq!(sessions, 1, "its session may yet run");
    assert_eq!(worker(&w.app).await["status"], "waiting", "the worker is not ended");
}

#[tokio::test]
async fn the_worker_and_the_watch_start_alike_each_as_its_role() {
    let w = World::on("other.db");
    w.host.tokens.lock().unwrap().insert(
        "agent-account-team".into(),
        r#"{"fields":{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk-ant-very-secret"}}"#.into(),
    );
    let wk = escalated_alone(&w).await;
    let db = w.db.to_string_lossy().into_owned();
    let bin = std::env::current_exe().unwrap().parent().unwrap().to_string_lossy().into_owned();
    let worker = w.terminal(wk["terminal"].as_str().unwrap()).line;
    let watch = w.terminal(&watch_terminal(&w)).line;
    assert_eq!(wk["account"], "team");
    for (role, line) in [("worker", &worker), ("watch", &watch)] {
        assert!(line.contains(&format!("FACTORIUM_DB='{db}'")), "{role}: the app's database: {line}");
        assert!(line.contains(&format!("PATH='{bin}:")) || line.contains(&format!("PATH={bin}:")), "{role}: {line}");
        assert!(line.contains(" guard claude-code"), "{role}: guarded: {line}");
        assert!(!line.contains("sk-ant-very-secret"), "{role}: {line}");
        // What the shell does with the line's start: read the secret, remove the file.
        let parts: Vec<&str> = line.splitn(3, " && ").collect();
        let file = parts[0].strip_prefix(". ").unwrap().trim_matches('\'').to_string();
        let shell = format!("{} && {} && printenv CLAUDE_CODE_OAUTH_TOKEN", parts[0], parts[1]);
        let out = std::process::Command::new("sh").args(["-c", &shell]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "sk-ant-very-secret", "{role}");
        assert!(!std::path::Path::new(&file).exists(), "{role}");
    }
    // Each is named as its role, and only as its role.
    assert!(worker.contains("FACTORIUM_WORKER=") && !worker.contains("FACTORIUM_WATCH="), "{worker}");
    assert!(watch.contains("FACTORIUM_WATCH=") && !watch.contains("FACTORIUM_WORKER="), "{watch}");
    let rules = std::fs::read_to_string(w.data().watch().join(RULES)).unwrap();
    assert!(rules.contains(&format!("The database is {db}.")), "{rules}");
    let rules = std::fs::read_to_string(w.data().worker_role().join(RULES)).unwrap();
    assert!(rules.contains("You are a factorium worker"), "{rules}");
}

#[tokio::test]
async fn the_watch_looks_once_every_look_every() {
    let w = World::new();
    assert!(w.tick().await.watch.is_some(), "the first tick looks");
    assert!(w.tick().await.watch.is_none());
    w.advance(4);
    assert!(w.tick().await.watch.is_none(), "4 of 5 minutes");
    w.advance(1);
    assert!(w.tick().await.watch.is_some());
}

#[tokio::test]
async fn a_watch_home_orca_never_finds_is_given_up_after_five_more_tries() {
    let w = World::new();
    *w.host.orca.never_found.lock().unwrap() = true;
    w.checkout("web").await;
    w.kit().await;
    // The first tick looks, and the look opens the watch's session.
    let failed = w.app.tick().await;
    let ran = w.host.ran.lock().unwrap().clone();
    let home = w.data().watch();
    let by_repo = ran.iter().filter(|r| r.starts_with("orca terminal create --worktree id:")).count();
    assert_eq!(by_repo, 6, "by its repo, then five tries more: {failed:?} {ran:#?}");
    let slept = ran.iter().filter(|r| *r == "sleep 1").count();
    assert_eq!(slept, 5, "a second between tries");
    assert!(ran.iter().any(|r| r.starts_with(&format!("orca repo add --path {}", home.display()))));
}

#[tokio::test]
async fn a_watch_whose_rules_cannot_be_written_is_refused_io_and_no_terminal_opens() {
    let w = World::new();
    w.kit().await;
    // Its home a repository already; where its rules go, a directory stands.
    std::fs::create_dir_all(w.data().watch().join(".git")).unwrap();
    std::fs::create_dir_all(w.data().watch().join(RULES).join("in-the-way")).unwrap();
    let r = w.app.tick().await.expect_err("the look fails");
    assert_eq!(r.code, "io", "{r}");
    assert!(w.host.orca.terminals.lock().unwrap().is_empty(), "no session without its rules");
}

/// A session file's stamp after any tell a scenario sends.
pub(crate) const NEXT_CENTURY: &str = "2100-01-01T00:00:00.000Z";

#[tokio::test]
async fn a_tell_orca_saw_no_turn_for_is_submitted_when_claude_code_s_session_took_it() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let (handle, cwd) = (wk["terminal"].as_str().unwrap(), wk["worktree"].as_str().unwrap());
    w.set_busy(handle, true, true);
    let line = |kind: &str, at: &str, body: serde_json::Value| {
        let mut l = json!({ "type": kind, "timestamp": at, "cwd": cwd, "isSidechain": false });
        l.as_object_mut().unwrap().extend(body.as_object().unwrap().clone());
        l.to_string()
    };
    let said = "factorium (the watch): look at the FIFO";
    // Queued behind a turn whose tools still answer: no prompt of its own yet.
    let queued = [
        line("queue-operation", NEXT_CENTURY, json!({ "operation": "enqueue", "content": said })),
        line(
            "user",
            NEXT_CENTURY,
            json!({ "message": { "role": "user", "content": [{ "type": "tool_result", "content": said }] } }),
        ),
    ];
    let file = w.host.home().join(".claude/projects/web/session.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    // Written now by the world's clock, as the session would be.
    let now = std::time::UNIX_EPOCH + std::time::Duration::from_millis(w.app_now() as u64);
    let write = |file: &std::path::Path, text: &str| {
        std::fs::write(file, text).unwrap();
        std::fs::File::options().write(true).open(file).unwrap().set_modified(now).unwrap();
    };
    write(&file, &queued.join("\n"));
    assert_eq!(w.app.watch_tell(id(&wk), "look at the FIFO", false).await.unwrap()["delivery"], "queued");

    // The same words taken long before this tell are not this tell.
    let long_ago = line("user", "2000-01-01T00:00:00.000Z", json!({ "message": { "role": "user", "content": said } }));
    write(&file, &format!("{}\n{long_ago}\n", queued.join("\n")));
    assert_eq!(w.app.watch_tell(id(&wk), "look at the FIFO", false).await.unwrap()["delivery"], "queued");

    // Taken as its prompt: the session's own file says so where Orca saw nothing.
    let took = line("user", NEXT_CENTURY, json!({ "message": { "role": "user", "content": said } }));
    write(&file, &format!("{}\n{took}\n", queued.join("\n")));
    assert_eq!(w.app.watch_tell(id(&wk), "look at the FIFO", true).await.unwrap()["delivery"], "submitted");
}

/// factorium#64's worker on 2026-11-18: its red was taken as its prompt
/// 9.1s after it was typed, as Orca's window ended, and written to its
/// session file just after factorium read it.
#[tokio::test]
async fn a_tell_its_session_takes_just_after_orca_s_window_is_submitted_and_one_never_taken_is_missed() {
    for takes in [true, false] {
        let w = World::new();
        w.checkout("web").await;
        w.kit().await;
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let wk = worker(&w.app).await;
        let (handle, cwd) = (wk["terminal"].as_str().unwrap(), wk["worktree"].as_str().unwrap());
        // Idle, and Orca sees no turn start.
        w.set_busy(handle, false, true);
        let file = w.host.home().join(".claude/projects/web/session.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_millis(w.app_now() as u64);
        let took = json!({
            "type": "user", "timestamp": NEXT_CENTURY, "cwd": cwd, "isSidechain": false,
            "message": { "role": "user", "content": "factorium (the watch): look at the FIFO" },
        });
        if takes {
            // The row lands while factorium waits, after Orca's window.
            *w.host.while_sleeping.lock().unwrap() = Some(Box::new(move || {
                std::fs::write(&file, format!("{took}\n")).unwrap();
                std::fs::File::options().write(true).open(&file).unwrap().set_modified(now).unwrap();
            }));
        }
        let sleeps = || w.host.ran.lock().unwrap().iter().filter(|r| *r == "sleep 1").count();
        let before = sleeps();
        let delivery = w.app.watch_tell(id(&wk), "look at the FIFO", false).await.unwrap()["delivery"].clone();
        let slept = sleeps() - before;
        let status = worker(&w.app).await["status"].clone();
        if takes {
            assert_eq!((delivery, slept, status), (json!("submitted"), 1, json!("working")), "read again once written");
        } else {
            assert_eq!((delivery, slept, status), (json!("missed"), 3, json!("waiting")), "a few seconds' grace");
        }
    }
}

/// A worker that ends while a tell to it is being typed (its issue closed by
/// another run): the tell it missed sends it to no one, and is answered.
#[tokio::test]
async fn a_tell_missed_by_a_worker_that_ended_meanwhile_is_answered_missed() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    w.set_busy(wk["terminal"].as_str().unwrap(), false, true);
    let (db, host) = (w.db.clone(), w.host.clone());
    *w.host.while_sleeping.lock().unwrap() = Some(Box::new(move || {
        let closed = crate::fixture::another(&db, &host, |app| async move { app.issue_close("web", 1, "done").await });
        closed.unwrap().unwrap();
    }));
    let told = w.app.watch_tell(id(&wk), "look at the FIFO", false).await.unwrap();
    assert_eq!((told["delivery"].clone(), w.workers().await.len()), (json!("missed"), 0), "{told}");
}

/// factorium#170: guild#17's worker on 2026-11-21 read a comment typed
/// while its turn ran: Claude Code wrote it only as a `queue-operation` and a
/// `queued_command` attachment of that turn, in the session file of the
/// directory it started in, and the turn ended before factorium looked. It
/// was taken; words another prompt took instead were not.
#[tokio::test]
async fn a_tell_claude_code_takes_into_its_running_turn_is_submitted_and_one_another_prompt_took_is_missed() {
    for taken in [true, false] {
        let w = World::new();
        w.checkout("web").await;
        w.kit().await;
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let wk = worker(&w.app).await;
        let (handle, cwd) = (wk["terminal"].as_str().unwrap(), wk["worktree"].as_str().unwrap());
        // The turn it was typed into has ended, and Orca saw no turn start.
        w.set_busy(handle, false, true);
        let said = "factorium (the watch): look at the FIFO";
        let took = if taken { said } else { "factorium (the watch): something else" };
        let role = w.data().worker_role().to_string_lossy().into_owned();
        let line = |kind: &str, body: serde_json::Value| {
            let mut l = json!({ "type": kind, "timestamp": NEXT_CENTURY, "cwd": role, "isSidechain": false });
            l.as_object_mut().unwrap().extend(body.as_object().unwrap().clone());
            l.to_string()
        };
        let lines = [
            line(
                "attachment",
                json!({ "attachment": { "type": "environment", "additionalWorkingDirectories": [cwd] } }),
            ),
            line("queue-operation", json!({ "operation": "enqueue", "content": took })),
            line("queue-operation", json!({ "operation": "remove", "content": took, "reason": "absorbed_mid_turn" })),
            line(
                "attachment",
                json!({ "attachment": { "type": "queued_command", "prompt": took, "commandMode": "prompt" } }),
            ),
            line("assistant", json!({ "message": { "role": "assistant", "stop_reason": "end_turn" } })),
        ];
        let file = w.host.home().join(".claude/projects/-role-worker/session.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_millis(w.app_now() as u64);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(now).unwrap();
        let delivery = w.app.watch_tell(id(&wk), "look at the FIFO", false).await.unwrap()["delivery"].clone();
        let status = worker(&w.app).await["status"].clone();
        if taken {
            assert_eq!((delivery, status), (json!("submitted"), json!("working")), "taken into its turn");
        } else {
            assert_eq!((delivery, status), (json!("missed"), json!("waiting")), "another prompt took the keys");
        }
    }
}

#[tokio::test]
async fn an_escalation_makes_the_look_due_on_the_next_tick() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "u", "", false, &[]).await.unwrap();
    w.tick().await;
    w.tick().await;
    w.watch_session(false);
    w.advance(5);
    assert!(w.tick().await.watch.is_some(), "a timed look");
    let workers = w.workers().await;
    assert_eq!(workers.len(), 2);

    // Right after the look, well inside look_every, both escalate.
    w.advance(1);
    for wk in &workers {
        w.app.worker_escalate(id(wk), &format!("stuck on {}", wk["number"])).await.unwrap();
    }
    let now = w.app_now();
    let looked = w.tick().await.watch.expect("the escalations make the look due");
    assert_eq!(looked["episodes"], 2, "{looked}");
    let next = w.app.watch_next().await.unwrap();
    for e in next["episodes"].as_object().unwrap().values() {
        assert_eq!(e["opened_at"], now, "opened at this tick's look: {e}");
    }
    assert_eq!(next["round"]["n"], 1, "one round; the other episode queues");
    assert!(w.tick().await.watch.is_none(), "two waits, one look");

    // While the round runs, a third wait still makes the look due; its episode queues.
    w.app.issue_add("web", "v", "", false, &[]).await.unwrap();
    w.advance(1);
    w.tick().await;
    let third = w.worker_on("web", 3).await;
    w.advance(1);
    w.app.worker_escalate(id(&third), "stuck on 3").await.unwrap();
    let looked = w.tick().await.watch.expect("the third escalation makes the look due");
    assert_eq!(looked["episodes"], 3, "{looked}");
    assert_eq!(looked["round"]["n"], 1, "the running round goes on");
}

/// factorium#94: the watch started at 00:45 on one binary's rules parked
/// an issue at 17:04 by rules v0.1.20 had already changed. A watch briefed
/// with other rules than the running binary's is given this binary's before
/// its next round — between rounds, never in one.
#[tokio::test]
async fn a_watch_briefed_with_older_rules_gets_the_running_binary_s_before_its_next_round() {
    let w = World::new();
    escalated(&w).await;
    w.advance(5);
    w.tick().await;
    let first = watch_terminal(&w);
    assert_eq!(w.terminal(&first).sent.len(), 1, "round 1");
    // As an older binary briefed it: no rules recorded (or other ones).
    w.watch_started_as_an_older_binary_wrote_it();
    std::fs::write(w.data().watch_brief(), "the old rules").unwrap();

    // Mid-round, the session stays.
    w.advance(5);
    w.tick().await;
    assert!(w.terminal(&first).alive, "never restarted mid-round");
    assert_eq!(w.app.watch_next().await.unwrap()["round"]["n"], 1);

    // The round ends; another episode waits for the next.
    w.app.watch_report("Answered.", None).await.unwrap();
    w.app.issue_add("web", "u", "", false, &[]).await.unwrap();
    w.tick().await;
    let second = w.worker_on("web", 2).await;
    w.advance(1);
    w.app.worker_escalate(id(&second), "stuck too").await.unwrap();
    w.tick().await;
    let now = live_watch(&w);
    assert_ne!(now, first, "a fresh session, between rounds");
    assert!(crate::fixture::closed(&w.app, &first).await, "the old one is closed");
    let brief = std::fs::read_to_string(w.data().watch_brief()).unwrap();
    assert_eq!(brief, std::fs::read_to_string(w.data().watch().join(RULES)).unwrap());
    assert!(brief.contains(&format!("The database is {}.", w.db.to_string_lossy())), "this binary's rules: {brief}");
    assert!(w.terminal(&now).line.contains(&*w.data().watch_brief().to_string_lossy()), "handed its brief");

    // Its next round goes to the fresh session, which is not restarted again.
    w.watch_session(false);
    w.advance(5);
    w.tick().await;
    let sent = w.terminal(&now).sent;
    assert!(sent.len() == 1 && sent[0].contains("round 2") && sent[0].contains("stuck too"), "{sent:?}");
    w.app.watch_report("Answered too.", None).await.unwrap();
    w.advance(5);
    w.tick().await;
    assert_eq!(live_watch(&w), now, "the same rules: the same session");
}

/// The watch's terminal that runs: a closed one stays in Orca's fake.
fn live_watch(w: &World) -> String {
    let t = w.host.orca.terminals.lock().unwrap();
    let live: Vec<&String> =
        t.iter().filter(|(_, t)| t.title == "factorium watch" && t.alive).map(|(h, _)| h).collect();
    assert_eq!(live.len(), 1, "one watch session runs");
    live[0].clone()
}

/// factorium#268, rollmind#8 on 2026-11-23: the watch's tell answered a
/// waiting worker, which went back to work; it stayed waiting, and its
/// `done` was refused for hours.
#[tokio::test]
async fn a_waiting_worker_the_watch_s_tell_reaches_works_again_and_its_done_lands() {
    let w = World::new();
    let wk = escalated(&w).await;
    w.advance(5);
    w.tick().await;
    w.app.watch_report("Answered: v2.", None).await.unwrap();
    let told = w.app.watch_tell(id(&wk), "Use v2.", false).await.unwrap();
    assert_eq!(told["delivery"], "submitted");
    assert_eq!(worker(&w.app).await["status"], "working", "it read the watch's word");
    w.commit(&crate::fixture::worktree(&wk), "v2.txt", "v2\n");
    w.app.worker_done(id(&wk), None).await.unwrap();
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
}

/// The round's action did not end what it found (here: the worker, still
/// waiting, is refused `done`, its key unchanged): `round_again` after the
/// report, the next look starts another round, carrying that report.
#[tokio::test]
async fn a_finding_still_standing_after_its_round_gets_another_carrying_the_last_report() {
    let w = World::new();
    let wk = escalated(&w).await;
    w.advance(5);
    w.tick().await;
    w.app.watch_report("Answered: v2.", None).await.unwrap();
    w.commit(&crate::fixture::worktree(&wk), "v2.txt", "v2\n");
    crate::fixture::refused(w.app.worker_done(id(&wk), None).await, "waiting");
    w.advance(25);
    w.tick().await;
    assert_eq!(w.app.watch_next().await.unwrap()["round"], json!(null), "not before round_again");
    w.advance(5);
    w.tick().await;
    assert_eq!(w.app.watch_next().await.unwrap()["round"]["n"], 2);
    let round = w.terminal(&watch_terminal(&w)).sent.last().unwrap().clone();
    assert!(round.contains("round 2") && round.contains("Answered: v2.") && round.contains("still there"), "{round}");
}

/// Past `rounds_max` rounds on one finding, no round starts; doctor shows
/// it unresolved, with the last report.
#[tokio::test]
async fn past_its_rounds_a_finding_gets_none_more_and_doctor_shows_it_unresolved() {
    let w = World::new();
    let wk = escalated(&w).await;
    for n in 1..=3 {
        w.advance(30);
        w.tick().await;
        assert_eq!(w.app.watch_next().await.unwrap()["round"]["n"], n);
        assert_eq!(w.app.doctor().await.unwrap()["unresolved"], json!([]), "rounds left");
        w.app.watch_report(&format!("try {n}"), None).await.unwrap();
    }
    w.advance(30);
    w.tick().await;
    w.advance(60);
    w.tick().await;
    assert_eq!(w.app.watch_next().await.unwrap()["round"], json!(null), "its rounds are spent");
    let unresolved = w.app.doctor().await.unwrap()["unresolved"].clone();
    let key = format!("worker:{}", id(&wk));
    assert_eq!(unresolved, json!([{ "key": key, "what": unresolved[0]["what"], "rounds": 3, "last_report": "try 3" }]));
}
