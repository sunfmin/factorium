//! Idle work: a checkout's `[idle]` steps run on its base tip only while the
//! host is idle, at most once per `every`, behind everything else the
//! heavy queue holds; they yield to what queues beside them, and a red run
//! goes to the watch.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::fixture::{STUCK, World, entry, escaped, escaping, nice_is, own_nice, until_file};

/// A checkout `web` whose base declares `[idle]` with `steps`, `every`
/// seconds apart and within `limit`, as last fetched; and a kit.
async fn declares(w: &World, steps: &[String], every: u64, limit: u64) -> PathBuf {
    let web = w.checkout("web").await;
    w.kit().await;
    let steps: Vec<String> = steps.iter().map(|s| format!("{s:?}")).collect();
    let toml = format!(
        "[gate]\nfast = \"true\"\n\n[idle]\nsteps = [{}]\nevery = {every}\nlimit = {limit}\n",
        steps.join(", ")
    );
    w.push_by_hand("web", "main", "factorium.toml", &toml);
    w.git(&web, &["fetch", "-q", "origin"]).unwrap();
    web
}

/// Ticks until a tick says idle work ended green or red; answers what it
/// said. Between ticks it waits for the run's steps to end, so it ticks a
/// few times, not once per turn of a spin: one tick starts it, the next
/// finds it ended.
async fn ended(w: &World) -> Value {
    let ticks = async {
        for _ in 0..3 {
            if let Some(v) = w.tick().await.idle.into_iter().find(|v| v.get("green").is_some()) {
                return v;
            }
            w.app.beside_ended().await;
        }
        panic!("three ticks and no idle work ended");
    };
    tokio::time::timeout(STUCK, ticks).await.expect("the idle work ends")
}

fn started(idle: &[Value]) -> bool {
    idle.iter().any(|v| v["idle"] == "web" && v["started"] == true)
}

#[test]
fn idle_work_left_without_every_or_limit_runs_once_a_day_within_an_hour() {
    let idle = factorium_domain::declared::read(Some("[idle]\nsteps = [\"./soak.sh\"]\n")).unwrap().idle.unwrap();
    assert_eq!((idle.every, idle.limit), (86400, 3600));
}

#[tokio::test]
async fn idle_work_runs_when_the_host_is_idle_and_not_again_within_every() {
    let w = World::new();
    declares(&w, &["echo all mutants caught".into()], 3600, 60).await;
    let t = w.tick().await;
    assert!(started(&t.idle), "{:?}", t.idle);
    assert_eq!(w.queued().await, ["idle work of web"], "it holds a heavy slot");
    let run = ended(&w).await;
    assert_eq!(run["green"], true, "{run}");
    assert_eq!(run["tip"], w.remote_tip("web", "main"), "on the base's tip");
    let d = w.app.doctor().await.unwrap();
    let last = &d["idle"][0]["last"];
    assert_eq!((last["green"].clone(), last["tail"].clone()), (json!(true), json!("all mutants caught")), "{d}");
    assert_eq!(w.queued().await, Vec::<String>::new());

    w.advance(59);
    assert_eq!(w.tick().await.idle, Vec::<Value>::new(), "not again within `every`");
    w.advance(2);
    assert!(started(&w.tick().await.idle), "again once `every` has passed");
    // The daemon stops before its next tick: the run ends with it, unrecorded.
    assert_eq!(w.app.run(&std::sync::atomic::AtomicBool::new(true)).await.unwrap()["ticks"], 0);
    assert_eq!(w.queued().await, Vec::<String>::new(), "it left the queue");
    assert_eq!(w.app.doctor().await.unwrap()["idle"][0]["last"], d["idle"][0]["last"], "the first run is the last");
}

#[tokio::test]
async fn idle_work_waits_while_a_worker_works_or_a_heavy_command_runs() {
    let w = World::new();
    // A worker at work: the host is not idle.
    declares(&w, &["true".into()], 3600, 60).await;
    w.app.issue_add("web", "a", "", false, &[]).await.unwrap();
    let t = w.tick().await;
    assert!(t.started.is_some() && t.idle.is_empty(), "{t:?}");
    assert_eq!(w.tick().await.idle, Vec::<Value>::new(), "{t:?}");
    drop(w);

    let w = World::new();
    declares(&w, &["true".into()], 3600, 60).await;
    w.while_busy(async {
        // Room on the host and a new reading, but the queue holds a worker's heavy command.
        w.host.reading.lock().unwrap().cpu_busy = 10;
        w.advance(1);
        assert_eq!(w.tick().await.idle, Vec::<Value>::new());
        assert!(!w.queued().await.contains(&"idle work of web".to_string()));
    })
    .await;
    assert!(started(&w.tick().await.idle), "once the queue is empty");
    ended(&w).await;
}

