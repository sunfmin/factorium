//! The gate as the app runs it: green, red or not yet for a tree of a
//! checkout, run as a heavy command, and recorded against the tree so the
//! same tree is never gated twice. The one writer of gate records.

use factorium_domain::declared::Kind;
use factorium_domain::gate::{Gates, Ran as GateRan, RecordGate, record_id, record_key};
use factorium_domain::heavy;
use factorium_domain::named;
use factorium_host::{Git, Ran};
use factum::Uuid;

use crate::heavy::{Job, Turn};
use crate::{Answer, App, fail};

/// What a gate is run on.
pub(crate) enum At<'a> {
    /// A commit of the line's clone: the daemon asks once per tick; by hand
    /// it waits its turn. `bound_at`: when the move it is for falls due however busy.
    Commit { rev: &'a str, by_hand: bool, bound_at: Option<i64> },
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
}

impl Gated {
    /// Why it is red: the tail of its run.
    pub fn why(&self) -> String {
        let Some(ran) = &self.ran else { return String::new() };
        if ran.timed_out { format!("past its limit\n{}", ran.tail) } else { ran.tail.clone() }
    }
}

impl App {
    /// Runs the `kind` gate of `checkout` on `at` in `here`, unless that tree
    /// is green already. `None`: not its turn yet; ask again next tick.
    pub(crate) async fn gate(&self, checkout: &str, here: &Git, at: At<'_>, kind: Kind) -> Answer<Option<Gated>> {
        let (rev, dirty) = match at {
            At::Commit { rev, .. } => (rev, false),
            At::Worktree { .. } => ("HEAD", here.dirty().map_err(fail("git"))?),
        };
        let tree = here.tree(rev).map_err(fail("git"))?;
        let recorded = !dirty;
        if recorded && self.state::<Gates>(record_id(checkout, &tree, kind)).await?.green {
            return Ok(Some(Gated { green: true, tree, recorded, ran: None }));
        }
        let Some((cmd, limit)) = self.declared(here, rev)?.gate.of(kind).map(|(c, l)| (c.to_string(), l)) else {
            return Ok(Some(Gated { green: true, tree, recorded: false, ran: None }));
        };
        let steps = [cmd];
        let job = |what| Job::gate(what, &here.dir, &steps, limit);
        let (job, turn) = match at {
            At::Commit { by_hand, bound_at, .. } => {
                let turn = if by_hand { Turn::Wait } else { Turn::Once(record_key(checkout, &tree, kind)) };
                (job(heavy::LineJob::Gate(kind).of(checkout)).of(None, bound_at), turn)
            }
            At::Worktree { worker, number } => {
                (job(format!("{} gate of {checkout}#{number}", named(kind))).of(Some(worker), None), Turn::Wait)
            }
        };
        let Some(mut rans) = self.heavy(job, turn).await? else { return Ok(None) };
        let ran = rans.pop().expect("one step");
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
        Ok(Some(Gated { green: ran.ok, tree, recorded, ran: Some(ran) }))
    }
}
