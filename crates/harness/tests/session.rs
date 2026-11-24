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
        logins: &[],
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

/// The one session file a stand-in wrote under `root`.
fn written(root: &std::path::Path) -> std::path::PathBuf {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p)
            } else if p.extension().is_some_and(|x| x == "jsonl") {
                found.push(p)
            }
        }
    }
    assert_eq!(found.len(), 1, "{found:?}");
    found.pop().unwrap()
}

/// What a session spent, read as each harness writes it (shapes taken from
/// real session files on this host): Claude Code writes a line per part of
/// a message, each with the whole message's usage, so a message counts
/// once; Codex's `token_count` events carry running totals, so the last
/// counts; pi's assistant messages each carry their own; so do agy's answers, cache
/// reads beside input.
#[test]
fn a_session_s_tokens_are_read_as_its_harness_writes_them() {
    use serde_json::json;
    let line = |v: serde_json::Value| format!("{v}\n");
    let cases = [
        (
            "claude-code",
            [
                json!({"type": "assistant", "message": {"id": "m1", "role": "assistant", "content": [{"type": "thinking"}],
                    "usage": {"input_tokens": 2, "cache_creation_input_tokens": 860, "cache_read_input_tokens": 40062, "output_tokens": 151}}}),
                json!({"type": "assistant", "message": {"id": "m1", "role": "assistant", "content": [{"type": "tool_use"}],
                    "usage": {"input_tokens": 2, "cache_creation_input_tokens": 860, "cache_read_input_tokens": 40062, "output_tokens": 151}}}),
                json!({"type": "user", "message": {"role": "user", "content": "go on"}}),
            ],
            (15 + 2 + 860 + 40062, 11 + 151),
        ),
        (
            "codex",
            [
                json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage":
                    {"input_tokens": 24655, "cached_input_tokens": 13440, "output_tokens": 12}}}}),
                json!({"type": "event_msg", "payload": {"type": "token_count", "info": null}}),
                json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage":
                    {"input_tokens": 30000, "cached_input_tokens": 20000, "output_tokens": 40}}}}),
            ],
            (30000, 40),
        ),
        (
            "pi",
            [
                json!({"type": "message", "message": {"role": "assistant", "stopReason": "toolUse",
                    "usage": {"input": 7744, "output": 16, "cacheRead": 100, "cacheWrite": 1, "totalTokens": 7861}}}),
                json!({"type": "message", "message": {"role": "user"}}),
                json!({"type": "message", "message": {"role": "assistant", "stopReason": "stop",
                    "usage": {"input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0}}}),
            ],
            (15 + 7744 + 100 + 1 + 10, 11 + 16 + 5),
        ),
        (
            "agy",
            [
                json!({"type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2027-01-15T08:00:01Z", "input_tokens": 2166,
                    "cache_read_tokens": 5272, "output_tokens": 204, "tool_calls": [{"name": "run_command", "args": "{}"}]}),
                json!({"type": "GENERIC", "status": "DONE", "created_at": "2027-01-15T08:00:02Z", "content": "Command exited with code 0."}),
                json!({"type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2027-01-15T08:00:03Z", "input_tokens": 9155,
                    "cache_read_tokens": 0, "output_tokens": 23, "content": "OK"}),
            ],
            (15 + 2166 + 5272 + 9155, 11 + 204 + 23),
        ),
    ];
    for (key, lines, want) in cases {
        let h = Harness::read(key).unwrap();
        let kept = scratch(&format!("spent-{key}"));
        let home = h.home(&kept, None, &kept);
        assert_eq!(h.spent(&home, "/w", "/briefs/w-1-1.md"), None, "{key}: no session, nothing said");
        // A stand-in's ended turn spent 15 given and 11 written; the real lines follow it.
        h.write_session(&home, "/w", "/briefs/w-1-1.md", false, 1_800_000_000).unwrap();
        let file = written(&kept);
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.extend(lines.into_iter().map(line));
        std::fs::write(&file, text).unwrap();
        assert_eq!(h.spent(&home, "/w", "/briefs/w-1-1.md"), Some(want), "{key}");
        assert_eq!(h.spent(&home, "/other", "/briefs/other-1-1.md"), None, "{key}: another session's are not its");
        let _ = std::fs::remove_dir_all(&kept);
    }
    for key in ["dsh", "kimi", "grok"] {
        let h = Harness::read(key).unwrap();
        let kept = scratch(&format!("spent-{key}"));
        let home = h.home(&kept, None, &kept);
        h.write_session(&home, "/w", "/briefs/w-1-1.md", false, 1_800_000_000).unwrap();
        assert_eq!(h.spent(&home, "/w", "/briefs/w-1-1.md"), None, "{key}: its files are not read for tokens");
        let _ = std::fs::remove_dir_all(&kept);
    }
}

/// A session file that names the worktree's path without recording its
/// turn (another worktree's, beneath it) spends nothing of the session's.
#[test]
fn a_file_that_only_names_the_worktree_spends_nothing_of_its_session() {
    use serde_json::json;
    let claude = Harness::read("claude-code").unwrap();
    let kept = scratch("spent-decoy");
    let home = claude.home(&kept, None, &kept);
    claude.write_session(&home, "/w", "/briefs/w-1-1.md", false, 1_800_000_000).unwrap();
    let decoy = [
        json!({"type": "user", "cwd": "/w/sub", "isSidechain": false, "timestamp": "2027-01-15T08:00:00Z",
            "message": {"role": "user", "content": "something else"}}),
        json!({"type": "assistant", "cwd": "/w/sub", "message": {"id": "d", "usage": {"input_tokens": 1000, "output_tokens": 1000}}}),
    ];
    let dir = kept.join(".claude/projects/elsewhere");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("decoy.jsonl"), decoy.iter().map(|v| format!("{v}\n")).collect::<String>()).unwrap();
    assert_eq!(claude.spent(&home, "/w", "/briefs/w-1-1.md"), Some((15, 11)));
    let _ = std::fs::remove_dir_all(&kept);
}

/// agy keeps every worktree's conversations in one `brain`: a transcript is
/// the session's handed the brief its first brief pointer names. A user's
/// input opens its turn; an answer that calls a tool, and the tool's result,
/// leave it open; one that calls none ends it. `transcript_full.jsonl`, beside
/// it, is not read (shapes from agy 1.5.14; 1_800_000_000 is 2027-01-15T08:00:00Z).
#[test]
fn an_agy_turn_is_read_from_the_transcript_its_brief_opened() {
    use serde_json::json;
    let agy = Harness::read("agy").unwrap();
    let kept = scratch("agy-turn");
    let home = agy.home(&kept, None, &kept);
    let brief = "/briefs/web-1-1.md";
    agy.write_session(&home, "/worktree", brief, true, 1_800_000_000).unwrap();
    assert_eq!(agy.turn(&home, "/worktree", brief), Some(Turn::Open { since: 1_800_000_000 }));
    let file = written(&kept);
    let append = |line: serde_json::Value| {
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, format!("{text}{line}\n")).unwrap();
    };
    let at = |second: u32| format!("2027-01-15T08:00:{second:02}Z");
    append(json!({"type": "GENERIC", "status": "DONE", "created_at": at(2), "content": "Command exited with code 0."}));
    assert_eq!(agy.turn(&home, "/worktree", brief), Some(Turn::Open { since: 1_800_000_000 }), "a tool's result");
    append(json!({"type": "PLANNER_RESPONSE", "status": "DONE", "created_at": at(3), "content": "Done."}));
    let closed = Some(Turn::Closed { since: 1_800_000_003, started: Some(1_800_000_000) });
    assert_eq!(agy.turn(&home, "/worktree", brief), closed);
    append(json!({"type": "USER_INPUT", "status": "DONE", "created_at": at(10),
        "content": "<USER_REQUEST>\ngo on\n</USER_REQUEST>"}));
    assert_eq!(agy.turn(&home, "/worktree", brief), Some(Turn::Open { since: 1_800_000_010 }), "a later prompt");

    assert_eq!(agy.turn(&home, "/worktree", "/briefs/web-1-1"), None, "a brief whose path begins it");
    assert_eq!(agy.turn(&home, "/worktree", "/briefs/web-2-1.md"), None, "another worktree's brief");
    std::fs::rename(&file, file.with_file_name("transcript_full.jsonl")).unwrap();
    assert_eq!(agy.turn(&home, "/worktree", brief), None, "only the full transcript says it");
    assert_eq!(agy.spent(&home, "/worktree", brief), None);
    let _ = std::fs::remove_dir_all(&kept);
}

