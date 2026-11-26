# 30. The daemon runs only a binary proven on this host's database

Status: accepted

## Context

Nobody restarts the daemon by hand: a cut installs a new formula, and the
daemon must move to it by itself. A binary that cannot read the store
would fail every tick with nobody there, and a newer one may write an
event an older daemon cannot read.

## Decision

- A daemon Homebrew installed notes its formula's link at start; once the link names another version, it execs the link between ticks, as the same pid, once that binary is proven to open this host's database.
- The proof answers `doctor` on a copy (SQLite's `VACUUM INTO`), never the
  live file. Each binary is proven once on the host, processes taking turns
  on its lock; a tick only starts it, beside the ticks, and swaps once it
  passed. A cut proves its install right after it; one that fails is a
  failed cut, and the old binary runs on.
- A daemon of an older version whose store holds a newer event fails its ticks; a failed tick still starts that proof and swaps once it passed.
- Which host installs: a host one of whose checkouts declares the cut of the daemon's own formula (its base, as last fetched, names it in `[cut]`) installs it at that cut, and only then.
- Every other host upgrades from the tap each `upgrade_every`, while no
  line move runs; a failure is a finding (`daemon:upgrade`), as a link
  whose proof failed is (`daemon:refused:<version>`).
- A binary's first good tick keeps a copy of it (`binary/<version>/bin/`).
  A linked binary whose ticks fail `tick_failures_max` in a row before its
  first good one execs the kept copy, reading no database to decide it;
  that one records the rollback, a finding, and swaps to no link naming
  the version it left.

## Consequences

- No session stops the daemon: the guard refuses `stop` (ADR 0026).
- Scenarios: `crates/factorium/tests/scenarios/upgrade.rs`.
