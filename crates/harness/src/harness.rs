//! A harness: the program a session runs in. How it starts, how a tool call
//! reaches the hook, where it keeps its session and which lines start or end
//! a turn, and which screen words mean a quota or a refusal are each one
//! harness's own, in its module here and nowhere else.

use std::path::{Path, PathBuf};

use crate::line::Line;
use crate::start::{Home, Kept, Launch, Start};
use crate::turn::Turn;

mod claude_code;
mod codex;
mod dsh;
mod grok;
mod kimi;
mod pi;
mod session;

/// The same item of each harness's module, for `$harness`.
macro_rules! each {
    ($harness:expr, $($item:tt)+) => {
        match $harness {
            Harness::ClaudeCode => claude_code::$($item)+,
            Harness::Pi => pi::$($item)+,
            Harness::Codex => codex::$($item)+,
            Harness::Dsh => dsh::$($item)+,
            Harness::Kimi => kimi::$($item)+,
            Harness::Grok => grok::$($item)+,
        }
    };
}

/// One tool call a session is about to make, as far as a hook judges it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    /// It runs this shell line: the only kind a hook judges.
    Shell(String),
    Other,
}

/// A file or directory of the host user's that a kept directory links to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Link {
    /// Where it is, under the host user's home.
    home: &'static str,
    /// Where the link is, under the kept directory.
    kept: &'static str,
}

/// How the session files of one worktree are laid out under the harness
/// directory. The reader opens the files; a missing directory is no turn.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionLayout {
    /// Each file of `extension` directly in `dir` is one session.
    Files { dir: String, extension: &'static str },
    /// Each file of `extension` anywhere under `dir` is one session.
    Tree { dir: String, extension: &'static str },
    /// Each `dir/*/<name>` is one session.
    Named { dir: String, name: &'static str },
    /// `file` names the session files (`kimi::indexed_sessions`).
    Index { file: String },
}

/// One file a stand-in writes under the harness directory; the body is the
/// text the harness's turn reader reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionFile {
    /// Path under the harness directory.
    path: String,
    /// The session text, before `compressed` storage.
    body: String,
    /// The body is stored as zstd.
    compressed: bool,
    /// Before writing, the file of this name under the directory is removed.
    replaces: Option<String>,
}

/// What a session's screen says stopped it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// Its account is used up: the line that says so.
    Quota(String),
    /// Its harness refused the task or ran out of capacity: the line.
    Refused(String),
}

/// A harness a session can be started in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    ClaudeCode,
    Pi,
    Codex,
    Dsh,
    Kimi,
    Grok,
}

const ALL: [Harness; 6] =
    [Harness::ClaudeCode, Harness::Pi, Harness::Codex, Harness::Dsh, Harness::Kimi, Harness::Grok];

