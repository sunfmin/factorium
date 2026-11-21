//! A cut's install as Homebrew answers it: another brew holding the
//! formula's lock is waited for, and named once it outlasts the bound
//! (factorium#173).

use std::sync::Mutex;

use factorium_host::brew::Brew;
use factorium_host::shell::{Programs, Said};

/// Homebrew with web 0.0.9 installed, whose install and upgrade answer
/// `upgrade` until it says `None`: then web is at 0.1.0.
struct Scripted {
    upgrade: Mutex<Vec<Option<&'static str>>>,
    installed: Mutex<&'static str>,
    ps: &'static str,
    ran: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(upgrade: Vec<Option<&'static str>>, ps: &'static str) -> Self {
        Scripted { upgrade: Mutex::new(upgrade), installed: Mutex::new("0.0.9"), ps, ran: Mutex::default() }
    }
    fn upgrades(&self) -> usize {
        self.ran.lock().unwrap().iter().filter(|r| *r == "brew upgrade web").count()
    }
}

impl Programs for Scripted {
    fn run(&self, program: &str, args: &[&str]) -> Said {
        self.ran.lock().unwrap().push(format!("{program} {}", args.join(" ")));
        let ok = |out: String| Said { ok: true, out, err: String::new() };
        match (program, args) {
            ("brew", ["upgrade", _]) => {
                let mut left = self.upgrade.lock().unwrap();
                let next = if left.len() > 1 { left.remove(0) } else { left[0] };
                match next {
                    Some(err) => Said { ok: false, out: String::new(), err: err.into() },
                    None => {
                        *self.installed.lock().unwrap() = "0.1.0";
                        ok(String::new())
                    }
                }
            }
            ("brew", ["list", "--versions", _]) => ok(format!("web {}\n", self.installed.lock().unwrap())),
            ("ps", _) => ok(self.ps.into()),
            _ => ok(String::new()),
        }
    }
}

const LOCKED: &str = "Error: A `brew upgrade web` process has already locked /h/Cellar/web.";

#[test]
fn a_lock_freed_within_the_bound_is_waited_for() {
    let brew = Scripted::new(vec![Some(LOCKED), None], "");
    assert!(Brew(&brew).upgrade("tap/web", "v0.1.0").is_ok());
}

#[test]
fn an_error_other_than_a_lock_fails_at_once() {
    let brew = Scripted::new(vec![Some("Error: SHA256 mismatch")], "");
    let why = Brew(&brew).upgrade("web", "v0.1.0").unwrap_err();
    assert!(why.contains("SHA256 mismatch") && !why.contains("still locked"), "{why}");
    assert_eq!(brew.upgrades(), 1, "not waited for as a lock");
}

#[test]
fn a_lock_held_past_the_bound_names_brews_processes_on_the_formula() {
    let ps =
        "  1 /usr/sbin/sshd\n 42 /h/ruby brew.rb upgrade sunfmin/tap/web\n 43 vim web\n 44 /h/bin/brew upgrade api\n";
    let brew = Scripted::new(vec![Some(LOCKED)], ps);
    let why = Brew(&brew).upgrade("sunfmin/tap/web", "v0.1.0").unwrap_err();
    assert!(why.contains("still locked after 90s"), "{why}");
    assert!(why.ends_with("; held by 42 /h/ruby brew.rb upgrade sunfmin/tap/web)"), "only brew on web: {why}");
}
