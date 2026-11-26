# 16. The line's jobs run in named Orca worktrees, removed when each ends

Status: accepted

## Context

The owner, 2026-11-26: every worktree factorium makes is to be made in
Orca, saying what it is and what it is for, and removed once its job is
done, so that all that runs is visible there.

The line's jobs (a landing's merge and fast gate, a promotion's gate, a
cut's steps, a run of idle work) ran in one clone per checkout and kind
under the data directory, reused forever and kept out of Orca so that it
listed no unnamed "Detached HEAD" workspaces. Those clones held their
builds, about 20 GB each for factorium's landing and promotion.

## Decision

- Each job runs in an Orca worktree of its checkout made for it alone,
  detached at the rev it reads. As a worktree of the checkout, it sees the
  checkout's tags and `origin/*` refs, and the landing merges the worker's
  branch the checkout holds.
- Orca shows it named for its job (`landing <checkout>#<n>`,
  `promotion <checkout> <short tip>`, `cut <checkout> <version>`,
  `idle <checkout>`), with a comment saying what runs there, that
  factorium removes it when the job ends, and the pid of the process that
  made it. Named and short-lived, none is unnamed or stale.
- It is removed (`orca worktree rm`) when the job ends, green or red. A
  job waiting its turn in the heavy queue keeps it, reset, while a tick
  asks for it.
- The build cache is the one thing kept across jobs, outside the worktree:
  one per checkout and kind under the data directory
  (`cache/<kind>/<checkout>`), given to the job's steps as
  `CARGO_TARGET_DIR`. The work dir is made at one path per checkout and
  kind, as rustc's incremental cache holds absolute paths: Orca's path
  follows the name a worktree is made with (`<kind> <checkout>`), and it
  is then shown as its job's. A build tool reads a file as changed by its mtime,
  and a new worktree's are all new: as a job ends, each tracked file's
  content id and mtime are kept beside its cache, and the next job's work
  dir gives each file whose content is the same that mtime back, so the
  build finds fresh what a work dir reused across jobs would have. (Cargo's
  `-Zchecksum-freshness`, on the pinned nightly of 2026-10-22, still read
  a touched file as changed by its mtime.) A cache no variable moves is declared in `factorium.toml`
  (`cache = ["node_modules"]`), moved in at the job's start and out at its
  end. A script that reads a build from `target/` reads it from
  `${CARGO_TARGET_DIR:-target}`.
- At each tick the daemon removes a work dir factorium made whose job no
  longer runs: its process is gone, or it is the daemon's own and neither a
  job running beside the ticks nor this tick asked for it. One holding
  commits that no branch, tag or remote-tracking ref holds (a landing's
  merge aside) is kept, its comment saying so, and is a doctor finding.
- The old clones are retired once, as the daemon starts: each one's
  `target` becomes its cache, and the rest is removed.
- A worker's worktree carries a comment naming its issue and title, and
  that factorium removes it when the issue lands or closes. Any other tree
  a worker wants (to time a gate, to compare) it makes with
  `orca worktree create --comment "<what, for which issue>"` and removes
  with `orca worktree rm` before `done`, never with `git worktree add` or
  `git clone`.

## Consequences

- A gate rebuilds what changed since the last job of its kind, as it did
  in a reused clone.
- A process that dies mid-job leaves its work dir until the daemon's next
  tick.
