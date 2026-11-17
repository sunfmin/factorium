//! The host's effects: git, Orca's terminals, shell commands under a limit,
//! the host's readings, its processes, mytokens and Homebrew. Every one the
//! app reaches through [`Host`] so that a scenario stands in for it; git and
//! shell commands are always real. Orca, Homebrew and mytokens are programs
//! whose answers the app reads: a scenario stands in for the programs, never
//! for what is made of their answers.

pub mod brew;
pub mod git;
pub mod orca;
pub mod procs;
pub mod reading;
pub mod shell;

pub use brew::Brew;
pub use git::Git;
pub use orca::{Orca, Term};
pub use procs::Proc;
pub use reading::Reading;
pub use shell::{Programs, Ran, Said, System, run};

/// What the app asks of the machine beyond git and the shell.
pub trait Host: Send + Sync {
    /// The programs the app runs and reads the answers of: orca, brew, mytokens.
    fn programs(&self) -> &dyn Programs;
    /// The host's reading now.
    fn reading(&self) -> Reading;
    /// Every process of this user, with its environment's factorium tag.
    fn processes(&self) -> Vec<Proc>;
    /// Whether process `pid` is alive.
    fn alive(&self, pid: u32) -> bool;
    /// The user's home, where harnesses keep their sessions and git its config.
    fn home(&self) -> std::path::PathBuf;
}

impl dyn Host + '_ {
    /// Terminals and the worktrees they run in.
    pub fn orca(&self) -> Orca<'_> {
        Orca(self.programs())
    }

    /// git in `dir`, as this host runs it.
    pub fn git(&self, dir: impl Into<std::path::PathBuf>) -> Git {
        Git::new(self.home(), dir)
    }

    /// Homebrew.
    pub fn brew(&self) -> Brew<'_> {
        Brew(self.programs())
    }

    /// What `mytokens <args>` printed, or why it failed (never what it printed).
    pub fn mytokens(&self, args: &[&str]) -> Result<String, String> {
        let said = self.programs().run("mytokens", args);
        said.result("mytokens", args).map_err(|_| format!("mytokens {} failed", args.first().unwrap_or(&"")))
    }
}

/// The real machine.
pub struct Machine {
    cpu: std::sync::Mutex<reading::Cpu>,
    /// The directory every disk reading is of.
    pub data: std::path::PathBuf,
}

impl Machine {
    pub fn new(data: std::path::PathBuf) -> Machine {
        Machine { cpu: Default::default(), data }
    }
}

impl Host for Machine {
    fn programs(&self) -> &dyn Programs {
        &System
    }

    fn reading(&self) -> Reading {
        let mut cpu = self.cpu.lock().unwrap_or_else(|p| p.into_inner());
        reading::read(&mut cpu, &self.data)
    }

    fn processes(&self) -> Vec<Proc> {
        procs::list()
    }

    fn alive(&self, pid: u32) -> bool {
        procs::alive(pid)
    }

    fn home(&self) -> std::path::PathBuf {
        std::env::var_os("HOME").map(Into::into).unwrap_or_else(|| "/".into())
    }
}
