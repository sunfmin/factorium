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
