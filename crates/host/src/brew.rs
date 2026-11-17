//! Homebrew: installing the version a cut made, and finding the binary a
//! formula links. Every call is `brew …`, run through [`Programs`].

use std::path::{Path, PathBuf};

use crate::shell::Programs;

/// Homebrew, reached through the programs the host runs.
pub struct Brew<'a>(pub &'a dyn Programs);

impl Brew<'_> {
    fn brew(&self, args: &[&str]) -> Result<String, String> {
        self.0.run("brew", args).result("brew", args)
    }

    /// Installs or upgrades `formula` to `version` (a tag such as `v0.1.1`);
    /// answers what brew printed, or why Homebrew has not that version once
    /// done.
    pub fn upgrade(&self, formula: &str, version: &str) -> Result<String, String> {
        let upgrade = || {
            let _ = self.brew(&["update", "--quiet"]);
            // The first cut installs it: `brew upgrade` refuses a formula not installed.
            if self.brew(&["list", "--formula", formula]).is_err() {
                return self.brew(&["install", formula]);
            }
            self.brew(&["upgrade", formula])
        };
        let has = || self.brew(&["list", "--versions", formula]).unwrap_or_default();
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

    /// If this program is the binary `formula` installs, where that binary
    /// is found after an upgrade too; asked before the upgrade.
    pub fn binary_of(&self, formula: &str) -> Option<PathBuf> {
        // The opt prefix links whichever version is installed.
        let prefix = self.brew(&["--prefix", formula]).ok()?;
        let me = std::env::current_exe().ok()?.canonicalize().ok()?;
        let bin = Path::new(prefix.trim()).join("bin").join(me.file_name()?);
        (bin.canonicalize().ok()? == me).then_some(bin)
    }
}

/// Whether `brew list --versions` output names `version` (its leading v aside).
fn names(listed: &str, version: &str) -> bool {
    let want = version.strip_prefix('v').unwrap_or(version);
    listed.split_whitespace().skip(1).any(|v| v == want)
}

/// Why a cut's install failed: Homebrew still has another version.
fn stale(formula: &str, version: &str, listed: &str) -> String {
    let listed = listed.trim();
    let has = if listed.is_empty() { "nothing".to_string() } else { format!("`{listed}`") };
    format!(
        "Homebrew has {has} of {formula}, not {}, after brew update and upgrade twice",
        version.strip_prefix('v').unwrap_or(version)
    )
}
