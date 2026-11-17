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

impl App {
    fn service_file(&self) -> PathBuf {
        self.host.home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
    }

    /// launchd's name of the agent: this user's GUI domain and the label.
    fn service_target(&self) -> String {
        use std::os::unix::fs::MetadataExt;
        // The data directory is this user's: factorium made it.
        let uid = std::fs::metadata(self.data().root()).map(|m| m.uid()).unwrap_or(0);
        format!("gui/{uid}/{LABEL}")
    }

    /// The agent's target, if launchd has it loaded.
    pub(crate) fn service_loaded(&self) -> Option<String> {
        let target = self.service_target();
        self.host.programs().run("launchctl", &["print", &target]).ok.then_some(target)
    }

    pub(crate) fn service_stop(&self, target: &str) -> Answer {
        let said = self.host.programs().run("launchctl", &["bootout", target]);
        said.result("launchctl", &["bootout", target]).map_err(crate::fail("launchctl"))?;
        Ok(json!({ "stopped": true, "service": LABEL, "again": "factorium service install" }))
    }

    /// `factorium service show`: the definition this shell would install.
    pub fn service_show(&self) -> Answer {
        Ok(json!({ "label": LABEL, "file": self.service_file(), "plist": self.service_plist() }))
    }

    fn service_plist(&self) -> String {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("factorium"));
        let mut path: Vec<PathBuf> = Vec::new();
        for d in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            if d.is_absolute() && d.is_dir() && !path.contains(&d) {
                path.push(d);
            }
        }
        let path = std::env::join_paths(path).unwrap_or_default();
        let fill = [
            ("{label}", LABEL.to_string()),
            ("{program}", stable(&exe).to_string_lossy().into_owned()),
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
            self.service_stop(&target)?;
        }
        let domain = self.service_target().rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        let args = ["bootstrap", &domain, &file.to_string_lossy()];
        self.host.programs().run("launchctl", &args).result("launchctl", &args).map_err(crate::fail("launchctl"))?;
        Ok(json!({ "installed": file, "service": LABEL }))
    }

    /// `factorium service remove`: unloads the agent and deletes its file.
    pub fn service_remove(&self) -> Answer {
        if let Some(target) = self.service_loaded() {
            self.service_stop(&target)?;
        }
        let file = self.service_file();
        let removed = std::fs::remove_file(&file).is_ok();
        Ok(json!({ "removed": removed, "file": file }))
    }
}

/// The binary as a path that outlives upgrades: Homebrew's opt link in
/// place of a versioned keg.
fn stable(exe: &Path) -> PathBuf {
    let s = exe.to_string_lossy();
    match s.split_once("/Cellar/factorium/") {
        Some((prefix, rest)) => match rest.split_once('/') {
            Some((_version, inside)) => PathBuf::from(format!("{prefix}/opt/factorium/{inside}")),
            None => exe.to_path_buf(),
        },
        None => exe.to_path_buf(),
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
