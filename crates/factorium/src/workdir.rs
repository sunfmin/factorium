//! The work dirs of the line and idle work (docs/adr/0016-the-line-s-jobs-run-in-work-dirs.md). Each job
//! (a landing's merge and fast gate, a promotion's slow gate, a cut's steps,
//! a run of idle work) runs in an Orca worktree of its checkout of its own:
//! named for the job, commented with what runs there, detached at the rev
//! the job reads, and removed when the job ends, green or red. A worktree of
//! the checkout sees its tags and `origin/*` refs as the checkout has them.
//!
//! The build cache is the one thing kept across jobs, outside the worktree:
//! one per checkout and kind under the data directory, given to the job's
//! steps as `CARGO_TARGET_DIR`, and the dirs `factorium.toml` declares
//! `cache` moved in at the start and out at the end.
//!
//! A work dir whose job no longer runs (its process gone, or the daemon's
//! own that no tick asks for) is removed by the daemon's next tick; one that
//! holds commits found nowhere else is kept, and doctor names it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use factorium_domain::checkout::Checkout;
use factorium_domain::declared;
use factorium_host::{Git, Listed};
use serde_json::{Value, json};

use crate::{Answer, App, LineDir, Refusal, fail};

/// How the comment of every work dir factorium makes ends, before the pid
/// of the process that made it: how factorium knows its own.
const MARK: &str = "factorium removes it when this ends (pid ";

/// How the comment of a left-over work dir that holds commits found nowhere
/// else ends, once the sweep kept it: no sweep removes it, and doctor names it.
const KEPT: &str = "Its job is gone; factorium kept it, as it holds commits found nowhere else.";

/// The work dirs this process made for jobs of an app that does not tick
/// (a command by hand, waiting for its job), until each job ends: no sweep
/// of a daemon in the same process removes them.
static HELD: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// The subject of a landing's merge, the one commit a work dir holds that
/// is factorium's own.
const LANDING: &str = "Land ";

/// A work dir factorium made, as Orca lists it.
#[derive(Clone, Debug)]
pub(crate) struct Made {
    pub(crate) kind: LineDir,
    pub(crate) checkout: String,
    pub(crate) name: String,
    /// The process that made it, for its job; none once the sweep kept it.
    pub(crate) pid: Option<u32>,
    pub(crate) path: PathBuf,
    comment: String,
}

impl Made {
    /// The work dir `l` is, if factorium made it: named `<kind> <checkout>…`,
    /// its comment ending with the mark and the pid that made it, or kept.
    fn of(l: &Listed) -> Option<Made> {
        let pid = match l.comment.strip_suffix(KEPT) {
            Some(_) => None,
            None => Some(l.comment.rsplit_once(MARK)?.1.strip_suffix(").")?.parse().ok()?),
        };
        let (word, rest) = l.name.split_once(' ')?;
        let kind = LineDir::ALL.into_iter().find(|k| k.name() == word)?;
        let checkout = rest.split([' ', '#']).next()?.to_string();
        Some(Made { kind, checkout, name: l.name.clone(), pid, path: l.path.clone(), comment: l.comment.clone() })
    }
}

/// What a job's work dir is named and commented: `<kind> <label>`, where
/// the label starts with the checkout's name; what runs there, and who
/// removes it when.
fn named(kind: LineDir, label: &str, about: &str) -> (String, String) {
    (format!("{} {label}", kind.name()), format!("{about} {MARK}{}).", std::process::id()))
}

