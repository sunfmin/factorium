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

/// Homebrew as the host had it after the cut of v0.1.81 (factorium#244):
/// the cellar holds only 0.1.81, opt follows it, and PATH's `bin/factorium`
/// still points to `../Cellar/factorium/0.1.80_1/bin/factorium`, removed.
/// `brew link --overwrite` relinks it, unless `relinks` is false.
struct Stale {
    root: std::path::PathBuf,
    relinks: bool,
    ran: Mutex<Vec<String>>,
}

impl Stale {
    fn new(name: &str, relinks: bool) -> Self {
        let root = std::env::temp_dir().join(format!("factorium-brew-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let keg = root.join("Cellar/factorium/0.1.81");
        std::fs::create_dir_all(keg.join("bin")).unwrap();
        std::fs::write(keg.join("bin/factorium"), "#!/bin/sh\n").unwrap();
        std::fs::create_dir_all(root.join("opt")).unwrap();
        std::os::unix::fs::symlink("../Cellar/factorium/0.1.81", root.join("opt/factorium")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::os::unix::fs::symlink("../Cellar/factorium/0.1.80_1/bin/factorium", root.join("bin/factorium")).unwrap();
        Stale { root, relinks, ran: Mutex::default() }
    }
}

impl Programs for Stale {
    fn run(&self, program: &str, args: &[&str]) -> Said {
        self.ran.lock().unwrap().push(format!("{program} {}", args.join(" ")));
        let ok = |out: String| Said { ok: true, out, err: String::new() };
        match (program, args) {
            ("brew", ["list", "--versions", _]) => ok("factorium 0.1.81\n".into()),
            ("brew", ["--cellar", _]) => ok(format!("{}\n", self.root.join("Cellar/factorium").display())),
            ("brew", ["--prefix"]) => ok(format!("{}\n", self.root.display())),
            ("brew", ["link", "--overwrite", _]) if self.relinks => {
                let link = self.root.join("bin/factorium");
                std::fs::remove_file(&link).unwrap();
                std::os::unix::fs::symlink("../Cellar/factorium/0.1.81/bin/factorium", &link).unwrap();
                ok("Linking /h/Cellar/factorium/0.1.81... 1 symlinks created.\n".into())
            }
            _ => ok(String::new()),
        }
    }
}

#[test]
fn the_v0_1_81_cut_relinks_path_onto_its_keg() {
    let brew = Stale::new("relinks", true);
    Brew(&brew).upgrade("sunfmin/tap/factorium", "v0.1.81").unwrap();
    let links = brew.ran.lock().unwrap().iter().filter(|r| r.starts_with("brew link")).count();
    assert_eq!(links, 1, "relinked once");
    let on_path = brew.root.join("bin/factorium").canonicalize().unwrap();
    assert_eq!(on_path, brew.root.join("Cellar/factorium/0.1.81/bin/factorium").canonicalize().unwrap());
}

#[test]
fn the_v0_1_81_cut_fails_naming_the_link_relinking_leaves_stale() {
    let brew = Stale::new("stays", false);
    let why = Brew(&brew).upgrade("sunfmin/tap/factorium", "v0.1.81").unwrap_err();
    let link = brew.root.join("bin/factorium");
    let named = format!("`{}` points to `../Cellar/factorium/0.1.80_1/bin/factorium`", link.display());
    assert!(why.contains(&named) && why.contains("brew link --overwrite sunfmin/tap/factorium"), "{why}");
}

#[test]
fn a_path_entry_missing_or_not_a_link_is_named_as_such() {
    let brew = Stale::new("missing", false);
    let link = brew.root.join("bin/factorium");
    std::fs::remove_file(&link).unwrap();
    let why = Brew(&brew).upgrade("sunfmin/tap/factorium", "v0.1.81").unwrap_err();
    assert!(why.contains(&format!("`{}` is missing", link.display())), "{why}");
    std::fs::write(&link, "#!/bin/sh\n").unwrap();
    let why = Brew(&brew).upgrade("sunfmin/tap/factorium", "v0.1.81").unwrap_err();
    assert!(why.contains(&format!("`{}` is not a link", link.display())), "{why}");
}
