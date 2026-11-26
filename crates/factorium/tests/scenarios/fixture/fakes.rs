//! The machine a scenario runs on: Orca, Homebrew, stablehand, slimfit and launchd as
//! programs whose answers the scenario gives, ssh to the other hosts it
//! knows, the host's readings and processes, and its one clock. Git is real.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use factorium::host::{Clock, Git, Host, ManualClock, Proc, Programs, Reading, Said};
use factum::Uuid;
use serde_json::{Value, json};

/// A terminal the stand-in Orca keeps: what started it, what was typed at
/// it, and what it shows.
#[derive(Clone, Debug, Default)]
pub struct Terminal {
    pub worktree: PathBuf,
    pub title: String,
    pub line: String,
    pub sent: Vec<String>,
    pub screen: Vec<String>,
    pub alive: bool,
    /// Its agent is in a turn that does not end (a hung call): a line typed
    /// with Enter queues behind it, and only Escape stops it.
    pub busy: bool,
    /// Orca does not see its agent's turns start (one that ends within a
    /// second, or starts right after an interrupt).
    pub unobserved: bool,
    /// Its agent holds a shell it moved to the background: Orca sees it
    /// working, in a turn or not (Claude Code 2.1.330), so a wait for it
    /// to sit idle times out.
    pub background: bool,
    /// Closed through Orca: its tab is gone, not only its program.
    pub closed: bool,
    /// Its harness opened a dialog of its own: a line typed with Enter
    /// answers it and starts no turn; Escape dismisses it, back to its prompt.
    pub dialog: bool,
}

/// A terminal's screen at its shell's prompt, as zsh draws it.
pub const SHELL: &str = "felix@macnoMac-Studio web-1-1 % ";
/// The Escape key, as Orca is given it to type.
pub const ESCAPE: &str = "\x1b";
/// A terminal's screen with its harness up: its prompt, and a footer with
/// the words a harness that names some shows once up (Kimi Code's).
pub const UP: &str = "› Ask anything · ? for shortcuts · context: 0% (0/1M)";

/// What runs while a terminal closes, given its handle.
pub type Closing = Box<dyn FnMut(&str) + Send>;

/// Orca as `orca … --json` answers: its envelopes are shaped as Orca 1.4
/// answered them (`{"ok":true,"result":…}`, `{"ok":false,"error":{"code",
/// "message"}}`, `terminal read` giving `terminal.status` and `terminal.tail`).
#[derive(Default)]
pub struct FakeOrca {
    pub terminals: Mutex<BTreeMap<String, Terminal>>,
    pub removed: Mutex<Vec<PathBuf>>,
    /// The worktrees it made and holds, by path: the name it shows and the comment it keeps.
    pub made: Mutex<BTreeMap<PathBuf, (String, String)>>,
    /// Orca refuses to open terminals while set.
    pub refuse_open: Mutex<bool>,
    /// Terminals open at their shell's prompt while set, their harness not
    /// up; else at the harness's own screen.
    pub opens_at_shell: Mutex<bool>,
    /// Orca refuses to type at terminals while set ([`FakeOrca::fault`]).
    refuse_send: Mutex<bool>,
    /// While set, a terminal's program ends at the key typed at it, and Orca
    /// refuses what comes after, as it does for one exited
    /// ([`FakeOrca::fault`]).
    key_ends: Mutex<bool>,
    /// While set, `orca` prints no answer, only this on stderr (its runtime is down).
    pub down: Mutex<Option<String>>,
    /// The repos and worktrees Orca holds, by id.
    held: Mutex<BTreeMap<String, PathBuf>>,
    /// Repos Orca was just given: their own worktree is not found once yet.
    finding: Mutex<Vec<PathBuf>>,
    /// While set, a repo Orca was just given is never found.
    pub never_found: Mutex<bool>,
    /// Orca refuses to remove worktrees while set ([`FakeOrca::fault`]).
    refuse_rm: Mutex<bool>,
    /// Orca refuses to close a terminal while set.
    pub refuse_close: Mutex<bool>,
    /// While set, Orca answers a terminal's close and leaves it as it was.
    pub ignore_close: Mutex<bool>,
    /// Real Orca (1.4.254) takes seconds to close an agent's terminal (2.2s
    /// for dsh's) and lists it, connected, until the close returns. While
    /// set, this runs with the handle when one terminal's close is asked,
    /// before it takes: what another process sees in that time.
    pub while_closing: Mutex<Option<Closing>>,
    /// While set, this runs with the worktree when a terminal is asked to
    /// open there, before Orca answers.
    pub while_opening: Mutex<Option<Closing>>,
    /// While set, this runs with the worktree when one is asked to be
    /// removed, before Orca removes it.
    pub while_removing: Mutex<Option<Closing>>,
    /// While set, a `terminal send --wait-submit` that starts no turn (to a
    /// busy agent, one Orca does not see, or one in a dialog) takes this long
    /// before it answers, as real Orca waits its whole window for a turn that
    /// does not start; sends to other terminals are not held up by it.
    pub window: Mutex<Option<std::time::Duration>>,
}

fn said(ok: bool, out: Value) -> Said {
    Said { ok, out: out.to_string(), err: String::new() }
}

fn answer(result: Value) -> Said {
    said(true, json!({ "id": Uuid::now_v7().to_string(), "ok": true, "result": result }))
}

fn error(code: &str) -> Said {
    said(false, json!({ "id": Uuid::now_v7().to_string(), "ok": false, "error": { "code": code, "message": code } }))
}

/// The value after `--name` in `args`.
fn flag<'a>(args: &[&'a str], name: &str) -> &'a str {
    args.iter().position(|a| *a == name).and_then(|i| args.get(i + 1)).copied().unwrap_or_default()
}

/// The failures the stand-in Orca answers when asked (truefit.toml's orca
/// `faults`, named by `TRUEFIT_FAULT` to `standin fake`), each as real Orca
/// 1.4.267 answers it: `send-refused`, a terminal it will not type at (as
/// one closed); `remove-refused`, a worktree it will not remove (as a
/// directory it does not hold); `refused-after-key`, a terminal that takes
/// a key and then refuses the wait for it and the text after (as one whose
/// program the key ended).
pub const ORCA_FAULTS: [&str; 3] = ["send-refused", "remove-refused", "refused-after-key"];

impl FakeOrca {
    /// Sets, or clears, the failure `name` (one of [`ORCA_FAULTS`]).
    pub fn fault(&self, name: &str, on: bool) {
        let flag = match name {
            "send-refused" => &self.refuse_send,
            "remove-refused" => &self.refuse_rm,
            "refused-after-key" => &self.key_ends,
            _ => panic!("no Orca fault {name}: {ORCA_FAULTS:?}"),
        };
        *flag.lock().unwrap() = on;
    }

