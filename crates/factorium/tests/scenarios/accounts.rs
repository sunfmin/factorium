//! Accounts are stablehand's (factorium#288): a worker starts on the
//! account `stablehand pick` takes, told the sessions factorium runs on
//! each; its session is handed its login by `stablehand credential`, a token by a
//! file, a bound login's files linked into a home kept for the account;
//! a limit its screen shows is told to `stablehand limit`; and
//! `factorium accounts` answers `stablehand status`.

use factorium_harness::Harness;
use serde_json::json;

use crate::fixture::{CLAUDE, World, quoted_word, refused, worker, write_session};
use factorium::host::Host;
use factorium_domain::issue::On;

/// This machine's name, as a bound login's machine names it.
pub(crate) fn this_machine() -> String {
    let out = std::process::Command::new("hostname").arg("-s").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A codex account `name` bound to this machine, its home where stablehand
/// keeps it, wherever that is: that home.
fn bound(w: &World, name: &str) -> std::path::PathBuf {
    let home = w.host.home().join("logins").join(name);
    std::fs::create_dir_all(&home).unwrap();
    w.host.stablehand.lock().unwrap().bound(name, "codex", &this_machine(), &home);
    home
}

/// The `stablehand` commands run, oldest first.
fn asked(w: &World) -> Vec<String> {
    let ran = w.host.ran.lock().unwrap();
    ran.iter().filter_map(|r| r.strip_prefix("stablehand ")).map(str::to_string).collect()
}

#[tokio::test]
async fn a_worker_starts_on_stablehand_s_pick_its_secret_off_the_command_line_and_its_file_gone_once_read() {
    // A data directory a shell would split, or end a quote in: the secrets
    // file's path is one word of the line all the same.
    let w = World::on("it's a dir $HOME/factorium.db");
    // A secret a shell would split, expand or end a quote in: one value all the same.
    let secret = "sk-ant-it's $HOME `x` \"very\" secret";
    w.host.stablehand.lock().unwrap().portable("team", "claude-code", secret);
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let secrets = w.data().secrets();
    *w.host.orca.refuse_open.lock().unwrap() = true;
    let refused = w.app.tick().await.unwrap_err();
    assert!(refused.message.contains("orca terminal create: terminal_create_failed"), "not the kit's: {refused:?}");
    // Refused, not unknown: its worktree is not given to Orca as a repo.
    assert!(
        !w.host.ran.lock().unwrap().iter().any(|r| r.starts_with("orca repo add --path") && r.contains("worktrees"))
    );
    assert_eq!(std::fs::read_dir(&secrets).unwrap().count(), 0, "a terminal that never opened leaves no secret");
    *w.host.orca.refuse_open.lock().unwrap() = false;
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!(wk["account"], "team");
    let pick = format!("pick --machine {}", this_machine());
    assert!(asked(&w).contains(&pick), "on this machine, none running yet: {:?}", asked(&w));
    assert!(asked(&w).contains(&"credential team --json".to_string()), "{:?}", asked(&w));
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    assert!(!line.contains("sk-ant-"), "{line}");
    let bin = std::env::current_exe().unwrap().parent().unwrap().to_string_lossy().into_owned();
    assert!(
        line.contains(&format!("PATH='{bin}:")) || line.contains(&format!("PATH={bin}:")),
        "factorium by name: {line}"
    );
    // The kit's `claude` is named by name: no directory of its own, so no
    // empty entry, which would search the session's working directory.
    assert!(!line.contains(&format!("{bin}::")), "{line}");
    // What the shell does with the line's start: read the secret, remove the file.
    let parts: Vec<&str> = line.splitn(3, " && ").collect();
    let file = quoted_word(parts[0], 2);
    assert!(file.starts_with(&secrets.to_string_lossy().into_owned()), "{file}");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    let shell = format!("{} && {} && printenv CLAUDE_CODE_OAUTH_TOKEN", parts[0], parts[1]);
    let out = std::process::Command::new("sh").args(["-c", &shell]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n'), secret);
    assert!(!std::path::Path::new(&file).exists());
}

#[tokio::test]
async fn stablehand_is_told_the_sessions_factorium_runs_on_each_account_and_a_full_one_takes_no_more() {
    let w = World::new();
    {
        let mut books = w.host.stablehand.lock().unwrap();
        books.portable("team", "claude-code", "sk-1");
        books.accounts.get_mut("team").unwrap().sessions = Some(1);
    }
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "one", "", false, &[]).await.unwrap();
    w.app.issue_add("web", "two", "", false, &[]).await.unwrap();
    assert!(w.tick().await.started.is_some());
    assert_eq!(worker(&w.app).await["account"], "team");
    let t = w.tick().await;
    assert!(t.started.is_none(), "{t:?}");
    let pick = format!("pick --machine {} --running team=1", this_machine());
    assert!(asked(&w).contains(&pick), "{:?}", asked(&w));
    assert!(t.held.as_deref().unwrap().contains("stablehand picks none (team: full)"), "{t:?}");
    let envs = asked(&w).iter().filter(|a| a.starts_with("credential ")).count();
    assert_eq!(envs, 1, "the secret is read for the one start, not for each look at a waiting issue");
}

#[tokio::test]
async fn a_bound_login_s_sessions_start_in_a_home_of_their_own_its_login_linked_and_their_turns_are_read_there() {
    let w = World::new();
    let account = bound(&w, "night");
    // The host user's own codex login: not the session's.
    let own = w.host.home().join(".codex/auth.json");
    std::fs::create_dir_all(own.parent().unwrap()).unwrap();
    std::fs::write(&own, "{}").unwrap();
    let wk = w.working("codex=codex", "t").await;
    assert_eq!(wk["account"], "night");
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    // Its home is the one this host keeps for the account, not the
    // account's: only its login file is linked in from there.
    let home = w.data().kept_on("codex", "night");
    assert_eq!(line.matches("CODEX_HOME=").count(), 1, "{line}");
    let at = home.display();
    assert!(line.contains(&format!("CODEX_HOME='{at}'")) || line.contains(&format!("CODEX_HOME={at} ")), "{line}");
    assert_eq!(std::fs::read_link(home.join("auth.json")).unwrap(), account.join("auth.json"));
    assert!(!line.starts_with(". "), "a bound login has no secret to read: {line}");

    let cwd = wk["worktree"].as_str().unwrap();
    let now = (w.app_now() / 1000) as u64;
    write_session(&w, Harness::Codex, Some("night"), cwd, true, now);
    w.advance(45);
    w.tick().await;
    assert_eq!(worker(&w.app).await["status"], "working", "its open turn, in the login's home, is read");

    // The watch on the same account: its turn is read there too.
    w.app.worker_escalate(crate::fixture::id(&wk), "which API version?").await.unwrap();
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"], json!(null), "its session is not up yet");
    let watch = w.data().watch();
    let at = (w.app_now() / 1000) as u64;
    write_session(&w, Harness::Codex, Some("night"), &watch.to_string_lossy(), false, at);
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"]["n"], 1, "its idle turn, in the login's home, is read");
}

#[tokio::test]
async fn a_used_up_account_is_told_to_stablehand_and_no_worker_starts_on_it_until_it_lifts() {
    let w = World::new();
    bound(&w, "night");
    let wk = w.working("codex=codex", "t").await;
    assert_eq!(wk["account"], "night");
    w.set_screen(
        wk["terminal"].as_str().unwrap(),
        &["■ You've hit your usage limit reached.", "Increase your limits to continue using Codex."],
    );
    let t = w.tick().await;
    assert_eq!(t.seen[0]["ended"], "quota");
    assert!(crate::fixture::closed(&w.app, wk["terminal"].as_str().unwrap()).await, "its terminal is closed");
    assert!(asked(&w).contains(&"limit night".to_string()), "no reset on its screen: {:?}", asked(&w));
    assert!(t.started.is_none() && t.held.as_deref().unwrap().contains("(night: limit)"), "{t:?}");
    // No rest of factorium's own: the limit holds until stablehand lifts it.
    w.advance(120);
    assert!(w.tick().await.started.is_none(), "still at its limit");
    assert_eq!(w.app.accounts().await.unwrap()[0]["pace"], "used_up");
    w.host.stablehand.lock().unwrap().read("night");
    assert!(w.tick().await.started.is_some(), "read again, it takes a session");
    let issue = w.app.issue_show("web", 1).await.unwrap();
    assert_ne!(issue["worker"], wk["worker"], "its issue starts again, on a new worker");
}

#[tokio::test]
async fn an_engine_s_limit_with_its_reset_is_told_to_stablehand_with_that_time() {
    let w = World::new();
    let home = w.host.home().join("logins/sky");
    std::fs::create_dir_all(&home).unwrap();
    w.host.stablehand.lock().unwrap().bound("sky", "agy", &this_machine(), &home);
    let wk = w.working("agy=agy", "t").await;
    assert_eq!(wk["account"], "sky");
    w.set_screen(
        wk["terminal"].as_str().unwrap(),
        &["⚠ Individual quota reached. Please upgrade your subscription to increase your limits. Resets in 2h.", ""],
    );
    let until = w.app_now() + 2 * 3_600_000;
    let t = w.tick().await;
    assert_eq!(t.seen[0]["limited"], until, "{t:?}");
    let at = factorium_domain::when::utc(until + 999);
    assert!(asked(&w).contains(&format!("limit sky --resets-at {at}")), "{:?}", asked(&w));
    assert_eq!(w.host.stablehand.lock().unwrap().limits["sky"], Some(until / 1000 * 1000));
}

#[tokio::test]
async fn a_limit_stablehand_cannot_take_is_said_and_the_worker_still_ends() {
    let w = World::new();
    bound(&w, "night");
    let wk = w.working("codex=codex", "t").await;
    w.host.stablehand.lock().unwrap().accounts.remove("night");
    w.set_screen(
        wk["terminal"].as_str().unwrap(),
        &["■ You've hit your usage limit reached.", "Increase your limits to continue using Codex."],
    );
    let t = w.tick().await;
    assert_eq!(t.seen[0]["ended"], "quota");
    assert!(t.seen[0]["unbooked"].as_str().unwrap().contains("no_account"), "{t:?}");
}

#[tokio::test]
async fn an_account_stablehand_does_not_keep_or_no_session_may_start_on_here_is_refused_at_add() {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    {
        let mut books = w.host.stablehand.lock().unwrap();
        books.bound("away", "codex", "some-other-mac", std::path::Path::new("/nowhere"));
        books.portable("out", "claude-code", "sk-1");
        books.accounts.get_mut("out").unwrap().logged_in = false;
        books.portable("ok", "claude-code", "sk-2");
    }
    let on = |account: &str| On { role: None, account: Some(account.into()), model: None };
    let why = |r: factorium::Answer| r.unwrap_err().message;
    let add = |a: &'static str| w.app.issue_add_on("web", "t", "", None, &[], on(a));
    refused(add("ghost").await, "no_account");
    assert!(why(add("ghost").await).contains("no account `ghost`"));
    assert!(why(add("away").await).contains("its login is bound to `some-other-mac`"));
    assert!(why(add("out").await).contains("it is not logged in: `stablehand account login out`"));
    assert_eq!(add("ok").await.unwrap()["account"], "ok");
}

#[tokio::test]
async fn without_stablehand_a_worker_starts_on_claude_code_alone_on_the_host_s_own_login_and_doctor_says_why() {
    // Not found, and found but answering a fault: the same fallback.
    for broken in ["missing", "fault"] {
        let w = World::new();
        w.host.stablehand.lock().unwrap().portable("team", "claude-code", "sk-1");
        w.checkout("web").await;
        // codex first: without stablehand no account of it, nor its windows, is known.
        w.kit_of(&["codex=codex", CLAUDE]).await;
        // The command stablehand keeps, which it cannot be asked for: not the fallback's.
        w.stablehand_commands(&["claude-code=claude --kept"]);
        match broken {
            "missing" => w.host.missing.lock().unwrap().push("stablehand".into()),
            _ => w.host.stablehand.lock().unwrap().fault = Some("its books are of a schema it cannot read".into()),
        }
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        let found = w.finding("stablehand").await.unwrap_or_else(|| panic!("{broken}: a finding"));
        let (what, fix) = (found["what"].as_str().unwrap(), found["fix"].as_str().unwrap());
        assert!(what.contains("stablehand cannot say which account"), "{broken}: {what}");
        match broken {
            "missing" => assert!(fix.contains("brew install sunfmin/tap/stablehand"), "{fix}"),
            _ => assert!(fix.contains("its books are of a schema it cannot read"), "the fault's own words: {fix}"),
        }
        let t = w.tick().await;
        assert!(t.started.is_some(), "{broken}: {t:?}");
        let wk = worker(&w.app).await;
        assert_eq!((wk["harness"].clone(), wk["account"].clone()), (json!("claude-code"), json!(null)), "{broken}");
        let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
        assert!(!line.starts_with(". "), "{broken}: no account's secret: {line}");
        assert!(line.contains("claude --dangerously-skip-permissions"), "{broken}: the built-in command: {line}");
        assert!(!line.contains("--kept"), "{broken}: {line}");

        // stablehand answers again: starts go through it, and the finding is gone.
        w.host.missing.lock().unwrap().clear();
        w.host.stablehand.lock().unwrap().fault = None;
        assert!(w.finding("stablehand").await.is_none(), "{broken}");
        w.app.issue_add("web", "u", "", false, &[]).await.unwrap();
        assert!(w.tick().await.started.is_some());
        // stablehand's order again: codex first.
        assert_eq!(w.worker_on("web", 2).await["harness"], "codex", "{broken}");
        assert!(w.host.ran.lock().unwrap().iter().any(|r| r.starts_with("stablehand pick --machine")));
    }
}

#[tokio::test]
async fn factorium_accounts_answers_stablehand_s_status() {
    let w = World::new();
    w.host.stablehand.lock().unwrap().portable("team", "claude-code", "sk-ant-very-secret");
    bound(&w, "night");
    let accounts = w.app.accounts().await.unwrap();
    let names: Vec<_> = accounts.as_array().unwrap().iter().map(|a| a["name"].clone()).collect();
    assert_eq!(names, [json!("night"), json!("team")]);
    assert!(!accounts.to_string().contains("sk-ant"), "{accounts}");
    assert_eq!(asked(&w), ["status"]);
}

#[tokio::test]
async fn the_watch_starts_on_its_account_s_secret() {
    let w = World::new();
    w.host.stablehand.lock().unwrap().portable("team", "claude-code", "sk-ant-very-secret");
    let wk = w.working(CLAUDE, "t").await;
    w.app.worker_escalate(crate::fixture::id(&wk), "which API version?").await.unwrap();
    w.advance(5);
    w.tick().await;
    let t = w.host.orca.terminals.lock().unwrap();
    let watch = t.values().find(|t| t.title == "factorium watch").expect("the watch's terminal");
    let reads = watch.line.starts_with(". ") && watch.line.contains(&format!("{}/", w.data().secrets().display()));
    assert!(reads && !watch.line.contains("sk-ant"), "it reads its secret from a file first");
}

#[tokio::test]
async fn an_account_s_login_file_takes_the_place_of_the_host_user_s_link_that_would_hold_it() {
    let w = World::new();
    // kimi links the host user's whole `credentials` directory; an
    // account's login is one file in it.
    let own = w.host.home().join(".kimi-code/credentials");
    std::fs::create_dir_all(&own).unwrap();
    let account = w.host.home().join("logins/moon");
    std::fs::create_dir_all(&account).unwrap();
    w.host.stablehand.lock().unwrap().bound("moon", "kimi", &this_machine(), &account);
    let wk = w.working("kimi=kimi", "t").await;
    assert_eq!(wk["account"], "moon");
    let home = w.data().kept_on("kimi", "moon");
    let credentials = home.join("credentials");
    assert!(!credentials.is_symlink(), "not the host user's: {}", credentials.display());
    let linked = std::fs::read_link(credentials.join("kimi-code.json")).unwrap();
    assert_eq!(linked, account.join("credentials/kimi-code.json"));
    assert!(std::fs::read_dir(&own).unwrap().next().is_none(), "nothing is written into the host user's");
}