/// A tick while idle work runs starts none beside it, overdue as it is: its
/// clone, which its line uses, is left as it made it.
#[tokio::test]
async fn a_tick_while_overdue_idle_work_runs_leaves_it_and_its_clone_be() {
    let w = World::new();
    let (web, answer, dir) = (w.checkout("web").await, w.dir.join("next.json"), w.dir.join("held"));
    std::fs::write(&answer, next(&[("deepen", "a.rs")])).unwrap();
    let line = format!(
        "touch mark && ( {} ) >/dev/null && test -f mark && echo slimfit 0.1.16 && cat '{}'",
        w.holds(&dir),
        answer.display()
    );
    w.push_by_hand("web", "main", "factorium.toml", &format!("[idle]\ntasks = {line:?}\nevery = 60\nlimit = 60\n"));
    w.git(&web, &["fetch", "-q", "origin"]).unwrap();
    w.app.settings_set(Some("tasks_bound"), Some(600)).await.unwrap();
    w.host.reading.lock().unwrap().cpu_busy = 99;
    w.advance(1);
    assert!(started(&w.tick().await.idle));
    until_file(&dir.join("started")).await;
    assert_eq!(w.tick().await.idle, Vec::<Value>::new(), "it runs on");
    std::fs::write(dir.join("release"), "").unwrap();
    let run = ended(&w).await;
    assert_eq!(run["green"], true, "{run}");
}

/// Idle work whose steps ended green before a command queued beside it is
/// a run, recorded green: giving way ends only what still runs.
#[tokio::test]
async fn idle_work_that_ended_green_before_a_command_queued_beside_it_is_recorded_green() {
    let w = World::new();
    declares(&w, &["true".into()], 3600, 60).await;
    assert!(started(&w.tick().await.idle));
    w.app.beside_ended().await;
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let cli = w.run();
    let wants = cli.worker_exclusive(None, &w.dir, "echo after");
    let ticks = async {
        w.until_queued("echo after").await;
        let t = w.tick().await;
        w.host.reading.lock().unwrap().cpu_busy = 10;
        w.poll(1).await;
        t
    };
    let (ran, t) = tokio::time::timeout(STUCK, async { tokio::join!(wants, ticks) }).await.expect("it ends");
    assert_eq!(ran.unwrap()["ok"], true);
    assert!(t.idle.iter().any(|v| v["idle"] == "web" && v["green"] == true), "{:?}", t.idle);
    assert_eq!(w.app.doctor().await.unwrap()["idle"][0]["last"]["green"], true, "{:?}", t.idle);
}

#[tokio::test]
async fn idle_work_yields_to_a_heavy_command_queued_beside_it_and_runs_next_idle_window() {
    let w = World::new();
    let dir = w.dir.join("mutants");
    declares(&w, &[w.holds(&dir)], 3600, 60).await;
    assert!(started(&w.tick().await.idle));
    until_file(&dir.join("started")).await;
    // No room beside it: the worker's command waits, until the idle work yields.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    let cli = w.run();
    let wants = cli.worker_exclusive(None, &w.dir, "echo after");
    let ticks = async {
        w.until_queued("echo after").await;
        let t = w.tick().await;
        // The idle work yielded: the worker's command asks again.
        w.gone("idle work of web").await;
        w.poll(1).await;
        t
    };
    let (ran, t) = tokio::time::timeout(STUCK, async { tokio::join!(wants, ticks) }).await.expect("it yields");
    assert_eq!(ran.unwrap()["ok"], true, "the worker's command runs");
    assert!(t.idle.iter().any(|v| v["yielded"].is_string()), "{:?}", t.idle);
    assert_eq!(w.app.doctor().await.unwrap()["idle"][0]["last"], json!(null), "a yielded run is no run");
    w.host.reading.lock().unwrap().cpu_busy = 10;
    std::fs::remove_file(dir.join("started")).unwrap();
    assert!(started(&w.tick().await.idle), "the next idle window runs it again");
    until_file(&dir.join("started")).await;
    std::fs::write(dir.join("release"), "").unwrap();
    assert_eq!(ended(&w).await["green"], true);
}

#[tokio::test]
async fn red_idle_work_is_a_finding_and_opens_an_episode() {
    let w = World::new();
    declares(&w, &["echo 3 mutants missed; exit 3".into()], 3600, 60).await;
    w.tick().await;
    let run = ended(&w).await;
    assert_eq!(run["green"], false, "{run}");
    let tail = run["tail"].as_str().unwrap();
    assert!(tail.contains("failed") && tail.contains("3 mutants missed"), "{tail}");
    let key = format!("idle:web:{}", w.remote_tip("web", "main"));
    let found = w.finding(&key).await.expect("a finding");
    assert!(found["what"].as_str().unwrap().contains("3 mutants missed"), "{found}");
    w.advance(6);
    w.tick().await;
    let episodes = w.app.watch_next().await.unwrap()["episodes"].clone();
    assert!(episodes.get(&key).is_some(), "{episodes}");
}

