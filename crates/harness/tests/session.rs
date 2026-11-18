//! The stand-in session each harness writes where its turn is read.

use factorium_harness::{Harness, Turn};

/// A fresh, empty directory for one test.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("factorium-harness-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Kimi Code's index names each worktree's session: the stand-in replaces
/// this worktree's line and keeps the others, a line that is not JSON too,
/// and its turn is read back through it.
#[test]
fn a_kimi_stand_in_names_itself_in_the_index_and_keeps_the_other_lines() {
    let kimi = Harness::read("kimi").unwrap();
    let kept = scratch("kimi-index");
    let home = kimi.home(&kept, None, &kept);
    let other = r#"{"sessionId":"x","sessionDir":"sessions/x","workDir":"/other"}"#;
    let own = r#"{"sessionId":"y","sessionDir":"sessions/y","workDir":"/w/"}"#;
    std::fs::write(kept.join("session_index.jsonl"), format!("not json\n\n{own}\n{other}\n")).unwrap();

    kimi.write_session(&home, "/w", "brief", false, 1_800_000_000).unwrap();

    let index = std::fs::read_to_string(kept.join("session_index.jsonl")).unwrap();
    let lines: Vec<&str> = index.lines().collect();
    assert_eq!(lines[..2], ["not json", other], "{index}");
    assert_eq!(lines.len(), 3, "{index}");
    assert!(lines[2].contains(r#""workDir":"/w""#), "{index}");
    assert_eq!(
        kimi.turn(&home, "/w", "brief"),
        Some(Turn::Closed { since: 1_800_000_000, started: Some(1_800_000_000) })
    );
    let _ = std::fs::remove_dir_all(&kept);
}

/// One JSON object per line, as a harness writes its session file.
fn jsonl(lines: &[serde_json::Value]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

/// Codex handed a brief as its goal, started outside the worktree, is that
/// worktree's session by the goal alone: the objective it was typed, and no
/// other brief's, not even one whose path begins it.
#[test]
fn a_codex_session_handed_the_brief_as_its_goal_is_the_worktree_s() {
    use serde_json::json;
    let codex = Harness::read("codex").unwrap();
    let kept = scratch("codex-goal");
    let home = codex.home(&kept, None, &kept);
    let brief = "/briefs/web-1-1.md";
    let start = factorium_harness::Start {
        command: "codex",
        dir: "/start",
        worktree: "/worktree",
        model: None,
        guard: "factorium",
        home: &home,
        user_home: &kept,
        env: Vec::new(),
        brief,
        secrets: None,
    };
    let typed = codex.launch(&start).unwrap().typed.unwrap();
    let objective = typed.strip_prefix("/goal ").unwrap();
    let at = |seconds| format!("2027-01-15T08:00:{seconds:02}Z");
    let session = jsonl(&[
        json!({"timestamp": at(0), "type": "session_meta", "payload": {"cwd": "/start", "id": "t"}}),
        json!({"timestamp": at(0), "type": "event_msg",
               "payload": {"type": "thread_goal_updated", "goal": {"objective": objective}}}),
        json!({"timestamp": at(1), "type": "event_msg", "payload": {"type": "task_started", "started_at": 1_800_000_001u64}}),
    ]);
    std::fs::create_dir_all(kept.join("sessions/2027")).unwrap();
    std::fs::write(kept.join("sessions/2027/rollout-t.jsonl"), session).unwrap();

    assert_eq!(codex.turn(&home, "/worktree", brief), Some(Turn::Open { since: 1_800_000_001 }));
    assert_eq!(codex.turn(&home, "/worktree", "/briefs/web-1-1"), None, "a brief whose path begins the goal's");
    let _ = std::fs::remove_dir_all(&kept);
}

/// The one session file under `dir`, which a stand-in wrote.
fn only_file(dir: &std::path::Path) -> std::path::PathBuf {
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() { dirs.push(path) } else { found.push(path) }
        }
    }
    assert_eq!(found.len(), 1, "{found:?}");
    found.remove(0)
}

/// A Codex turn the owner interrupts ends where Codex wrote it aborted
/// (1_800_000_000 is 2027-01-15T08:00:00Z).
#[test]
fn a_codex_turn_aborted_is_a_turn_ended() {
    let codex = Harness::read("codex").unwrap();
    let kept = scratch("codex-aborted");
    let home = codex.home(&kept, None, &kept);
    codex.write_session(&home, "/worktree", "brief", true, 1_800_000_000).unwrap();
    assert_eq!(codex.turn(&home, "/worktree", "brief"), Some(Turn::Open { since: 1_800_000_000 }));

    let aborted = serde_json::json!({"timestamp": "2027-01-15T08:00:05Z", "type": "event_msg",
                                     "payload": {"type": "turn_aborted", "reason": "interrupted"}});
    let file = only_file(&kept);
    let session = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("{session}{aborted}\n")).unwrap();

    assert_eq!(
        codex.turn(&home, "/worktree", "brief"),
        Some(Turn::Closed { since: 1_800_000_005, started: Some(1_800_000_000) })
    );
    let _ = std::fs::remove_dir_all(&kept);
}

/// pi's turn ends at a reply that stops or errs; a reply that stops for a
/// tool call, and the tool's result, leave it open (1_800_000_000 is 2027-01-15T08:00:00Z).
#[test]
fn a_pi_turn_ends_at_a_reply_that_stops_or_errs_not_at_a_tool_call() {
    use serde_json::json;
    let pi = Harness::read("pi").unwrap();
    let kept = scratch("pi-replies");
    let home = pi.home(&kept, None, &kept);
    pi.write_session(&home, "/worktree", "brief", false, 1_800_000_000).unwrap();
    assert_eq!(
        pi.turn(&home, "/worktree", "brief"),
        Some(Turn::Closed { since: 1_800_000_000, started: Some(1_800_000_000) })
    );
    let file = only_file(&kept);
    let append = |role: &str, stop: Option<&str>, second: u32| {
        let message = json!({"type": "message", "timestamp": format!("2027-01-15T08:00:{second:02}Z"),
                             "message": {"role": role, "stopReason": stop}});
        let session = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, format!("{session}{message}\n")).unwrap();
    };

    append("user", None, 10);
    append("assistant", Some("toolUse"), 11);
    append("toolResult", None, 11);
    assert_eq!(pi.turn(&home, "/worktree", "brief"), Some(Turn::Open { since: 1_800_000_010 }));
    append("assistant", Some("error"), 12);
    assert_eq!(
        pi.turn(&home, "/worktree", "brief"),
        Some(Turn::Closed { since: 1_800_000_012, started: Some(1_800_000_010) })
    );
    let _ = std::fs::remove_dir_all(&kept);
}
