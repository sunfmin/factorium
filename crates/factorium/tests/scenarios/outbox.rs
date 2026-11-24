//! The outbox: every message to a session — a landing's red, the watch's
//! tell and restart, a comment, a round, the brief — goes one way, and the
//! session's outbox, as `factorium outbox` answers it, says what was typed
//! and how it went. One family of scenarios runs each kind of message
//! through the same cases: idle, busy, behind a dialog, before its harness
//! is up, and missed.

use serde_json::{Value, json};

use crate::fixture::{CLAUDE, ESCAPE, SHELL, UP, World, findings, id, worker};

/// The messages a worker's session is passed.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Red,
    Tell,
    Restart,
    Comment,
}

const KINDS: [Kind; 4] = [Kind::Red, Kind::Tell, Kind::Restart, Kind::Comment];

impl Kind {
    /// How the line typed for it opens.
    fn opens(self) -> &'static str {
        match self {
            Kind::Red => "factorium: your landing was red. ",
            Kind::Tell => "factorium (the watch): use v2",
            Kind::Restart => "factorium (the watch): use v2. Then go on",
            Kind::Comment => "factorium (a comment on your issue): use v2",
        }
    }

    /// Gets a worker ready for it: a red needs red work asked to land; a
    /// restart, a worker that waits.
    async fn prepare(self, w: &World, wk: &Value) {
        match self {
            Kind::Red => w.done(wk, "RED", "x").await,
            Kind::Restart => _ = w.app.worker_escalate(id(wk), "stuck").await.unwrap(),
            _ => {}
        }
    }

    /// Passes it; answers the command's answer (a red: the landing's).
    async fn say(self, w: &World, wk: &Value, now: bool) -> factorium::Answer {
        match self {
            Kind::Red => Ok(w.tick().await.landed[0].clone()),
            Kind::Tell => w.app.watch_tell(id(wk), "use v2", now).await,
            Kind::Restart => w.app.watch_restart(id(wk), "use v2.", now).await,
            Kind::Comment => w.app.issue_comment("web", 1, "use v2").await,
        }
    }
}

/// The last message of a worker's outbox.
async fn last(w: &World, wk: &Value) -> Value {
    let outbox = w.app.outbox_of(Some(id(wk))).await.unwrap();
    outbox["messages"].as_array().unwrap().last().cloned().expect("a message")
}

#[tokio::test]
async fn behind_a_running_turn_every_message_queues_and_only_now_stops_the_turn_first() {
    for kind in KINDS {
        let w = World::new();
        let wk = w.working(CLAUDE, "t").await;
        let handle = wk["terminal"].as_str().unwrap();
        kind.prepare(&w, &wk).await;
        w.set_busy(handle, true, false);
        let before = w.terminal(handle).sent.len();
        kind.say(&w, &wk, false).await.unwrap();
        assert_eq!(w.terminal(handle).sent.len(), before + 1, "{kind:?}: no key goes ahead of it");
        assert!(w.terminal(handle).busy, "{kind:?}: its turn runs on");
        assert_eq!(last(&w, &wk).await["outcome"]["outcome"], "queued", "{kind:?}");
        if matches!(kind, Kind::Tell | Kind::Restart) {
            // With --now, the harness's own key goes first, then the text, and it reads it.
            kind.prepare(&w, &wk).await;
            assert_eq!(kind.say(&w, &wk, true).await.unwrap()["delivery"], "submitted", "{kind:?}");
            let sent = w.terminal(handle).sent;
            assert_eq!(sent[sent.len() - 2], ESCAPE, "{kind:?}: {sent:?}");
            assert!(!w.terminal(handle).busy, "{kind:?}");
        }
    }
}

#[tokio::test]
async fn told_now_while_idle_with_a_shell_in_the_background_the_message_is_submitted_though_orca_never_sees_it_idle() {
    for kind in [Kind::Tell, Kind::Restart] {
        let w = World::new();
        let wk = w.working(CLAUDE, "t").await;
        let handle = wk["terminal"].as_str().unwrap();
        kind.prepare(&w, &wk).await;
        w.background_shell(handle);
        let before = w.terminal(handle).sent.len();
        assert_eq!(kind.say(&w, &wk, true).await.unwrap()["delivery"], "submitted", "{kind:?}");
        let sent = w.terminal(handle).sent[before..].to_vec();
        assert!(sent.len() == 2 && sent[0] == ESCAPE && sent[1].starts_with(kind.opens()), "{kind:?}: {sent:?}");
        assert_eq!(last(&w, &wk).await["outcome"]["outcome"], "submitted", "{kind:?}");
    }
}