#[tokio::test]
async fn idle_work_past_its_limit_ends_red_with_its_tail() {
    let w = World::new();
    let dir = w.dir.join("long");
    declares(&w, &[format!("echo still mutating; {}", w.holds(&dir))], 3600, 1).await;
    assert!(started(&w.tick().await.idle));
    // The limit is the world's clock: the step has printed before it runs out,
    // however long a busy host leaves a background step waiting.
    until_file(&dir.join("started")).await;
    w.advance(1);
    let run = ended(&w).await;
    assert_eq!(run["green"], false, "{run}");
    let tail = run["tail"].as_str().unwrap();
    assert!(tail.contains("past its limit of 1s") && tail.contains("still mutating"), "{tail}");
    assert!(w.finding(&format!("idle:web:{}", w.remote_tip("web", "main"))).await.is_some());
}

#[tokio::test]
async fn idle_work_runs_below_factoriums_priority() {
    let w = World::new();
    declares(&w, &[nice_is("-gt", own_nice())], 3600, 60).await;
    w.tick().await;
    let run = ended(&w).await;
    assert_eq!(run["green"], true, "{run}");
}

/// Idle work asks the heavy queue once. Refused, its entry leaves the queue
/// again, so it holds nothing up: overdue tasks wait behind what runs on a
/// host with no room, and under rules a newer binary replaced (the daemon
/// execs that binary after the tick, as for any job it asks once).
#[tokio::test]
async fn idle_work_refused_its_start_leaves_the_queue_as_it_found_it() {
    let w = World::new();
    let answer = finds_tasks(&w).await;
    std::fs::write(&answer, next(&[("deepen", "a.rs")])).unwrap();
    w.app.settings_set(Some("tasks_bound"), Some(600)).await.unwrap();
    w.while_busy(async {
        let running = w.queued().await;
        w.advance(1);
        assert_eq!(w.tick().await.idle, Vec::<Value>::new(), "it waits behind what runs");
        assert_eq!(w.queued().await, running);
        // A newer binary's daemon starts: its rules replace this one's.
        w.run().under_rules("99.0.0").run(&std::sync::atomic::AtomicBool::new(true)).await.unwrap();
        assert_eq!(w.tick().await.idle, Vec::<Value>::new(), "refused under replaced rules");
        assert_eq!(w.queued().await, running);
    })
    .await;
}

/// What slimfit's `next --all` answers with tasks of each `(kind, file)`.
fn next(tasks: &[(&str, &str)]) -> String {
    let task = |(kind, file): &(&str, &str)| {
        let summary = format!("{file}: 316 lines, changed in 28 commits in 90 days");
        json!({ "kind": kind, "files": [file], "summary": summary, "steps": "Deepen it.", "accept": "slimfit check" })
    };
    let tasks: Vec<Value> = tasks.iter().map(task).collect();
    serde_json::to_string_pretty(&json!({ "next": "do the task", "tasks": tasks })).unwrap()
}

/// A checkout `web` whose idle work's tasks line reads slimfit 0.1.16, then
/// what the file it answers holds, at most once a minute. No kit: no worker
/// takes what is filed, so the host stays idle.
async fn finds_tasks(w: &World) -> PathBuf {
    let web = w.checkout("web").await;
    let answer = w.dir.join("next.json");
    let line = format!("echo slimfit 0.1.16 && cat '{}'", answer.display());
    let toml = format!("[idle]\ntasks = {line:?}\nevery = 60\nlimit = 60\n");
    let clone = w.dir.join("edit");
    let remote = w.dir.join("remotes/web.git");
    w.git(&w.dir, &["clone", "-q", &remote.to_string_lossy(), &clone.to_string_lossy()]).unwrap();
    w.commit(&clone, "factorium.toml", &toml);
    w.git(&clone, &["push", "-q", "origin", "main"]).unwrap();
    w.git(&web, &["fetch", "-q", "origin"]).unwrap();
    answer
}

/// The next idle run, a minute on: the issues it filed.
async fn files(w: &World) -> Value {
    w.advance(2);
    let run = ended(w).await;
    assert_eq!(run["green"], true, "{run}");
    run["filed"].clone()
}

