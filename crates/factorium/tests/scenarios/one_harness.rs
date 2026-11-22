//! What holds of one harness's ways: each scenario names the harness it is
//! about. What holds of every harness is in `harnesses.rs`.

use factorium_harness::Harness;

use crate::fixture::{CLAUDE, SHELL, UP, World, id, quoted_word, refused, worker};
use crate::fixture::{write_session, write_session_in};
use factorium::host::Host;

#[tokio::test]
async fn a_codex_worker_is_told_the_repos_rules_and_runs_on_the_host_s_model_settings_only() {
    for (kit, model) in [("codex=codex", "model=\"gpt-host\""), ("codex=codex@gpt-kit", "--model 'gpt-kit'")] {
        let w = World::new();
        // Rules codex does not read from its start directory.
        let repo = w.repo("web");
        w.commit(&repo, "CLAUDE.md", "Always run the linter.\n");
        w.git(&repo, &["push", "--quiet", "origin", "main"]).unwrap();
        w.app.checkout_add("web", &repo, "main", "proving", None).await.unwrap();
        let user = w.host.home().join(".codex");
        std::fs::create_dir_all(&user).unwrap();
        let config = "model = \"gpt-host\"\nmodel_reasoning_effort = \"high\"\nmodel_provider = \"mine\"\n\
             [tui]\nscreen_reader_detection_done = true\n[projects.\"/elsewhere\"]\ntrust_level = \"trusted\"\n";
        std::fs::write(user.join("config.toml"), config).unwrap();
        w.app.kit_set(&[kit.into()]).await.unwrap();
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let wk = worker(&w.app).await;
        let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
        assert!(line.contains("Always run the linter."), "{kit}: told the repo's rules: {line}");
        let rules = format!("{}/CLAUDE.md", wk["worktree"].as_str().unwrap());
        assert!(line.contains(&rules), "{kit}: told where they are: {line}");
        assert!(line.contains(model) && line.contains("model_reasoning_effort=\"high\""), "{kit}: {line}");
        for other in ["gpt-host\" --model", "model_provider", "screen_reader", "/elsewhere"] {
            assert!(!line.contains(other), "{kit}: no {other} of the host's: {line}");
        }
    }
}

