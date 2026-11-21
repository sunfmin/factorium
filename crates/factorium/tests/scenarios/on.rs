//! An issue that names the role its worker plays and the account it starts
//! on (factorium#165): its worker starts with that role's rules and skills
//! beside factorium's, on that account whatever the kit's order; and
//! `issue show` answers what the issue took, its time and its tokens.

use factorium::host::Host;
use factorium_domain::issue::On;
use factorium_harness::{Harness, RULES, SKILLS};
use serde_json::json;

use crate::fixture::{CLAUDE, World, id, refused, worker, write_session_in};

/// A claude-code account `name` in mytokens.
fn account(w: &World, name: &str) {
    let fields = json!({"fields": {"HARNESS": "claude-code", "CLAUDE_CODE_OAUTH_TOKEN": format!("sk-{name}")}});
    w.host.tokens.lock().unwrap().insert(format!("agent-account-{name}"), fields.to_string());
}

#[tokio::test]
async fn an_issue_s_worker_starts_as_its_role_on_its_account_and_its_time_and_tokens_are_read_back() {
    let w = World::new();
    w.checkout("web").await;
    // pi first: a worker of an issue naming no account would start on it.
    w.app.kit_set(&["pi=pi".into(), CLAUDE.into()]).await.unwrap();
    account(&w, "a");
    account(&w, "guild");
    // Pinned to the account's harness, the checkout starts it.
    w.app.kit_pin("web", Some("claude-code")).await.unwrap();
    let role = w.host.home().join("lore/roles/reviewer");
    std::fs::create_dir_all(role.join(SKILLS).join("review")).unwrap();
    std::fs::write(role.join(RULES), "You are the reviewer.\n").unwrap();
    std::fs::write(role.join(SKILLS).join("review/SKILL.md"), "Review the diff.\n").unwrap();
    let role_text = role.to_string_lossy().into_owned();

    // A role not named by an absolute directory, or an account not here, is refused.
    let on = |role: &str, account: &str| On { role: Some(role.into()), account: Some(account.into()) };
    refused(w.app.issue_add_on("web", "t", "", false, &[], on("lore/roles/reviewer", "guild")).await, "no_role");
    refused(w.app.issue_add_on("web", "t", "", false, &[], on(&role_text, "nobody")).await, "no_account");

    let shown = w.app.issue_add_on("web", "Review it", "", false, &[], on(&role_text, "guild")).await.unwrap();
    assert_eq!((shown["role"].clone(), shown["account"].clone()), (json!(role_text), json!("guild")));
    assert_eq!(shown["spent"], json!({ "ms": 0, "tokens": null, "attempts": [] }), "nothing taken yet");
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!((wk["harness"].clone(), wk["account"].clone()), (json!("claude-code"), json!("guild")), "{wk}");

    // It starts in a directory of its own, with factorium's rules and its role's, and its role's skills.
    let start = w.data().root().join("role/issues/web-1-1");
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    assert!(line.contains(&format!("cd '{}'", start.display())), "{line}");
    let rules = std::fs::read_to_string(start.join(RULES)).unwrap();
    assert!(rules.contains("You are a factorium worker") && rules.contains("You are the reviewer."), "{rules}");
    assert_eq!(std::fs::read_to_string(start.join(SKILLS).join("review/SKILL.md")).unwrap(), "Review the diff.\n");
    let held: Vec<_> = std::fs::read_dir(&role).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(held.len(), 2, "the role's directory is read, never written: {held:?}");

    // Live, its time runs to now and its tokens are what its session files say.
    let brief = w.data().brief("web-1-1").to_string_lossy().into_owned();
    let at = (w.app_now() / 1000) as u64;
    write_session_in(&w, Harness::ClaudeCode, Some("guild"), &start.to_string_lossy(), &brief, false, at);
    w.advance(30);
    let spent = w.app.issue_show("web", 1).await.unwrap()["spent"].clone();
    assert_eq!((spent["ms"].clone(), spent["tokens"].clone()), (json!(1_800_000), json!({ "in": 15, "out": 11 })));
    assert_eq!(spent["attempts"][0]["ended"], false, "{spent}");

    let tree = crate::fixture::worktree(&wk);
    w.commit(&tree, "review.txt", "ok\n");
    w.app.worker_done(id(&wk)).await.unwrap();
    w.advance(5);
    assert_eq!(w.tick().await.landed[0]["landed"], "web#1");
    // Ended, what it spent was kept as it ended: its harness may clear its session files.
    std::fs::remove_dir_all(w.host.home().join(".claude/projects")).unwrap();
    let issue = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!(issue["status"], "closed");
    let spent = &issue["spent"];
    assert_eq!((spent["ms"].clone(), spent["tokens"].clone()), (json!(2_100_000), json!({ "in": 15, "out": 11 })));
    let first = &spent["attempts"][0];
    assert_eq!((first["ended"].clone(), first["account"].clone()), (json!(true), json!("guild")), "{spent}");

    // An issue naming no role starts in the worker's own directory, on the kit's first.
    w.app.kit_pin("web", None).await.unwrap();
    w.app.issue_add("web", "Plain", "", false, &[]).await.unwrap();
    w.tick().await;
    let plain = w.worker_on("web", 2).await;
    assert_eq!((plain["harness"].clone(), plain["account"].clone()), (json!("pi"), json!(null)), "{plain}");
    let line = w.terminal(plain["terminal"].as_str().unwrap()).line;
    assert!(!line.contains("role/issues"), "{line}");
}

