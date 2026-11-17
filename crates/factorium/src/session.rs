//! Watching a worker's session from outside: its terminal's screen, its
//! session files (the session file is the turn), and its processes' load.
//! What goes wrong goes to the watch; a used-up account is noted and the
//! issue is started again on another.

use std::path::{Path, PathBuf};

use factorium_domain::hot;
use factorium_domain::kit::{AccountNote, LimitAccount};
use factorium_domain::settings::{Settings, Time};
use factorium_domain::worker::{self as w, Status, Workers};
use factorium_harness::turn::{self, Turn};
use factorium_harness::{Harness, SessionLayout};
use factum::{Uuid, Value};
use serde_json::json;

use crate::{Answer, App};

/// How long a used-up account is left alone when its screen gave no time.
const QUOTA_REST_MS: i64 = 60 * 60 * 1000;

fn files_under(dir: &Path, ext: &str, deep: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if deep {
                files_under(&p, ext, deep, out);
            }
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

/// A session file's text; a zstd file decompressed.
fn read(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if path.extension().is_some_and(|e| e == "zstd" || e == "zst") {
        let mut text = String::new();
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(&bytes[..]).ok()?;
        std::io::Read::read_to_string(&mut decoder, &mut text).ok()?;
        return Some(text);
    }
    String::from_utf8(bytes).ok()
}

/// The turn a harness's session files record for the worktree `cwd`, read
/// under `root` (the harness's directory). `None`: no session says.
pub(crate) fn turn_of(harness: Harness, root: &Path, cwd: &str, brief: &str) -> Option<Turn> {
    let place = harness.session_place(cwd);
    let mut files = Vec::new();
    for layout in std::iter::once(&place.layout).chain(place.also.as_ref()) {
        match layout {
            SessionLayout::Files { dir, extension } => files_under(&root.join(dir), extension, false, &mut files),
            SessionLayout::Tree { dir, extension } => files_under(&root.join(dir), extension, true, &mut files),
            SessionLayout::Named { dir, name, .. } => {
                for d in std::fs::read_dir(root.join(dir)).into_iter().flatten().flatten() {
                    files.push(d.path().join(name));
                }
            }
            SessionLayout::Index { file } => {
                let index = std::fs::read_to_string(root.join(file)).unwrap_or_default();
                for i in harness.indexed_sessions(cwd, &index) {
                    let p = PathBuf::from(&i.file);
                    files.push(if p.is_absolute() { p } else { root.join(p) });
                }
            }
        }
    }
    let turns: Vec<Turn> = files
        .iter()
        .filter_map(|f| read(f))
        .filter(|text| harness.could_record(cwd, text, Some(brief)))
        .filter_map(|text| harness.recorded_turn(cwd, &text, None, Some(brief)))
        .collect();
    turn::prefer(&turns)
}

impl App {
    /// Where a harness keeps its sessions for factorium's sessions: the
    /// user's own directory for a harness left on it, else the kept one.
    pub(crate) fn session_root(&self, harness: Harness) -> PathBuf {
        match harness {
            Harness::ClaudeCode => self.host.home().join(harness.session_place("").under_home),
            _ => self.dir(&["kept", harness.key()]),
        }
    }

    /// Looks at one live worker; answers what it found, if anything.
    pub(crate) async fn observe(&self, worker: Uuid, load: &hot::Load, settings: &Settings) -> Answer<Option<Value>> {
        let wk = self.state::<Workers>(worker).await?;
        let Some(st) = wk.started.clone() else { return Ok(None) };
        if matches!(wk.status, Status::Ended | Status::Waiting) {
            return Ok(None);
        }
        let harness = Harness::read(&st.harness).unwrap_or(Harness::ClaudeCode);
        let now = self.now();
        let wait = |why: String| async move {
            self.submit::<w::Wait>(w::Because { worker, why: why.clone() }).await?;
            Ok::<_, crate::Refusal>(Some(json!({ "worker": worker, "waits": why })))
        };
        let Some(screen) = self.host.orca().screen(&st.terminal) else {
            return wait("its session's terminal is gone".into()).await;
        };
        if let Some(quota) = harness.account_quota(&screen) {
            if let Some(account) = &st.account {
                let note = AccountNote { name: account.clone(), windows: Vec::new(), until: now + QUOTA_REST_MS };
                self.submit::<LimitAccount>(note).await?;
            }
            self.end_worker(worker, &format!("its account is used up: {quota}"), false).await?;
            return Ok(Some(json!({ "worker": worker, "ended": "quota", "said": quota })));
        }
        if let Some(said) = harness.task_refusal(&screen).or_else(|| harness.capacity_stop(&screen)) {
            return wait(format!("its harness stopped: {said}")).await;
        }
        // Hot: over its share for `hot_for`.
        let over = hot::over(load, settings);
        let since = {
            let mut m = self.memory.lock().unwrap_or_else(|p| p.into_inner());
            match &over {
                Some(_) => Some(*m.over.entry(worker).or_insert(now)),
                None => m.over.remove(&worker).and(None),
            }
        };
        if let (Some(what), true) = (&over, hot::hot(since, now, settings)) {
            let evidence = self.evidence(worker);
            self.memory.lock().unwrap_or_else(|p| p.into_inner()).over.remove(&worker);
            return wait(format!("hot: {what}; evidence in {}", evidence.display())).await;
        }
        // Silent: its turn ended, and no done, past the bound.
        if wk.status == Status::Working {
            let root = self.session_root(harness);
            let idle_since = match turn_of(harness, &root, &st.worktree, &st.brief) {
                Some(Turn::Closed { since, .. }) => Some((since as i64 * 1000).max(wk.since)),
                Some(Turn::Open { .. }) => None,
                None => Some(wk.since),
            };
            if let Some(t) = idle_since.filter(|t| now - t > settings.ms(Time::SilentBound)) {
                return wait(format!("silent for {} min without `factorium worker done`", (now - t) / 60_000)).await;
            }
        }
        Ok(None)
    }

    /// Writes what the worker's processes are doing now; answers where.
    fn evidence(&self, worker: Uuid) -> PathBuf {
        let path = self.dir(&["evidence", &format!("{worker}-{}.txt", self.now())]);
        let procs: Vec<String> = self
            .host
            .processes()
            .into_iter()
            .filter(|p| p.worker.as_deref() == Some(&worker.to_string()))
            .map(|p| format!("{:>7} {:>7} {:>6.1}% {:>9}KiB {}", p.pid, p.ppid, p.cpu, p.rss_kib, p.command))
            .collect();
        let _ = std::fs::create_dir_all(path.parent().expect("a parent"));
        let _ = std::fs::write(&path, format!("    pid    ppid    cpu       rss command\n{}\n", procs.join("\n")));
        path
    }
}
