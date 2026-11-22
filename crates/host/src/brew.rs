//! Homebrew: installing the version a cut made, and finding the binary a
//! formula links. Every call is `brew …`, run through [`Programs`].

use std::path::{Path, PathBuf};

use crate::shell::Programs;

/// Seconds to wait before each try of `brew update` and upgrade: a tap
/// pushed seconds ago may not show yet, or another brew holds the
/// formula's lock. 90s in all.
const TAP_WAITS: [u64; 7] = [0, 2, 4, 8, 16, 30, 30];

/// Homebrew, reached through the programs the host runs.
pub struct Brew<'a>(pub &'a dyn Programs);

impl Brew<'_> {
    fn brew(&self, args: &[&str]) -> Result<String, String> {
        self.0.run("brew", args).result("brew", args)
    }

    /// Installs or upgrades `formula` to `version` (a tag such as `v0.1.1`);
    /// answers what brew printed, or why Homebrew has not that version, or
    /// does not link it, once done. Another brew holding the formula's lock
    /// is waited for: once it frees, the version installed is what counts,
    /// whoever installed it.
    pub fn upgrade(&self, formula: &str, version: &str) -> Result<String, String> {
        let upgrade = || {
            let _ = self.brew(&["update", "--quiet"]);
            // The first cut installs it: `brew upgrade` refuses a formula not installed.
            if self.brew(&["list", "--formula", formula]).is_err() {
                return self.brew(&["install", formula]);
            }
            self.brew(&["upgrade", formula])
        };
        // `brew update` may see the tap seconds before its new formula; then
        // the upgrade has nothing to do and succeeds. Wait, backing off, up
        // to a bound before it fails.
        let (mut waited, mut locked) = (0, None);
        for wait in TAP_WAITS {
            if wait > 0 {
                self.0.run("sleep", &[&wait.to_string()]);
                waited += wait;
            }
            let said = match upgrade() {
                Err(e) if e.contains("has already locked") => {
                    locked = Some(e);
                    String::new()
                }
                said => {
                    locked = None;
                    said?
                }
            };
            if self.has(formula, version) {
                return self.linked(formula, version).map(|()| said);
            }
        }
        match locked {
            Some(e) => Err(format!("{e} (still locked after {waited}s{})", self.holder(formula))),
            None => Err(stale(formula, version, &self.listed(formula), waited)),
        }
    }

    /// Upgrades `formula`, installed, to the newest version its tap has once
    /// `brew update` sees it (factorium#269): answers what brew printed, or
    /// why it failed. A tap with nothing newer is no failure.
    pub fn upgrade_newest(&self, formula: &str) -> Result<String, String> {
        self.brew(&["update", "--quiet"])?;
        self.brew(&["upgrade", formula])
    }

    /// Whether Homebrew has `version` (a tag such as `v0.1.1`) of `formula`
    /// installed, whoever installed it.
    pub fn has(&self, formula: &str, version: &str) -> bool {
        names(&self.listed(formula), version)
    }

    /// Whether every program in `formula`'s keg of `version` is what PATH
    /// finds: `<brew --prefix>/bin/<name>` resolves into that keg. A keg in
    /// the cellar whose link is missing or stale (factorium#244) is linked
    /// once with `brew link --overwrite`; answers why, naming each link and
    /// where it points, when that leaves one still outside the keg.
    pub fn linked(&self, formula: &str, version: &str) -> Result<(), String> {
        if self.unlinked(formula, version)?.is_empty() {
            return Ok(());
        }
        let said = self.brew(&["link", "--overwrite", formula]).err().map(|e| format!(" ({e})")).unwrap_or_default();
        let left = self.unlinked(formula, version)?;
        if left.is_empty() {
            return Ok(());
        }
        let v = version.strip_prefix('v').unwrap_or(version);
        Err(format!(
            "Homebrew has {formula} {v} in its cellar, but {} after `brew link --overwrite {formula}`{said}",
            left.join("; ")
        ))
    }

    /// Each program in `formula`'s keg of `version` whose PATH link does not
    /// resolve into that keg, as `` `<link>` points to `<target>` ``.
    fn unlinked(&self, formula: &str, version: &str) -> Result<Vec<String>, String> {
        let cellar = self.brew(&["--cellar", formula])?;
        let keg = Path::new(cellar.trim()).join(version.strip_prefix('v').unwrap_or(version));
        let Ok(programs) = std::fs::read_dir(keg.join("bin")) else { return Ok(Vec::new()) };
        let bin = Path::new(self.brew(&["--prefix"])?.trim()).join("bin");
        let keg = keg.canonicalize().unwrap_or(keg);
        let mut names: Vec<_> = programs.flatten().map(|p| p.file_name()).collect();
        names.sort();
        let stale = |name| {
            let link = bin.join(name);
            let into = link.canonicalize().is_ok_and(|to| to.starts_with(&keg));
            let to = match std::fs::read_link(&link) {
                Ok(to) => format!("points to `{}`", to.display()),
                Err(_) if link.exists() => "is not a link".to_string(),
                Err(_) => "is missing".to_string(),
            };
            (!into).then(|| format!("`{}` {to}", link.display()))
        };
        Ok(names.into_iter().filter_map(stale).collect())
    }

    fn listed(&self, formula: &str) -> String {
        self.brew(&["list", "--versions", formula]).unwrap_or_default()
    }

    /// `; held by <pid command>` for each brew process `ps` finds on
    /// `formula`: nothing when it finds none.
    fn holder(&self, formula: &str) -> String {
        let name = formula.rsplit('/').next().unwrap_or(formula);
        let ps = self.0.run("ps", &["-Ao", "pid=,command="]);
        ps.out
            .lines()
            .map(str::trim)
            .filter(|l| l.contains("brew") && l.split_whitespace().any(|w| w.rsplit('/').next() == Some(name)))
            .map(|l| format!("; held by {l}"))
            .collect()
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