/// Claude Code's dialog after a turn ended, as two workers showed it on
/// 2026-11-18 while their landing's red was typed at them.
const AUTO_MODE_DIALOG: [&str; 4] = [
    "Teach auto mode about your environment?",
    "Claude Code reads this project, your recent Claude sessions, and optionally your shell history.",
    "› Continue",
    "←/→ to change · Enter to continue · Esc to cancel",
];

#[tokio::test]
async fn a_known_dialog_is_dismissed_first_and_every_message_then_starts_a_turn_typed_once_as_its_sender_s() {
    for kind in KINDS {
        let w = World::new();
        let wk = w.working(CLAUDE, "t").await;
        let handle = wk["terminal"].as_str().unwrap();
        kind.prepare(&w, &wk).await;
        w.open_dialog(handle, &AUTO_MODE_DIALOG);
        let before = w.terminal(handle).sent.len();
        kind.say(&w, &wk, false).await.unwrap();
        let sent = w.terminal(handle).sent[before..].to_vec();
        assert!(sent.len() == 2 && sent[0] == ESCAPE, "{kind:?}: {sent:?}");
        assert!(sent[1].starts_with(kind.opens()) && !sent[1].contains('\n'), "{kind:?}: one line: {sent:?}");
        assert!(!w.terminal(handle).dialog, "{kind:?}: dismissed");
        let m = last(&w, &wk).await;
        assert_eq!((&m["text"], &m["outcome"]["outcome"]), (&json!(sent[1]), &json!("submitted")), "{kind:?}");
        assert_eq!(worker(&w.app).await["status"], "working", "{kind:?}");
        assert!(findings(&w.app.doctor().await.unwrap()).is_empty(), "{kind:?}");
    }
}

#[tokio::test]
async fn a_message_an_unknown_dialog_takes_is_missed_and_its_worker_goes_to_the_watch_at_once() {
    for kind in KINDS {
        let w = World::new();
        let wk = w.working(CLAUDE, "t").await;
        let handle = wk["terminal"].as_str().unwrap();
        kind.prepare(&w, &wk).await;
        w.open_dialog(handle, &["Try the new thing?", "› Yes", "Enter to confirm"]);
        let said = kind.say(&w, &wk, false).await.unwrap();
        assert!(!w.terminal(handle).sent.contains(&ESCAPE.to_string()), "{kind:?}: no key is guessed");
        let m = last(&w, &wk).await;
        assert_eq!(m["outcome"]["outcome"], "missed", "{kind:?}");
        let waiting = worker(&w.app).await;
        assert_eq!(waiting["status"], "waiting", "{kind:?}: not 30 min of silence");
        let why = waiting["why"].as_str().unwrap();
        let what = if matches!(kind, Kind::Restart) { "stuck" } else { "did not reach" };
        assert!(why.contains(what), "{kind:?}: {why}");
        if !matches!(kind, Kind::Restart) {
            assert!(why.contains("Enter to confirm"), "{kind:?}: {why}");
        }
        // Who it was from, in its own words.
        let from = match kind {
            Kind::Tell => "the watch's word did not reach it",
            Kind::Comment => "a comment on its issue did not reach it",
            Kind::Red => "factorium's word did not reach it",
            Kind::Restart => "",
        };
        assert!(why.starts_with(from), "{kind:?}: {why}");
        match kind {
            Kind::Tell | Kind::Comment => assert_eq!(said["delivery"], "missed", "{kind:?}"),
            Kind::Restart => assert_eq!(said["resumed"], false, "{kind:?}: it waits on"),
            Kind::Red => {}
        }
    }
}

#[tokio::test]
async fn a_message_its_session_cannot_take_is_refused_to_whoever_asked_and_a_red_sends_its_worker_to_the_watch() {
    for kind in KINDS {
        let w = World::new();
        let wk = w.working(CLAUDE, "t").await;
        kind.prepare(&w, &wk).await;
        w.host.orca.fault("send-refused", true);
        let said = kind.say(&w, &wk, false).await;
        let m = last(&w, &wk).await;
        assert_eq!(m["outcome"]["outcome"], "not_typed", "{kind:?}");
        assert!(m["outcome"]["why"].as_str().unwrap().contains("terminal_not_writable"), "{kind:?}: {m}");
        match kind {
            Kind::Tell | Kind::Restart => crate::fixture::refused(said, "orca"),
            Kind::Comment => {
                let said = said.unwrap();
                assert_eq!(said["delivery"], "none");
                assert!(said["undelivered"].as_str().unwrap().contains("terminal_not_writable"), "{said}");
            }
            Kind::Red => {
                let why = worker(&w.app).await["why"].as_str().unwrap().to_string();
                assert!(why.contains("did not reach it") && why.contains("terminal_not_writable"), "{why}");
            }
        }
        // Not typed, it is not tried again: whoever said it heard so.
        w.host.orca.fault("send-refused", false);
        let n = w.terminal(wk["terminal"].as_str().unwrap()).sent.len();
        w.advance(1);
        w.tick().await;
        assert_eq!(w.terminal(wk["terminal"].as_str().unwrap()).sent.len(), n, "{kind:?}");
        if matches!(kind, Kind::Comment) {
            // Its terminal gone, a comment says so.
            w.kill_terminal(wk["terminal"].as_str().unwrap());
            let said = kind.say(&w, &wk, false).await.unwrap();
            assert!(said["undelivered"].as_str().unwrap().contains("gone"), "{said}");
        }
    }
}

