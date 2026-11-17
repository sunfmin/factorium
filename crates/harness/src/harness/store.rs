//! Where a harness keeps its sessions under its home, said once and gone
//! through both ways: the turn a worktree's session files record is read
//! from where its `Layout` says, and the stand-in session is written there.
//! Which lines start or end a turn is each harness's own module's.

use std::path::{Path, PathBuf};

use super::{Harness, Kind, session};
use crate::start::Home;
use crate::turn::Turn;

/// How the session files of one worktree lie under a harness's home. A
/// missing directory or index is no session.
pub(super) enum Layout {
    /// Each file of `extension` directly in `dir` is one session.
    Files { dir: String, extension: &'static str },
    /// Each file of `extension` anywhere under `dir` is one session; each
    /// file says its worktree.
    Tree { dir: String, extension: &'static str },
    /// Each `dir/*/<name>` is one session.
    Named { dir: String, name: &'static str },
    /// The `index` names each worktree's session directories: `dirs` reads
    /// those of a worktree from its text, `with` adds one to it. Each
    /// `<directory>/<name>` is one session.
    Index {
        index: &'static str,
        name: &'static str,
        dirs: fn(index: &str, cwd: &str) -> Vec<String>,
        with: fn(cwd: &str, index: &str, dir: &str) -> String,
    },
}

impl Layout {
    /// Every file under `root` that may be a session of `cwd`.
    fn sessions(&self, root: &Path, cwd: &str) -> Vec<PathBuf> {
        let mut files = Vec::new();
        match self {
            Layout::Files { dir, extension } => files_under(&root.join(dir), extension, false, &mut files),
            Layout::Tree { dir, extension } => files_under(&root.join(dir), extension, true, &mut files),
            Layout::Named { dir, name } => {
                for d in std::fs::read_dir(root.join(dir)).into_iter().flatten().flatten() {
                    files.push(d.path().join(name));
                }
            }
            Layout::Index { index, name, dirs, .. } => {
                let index = std::fs::read_to_string(root.join(index)).unwrap_or_default();
                for dir in dirs(&index, cwd) {
                    files.push(root.join(dir).join(name));
                }
            }
        }
        files
    }

    /// Where the stand-in session of `cwd` goes under `root`: a place
    /// `sessions` finds. An index is made to name it.
    fn stand_in(&self, root: &Path, cwd: &str) -> std::io::Result<PathBuf> {
        let own = format!("factorium-{}", session::mark(cwd));
        Ok(match self {
            Layout::Files { dir, extension } => root.join(dir).join(format!("{own}.{extension}")),
            Layout::Tree { dir, extension } => root.join(dir).join("factorium").join(format!("{own}.{extension}")),
            Layout::Named { dir, name } => root.join(dir).join(own).join(name),
            Layout::Index { index, name, with, .. } => {
                let (index, dir) = (root.join(index), format!("sessions/{own}"));
                let existing = std::fs::read_to_string(&index).unwrap_or_default();
                std::fs::write(&index, with(cwd, &existing, &dir))?;
                root.join(dir).join(name)
            }
        })
    }
}

impl Harness {
    /// The turn its session files record for the worktree `cwd` of the
    /// session handed `brief`, read under its `home`. `None`: no session says.
    pub fn turn(self, home: &Home, cwd: &str, brief: &str) -> Option<Turn> {
        each!(self, |K| turn::<K>(home.path(), cwd, brief))
    }

    /// The stand-in session: writes the session file this harness would
    /// for the worktree `cwd`, where `turn` reads it, with its turn left
    /// open when `open`, written at `at` (epoch seconds), under its `home`.
    /// A stand-in written again for `cwd` replaces the one before.
    pub fn write_session(self, home: &Home, cwd: &str, open: bool, at: u64) -> std::io::Result<()> {
        each!(self, |K| write_session::<K>(home.path(), cwd, open, at))
    }
}

fn turn<K: Kind>(root: &Path, cwd: &str, brief: &str) -> Option<Turn> {
    let turns: Vec<Turn> = K::layout(cwd)
        .sessions(root, cwd)
        .iter()
        .filter_map(|f| read(f))
        .filter(|text| K::could_record(cwd, text, brief))
        .filter_map(|text| K::recorded_turn(cwd, &text, brief))
        .collect();
    crate::turn::prefer(&turns)
}

fn write_session<K: Kind>(root: &Path, cwd: &str, open: bool, at: u64) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    let path = K::layout(cwd).stand_in(root, cwd)?;
    let body = K::stand_in(cwd, open, at);
    std::fs::create_dir_all(path.parent().unwrap_or(root))?;
    if compressed(&path) {
        let fast = ruzstd::encoding::CompressionLevel::Fastest;
        std::fs::write(&path, ruzstd::encoding::compress_to_vec(body.as_bytes(), fast))
    } else {
        std::fs::write(&path, body)
    }
}

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

/// Whether a session file at `path` is stored as zstd.
fn compressed(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "zstd" || e == "zst")
}

/// A session file's text; a zstd file decompressed.
fn read(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if compressed(path) {
        let mut text = String::new();
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(&bytes[..]).ok()?;
        std::io::Read::read_to_string(&mut decoder, &mut text).ok()?;
        return Some(text);
    }
    String::from_utf8(bytes).ok()
}
