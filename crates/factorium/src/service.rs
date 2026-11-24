//! The daemon as a launchd agent: its definition, filled in from the
//! installing shell, and the launchctl calls that load and unload it. launchd
//! keeps it running; a cut's restart execs in place, as the same pid, so
//! launchd keeps tracking it.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::{Answer, App, Refusal};

/// The agent's label, and its file's name in ~/Library/LaunchAgents.
pub const LABEL: &str = "com.github.sunfmin.factorium";

/// The definition, with `{label}`, `{program}`, `{path}`, `{db}` and `{log}`
/// to fill in.
const TEMPLATE: &str = include_str!("../../../packaging/factorium.plist");

/// How long install waits for launchd to drop an agent it booted out, in
/// tenths of a second: bootout of a running job returns before launchd has
/// dropped it (1.24 s seen), and a bootstrap inside that window fails.
const DROP_TENTHS: u32 = 100;

impl App {
    fn service_file(&self) -> PathBuf {
        self.host.home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
    }

    /// One of this user's launchd domains, `user/<uid>` or `gui/<uid>`.
    /// The agent is a job of `user/<uid>`, which launchd keeps with no
    /// desktop login; `gui/<uid>` lasts only while a desktop session does.
    fn service_domain(&self, kind: &str) -> String {
        use std::os::unix::fs::MetadataExt;
        // The data directory is this user's: factorium made it.
        let uid = std::fs::metadata(self.data().root()).map(|m| m.uid()).unwrap_or(0);
        format!("{kind}/{uid}")
    }

    /// launchd's name of the agent: this user's own domain and the label.
    fn service_target(&self) -> String {
        format!("{}/{LABEL}", self.service_domain("user"))
    }

    /// The agent's target, if launchd has it loaded.
    pub(crate) fn service_loaded(&self) -> Option<String> {
        let target = self.service_target();
        self.host.programs().run("launchctl", &["print", &target]).ok.then_some(target)
    }

    /// Unloads the agent, and keeps that it was stopped on purpose, so that
    /// no command loads it back ([`Self::service_revive`]) before an install.
    pub(crate) fn service_stop(&self, target: &str) -> Answer {
        self.service_bootout(target)?;
        let _ = std::fs::write(self.data().stopped(), "");
        Ok(json!({ "stopped": true, "service": LABEL, "again": "factorium service install" }))
    }

    fn service_bootout(&self, target: &str) -> Result<(), Refusal> {
        let said = self.host.programs().run("launchctl", &["bootout", target]);
        said.result("launchctl", &["bootout", target]).map_err(crate::fail("launchctl"))?;
        Ok(())
    }

    fn service_bootstrap(&self, file: &Path) -> Result<(), Refusal> {
        let domain = self.service_domain("user");
        let args = ["bootstrap", &domain, &file.to_string_lossy()];
        self.host.programs().run("launchctl", &args).result("launchctl", &args).map_err(crate::fail("launchctl"))?;
        Ok(())
    }

    /// Loads the installed agent again when launchd lost it: a boot loads no
    /// Background agent from ~/Library/LaunchAgents into `user/<uid>`, at
    /// boot or at a login, and only root's jobs run before one
    /// (factorium#287). Answers its target when it loaded it; nothing when
    /// none is installed, launchd holds it, a daemon runs, or `stop` stopped it.
    pub fn service_revive(&self) -> Result<Option<String>, Refusal> {
        let file = self.service_file();
        // In this order: the launchctl call last, as the dearest.
        if !file.exists()
            || self.data().stopped().exists()
            || self.daemon_pid().is_some()
            || self.service_loaded().is_some()
        {
            return Ok(None);
        }
        self.service_bootstrap(&file)?;
        Ok(Some(self.service_target()))
    }

    /// Boots `target` out and waits, within a bound, until launchd no longer
    /// finds it, so the bootstrap that follows does not meet it (factorium#215).
    fn service_unload(&self, target: &str) -> Result<(), Refusal> {
        self.service_bootout(target)?;
        for waited in 0..=DROP_TENTHS {
            if !self.host.programs().run("launchctl", &["print", target]).ok {
                return Ok(());
            }
            if waited < DROP_TENTHS {
                self.host.programs().run("sleep", &["0.1"]);
            }
        }
        let secs = DROP_TENTHS / 10;
        Err(Refusal::new("launchctl", format!("launchd still holds {target} {secs} s after its bootout")))
    }

    /// `factorium service show`: the definition this shell would install.
    pub fn service_show(&self) -> Answer {
        Ok(json!({ "label": LABEL, "file": self.service_file(), "plist": self.service_plist() }))
    }

    fn service_plist(&self) -> String {
        // As Homebrew links it, so the agent outlives upgrades; else as it is.
        let exe = self.linked().or_else(|| std::env::current_exe().ok()).unwrap_or_else(|| PathBuf::from("factorium"));
        let mut path: Vec<PathBuf> = Vec::new();
        for d in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            if d.is_absolute() && d.is_dir() && !path.contains(&d) {
                path.push(d);
            }
        }
        let path = std::env::join_paths(path).unwrap_or_default();
        let fill = [
            ("{label}", LABEL.to_string()),
            ("{program}", exe.to_string_lossy().into_owned()),
            ("{path}", path.to_string_lossy().into_owned()),
            ("{db}", self.db.to_string_lossy().into_owned()),
            ("{log}", self.data().log().to_string_lossy().into_owned()),
        ];
        fill.iter().fold(TEMPLATE.to_string(), |t, (k, v)| t.replace(k, &escape(v)))
    }

    /// `factorium service install`: writes the agent with this shell's PATH
    /// and loads it, in place of one loaded before.
    pub fn service_install(&self) -> Answer {
        if self.service_loaded().is_none() && self.daemon_pid().is_some() {
            return Err(Refusal::new("running", "a daemon runs by hand: `factorium stop` it first"));
        }
        let file = self.service_file();
        let io = |e: std::io::Error| Refusal::new("io", format!("{}: {e}", file.display()));
        std::fs::create_dir_all(file.parent().unwrap_or(Path::new("."))).map_err(io)?;
        std::fs::write(&file, self.service_plist()).map_err(io)?;
        if let Some(target) = self.service_loaded() {
            self.service_unload(&target)?;
        }
        // One loaded in the GUI domain, as factorium once installed it, goes:
        // it would run a second daemon while a desktop session lasts.
        let gui = format!("{}/{LABEL}", self.service_domain("gui"));
        if self.host.programs().run("launchctl", &["print", &gui]).ok {
            self.service_unload(&gui)?;
        }
        self.service_bootstrap(&file)?;
        let _ = std::fs::remove_file(self.data().stopped());
        Ok(json!({ "installed": file, "service": LABEL }))
    }

    /// `factorium service remove`: unloads the agent and deletes its file.
    pub fn service_remove(&self) -> Answer {
        if let Some(target) = self.service_loaded() {
            self.service_stop(&target)?;
        }
        let file = self.service_file();
        let removed = std::fs::remove_file(&file).is_ok();
        let _ = std::fs::remove_file(self.data().stopped());
        Ok(json!({ "removed": removed, "file": file }))
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