/// Runs worker `wk`'s `guard` command, by the built binary, as a hook runs
/// it: `/bin/sh -c`, the call `payload` on stdin; answers its exit code and
/// what it said.
fn run_guard(wk: &serde_json::Value, guard: &str, payload: serde_json::Value) -> (Option<i32>, String) {
    use std::io::Write;
    let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", &guard.replace(&exe, env!("CARGO_BIN_EXE_factorium"))])
        .env("FACTORIUM_WORKER", id(wk).to_string())
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// The guard as a daemon-started codex worker's line gives it, run as Codex
/// runs a hook, by the built binary, on a call as codex-cli 0.169.0 described
/// it on a real host (its code-mode `exec_command` is `Bash` to a hook).
#[tokio::test]
async fn a_codex_worker_s_line_has_the_real_guard_refuse_its_push_as_codex_describes_it() {
    let w = World::new();
    let wk = w.working("codex=codex", "t").await;
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    let config: toml::Table = quoted_word(&line, line.find("'hooks=").unwrap()).parse().unwrap();
    let before = config["hooks"]["PreToolUse"][0]["hooks"].as_array().unwrap();
    let guard = before.iter().map(|hook| hook["command"].as_str().unwrap()).find(|c| c.contains(" guard codex "));
    let guard = guard.expect("a guard before each call");
    let hook = |command: &str| {
        let payload = serde_json::json!({"session_id": "s", "turn_id": "t", "cwd": "/w",
            "hook_event_name": "PreToolUse", "model": "gpt-6.2-sol", "permission_mode": "bypassPermissions",
            "tool_name": "Bash", "tool_input": {"command": command}, "tool_use_id": "exec-1"});
        run_guard(&wk, guard, payload)
    };
    let (code, said) = hook("git push origin HEAD:refs/heads/probe-codex");
    assert_eq!(code, Some(2), "Codex blocks a call only on 2: {said}");
    assert!(said.contains("a worker does not push"), "{said}");
    assert_eq!(hook("git status --short").0, Some(0));
}

/// The guard as a daemon-started pi worker's line gives its extension, run as
/// the extension runs it (`/bin/sh -c`, the call on stdin), by the built
/// binary, on a call as pi 1.5.2 described it to the extension on a real host.
/// The extension is the kept one, named on the line: pi loads it from there,
/// not from the kept `extensions/`.
#[tokio::test]
async fn a_pi_worker_s_line_has_the_real_guard_refuse_its_push_as_pi_describes_it() {
    let w = World::new();
    let wk = w.working("pi=pi", "t").await;
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    let home = w.data().kept("pi");
    let extension = home.join("factorium.js").to_string_lossy().into_owned();
    assert!(line.contains(&format!("--extension '{extension}'")), "{line}");
    assert!(std::fs::read_to_string(&extension).unwrap().contains("FACTORIUM_GUARD"));
    let guard = quoted_word(&line, line.find("FACTORIUM_GUARD=").unwrap() + "FACTORIUM_GUARD=".len());
    assert!(guard.contains(" guard pi"), "{guard}");
    let hook =
        |command: &str| run_guard(&wk, &guard, serde_json::json!({"toolName": "bash", "input": {"command": command}}));
    let (code, said) = hook("git push origin HEAD:refs/heads/probe-pi");
    assert_eq!(code, Some(2), "the extension blocks on any but 0: {said}");
    assert!(said.contains("a worker does not push"), "{said}");
    assert_eq!(hook("git status --short").0, Some(0));
}

/// The guard a grok worker's kept config runs before a call, run as Grok runs
/// a hook, by the built binary, on a call as Grok describes it: the tool and
/// its input in camelCase and again in snake_case, or in only one of them.
#[tokio::test]
async fn a_grok_worker_s_kept_config_has_the_real_guard_refuse_its_push_as_grok_describes_it() {
    let w = World::new();
    let wk = w.working("grok=grok", "t").await;
    let config: toml::Table =
        std::fs::read_to_string(w.data().kept("grok").join("config.toml")).unwrap().parse().unwrap();
    let before = config["hooks"]["PreToolUse"][0]["hooks"].as_array().unwrap();
    let guard = before.iter().map(|hook| hook["command"].as_str().unwrap()).find(|c| c.contains(" guard grok"));
    let guard = guard.expect("a guard before each call");
    let both = |tool: &str, command: &str| {
        let input = serde_json::json!({"command": command});
        serde_json::json!({"hookEventName": "PreToolUse", "toolName": tool, "tool_name": tool,
            "toolInput": input, "tool_input": input})
    };
    let (code, said) = run_guard(&wk, guard, both("run_terminal_command", "git push origin HEAD"));
    assert_eq!(code, Some(2), "Grok blocks a call on 2: {said}");
    assert!(said.contains("a worker does not push"), "{said}");
    let snake = serde_json::json!({"tool_name": "run_terminal_command", "tool_input": {"command": "git push"}});
    assert_eq!(run_guard(&wk, guard, snake).0, Some(2), "one spelling alone is the call");
    assert_eq!(run_guard(&wk, guard, both("run_terminal_command", "git status --short")).0, Some(0));
    assert_eq!(run_guard(&wk, guard, both("edit_file", "git push")).0, Some(0), "only the shell tool runs a line");
}

/// The config a kimi worker's home keeps, the host user's own Kimi config
/// being `user`.
async fn kimi_kept(user: &str) -> String {
    let w = World::new();
    w.checkout("web").await;
    let dir = w.host.home().join(".kimi-code");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), user).unwrap();
    w.app.kit_set(&["kimi=kimi".into()]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    assert_eq!(worker(&w.app).await["harness"], "kimi");
    let kept = w.data().kept("kimi").join("config.toml");
    std::fs::read_to_string(kept).unwrap()
}