impl Harness {
    /// What a config or a record writes it as.
    pub const fn key(self) -> &'static str {
        each!(self, KEY)
    }

    /// The harness written as `key`.
    pub fn read(key: &str) -> Option<Harness> {
        ALL.into_iter().find(|harness| harness.key() == key)
    }

    /// Whether its login refreshes and is bound to one machine, so that an
    /// account of it names that machine.
    pub(crate) const fn logs_in(self) -> bool {
        matches!(self, Harness::Codex | Harness::Grok | Harness::Kimi)
    }

    /// The file under its user configuration whose being there means it is
    /// logged in.
    pub(crate) fn login_file(self) -> Option<&'static str> {
        each!(self, KEPT_LINKS).first().map(|link| link.kept)
    }

    /// Where its sessions keep their files on this host, decided once from
    /// the host user's home `user_home`, the session's `account` and the
    /// directory `kept` this host keeps for the harness: a login account's
    /// own home, where its login is; else Claude Code's under the host
    /// user's home, every other's the kept directory.
    pub fn home(self, user_home: &Path, account: Option<&str>, kept: &Path) -> Home {
        Home::new(match account {
            Some(name) if self.logs_in() => crate::account::login_home(user_home, name),
            _ if self == Harness::ClaudeCode => user_home.join(claude_code::HOME),
            _ => kept.to_path_buf(),
        })
    }

    /// Everything it takes to start a session as `start` says, read but
    /// not done: what its home `start.home` must hold — the host user's
    /// login files linked, and the files the tool carries or makes for this
    /// harness, its guard among them — the line a terminal runs, and what is
    /// typed at it once it runs to hand it its brief. Each harness starts in
    /// `start.dir` and is given the worktree, the hook, Orca's status report
    /// and the model; Grok works only in its cwd, so it starts in the
    /// worktree. An error: a file of the host user's it is made from could
    /// not be read as it must be.
    pub fn launch(self, start: &Start) -> Result<Launch, String> {
        let home = start.home.path();
        let mut kept = vec![Kept::Dir { path: home.to_path_buf() }];
        let links = each!(self, KEPT_LINKS)
            .iter()
            .map(|link| (start.user_home.join(link.home), home.join(link.kept)))
            .filter(|(to, _)| to.exists())
            .map(|(to, path)| Kept::Link { path, to });
        kept.extend(links);
        let files = match self {
            Harness::Pi => pi::kept(start),
            Harness::Dsh => dsh::kept(start),
            Harness::Kimi => kimi::kept(start)?,
            Harness::Grok => grok::kept(start)?,
            Harness::ClaudeCode | Harness::Codex => Vec::new(),
        };
        kept.extend(files.into_iter().map(|(path, text)| Kept::File { path: home.join(path), text }));
        let directory = match self {
            Harness::Grok => start.worktree,
            _ => start.dir,
        };
        let mut line = Line::new().fixed("cd ").word(directory).fixed(" && export");
        for variable in self.environment() {
            line = line.fixed(" ").fixed(variable).fixed("=1");
        }
        let mut line = each!(self, environment(line, start));
        for (name, value) in &start.env {
            line = line.fixed(" ").fixed(name).fixed("=").word(value);
        }
        let line = line.fixed(" && ").launch(start.command);
        let line = each!(self, added(line, start)).done();
        Ok(match self {
            Harness::ClaudeCode => Launch { kept, line: claude_code::brief_at_start(&line, start.brief), typed: None },
            Harness::Codex => Launch { kept, line, typed: Some(codex::brief_line(start.brief)) },
            _ => Launch { kept, line, typed: Some(crate::brief::brief_pointer(start.brief)) },
        })
    }

    /// The variables that switch this harness, each set to 1.
    const fn environment(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode => &claude_code::ENVIRONMENT,
            Harness::Kimi => &kimi::ENVIRONMENT,
            Harness::Grok => &grok::ENVIRONMENT,
            Harness::Pi | Harness::Codex | Harness::Dsh => &[],
        }
    }

    /// The shell line of the tool call a hook is handed as `payload`;
    /// `None` when it is no shell call.
    pub fn shell_call(self, payload: &str) -> Option<String> {
        let mut described = serde_json::Deserializer::from_str(payload);
        match each!(self, call(&mut described)) {
            Ok(Call::Shell(line)) => Some(line),
            _ => None,
        }
    }

    /// The hook exit code that refuses a call: it is not made, and the
    /// hook's stderr is shown to the session.
    pub const fn refused(self) -> u8 {
        each!(self, REFUSED)
    }

    /// What `screen`, the end of a session's terminal, says stopped it.
    pub fn stopped(self, screen: &[String]) -> Option<Stop> {
        let quota = match self {
            Harness::Codex => codex::account_quota(screen),
            Harness::Pi => pi::account_quota(screen),
            Harness::Kimi => kimi::account_quota(screen),
            _ => None,
        };
        if let Some(said) = quota {
            return Some(Stop::Quota(said));
        }
        match self {
            Harness::Codex => codex::task_refusal(screen).or_else(|| codex::capacity_stop(screen)).map(Stop::Refused),
            _ => None,
        }
    }

    /// The turn its session files record for the worktree `cwd` of the
    /// session handed `brief`, read under its `home`. `None`: no session says.
    pub fn turn(self, home: &Home, cwd: &str, brief: &str) -> Option<Turn> {
        let root = home.path();
        let mut files = Vec::new();
        match each!(self, session_layout(cwd)) {
            SessionLayout::Files { dir, extension } => files_under(&root.join(dir), extension, false, &mut files),
            SessionLayout::Tree { dir, extension } => files_under(&root.join(dir), extension, true, &mut files),
            SessionLayout::Named { dir, name } => {
                for d in std::fs::read_dir(root.join(dir)).into_iter().flatten().flatten() {
                    files.push(d.path().join(name));
                }
            }
            SessionLayout::Index { file } => {
                let index = std::fs::read_to_string(root.join(file)).unwrap_or_default();
                for i in kimi::indexed_sessions(&index) {
                    let p = PathBuf::from(&i.file);
                    files.push(if p.is_absolute() { p } else { root.join(p) });
                }
            }
        }
        let turns: Vec<Turn> = files
            .iter()
            .filter_map(|f| read_session(f))
            .filter(|text| self.could_record(cwd, text, brief))
            .filter_map(|text| self.recorded_turn(cwd, &text, brief))
            .collect();
        crate::turn::prefer(&turns)
    }

    /// The stand-in session: writes the session files this harness would
    /// for the worktree `cwd`, where `turn` reads them, with its turn left
    /// open when `open`, written at `at` (epoch seconds), under its `home`.
    pub fn write_session(self, home: &Home, cwd: &str, open: bool, at: u64) -> std::io::Result<()> {
        let root = home.path();
        for f in each!(self, session_files(cwd, open, at)) {
            if let Some(name) = &f.replaces {
                let _ = std::fs::remove_file(root.join(name));
            }
            let path = root.join(&f.path);
            std::fs::create_dir_all(path.parent().unwrap_or(root))?;
            if f.compressed {
                let fast = ruzstd::encoding::CompressionLevel::Fastest;
                std::fs::write(&path, ruzstd::encoding::compress_to_vec(f.body.as_bytes(), fast))?;
            } else {
                std::fs::write(&path, &f.body)?;
            }
        }
        if self == Harness::Kimi {
            std::fs::write(root.join(kimi::INDEX), kimi::session_index(cwd, ""))?;
        }
        Ok(())
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

/// A session file's text; a zstd file decompressed.
fn read_session(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if path.extension().is_some_and(|e| e == "zstd" || e == "zst") {
        let mut text = String::new();
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(&bytes[..]).ok()?;
        std::io::Read::read_to_string(&mut decoder, &mut text).ok()?;
        return Some(text);
    }
    String::from_utf8(bytes).ok()
}
