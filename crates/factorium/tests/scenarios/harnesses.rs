//! Every harness: a worker of it starts with its own start line and brief,
//! and its session files — written by the harness's own stand-in — say
//! whether its turn is open.

use factorium_harness::Harness;

use crate::{World, worker};

/// Writes the session files a harness's stand-in describes for `cwd`.
pub fn write_session(w: &World, h: Harness, cwd: &str, open: bool, at: u64) {
    let root = match h {
        Harness::ClaudeCode => w.host.home.join(h.session_place("").under_home),
        _ => w.dir.join("data").join("kept").join(h.key()),
    };
    for f in h.session_files(cwd, open, at) {
        if let Some(name) = &f.replaces {
            let _ = std::fs::remove_file(root.join(name));
        }
        let path = root.join(&f.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if f.compressed {
            let packed =
                ruzstd::encoding::compress_to_vec(f.body.as_bytes(), ruzstd::encoding::CompressionLevel::Fastest);
            std::fs::write(&path, packed).unwrap();
        } else {
            std::fs::write(&path, &f.body).unwrap();
        }
    }
    if let Some(index) = h.session_index(cwd, "") {
        let place = h.session_place(cwd);
        if let factorium_harness::SessionLayout::Index { file } = place.layout {
            std::fs::write(root.join(file), index).unwrap();
        }
    }
}

const ALL: [(&str, &str); 6] =
    [("claude-code", "claude"), ("codex", "codex"), ("pi", "pi"), ("dsh", "dsh"), ("kimi", "kimi"), ("grok", "grok")];

#[tokio::test]
async fn each_harness_starts_with_its_line_and_its_turn_is_read_from_its_own_session_files() {
    for (key, command) in ALL {
        let w = World::new();
        w.checkout("web").await;
        w.app.kit_set(&[format!("{key}={command}")]).await.unwrap();
        w.app.issue_add("web", "t", "", false).await.unwrap();
        w.tick().await;
        let wk = worker(&w.app).await;
        assert_eq!(wk["harness"], key);
        let term = w.terminal(wk["terminal"].as_str().unwrap());
        assert!(term.line.contains(command), "{key}: {}", term.line);
        assert!(term.line.contains("FACTORIUM_WORKER"), "{key}: the worker is named in its environment");
        let briefed = term.line.contains("web-1-1.md") || term.sent.iter().any(|s| s.contains("web-1-1.md"));
        assert!(briefed, "{key}: handed its brief");

        let h = Harness::read(key).unwrap();
        let cwd = wk["worktree"].as_str().unwrap();
        let now = (w.app_now() / 1000) as u64;
        write_session(&w, h, cwd, true, now);
        w.advance(45);
        w.tick().await;
        assert_eq!(worker(&w.app).await["status"], "working", "{key}: its open turn keeps it from silence");
    }
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
    w.app.issue_add("web", "t", "", false).await.unwrap();
    let secrets = w.dir.join("data").join("secrets");
    *w.host.orca.refuse_open.lock().unwrap() = true;
    let _ = w.app.tick().await;
    assert_eq!(std::fs::read_dir(&secrets).unwrap().count(), 0, "a terminal that never opened leaves no secret");
    *w.host.orca.refuse_open.lock().unwrap() = false;
    w.tick().await;
    let wk = worker(&w.app).await;
    assert_eq!(wk["account"], "team");
    let line = w.terminal(wk["terminal"].as_str().unwrap()).line;
    assert!(!line.contains("sk-ant-very-secret"), "{line}");
    let bin = std::env::current_exe().unwrap().parent().unwrap().to_string_lossy().into_owned();
    assert!(line.contains(&format!("PATH='{bin}:")) || line.contains(&format!("PATH={bin}:")), "factorium by name: {line}");
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