impl App {
    /// The work dir of `c`'s job of `kind`, `label` naming it after the kind
    /// (it starts with the checkout's name) and `about` saying what runs
    /// there: an Orca worktree of the checkout detached at `rev`, the
    /// declared caches moved in. One of the same name this process made is
    /// reused, reset to `rev`: a job waiting its turn asks each tick.
    pub(crate) fn work_dir(&self, c: &Checkout, kind: LineDir, label: &str, about: &str, rev: &str) -> Answer<Git> {
        let (name, comment) = named(kind, label, about);
        let orca = self.host.orca();
        let listed = orca.worktrees().map_err(fail("orca"))?;
        let mine = listed.iter().filter_map(Made::of).find(|m| m.name == name && m.pid == Some(std::process::id()));
        let path = match mine.filter(|m| m.path.is_dir()) {
            Some(m) => m.path,
            None => {
                let branch = format!("factorium-line/{}-{}", kind.name(), c.name);
                orca.worktree(Path::new(&c.path), &name, &branch, rev, &comment).map_err(fail("orca"))?
            }
        };
        let here = self.host.git(&c.path).at(&path);
        here.detached(rev).map_err(fail("git"))?;
        let cache = self.data().cache(kind, &c.name);
        for dir in declared_caches(&path) {
            let (kept, there) = (cache.join("dirs").join(&dir), path.join(&dir));
            if kept.exists() && !there.exists() {
                let _ = std::fs::create_dir_all(there.parent().unwrap_or(&path));
                let _ = std::fs::rename(&kept, &there);
            }
        }
        let mut m = self.memory.lock().unwrap();
        if m.ticks {
            m.work_dirs.insert(path);
        } else {
            HELD.lock().unwrap().insert(path);
        }
        Ok(here)
    }

    /// The environment the steps of `checkout`'s job of `kind` run with: its
    /// build cache, kept across its work dirs. Cargo's freshness by content,
    /// not by mtime, as every work dir is checked out anew (`[unstable]`,
    /// which a stable cargo ignores).
    pub(crate) fn cache_env(&self, kind: LineDir, checkout: &str) -> Vec<(String, String)> {
        let target = self.data().cache(kind, checkout).join("target");
        vec![
            ("CARGO_TARGET_DIR".into(), target.to_string_lossy().into_owned()),
            ("CARGO_UNSTABLE_CHECKSUM_FRESHNESS".into(), "true".into()),
        ]
    }

    /// Ends the work dir at `dir` of `checkout`'s job of `kind`: the declared
    /// caches moved back out, the worktree removed through Orca. Answers why
    /// it is not removed, if it is not.
    pub(crate) fn work_dir_done(&self, kind: LineDir, checkout: &str, dir: &Path) -> Result<(), String> {
        self.memory.lock().unwrap().work_dirs.remove(dir);
        HELD.lock().unwrap().remove(dir);
        let cache = self.data().cache(kind, checkout).join("dirs");
        for d in declared_caches(dir) {
            let (there, kept) = (dir.join(&d), cache.join(&d));
            if there.exists() {
                let _ = std::fs::remove_dir_all(&kept);
                let _ = std::fs::create_dir_all(kept.parent().unwrap_or(&cache));
                let _ = std::fs::rename(&there, &kept);
            }
        }
        self.host.orca().remove(dir)
    }

    /// The work dirs factorium made, as Orca lists them now.
    pub(crate) fn work_dirs(&self) -> Result<Vec<Made>, String> {
        Ok(self.host.orca().worktrees()?.iter().filter_map(Made::of).collect())
    }

    /// The work dirs the sweep kept, as doctor names them: each with the
    /// commits it holds found nowhere else.
    pub(crate) fn work_dirs_kept(&self) -> Vec<factorium_domain::doctor::KeptDir> {
        let dirs = self.work_dirs().unwrap_or_default().into_iter().filter(|m| m.pid.is_none());
        let kept = |m: Made| factorium_domain::doctor::KeptDir {
            commits: self.work_dir_holds(&m),
            name: m.name,
            path: m.path.display().to_string(),
        };
        dirs.map(kept).collect()
    }

    /// The commits of the work dir `m` found nowhere else: other than a
    /// landing's merge, none a job makes. Unread, none.
    pub(crate) fn work_dir_holds(&self, m: &Made) -> Vec<String> {
        let own = self.host.git(&m.path).own_commits().unwrap_or_default();
        own.into_iter().filter(|c| !c.split_once(' ').is_some_and(|(_, s)| s.starts_with(LANDING))).collect()
    }

