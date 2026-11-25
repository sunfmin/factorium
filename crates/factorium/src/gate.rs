//! The gate as the app runs it: green, red or not yet for a tree of a
//! checkout, run as a heavy command, and recorded against the tree so the
//! same tree is never gated twice. The one writer of gate records, and of
//! the files that keep each run's whole output.

use factorium_domain::declared::Kind;
use factorium_domain::gate::{Gates, Ran as GateRan, RecordGate, record_id, record_key};
use factorium_domain::heavy;
use factorium_domain::named;
use factorium_host::{Git, Ran};
use factum::Uuid;
use std::path::PathBuf;

use crate::heavy::{For, Job, Turn};
use crate::{Answer, App, fail};

/// What a gate is run on.
pub(crate) enum At<'a> {
    /// A commit of the line's clone, gated by hand: it waits its turn. `bound_at`: when the move
    /// it is for falls due however busy.
    Commit { rev: &'a str, bound_at: Option<i64> },
    /// A commit of the line's clone the daemon gates beside its ticks, for the line's move
    /// `moving`: it asks once per tick and, let start, answers not yet until
    /// [`App::gate_ran`] records how it ran.
    Beside { rev: &'a str, bound_at: Option<i64>, moving: Box<crate::line::Moving> },
    /// A worker's worktree as it stands, waiting its turn. A dirty one is
    /// run but not recorded: its tree is no commit's.
    Worktree { worker: Uuid, number: u64 },
}

/// A gate's answer.
pub(crate) struct Gated {
    pub green: bool,
    /// The tree it is of: the commit's, or the worktree's `HEAD`.
    pub tree: String,
    pub recorded: bool,
    /// The run, if it ran: `None` when the tree was green already.
    pub ran: Option<Ran>,
    /// Where the run's whole output is kept, or why it is not: empty when it did not run.
    pub kept: String,
}

/// How many gate runs' whole outputs a checkout keeps, the newest.
pub(crate) const OUTPUTS_KEPT: usize = 20;

impl Gated {
    /// Why it is red: the tail of its run, and the file that keeps all of it.
    pub fn why(&self) -> String {
        let Some(ran) = &self.ran else { return String::new() };
        let tail = if ran.timed_out { format!("past its limit\n{}", ran.tail) } else { ran.tail.clone() };
        format!("{tail}\n{}", self.kept)
    }
}

impl App {
    /// Runs the `kind` gate of `checkout` on `at` in `here`, unless that tree
    /// is green already. `None`: not its turn yet; ask again next tick.
    pub(crate) async fn gate(&self, checkout: &str, here: &Git, at: At<'_>, kind: Kind) -> Answer<Option<Gated>> {
        let (rev, dirty) = match at {
            At::Commit { rev, .. } | At::Beside { rev, .. } => (rev, false),
            At::Worktree { .. } => ("HEAD", here.dirty().map_err(fail("git"))?),
        };
        let tree = here.tree(rev).map_err(fail("git"))?;
        if !dirty && self.state::<Gates>(record_id(checkout, &tree, kind)).await?.green {
            return Ok(Some(Gated { green: true, tree, recorded: true, ran: None, kept: String::new() }));
        }
        let Some((cmd, limit)) = self.declared(here, rev)?.gate.of(kind).map(|(c, l)| (c.to_string(), l)) else {
            return Ok(Some(Gated { green: true, tree, recorded: false, ran: None, kept: String::new() }));
        };
        let steps = [cmd];
        let job = |what| Job::gate(what, &here.dir, &steps, limit);
        let (job, turn) = match at {
            At::Commit { bound_at, .. } => {
                (job(heavy::LineJob::Gate(kind).of(checkout)).of(None, bound_at), Turn::Wait)
            }
            At::Beside { bound_at, moving, .. } => {
                let (key, lands) = (Some(record_key(checkout, &tree, kind)), moving.lands());
                let turn = Turn::Beside { key, yields: false, of: For::Line(moving) };
                (job(heavy::LineJob::Gate(kind).of(checkout)).of(None, bound_at).landing(lands), turn)
            }
            At::Worktree { worker, number } => {
                (job(heavy::LineJob::Gate(kind).of(&format!("{checkout}#{number}"))).of(Some(worker), None), Turn::Wait)
            }
        };
        let beside = matches!(turn, Turn::Beside { .. });
        let Some(mut rans) = self.heavy(job, turn).await?.filter(|_| !beside) else { return Ok(None) };
        let ran = rans.pop().expect("one step");
        self.gate_ran(checkout, here, rev, dirty, kind, ran).await.map(Some)
    }

    /// Records how the `kind` gate of `checkout` ran on `rev` in `here` (unless `dirty`: its tree
    /// is no commit's), and keeps its whole output; answers what it says.
    pub(crate) async fn gate_ran(
        &self,
        checkout: &str,
        here: &Git,
        rev: &str,
        dirty: bool,
        kind: Kind,
        ran: Ran,
    ) -> Answer<Gated> {
        let (tree, recorded) = (here.tree(rev).map_err(fail("git"))?, !dirty);
        if recorded {
            let record = GateRan {
                checkout: checkout.into(),
                tree: tree.clone(),
                kind,
                green: ran.ok,
                took_ms: ran.took_ms,
                tail: ran.tail.clone(),
            };
            self.submit::<RecordGate>(record).await?;
        }
        let commit = here.sha(rev).map(|sha| if dirty { format!("{sha}-dirty") } else { sha });
        let kept = commit.and_then(|commit| self.keep_output(checkout, kind, &commit, &ran.said));
        let kept =
            kept.map_or_else(|e| format!("whole output not kept: {e}"), |p| format!("whole output: {}", p.display()));
        Ok(Gated { green: ran.ok, tree, recorded, ran: Some(ran), kept })
    }

    /// Keeps `said`, the whole output of `checkout`'s `kind` gate on
    /// `commit`, in a file of its own; removes the oldest past [`OUTPUTS_KEPT`].
    fn keep_output(&self, checkout: &str, kind: Kind, commit: &str, said: &str) -> Result<PathBuf, String> {
        let dir = self.data().gate_outputs(checkout);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        let path = dir.join(format!("{:013}-{}-{commit}.log", self.now(), named(kind)));
        std::fs::write(&path, said).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        let mut kept: Vec<PathBuf> =
            std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        kept.sort();
        for old in &kept[..kept.len().saturating_sub(OUTPUTS_KEPT)] {
            let _ = std::fs::remove_file(old);
        }
        Ok(path)
    }
}
