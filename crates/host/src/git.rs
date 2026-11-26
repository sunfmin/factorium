//! git, as a program: every call runs `git` in a directory as the host runs
//! it (or answers as it would, where that is certain: `refs`) — with the host's home, so with the config that home holds and no
//! other — and answers what it printed, or why it failed. The app makes one
//! through [`crate::Host`]; a checkout's repo is one value, and what the app
//! does to it (fetch a tip, a work dir detached, a push that requires what it
//! read, a file at a rev, a tag) is a method here, not git arguments there.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};

use crate::Clock;

mod refs;

/// The git program, found once: the binary in the exec path of the `git` on
/// PATH. On macOS `/usr/bin/git` is a shim that looks Xcode's git up on every
/// call, which more than doubled a call's time (6.7ms against 15.2ms,
/// factorium#98); the exec path holds the binary it runs. `git` when there is
/// none.
fn binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let out = Command::new("git").arg("--exec-path").stdin(Stdio::null()).output();
        let dir =
            out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        dir.map(|d| Path::new(&d).join("git")).filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from("git"))
    })
}

/// Why a branch does not merge: git's own words, where git refused or
/// failed, and every conflicting file. Said as one line by its `Display`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoMerge {
    pub git: Option<String>,
    pub files: Vec<String>,
}

impl NoMerge {
    fn git(e: String) -> NoMerge {
        NoMerge { git: Some(e), files: Vec::new() }
    }
}