#[tokio::test]
async fn a_kimi_worker_starts_with_only_the_host_s_model_routing() {
    // The host user's own Kimi config, as its login writes it, and more.
    let routing = "default_model = \"kimi-code/k3\"\n\
         [providers.\"managed:kimi-code\"]\ntype = \"kimi\"\n\
         [models.\"kimi-code/k3\"]\nprovider = \"managed:kimi-code\"\nmodel = \"k3\"\n";
    let (top, rest) = (
        "secondary_model = \"kimi-code/k3\"\nmerge_all_available_skills = true\n",
        "[thinking]\nenabled = true\n[[hooks]]\nevent = \"PreToolUse\"\ncommand = \"true\"\n",
    );
    let kept = kimi_kept(&format!("{top}{routing}{rest}")).await;
    // Kept as if the host's held only its routing: none of the rest, nor its hooks.
    assert_eq!(kept, kimi_kept(routing).await);
    assert_ne!(kept, kimi_kept("").await, "the host's routing is kept");
}

#[tokio::test]
async fn a_dsh_worker_starts_on_its_kept_files_with_its_brief_first_and_its_program_s_directory_on_path() {
    let w = World::new();
    w.checkout("web").await;
    // The kit names DSH's TUI by path; its launcher runs `dsh` by name.
    let tools = w.dir.join("tools");
    let ran = w.dir.join("ran");
    let tui = format!("#!/bin/sh\n{{ printf '%s\\n' \"$@\"; command -v dsh; }} > '{}'\n", ran.display());
    for (name, text) in [("dsh-tui", tui.as_str()), ("dsh", "#!/bin/sh\n")] {
        crate::fixture::program(&tools.join(name), text);
    }
    w.app.kit_set(&[format!("dsh={}", tools.join("dsh-tui").display())]).await.unwrap();
    w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
    w.tick().await;
    let wk = worker(&w.app).await;
    let term = w.terminal(wk["terminal"].as_str().unwrap());
    assert!(term.sent.is_empty(), "nothing is typed at it: {:?}", term.sent);

    // Its kept files are written, every placeholder filled with its kept file.
    let home = w.data().kept("dsh");
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
    let brief = w.data().brief("web-1-1");
    assert!(brief.exists(), "{wk}");
    format!(
        "Your task brief is the file {} — read it now and carry it out end to end. It is my instruction to you; \
         do not ask me to confirm.",
        brief.display()
    )
}

#[tokio::test]
async fn a_kit_command_that_names_only_a_program_keeps_its_harness_stock_arguments() {
    for (key, program, stock) in [
        ("grok", "/opt/grok/bin/grok", "--always-approve"),
        ("kimi", "/opt/kimi/kimi", "--auto"),
        ("agy", "/opt/agy/agy", "--dangerously-skip-permissions"),
    ] {
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

/// footer shows.
#[tokio::test]
async fn a_kimi_brief_waits_for_kimi_s_own_screen_not_its_launch_line() {
    let w = World::new();
    let wk = w.at_shell().await;
    let handle = wk["terminal"].as_str().unwrap();
    let launching = [&format!("{SHELL}. '/var/folders/T/orca-launch-3a623b76a5c8c688.sh"), "'"];
    w.set_screen(handle, &launching);
    w.advance(1);
    w.tick().await;
    assert!(w.terminal(handle).sent.is_empty(), "not typed while it starts");
    refused(w.app.watch_tell(id(&wk), "hello", false).await, "not_up");
    let welcome =
        ["'", " │  ▐█▛█▛█▌  Welcome to Kimi Code!", "   No session yet — one will be created on your first message."];
    w.set_screen(handle, &welcome);
    w.advance(1);
    w.tick().await;
    assert!(w.terminal(handle).sent.is_empty(), "its banner is not its prompt");

    let up = [
        "   No session yet — one will be created on your first message.",
        " │ >                                                  │",
        " Never Ask  K3.1 Pro thinking: high  …/factorium-39/role/worker",
        "                                           context: 0% (0/1M)",
    ];
    w.set_screen(handle, &up);
    w.advance(1);
    w.tick().await;
    let sent = w.terminal(handle).sent;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].contains("web-1-1.md"), "{sent:?}");
}

