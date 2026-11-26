# 17. The line's jobs run in named Orca worktrees, removed when each ends

Status: accepted

## Context

The owner, 2026-11-26: every worktree factorium makes is made in Orca,
saying what it is and what it is for, and removed once its job is done.

The line's jobs ran in one clone per checkout and kind under the data
directory, reused forever and kept out of Orca so that it listed no
unnamed "Detached HEAD" workspaces.

## Decision

- Each job (a landing, a promotion, a cut, a run of idle work) runs in an
  Orca worktree of its checkout made for it, detached at the rev it reads,
  and removed when it ends, green or red. It sees the checkout's tags,
  `origin/*` refs and worker branches.
- Orca shows it as `landing <checkout>#<n>`, `promotion <checkout> <tip>`,
  `cut <checkout> <version>` or `idle <checkout>`, with a comment saying
  what runs there, that factorium removes it, and its maker's pid.
- The build cache is kept outside it, per checkout and kind under the
  data directory: its `target`, and the dirs `factorium.toml` declares
  `cache`, moved into the work dir at the job's start and out at its end.
  No `CARGO_TARGET_DIR` is set: a job builds where a worker's own run does,
  and scripts that read `target/` need no change.
- So the cache is as fresh as a reused clone's: the worktree is made at
  one path per checkout and kind (made as `<kind> <checkout>`, then shown
  as its job's), as rustc's incremental cache holds absolute paths; and a
  file whose content the last job left gets back that job's mtime.
  Cargo's `-Zchecksum-freshness` still read mtimes on the pinned nightly.
- Each tick, the daemon removes a work dir whose job no longer runs. One
  holding commits no ref holds (a landing's merge aside) is kept, its
  comment saying so, and doctor names it.
- The old clones are retired once, as the daemon starts: each `target`
  becomes its cache.
- A worker's worktree is commented with its issue. Any other tree a
  worker wants it makes with `orca worktree create --comment` and removes
  with `orca worktree rm`, never `git worktree add` or `git clone`.

## Consequences

- A gate rebuilds what changed since the last job of its kind.
- A process that dies mid-job leaves its work dir until the next tick.
