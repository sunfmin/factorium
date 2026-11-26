# factorium

factorium runs AI coding agents on one machine while nobody watches. It
takes issues from a local tracker, gives each to a worker in its own
worktree, lands what passes the repo's gate onto a proving branch, promotes
that branch to the base once the slow gate passes, cuts versions, and keeps
the machine healthy. A resident watch agent deals with everything that goes
wrong.

- What it is and why: [SPEC.md](SPEC.md)
- What it left behind from afkconvoy: [docs/dropped.md](docs/dropped.md)

## Install

```sh
brew install sunfmin/tap/factorium
```

factorium cuts its own versions (`factorium.toml`, `packaging/release.sh`):
it builds them universal, and sunfmin/homebrew-tap's release workflow signs
them with Developer ID, notarizes and publishes them
([ADR 4](docs/adr/0004-cuts-are-signed-and-notarized-by-the-taps-workflow.md)).

The daemon runs as a launchd agent, started again whenever it exits.
Install it from a shell whose PATH finds git, orca, stablehand and the
engines' programs: that PATH is written into the agent. launchd does not load it
by itself after a reboot: a boot loads no Background agent from
~/Library/LaunchAgents, at boot or at a login, and only root's jobs run
before a login. So the first `factorium` command run after
the boot loads it, unless `factorium stop` stopped it.

```sh
factorium service install   # writes ~/Library/LaunchAgents/com.github.sunfmin.factorium.plist and loads it
factorium service show      # the definition it would write (packaging/factorium.plist, filled in)
factorium stop              # unloads it; `service install` starts it again
factorium service remove    # unloads it and deletes the file
```

It logs to `$XDG_DATA_HOME/factorium/daemon.log`. A program it needs and
cannot find at start, and ticks failing in a row (`tick_failures_max`), are
`doctor` findings, and so rounds for the watch.

## Use

```sh
factorium checkout add web ~/src/web --base main --proving proving
stablehand command set 'claude-code=claude --dangerously-skip-permissions'   # how each engine starts: stablehand's, as are which one and its model (`stablehand order set`)
factorium issue add web "Fix the login" --body-file body.md   # or --body "..."; - reads stdin
factorium run          # the daemon in the foreground (the service runs it for you)
factorium doctor       # what needs you
factorium spent web 198..222 --since 2026-11-25T00:00:00Z   # where those issues' time went
```

A repo says how it is gated and cut in `factorium.toml`:

```toml
[gate]
fast = "cargo test"
slow = "./slow.sh"

[cut]
steps = ["./bump.sh $FACTORIUM_VERSION"]
formula = "web"
```

## Develop

`./gate.sh`: fmt, clippy, the per-crate lines against their targets (shown, not held, SPEC.md §5), and the tests under slimfit (SPEC.md §6).

`./slow.sh`: the slow gate, before a promotion: `./checks.sh` (the fast
gate's checks without its test run), then `slimfit check`, which runs the
tests once, instrumented, against `.slimfit/baseline.json` (no failures, no more
uncovered lines, tests no slower than the baseline allows; SPEC.md §6).
The loop for a worker on factorium, mutation testing among it, is `slimfit --help`'s.