    fn answer(&self, root: &Path, args: &[&str]) -> Said {
        if let Some(why) = &*self.down.lock().unwrap() {
            return Said { ok: false, out: String::new(), err: why.clone() };
        }
        if args[..2] == ["terminal", "close"] && !args.contains(&"--all") {
            let during = self.while_closing.lock().unwrap().take();
            if let Some(mut during) = during {
                during(flag(args, "--terminal"));
                *self.while_closing.lock().unwrap() = Some(during);
            }
        }
        if args[..2] == ["terminal", "create"] {
            let during = self.while_opening.lock().unwrap().take();
            if let Some(mut during) = during {
                during(flag(args, "--worktree"));
                *self.while_opening.lock().unwrap() = Some(during);
            }
        }
        if args[..2] == ["worktree", "rm"] {
            let during = self.while_removing.lock().unwrap().take();
            if let Some(mut during) = during {
                during(flag(args, "--worktree"));
                *self.while_removing.lock().unwrap() = Some(during);
            }
        }
        // Outside every lock: Orca waits on one terminal while it types at another.
        let no_turn = || {
            let t = self.terminals.lock().unwrap();
            t.get(flag(args, "--terminal")).is_some_and(|t| t.busy || t.unobserved || t.dialog)
        };
        let takes = *self.window.lock().unwrap();
        if let Some(takes) = takes
            && args[..2] == ["terminal", "send"]
            && args.contains(&"--wait-submit")
            && no_turn()
        {
            std::thread::sleep(takes);
        }
        let mut held = self.held.lock().unwrap();
        let path_of = |sel: &str| sel.strip_prefix("path:").map(PathBuf::from);
        match (args[0], args[1]) {
            ("repo", "show") => match held.iter().find(|(_, p)| Some(*p) == path_of(flag(args, "--repo")).as_ref()) {
                Some((id, p)) => answer(json!({ "repo": { "id": id, "path": p } })),
                None => error("repo_not_found"),
            },
            ("repo", "add") => {
                let (id, path) = (format!("repo-{}", held.len() + 1), PathBuf::from(flag(args, "--path")));
                held.insert(id.clone(), path.clone());
                self.finding.lock().unwrap().push(path.clone());
                answer(json!({ "repo": { "id": id, "path": path } }))
            }
            ("worktree", "create") => {
                let repo = path_of(flag(args, "--repo")).unwrap();
                // As Orca: the dir is the name made a slug, and a taken dir or branch gets a suffix.
                let slug: String = flag(args, "--name")
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '-' })
                    .collect();
                let free = |n: u32| {
                    let s = if n == 1 { String::new() } else { format!("-{n}") };
                    let taken = git_of(
                        root,
                        &repo,
                        &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{}{s}", flag(args, "--branch"))],
                    )
                    .is_ok();
                    (!root.join("worktrees").join(format!("{slug}{s}")).exists() && !taken).then_some(s)
                };
                let suffix = (1..).find_map(free).unwrap();
                let path = root.join("worktrees").join(format!("{slug}{suffix}"));
                let (branch, sha) = (format!("{}{suffix}", flag(args, "--branch")), flag(args, "--base-branch"));
                let branch = branch.as_str();
                if let Err(e) =
                    git_of(root, &repo, &["worktree", "add", "--quiet", "-b", branch, &path.to_string_lossy(), sha])
                {
                    return Said { ok: false, out: String::new(), err: e };
                }
                let shown = (flag(args, "--name").to_string(), flag(args, "--comment").to_string());
                self.made.lock().unwrap().insert(path.clone(), shown);
                let id = format!("wt-{}", held.len() + 1);
                held.insert(id, path.clone());
                // Orca opens a shell in a new workspace.
                let shell =
                    Terminal { worktree: path.clone(), title: "shell".into(), alive: true, ..Default::default() };
                self.terminals.lock().unwrap().insert(format!("shell-{}", flag(args, "--name")), shell);
                answer(json!({ "worktree": { "path": path, "branch": branch } }))
            }
            ("worktree", "set") => {
                let path = path_of(flag(args, "--worktree")).unwrap();
                match self.made.lock().unwrap().get_mut(&path) {
                    Some((name, comment)) => {
                        if args.contains(&"--comment") {
                            *comment = flag(args, "--comment").into();
                        }
                        if args.contains(&"--display-name") {
                            *name = flag(args, "--display-name").into();
                        }
                        answer(json!({ "worktree": { "path": path, "displayName": name, "comment": comment } }))
                    }
                    None => error("selector_not_found"),
                }
            }
            ("worktree", "list") => {
                let mut made = self.made.lock().unwrap();
                made.retain(|p, _| p.exists());
                let rows: Vec<Value> = made
                    .iter()
                    .map(|(p, (name, comment))| json!({ "path": p, "displayName": name, "comment": comment }))
                    .collect();
                answer(json!({ "worktrees": rows, "totalCount": rows.len(), "truncated": false }))
            }
            ("worktree", "rm") => {
                let path = path_of(flag(args, "--worktree")).unwrap();
                self.removed.lock().unwrap().push(path.clone());
                // A worktree gone from disk is gone from Orca too.
                if !path.exists() {
                    return error("selector_not_found");
                }
                if *self.refuse_rm.lock().unwrap() {
                    return error("selector_not_found");
                }
                // Orca refuses a worktree that holds another git registers.
                let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
                let listed = git_of(root, &path, &["worktree", "list", "--porcelain"]).unwrap_or_default();
                let inner = listed.lines().filter_map(|l| l.strip_prefix("worktree ")).map(PathBuf::from);
                if let Some(inner) =
                    inner.into_iter().find(|p| real(p) != real(&path) && real(p).starts_with(real(&path)))
                {
                    let message = format!(
                        "Refusing to delete worktree because it contains another registered worktree: {}",
                        inner.display()
                    );
                    let error = json!({ "id": Uuid::now_v7().to_string(), "ok": false, "error": { "code": "refused", "message": message } });
                    return said(false, error);
                }
                if path.exists()
                    && let Err(e) = git_of(root, &path, &["worktree", "remove", "--force", &path.to_string_lossy()])
                {
                    return Said { ok: false, out: String::new(), err: e };
                }
                self.made.lock().unwrap().remove(&path);
                answer(json!({ "removed": true }))
            }
            ("terminal", "create") => {
                if *self.refuse_open.lock().unwrap() {
                    return error("terminal_create_failed");
                }
                let sel = flag(args, "--worktree");
                let worktree = match sel.split_once("::") {
                    Some((id, path)) if held.get(id.strip_prefix("id:").unwrap_or(id)).is_some() => {
                        let mut finding = self.finding.lock().unwrap();
                        match finding.iter().position(|p| p == Path::new(path)) {
                            Some(i) => {
                                if !*self.never_found.lock().unwrap() {
                                    finding.remove(i);
                                }
                                return error("selector_not_found");
                            }
                            None => PathBuf::from(path),
                        }
                    }
                    Some(_) => return error("selector_not_found"),
                    None => match path_of(sel).filter(|p| held.values().any(|h| h == p)) {
                        Some(p) => p,
                        None => return error("selector_not_found"),
                    },
                };
                let mut t = self.terminals.lock().unwrap();
                let handle = format!("term-{}", t.keys().filter(|k| k.starts_with("term-")).count() + 1);
                let screen = if *self.opens_at_shell.lock().unwrap() { SHELL } else { UP };
                let term = Terminal {
                    worktree,
                    title: flag(args, "--title").into(),
                    line: flag(args, "--command").into(),
                    alive: true,
                    screen: vec![screen.into()],
                    ..Default::default()
                };
                t.insert(handle.clone(), term);
                answer(json!({ "terminal": { "handle": handle } }))
            }
            ("terminal", verb) => {
                let mut t = self.terminals.lock().unwrap();
                if verb == "close" && args.contains(&"--all") {
                    let worktree = path_of(flag(args, "--worktree"));
                    for t in t.values_mut().filter(|t| Some(&t.worktree) == worktree.as_ref()) {
                        (t.alive, t.closed) = (false, true);
                    }
                    return answer(json!({ "closed": true }));
                }
                if verb == "list" {
                    let open = t.iter().filter(|(_, t)| !t.closed);
                    let rows: Vec<Value> = open.map(|(h, t)| json!({ "handle": h, "title": t.title })).collect();
                    return answer(json!({ "terminals": rows }));
                }
                let handle = flag(args, "--terminal");
                let Some(term) = t.get_mut(handle) else { return error("terminal_handle_stale") };
                let status = if term.alive { "running" } else { "exited" };
                let ended = *self.key_ends.lock().unwrap() && !term.alive;
                match verb {
                    "read" => {
                        answer(json!({ "terminal": { "handle": handle, "status": status, "tail": term.screen } }))
                    }
                    "wait" if ended => error("terminal_exited"),
                    "send" if ended => error("terminal_not_writable"),
                    "wait" if term.background && flag(args, "--for") == "tui-idle" => error("timeout"),
                    "wait" => answer(json!({ "terminal": { "handle": handle, "status": status }, "satisfied": true })),
                    "send" if *self.refuse_send.lock().unwrap() => error("terminal_not_writable"),
                    "send" if term.alive => {
                        let (text, enter) = (flag(args, "--text"), args.contains(&"--enter"));
                        term.sent.push(text.into());
                        if !enter && text == ESCAPE {
                            term.busy = false;
                            term.alive = !*self.key_ends.lock().unwrap();
                            if term.dialog {
                                (term.dialog, term.screen) = (false, vec![UP.into()]);
                            }
                        }
                        // A watched prompt that reaches an idle agent starts a turn.
                        let mut stages = vec!["input_accepted"];
                        if enter && !term.busy && !term.unobserved && !term.dialog && args.contains(&"--wait-submit") {
                            stages.push("turn_started");
                        }
                        let prompt = enter.then(|| json!({ "stages": stages, "provider": "claude" }));
                        answer(json!({ "send": { "handle": handle, "accepted": true, "prompt": prompt } }))
                    }
                    "close" if *self.refuse_close.lock().unwrap() => error("terminal_close_failed"),
                    "close" if *self.ignore_close.lock().unwrap() => answer(json!({ "closed": true })),
                    "close" => {
                        (term.alive, term.closed) = (false, true);
                        answer(json!({ "closed": true }))
                    }
                    _ => error("terminal_exited"),
                }
            }
            _ => error("unknown_command"),
        }
    }
}

