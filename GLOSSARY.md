# Glossary

## The concepts

factorium has these concepts and no others: adding one needs the owner's
approval and an entry here ([ADR 0007](docs/adr/0007-the-concepts-are-fixed.md)).
Each command's `--help` says how it works; the ADRs say why.

1. **Host**: the one machine and its one user account. One daemon
   (`factorium run`) and one SQLite database hold everything factorium
   knows of it.
2. **Checkout**: a repo on the host's checkout list: its path, base branch,
   proving branch, its default role, if any, and what the repo declares in
   its `factorium.toml`.
3. **Issue**: a piece of work in the host's tracker: title, body, the files
   it names, what blocks it, comments, and whether it is ready, waiting for
   a human, or closed. It may name the role its worker plays and the
   account it starts on ([ADR 0008](docs/adr/0008-roles-and-waiting-issues.md)).
4. **Worker**: one harness session working one issue in its own worktree,
   branched from the proving tip. It ends with `factorium worker done`,
   `factorium worker escalate`, or by failing.
5. **Harness**: a coding agent program factorium starts (Claude Code,
   Codex, pi, DSH, Kimi Code, Grok, Antigravity CLI). The session file is
   the turn.
6. **Kit**: the checkouts pinned to one engine (`kit pin`).
7. **Account**: an engine subscription, kept in stablehand, its one home
   ([ADR 0009](docs/adr/0009-accounts-are-stablehands.md)).
8. **Admission**: whether one more worker may start now, from the host's
   readings alone; no concurrency number
   ([ADR 0010](docs/adr/0010-admission-and-the-heavy-queue.md)).
9. **Resource** and **heavy command**: a resource is a name and a capacity
   rule, with one queue each: a named resource of `slots = n`, or `host`,
   as many as the host's readings allow, which is the heavy queue. A heavy
   command asks for one or more (`worker exclusive`, `heavy run`, every
   gate run; none named is `host`)
   ([ADR 0011](docs/adr/0011-resources-queue-shortest-hold-first.md)).
10. **Hot worker**: a worker whose processes take more than their share of
    CPU, memory, disk written, disk held or process creation, or that
    stalled holding build locks. Its evidence is taken at once; it is a
    finding for the watch.
11. **Gate**: the repo's own fast and slow commands, each with its limit.
12. **Landing**: a worker's `done`: its branch merged onto the proving tip,
    the fast gate run on the merge, and proving moved to it
    ([ADR 0012](docs/adr/0012-the-line-gate-landing-promotion-cut.md)).
    A checkout's landings are a **stack**: each merged onto the one below,
    their gates at once; proving moves to the deepest all-green merge
    ([ADR 0019](docs/adr/0019-stacked-landing-gates.md)).
13. **Promotion**: its gate on the proving tip, then the base branch
    fast-forwarded to it. One at a time per checkout.
14. **Cut**: a version made from the base branch: set, push its own
    branch, gate, tag, publish, install.
15. **Watch**: one resident agent session on the host, to which every
    finding goes
    ([ADR 0013](docs/adr/0013-the-watch-and-the-daemon-it-reads.md)).
16. **Idle work**: what a checkout declares under `[idle]`: steps too slow
    for any gate, run while the host is idle, then its tasks line
    ([ADR 0014](docs/adr/0014-idle-work.md)).
17. **Urgent**: the owner's mark on an issue, which puts its start, its
    worker's heavy commands and its landing ahead, and halts every other
    worker's or program's heavy command, not starts or line moves, while its
    worker lives
    ([ADR 0015](docs/adr/0015-an-urgent-issue-halts-the-host.md)).

## Words within them

- **Role**: a directory, by its absolute path, holding a role's rules
  (CLAUDE.md) and skills (.claude/skills), which a worker starts with.
- **Hold**: a worker's hold of a resource across commands (`worker hold`),
  until `worker release` or its worker ends.
- **Look**: the watch's reading of doctor's findings, with no model.
- **Episode**: one condition doctor finds, while it stands.
- **Round**: a brief, an hour (`round_bound`), and the watch's report on
  one episode.
- **doctor**: not a concept of its own: the findings computed from the
  database and the host's readings, each with how to fix it. It is the
  watch's look and the operator's view.
