//! A tool call as the guard judges it, read from the hook payload each
//! harness gives.

use factorium_harness::{Call, Harness};

/// DSH runs a shell line by its `bash` and `bash_persistent` tools; any other
/// tool, or one with no command, runs none.
#[test]
fn a_dsh_call_is_a_shell_line_only_by_its_bash_tools() {
    let dsh = Harness::read("dsh").unwrap();
    for tool in ["bash", "bash_persistent"] {
        let payload = format!(r#"{{"name":"{tool}","arguments":{{"command":"git push"}}}}"#);
        assert_eq!(dsh.shell_call(&payload), Call::Shell("git push".into()), "{tool}");
    }
    assert_eq!(dsh.shell_call(r#"{"name":"read","arguments":{"command":"git push"}}"#), Call::NotShell);
    assert_eq!(dsh.shell_call(r#"{"name":"bash"}"#), Call::NotShell, "no command");
}

/// agy runs a shell line by `run_command`, its `CommandLine` the line, as its
/// `PreToolUse` hook is handed it (agy 1.5.14, beside the session's own
/// fields); any other tool runs none; a `run_command` whose line is not
/// there, or a payload with no call, is not read, and so refused.
#[test]
fn an_agy_call_is_a_shell_line_only_by_run_command() {
    let agy = Harness::read("agy").unwrap();
    let call = |name: &str, args: serde_json::Value| {
        serde_json::json!({"conversationId": "c", "cwd": "/w", "hookEventName": "PreToolUse", "stepIdx": 2,
            "toolCall": {"args": args, "id": "call_1", "name": name}, "workspacePaths": ["/w"]})
        .to_string()
    };
    let push = serde_json::json!({"CommandLine": "git push", "WaitMsBeforeAsync": 5000, "toolSummary": "Push"});
    assert_eq!(agy.shell_call(&call("run_command", push.clone())), Call::Shell("git push".into()));
    assert_eq!(agy.shell_call(&call("read_file", push)), Call::NotShell);
    assert_eq!(agy.shell_call(r#"{"toolCall":{"name":"view_file"}}"#), Call::NotShell, "no args");
    for unread in [
        call("run_command", serde_json::json!({"Cwd": "/w"})),
        call("run_command", serde_json::json!({"CommandLine": 1})),
        r#"{"toolCall":{"name":"run_command"}}"#.into(),
        r#"{"toolCall":{"args":{"CommandLine":"git push"}}}"#.into(),
        r#"{"tool_name":"run_command","tool_input":{"command":"git push"}}"#.into(),
    ] {
        assert!(matches!(agy.shell_call(&unread), Call::Unreadable(_)), "{unread}");
    }
}