/// Homebrew as `brew` answers. Its tap holds, once `brew update` sees it,
/// the newest tag of the remote of the formula's name.
#[derive(Default)]
pub struct FakeBrew {
    /// Formula → the version installed.
    pub installed: BTreeMap<String, String>,
    /// Formula → the version the tap has.
    pub tap: BTreeMap<String, String>,
    /// How many more `brew update`s see the tap as it was.
    pub behind: u32,
    /// Install and upgrade succeed and change nothing (Homebrew does not link).
    pub stuck: bool,
    /// How many more installs and upgrades find the formula locked by another brew.
    pub locked: u32,
    /// The other brew installs the tap's version while it holds the lock.
    pub holder_installs: bool,
    /// What `ps -Ao pid=,command=` prints.
    pub ps: String,
    /// What installs and upgrades fail with, if they fail as no lock does.
    pub refuses: Option<String>,
    /// Install and upgrade leave PATH's `bin/<name>` on the keg before, and
    /// remove that keg (factorium#244).
    pub stale_link: bool,
    /// `brew link --overwrite` leaves the links as they are.
    pub relink_fails: bool,
}

/// The failures the stand-in Homebrew answers when asked (truefit.toml's
/// brew `faults`, named by `TRUEFIT_FAULT` to `standin fake`):
/// `upgrade-refused`, every install and upgrade refused, though `brew
/// update` took effect, as brew refuses one whose download fails.
pub const BREW_FAULTS: [&str; 1] = ["upgrade-refused"];

impl FakeBrew {
    /// Sets, or clears, the failure `name` (one of [`BREW_FAULTS`]).
    pub fn fault(&mut self, name: &str, on: bool) {
        match name {
            "upgrade-refused" => {
                self.refuses = on.then(|| "Error: Failure while executing; `curl` exited with 6".to_string())
            }
            _ => panic!("no Homebrew fault {name}: {BREW_FAULTS:?}"),
        }
    }

    fn answer(&mut self, root: &Path, args: &[&str]) -> Said {
        let text =
            |ok: bool, out: String| Said { ok, out, err: if ok { String::new() } else { "Error: No such keg".into() } };
        let formula = args.last().copied().unwrap_or_default().to_string();
        match args {
            ["update", ..] if self.behind > 0 => {
                self.behind -= 1;
                text(true, String::new())
            }
            ["update", ..] => {
                for remote in std::fs::read_dir(root.join("remotes")).into_iter().flatten().flatten() {
                    let name = remote.file_name().to_string_lossy().trim_end_matches(".git").to_string();
                    let tags = git_of(root, &remote.path(), &["tag", "--sort=-v:refname"]).unwrap_or_default();
                    if let Some(tag) = tags.lines().next() {
                        self.tap.insert(name, tag.trim_start_matches('v').to_string());
                    }
                }
                text(true, String::new())
            }
            ["list", "--formula", _] => text(self.installed.contains_key(&formula), formula.clone()),
            ["list", "--versions", _] => match self.installed.get(&formula) {
                Some(v) => text(true, format!("{formula} {v}\n")),
                None => text(false, String::new()),
            },
            ["upgrade", _] if !self.installed.contains_key(&formula) => text(false, String::new()),
            ["install" | "upgrade", _] if self.refuses.is_some() => {
                Said { ok: false, out: String::new(), err: self.refuses.clone().unwrap() }
            }
            ["install" | "upgrade", _] if self.locked > 0 => {
                self.locked -= 1;
                if let Some(v) = self.tap.get(&formula).filter(|_| self.holder_installs).cloned() {
                    self.installed.insert(formula.clone(), v.clone());
                    link(root, &formula, &v, false);
                }
                let cellar = root.join("brew/Cellar").join(&formula);
                let err = format!(
                    "Error: A `brew upgrade {formula}` process has already locked {}.\nPlease wait for it to finish or terminate it to continue.",
                    cellar.display()
                );
                Said { ok: false, out: String::new(), err }
            }
            // No tap has it, as `brew update` last saw them.
            ["install", _] if !self.tap.contains_key(&formula) && !self.installed.contains_key(&formula) => {
                text(false, String::new())
            }
            ["install" | "upgrade", _] => {
                if let Some(v) = self.tap.get(&formula).filter(|_| !self.stuck) {
                    self.installed.insert(formula.clone(), v.clone());
                    link(root, &formula, v, self.stale_link);
                }
                text(true, format!("==> {} {formula}\n", args[0]))
            }
            ["--prefix", _] => text(true, format!("{}\n", root.join("brew/opt").join(&formula).display())),
            ["--prefix"] => text(true, format!("{}\n", root.join("brew").display())),
            ["--cellar", _] => text(true, format!("{}\n", root.join("brew/Cellar").join(&formula).display())),
            ["link", "--overwrite", _] => {
                if !self.relink_fails {
                    link_bin(root, &formula);
                }
                text(true, String::new())
            }
            _ => text(false, String::new()),
        }
    }
}

