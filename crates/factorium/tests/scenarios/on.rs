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

/// A role directory under the world's home, with rules `rules` and skill `skill`.
fn role_at(w: &World, name: &str, rules: &str, skill: &str) -> String {
    let role = w.host.home().join("roles").join(name);
    std::fs::create_dir_all(role.join(SKILLS).join(skill)).unwrap();
    std::fs::write(role.join(RULES), rules).unwrap();
    std::fs::write(role.join(SKILLS).join(skill).join("SKILL.md"), format!("{skill}\n")).unwrap();
    role.to_string_lossy().into_owned()
}

#[tokio::test]
async fn an_issue_filed_with_no_role_is_edited_to_name_one_and_its_next_worker_starts_as_it() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&[CLAUDE.into()]).await.unwrap();
    w.app.issue_add("web", "Build the app", "", false, &[]).await.unwrap();
    let role = role_at(&w, "ios", "You are the ios-app-coder.\n", "xcode");

    let shown = w.app.issue_name_role("web", 1, Some(&role)).await.unwrap();
    assert_eq!(shown["role"], json!(role), "issue show answers the role as edited");
    let said = shown["comments"][0].clone();
    assert_eq!(said["by"], "factorium");
    assert!(said["text"].as_str().unwrap().contains(&format!("from no role to `{role}`")), "{said}");

    w.tick().await;
    let start = w.data().root().join("role/issues/web-1-1");
    let rules = std::fs::read_to_string(start.join(RULES)).unwrap();
    assert!(rules.ends_with("You are the ios-app-coder.\n"), "{rules}");
    assert_eq!(std::fs::read_to_string(start.join(SKILLS).join("xcode/SKILL.md")).unwrap(), "xcode\n");
    let brief = std::fs::read_to_string(w.data().brief("web-1-1")).unwrap();
    assert!(brief.contains("changed this issue's role from no role"), "its brief carries why: {brief}");
}

#[tokio::test]
async fn a_live_worker_keeps_the_role_it_started_with_and_the_next_attempt_takes_the_edited_one() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&[CLAUDE.into()]).await.unwrap();
    let first = role_at(&w, "first", "You are the first.\n", "one");
    let on = On { role: Some(first.clone()), account: None };
    w.app.issue_add_on("web", "t", "", false, &[], on).await.unwrap();
    w.tick().await;
    let wk = w.worker_on("web", 1).await;
    let start = w.data().root().join("role/issues/web-1-1");
    let rules_before = std::fs::read_to_string(start.join(RULES)).unwrap();

    let second = role_at(&w, "second", "You are the second.\n", "two");
    w.app.issue_name_role("web", 1, Some(&second)).await.unwrap();
    w.tick().await;
    assert_eq!(w.worker_on("web", 1).await["id"], wk["id"], "the edit restarts no one");
    assert_eq!(std::fs::read_to_string(start.join(RULES)).unwrap(), rules_before);
    assert!(start.join(SKILLS).join("one/SKILL.md").is_file(), "its start directory is as it was");

    // The attempt that starts after the edit starts with the new role.
    w.app.issue_wait_for_human("web", 1, "again").await.unwrap();
    w.app.issue_ready("web", 1).await.unwrap();
    w.tick().await;
    let next = w.data().root().join("role/issues/web-1-2");
    assert!(std::fs::read_to_string(next.join(RULES)).unwrap().ends_with("You are the second.\n"));
    assert!(next.join(SKILLS).join("two/SKILL.md").is_file());
}

#[tokio::test]
async fn no_role_takes_the_role_away_and_a_missing_or_relative_dir_is_refused_as_at_add() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&[CLAUDE.into()]).await.unwrap();
    let role = role_at(&w, "r", "You are r.\n", "s");
    w.app.issue_add_on("web", "t", "", false, &[], On { role: Some(role.clone()), account: None }).await.unwrap();

    refused(w.app.issue_name_role("web", 1, Some("roles/r")).await, "no_role");
    let missing = w.host.home().join("roles/none").to_string_lossy().into_owned();
    refused(w.app.issue_name_role("web", 1, Some(&missing)).await, "no_role");
    let shown = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!(
        (shown["role"].clone(), shown["comments"].clone()),
        (json!(role), json!([])),
        "a refusal changes nothing"
    );

    let shown = w.app.issue_name_role("web", 1, None).await.unwrap();
    assert_eq!(shown["role"], json!(null));
    assert!(shown["comments"][0]["text"].as_str().unwrap().contains(&format!("from `{role}` to no role")));
    w.tick().await;
    let line = w.terminal(w.worker_on("web", 1).await["terminal"].as_str().unwrap()).line;
    assert!(!line.contains("role/issues"), "it starts in the worker's own directory: {line}");
}

/// A checkout named `name` on the list, whose default role is `role`.
async fn checkout_as(w: &World, name: &str, role: &str) {
    let path = w.repo(name);
    w.app.checkout_add(name, &path, "main", "proving", Some(role)).await.unwrap();
}

