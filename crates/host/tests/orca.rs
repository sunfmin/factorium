//! A key pressed at a terminal as Orca answers the wait after it: Orca
//! 1.4.267 never sees Claude Code 2.1.330 idle while it holds a shell in
//! the background, so a wait that times out is no failure (factorium#179);
//! any other answer still is.

use std::sync::Mutex;

use factorium_host::orca::{Orca, Term};
use factorium_host::shell::{Programs, Said};

/// Orca that types whatever it is given, and answers each wait for
/// tui-idle with `wait` (an error code, or `None`: satisfied).
struct Scripted {
    wait: Option<&'static str>,
    ran: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(wait: Option<&'static str>) -> Self {
        Scripted { wait, ran: Mutex::default() }
    }
}

impl Programs for Scripted {
    fn run(&self, program: &str, args: &[&str]) -> Said {
        self.ran.lock().unwrap().push(format!("{program} {}", args.join(" ")));
        let out = match (args.get(1), self.wait) {
            (Some(&"wait"), Some(code)) => {
                return Said {
                    ok: false,
                    out: format!(r#"{{"ok":false,"error":{{"code":"{code}","message":"{code}"}}}}"#),
                    err: String::new(),
                };
            }
            (Some(&"wait"), None) => r#"{"ok":true,"result":{"satisfied":true}}"#,
            (Some(&"read"), _) => r#"{"ok":true,"result":{"terminal":{"status":"running","tail":["$ "]}}}"#,
            _ => r#"{"ok":true,"result":{"send":{"accepted":true}}}"#,
        };
        Said { ok: true, out: out.into(), err: String::new() }
    }
}

#[test]
fn a_press_whose_wait_times_out_succeeds_having_waited_five_seconds() {
    let orca = Scripted::new(Some("timeout"));
    assert_eq!(Orca(&orca).press("t", "\x1b"), Ok(()));
    let ran = orca.ran.lock().unwrap();
    assert_eq!(ran[0], "orca terminal send --terminal t --text \x1b --json");
    assert_eq!(ran[1], "orca terminal wait --terminal t --for tui-idle --timeout-ms 5000 --json");
}

#[test]
fn a_press_whose_wait_fails_otherwise_fails() {
    let orca = Scripted::new(Some("terminal_exited"));
    assert_eq!(Orca(&orca).press("t", "\x1b"), Err("orca terminal wait: terminal_exited".into()));
}

#[test]
fn a_terminal_never_seen_idle_is_not_ready() {
    let orca = Scripted::new(Some("timeout"));
    assert_eq!(Orca(&orca).ready("t"), Err("orca terminal wait: timeout".into()));
    assert!(Orca(&Scripted::new(None)).ready("t").is_ok());
}

/// A screen read is given a limit; programs whose answers
/// come at once, as this stand-in's, are read as they answer.
#[test]
fn a_screen_read_through_programs_that_answer_at_once_is_the_screen() {
    let orca = Scripted::new(None);
    assert_eq!(Orca(&orca).screen("t"), Term::Running(vec!["$ ".into()]));
    assert_eq!(orca.ran.lock().unwrap()[0], "orca terminal read --terminal t --screen --limit 200 --json");
}
