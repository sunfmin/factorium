//! Whether a session took a text as a prompt, as each harness's session
//! files say it, or do not.

use std::path::Path;

use factorium_harness::Harness;

/// Claude Code's session files say what it took: with none, it took
/// nothing. Every other harness's files do not say, so it never answers.
#[test]
fn only_claude_code_s_session_files_say_whether_a_text_was_taken_as_a_prompt() {
    let nowhere = Path::new("/nonexistent/factorium-prompted");
    for &h in Harness::all() {
        let home = h.home(nowhere, None, nowhere);
        let said = h.prompted(&home, "/w", "brief", "use the v2 API", 0);
        let says = if h == Harness::read("claude-code").unwrap() { Some(false) } else { None };
        assert_eq!(said, says, "{}", h.key());
    }
}