#[tokio::test]
async fn a_harness_that_never_comes_up_or_exits_sends_its_worker_to_the_watch() {
    let w = World::new();
    let wk = w.at_shell().await;
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
    let wk = w.at_shell().await;
    let handle = wk["terminal"].as_str().unwrap();
    w.set_screen(handle, &[UP]);
    w.tick().await;
    assert_eq!(w.terminal(handle).sent.len(), 1);
    w.set_screen(handle, &[UP, "zsh: parse error near `do'", SHELL]);
    w.advance(6);
    w.tick().await;
    assert!(worker(&w.app).await["why"].as_str().unwrap().contains("not up"));
    refused(w.app.watch_restart(id(&wk), "go on", false).await, "not_up");
    assert_eq!(w.terminal(handle).sent.len(), 1, "nothing more is typed at its shell");
    assert_eq!(worker(&w.app).await["status"], "waiting");
}

/// DSH's question before its screen is up (seen 2026-11): typed text would answer it.
#[tokio::test]
async fn a_harness_s_question_before_its_screen_is_up_is_not_typed_at() {
    let w = World::new();
    let wk = w.working("dsh=dsh", "t").await;
    let handle = wk["terminal"].as_str().unwrap();
    w.set_screen(handle, &["是否进入安全模式？(y/n)"]);
    refused(w.app.watch_tell(id(&wk), "hello", false).await, "not_up");
    assert!(w.terminal(handle).sent.is_empty(), "the question is not answered");
    w.set_screen(handle, &[UP]);
    w.app.watch_tell(id(&wk), "hello", false).await.unwrap();
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
async fn a_shell_s_prompt_is_told_from_a_harness_s_screen() {
    let w = World::new();
    let wk = w.at_shell().await;
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
        refused(w.app.watch_tell(id(&wk), "hello", false).await, "not_up");
    }
    assert!(w.terminal(handle).sent.is_empty());
    // A kimi worker: its footer is up above the row told.
    for (n, row) in harness_rows.iter().enumerate() {
        w.set_screen(handle, &["context: 0% (0/1M)", row]);
        w.app.watch_tell(id(&wk), "hello", false).await.unwrap_or_else(|r| panic!("{row}: {r:?}"));
        assert_eq!(w.terminal(handle).sent.len(), n + 2, "{row}");
    }
    assert!(w.terminal(handle).sent[0].contains("web-1-1.md"), "the brief first");
}

/// A Claude Code worker 60 min into its work whose worktree has the session
/// files `sessions` (name, and its notes as (start?, minutes after the
/// worker started)): whether it is silent, its turn ended past the silent
/// bound of 30 min.
async fn silent_with(sessions: &[(&str, &[(bool, u64)])]) -> bool {
    let w = World::new();
    let wk = w.working(CLAUDE, "t").await;
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
    at_once!(
        // An older session left open does not keep the one that started last busy.
        async { assert!(both(&[(true, 1)], &[(true, 2), (false, 5)]).await, "the last start ended 55 min ago") },
        // Nor does an older one that ended make an open one idle.
        async { assert!(!both(&[(true, 1), (false, 5)], &[(true, 2)]).await, "the last start is open") },
        // Of sessions on the same start, the end written last.
        async { assert!(!both(&[(true, 2), (false, 5)], &[(true, 2), (false, 59)]).await, "it ended a minute ago") },
        // A session that names no start does not outrank one that does.
        async {
            let why = "the one that names its start ended long ago";
            assert!(both(&[(false, 59)], &[(true, 2), (false, 5)]).await, "{why}")
        },
        // With no start named, the end written last.
        async { assert!(!both(&[(false, 5)], &[(false, 59)]).await, "it ended a minute ago") },
        async { assert!(both(&[(false, 5)], &[(false, 6)]).await, "it ended 54 min ago") },
    );
}

#[tokio::test]
async fn a_turn_s_end_is_read_to_the_second_on_any_day() {
    // Year's end at 23:59, a leap day, the day after, a month's first: each
    // end at :47.
    crate::fixture::each([1_830_297_587_u64, 1_835_440_487, 1_835_481_647, 1_840_789_247], |end| async move {
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
    });
}

