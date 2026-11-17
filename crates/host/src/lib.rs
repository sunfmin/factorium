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
    /// Installs or upgrades a Homebrew formula to `version` (a tag such as
    /// `v0.1.1`); answers what brew printed, or why Homebrew has not that
    /// version once done.
    fn brew_upgrade(&self, formula: &str, version: &str) -> Result<String, String>;
    /// If this program is the binary `formula` installs, where that binary
    /// is found after an upgrade too; asked before the upgrade.
    fn binary_of(&self, formula: &str) -> Option<std::path::PathBuf>;
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

    fn brew_upgrade(&self, formula: &str, version: &str) -> Result<String, String> {
        let upgrade = || {
            let _ = shell::output("brew", &["update", "--quiet"]);
            // The first cut installs it: `brew upgrade` refuses a formula not installed.
            if shell::output("brew", &["list", "--formula", formula]).is_err() {
                return shell::output("brew", &["install", formula]);
            }
            shell::output("brew", &["upgrade", formula])
        };
        let has = || shell::output("brew", &["list", "--versions", formula]).unwrap_or_default();
        // `brew update` may see the tap before its new formula; then the
        // upgrade has nothing to do and succeeds. Once more, then it fails.
        let said = upgrade()?;
        if names(&has(), version) {
            return Ok(said);
        }
        let said = upgrade()?;
        match has() {
            h if names(&h, version) => Ok(said),
            h => Err(stale(formula, version, &h)),
        }
    }

    fn binary_of(&self, formula: &str) -> Option<std::path::PathBuf> {
        // The opt prefix links whichever version is installed.
        let prefix = shell::output("brew", &["--prefix", formula]).ok()?;
        let me = std::env::current_exe().ok()?.canonicalize().ok()?;
        let bin = std::path::Path::new(prefix.trim()).join("bin").join(me.file_name()?);
        (bin.canonicalize().ok()? == me).then_some(bin)
    }

    fn home(&self) -> std::path::PathBuf {
        std::env::var_os("HOME").map(Into::into).unwrap_or_else(|| "/".into())
    }
}

/// Whether `brew list --versions` output names `version` (its leading v aside).
pub fn names(listed: &str, version: &str) -> bool {
    let want = version.strip_prefix('v').unwrap_or(version);
    listed.split_whitespace().skip(1).any(|v| v == want)
}

/// Why a cut's install failed: Homebrew still has another version.
pub fn stale(formula: &str, version: &str, listed: &str) -> String {
    let listed = listed.trim();
    let has = if listed.is_empty() { "nothing".to_string() } else { format!("`{listed}`") };
    format!(
        "Homebrew has {has} of {formula}, not {}, after brew update and upgrade twice",
        version.strip_prefix('v').unwrap_or(version)
    )
}

#[cfg(test)]
mod tests {
    use super::names;

    #[test]
    fn names_the_version_without_its_v() {
        assert!(names("web 0.1.1\n", "v0.1.1"));
        assert!(names("web 0.1.0 0.1.1", "v0.1.1"));
        assert!(!names("web 0.1.0\n", "v0.1.1"));
        assert!(!names("", "v0.1.1"));
        assert!(!names("0.1.1 0.1.0", "v0.1.1"), "the first word is the formula");
    }
}
