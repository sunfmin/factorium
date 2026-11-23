//! Another host, as `issue move` reaches it by ssh: a world of its own,
//! whose factorium is the built binary on that world's database, run in
//! that host's login shell.

use std::path::{Path, PathBuf};

use super::fakes::home;
use super::{World, program};

/// The programs through which the binary reaches the host's own service and
/// sessions: each a stub that fails there, so no command run there reaches
/// this machine's.
const HOST_PROGRAMS: [&str; 4] = ["launchctl", "orca", "mytokens", "brew"];

impl World {
    /// Lets `from` reach this world's host by ssh as `target`: a command
    /// runs there with `sh -c`, as sshd runs one, where the PATH has no
    /// factorium; its user's login shell (`SHELL`) puts it on PATH, as a
    /// profile that sets Homebrew up does. Its factorium is the built
    /// binary on this world's database.
    pub fn reached_by(&self, from: &World, target: &str) {
        let bin = self.dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let link = bin.join("factorium");
        if !link.exists() {
            std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_factorium"), &link).unwrap();
        }
        for p in HOST_PROGRAMS {
            program(&bin.join(p), "#!/bin/sh\nexit 1\n");
        }
        let shell = self.dir.join("login-shell");
        let script = format!(
            "#!/bin/sh\n# A login shell: its profile puts factorium on PATH.\n[ \"$1\" = -lc ] || exit 64\nPATH='{}':\"$PATH\" exec /bin/sh -c \"$2\"\n",
            bin.display()
        );
        program(&shell, &script);
        let env = [
            ("SHELL", shell),
            ("PATH", PathBuf::from("/usr/bin:/bin")),
            ("HOME", home(&self.dir)),
            (factorium::DB_ENV, self.db.clone()),
        ];
        let env = env.into_iter().map(|(k, v)| (k.to_string(), v.display().to_string())).collect();
        from.host.hosts.lock().unwrap().insert(target.into(), env);
    }

    /// A checkout `name` that is a clone of `of`'s remote `name`: the same
    /// origin, on another host.
    pub async fn checkout_of(&self, of: &World, name: &str) {
        let remote = of.dir.join("remotes").join(format!("{name}.git"));
        let path = self.dir.join("repos").join(name);
        self.git(Path::new(&self.dir), &["clone", "--quiet", &remote.to_string_lossy(), &path.to_string_lossy()])
            .unwrap();
        self.app.checkout_add(name, &path, "main", "proving", None).await.unwrap();
    }
}