#[tokio::test]
async fn a_checkout_s_default_role_is_the_role_of_a_worker_whose_issue_names_none() {
    let w = World::new();
    w.app.kit_set(&[CLAUDE.into()]).await.unwrap();
    let ios = role_at(&w, "ios", "You are the ios-app-coder.\n", "xcode");
    checkout_as(&w, "web", &ios).await;
    assert_eq!(w.app.checkout_list().await.unwrap()[0]["role"], json!(ios), "checkout list answers it");
    w.app.issue_add("web", "Build the app", "", false, &[]).await.unwrap();
    let shown = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!((shown["role"].clone(), shown["role_from"].clone()), (json!(ios), json!("checkout")), "{shown}");

    w.tick().await;
    let start = w.data().root().join("role/issues/web-1-1");
    assert!(std::fs::read_to_string(start.join(RULES)).unwrap().ends_with("You are the ios-app-coder.\n"));
    assert_eq!(std::fs::read_to_string(start.join(SKILLS).join("xcode/SKILL.md")).unwrap(), "xcode\n");
}

#[tokio::test]
async fn an_issue_s_own_role_wins_over_its_checkout_s_default() {
    let w = World::new();
    w.app.kit_set(&[CLAUDE.into()]).await.unwrap();
    let ios = role_at(&w, "ios", "You are the ios-app-coder.\n", "xcode");
    let docs = role_at(&w, "docs", "You are the writer.\n", "prose");
    checkout_as(&w, "web", &ios).await;
    w.app
        .issue_add_on("web", "Write it up", "", false, &[], On { role: Some(docs.clone()), account: None })
        .await
        .unwrap();
    let shown = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!((shown["role"].clone(), shown["role_from"].clone()), (json!(docs), json!("issue")), "{shown}");

    w.tick().await;
    let start = w.data().root().join("role/issues/web-1-1");
    assert!(std::fs::read_to_string(start.join(RULES)).unwrap().ends_with("You are the writer.\n"));
    assert!(start.join(SKILLS).join("prose/SKILL.md").is_file());
    assert!(!start.join(SKILLS).join("xcode").exists(), "the default's skills are not linked");
}

#[tokio::test]
async fn checkout_edit_names_or_clears_the_default_and_a_worker_then_starts_with_factorium_s_rules_alone() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&[CLAUDE.into()]).await.unwrap();
    let ios = role_at(&w, "ios", "You are the ios-app-coder.\n", "xcode");
    refused(w.app.checkout_name_role("web", Some("roles/ios")).await, "no_role");
    refused(w.app.checkout_name_role("nowhere", Some(&ios)).await, "not_listed");
    assert_eq!(w.app.checkout_name_role("web", Some(&ios)).await.unwrap()["role"], json!(ios));
    assert_eq!(w.app.checkout_name_role("web", None).await.unwrap()["role"], json!(null));
    assert_eq!(w.app.checkout_list().await.unwrap()[0]["role"], json!(null));

    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let shown = w.app.issue_show("web", 1).await.unwrap();
    assert_eq!((shown["role"].clone(), shown["role_from"].clone()), (json!(null), json!(null)), "{shown}");
    w.tick().await;
    let line = w.terminal(w.worker_on("web", 1).await["terminal"].as_str().unwrap()).line;
    assert!(!line.contains("role/issues"), "it starts in the worker's own directory: {line}");
}

#[tokio::test]
async fn each_attempt_answers_the_role_it_started_with_and_a_hash_of_what_that_role_gave_it() {
    let w = World::new();
    w.app.kit_set(&[CLAUDE.into()]).await.unwrap();
    // Under a home of its own the role's path differs between runs: its hash does not.
    let ios = role_at(&w, "ios", "You are the ios-app-coder.\n", "xcode");
    checkout_as(&w, "web", &ios).await;
    w.app.issue_add("web", "Build the app", "", false, &[]).await.unwrap();
    w.tick().await;

    // A skill file of the role changes during the first attempt: the second starts with it.
    std::fs::write(w.host.home().join("roles/ios").join(SKILLS).join("xcode/SKILL.md"), "xcode 2\n").unwrap();
    w.app.issue_wait_for_human("web", 1, "again").await.unwrap();
    w.app.issue_ready("web", 1).await.unwrap();
    w.tick().await;

    let spent = w.app.issue_show("web", 1).await.unwrap()["spent"].clone();
    let (one, two) = (&spent["attempts"][0], &spent["attempts"][1]);
    let role = json!({ "dir": ios, "from": "checkout" });
    assert_eq!((one["role"].clone(), two["role"].clone()), (role.clone(), role), "{spent}");
    // The same content hashes the same from one build to the next: FNV-1a of the rules, then each
    // skill file's path and bytes, each led by its length as 8 little-endian bytes.
    assert_eq!(one["role_hash"], json!("2b87fcb63ebf6484"), "{spent}");
    assert_ne!(one["role_hash"], two["role_hash"], "{spent}");
}

#[tokio::test]
async fn an_attempt_with_no_role_answers_null_for_its_role_and_hash() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&[CLAUDE.into()]).await.unwrap();
    w.app.issue_add("web", "Plain", "", false, &[]).await.unwrap();
    w.tick().await;
    let spent = w.app.issue_show("web", 1).await.unwrap()["spent"].clone();
    let one = &spent["attempts"][0];
    assert_eq!((one["role"].clone(), one["role_hash"].clone()), (json!(null), json!(null)), "{spent}");
    assert!(one.as_object().unwrap().contains_key("role_hash"), "{spent}");
}