#[tokio::test]
async fn idle_work_files_each_kinds_next_task_once_none_of_its_kind_is_open() {
    let w = World::new();
    let answer = finds_tasks(&w).await;
    std::fs::write(&answer, next(&[("deepen", "a.rs"), ("deepen", "b.rs"), ("redundant-test", "t.rs")])).unwrap();
    assert_eq!(files(&w).await, json!([1, 2]), "one of each kind");
    let one = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!(one["title"], "slimfit deepen: a.rs: 316 lines, changed in 28 commits in 90 days");
    assert_eq!(one["task"], json!({ "kind": "deepen", "files": ["a.rs"] }));
    let body = one["body"].as_str().unwrap();
    let tip = w.remote_tip("web", "main");
    for carried in ["slimfit 0.1.16", tip.as_str(), "\"steps\": \"Deepen it.\"", "\"accept\": \"slimfit check\""] {
        assert!(body.contains(carried), "{carried} in {body}");
    }
    assert_eq!(w.app.issue_show("web", 2).await.unwrap()["task"]["kind"], "redundant-test");
    assert_eq!(files(&w).await, json!([]), "none while one of its kind is open");

    w.app.issue_close("web", 1, "deepened").await.unwrap();
    assert_eq!(files(&w).await, json!([3]), "the next of its kind, not the one just closed");
    assert_eq!(w.app.issue_show("web", 3).await.unwrap()["task"]["files"], json!(["b.rs"]));
    std::fs::write(&answer, next(&[("deepen", "a.rs")])).unwrap();
    w.app.issue_close("web", 3, "deepened").await.unwrap();
    assert_eq!(files(&w).await, json!([]), "a task closed this week is not filed again");
    w.advance(7 * 24 * 60);
    assert_eq!(files(&w).await, json!([4]), "a week on, it is");

    std::fs::write(&answer, "no tasks").unwrap();
    w.advance(2);
    let run = ended(&w).await;
    assert_eq!(run["green"], false, "{run}");
    assert!(run["tail"].as_str().unwrap().contains("printed no answer"), "{run}");
}

#[tokio::test]
async fn tasks_overdue_by_tasks_bound_are_filed_on_a_host_never_idle() {
    let w = World::new();
    let answer = finds_tasks(&w).await;
    std::fs::write(&answer, next(&[("deepen", "a.rs")])).unwrap();
    w.app.settings_set(Some("tasks_bound"), Some(600)).await.unwrap();
    // No room on the host, ever: it is never idle.
    w.host.reading.lock().unwrap().cpu_busy = 99;
    w.advance(1);
    let run = ended(&w).await;
    assert_eq!((run["green"].clone(), run["filed"].clone()), (json!(true), json!([1])), "never read: overdue at once");
    w.app.issue_close("web", 1, "deepened").await.unwrap();
    std::fs::write(&answer, next(&[("deepen", "b.rs")])).unwrap();
    w.advance(2);
    assert_eq!(w.tick().await.idle, Vec::<Value>::new(), "past `every`, but the host is not idle");
    w.advance(10);
    let run = ended(&w).await;
    assert_eq!(run["filed"], json!([2]), "past `every` and `tasks_bound`: {run}");
}

/// Idle work runs through the one heavy run (factorium#136): `unqueue` ends
/// its whole process tree as it does any running entry's — a later step's
/// too, its session noted at a tick — and the next tick finds it gone,
/// unrecorded, so it is due again.
#[tokio::test]
async fn unqueue_ends_running_idle_work_and_its_whole_process_tree() {
    let w = World::new();
    let dir = w.dir.join("escaping");
    declares(&w, &["true".into(), escaping(&w, &dir)], 3600, 60).await;
    assert!(started(&w.tick().await.idle));
    let pids = escaped(&dir).await;
    assert_eq!(w.tick().await.idle, Vec::<Value>::new(), "it runs on");
    let id = entry(&w, "idle work of web").await;
    let unqueued = w.run().unqueue(id).await.unwrap();
    assert_eq!(unqueued["ended"], true, "{unqueued}");
    assert_eq!(pids.left(), Vec::<u32>::new(), "still running");
    let t = w.tick().await;
    assert!(t.idle.iter().any(|v| v["idle"] == "web" && v["yielded"] == "unqueued"), "{:?}", t.idle);
    assert_ne!(entry(&w, "idle work of web").await, id, "unrecorded, it is due again: a new entry");
    assert_eq!(w.app.doctor().await.unwrap()["idle"][0]["last"], json!(null), "an unqueued run is no run");
    // The daemon stops: the run it started again ends with it.
    w.app.run(&std::sync::atomic::AtomicBool::new(true)).await.unwrap();
    assert_eq!(w.queued().await, Vec::<String>::new());
}