/// Homebrew's layout, where a scenario made one for `formula`: version `v`
/// in the cellar, its binaries copied from the version before, the opt link
/// moved to it, and PATH's links to its binaries; or, `stale`, those left
/// on the version before, which is removed.
fn link(root: &Path, formula: &str, v: &str, stale: bool) {
    let cellar = root.join("brew/Cellar").join(formula);
    let opt = root.join("brew/opt").join(formula);
    let Ok(was) = opt.canonicalize() else { return };
    std::fs::create_dir_all(cellar.join(v)).unwrap();
    Command::new("cp")
        .args(["-R", &was.join("bin").to_string_lossy(), &cellar.join(v).to_string_lossy()])
        .status()
        .unwrap();
    std::fs::remove_file(&opt).unwrap();
    std::os::unix::fs::symlink(cellar.join(v), &opt).unwrap();
    if stale {
        std::fs::remove_dir_all(was).unwrap();
    } else {
        link_bin(root, formula);
    }
}

/// Links PATH's `bin/<name>` to each binary of the keg `formula`'s opt link
/// follows, relative as Homebrew links them, over what was there.
pub fn link_bin(root: &Path, formula: &str) {
    let Ok(keg) = root.join("brew/opt").join(formula).canonicalize() else { return };
    let bin = root.join("brew/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cellar = root.join("brew/Cellar").canonicalize().unwrap();
    let kept = keg.strip_prefix(&cellar).unwrap();
    for program in std::fs::read_dir(keg.join("bin")).into_iter().flatten().flatten() {
        let link = bin.join(program.file_name());
        let _ = std::fs::remove_file(&link);
        let to = Path::new("../Cellar").join(kept).join("bin").join(program.file_name());
        std::os::unix::fs::symlink(to, link).unwrap();
    }
}

/// rolesmith as factorium runs it, `doctor --project <dir> --json` and
/// `tool install --project <dir> --json`, answering as rolesmith does: a
/// directory without `rolesmith.toml` is refused; else each thing its roles
/// lack, an install giving the programs it can.
#[derive(Default)]
pub struct FakeRolesmith {
    /// Each project's directory → what its roles lack, as `doctor` lists
    /// each under `error.missing` (`kind`, `name`, `reason`, `fix`).
    pub lacks: BTreeMap<String, Vec<Value>>,
    /// The programs an install cannot give: its script fails.
    pub uninstallable: Vec<String>,
}

impl FakeRolesmith {
    /// A program `name` that `dir`'s roles lack, as doctor names it.
    pub fn lacks_program(&mut self, dir: &str, name: &str) {
        let fix = format!("rolesmith tool install --project {dir}");
        let lack = json!({ "kind": "program", "name": name, "reason": format!("check `{name} --version` exits 127"), "fix": fix });
        self.lacks.entry(dir.into()).or_default().push(lack);
    }

    /// A mytokens entry `name` that `dir`'s roles lack, as doctor names it.
    pub fn lacks_entry(&mut self, dir: &str, name: &str) {
        let fix = format!("mytokens put {name} --fields TOKEN");
        let lack = json!({ "kind": "entry", "name": name, "reason": "no mytokens entry", "fix": fix });
        self.lacks.entry(dir.into()).or_default().push(lack);
    }

    fn answer(&mut self, args: &[&str]) -> Said {
        let refused = |message: String, extra: Option<(&str, Value)>| {
            let mut error = json!({ "kind": "failed", "message": message });
            if let Some((key, list)) = extra {
                error[key] = list;
            }
            Said { ok: false, out: json!({ "error": error }).to_string(), err: format!("rolesmith: {message}\n") }
        };
        let (verb, dir) = match args {
            ["doctor", "--project", dir, "--json"] => ("doctor", *dir),
            ["tool", "install", "--project", dir, "--json"] => ("install", *dir),
            _ => return refused(format!("no stand-in for {args:?}"), None),
        };
        if !Path::new(dir).join("rolesmith.toml").is_file() {
            return refused(format!("{dir} has no rolesmith.toml"), None);
        }
        let lacks = self.lacks.entry(dir.into()).or_default();
        let ok = |out: Value| Said { ok: true, out: out.to_string(), err: String::new() };
        match verb {
            "doctor" if lacks.is_empty() => {
                ok(json!({ "entries": [], "host": null, "programs": [], "project": dir, "roles": [] }))
            }
            "doctor" => refused(
                format!("the project lacks {} thing(s), on this machine:", lacks.len()),
                Some(("missing", json!(lacks))),
            ),
            _ => {
                // In order; the first that fails stops it, those before it installed.
                let programs = lacks.iter().filter(|l| l["kind"] == "program").map(|l| l["name"].clone());
                let names: Vec<String> = programs.map(|n| n.as_str().unwrap_or_default().to_string()).collect();
                let failing = names.iter().position(|n| self.uninstallable.contains(n));
                let installed = &names[..failing.unwrap_or(names.len())];
                lacks.retain(|l| !installed.iter().any(|n| l["name"] == n.as_str()));
                if let Some(i) = failing {
                    return refused(format!("tool {}: its script exits 1: no such toolchain", names[i]), None);
                }
                let tools: Vec<Value> =
                    installed.iter().map(|n| json!({ "name": n, "method": "script", "formula": null })).collect();
                ok(json!({ "host": null, "project": dir, "roles": [], "tools": tools }))
            }
        }
    }
}

/// stablehand as a scenario has it: its register of engine accounts and the
/// limits sessions reported, answering as stablehand does what factorium
/// asks (`pick`, `env`, `limit`, `status`, `account show`), and what a case
/// sets up with (`account add`, `account login`, `order set`). It reads no engine: an
/// account has no windows, so a pick is by name of those that can take one
/// more session.
#[derive(Default)]
pub struct FakeStablehand {
    /// Each account, by name.
    pub accounts: BTreeMap<String, FakeAccount>,
    /// Each account a session met the limit of → until when (ms); `None`:
    /// until its next reading.
    pub limits: BTreeMap<String, Option<i64>>,
    /// A fault every command answers, as stablehand does one it cannot read
    /// its books through (`fault`, exit 1).
    pub fault: Option<String>,
    /// The order `pick` walks (`order set`): each engine, and its models
    /// (none: the engine's own), highest first.
    pub order: Vec<(String, Vec<String>)>,
    /// Each engine's command kept (`command set`), as `command show` answers.
    pub commands: BTreeMap<String, String>,
    /// `limit` is refused as books it cannot write are (`limit-refused`).
    pub limit_refused: bool,
}

/// The failures the stand-in stablehand answers when asked (truefit.toml's
/// stablehand `faults`, named by `TRUEFIT_FAULT` to `standin fake`), each as
/// stablehand 0.1.17 answers it: `limit-refused`, every `limit` refused
/// (`fault`, exit 1), as one whose books it may not write.
pub const STABLEHAND_FAULTS: [&str; 1] = ["limit-refused"];

/// An account as stablehand keeps it.
#[derive(Clone, Debug, Default)]
pub struct FakeAccount {
    pub engine: String,
    /// The machine its bound login lives on; `None`: a portable login.
    pub machine: Option<String>,
    pub home: Option<String>,
    pub sessions: Option<u32>,
    pub off: bool,
    /// A portable login's: whether its token command prints a token. A
    /// bound one is logged in when its home holds its login file.
    pub logged_in: bool,
    /// A portable login's token, as its token command prints it.
    pub token: Option<String>,
}

impl FakeAccount {
    /// Whether it is logged in here: a bound login, by its login file in its
    /// home on this machine; a portable one, by its token.
    fn logged_in_here(&self) -> bool {
        match (&self.machine, &self.home, engine_vars(&self.engine)) {
            (Some(m), Some(home), Some((_, _, file))) => {
                *m == stablehand_machine() && Path::new(home).join(file).exists()
            }
            (Some(_), ..) => false,
            (None, ..) => self.logged_in && self.token.is_some(),
        }
    }
}

/// The variable each engine's bound login reads for its home, the one its
/// token goes in, and its login file in its home, as `stablehand credential
/// --help` lists them.
fn engine_vars(engine: &str) -> Option<(&'static str, Option<&'static str>, &'static str)> {
    Some(match engine {
        "claude-code" => ("CLAUDE_CONFIG_DIR", Some("CLAUDE_CODE_OAUTH_TOKEN"), ".credentials.json"),
        "codex" => ("CODEX_HOME", None, "auth.json"),
        "pi" => ("PI_CODING_AGENT_DIR", None, "auth.json"),
        "kimi" => ("KIMI_CODE_HOME", None, "credentials/kimi-code.json"),
        "agy" => ("HOME", None, ".gemini/antigravity-cli/antigravity-oauth-token"),
        "grok" => ("GROK_HOME", None, "auth.json"),
        "dsh" => ("DSH_HOME", None, ".credentials.yaml"),
        _ => return None,
    })
}

/// This machine as stablehand names it: `$STABLEHAND_MACHINE`, else
/// `hostname -s`. Read once a run: each login a stand-in checks asks it.
fn stablehand_machine() -> String {
    static MACHINE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    MACHINE
        .get_or_init(|| {
            std::env::var("STABLEHAND_MACHINE").unwrap_or_else(|_| {
                let out = Command::new("hostname").arg("-s").output().unwrap();
                String::from_utf8(out.stdout).unwrap().trim().to_string()
            })
        })
        .clone()
}

impl FakeStablehand {
    /// Sets, or clears, the failure `name` (one of [`STABLEHAND_FAULTS`]).
    pub fn fault(&mut self, name: &str, on: bool) {
        match name {
            "limit-refused" => self.limit_refused = on,
            _ => panic!("no stablehand fault {name}: {STABLEHAND_FAULTS:?}"),
        }
    }

    /// A portable account `name` of `engine`, logged in with `token`.
    pub fn portable(&mut self, name: &str, engine: &str, token: &str) {
        let sessions = (engine == "claude-code").then_some(2);
        let account = FakeAccount {
            engine: engine.into(),
            sessions,
            logged_in: true,
            token: Some(token.into()),
            ..FakeAccount::default()
        };
        self.accounts.insert(name.into(), account);
    }

    /// An account `name` of `engine` whose login is bound to `machine`,
    /// its home `home`, logged in there.
    pub fn bound(&mut self, name: &str, engine: &str, machine: &str, home: &Path) {
        // Logged in: its login file is in its home, where it can be.
        if let Some((_, _, file)) = engine_vars(engine) {
            let login = home.join(file);
            let _ = login.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(login, "{}");
        }
        let home = Some(home.display().to_string());
        let sessions = (engine == "claude-code").then_some(2);
        let account = FakeAccount {
            engine: engine.into(),
            machine: Some(machine.into()),
            home,
            sessions,
            ..FakeAccount::default()
        };
        self.accounts.insert(name.into(), account);
    }

    /// The order `pick` walks, as `order set` takes it: each
    /// `<engine>[@<model>[,<model>…]]`, highest first.
    pub fn order(&mut self, ranks: &[&str]) {
        self.order = ranks
            .iter()
            .map(|r| match r.split_once('@') {
                Some((e, models)) => (e.to_string(), models.split(',').map(String::from).collect()),
                None => (r.to_string(), Vec::new()),
            })
            .collect();
    }

    /// An account named `engine`, of that engine, logged in, taking any
    /// number of sessions: bound to this machine, its home `home`.
    /// Claude Code's is portable, its token `sk-own`: its sessions keep
    /// the host user's home.
    pub fn own(&mut self, engine: &str, home: &Path) {
        match engine {
            "claude-code" => self.portable(engine, engine, "sk-own"),
            _ => self.bound(engine, engine, &stablehand_machine(), home),
        }
        self.accounts.get_mut(engine).unwrap().sessions = None;
    }

    /// A reading of `name`: a limit that waited for one lifts.
    pub fn read(&mut self, name: &str) {
        if self.limits.get(name) == Some(&None) {
            self.limits.remove(name);
        }
    }

    pub fn answer(&mut self, args: &[&str], now: i64) -> Said {
        if let Some(fault) = &self.fault {
            return Said { ok: false, out: json!({ "error": fault, "code": "fault" }).to_string(), err: String::new() };
        }
        let ok = |out: Value| Said { ok: true, out: out.to_string(), err: String::new() };
        let refused = |code: &str, error: String| Said {
            ok: false,
            out: json!({ "error": error, "code": code }).to_string(),
            err: String::new(),
        };
        let no_account = |name: &str| {
            refused("no_account", format!("no account `{name}`: `stablehand account list` names those kept"))
        };
        let flag = |name: &str| args.iter().position(|a| *a == name).and_then(|i| args.get(i + 1)).copied();
        match args {
            ["account", "add", name, ..] => {
                let Some(engine) = flag("--engine").filter(|e| engine_vars(e).is_some()) else {
                    return refused("usage", "an engine stablehand knows".into());
                };
                if self.accounts.contains_key(*name) {
                    return refused("name_taken", format!("an account `{name}` is kept already"));
                }
                let sessions =
                    flag("--sessions").and_then(|s| s.parse().ok()).or((engine == "claude-code").then_some(2));
                let (machine, home) = (flag("--machine").map(String::from), flag("--home").map(String::from));
                let account = FakeAccount { engine: engine.into(), machine, home, sessions, ..FakeAccount::default() };
                self.accounts.insert(name.to_string(), account);
                ok(json!({ "account": self.shown(name) }))
            }
            ["account", "show", name] => match self.accounts.contains_key(*name) {
                true => ok(self.show(name)),
                false => no_account(name),
            },
            ["credential", name, "--json"] => {
                let Some(a) = self.accounts.get(*name) else { return no_account(name) };
                let Some((_, token_var, login_file)) = engine_vars(&a.engine) else {
                    return refused("fault", "no engine".into());
                };
                let no_session = |why: String| Said { ok: false, ..refused("no_session", why) };
                if a.off || !a.logged_in_here() {
                    return no_session(format!("account `{name}` is not logged in"));
                }
                match (&a.machine, &a.home, token_var, &a.token) {
                    (Some(m), _, _, _) if *m != stablehand_machine() => {
                        no_session(format!("account `{name}` is bound to {m}"))
                    }
                    (Some(_), Some(home), _, _) => {
                        let path = Path::new(home).join(login_file);
                        if !path.exists() {
                            return no_session(format!("account `{name}` is not logged in: no {}", path.display()));
                        }
                        let files = json!([{ "path": path.display().to_string(), "as": login_file }]);
                        ok(json!({ "name": name, "login": "bound", "files": files }))
                    }
                    (Some(_), None, _, _) => no_session(format!("account `{name}` has no home")),
                    (None, _, Some(var), Some(token)) => {
                        ok(json!({ "name": name, "login": "portable", "env": { var: token } }))
                    }
                    (None, ..) => no_session(format!("account `{name}` is not logged in")),
                }
            }
            ["limit", name, ..] => {
                if !self.accounts.contains_key(*name) {
                    return no_account(name);
                }
                if self.limit_refused {
                    return refused("fault", "stablehand.db: Permission denied (os error 13)".into());
                }
                let resets = match flag("--resets-at") {
                    Some(t) => match factorium_domain::when::parse(t) {
                        Ok(ms) if ms > now => Some(ms),
                        _ => return refused("bad_limit", format!("`{t}` is no RFC 3339 time to come")),
                    },
                    None => None,
                };
                self.limits.insert(name.to_string(), resets);
                let at = factorium_domain::when::utc(now);
                let window = flag("--window");
                ok(
                    json!({ "limit": { "account": name, "at": at, "window": window, "resets_at": resets.map(factorium_domain::when::utc) } }),
                )
            }
            ["command", "show"] => ok(json!({ "commands": self.commands })),
            ["command", "set", given @ ..] => {
                let mut commands = self.commands.clone();
                for g in given {
                    let Some((engine, command)) = g.split_once('=') else {
                        return refused("bad_command", format!("{g}: <engine>=<command>"));
                    };
                    if engine_vars(engine).is_none() {
                        return refused("bad_command", format!("{engine}: no such engine"));
                    }
                    // Its program is not checked here: a scenario's stand-in is a path of its own.
                    if command.trim().is_empty() {
                        commands.remove(engine);
                    } else if command.split_whitespace().any(|w| w == "--model") {
                        return refused("bad_command", format!("{engine}: `{command}` names a model"));
                    } else {
                        commands.insert(engine.to_string(), command.to_string());
                    }
                }
                self.commands = commands;
                ok(json!({ "commands": self.commands }))
            }
            ["order", "set", ranks @ ..] => {
                self.order(ranks);
                ok(self.order_shown())
            }
            ["order", "show"] => ok(self.order_shown()),
            ["pick", ..] => {
                let engines: Vec<&str> = args.windows(2).filter(|w| w[0] == "--engine").map(|w| w[1]).collect();
                self.pick(&engines, flag("--model"), flag("--machine"), flag("--running"), now)
            }
            ["status"] | ["status", _] => {
                let only = args.get(1);
                let accounts: Vec<Value> = (self.accounts.iter())
                    .filter(|(n, _)| only.is_none_or(|o| o == n))
                    .map(|(n, a)| {
                        let limit = self.limits.get(n).filter(|l| l.is_none_or(|t| t > now));
                        let limit = limit.map(|l| json!({ "resets_at": l.map(factorium_domain::when::utc), "window": null }));
                        let pace = if limit.is_some() { "used_up" } else { "steady" };
                        json!({ "name": n, "engine": a.engine, "off": a.off, "pace": pace, "limit": limit, "windows": [], "balances": [] })
                    })
                    .collect();
                ok(json!({ "accounts": accounts }))
            }
            _ => refused("usage", format!("no stand-in for {args:?}")),
        }
    }

    /// `account show`'s `account` of `name`.
    fn shown(&self, name: &str) -> Value {
        let a = &self.accounts[name];
        let login = if a.machine.is_some() { "bound" } else { "portable" };
        json!({ "name": name, "engine": a.engine, "login": login, "machine": a.machine, "home": a.home, "sessions": a.sessions, "off": a.off.then_some("off") })
    }

    /// What `account show` answers of `name`.
    fn show(&self, name: &str) -> Value {
        json!({ "account": self.shown(name), "logged_in": self.accounts[name].logged_in_here() })
    }

    /// What `order show` answers.
    fn order_shown(&self) -> Value {
        let order: Vec<Value> = self.order.iter().map(|(e, m)| json!({ "engine": e, "models": m })).collect();
        json!({ "order": order })
    }

    /// The (engine, model) steps a pick walks: the order's, narrowed to
    /// `engines` and `model` where named, those named and not in the order
    /// after the rest; `None`: nothing to walk.
    fn steps(&self, engines: &[&str], model: Option<&str>) -> Option<Vec<(String, Option<String>)>> {
        if engines.is_empty() && self.order.is_empty() {
            return None;
        }
        let mut steps = Vec::new();
        let named = |e: &str| engines.is_empty() || engines.contains(&e);
        for (engine, models) in self.order.iter().filter(|(e, _)| named(e)) {
            match model {
                Some(m) if models.iter().any(|x| x == m) => steps.push((engine.clone(), Some(m.to_string()))),
                Some(_) => {}
                None if models.is_empty() => steps.push((engine.clone(), None)),
                None => steps.extend(models.iter().map(|m| (engine.clone(), Some(m.clone())))),
            }
        }
        let rest: Vec<String> = match engines.is_empty() {
            true => self.order.iter().map(|(e, _)| e.clone()).collect(),
            false => engines.iter().map(|e| e.to_string()).collect(),
        };
        for step in rest.into_iter().map(|e| (e, model.map(String::from))) {
            if !steps.iter().any(|s| *s == step || (model.is_none() && s.0 == step.0)) {
                steps.push(step);
            }
        }
        Some(steps)
    }

    /// `pick [--engine <engine>…] [--model <model>] --machine <machine> --running <running>`.
    fn pick(
        &self,
        engines: &[&str],
        model: Option<&str>,
        machine: Option<&str>,
        running: Option<&str>,
        now: i64,
    ) -> Said {
        let refused = |code: &str, error: String| Said {
            ok: false,
            out: json!({ "error": error, "code": code }).to_string(),
            err: String::new(),
        };
        if let Some(e) = engines.iter().find(|e| engine_vars(e).is_none()) {
            return refused("usage", format!("no engine `{e}`"));
        }
        let Some(steps) = self.steps(engines, model) else {
            return refused("no_order", "nothing to walk: no order is kept and no --engine named".into());
        };
        let machine = machine.map_or_else(stablehand_machine, String::from);
        let mut counts = BTreeMap::new();
        for pair in running.into_iter().flat_map(|r| r.split(',')).filter(|p| !p.is_empty()) {
            let (name, n) = pair.split_once('=').unwrap_or((pair, "0"));
            if !self.accounts.contains_key(name) {
                return refused(
                    "no_account",
                    format!("no account `{name}`: `stablehand account list` names those kept"),
                );
            }
            counts.insert(name, n.parse::<u32>().unwrap_or(0));
        }
        let mut why = Vec::new();
        for (engine, model) in steps {
            let of: Vec<_> = self.accounts.iter().filter(|(_, a)| a.engine == engine).collect();
            if of.is_empty() {
                let said = format!(
                    "no {engine} account is kept: skipped; add one with `stablehand account add <name> --engine {engine}`, \
                     or `stablehand account scan --add`"
                );
                why.push(json!({ "engine": engine, "model": model, "account": null, "reason": "no_account", "why": said, "until": null }));
                continue;
            }
            for (name, a) in of {
                let limit = self.limits.get(name).copied().filter(|l| l.is_none_or(|t| t > now));
                let (reason, until) = if a.off {
                    ("off", None)
                } else if a.machine.as_ref().is_some_and(|m| *m != machine) {
                    ("elsewhere", None)
                } else if !a.logged_in_here() {
                    ("not_logged_in", None)
                } else if let Some(until) = limit {
                    ("limit", until)
                } else if a.sessions.is_some_and(|s| counts.get(name.as_str()).copied().unwrap_or(0) >= s) {
                    ("full", None)
                } else {
                    let out = json!({ "engine": engine, "model": model, "account": name, "why": why, "model_in_no_group": [] });
                    return Said { ok: true, out: out.to_string(), err: String::new() };
                };
                let until = until.map(factorium_domain::when::utc);
                why.push(json!({ "engine": engine, "model": model, "account": name, "reason": reason, "why": format!("{name}: {reason}"), "until": until }));
            }
        }
        let until = why.iter().filter_map(|u| u["until"].as_str().map(String::from)).min();
        let out = json!({ "none": true, "until": until, "why": why, "model_in_no_group": [] });
        Said { ok: true, out: out.to_string(), err: String::new() }
    }
}

/// The machine as a scenario has it.
pub struct FakeHost {
    pub orca: FakeOrca,
    pub brew: Mutex<FakeBrew>,
    pub reading: Mutex<Reading>,
    pub procs: Mutex<Vec<Proc>>,
    /// Runs each time the processes are read, as every tick does.
    pub reading_procs: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// stablehand: its accounts and the limits reported.
    pub stablehand: Mutex<FakeStablehand>,
    /// slimfit: its whole command line, else its subcommand → what it prints
    /// (`help picker`, `help reviewer`, `check`).
    pub slimfit: Mutex<BTreeMap<String, String>>,
    /// rolesmith: what each project's roles lack of this host.
    pub rolesmith: Mutex<FakeRolesmith>,
    /// Programs not found on PATH.
    pub missing: Mutex<Vec<String>>,
    /// ssh: each host it reaches → the environment a command there runs
    /// in, as sshd gives it (`SHELL`, the user's login shell; `PATH`, the
    /// one a shell that is not a login one has).
    pub hosts: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    /// The agents launchd has loaded, as `<domain>/<uid>/<label>`.
    pub service: Mutex<Vec<String>>,
    /// How many prints after its bootout still find an agent, as launchd
    /// still holds a running one a moment after bootout returns.
    pub lingers: Mutex<usize>,
    /// Agents booted out that launchd still holds: prints left that find them.
    pub dropping: Mutex<Vec<(String, usize)>>,
    /// Every program run, as `program args…`, oldest first.
    pub ran: Mutex<Vec<String>>,
    /// Runs, once, on the next `sleep`: what happens while the app waits.
    pub while_sleeping: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// While set, `sleep` takes the seconds it is given; otherwise none.
    pub sleeps_take: Mutex<bool>,
    /// While set, this process reads as gone too: what a later process sees
    /// of one of its runs that died.
    pub gone: Mutex<bool>,
    /// The app's one clock: only the scenario moves it.
    pub clock: Arc<ManualClock>,
    pub(super) root: PathBuf,
}

impl FakeHost {
    /// The machine of the world at `root`, its home made, its clock and its
    /// reading as given, nothing else on it yet.
    pub fn at(root: &Path, clock: Arc<ManualClock>, reading: Reading) -> FakeHost {
        make_home(root);
        let (orca, brew, procs, reading_procs) = Default::default();
        let (stablehand, slimfit, missing, service, ran, while_sleeping, gone) = Default::default();
        let sleeps_take = Default::default();
        let (lingers, dropping, hosts, rolesmith) = Default::default();
        let reading = Mutex::new(reading);
        FakeHost {
            orca,
            brew,
            reading,
            procs,
            reading_procs,
            stablehand,
            slimfit,
            rolesmith,
            missing,
            hosts,
            service,
            lingers,
            dropping,
            ran,
            while_sleeping,
            sleeps_take,
            gone,
            clock,
            root: root.to_path_buf(),
        }
    }

    /// Whether launchd has factorium's agent loaded in this user's domain.
    pub fn loaded(&self) -> bool {
        let target = self.service_target("user");
        self.service.lock().unwrap().contains(&target)
    }

    /// factorium's agent's name in this user's `domain` (`user`, `gui`).
    pub fn service_target(&self, domain: &str) -> String {
        use std::os::unix::fs::MetadataExt;
        format!("{domain}/{}/com.github.sunfmin.factorium", std::fs::metadata(&self.root).unwrap().uid())
    }

    /// The `brew` commands run, oldest first.
    pub fn brewed(&self) -> Vec<String> {
        self.ran.lock().unwrap().iter().filter_map(|r| r.strip_prefix("brew ")).map(str::to_string).collect()
    }
}

impl Programs for FakeHost {
    fn run(&self, program: &str, args: &[&str]) -> Said {
        self.ran.lock().unwrap().push(format!("{program} {}", args.join(" ")));
        if !self.found(program) {
            return Said { ok: false, out: String::new(), err: format!("{program}: not found") };
        }
        match program {
            "orca" => self.orca.answer(&self.root, args),
            "brew" => self.brew.lock().unwrap().answer(&self.root, args),
            "stablehand" => self.stablehand.lock().unwrap().answer(args, self.clock.now()),
            "slimfit" => {
                let said = self.slimfit.lock().unwrap();
                // A refusal is printed as slimfit prints one: `{"error": …}`, exit 2.
                let out = said.get(&args.join(" ")).or_else(|| said.get(args[0])).cloned();
                let ok = out.as_ref().is_some_and(|o| !o.starts_with(r#"{"error""#));
                Said { ok, out: out.unwrap_or_default(), err: "slimfit: no such answer".into() }
            }
            "ssh" => self.ssh(args),
            "rolesmith" => self.rolesmith.lock().unwrap().answer(args),
            "ps" => Said { ok: true, out: self.brew.lock().unwrap().ps.clone(), err: String::new() },
            "sleep" => {
                if let Some(then) = self.while_sleeping.lock().unwrap().take() {
                    then();
                }
                if *self.sleeps_take.lock().unwrap() {
                    let secs = args.first().and_then(|a| a.parse().ok()).unwrap_or(0);
                    std::thread::sleep(std::time::Duration::from_secs(secs));
                }
                Said { ok: true, ..Default::default() }
            }
            "launchctl" => {
                // As launchd: this user's domains, an agent by the label its
                // file names; `user/<uid>` takes only a Background job.
                use std::os::unix::fs::MetadataExt;
                let uid = std::fs::metadata(&self.root).unwrap().uid();
                let mut loaded = self.service.lock().unwrap();
                let mut dropping = self.dropping.lock().unwrap();
                let held = |t: &str, dropping: &[(String, usize)]| dropping.iter().any(|(x, _)| x == t);
                let ok = match args {
                    ["print", t] => {
                        let found = loaded.iter().any(|x| x == t) || held(t, &dropping);
                        if let Some(i) = dropping.iter().position(|(x, _)| x == t) {
                            dropping[i].1 -= 1;
                            if dropping[i].1 == 0 {
                                dropping.remove(i);
                            }
                        }
                        found
                    }
                    ["bootstrap", d, file] if *d == format!("user/{uid}") && background(file) => {
                        match label_of(file).map(|l| format!("{d}/{l}")) {
                            Some(t) if !loaded.contains(&t) && !held(&t, &dropping) => {
                                loaded.push(t);
                                true
                            }
                            _ => false,
                        }
                    }
                    ["bootout", t] => match loaded.iter().position(|x| x == t) {
                        Some(i) => {
                            let t = loaded.remove(i);
                            let lingers = *self.lingers.lock().unwrap();
                            if lingers > 0 {
                                dropping.push((t, lingers));
                            }
                            true
                        }
                        None => false,
                    },
                    _ => false,
                };
                Said { ok, out: String::new(), err: if ok { String::new() } else { "Input/output error".into() } }
            }
            _ => Said { ok: false, out: String::new(), err: format!("{program}: not found") },
        }
    }

    fn found(&self, program: &str) -> bool {
        !self.missing.lock().unwrap().iter().any(|m| m == program)
    }
}

impl FakeHost {
    /// ssh as factorium runs it, `ssh -o BatchMode=yes <target> <command>`:
    /// on a host it knows, `<command>` runs as sshd runs one, with `sh -c`
    /// in that host's environment alone (and a test build's coverage file),
    /// from its home, and answers as it exits; any other is not reached
    /// (exit 255), as one whose name does not resolve.
    fn ssh(&self, args: &[&str]) -> Said {
        let ["-o", "BatchMode=yes", target, command] = args else {
            return Said { ok: false, err: format!("ssh: no stand-in for {args:?}"), ..Said::default() };
        };
        let Some(env) = self.hosts.lock().unwrap().get(*target).cloned() else {
            let err =
                format!("ssh: Could not resolve hostname {target}: nodename nor servname provided, or not known\n");
            return Said { ok: false, out: String::new(), err };
        };
        let mut sh = Command::new("/bin/sh");
        sh.arg("-c").arg(command).env_clear().current_dir(env.get("HOME").map_or("/", String::as_str));
        let profile = std::env::var_os("LLVM_PROFILE_FILE").map(|p| ("LLVM_PROFILE_FILE", p));
        let out = sh.envs(env).envs(profile).output().unwrap();
        Said {
            ok: out.status.success(),
            out: String::from_utf8_lossy(&out.stdout).into_owned(),
            err: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

/// Whether the agent's file at `file` is a job of the Background session.
fn background(file: &str) -> bool {
    let plist = std::fs::read_to_string(file).unwrap_or_default();
    plist.contains("<key>LimitLoadToSessionType</key>\n\t<string>Background</string>")
}

/// The label a launchd agent's file at `file` names.
fn label_of(file: &str) -> Option<String> {
    let plist = std::fs::read_to_string(file).ok()?;
    let (_, after) = plist.split_once("<key>Label</key>")?;
    let (_, rest) = after.split_once("<string>")?;
    rest.split_once("</string>").map(|(l, _)| l.to_string())
}

impl Host for FakeHost {
    fn programs(&self) -> &dyn Programs {
        self
    }
    fn reading(&self) -> Reading {
        *self.reading.lock().unwrap()
    }
    fn processes(&self) -> Vec<Proc> {
        if let Some(during) = &*self.reading_procs.lock().unwrap() {
            during();
        }
        self.procs.lock().unwrap().clone()
    }
    fn alive(&self, pid: u32) -> bool {
        pid == std::process::id() && !*self.gone.lock().unwrap()
    }
    fn home(&self) -> PathBuf {
        home(&self.root)
    }
    fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }
}

/// The home of the world at `root`: as a fresh host's, with no identity.
pub(super) fn home(root: &Path) -> PathBuf {
    root.join("home")
}

/// Makes the home of the world at `root`. Its git config turns off git's
/// automatic maintenance, which each fetch, commit and merge would otherwise
/// start as one more process, left running detached (factorium#98), and
/// lets git reach only local paths: a scenario's origin that names a host
/// (`git@example.com:…`) is refused at once, as one that cannot be
/// reached, instead of waiting out ssh's connect timeout (458s of one
/// scenario's wall time, measured 2026-11-26).
pub(super) fn make_home(root: &Path) {
    std::fs::create_dir_all(home(root)).unwrap();
    let config = "[maintenance]\n\tauto = false\n[protocol]\n\tallow = never\n[protocol \"file\"]\n\tallow = always\n";
    std::fs::write(home(root).join(".gitconfig"), config).unwrap();
}

/// git in `dir` as the host of the world at `root` runs it: with its home,
/// which holds no config — a fresh host, with no identity of its own.
pub(super) fn git_of(root: &Path, dir: &Path, args: &[&str]) -> Result<String, String> {
    Git::new(home(root), dir).run(args)
}
