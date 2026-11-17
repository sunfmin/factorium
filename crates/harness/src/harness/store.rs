//! Where a harness keeps its sessions under its home, said once and gone
//! through both ways: the turn a worktree's session files record is read
//! from where its `Layout` says, and the stand-in session is written there.
//! Which lines start or end a turn is each harness's own module's.

use std::path::{Path, PathBuf};

use super::{Harness, claude_code, codex, dsh, grok, kimi, pi, session};
use crate::start::Home;
use crate::turn::Turn;

/// How the session files of one worktree lie under a harness's home. A
/// missing directory or index is no session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Layout {
    /// Each file of `extension` directly in `dir` is one session.
    Files { dir: String, extension: &'static str },
    /// Each file of `extension` anywhere under `dir` is one session; each
    /// file says its worktree.
    Tree { dir: String, extension: &'static str },
    /// Each `dir/*/<name>` is one session.
    Named { dir: String, name: &'static str },
    /// The JSONL `index` names each worktree's session directories; each
    /// `<directory>/<name>` is one session.
    Index { index: &'static str, name: &'static str },
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
            Layout::Index { index, name } => {
                let index = std::fs::read_to_string(root.join(index)).unwrap_or_default();
                for dir in kimi::session_dirs(&index, cwd) {
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
            Layout::Index { index, name } => {
                let (index, dir) = (root.join(index), format!("sessions/{own}"));
                let existing = std::fs::read_to_string(&index).unwrap_or_default();
                std::fs::write(&index, kimi::session_index(cwd, &existing, &dir))?;
                root.join(dir).join(name)
            }
        })
    }
}

impl Harness {
    /// The turn its session files record for the worktree `cwd` of the
    /// session handed `brief`, read under its `home`. `None`: no session says.
    pub fn turn(self, home: &Home, cwd: &str, brief: &str) -> Option<Turn> {
        let turns: Vec<Turn> = self
            .layout(cwd)
            .sessions(home.path(), cwd)
            .iter()
            .filter_map(|f| read(f))
            .filter(|text| self.could_record(cwd, text, brief))
            .filter_map(|text| self.recorded_turn(cwd, &text, brief))
            .collect();
        crate::turn::prefer(&turns)
    }

    /// The stand-in session: writes the session file this harness would
    /// for the worktree `cwd`, where `turn` reads it, with its turn left
    /// open when `open`, written at `at` (epoch seconds), under its `home`.
    /// A stand-in written again for `cwd` replaces the one before.
    pub fn write_session(self, home: &Home, cwd: &str, open: bool, at: u64) -> std::io::Result<()> {
        let root = home.path();
        std::fs::create_dir_all(root)?;
        let path = self.layout(cwd).stand_in(root, cwd)?;
        let body = match self {
            Harness::ClaudeCode => claude_code::stand_in(cwd, open, at),
            Harness::Pi => pi::stand_in(cwd, open, at),
            Harness::Codex => codex::stand_in(cwd, open, at),
            Harness::Dsh => dsh::stand_in(open, at),
            Harness::Kimi => kimi::stand_in(open, at),
            Harness::Grok => grok::stand_in(open, at),
        };
        std::fs::create_dir_all(path.parent().unwrap_or(root))?;
        if compressed(&path) {
            let fast = ruzstd::encoding::CompressionLevel::Fastest;
            std::fs::write(&path, ruzstd::encoding::compress_to_vec(body.as_bytes(), fast))
        } else {
            std::fs::write(&path, body)
        }
    }

    fn layout(self, cwd: &str) -> Layout {
        match self {
            Harness::ClaudeCode => claude_code::layout(),
            Harness::Pi => pi::layout(cwd),
            Harness::Codex => codex::layout(),
            Harness::Dsh => dsh::layout(cwd),
            Harness::Kimi => kimi::layout(),
            Harness::Grok => grok::layout(cwd),
        }
    }

    /// The turn a session file's `text` records for the worktree `cwd`.
    /// `None` when the file is another worktree's or holds no turn. A child
    /// session does not start or end the turn. `brief` is the file handed to
    /// this session: a harness started elsewhere is known by its first
    /// prompt naming it.
    fn recorded_turn(self, cwd: &str, text: &str, brief: &str) -> Option<Turn> {
        match self {
            Harness::ClaudeCode => claude_code::recorded_turn(cwd, text, None, Some(brief)),
            Harness::Pi => pi::recorded_turn(cwd, text, None),
            Harness::Codex => codex::recorded_turn(cwd, text, None, Some(brief)),
            Harness::Dsh => dsh::recorded_turn(cwd, text, None),
            Harness::Kimi => kimi::recorded_turn(cwd, text, None),
            Harness::Grok => grok::recorded_turn(cwd, text, None),
        }
    }

    /// Whether `text` could make it `cwd`'s session for `recorded_turn`,
    /// without parsing it. A harness writing every worktree's sessions into
    /// one tree knows its own by a line naming `cwd` or `brief`, which JSON
    /// writes as themselves unless they hold a quote, a backslash or a
    /// control character.
    fn could_record(self, cwd: &str, text: &str, brief: &str) -> bool {
        match self {
            Harness::ClaudeCode | Harness::Codex => {
                let written_as_is = |path: &str| !path.chars().any(|ch| ch == '"' || ch == '\\' || ch.is_control());
                let cwd = cwd.trim_end_matches('/');
                if cwd.is_empty() || !written_as_is(cwd) || !written_as_is(brief) {
                    return true;
                }
                text.contains(cwd) || text.contains(brief)
            }
            _ => true,
        }
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