    /// The daemon's sweep, once a tick: each work dir factorium made whose
    /// job no longer runs is removed; one that holds commits found nowhere
    /// else is kept, its comment saying so, for doctor to name. A job runs
    /// while the process that made it does; the daemon's own, while it runs
    /// beside the ticks or this tick asked for its dir. Answers what it
    /// removed or kept, and why it failed to.
    pub(crate) fn sweep_work_dirs(&self) -> Vec<Value> {
        let used: BTreeSet<PathBuf> = {
            let mut m = self.memory.lock().unwrap();
            let mut used = std::mem::take(&mut m.work_dirs);
            used.extend(m.beside.iter().map(|b| b.dir().to_path_buf()));
            used.extend(HELD.lock().unwrap().iter().cloned());
            used
        };
        let dirs = match self.work_dirs() {
            Ok(dirs) => dirs,
            Err(e) => return vec![json!({ "work_dirs": e })],
        };
        let me = std::process::id();
        let mut did = Vec::new();
        for m in dirs {
            let runs = match m.pid {
                None => true, // kept: the owner's now
                Some(pid) if pid == me => used.contains(&m.path),
                Some(pid) => self.host.alive(pid),
            };
            if runs {
                continue;
            }
            let path = m.path.display().to_string();
            if !self.work_dir_holds(&m).is_empty() {
                let about = m.comment.rsplit_once(MARK).map_or(m.comment.as_str(), |(about, _)| about.trim_end());
                let kept = match self.host.orca().comment(&m.path, &format!("{about} {KEPT}")) {
                    Ok(()) => {
                        json!({ "work_dir": m.name, "path": path, "kept": "it holds commits found nowhere else" })
                    }
                    Err(e) => json!({ "work_dir": m.name, "path": path, "error": e }),
                };
                did.push(kept);
                continue;
            }
            did.push(match self.work_dir_done(m.kind, &m.checkout, &m.path) {
                Ok(()) => json!({ "work_dir": m.name, "path": path, "removed": true }),
                Err(e) => json!({ "work_dir": m.name, "path": path, "error": e }),
            });
        }
        did
    }

    /// Once, as the daemon starts: the clones the line's jobs ran in before
    /// they ran in work dirs, their `target` kept as the cache, the rest
    /// removed (beside the ticks: they are large).
    pub(crate) fn retire_clones(&self) -> Answer<()> {
        let io = |e: std::io::Error| Refusal::new("io", e.to_string());
        let trash = self.data().caches().join(".retired");
        for kind in LineDir::ALL {
            let Ok(dirs) = std::fs::read_dir(self.data().line(kind)) else { continue };
            for d in dirs.flatten().filter(|d| d.path().join(".git").is_dir()) {
                let checkout = d.file_name().to_string_lossy().into_owned();
                let (target, cache) = (d.path().join("target"), self.data().cache(kind, &checkout).join("target"));
                std::fs::create_dir_all(cache.parent().unwrap_or(&cache)).map_err(io)?;
                if target.is_dir() && !cache.exists() {
                    std::fs::rename(&target, &cache).map_err(io)?;
                }
                std::fs::create_dir_all(&trash).map_err(io)?;
                std::fs::rename(d.path(), trash.join(format!("{}-{checkout}", kind.name()))).map_err(io)?;
            }
        }
        if trash.exists() {
            std::thread::spawn(move || std::fs::remove_dir_all(trash));
        }
        Ok(())
    }
}

/// The dirs the `factorium.toml` in `dir` declares `cache`; none unread.
fn declared_caches(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join(declared::FILE)).ok();
    let cache = declared::read(text.as_deref()).map(|d| d.cache).unwrap_or_default();
    cache.into_iter().filter(|d| !d.is_empty() && !Path::new(d).is_absolute() && !d.contains("..")).collect()
}
