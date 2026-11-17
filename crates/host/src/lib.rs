//! The host's effects: git, Orca's terminals, shell commands under a limit,
//! the host's readings, its processes, mytokens and Homebrew. Every one the
//! app reaches through [`Host`] so that a scenario stands in for it; git and
//! shell commands are always real.

pub mod git;
pub mod orca;
pub mod procs;
pub mod reading;
pub mod shell;

pub use git::Git;
pub use orca::{Orca, OrcaCli};
pub use procs::Proc;
pub use reading::Reading;
pub use shell::{Ran, run};

/// What the app asks of the machine beyond git and the shell.
pub trait Host: Send + Sync {
    /// Terminals and the worktrees they run in.
    fn orca(&self) -> &dyn Orca;
    /// The host's reading now.
    fn reading(&self) -> Reading;
    /// Every process of this user, with its environment's factorium tag.
    fn processes(&self) -> Vec<Proc>;
    /// Whether process `pid` is alive.
    fn alive(&self, pid: u32) -> bool;
    /// What `mytokens <args>` printed, or why it failed (never what it printed).
    fn mytokens(&self, args: &[&str]) -> Result<String, String>;
    /// Upgrades a Homebrew formula; answers what brew printed.
    fn brew_upgrade(&self, formula: &str) -> Result<String, String>;
    /// The user's home, where harnesses keep their sessions.
    fn home(&self) -> std::path::PathBuf;
}

/// The real machine.
pub struct Machine {
    orca: OrcaCli,
    cpu: std::sync::Mutex<reading::Cpu>,
    /// The directory every disk reading is of.
    pub data: std::path::PathBuf,
}

impl Machine {
    pub fn new(data: std::path::PathBuf) -> Machine {
        Machine { orca: OrcaCli, cpu: Default::default(), data }
    }
}

impl Host for Machine {
    fn orca(&self) -> &dyn Orca {
        &self.orca
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

    fn mytokens(&self, args: &[&str]) -> Result<String, String> {
        shell::output("mytokens", args).map_err(|_| format!("mytokens {} failed", args.first().unwrap_or(&"")))
    }

    fn brew_upgrade(&self, formula: &str) -> Result<String, String> {
        let _ = shell::output("brew", &["update", "--quiet"]);
        shell::output("brew", &["upgrade", formula])
    }

    fn home(&self) -> std::path::PathBuf {
        std::env::var_os("HOME").map(Into::into).unwrap_or_else(|| "/".into())
    }
}
