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

factorium cuts its own versions (`factorium.toml`, `packaging/release.sh`).

The daemon runs as a launchd agent, started at login and again whenever it
exits. Install it from a shell whose PATH finds git, orca and the kit's
harnesses: that PATH is written into the agent.

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
factorium kit set claude-code=claude@opus codex=codex
factorium issue add web "Fix the login" --body-file body.md   # or --body "..."; - reads stdin
factorium run          # the daemon in the foreground (the service runs it for you)
factorium doctor       # what needs you
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

`./gate.sh`: fmt, clippy, the per-crate line budgets, and the tests (60s cap).

`./slow.sh`: the slow gate, before a promotion: `./checks.sh` (the fast
gate's checks without its test run), then `slimfit check`, which runs the
tests once, instrumented, against `.slimfit/baseline.json` (no failures, no more
uncovered lines, tests no slower than the baseline allows; SPEC.md §6).
The loop, for a worker on factorium:

1. `slimfit next`: one task (an uncovered line, a slow test, …) with its
   files and the command that accepts it. This is how to find what to
   test or make smaller.
2. Do that task; run its accept command (`slimfit check`).
3. `slimfit ratchet` locks the gain into `.slimfit/baseline.json`; commit
   the baseline with the code, in the same commit.

Any change that lowers uncovered lines or test time ratchets the same
way. A bound is raised only by `slimfit ratchet --loosen <bound> --reason
"<why>"`.

`./mutants.sh`: mutation testing (SPEC.md §6), too slow for the gate. Run
it after changing a rule or a scenario, before the change lands; pass
`-f <file>` to mutate only what you touched. Each mutant is caught or
missed, with its seconds; a missed one gets a scenario that catches it, or,
if it cannot be reached from the outermost layer, a line in
`.cargo/mutants.toml` saying why. It installs cargo-mutants 27.1.0 into
`~/.cargo/bin` if needed, and leaves the tree as it found it. In a worker
each shard of at most 40 mutants of one file is its own heavy command, a
few minutes long; the whole tree takes hours, so run it file by file
(`-f`), never inside one `factorium worker exclusive`.
