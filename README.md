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

## Use

```sh
factorium checkout add web ~/src/web --base main --proving proving
factorium kit set claude-code=claude@opus codex=codex
factorium issue add web "Fix the login" --body-file body.md   # or --body "..."; - reads stdin
factorium run          # the daemon: start workers, land, promote, cut, watch
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