/// truefit's orca-claude `refused-after-key`: the interrupt key went, so the
/// agent's turn is stopped (here its program ends), and Orca refuses what comes after.
#[tokio::test]
async fn told_now_when_orca_refuses_once_the_key_went_whoever_asked_hears_the_turn_was_stopped_and_nothing_is_retyped()
{
    for kind in [Kind::Tell, Kind::Restart] {
        let w = World::new();
        let wk = w.working(CLAUDE, "t").await;
        let handle = wk["terminal"].as_str().unwrap();
        kind.prepare(&w, &wk).await;
        w.set_busy(handle, true, false);
        w.host.orca.fault("refused-after-key", true);
        let said = kind.say(&w, &wk, true).await;
        assert_eq!(w.terminal(handle).sent.last().map(String::as_str), Some(ESCAPE), "{kind:?}: the key went");
        assert!(!w.terminal(handle).busy, "{kind:?}: its turn was stopped");
        let m = last(&w, &wk).await;
        assert_eq!(m["outcome"]["outcome"], "not_typed", "{kind:?}");
        let why = m["outcome"]["why"].as_str().unwrap();
        assert!(why.contains("terminal_exited") && why.contains("turn was stopped first"), "{kind:?}: {why}");
        crate::fixture::refused(said, "orca");
        // Whoever said it heard so: it is not typed again.
        w.host.orca.fault("refused-after-key", false);
        let n = w.terminal(handle).sent.len();
        w.advance(1);
        w.tick().await;
        assert_eq!(w.terminal(handle).sent.len(), n, "{kind:?}");
    }
}

#[tokio::test]
async fn before_its_harness_is_up_the_brief_is_held_and_what_comes_after_waits_on_it() {
    for kind in KINDS {
        let w = World::new();
        let wk = w.at_shell().await;
        let handle = wk["terminal"].as_str().unwrap();
        kind.prepare(&w, &wk).await;
        let said = kind.say(&w, &wk, false).await;
        assert!(w.terminal(handle).sent.is_empty(), "{kind:?}: the shell's prompt is not typed at");
        let brief = std::fs::read_to_string(w.data().brief("web-1-1")).unwrap();
        match kind {
            // In the brief, read once handed.
            Kind::Comment => {
                assert_eq!(said.unwrap()["delivery"], "brief");
                assert!(brief.ends_with("\n## Comments since\n\n- use v2\n"), "{brief}");
            }
            Kind::Tell | Kind::Restart => crate::fixture::refused(said, "not_up"),
            Kind::Red => {
                let why = worker(&w.app).await["why"].as_str().unwrap().to_string();
                assert!(why.contains("did not reach it") && why.contains("not up"), "{why}");
            }
        }
        // Up, the brief is typed, once, and nothing after it.
        w.set_screen(handle, &[SHELL, "Welcome to Kimi Code!", UP]);
        w.advance(1);
        w.tick().await;
        w.advance(1);
        w.tick().await;
        let sent = w.terminal(handle).sent;
        assert!(sent.len() == 1 && sent[0].contains("web-1-1.md"), "{kind:?}: {sent:?}");
        let outbox = w.app.outbox_of(Some(id(&wk))).await.unwrap();
        assert_eq!(outbox["messages"][0]["from"], json!({ "kind": "brief" }), "the first message");
        assert_eq!(outbox["messages"][0]["outcome"]["outcome"], "typed");
        if matches!(kind, Kind::Comment) {
            // Once handed, the next one is typed after it, not put in it.
            assert_eq!(kind.say(&w, &wk, false).await.unwrap()["delivery"], "submitted");
            assert_eq!(w.terminal(handle).sent.len(), 2);
            let brief = std::fs::read_to_string(w.data().brief("web-1-1")).unwrap();
            assert_eq!(brief.matches("- use v2").count(), 1, "{brief}");
        }
    }
}