#[tokio::test]
async fn a_session_started_in_the_role_s_directory_is_known_by_its_brief_pointer() {
    crate::fixture::each([Harness::ClaudeCode, Harness::Codex], |h| async move {
        // Open in the role's directory 45 min, handed `brief`: whether it
        // keeps the worker from silence.
        let working = async |brief: &str| {
            let w = World::new();
            w.checkout("web").await;
            w.app.kit_set(&[format!("{}=run-{}", h.key(), h.key())]).await.unwrap();
            w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
            w.tick().await;
            let role = w.data().worker_role();
            let brief = w.data().brief(brief);
            let now = (w.app_now() / 1000) as u64;
            write_session_in(&w, h, None, &role.to_string_lossy(), &brief.to_string_lossy(), true, now);
            w.advance(45);
            w.tick().await;
            worker(&w.app).await["status"] == "working"
        };
        at_once!(async { assert!(working("web-1-1").await, "{}: its own brief", h.key()) }, async {
            assert!(!working("web-2-1").await, "{}: another worker's", h.key())
        },);
    });
}

/// Claude Code's question quoted at its prompt, and Claude Code's dialog at
/// a Codex worker's screen, are no dialog of theirs: what is typed goes in
/// with no key pressed first. Its own dialog at Claude Code's is dismissed
/// first (`path.rs`).
#[tokio::test]
async fn a_dialog_is_dismissed_only_where_its_own_harness_opened_it() {
    let quoted = ["⏺ The modal said: Teach auto mode about your environment?", "", "› ", "? for shortcuts"];
    let dialog = ["Teach auto mode about your environment?", "› Continue", "Enter to continue · Esc to cancel"];
    for (kit, screen) in [("claude-code=claude", &quoted[..]), ("codex=codex", &dialog[..])] {
        let w = World::new();
        w.checkout("web").await;
        w.app.kit_set(&[kit.into()]).await.unwrap();
        w.app.issue_add("web", "t", "", false, &[]).await.unwrap();
        w.tick().await;
        let wk = worker(&w.app).await;
        let handle = wk["terminal"].as_str().unwrap();
        w.set_screen(handle, screen);
        w.app.watch_tell(id(&wk), "hello", false).await.unwrap();
        let sent = w.terminal(handle).sent;
        assert!(!sent.contains(&crate::fixture::ESCAPE.to_string()), "{kit}: {sent:?}");
        assert!(sent.last().unwrap().contains("hello"), "{kit}: {sent:?}");
    }
}

/// Z.ai's usage limit on a pi worker's screen ends it used up however the
/// terminal wraps it: at a word's end, or within a word ("quo" / "ta",
/// "usage li" / "mit"), with pi's footer below it, or on one line. A screen
/// of no quota does not, nor the same body on another harness's screen.
#[tokio::test]
async fn a_pi_worker_s_z_ai_usage_limit_is_seen_wrapped_at_a_word_s_end_or_within_one() {
    let line = r#"Error: 429 {"code":"1308","message":"Usage limit reached for 5 hour quota. Resets at 21:00"}"#;
    let wrapped = |at: &str, before: bool| {
        let cut = line.find(at).unwrap() + if before { 0 } else { at.len() };
        vec![line[..cut].to_string(), line[cut..].to_string(), "~/work (main)  0.0%/200k".to_string()]
    };
    let ended_on = |kit: &'static str, screen: Vec<String>| async move {
        let w = World::new();
        let wk = w.working(kit, "t").await;
        w.set_screen(wk["terminal"].as_str().unwrap(), &screen.iter().map(String::as_str).collect::<Vec<_>>());
        w.tick().await.seen.first().map(|seen| seen["ended"].clone())
    };
    let ended = |screen: Vec<String>| ended_on("pi=pi", screen);
    let quota = Some(serde_json::Value::from("quota"));
    let other = r#"Error: 429 {"code":"1302","message":"Rate limit reached for requests"}"#;
    at_once!(
        async { assert_eq!(ended(wrapped("limit", true)).await, quota, "wrapped at a word's end") },
        async { assert_eq!(ended(wrapped("Usage li", false)).await, quota, "wrapped within \"limit\"") },
        async { assert_eq!(ended(wrapped("quo", false)).await, quota, "wrapped within \"quota\"") },
        async { assert_eq!(ended(wrapped("13", false)).await, quota, "wrapped within the code") },
        async { assert_eq!(ended(vec![other[..30].into(), other[30..].into()]).await, None, "no quota") },
        async { assert_eq!(ended(vec![line.into()]).await, quota, "on one line") },
        async { assert_eq!(ended_on("kimi=kimi", vec![line.into()]).await, None, "not kimi's quota") },
    );
}