impl std::fmt::Display for NoMerge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let files = (!self.files.is_empty()).then(|| format!("conflicts in: {}", self.files.join(", ")));
        match (&self.git, files) {
            (Some(e), Some(files)) => write!(f, "{e}; {files}"),
            (Some(e), None) => f.write_str(e),
            (None, files) => f.write_str(&files.unwrap_or_default()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Git {
    pub dir: PathBuf,
    home: PathBuf,
    /// What dates the commits, merges and tags it makes; git's own clock if none.
    clock: Option<Arc<dyn Clock>>,
}

impl Git {
    /// git in `dir` as the host whose home is `home` runs it.
    pub fn new(home: impl Into<PathBuf>, dir: impl Into<PathBuf>) -> Git {
        Git { dir: dir.into(), home: home.into(), clock: None }
    }

    /// The same git, dating what it commits by `clock`.
    pub fn dated(self, clock: Arc<dyn Clock>) -> Git {
        Git { clock: Some(clock), ..self }
    }

    /// git in `dir`, as the same host.
    pub fn at(&self, dir: impl Into<PathBuf>) -> Git {
        Git { dir: dir.into(), ..self.clone() }
    }

    /// Runs git; its output trimmed, or its error.
    pub fn run(&self, args: &[&str]) -> Result<String, String> {
        self.run_with(args, &[])
    }

    /// [`Git::run`], with `env` added to git's environment.
    fn run_with(&self, args: &[&str], env: &[(&str, &Path)]) -> Result<String, String> {
        self.run_coded(args, env, None).map(|(_, said)| said)
    }

    /// [`Git::run_with`], exiting `also` answered as well, with its exit code.
    fn run_coded(&self, args: &[&str], env: &[(&str, &Path)], also: Option<i32>) -> Result<(i32, String), String> {
        // What factorium itself commits (a landing merge, a cut tag, the
        // watch's home) needs an identity; a host with none commits as factorium.
        let commits = matches!(args.first(), Some(&("commit" | "commit-tree" | "merge" | "tag")));
        let as_factorium = commits && !self.holds(&["config", "user.email"]);
        let date = self.clock.as_ref().filter(|_| commits).map(|c| format!("@{} +0000", c.now().div_euclid(1000)));
        let output = |program: &Path| {
            let mut git = Command::new(program);
            // The host's own config is what its home holds.
            git.env("HOME", &self.home).env_remove("XDG_CONFIG_HOME").env_remove("GIT_CONFIG_GLOBAL");
            git.envs(env.iter().copied());
            if let Some(d) = &date {
                git.env("GIT_AUTHOR_DATE", d).env("GIT_COMMITTER_DATE", d);
            }
            if as_factorium {
                for v in ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"] {
                    git.env(v, "factorium");
                }
                for v in ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"] {
                    git.env(v, "factorium@localhost");
                }
            }
            git.args(args).current_dir(&self.dir).env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null()).output()
        };
        // A binary found once may be gone since (git upgraded): `git` on PATH then.
        let out = match output(binary()) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => output(Path::new("git")),
            out => out,
        }
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
        let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let code = out.status.code();
        if out.status.success() || (also.is_some() && code == also) {
            Ok((code.unwrap_or(0), said))
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(format!("git {}: {}", args.join(" "), if err.is_empty() { said } else { err }))
        }
    }

    /// Whether git answers yes (exit 0).
    pub fn holds(&self, args: &[&str]) -> bool {
        self.run(args).is_ok()
    }

    /// Makes `dir` a repo of one empty commit, unless it is one.
    pub fn init(&self, message: &str) -> Result<(), String> {
        if self.dir.join(".git").exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        self.run(&["init", "--quiet"])?;
        self.run(&["commit", "--quiet", "--allow-empty", "-m", message]).map(|_| ())
    }

    /// The commit `rev` names.
    pub fn sha(&self, rev: &str) -> Result<String, String> {
        if let Some(id) = refs::read(&self.dir, rev) {
            return Ok(id);
        }
        self.run(&["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
    }

    /// The tree of `rev`.
    pub fn tree(&self, rev: &str) -> Result<String, String> {
        self.run(&["rev-parse", &format!("{rev}^{{tree}}")])
    }

    /// The text of `file` at `rev`, if it has one.
    pub fn shown(&self, rev: &str, file: &str) -> Option<String> {
        let Some(id) = refs::object_id(rev).or_else(|| refs::read(&self.dir, rev)) else {
            return self.run(&["show", &format!("{rev}:{file}")]).ok();
        };
        let at = format!("{id}:{file}");
        // A commit that lacks the file lacks it for good.
        let lacks = |e: &str| e.contains("does not exist in") || e.contains("exists on disk, but not in");
        refs::once(&self.dir, &format!("show {at}"), lacks, || self.run(&["show", &at])).ok()
    }

    /// Fetches `branch` from origin, and origin's tags with it; answers its
    /// tip there. The tags come along so that what gates in a clone of this
    /// repo sees the refs a worker's worktree sees (factorium#109). A tag
    /// here that origin moved is left as it is: the branch is still fetched.
    pub fn fetch(&self, branch: &str) -> Result<String, String> {
        self.fetch_all(&[branch]).map(|mut tips| tips.remove(0))
    }

    /// [`Git::fetch`] of each of `branches` at once, in one fetch; answers
    /// their tips, in order. Refused, nothing fetched, if origin lacks one.
    pub fn fetch_all(&self, branches: &[&str]) -> Result<Vec<String>, String> {
        let refspecs: Vec<String> =
            branches.iter().map(|b| format!("+refs/heads/{b}:refs/remotes/origin/{b}")).collect();
        let fetch = |tags: &[&str]| {
            let args = ["fetch", "--quiet"].iter().chain(tags).copied().chain(["origin"]);
            self.run(&args.chain(refspecs.iter().map(String::as_str)).collect::<Vec<_>>())
        };
        match fetch(&["--tags"]) {
            // Origin has no such branch: fetching it without the tags finds none either.
            Err(e) if e.contains("couldn't find remote ref") => return Err(e),
            Err(_) => {
                fetch(&[])?;
            }
            Ok(_) => {}
        }
        branches.iter().map(|b| self.sha(&format!("refs/remotes/origin/{b}"))).collect()
    }

    /// Whether `a` is an ancestor of `b`.
    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        self.holds(&["merge-base", "--is-ancestor", a, b])
    }

    /// Pushes `sha` to origin's `branch`, but only while `branch` there is
    /// still `expected` (`None`: absent). The push requires what it read.
    pub fn push_requiring(&self, sha: &str, branch: &str, expected: Option<&str>) -> Result<(), String> {
        let lease = format!("--force-with-lease=refs/heads/{branch}:{}", expected.unwrap_or(""));
        self.run(&["push", "--quiet", &lease, "origin", &format!("{sha}:refs/heads/{branch}")]).map(|_| ())
    }

    /// This worktree, detached on `rev` as git checks it out, nothing else
    /// in it: the branch it was made on (Orca makes each on one) deleted.
    /// A worktree of the checkout sees its tags and remote-tracking refs
    /// as the checkout has them, so a gate here measures against the base
    /// the checkout has.
    pub fn detached(&self, rev: &str) -> Result<(), String> {
        let branch = self.run(&["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
        self.run(&["checkout", "--quiet", "--detach", "--force", rev])?;
        self.run(&["clean", "-fdq"])?;
        match branch {
            Some(b) => self.run(&["branch", "-D", &b]).map(|_| ()),
            None => Ok(()),
        }
    }

    /// Each file tracked here as it stands in the work tree: its path and
    /// the id git gives its content (a file changed since checkout hashed
    /// anew; one deleted left out).
    pub fn contents(&self) -> Result<Vec<(String, String)>, String> {
        let staged = self.run(&["ls-files", "-s", "-z"])?;
        // `<mode> <id> <stage>\t<path>`, each ended by NUL.
        let mut ids: std::collections::BTreeMap<String, String> = staged
            .split('\0')
            .filter_map(|e| {
                let (meta, path) = e.split_once('\t')?;
                Some((path.to_string(), meta.split(' ').nth(1)?.to_string()))
            })
            .collect();
        let changed = self.run(&["diff", "--name-only", "-z"])?;
        let changed: Vec<&str> = changed.split('\0').filter(|p| !p.is_empty()).collect();
        let (here, gone): (Vec<&str>, Vec<&str>) = changed.into_iter().partition(|p| self.dir.join(p).is_file());
        for path in gone {
            ids.remove(path);
        }
        if !here.is_empty() {
            let mut args = vec!["hash-object", "--"];
            args.extend(&here);
            let hashed = self.run(&args)?;
            for (path, id) in here.into_iter().zip(hashed.lines()) {
                ids.insert(path.to_string(), id.to_string());
            }
        }
        Ok(ids.into_iter().collect())
    }

    /// The commits `HEAD` holds that no branch, tag or remote-tracking ref
    /// holds, newest first, as `<sha> <subject>`.
    pub fn own_commits(&self) -> Result<Vec<String>, String> {
        let listed = self.run(&["log", "--format=%H %s", "HEAD", "--not", "--branches", "--tags", "--remotes"])?;
        Ok(listed.lines().map(str::to_string).collect())
    }

    /// Merges `branch`, of this repo (a worktree shares its branches), here
    /// as one merge commit with `message`; answers it and the conflicting
    /// paths it took this side's copy of. Those are taken only when every conflicting path matches a
    /// glob pathspec of `regenerated`; any other conflict aborts the merge.
    pub fn merge(&self, branch: &str, message: &str, regenerated: &[String]) -> Result<(String, Vec<String>), NoMerge> {
        let merged = self.run(&["merge", "--no-ff", "--no-edit", "-m", message, branch]);
        let Err(e) = merged else { return Ok((self.sha("HEAD").map_err(NoMerge::git)?, Vec::new())) };
        let unmerged = |specs: &[String]| -> Vec<String> {
            let mut args = vec!["diff", "--name-only", "--diff-filter=U", "--"];
            args.extend(specs.iter().map(String::as_str));
            self.run(&args).unwrap_or_default().lines().map(str::to_string).collect()
        };
        let files = unmerged(&[]);
        let globs: Vec<String> = regenerated.iter().map(|g| format!(":(glob){g}")).collect();
        if !files.is_empty() && !globs.is_empty() && unmerged(&globs) == files {
            let mut take = vec!["checkout", "HEAD", "--"];
            take.extend(files.iter().map(String::as_str));
            let mut add = vec!["add", "--"];
            add.extend(files.iter().map(String::as_str));
            let taken =
                self.run(&take).and_then(|_| self.run(&add)).and_then(|_| self.run(&["commit", "-q", "-m", message]));
            if taken.is_ok() {
                return Ok((self.sha("HEAD").map_err(NoMerge::git)?, files));
            }
        }
        let _ = self.run(&["merge", "--abort"]);
        Err(NoMerge { git: Some(e), files })
    }

    /// Whether `branch` merges onto `tip` here, without a work tree or a
    /// commit (`git merge-tree`), as [`Git::merge`] would merge it: one
    /// whose every conflicting path matches a glob pathspec of `regenerated`
    /// merges. Answers why not, naming every conflicting file.
    pub fn merges(&self, tip: &str, branch: &str, regenerated: &[String]) -> Result<(), NoMerge> {
        let args = ["merge-tree", "--write-tree", "--name-only", "--no-messages", tip, branch];
        // 0: it merges; 1: it conflicts, its tree then each conflicting path.
        let (code, said) = self.run_coded(&args, &[], Some(1)).map_err(NoMerge::git)?;
        if code == 0 {
            return Ok(());
        }
        let mut lines = said.lines().map(str::trim).filter(|l| !l.is_empty());
        let tree = lines.next().unwrap_or_default().to_string();
        let mut files: Vec<String> = lines.map(str::to_string).collect();
        files.dedup();
        if !files.is_empty() && !regenerated.is_empty() {
            let globs: Vec<String> = regenerated.iter().map(|g| format!(":(glob){g}")).collect();
            // The merged tree's paths that match, as a diff from the empty tree lists them:
            // `ls-tree` takes no glob pathspec.
            let empty = self.run(&["hash-object", "-t", "tree", "--stdin"]).map_err(NoMerge::git)?;
            let mut list = vec!["diff", "--name-only", empty.as_str(), tree.as_str(), "--"];
            list.extend(globs.iter().map(String::as_str));
            let records = self.run(&list).map_err(NoMerge::git)?;
            if files.iter().all(|f| records.lines().any(|r| r == f)) {
                return Ok(());
            }
        }
        Err(NoMerge { git: None, files })
    }

    /// Amends `HEAD` with `paths` as they stand in the work tree, and
    /// `message`; answers the new commit.
    pub fn amend(&self, paths: &[String], message: &str) -> Result<String, String> {
        let mut add = vec!["add", "--"];
        add.extend(paths.iter().map(String::as_str));
        self.run(&add)?;
        self.run(&["commit", "-q", "--amend", "-m", message])?;
        self.sha("HEAD")
    }

    /// Tags `sha` as `version` and pushes the tag to origin. A tag origin
    /// refused is deleted here: a tag the repo has is one origin has.
    pub fn tag_pushed(&self, version: &str, sha: &str) -> Result<(), String> {
        self.run(&["tag", "-a", version, "-m", version, sha])?;
        let pushed = self.run(&["push", "--quiet", "origin", &format!("refs/tags/{version}")]).map(|_| ());
        if pushed.is_err() {
            let _ = self.run(&["tag", "-d", version]);
        }
        pushed
    }

    /// Removes the worktree at `path`; one already gone is removed.
    pub fn worktree_remove(&self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            let _ = self.run(&["worktree", "prune"]);
            return Ok(());
        }
        self.run(&["worktree", "remove", "--force", &path.to_string_lossy()]).map(|_| ())
    }

    /// The worktrees git holds of this repo, other than its own, each with
    /// its branch (`None`: detached).
    pub fn worktrees(&self) -> Result<Vec<(PathBuf, Option<String>)>, String> {
        let listed = self.run(&["worktree", "list", "--porcelain"])?;
        let tree = |entry: &str| {
            let path = entry.lines().find_map(|l| l.strip_prefix("worktree "))?;
            let branch = entry.lines().find_map(|l| l.strip_prefix("branch refs/heads/"));
            Some((PathBuf::from(path), branch.map(str::to_string)))
        };
        Ok(listed.split("\n\n").skip(1).filter_map(tree).collect())
    }

    /// Forgets a worker's worktree at `path` and its `branch`, whatever of
    /// them is left.
    pub fn forget(&self, path: &Path, branch: &str) {
        let _ = self.worktree_remove(path);
        let _ = self.run(&["branch", "-D", branch]);
    }

    /// Whether the worktree has changes not committed.
    pub fn dirty(&self) -> Result<bool, String> {
        Ok(!self.run(&["status", "--porcelain", "--untracked-files=normal"])?.is_empty())
    }

    /// The commits of `head` not in `base`, newest first, one line each.
    pub fn commits(&self, base: &str, head: &str) -> Result<Vec<String>, String> {
        let log = self.run(&["log", "--oneline", "--no-decorate", &format!("{base}..{head}")])?;
        Ok(log.lines().map(str::to_string).collect())
    }

    /// The commits of `head` not in `base` along its first parents (its
    /// landings, on a line), newest first, one line each.
    pub fn landings(&self, base: &str, head: &str) -> Result<Vec<String>, String> {
        let log = self.run(&["log", "--first-parent", "--oneline", "--no-decorate", &format!("{base}..{head}")])?;
        Ok(log.lines().map(str::to_string).collect())
    }

    /// A commit of everything this worktree holds: `HEAD`, and on top of it,
    /// if it has changes not committed (those git does not track and does
    /// not ignore among them), one commit of them with `message`. Its work
    /// tree, index and branch are left as they are: the commit is made
    /// through an index of its own. Answers the commit.
    pub fn snapshot(&self, message: &str) -> Result<String, String> {
        let head = self.sha("HEAD")?;
        if !self.dirty()? {
            return Ok(head);
        }
        let index = PathBuf::from(self.run(&["rev-parse", "--absolute-git-dir"])?).join("factorium-snapshot-index");
        let own = [("GIT_INDEX_FILE", index.as_path())];
        let made = (|| {
            self.run_with(&["read-tree", &head], &own)?;
            self.run_with(&["add", "--all"], &own)?;
            let tree = self.run_with(&["write-tree"], &own)?;
            self.run(&["commit-tree", &tree, "-p", &head, "-m", message])
        })();
        let _ = std::fs::remove_file(&index);
        made
    }

    /// The tip of `branch` on origin, without fetching it; `None`: origin has none.
    pub fn origin_tip(&self, branch: &str) -> Result<Option<String>, String> {
        let listed = self.run(&["ls-remote", "origin", &format!("refs/heads/{branch}")])?;
        Ok(listed.split_whitespace().next().map(str::to_string))
    }

    /// The URL of origin.
    pub fn origin(&self) -> Result<String, String> {
        self.run(&["remote", "get-url", "origin"])
    }

    /// The files with changes not committed: changed since `HEAD`, staged or
    /// not, then those git does not track (and does not ignore).
    pub fn changes(&self) -> Result<Vec<String>, String> {
        let changed = self.run(&["diff", "--name-only", "HEAD"])?;
        let untracked = self.run(&["ls-files", "--others", "--exclude-standard"])?;
        Ok(changed.lines().chain(untracked.lines()).map(str::to_string).collect())
    }

    /// The commits of `head` not in `base`.
    pub fn ahead(&self, base: &str, head: &str) -> Result<u64, String> {
        self.run(&["rev-list", "--count", &format!("{base}..{head}")])?.parse().map_err(|e| format!("{e}"))
    }

    /// Origin's tags that look like versions, newest by version first.
    pub fn version_tags(&self) -> Vec<String> {
        let _ = self.run(&["fetch", "--quiet", "--tags", "origin"]);
        self.run(&["tag", "--list", "v[0-9]*", "--sort=-version:refname"])
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// The local branch checked out here, if any.
    pub fn branch(&self) -> Option<String> {
        self.run(&["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()
    }
}

/// How a local branch stands to the tip it follows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lag {
    /// At the tip, ahead of it only, or absent: nothing to do.
    Even,
    /// Behind the tip by this many commits, and nothing of its own: it can
    /// fast-forward.
    Behind(u64),
    /// Commits of its own not on the tip: left as it is.
    Diverged { behind: u64, ours: u64 },
    /// Behind, but checked out over changes not committed: left as it is.
    Dirty { behind: u64 },
}

impl Git {
    /// The worktree where local `branch` is checked out, if one has it.
    fn checked_out(&self, branch: &str) -> Option<PathBuf> {
        let list = self.run(&["worktree", "list", "--porcelain"]).ok()?;
        let mut path = None;
        for line in list.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if line.strip_prefix("branch refs/heads/") == Some(branch) {
                return path;
            }
        }
        None
    }

    /// How local `branch` stands to `tip`.
    pub fn lag(&self, branch: &str, tip: &str) -> Result<Lag, String> {
        // Both sides in one git: a tick asks this of every checkout's base.
        let local = format!("refs/heads/{branch}");
        let ends = refs::read(&self.dir, &local).zip(refs::object_id(tip).or_else(|| refs::read(&self.dir, tip)));
        let range = ends.as_ref().map_or_else(|| format!("{local}...{tip}"), |(at, tip)| format!("{at}...{tip}"));
        let count = || self.run(&["rev-list", "--left-right", "--count", &range]);
        let counted = match ends {
            // Fixed by the two commits: asked once.
            Some(_) => refs::once(&self.dir, &format!("rev-list --left-right --count {range}"), |_| false, count),
            None => count(),
        };
        let counted = match counted {
            Ok(counted) => counted,
            Err(_) if self.sha(&local).is_err() => return Ok(Lag::Even),
            Err(e) => return Err(e),
        };
        let mut sides = counted.split_whitespace().map(str::parse::<u64>);
        let (Some(Ok(ours)), Some(Ok(behind))) = (sides.next(), sides.next()) else {
            return Err(format!("git rev-list --left-right --count {local}...{tip}: answered {counted:?}"));
        };
        if behind == 0 {
            return Ok(Lag::Even);
        }
        if ours > 0 {
            return Ok(Lag::Diverged { behind, ours });
        }
        match self.checked_out(branch) {
            Some(tree) if self.at(tree.clone()).dirty()? => Ok(Lag::Dirty { behind }),
            _ => Ok(Lag::Behind(behind)),
        }
    }

    /// Fast-forwards local `branch` to `tip`, and no further, where nothing
    /// is lost: the worktree it is checked out in moves with it; one with
    /// commits of its own or checked out over changes is left as it is.
    /// Answers how it stood.
    pub fn follow(&self, branch: &str, tip: &str) -> Result<Lag, String> {
        let lag = self.lag(branch, tip)?;
        if let Lag::Behind(_) = lag {
            let tip = self.sha(tip)?;
            match self.checked_out(branch) {
                Some(tree) => self.at(tree).run(&["merge", "--ff-only", "--quiet", &tip])?,
                None => {
                    // Required to be what it was read as: one moved since is left.
                    let at = self.sha(&format!("refs/heads/{branch}"))?;
                    if !self.is_ancestor(&at, &tip) {
                        return self.lag(branch, &tip);
                    }
                    self.run(&["update-ref", &format!("refs/heads/{branch}"), &tip, &at])?
                }
            };
        }
        Ok(lag)
    }
}
