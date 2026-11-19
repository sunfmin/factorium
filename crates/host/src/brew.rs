//! Homebrew: installing the version a cut made, and finding the binary a
//! formula links. Every call is `brew …`, run through [`Programs`].

use std::path::{Path, PathBuf};

use crate::shell::Programs;

/// Seconds to wait before each try of `brew update` and upgrade: a tap
/// pushed seconds ago may not show yet. 90s in all.
const TAP_WAITS: [u64; 7] = [0, 2, 4, 8, 16, 30, 30];

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
        // `brew update` may see the tap seconds before its new formula; then
        // the upgrade has nothing to do and succeeds. Wait, backing off, up
        // to a bound before it fails.
        let mut waited = 0;
        for wait in TAP_WAITS {
            if wait > 0 {
                self.0.run("sleep", &[&wait.to_string()]);
                waited += wait;
            }
            let said = upgrade()?;
            if names(&has(), version) {
                return Ok(said);
            }
        }
        Err(stale(formula, version, &has(), waited))
    }

    /// The program `name` that `formula`, as Homebrew links it now, has, if it has one.
    pub fn bin(&self, formula: &str, name: &str) -> Option<PathBuf> {
        let prefix = self.brew(&["--prefix", formula]).ok()?;
        Some(Path::new(prefix.trim()).join("bin").join(name)).filter(|b| b.is_file())
    }

    /// Where Homebrew links the program `me` (a path canonical, as
    /// `<cellar>/<formula>/<version>/bin/<name>`), if Homebrew installed
    /// it: the opt link, which follows whichever version is installed, `me`'s
    /// or a newer one.
    pub fn link_of(&self, me: &Path) -> Option<PathBuf> {
        let kegs = me.parent()?.parent()?.parent()?;
        let prefix = self.brew(&["--prefix", kegs.file_name()?.to_str()?]).ok()?;
        let bin = Path::new(prefix.trim()).join("bin").join(me.file_name()?);
        (bin.canonicalize().ok()?.parent()?.parent()?.parent()? == kegs).then_some(bin)
    }
}

/// Whether `brew list --versions` output names `version` (its leading v aside).
fn names(listed: &str, version: &str) -> bool {
    let want = version.strip_prefix('v').unwrap_or(version);
    listed.split_whitespace().skip(1).any(|v| v == want)
}

/// Why a cut's install failed: Homebrew still has another version.
fn stale(formula: &str, version: &str, listed: &str, waited: u64) -> String {
    let listed = listed.trim();
    let has = if listed.is_empty() { "nothing".to_string() } else { format!("`{listed}`") };
    format!(
        "Homebrew has {has} of {formula}, not {}, after brew update and upgrade for {waited}s",
        version.strip_prefix('v').unwrap_or(version)
    )
}
