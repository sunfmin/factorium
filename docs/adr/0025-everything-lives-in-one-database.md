# 25. Everything factorium knows lives in one database; what a repo declares lives in the repo

Status: accepted

## Context

State kept in several places drifts: a cache directory, a remote ref and
a database each say something of the same issue, and nothing says which
one is right. A CLI that had to reach the daemon over a socket would fail
whenever the daemon is down. The daemon moves to a newer binary by itself,
with nobody there to run a migration, so a store must stay readable across
binaries as it is.

## Decision

- Everything factorium knows lives in one SQLite file,
  `$XDG_DATA_HOME/factorium/factorium.db`: factum's log, its work rows and
  its projections. The tracker, the checkout list, the kit, gate records,
  landings, promotions, cuts, the watch's episodes, rounds and reports, and
  the host's settings are all events and projections in it.
- No cache directory holds state; no remote ref holds state.
- The CLI and the daemon open the same file (WAL); a CLI command is a
  factum submission or a query, so the CLI needs no socket to the daemon.
- A store written by an older binary is read by a newer one as it is: no script is run by hand, so the daemon's proof that a new binary opens this host's database passes on it, and the daemon moves to it by itself.
- The log is never rewritten; a change to what is kept is carried by the
  code that reads it:
  - a renamed event, field or value keeps its old name as a serde alias
    wherever it is read (an event's kind, its data, an aggregate's state in
    a snapshot);
  - an event whose data changed in shape takes a factum upcast;
  - an aggregate's state that changed in shape takes a new `code()`, so a
    snapshot kept by other code is folded again;
  - a projection whose rows hold a changed value takes a new `version()`,
    so its rows are rebuilt from the log.
- What a repo declares about itself (its gate commands and limits, its cut
  steps, its idle work, its heavy commands, its proving branch) is a file
  committed in the repo, `factorium.toml`, because it changes with the code
  and is reviewed with it. It is the one exception to "everything in the
  database".

## Consequences

- One backup of one file is all the host's state; nothing else needs
  syncing with it.
- An old name stays in the code that reads it for as long as any store may
  hold it (`IssueParked` is still read as `IssueWaitingForHuman`).