#[tokio::test]
async fn an_issue_whose_account_is_used_up_waits_behind_those_that_can_start() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&["codex=codex".into(), CLAUDE.into()]).await.unwrap();
    let night = w.host.home().join(".agent-accounts/night");
    std::fs::create_dir_all(&night).unwrap();
    std::fs::write(night.join("auth.json"), "{}").unwrap();
    let fields = json!({"fields": {"HARNESS": "codex", "MACHINE": crate::accounts::this_machine()}});
    w.host.tokens.lock().unwrap().insert("agent-account-night".into(), fields.to_string());
    account(&w, "guild");
    let on_night = On { role: None, account: Some("night".into()) };
    w.app.issue_add_on("web", "On the night account", "", false, &[], on_night).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!((wk["harness"].clone(), wk["account"].clone()), (json!("codex"), json!("night")));
    w.set_screen(
        wk["terminal"].as_str().unwrap(),
        &["■ You've hit your usage limit reached.", "Increase your limits to continue using Codex."],
    );
    // Its issue is ready again, and alone it holds admission, saying why.
    let t = w.tick().await;
    assert_eq!(t.seen[0]["ended"], "quota");
    assert!(t.held.as_deref().unwrap().contains("account night is used up"), "{t:?}");

    // A role with no skills, and no account: it starts, ahead of the waiting one.
    let role = w.host.home().join("roles/plain");
    std::fs::create_dir_all(&role).unwrap();
    let on_role = On { role: Some(role.to_string_lossy().into_owned()), account: None };
    let shown = w.app.issue_add_on("web", "A role alone", "", false, &[], on_role).await.unwrap();
    assert_eq!((shown["role"].clone(), shown["account"].clone()), (json!(role.to_string_lossy()), json!(null)));
    let t = w.tick().await;
    assert_eq!(t.started.as_ref().map(|s| s["issue"].clone()), Some(json!("web#2")), "{t:?}");
    assert_eq!(w.worker_on("web", 2).await["account"], "guild", "the kit's choice: night is used up");
    let skills = w.data().root().join("role/issues/web-2-1").join(SKILLS);
    assert!(skills.is_dir() && std::fs::read_dir(&skills).unwrap().count() == 0, "no skills of its own");
}