#[tokio::test]
async fn a_round_the_watch_s_session_missed_is_passed_again_on_the_next_look() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    w.app.worker_escalate(id(&wk), "which API version?").await.unwrap();
    w.watch_session(false);
    let watch = w.app.outbox_of(None).await.unwrap()["terminal"].as_str().unwrap().to_string();
    w.open_dialog(&watch, &["Try the new thing?", "› Yes", "Enter to confirm"]);
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"]["n"], 1);
    let outbox = w.app.outbox_of(None).await.unwrap();
    let m = &outbox["messages"][0];
    assert_eq!((&m["from"], &m["outcome"]["outcome"]), (&json!({ "kind": "round", "n": 1 }), &json!("missed")));
    assert!(m["text"].as_str().unwrap().starts_with("factorium round 1: "), "{m}");

    w.host.orca.terminals.lock().unwrap().get_mut(&watch).unwrap().dialog = false;
    w.set_screen(&watch, &[UP]);
    w.advance(5);
    w.tick().await;
    let outbox = w.app.outbox_of(None).await.unwrap();
    let m = &outbox["messages"][1];
    assert_eq!((&m["from"]["n"], &m["outcome"]["outcome"]), (&json!(1), &json!("submitted")), "{outbox}");
    w.advance(5);
    w.tick().await;
    assert_eq!(w.app.outbox_of(None).await.unwrap()["messages"].as_array().unwrap().len(), 2, "arrived: not again");
    assert_eq!(w.terminal(&watch).sent.len(), 2);
}

/// factorium#289: rollmind#144's worker was told twice while its turn ran;
/// both answered `queued`, its turn ended, and neither was ever typed. A
/// message queued behind a running turn is typed again the first time its
/// session is idle at its prompt, and only then.
#[tokio::test]
async fn a_message_queued_behind_a_running_turn_is_typed_once_the_turn_ends() {
    for kind in KINDS {
        let w = World::new();
        let wk = w.working(CLAUDE, "t").await;
        let handle = wk["terminal"].as_str().unwrap();
        kind.prepare(&w, &wk).await;
        w.set_busy(handle, true, false);
        kind.say(&w, &wk, false).await.unwrap();
        assert_eq!(last(&w, &wk).await["outcome"]["outcome"], "queued", "{kind:?}");
        let typed = w.terminal(handle).sent.len();
        // Its turn runs on: nothing is typed again.
        w.advance(1);
        w.tick().await;
        assert_eq!(w.terminal(handle).sent.len(), typed, "{kind:?}: not typed into a running turn twice");
        assert_eq!(last(&w, &wk).await["outcome"]["outcome"], "queued", "{kind:?}");
        // Its turn ends and the session never read it: typed at its prompt, once.
        w.set_busy(handle, false, false);
        w.advance(1);
        w.tick().await;
        let sent = w.terminal(handle).sent;
        assert_eq!(sent.len(), typed + 1, "{kind:?}: {sent:?}");
        assert!(sent[typed].starts_with(kind.opens()), "{kind:?}: {sent:?}");
        assert_eq!(last(&w, &wk).await["outcome"]["outcome"], "submitted", "{kind:?}");
        w.advance(1);
        w.tick().await;
        assert_eq!(w.terminal(handle).sent.len(), typed + 1, "{kind:?}: submitted, not again");
    }
}

/// factorium#289: a message that waits queued past its bound is not left to
/// silence: it is missed, and its worker goes to the watch once.
#[tokio::test]
async fn a_message_still_queued_past_its_bound_is_missed_and_its_worker_goes_to_the_watch() {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
    let handle = wk["terminal"].as_str().unwrap();
    w.set_busy(handle, true, false);
    w.app.watch_tell(id(&wk), "use v2", false).await.unwrap();
    w.advance(9);
    w.tick().await;
    assert_eq!(last(&w, &wk).await["outcome"]["outcome"], "queued");
    w.set_busy(handle, true, false);
    w.advance(2);
    w.tick().await;
    let m = last(&w, &wk).await;
    assert_eq!(m["outcome"]["outcome"], "missed", "{m}");
    assert!(m["outcome"]["why"].as_str().unwrap().contains("queued"), "{m}");
    let waiting = worker(&w.app).await;
    assert_eq!(waiting["status"], "waiting");
    assert!(waiting["why"].as_str().unwrap().starts_with("the watch's word did not reach it"), "{waiting}");
    assert_eq!(w.terminal(handle).sent.len(), 1, "never typed again into its running turn");
}