#[tokio::test]
async fn a_pi_session_s_home_links_the_host_user_s_pi_login_it_finds_and_no_other() {
    let w = World::new();
    let agent = w.host.home().join(".pi/agent");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::write(agent.join("auth.json"), "{}").unwrap();
    w.app.kit_set(&["pi=pi".into()]).await.unwrap();
    // The watch's home a repository already: its session opens with no git run.
    std::fs::create_dir_all(w.data().watch().join(".git")).unwrap();
    w.tick().await;
    let kept = w.data().kept("pi");
    assert_eq!(std::fs::read_link(kept.join("auth.json")).unwrap(), agent.join("auth.json"));
    assert!(kept.join("models.json").symlink_metadata().is_err(), "the host user has none to link");
}

/// Runs `command`, one of agy's hooks, as agy runs a hook (agy 1.5.14):
/// `sh -c`, with `HOME` the session's kept home and the variables `env`, the
/// call `payload` on stdin; answers its exit code and what it printed.
fn agy_hook(command: &str, home: &std::path::Path, env: &[(&str, String)], payload: &str) -> (Option<i32>, String) {
    use std::io::Write;
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", command])
        .env("HOME", home)
        .env_remove("ORCA_PANE_KEY")
        .env_remove("ORCA_AGENT_PANE")
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(payload.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The guard an agy worker's kept hooks run before a call, run as agy runs a
/// hook, by the built binary, on a call as agy describes it: a refusal is
/// agy's deny, printed, its reason whole; a call let by prints nothing; a
/// guard that cannot be run denies the call too. Each exits 0: agy reads
/// what is printed.
#[tokio::test]
async fn an_agy_worker_s_kept_hooks_have_the_real_guard_deny_its_push_as_agy_reads_a_denial() {
    let w = World::new();
    let wk = w.working("agy=agy", "t").await;
    let home = w.data().kept("agy");
    let hooks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".gemini/config/hooks.json")).unwrap()).unwrap();
    let before = hooks["factorium"]["PreToolUse"][0]["hooks"].as_array().unwrap();
    assert_eq!(hooks["factorium"]["PreToolUse"][0]["matcher"], "*", "every tool's call");
    let guard = before.iter().map(|hook| hook["command"].as_str().unwrap()).find(|c| c.contains(" guard agy"));
    let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    let guard = guard.expect("a guard before each call").replace(&exe, env!("CARGO_BIN_EXE_factorium"));
    let env = [("FACTORIUM_WORKER", id(&wk).to_string())];
    let call = |name: &str, line: &str| {
        serde_json::json!({"conversationId": "c", "hookEventName": "PreToolUse", "cwd": wk["worktree"],
            "toolCall": {"id": "call_1", "name": name, "args": {"CommandLine": line, "WaitMsBeforeAsync": 5000}}})
        .to_string()
    };
    let denied = |said: &str| -> String {
        let value: serde_json::Value = serde_json::from_str(said).unwrap_or_else(|e| panic!("{e}: {said}"));
        assert_eq!(value["decision"], "deny", "{said}");
        value["reason"].as_str().unwrap().to_string()
    };

    let (code, said) = agy_hook(&guard, &home, &env, &call("run_command", "git push origin HEAD"));
    assert_eq!(code, Some(0), "{said}");
    assert!(denied(&said).contains("a worker does not push"), "{said}");
    let (code, said) = agy_hook(&guard, &home, &env, &call("run_command", "git status --short"));
    assert_eq!((code, said.as_str()), (Some(0), ""), "let by: nothing printed");
    assert_eq!(agy_hook(&guard, &home, &env, &call("view_file", "git push")).1, "", "only the shell tool runs a line");
    let (_, said) = agy_hook(&guard, &home, &env, r#"{"toolCall":{"name":"run_command"}}"#);
    assert!(denied(&said).contains("cannot read this tool call"), "{said}");
    // A reason holding a quote, a backslash and a line break stays one JSON string.
    let (_, said) = agy_hook(
        &guard,
        &home,
        &env,
        "{\"toolCall\":{\"name\":\"run_command\",\"args\":{\"CommandLine\":\"git push \\\"a\\\\b\\\"\\ngit status\"}}}",
    );
    assert!(denied(&said).contains("a worker does not push"), "{said}");

    let gone = guard.replace(env!("CARGO_BIN_EXE_factorium"), "/no/such/factorium");
    let (code, said) = agy_hook(&gone, &home, &env, &call("run_command", "git status"));
    assert_eq!(code, Some(0), "{said}");
    assert!(!denied(&said).is_empty(), "a guard that cannot run denies: {said}");
}

