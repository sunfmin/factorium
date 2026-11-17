//! Every harness: a worker of it starts with its own start line and brief,
//! and its session files — written by the harness's own stand-in where its
//! reader looks — say whether its turn is open or closed, and whose it is.

use factorium_harness::Harness;

use crate::{SHELL, UP, World, id, refused, worker};
use factorium::host::Host;

/// Writes the session files a harness's stand-in describes for `cwd`, in
/// the home of its sessions on `account`.
pub fn write_session(w: &World, h: Harness, account: Option<&str>, cwd: &str, open: bool, at: u64) {
    let kept = w.dir.join("data").join("kept").join(h.key());
    h.write_session(&h.home(&w.host.home(), account, &kept), cwd, open, at).unwrap();
}

/// The harnesses that work only in their cwd, so start in the worktree.
const IN_WORKTREE: [&str; 1] = ["grok"];

#[tokio::test]
async fn each_harness_starts_with_its_line_and_its_turn_is_read_from_its_own_session_files() {
    assert!(Harness::all().len() >= 6, "every harness is listed");
    for h in Harness::all() {
        let (key, command) = (h.key(), format!("run-{}", h.key()));
        let w = World::new();
        w.checkout("web").await;
        w.app.kit_set(&[format!("{key}={command}")]).await.unwrap();
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let wk = worker(&w.app).await;
        assert_eq!(wk["harness"], key);
        let term = w.terminal(wk["terminal"].as_str().unwrap());
        assert!(term.line.contains(&command), "{key}: {}", term.line);
        assert!(term.line.contains("FACTORIUM_WORKER"), "{key}: the worker is named in its environment");
        let briefed = term.line.contains("web-1-1.md") || term.sent.iter().any(|s| s.contains("web-1-1.md"));
        assert!(briefed, "{key}: handed its brief");
        if !term.sent.is_empty() {
            // A typed brief waits for the session to be up: sent sooner, the shell takes it.
            let handle = wk["terminal"].as_str().unwrap();
            let ran = w.host.ran.lock().unwrap().clone();
            let at = |verb: &str| {
                ran.iter().position(|r| r.starts_with(&format!("orca terminal {verb} --terminal {handle} ")))
            };
            assert!(
                at("wait").is_some() && at("wait") < at("send"),
                "{key}: the brief is typed once its session is up"
            );
        }

        let h = Harness::read(key).unwrap();
        let cwd = wk["worktree"].as_str().unwrap();
        // Where it starts: its role's directory, or the worktree for one that works only in its cwd.
        let role = w.dir.join("data").join("role").join("worker");
        let dir = if IN_WORKTREE.contains(&key) { cwd.to_string() } else { role.to_string_lossy().into_owned() };
        // DSH's line is quoted once more, as one word of its start script's.
        let starts = term.line.split("cd ").skip(1).any(|rest| rest.trim_start_matches(['\'', '\\']).starts_with(&dir));
        assert!(starts, "{key} starts in {dir}: {}", term.line);
        // A home its line points it nowhere at is the host user's own: the host keeps none for it.
        let kept = w.dir.join("data").join("kept").join(key);
        assert!(term.line.contains(&*kept.to_string_lossy()) || !kept.exists(), "{key}: {}", term.line);
        let now = (w.app_now() / 1000) as u64;
        write_session(&w, h, None, cwd, true, now);
        w.advance(45);
        w.tick().await;
        assert_eq!(worker(&w.app).await["status"], "working", "{key}: its open turn keeps it from silence");

        // Its turn ends; another worktree's session, still open, is not its.
        let ended = (w.app_now() / 1000) as u64;
        write_session(&w, h, None, cwd, false, ended);
        write_session(&w, h, None, &format!("{cwd}-other"), true, ended + 60);
        w.advance(25);
        w.tick().await;
        assert_eq!(worker(&w.app).await["status"], "working", "{key}: idle since its turn ended, within the bound");
        w.advance(10);
        w.tick().await;
        let wk = worker(&w.app).await;
        assert_eq!(wk["status"], "waiting", "{key}: its closed turn, past the bound, is silence: {wk}");
    }
}

#[tokio::test]
async fn the_kit_takes_only_harnesses_factorium_starts_and_names_them_all() {
    let w = World::new();
    w.checkout("web").await;
    let refused = w.app.kit_set(&["vim=vim".into()]).await.unwrap_err();
    assert_eq!(refused.code, "no_harness");
    for h in Harness::all() {
        assert!(refused.message.contains(h.key()), "{}: {}", h.key(), refused.message);
    }
    assert_eq!(w.app.kit_pin("web", Some("vim")).await.unwrap_err().code, "no_harness");
    assert_eq!(w.app.kit().await.unwrap()["entries"], serde_json::json!([]), "nothing set");
    for h in Harness::all() {
        w.app.kit_set(&[format!("{}=x", h.key())]).await.unwrap();
        w.app.kit_pin("web", Some(h.key())).await.unwrap();
    }
}

