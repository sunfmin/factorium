# factorium — specification (draft 1, 2026-11-17)

factorium runs AI coding agents on one machine while nobody watches: it
takes issues from a local tracker, gives each to a worker in its own
worktree, lands what passes the repo's gate onto a proving branch,
promotes the proving branch to the base branch once the slow gate passes,
cuts versions, and keeps the machine healthy. A resident **watch** agent
deals with everything that goes wrong.

It succeeds afkconvoy and is built on factum. Guild depends on its crates.

Status: approved 2026-11-17. Phases 1–6 are done. Their real runs: one
Claude Code worker on a throwaway repo, one real watch round, and
factorium cut its own v0.1.0 through Homebrew. Phase 3 had real runs for
Claude Code, and (factorium#39) for DSH and Kimi Code: each worker started by
a daemon from its kit entry, refused by the real guard when it pushed, landed;
codex, pi and grok have had no such run on this host. Phase 7 waits
on the owner: which repo moves first, and how Guild moves its gate and landing dependencies, since factorium has no
counterpart for afkconvoy-gate and afkconvoy-landing. What afkconvoy had
and factorium does not is in `docs/dropped.md`.

## 1. Concepts

factorium has these concepts and no others. Adding one needs the owner's
approval and an entry here.

| # | Concept | What it is |
|---|---|---|
| 1 | **Host** | The one machine and its one user account. One daemon (`factorium run`) and one SQLite database hold everything factorium knows of it. |
| 2 | **Checkout** | A repo on the host's checkout list: its path, base branch, proving branch, and what the repo declares (§4). |
| 3 | **Issue** | A piece of work in the host's tracker: title, body, the files it names, what blocks it, comments, and whether it is ready, parked or closed. |
| 4 | **Worker** | One harness session working one issue in its own worktree, branched from the proving tip. It ends with `factorium worker done`, `factorium worker escalate`, or by failing. |
| 5 | **Harness** | A coding agent program factorium starts: Claude Code, Codex, pi, DSH, Kimi Code, Grok. How each starts, where it keeps its session and which lines start and end a turn are its own module's to say. The session file is the turn. |
| 6 | **Kit** | The harnesses this host can start now, in priority order, with their models. A worker starts on the first eligible one. |
| 7 | **Account** | An engine subscription, kept in mytokens (`agent-account-<name>`): its usage windows and the limits it met. A worker starts on the account of its harness with the most left. |
| 8 | **Admission** | Whether one more worker may start now, from the host's readings alone: CPU busy share, free memory, the disk floor, factorium's own share caps, and the accounts' windows. No concurrency number. |
| 9 | **Heavy command** | A command a worker runs through `factorium worker exclusive -- <cmd>`, and every gate run. One host-wide queue starts each when the host has room — by the same readings as admission, nothing else — never starves the oldest, and ends one that holds others up past the hold bound. |
| 10 | **Hot worker** | A worker whose processes take more than their share of CPU, memory, disk written, disk held or process creation, or that stalled holding build locks. Its evidence is taken at once; it is a finding for the watch. |
| 11 | **Gate** | The repo's own fast and slow commands, each with its limit. A green run is recorded against the tree it ran on, so the same tree is never gated twice. |
| 12 | **Landing** | A worker's `done`: its branch is merged onto the proving tip, the fast gate runs on the merge, and the proving branch is moved to it by a push that requires the tip it read. One landing per checkout at a time. Green closes the issue and ends the worker. |
| 13 | **Promotion** | The slow gate on the proving tip, then the base branch fast-forwarded to it by a push that requires the base it read. Started when the host is idle, or once the oldest unpromoted landing has waited the promotion bound. |
| 14 | **Cut** | A version made from the base branch when the checkout is idle, or once its oldest uncut landing has waited the cut bound: set, push its own branch, gate, tag, publish, install. |
| 15 | **Watch** | One resident agent session on the host. A **look** (no model) every five minutes, and on the next tick once a worker starts waiting for the watch, reads `doctor`'s findings; each condition is an **episode**, and an episode starts at most one **round**: a brief, an hour, and a report the watch files with `factorium watch report`. Escalations, failed workers, hot workers, red promotions, failed cuts and resource holds all go to it. It acts with the operator's commands. |

`doctor` is not a concept of its own: it is the findings computed from the
database and the host's readings, each with how to fix it. It is the
watch's look and the operator's view.

## 2. The issue's path

1. `factorium issue add` makes an issue, ready unless it says otherwise,
   with the issues it waits for (`--blocked-by N`, repeatable) recorded
   with it, so no tick sees it ready without them.
2. A tick (the daemon's loop) finds the ready issues nothing blocks and
   no worker holds; admission admits one start per reading.
3. The worker resumes in the worktree its issue's last attempt kept, if
   that one holds work that is nowhere else (commits not on proving, or
   changes not committed), and its brief says what is there; else its
   worktree is branched from the proving tip. Its harness and
   account are chosen; the guard is installed; its brief is sent; the
   session file says when its turn starts and ends. Text is typed at a
   session only once its harness's own screen is up there: typed at a
   shell, a line is a command it runs. A question the harness opens on its
   own mid-session (a dialog that would take the line) is dismissed first
   with the harness's own key.
4. The worker runs `factorium worker gate` (the fast gate) as often as it
   likes, then `factorium worker done`.
5. Landing (§1.12). Red: the worker is told why and goes on; red twice in
   a row goes to the watch. A red that does not reach the session's prompt
   (no turn starts and none runs) goes to the watch at once.
6. Green: the issue closes, the worker ends, its worktree is removed
   unless it holds work that is nowhere else: the one test that also
   decides where its issue's next start begins (step 3). It is read
   against the last attempt that started, and only a worktree still on
   disk is resumed or discarded. When the proving tip cannot be read, what
   a worktree holds is unknown, so it is held: a worker that ends keeps
   its worktree, start-over discards it (keeping its branch, as below), and
   a start, which has no tip to begin from either way, refuses and takes
   nothing.
   A kept worktree that went bad is discarded on purpose with
   `issue start-over <checkout> <n> --why …`, only while no worker is on
   the issue: its uncommitted changes are committed, its branch is kept as
   `<branch>-abandoned`, a comment names that branch and the why, and the
   worktree goes, so the next start is branched from the proving tip. The
   issue stays ready or parked as it was.
7. Promotion and cut follow on their own (§1.13, §1.14).

Off the path, everything goes to the watch: `worker escalate`, a worker
whose session died, never came up, went silent past its bound, or hung
past it (its turn open, its processes next to idle, its screen
unchanged), a hot worker, a red promotion, a failed cut, admission held
for one reason past the host's span. So does a line that stopped:
whatever it owes is done within its bound or is a finding — a heavy
queue that starts nothing past `stall_bound` (or whose oldest waits past
`hold_bound` and `stall_bound` more), a landing no gate runs for past
`stall_bound`, the oldest unpromoted landing past `promotion_bound` with
no promotion running, the oldest uncut promotion past `cut_bound` with no
cut running. The watch fixes, restarts, re-briefs, parks or closes, and reports.
Only spending money is left to the owner, in the report's `human` part,
which `doctor` shows.

## 3. One database

Everything factorium knows lives in one SQLite file,
`$XDG_DATA_HOME/factorium/factorium.db`: factum's log, its work rows and
its projections. The tracker, the checkout list, the kit, the accounts'
windows, gate records, landings, promotions, cuts, the watch's episodes,
rounds and reports, and the host's settings are all events and
projections in it. No cache directory holds state; no remote ref holds
state. The CLI and the daemon open the same file (WAL); a CLI command is
a factum submission or a query, so the CLI needs no socket to the daemon.

What a repo declares about itself — its gate commands and limits, its
cut steps, its heavy commands, its proving branch — is a file committed
in the repo, `factorium.toml`, because it changes with the code and is
reviewed with it. It is the one exception to "everything in the database".

## 4. Commands

One binary, `factorium`. Three groups, by who uses them; the guard lets a
worker run only `factorium worker …` and lets only the watch run
`factorium watch …`.

| Group | Who | Commands |
|---|---|---|
| `factorium …` | the owner | `run`, `stop`, `service install/remove/show`, `checkout add/remove/list`, `issue add/list/show/edit/close/park/ready/start-over/comment/block` (a comment reaches the issue's live worker at once), `doctor`, `promote`, `cut`, `hold` (landings), `unqueue` (a heavy-queue entry that does not move), `kit`, `switch`, `accounts`, `settings`, `help` |
| `factorium worker …` | a worker | `gate`, `done`, `escalate`, `exclusive -- <cmd>` |
| `factorium watch …` | the watch | `next` (its round), `report`, `restart`, `tell`, `park`, `release`, and every owner command |

`issue comment` records the comment and reaches the live worker on the
issue at once, through the same delivery as `watch tell`, never stopping
its running turn: a working worker gets one line typed at its session
(`delivery: submitted` or `queued`); one whose brief is not yet handed
gets it in that brief (`brief`); a waiting one is not typed at, its watch
episode carries it (`watch`); with no worker, or when delivery fails
(`undelivered` says why), `none`. The next brief carries every comment.
Comments factorium writes itself (start-over's) are typed at no one.

The daemon runs as a launchd agent (`factorium service install`,
defined in `packaging/factorium.plist`) with the installing shell's PATH
declared in it, restarted whenever it exits; `factorium stop` unloads it.
A daemon Homebrew installed notes its formula's link at start; once the
link names another version (a cut by the daemon, by `factorium cut`, or
by hand), it execs the link between ticks, as the same pid. So no session
stops it: the guard refuses `stop` to the watch, which lives under the
daemon, and to workers. At start it looks for
git, orca and the kit's harnesses; a missing one is a finding. A failing
tick is recorded, and `tick_failures_max` of them in a row is a finding;
the watch still looks when a tick fails.

The help in the binary is the manual (as in afkconvoy): every command's
`--help` says what it does and what it answers. Answers are JSON.

## 5. Crates

| Crate | Holds | Budget (source lines) |
|---|---|---|
| `factorium-domain` | The pure half, on factum: the issue, worker, landing, promotion, cut, account and watch aggregates and processes; admission, the heavy-command queue's start rule, hot-worker rules and the disk floor as pure functions of readings; doctor's findings. No IO, no clock. | 7,000 |
| `factorium-harness` | The six harnesses: start line and brief, the hook's tool call, session files and turn lines, quota and refusal screens; accounts from mytokens. Moved from afkconvoy-harness, nothing GitHub in it, and only what factorium calls. | 8,000 |
| `factorium-host` | The effects: git (worktrees, merge, push requiring a value), orca (terminals), host readings (CPU ticks, memory, pressure, disk, process creation), process trees and sampling, mytokens, Homebrew. | 4,000 |
| `factorium` | The binary: CLI, the daemon (factum's tick plus three workers — the starter, the gate runner, the lander), the watch's terminal, `doctor`'s text. | 4,000 |

`gate.sh` fails a crate over its budget. Total: 23,000.

**For Guild.** Guild depends on `factorium-harness`, `factorium-host`
and the domain's neutral parts (admission, the queue's start rule,
hot-worker rules, landing onto a proving branch), as git dependencies
pinned to a revision. Those public items name no issue, worker, watch or
checkout: what only the caller knows it passes in, or says through a
trait it implements. A test from outside checks they depend on nothing
else of factorium.

## 6. Tests: only at the outermost layer

- **Scenarios through `App`.** `App` is the program's one entry: every
  CLI command is one method of it, and the daemon's tick is one too.
  Every scenario drives `App` as the CLI does and reads only what the CLI
  could print. No crate has tests of its own beneath it.
- **What is faked, and how.** Orca, Homebrew and mytokens: the programs
  themselves. The app runs them through one port (run a program, get its
  answer), and a scenario stands in there with answers shaped as the real
  programs gave them — Orca's keeping terminals and what was sent to them —
  so what the app makes of their answers runs in every scenario.
  Harnesses: a stand-in session that writes the session file each harness
  describes, line by line, from its own module, so no second copy of a
  format exists. Host readings and the clock: in-process stand-ins the
  scenario moves.
  Git is real: a bare remote and clones in a temporary directory, made
  once per test run and copied, because landing's correctness is git's.
- **The binary.** A handful of tests run the built binary to prove the
  wiring: argument parsing, JSON answers, the guard refusing a worker an
  owner command, the CLI and the daemon sharing the database.
- **Real harnesses.** Opt-in smoke tests per harness, run by hand.
- **Every scenario is held to account by mutation**: break the rule it
  guards and it must fail.
- **Bounds.** The whole gate runs in 60 seconds or less. No test sleeps;
  time moves when a scenario moves it.

## 7. From afkconvoy

Kept, as afkconvoy decided them: the session file is the turn; the kit
orders harnesses and a checkout may pin one; a worker starts on the
account with the most left, and a login that refreshes is bound to one
machine; admission alone decides each start; the queue starts by the
host's CPU reading, never starves, and bounds a hold; hot workers and
their evidence; the disk floor and reclaiming `target/`; serial builds
(cargo's fine-grain locking opted out); the gate is local, declared, fast
and slow, recorded by tree; the proving branch and promotion; the fleet
cuts its own versions; the watch, its look, episodes, rounds and report;
guards, not identities; the help is the manual.

Not in factorium: teams, roles, workflows, stages, verdicts, rework,
consults, patrols, intake and flow labels; GitHub in every form (tracker,
pull requests, checks, branch protection, quota, Discussions); the
daybook; the panel; batched rounds and the pairing tree; several hosts
(remote claims, a shared proving branch, cross-host reports); Jira
deliveries; the asking session; remote refs as state; caches as state.

Each afkconvoy mechanism added after an incident is either a scenario
here or dropped on purpose, listed in `docs/dropped.md` with why.

## 8. Phases

| # | What | Done when |
|---|---|---|
| 0 | This spec | The owner approves it |
| 1 | Skeleton: crates, factum, `App`, the stand-ins, `gate.sh` with budgets, CI | Gate green |
| 2 | The path with one harness: issue → worker → `done` → landing on proving → promotion → issue closed | Its scenarios pass; one real Claude Code smoke run |
| 3 | All six harnesses, the kit, accounts and their windows | Scenarios; a smoke run per harness |
| 4 | Admission, the heavy-command queue, hot workers, the disk floor | Scenarios |
| 5 | The watch and `doctor` | Scenarios; a real watch round |
| 6 | Cut and self-upgrade through Homebrew | factorium cuts its own version |
| 7 | Move repos from afkconvoy one at a time (a repo belongs to one); Guild moves its dependencies from afkconvoy's crates to factorium's | First repo landed by factorium; Guild builds against it |

## 9. Decisions, and why

1. **Repo declarations live in the repo** (`factorium.toml`): they change
   with the code and are reviewed with it.
2. **Every checkout has a proving branch**: one path. A repo with no slow
   gate promotes as soon as it lands.
3. **A landing red twice in a row goes to the watch**; no second worker in
   a new worktree.
4. **No Flux.** Scenarios and mutation hold the rules; afkconvoy's fast gate
   spent 65 of its 138 seconds in Flux.
5. **factorium is the one writer of a managed repo's base.** A repo
   factorium manages has its base moved only by promotion; anything else
   that moves the base turns promotion red.