/// Orca's report in an agy worker's kept hooks: Orca's own hook for agy, run
/// with the host user's home whatever the session's is, where Orca installed
/// it and started the session; else what agy reads from a quiet hook, and
/// nothing at all before a call, which agy would take for a denial. The
/// commands its tools run get the host user's home back from the kept
/// `.zshenv`.
#[tokio::test]
async fn an_agy_worker_s_kept_hooks_report_to_orca_with_the_host_user_s_home() {
    let w = World::new();
    let user = w.host.home();
    let orca = user.join(".orca/agent-hooks/antigravity-hook.sh");
    std::fs::create_dir_all(orca.parent().unwrap()).unwrap();
    crate::fixture::program(
        &orca,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s %s\\n' \"$ORCA_ANTIGRAVITY_EVENT\" \"$HOME\"\n",
    );
    w.working("agy=agy", "t").await;
    let home = w.data().kept("agy");
    let hooks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".gemini/config/hooks.json")).unwrap()).unwrap();
    let report = |event: &str| {
        let entry = &hooks["factorium"][event][0];
        let hooks = entry["hooks"].as_array().map_or(vec![entry.clone()], Clone::clone);
        let reports = hooks.iter().filter_map(|h| h["command"].as_str()).filter(|c| c.contains("antigravity-hook.sh"));
        reports.map(String::from).collect::<Vec<_>>()
    };
    let pane = [("ORCA_PANE_KEY", "p".to_string())];
    for (event, quiet) in
        [("PreInvocation", "{}\n"), ("Stop", "{\"decision\":\"\"}\n"), ("PreToolUse", ""), ("PostToolUse", "{}\n")]
    {
        let [command] = &report(event)[..] else { panic!("{event}: one report: {hooks}") };
        let said = agy_hook(command, &home, &pane, "{}");
        assert_eq!(said, (Some(0), format!("{event} {}\n", user.display())), "{event}");
        assert_eq!(agy_hook(command, &home, &[], "{}"), (Some(0), quiet.to_string()), "{event}: no Orca pane");
    }
    // The host user's own `.zshenv` puts Homebrew first, as on the real host
    // (factorium#201), where it found the installed factorium, not the session's.
    std::fs::write(user.join(".zshenv"), "export PATH=\"/brew/bin:$PATH\"\n").unwrap();
    let zshenv = std::fs::read_to_string(home.join(".zshenv")).unwrap();
    let shell = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("{zshenv}\nprintf '%s\\n%s' \"$HOME\" \"$PATH\"")])
        .env("HOME", &home)
        .env("PATH", "/session/bin:/usr/bin:/bin")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&shell.stdout);
    let (said_home, path) = said.split_once('\n').unwrap();
    assert_eq!(said_home, user.to_string_lossy());
    assert!(path.starts_with("/session/bin:") && path.contains("/brew/bin"), "the session's PATH first: {path}");
}

/// An agy worker's commands run in its worktree: agy runs every command in
/// a directory added with `--add-dir` (agy 1.5.14, factorium#201's real run),
/// so its line adds none; it trusts both, and reads its rules by their path.
#[tokio::test]
async fn an_agy_worker_s_line_adds_no_directory_so_its_commands_run_in_its_worktree() {
    let w = World::new();
    let wk = w.working("agy=agy", "t").await;
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    let worktree = wk["worktree"].as_str().unwrap();
    assert!(!line.contains("--add-dir"), "{line}");
    assert!(line.starts_with(&format!("cd '{worktree}' && ")), "{line}");
    assert!(line.contains("trustedWorkspaces") && line.contains(worktree), "{line}");
}