#[tokio::test]
async fn an_account_is_usable_only_by_the_way_its_harness_logs_in() {
    let w = World::new();
    let machine = this_machine();
    let accounts = [
        ("token", r#"{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk"}"#.to_string()),
        ("bound", format!(r#"{{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk","MACHINE":"{machine}"}}"#)),
        ("tokenless", r#"{"HARNESS":"claude-code"}"#.to_string()),
        ("pi", r#"{"HARNESS":"pi","CLAUDE_CODE_OAUTH_TOKEN":"sk"}"#.to_string()),
        ("login", format!(r#"{{"HARNESS":"codex","MACHINE":"{machine}"}}"#)),
        ("login-token", format!(r#"{{"HARNESS":"codex","MACHINE":"{machine}","CLAUDE_CODE_OAUTH_TOKEN":"sk"}}"#)),
        ("nowhere", r#"{"HARNESS":"codex"}"#.to_string()),
    ];
    for (name, fields) in &accounts {
        let home = w.host.home().join(".agent-accounts").join(name);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("auth.json"), "{}").unwrap();
        w.host.tokens.lock().unwrap().insert(format!("agent-account-{name}"), format!(r#"{{"fields":{fields}}}"#));
    }
    let listed = w.app.accounts().await.unwrap();
    let mut names: Vec<&str> = listed.as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["login", "token"], "{listed}");
}

#[tokio::test]
async fn an_account_secret_stays_off_the_command_line_and_its_file_is_gone_once_read() {
    let w = World::new();
    w.host.tokens.lock().unwrap().insert(
        "agent-account-team".into(),
        r#"{"fields":{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk-ant-very-secret"}}"#.into(),
    );
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    let secrets = w.dir.join("data").join("secrets");
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
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    assert!(!line.contains("sk-ant-very-secret"), "{line}");
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
    let file = parts[0].strip_prefix(". ").unwrap().trim_matches('\'').to_string();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    let shell = format!("{} && {} && printenv CLAUDE_CODE_OAUTH_TOKEN", parts[0], parts[1]);
    let out = std::process::Command::new("sh").args(["-c", &shell]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "sk-ant-very-secret");
    assert!(!std::path::Path::new(&file).exists());
}

/// This machine's name, as an account's `MACHINE` names it.
fn this_machine() -> String {
    let out = std::process::Command::new("hostname").arg("-s").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[tokio::test]
async fn doctor_says_why_each_account_cannot_be_used_here_and_sessions_start_on_the_rest() {
    let w = World::new();
    let codex = |machine: &str| format!(r#"{{"fields":{{"HARNESS":"codex","MACHINE":"{machine}"}}}}"#);
    {
        let mut tokens = w.host.tokens.lock().unwrap();
        tokens.insert("agent-account-away".into(), codex("some-other-mac"));
        tokens.insert("agent-account-out".into(), codex(&this_machine()));
        tokens.insert("agent-account-odd".into(), r#"{"fields":{"HARNESS":"codex"}}"#.into());
        tokens.insert("agent-account-blank".into(), codex(" "));
        let claude = r#"{"fields":{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"#;
        tokens.insert("agent-account-tokenless".into(), format!(r#"{claude}"  "}}}}"#));
        tokens.insert("agent-account-extra".into(), format!(r#"{claude}"t","EXTRA":"x"}}}}"#));
        tokens.insert(
            "agent-account-team".into(),
            r#"{"fields":{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk-ant-very-secret"}}"#.into(),
        );
    }

    // `out`'s home is there, but it never logged in.
    std::fs::create_dir_all(w.host.home().join(".agent-accounts").join("out")).unwrap();
    let doctor = w.app.doctor().await.unwrap();
    let skipped = doctor["accounts_skipped"].as_array().unwrap();
    let why = |secret: &str| {
        let s = skipped.iter().find(|s| s["secret"] == secret).unwrap_or_else(|| panic!("{secret}: {doctor}"));
        s["why"].as_str().unwrap().to_string()
    };
    assert_eq!(skipped.len(), 6, "the token account is usable: {doctor}");
    assert_eq!(why("agent-account-blank"), "its `MACHINE` is empty");
    assert_eq!(why("agent-account-tokenless"), "its `CLAUDE_CODE_OAUTH_TOKEN` is empty");
    assert_eq!(why("agent-account-extra"), "`EXTRA` is no field of a claude-code account");
    assert!(why("agent-account-away").contains("its login is on `some-other-mac`"), "{doctor}");
    let home = w.host.home().join(".agent-accounts").join("out");
    let out = why("agent-account-out");
    assert!(out.contains("not logged in here: no `auth.json` in"), "{out}");
    assert!(out.contains(&format!("CODEX_HOME='{}' codex", home.display())), "a login line for a person: {out}");
    assert!(why("agent-account-odd").contains("MACHINE"), "{doctor}");
    assert!(!doctor.to_string().contains("sk-ant-very-secret"));
    assert_eq!(doctor["findings"], serde_json::json!([]), "an account here or not is no finding for the watch");
    let accounts = w.app.accounts().await.unwrap();
    let names: Vec<&str> = accounts.as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["team"]);

    // Logged in, it is usable here and a session starts on it.
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("auth.json"), "{}").unwrap();
    let doctor = w.app.doctor().await.unwrap();
    assert!(doctor["accounts_skipped"].as_array().unwrap().iter().all(|s| s["secret"] != "agent-account-out"));
    w.checkout("web").await;
    w.app.kit_set(&["codex=codex".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    assert_eq!(worker(&w.app).await["account"], "out");
}

#[tokio::test]
async fn doctor_says_when_mytokens_lists_no_account() {
    let w = World::new();
    w.host.missing.lock().unwrap().push("mytokens".into());
    let doctor = w.app.doctor().await.unwrap();
    assert_eq!(doctor["accounts_skipped"][0]["secret"], "mytokens", "{doctor}");
}

#[tokio::test]
async fn a_login_account_s_sessions_start_in_its_home_and_their_turns_are_read_there() {
    let w = World::new();
    let account = w.host.home().join(".agent-accounts").join("night");
    std::fs::create_dir_all(&account).unwrap();
    std::fs::write(account.join("auth.json"), "{}").unwrap();
    let fields = format!(r#"{{"fields":{{"HARNESS":"codex","MACHINE":"{}"}}}}"#, this_machine());
    w.host.tokens.lock().unwrap().insert("agent-account-night".into(), fields);
    w.checkout("web").await;
    w.app.kit_set(&["codex=codex".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!(wk["account"], "night");
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    // One home, the login's: nothing sets it back to another.
    assert_eq!(line.matches("CODEX_HOME=").count(), 1, "{line}");
    let home = account.display();
    assert!(line.contains(&format!("CODEX_HOME='{home}'")) || line.contains(&format!("CODEX_HOME={home} ")), "{line}");

    let cwd = wk["worktree"].as_str().unwrap();
    let now = (w.app_now() / 1000) as u64;
    write_session(&w, Harness::Codex, Some("night"), cwd, true, now);
    w.advance(45);
    w.tick().await;
    assert_eq!(worker(&w.app).await["status"], "working", "its open turn, in the login's home, is read");

    // The watch on the same account: its turn is read there too.
    w.app.worker_escalate(crate::id(&wk), "which API version?").await.unwrap();
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"], serde_json::json!(null), "its session is not up yet");
    let home = w.dir.join("data").join("watch");
    let at = (w.app_now() / 1000) as u64;
    write_session(&w, Harness::Codex, Some("night"), &home.to_string_lossy(), false, at);
    w.advance(5);
    assert_eq!(w.tick().await.watch.unwrap()["round"]["n"], 1, "its idle turn, in the login's home, is read");
}

#[tokio::test]
async fn a_codex_worker_is_told_the_repos_rules_it_does_not_read_from_its_start_directory() {
    let w = World::new();
    let repo = w.repo("web");
    w.commit(&repo, "CLAUDE.md", "Always run the linter.\n");
    w.git(&repo, &["push", "--quiet", "origin", "main"]).unwrap();
    w.app.checkout_add("web", &repo, "main", "proving").await.unwrap();
    w.app.kit_set(&["codex=codex".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    assert!(line.contains("Always run the linter."), "{line}");
    assert!(line.contains(&format!("{}/CLAUDE.md", wk["worktree"].as_str().unwrap())), "{line}");
}

/// What a session's home must hold before its line runs, by harness: the
/// file, and words in it that say it asks the guard. Claude Code and Codex
/// are given their hooks on the line.
const KEPT: [(&str, &str, &str); 6] = [
    ("pi", "factorium.js", "FACTORIUM_GUARD"),
    ("dsh", "start.sh", "factorium.patch.yml"),
    ("dsh", "factorium.patch.yml", "factoriumGuard"),
    ("dsh", "factorium.mjs", "FACTORIUM_GUARD"),
    ("kimi", "config.toml", "guard kimi"),
    ("grok", "config.toml", "guard grok"),
];

#[tokio::test]
async fn each_harness_s_home_holds_its_guard_and_where_its_rules_are_before_its_session_starts() {
    for key in ["pi", "dsh", "kimi", "grok"] {
        let w = World::new();
        let repo = w.repo("web");
        w.commit(&repo, "CLAUDE.md", "Always run the linter.\n");
        w.git(&repo, &["push", "--quiet", "origin", "main"]).unwrap();
        w.app.checkout_add("web", &repo, "main", "proving").await.unwrap();
        w.app.kit_set(&[format!("{key}={key}")]).await.unwrap();
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let wk = worker(&w.app).await;
        assert_eq!(wk["harness"], key);
        let home = w.dir.join("data").join("kept").join(key);
        for (_, file, says) in KEPT.iter().filter(|(k, ..)| *k == key) {
            let text = std::fs::read_to_string(home.join(file)).unwrap_or_else(|e| panic!("{key}: {file}: {e}"));
            assert!(text.contains(says), "{key}: {file} says {says:?}:\n{text}");
        }
        // A home is shared by every session of its harness on its account:
        // what it holds names where this session's rules are, and holds none
        // of them.
        let worktree = wk["worktree"].as_str().unwrap();
        let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
        let role = w.dir.join("data").join("role").join("worker");
        // DSH's line is quoted once more, as one word of its start script's.
        let set = |name: &str, value: &str| {
            line.contains(&format!("{name}='{value}'")) || line.contains(&format!("{name}='\\''{value}'\\''"))
        };
        assert!(set("FACTORIUM_WORKTREE", worktree), "{key}: {line}");
        assert!(set("FACTORIUM_ADDED_DIR", &role.to_string_lossy()), "{key}: {line}");
        assert!(std::fs::read_to_string(role.join("CLAUDE.md")).unwrap().contains("You are a factorium worker"));
        let told = match key {
            "kimi" => Some("SYSTEM.md"),
            "grok" => Some("rules/told.md"),
            _ => None,
        };
        if let Some(told) = told {
            let text = std::fs::read_to_string(home.join(told)).unwrap();
            assert!(text.contains("FACTORIUM_WORKTREE") && text.contains("FACTORIUM_ADDED_DIR"), "{key}: {text}");
        }
        for file in ["config.toml", "SYSTEM.md", "rules/told.md"] {
            let text = std::fs::read_to_string(home.join(file)).unwrap_or_default();
            // Neither the worker's nor the watch's, which started after it.
            let sessions = [worktree.to_string(), w.dir.join("data").join("watch").to_string_lossy().into_owned()];
            let mine = sessions.iter().any(|dir| text.contains(dir.as_str())) || text.contains("linter");
            assert!(!mine, "{key}: {file} holds one session's:\n{text}");
        }
    }
}

/// The guard commands a kept config runs before a tool call, by harness.
fn guard_hooks(key: &str, config: &toml::Value) -> Vec<String> {
    let commands: Vec<&toml::Value> = match key {
        "kimi" => config["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|hook| hook["event"].as_str() == Some("PreToolUse"))
            .collect(),
        _ => config["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|group| group["hooks"].as_array().unwrap())
            .collect(),
    };
    let commands = commands.into_iter().map(|hook| hook["command"].as_str().unwrap().to_string());
    commands.filter(|command| command.contains(&format!("guard {key}"))).collect()
}

#[tokio::test]
async fn a_grok_and_a_kimi_worker_start_guarded_and_kimi_with_only_the_host_s_model_routing() {
    for key in ["grok", "kimi"] {
        let w = World::new();
        w.checkout("web").await;
        // The host user's own Kimi config, as its login writes it, and more.
        let user = w.host.home().join(".kimi-code");
        std::fs::create_dir_all(&user).unwrap();
        let routing = "default_model = \"kimi-code/k3\"\n\
             [providers.\"managed:kimi-code\"]\ntype = \"kimi\"\n\
             [models.\"kimi-code/k3\"]\nprovider = \"managed:kimi-code\"\nmodel = \"k3\"\n";
        let (top, rest) = (
            "secondary_model = \"kimi-code/k3\"\nmerge_all_available_skills = true\n",
            "[thinking]\nenabled = true\n[[hooks]]\nevent = \"PreToolUse\"\ncommand = \"true\"\n",
        );
        std::fs::write(user.join("config.toml"), format!("{top}{routing}{rest}")).unwrap();
        w.app.kit_set(&[format!("{key}={key}")]).await.unwrap();
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        assert_eq!(worker(&w.app).await["harness"], key);
        let kept = w.dir.join("data").join("kept").join(key).join("config.toml");
        let config: toml::Value = std::fs::read_to_string(&kept).unwrap().parse().unwrap();
        assert_eq!(guard_hooks(key, &config).len(), 1, "{key}: one guard before each tool call:\n{config}");
        if key == "kimi" {
            let routing: toml::Value = routing.parse().unwrap();
            for cell in ["default_model", "providers", "models"] {
                assert_eq!(config.get(cell), routing.get(cell), "kimi keeps the host's {cell}");
            }
            for cell in ["secondary_model", "merge_all_available_skills", "thinking"] {
                assert!(config.get(cell).is_none(), "kimi keeps no {cell} of the host's:\n{config}");
            }
            let commands = config["hooks"].as_array().unwrap().iter().map(|hook| hook["command"].as_str());
            assert!(!commands.into_iter().any(|c| c == Some("true")), "nor its hooks:\n{config}");
        }
    }
}

#[tokio::test]
async fn a_dsh_worker_starts_on_its_kept_files_with_its_brief_first_and_its_program_s_directory_on_path() {
    let w = World::new();
    w.checkout("web").await;
    // The kit names DSH's TUI by path; its launcher runs `dsh` by name.
    let tools = w.dir.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let ran = w.dir.join("ran");
    let tui = format!("#!/bin/sh\n{{ printf '%s\\n' \"$@\"; command -v dsh; }} > '{}'\n", ran.display());
    for (name, text) in [("dsh-tui", tui.as_str()), ("dsh", "#!/bin/sh\n")] {
        std::fs::write(tools.join(name), text).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tools.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    w.app.kit_set(&[format!("dsh={}", tools.join("dsh-tui").display())]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let term = w.terminal(wk["terminal"].as_str().unwrap());
    assert!(term.sent.is_empty(), "nothing is typed at it: {:?}", term.sent);

    // Its kept files are written, every placeholder filled with its kept file.
    let home = w.dir.join("data").join("kept").join("dsh");
    for file in
        ["start.sh", "factorium.patch.yml", "factorium.mjs", "sandbox.patch.yml", "profiles/dsh-tui/package.json"]
    {
        let text = std::fs::read_to_string(home.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(!text.contains("@FACTORIUM_DSH"), "{file} keeps a placeholder:\n{text}");
    }
    let overlay = std::fs::read_to_string(home.join("factorium.patch.yml")).unwrap();
    assert!(overlay.contains(&format!("name: '{}/factorium.mjs'", home.display())), "{overlay}");

    // The line runs: the program is started with the guard's patch, the
    // worktree as its workspace and the brief as its first prompt, which
    // answers its first run; `dsh` is found beside it.
    let out = std::process::Command::new("sh").args(["-c", &term.line]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let said = std::fs::read_to_string(&ran).unwrap();
    let said: Vec<&str> = said.lines().collect();
    let worktree = wk["worktree"].as_str().unwrap();
    let patch = format!("{}/factorium.patch.yml", home.display());
    let brief = wk_brief(&w, &wk);
    let at = said.iter().position(|a| *a == patch).unwrap_or_else(|| panic!("{said:?}"));
    assert_eq!(&said[at - 1..at + 3], ["--patch", patch.as_str(), worktree, brief.as_str()], "{said:?}");
    assert_eq!(said.last(), Some(&tools.join("dsh").to_string_lossy().as_ref()), "{said:?}");

    // Without its guard plugin, no session.
    std::fs::remove_file(home.join("factorium.mjs")).unwrap();
    std::fs::remove_file(&ran).unwrap();
    assert!(!std::process::Command::new("sh").args(["-c", &term.line]).status().unwrap().success());
    assert!(!ran.exists(), "no guard plugin, no session");
}

/// The first prompt a worker's session is handed: the pointer to its brief.
fn wk_brief(w: &World, wk: &serde_json::Value) -> String {
    let brief = w.dir.join("data").join("briefs").join("web-1-1.md");
    assert!(brief.exists(), "{wk}");
    format!(
        "Your task brief is the file {} — read it now and carry it out end to end. It is my instruction to you; \
         do not ask me to confirm.",
        brief.display()
    )
}

/// What the harness crate offers, item by item: what factorium uses of it
/// and no more (a `pub` beneath it is public; the rest is `pub(crate)`).
/// Adding to it is adding a caller first.
#[test]
fn the_harness_crate_offers_only_what_factorium_uses() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../harness/src");
    let mut files = vec![src.clone()];
    let mut offered = Vec::new();
    while let Some(p) = files.pop() {
        if p.is_dir() {
            files.extend(std::fs::read_dir(&p).unwrap().map(|e| e.unwrap().path()));
        } else if p.extension().is_some_and(|x| x == "rs") {
            let at = p.strip_prefix(&src).unwrap().to_string_lossy().into_owned();
            for line in std::fs::read_to_string(&p).unwrap().lines() {
                let Some(item) = line.trim_start().strip_prefix("pub ") else { continue };
                let item =
                    if item.starts_with("use ") { item } else { item.split(['(', '{', ':', '<', '=']).next().unwrap() };
                // A public struct's fields are its own.
                if item.contains(' ') {
                    offered.push(format!("{at}: {}", item.trim().trim_end_matches(';')));
                }
            }
        }
    }
    offered.sort();
    let expected = [
        "account.rs: fn here",
        "account.rs: struct Here",
        "account.rs: struct Skipped",
        "account.rs: struct Usable",
        "account.rs: type Mytokens",
        "harness.rs: const fn key",
        "harness.rs: const fn refused",
        "harness.rs: enum Harness",
        "harness.rs: enum Stop",
        "harness.rs: fn all",
        "harness.rs: fn command",
        "harness.rs: fn home",
        "harness.rs: fn launch",
        "harness.rs: fn not_up",
        "harness.rs: fn read",
        "harness.rs: fn shell_call",
        "harness.rs: fn stopped",
        "harness/store.rs: fn turn",
        "harness/store.rs: fn write_session",
        "lib.rs: mod account",
        "lib.rs: use harness::{Harness, Stop}",
        "lib.rs: use start::{Home, Kept, Launch, RULES, SKILLS, Start}",
        "lib.rs: use turn::Turn",
        "start.rs: const RULES",
        "start.rs: const SKILLS",
        "start.rs: enum Kept",
        "start.rs: struct Home",
        "start.rs: struct Launch",
        "start.rs: struct Start",
        "turn.rs: enum Turn",
    ];
    assert_eq!(offered, expected);
}

#[tokio::test]
async fn a_kit_command_that_names_only_a_program_keeps_its_harness_stock_arguments() {
    for (key, program, stock) in
        [("grok", "/opt/grok/bin/grok", "--always-approve"), ("kimi", "/opt/kimi/kimi", "--auto")]
    {
        let w = World::new();
        w.checkout("web").await;
        let kit = w.app.kit_set(&[format!("{key}={program}")]).await.unwrap();
        let runs = format!("{program} {stock}");
        assert_eq!(kit["entries"][0]["command"], runs.as_str(), "{key}: kit set answers what will run");
        assert_eq!(w.app.kit().await.unwrap()["entries"][0]["command"], runs.as_str(), "{key}: so does kit show");
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let term = w.terminal(worker(&w.app).await["terminal"].as_str().unwrap());
        assert!(term.line.contains(&format!("&& {runs} ")), "{key}: {}", term.line);

        // A command with arguments of its own is taken as given.
        let given = format!("{program} --model-picker");
        let kit = w.app.kit_set(&[format!("{key}={given}")]).await.unwrap();
        assert_eq!(kit["entries"][0]["command"], given.as_str(), "{key}");
    }
}

/// A kimi worker whose terminal opens at its shell's prompt.
async fn at_shell(w: &World) -> serde_json::Value {
    w.checkout("web").await;
    w.app.kit_set(&["kimi=kimi".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    *w.host.orca.opens_at_shell.lock().unwrap() = true;
    w.tick().await;
    worker(&w.app).await
}

#[tokio::test]
async fn a_brief_is_typed_once_the_harness_is_up_never_at_its_shell() {
    let w = World::new();
    let wk = at_shell(&w).await;
    let handle = wk["terminal"].as_str().unwrap();
    assert!(w.terminal(handle).sent.is_empty(), "the shell's prompt is not typed at");
    w.advance(1);
    w.tick().await;
    assert!(w.terminal(handle).sent.is_empty(), "still the shell's");
    let told = w.app.watch_tell(id(&wk), "hello").await;
    refused(told, "not_up");
    assert!(w.terminal(handle).sent.is_empty(), "nor is the watch's word");

    w.set_screen(handle, &[SHELL, "Welcome to Kimi Code!", UP]);
    w.advance(1);
    w.tick().await;
    let sent = w.terminal(handle).sent;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].contains("web-1-1.md"), "{sent:?}");
    w.advance(1);
    w.tick().await;
    assert_eq!(w.terminal(handle).sent.len(), 1, "handed once");
    assert_eq!(worker(&w.app).await["status"], "working");
}

#[tokio::test]
async fn a_harness_that_never_comes_up_or_exits_sends_its_worker_to_the_watch() {
    let w = World::new();
    let wk = at_shell(&w).await;
    let handle = wk["terminal"].as_str().unwrap();
    w.advance(5);
    w.tick().await;
    assert_eq!(worker(&w.app).await["status"], "working", "it may yet come up: 5 min is the bound");
    w.advance(1);
    w.tick().await;
    let waiting = worker(&w.app).await;
    assert_eq!(waiting["status"], "waiting");
    let why = waiting["why"].as_str().unwrap();
    assert!(why.contains("not up 6 min after it started") && why.contains("shell's"), "{why}");
    assert!(w.terminal(handle).sent.is_empty());

    // Up, handed its brief, then its program exits back to the shell.
    let w = World::new();
    let wk = at_shell(&w).await;
    let handle = wk["terminal"].as_str().unwrap();
    w.set_screen(handle, &[UP]);
    w.tick().await;
    assert_eq!(w.terminal(handle).sent.len(), 1);
    w.set_screen(handle, &[UP, "zsh: parse error near `do'", SHELL]);
    w.advance(6);
    w.tick().await;
    assert!(worker(&w.app).await["why"].as_str().unwrap().contains("not up"));
    refused(w.app.watch_restart(id(&wk), "go on").await, "not_up");
    assert_eq!(w.terminal(handle).sent.len(), 1, "nothing more is typed at its shell");
    assert_eq!(worker(&w.app).await["status"], "waiting");
}

/// DSH's question before its screen is up (seen 2026-11): typed text would answer it.
#[tokio::test]
async fn a_harness_s_question_before_its_screen_is_up_is_not_typed_at() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&["dsh=dsh".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let handle = wk["terminal"].as_str().unwrap();
    w.set_screen(handle, &["是否进入安全模式？(y/n)"]);
    refused(w.app.watch_tell(id(&wk), "hello").await, "not_up");
    assert!(w.terminal(handle).sent.is_empty(), "the question is not answered");
    w.set_screen(handle, &[UP]);
    w.app.watch_tell(id(&wk), "hello").await.unwrap();
    assert_eq!(w.terminal(handle).sent.len(), 1);
}

#[tokio::test]
async fn a_kit_entry_names_a_model_after_its_last_at_and_only_a_harness_factorium_knows() {
    let w = World::new();
    let kit = w.app.kit_set(&["claude-code=claude@opus".into(), "codex=codex --x a@b c".into()]).await.unwrap();
    let e = &kit["entries"];
    assert_eq!(
        (&e[0]["command"], &e[0]["model"]),
        (&serde_json::json!("claude --dangerously-skip-permissions"), &serde_json::json!("opus"))
    );
    assert_eq!(e[1]["command"], "codex --x a@b c", "an @ inside an argument is no model");
    assert_eq!(e[1]["model"], serde_json::Value::Null);
    let no = w.app.kit_set(&["nope=nope".into()]).await.unwrap_err();
    assert_eq!(no.code, "no_harness");
}

#[tokio::test]
async fn only_a_well_formed_account_secret_is_an_account() {
    let w = World::new();
    let token = |t: &str| format!(r#"{{"fields":{{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"{t}"}}}}"#);
    let secrets = [
        ("agent-account-one", token("a")),
        ("agent-account-two", token("b")),
        // Not an account's secret, though it holds one's fields.
        ("deploy-key", token("c")),
        // No plain directory name.
        ("agent-account-.hidden", token("d")),
        ("agent-account-x:y", token("e")),
        // An empty token is none.
        ("agent-account-empty", token("")),
        // pi takes no account, wherever its login is.
        ("agent-account-pi", format!(r#"{{"fields":{{"HARNESS":"pi","MACHINE":"{}"}}}}"#, this_machine())),
        // A login on another machine, though logged in under this home.
        ("agent-account-away", r#"{"fields":{"HARNESS":"codex","MACHINE":"elsewhere-host"}}"#.into()),
    ];
    let away = w.host.home().join(".agent-accounts").join("away");
    std::fs::create_dir_all(&away).unwrap();
    std::fs::write(away.join("auth.json"), "{}").unwrap();
    w.host.tokens.lock().unwrap().extend(secrets.iter().map(|(n, j)| (n.to_string(), j.clone())));
    let names: Vec<_> = w.app.accounts().await.unwrap().as_array().unwrap().iter().map(|a| a["name"].clone()).collect();
    assert_eq!(names, [serde_json::json!("one"), serde_json::json!("two")]);
}

#[tokio::test]
async fn a_used_up_account_rests_an_hour_and_no_worker_starts_on_it_meanwhile() {
    let w = World::new();
    let account = w.host.home().join(".agent-accounts").join("night");
    std::fs::create_dir_all(&account).unwrap();
    std::fs::write(account.join("auth.json"), "{}").unwrap();
    let fields = format!(r#"{{"fields":{{"HARNESS":"codex","MACHINE":"{}"}}}}"#, this_machine());
    w.host.tokens.lock().unwrap().insert("agent-account-night".into(), fields);
    w.checkout("web").await;
    w.app.kit_set(&["codex=codex".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!(wk["account"], "night");
    w.set_screen(
        wk["terminal"].as_str().unwrap(),
        &["■ You've hit your usage limit reached.", "Increase your limits to continue using Codex."],
    );
    let at = w.app_now();
    let t = w.tick().await;
    assert_eq!(t.seen[0]["ended"], "quota");
    assert!(t.started.is_none() && t.held.as_deref().unwrap().contains("every account is used up"), "{t:?}");
    let night = w.app.accounts().await.unwrap()[0].clone();
    assert_eq!((night["left"].as_f64(), night["limited_until"].as_i64()), (Some(0.0), Some(at + 3_600_000)));
    w.advance(59);
    assert!(w.tick().await.started.is_none(), "still resting");
    w.advance(1);
    assert_eq!(w.app.accounts().await.unwrap()[0]["left"], 100.0);
    assert!(w.tick().await.started.is_some(), "rested");
}

#[tokio::test]
async fn a_pinned_checkout_starts_only_on_its_harness() {
    let w = World::new();
    w.checkout("web").await;
    w.app.kit_set(&["claude-code=claude".into(), "codex=codex".into()]).await.unwrap();
    w.app.kit_pin("web", Some("codex")).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let line = w.terminal(worker(&w.app).await["terminal"].as_str().unwrap()).line;
    assert!(line.contains("codex") && !line.contains("&& claude"), "{line}");
}

#[tokio::test]
async fn the_watch_starts_on_its_account_s_secret() {
    let w = World::new();
    w.host.tokens.lock().unwrap().insert(
        "agent-account-team".into(),
        r#"{"fields":{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"sk-ant-very-secret"}}"#.into(),
    );
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    w.app.worker_escalate(crate::id(&worker(&w.app).await), "which API version?").await.unwrap();
    w.advance(5);
    w.tick().await;
    let t = w.host.orca.terminals.lock().unwrap();
    let watch = t.values().find(|t| t.title == "factorium watch").expect("the watch's terminal");
    let reads = watch.line.starts_with(". ") && watch.line.contains("/data/secrets/");
    assert!(reads && !watch.line.contains("sk-ant"), "it reads its secret from a file first");
}

#[tokio::test]
async fn a_shell_s_prompt_is_told_from_a_harness_s_screen() {
    let w = World::new();
    let wk = at_shell(&w).await;
    let handle = wk["terminal"].as_str().unwrap();
    // As zsh and bash draw their prompts by default, with or without a line
    // typed after them.
    let shells = [
        "felix@macnoMac-Studio control %",
        "felix@macnoMac-Studio web-1-1 % \u{1b}[200~Your task brief is the file /b.md\u{1b}[201~",
        "felix@macnoMac-Studio ~ % cd /w && export A=1 && kimi",
        "root@box ~ #",
        "me@host:~/web$ ls",
        "Mac:web me$",
        "bash-3.2$ ",
    ];
    let harness_rows = [
        "? for shortcuts",
        "Context left until auto-compact: 12%",
        "› Write tests for @filename",
        "  100% context left · ? for shortcuts",
        "│ >                                           │",
        "felix@host wrote: 50 % done",
        "@types node % x",
        "me@ dir % x",
        "progress: 50 % done",
        "cost$ 5",
        "note: hello",
        "ratio: 1:2$",
        "bash-completion loaded",
    ];
    for row in shells {
        w.set_screen(handle, &[row]);
        refused(w.app.watch_tell(id(&wk), "hello").await, "not_up");
    }
    assert!(w.terminal(handle).sent.is_empty());
    for (n, row) in harness_rows.iter().enumerate() {
        w.set_screen(handle, &[row]);
        w.app.watch_tell(id(&wk), "hello").await.unwrap_or_else(|r| panic!("{row}: {r:?}"));
        assert_eq!(w.terminal(handle).sent.len(), n + 1, "{row}");
    }
}

/// A Claude Code worker 60 min into its work whose worktree has the session
/// files `sessions` (name, and its notes as (start?, minutes after the
/// worker started)): whether it is silent, its turn ended past the silent
/// bound of 30 min.
async fn silent_with(sessions: &[(&str, &[(bool, u64)])]) -> bool {
    let w = World::new();
    w.checkout("web").await;
    w.kit().await;
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let cwd = wk["worktree"].as_str().unwrap().to_string();
    let t0 = (w.app_now() / 1000) as u64;
    // Each note as the stand-in session writes it: its first line a start,
    // its second an end.
    // Claude Code keeps them under the host user's own home.
    let projects = w.host.home().join(".claude").join("projects");
    let stand_in = || {
        let dir = std::fs::read_dir(&projects).unwrap().flatten().next().unwrap().path();
        std::fs::read_dir(dir).unwrap().flatten().next().unwrap().path()
    };
    let note = |start: bool, min: u64| {
        write_session(&w, Harness::ClaudeCode, None, &cwd, start, t0 + min * 60);
        let text = std::fs::read_to_string(stand_in()).unwrap();
        text.lines().nth(usize::from(!start)).unwrap().to_string()
    };
    let written: Vec<(&str, Vec<String>)> =
        sessions.iter().map(|(name, ns)| (*name, ns.iter().map(|(s, m)| note(*s, *m)).collect())).collect();
    let file = stand_in();
    let dir = file.parent().unwrap().to_path_buf();
    std::fs::remove_file(file).unwrap();
    for (name, lines) in written {
        std::fs::write(dir.join(format!("{name}.jsonl")), lines.join("\n") + "\n").unwrap();
    }
    w.advance(60);
    w.tick().await;
    worker(&w.app).await["status"] == "waiting"
}

#[tokio::test]
async fn of_several_sessions_in_a_worktree_the_turn_that_started_last_speaks() {
    // Each case both ways round, whichever order the files are read in.
    let both = async |a: &[(bool, u64)], b: &[(bool, u64)]| {
        let one = silent_with(&[("a", a), ("b", b)]).await;
        assert_eq!(one, silent_with(&[("a", b), ("b", a)]).await, "the order the files are read in");
        one
    };
    // An older session left open does not keep the one that started last busy.
    assert!(both(&[(true, 1)], &[(true, 2), (false, 5)]).await, "the last start ended 55 min ago");
    // Nor does an older one that ended make an open one idle.
    assert!(!both(&[(true, 1), (false, 5)], &[(true, 2)]).await, "the last start is open");
    // Of sessions on the same start, the end written last.
    assert!(!both(&[(true, 2), (false, 5)], &[(true, 2), (false, 59)]).await, "it ended a minute ago");
    // A session that names no start does not outrank one that does.
    assert!(both(&[(false, 59)], &[(true, 2), (false, 5)]).await, "the one that names its start ended long ago");
    // With no start named, the end written last.
    assert!(!both(&[(false, 5)], &[(false, 59)]).await, "it ended a minute ago");
    assert!(both(&[(false, 5)], &[(false, 6)]).await, "it ended 54 min ago");
}

#[tokio::test]
async fn a_turn_s_end_is_read_to_the_second_on_any_day() {
    // Year's end at 23:59, a leap day, the day after: each end at :47.
    for end in [1_830_297_587_u64, 1_835_440_487, 1_835_481_647] {
        let w = World::new();
        w.checkout("web").await;
        w.kit().await;
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        let minute = end - 47;
        w.advance(((minute - 600 - (w.app_now() / 1000) as u64) / 60) as i64);
        w.tick().await;
        let wk = worker(&w.app).await;
        write_session(&w, Harness::ClaudeCode, None, wk["worktree"].as_str().unwrap(), false, end);
        // The silent bound is 30 min: 47s short of it, then 13s past it.
        w.advance(40);
        w.tick().await;
        assert_eq!(worker(&w.app).await["status"], "working", "{end}: ended 29m13s ago");
        w.advance(1);
        w.tick().await;
        assert_eq!(worker(&w.app).await["status"], "waiting", "{end}: ended 30m13s ago");
    }
}