/// A tick looks at every live session's files (factorium#221): a second look
/// at a long session file unchanged since reads it not at all, even shut to
/// reading, and still answers its turn and its tokens; once it grows, it is
/// read again and answers what was added.
#[test]
fn a_second_look_at_an_unchanged_session_file_does_not_read_it() {
    use std::os::unix::fs::PermissionsExt;
    let claude = Harness::read("claude-code").unwrap();
    let kept = scratch("unchanged");
    let home = claude.home(&kept, None, &kept);
    let brief = "/briefs/w-1-1.md";
    claude.write_session(&home, "/w", brief, true, 1_800_000_000).unwrap();
    let file = written(&kept);
    let output = serde_json::json!({"type": "progress", "cwd": "/w", "isSidechain": false,
        "timestamp": "2027-01-15T08:00:01Z", "toolUseResult": "x".repeat(64 * 1024)});
    let long: String = std::iter::repeat_n(format!("{output}\n"), 480).collect();
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(&long);
    std::fs::write(&file, &text).unwrap();
    assert!(text.len() > 30_000_000, "{}", text.len());

    let open = Some(Turn::Open { since: 1_800_000_000 });
    let shut = |mode: u32| std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
    let other = "/briefs/w-2-1.md";
    assert_eq!(claude.turn(&home, "/w", brief), open);
    assert_eq!(claude.spent(&home, "/w", brief), Some((0, 0)));
    assert_eq!(claude.turn(&home, "/w", other), open, "another session's look at the same file");
    shut(0o000);
    assert!(std::fs::read(&file).is_err(), "the file is shut to reading");
    assert_eq!(claude.turn(&home, "/w", brief), open, "answered unread");
    assert_eq!(claude.spent(&home, "/w", brief), Some((0, 0)), "answered unread");
    assert_eq!(claude.turn(&home, "/w", other), open, "answered unread");

    shut(0o644);
    let end = serde_json::json!({"type": "assistant", "cwd": "/w", "isSidechain": false,
        "timestamp": "2027-01-15T08:00:05Z",
        "message": {"id": "m", "stop_reason": "end_turn", "usage": {"input_tokens": 2, "output_tokens": 3}}});
    text.push_str(&format!("{end}\n"));
    std::fs::write(&file, &text).unwrap();
    let closed = Some(Turn::Closed { since: 1_800_000_005, started: Some(1_800_000_000) });
    assert_eq!(claude.turn(&home, "/w", brief), closed);
    assert_eq!(claude.spent(&home, "/w", brief), Some((2, 3)));
    shut(0o000);
    assert_eq!(claude.turn(&home, "/w", brief), closed, "the grown file answered unread once read");
    shut(0o644);
    let _ = std::fs::remove_dir_all(&kept);
}
