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
Claude Code, and for DSH and Kimi Code: each worker started by
a daemon from its kit entry, refused by the real guard when it pushed, landed;
and for agy: the same, its tokens read from its real
transcript, and a real headless ask read; codex, pi and grok have had no
such run on this host. Phase 7 waits
on the owner: which repo moves first, and how Guild moves its gate and landing dependencies, since factorium has no
counterpart for afkconvoy-gate and afkconvoy-landing. What afkconvoy had
and factorium does not is in `docs/dropped.md`.

## 3. One database

Everything factorium knows lives in one SQLite file,
`$XDG_DATA_HOME/factorium/factorium.db`: factum's log, its work rows and
its projections. The tracker, the checkout list, the kit, gate records, landings, promotions, cuts, the watch's episodes,
rounds and reports, and the host's settings are all events and
projections in it. No cache directory holds state; no remote ref holds
state. The CLI and the daemon open the same file (WAL); a CLI command is
a factum submission or a query, so the CLI needs no socket to the daemon.

A store written by an older binary is read by a newer one as it is: no
script is run by hand, so the swap's proof (§4) passes on it and the
daemon moves to the newer binary by itself. The log is
never rewritten; a change to what is kept is carried by the code that reads
it. A renamed event, field or value keeps its old name as a serde alias
wherever it is read (an event's kind, its data, an aggregate's state in a
snapshot); an event whose data changed in shape takes a factum upcast;
an aggregate's state that changed in shape takes a new `code()`, so a
snapshot kept by other code is folded again; a projection whose rows hold
a changed value takes a new `version()`, so its rows are rebuilt from the
log. factorium#95's rename (`parked` to `waiting_for_human`, `IssueParked`
to `IssueWaitingForHuman`) is read so.

What a repo declares about itself — its gate commands and limits, its
cut steps, its idle work, its heavy commands, its proving branch — is a file committed
in the repo, `factorium.toml`, because it changes with the code and is
reviewed with it. It is the one exception to "everything in the database".

## 4. Commands

One binary, `factorium`. Three groups, by who uses them; the guard lets a
worker run only `factorium worker …` and lets only the watch run
`factorium watch …`.

| Group | Who | Commands |
|---|---|---|
| `factorium …` | the owner | `run`, `stop`, `service install/remove/show`, `checkout add/edit/remove/list` (a checkout's default role, §1.2, is named at `add --role` or `edit --role`, and `edit --no-role` takes it away; an issue's own role wins over it; `edit --no-idle` turns its idle work, §1.16, off on this host and `edit --idle` back on, shown in `list` as `idle_off`), `issue add/list/show/edit/close/wait-for-human/ready/start-over/comment/block` (`show --steps` answers where each attempt's time went, §4; `add --waiting-for-human --why <text>` gives the wait its why, shown in doctor's `asks`; `add --blocked-by <n> --why <text>` and `block --by <n> --why <text>` give each block its why, required, shown in `show`'s `waits` and doctor's `parked`, §2; a comment reaches the issue's live worker at once; an issue's role, §1.3, is named at `add --role` or `edit --role`, and `edit --no-role` takes it away; `issue move <checkout> <n> --to <ssh-target>` hands one issue to another host's factorium: it checks first, over ssh in a login shell, that the target lists a checkout of that name with this one's origin, and changes nothing here until it does; then the issue's worker ends as at `wait-for-human`, its worktree kept; what that worktree holds that origin does not, its changes not committed as one commit on top, is pushed to origin's `factorium/move/<checkout>-<n>`; the target files it with `issue add` (its body, where it came from, that branch to merge first, then its comments with who and when; its role, account and blockers are named, not carried, being this host's; one that waited for a person here is filed `--waiting-for-human`, its `--why` naming where it waited and why, so no worker there takes it before a person decides; a ready one arrives ready); and it is closed here with a comment naming the target's issue. A step that fails answers what was done before it, and the issue here stays open), `doctor`, `decided [D<n>] [--read]` (the Decided records, §2; `D<n>`, that one record), `spent <checkout> [N|FROM..TO] [--since TIME]` (where those issues' time went, from their attempts' steps, §4), `discard` (a worker's worktree no open issue owns, as doctor names it), `outbox` (what was passed to a session, and how it went), `workers` (the live ones; `--all`: the ended too), `promote` (now, whatever the bound or a red tip says; while a promotion of that checkout runs, the daemon's or another `promote`, it is refused, its answer naming the running one, and the running one is left be), `cut`, `idle <checkout>` (runs its idle work now, §1.16; waits and answers the run's record), `hold` (landings; `--release` releases them), `unqueue` (a heavy-queue entry that does not move; a running one is ended with every process it started), `heavy run [--background] [--resource <name> …] -- <cmd>` (a program of the owner's that is not a worker queues a heavy command, §1.9, holding those resources, all at once, or `host`; exits with its exit code), `heavy depth` (the queue's depth as JSON: `running`, `waiting`, each entry, and each resource), `kit show/pin`, `accounts`, `settings` (with `--resource <name> --slots <n> [--note <text>]` declares a named resource, §1.9, with `--resource <name> --remove` takes it away, answering every one declared), `help`; hidden, `guard` (the hook a session runs before each tool call) |
| `factorium worker …` | a worker | `gate`, `done`, `escalate`, `comment`, `pick-mutants`, `review-growth`, `exclusive [--background] [--resource <name> …] -- <cmd>` (no `--resource`: `host`), `hold <name> … --for <duration> [--between HH:MM-HH:MM]` and `release <name>` (a hold across commands, §1.9) |
| `factorium watch …` | the watch | `next` (its round), `report`, `restart`, `tell`, and every owner command but `stop` (it leaves an issue to a person with `issue wait-for-human`, releases a hold with `hold --release`) |

Every message to a session — a brief, a landing's red, a round, `watch
tell` and `watch restart`, a comment, word that its role changed — goes through that session's outbox,
one record of who it was from, the line typed, and how it went: `typed` (a
brief), `submitted` (it started a turn), `queued` (it waits behind the
running turn: on each next look, found taken in its session file since, it
was submitted; the turn over and not taken, it is typed again at the
prompt; still unread 10 min after it was given, it is missed,
factorium#289), `brief` (written into a brief still to be typed),
`not_typed` (the harness was not up, the terminal gone, Orca refused), or
`missed` (typed, it started no turn and none runs: a dialog took it).
Nothing is typed before the harness is up, nor ahead of the brief. A brief
not typed is held and tried again on the session's next look; anything
else not typed is refused to its sender, and a red no one hears refused
sends its worker to the watch. A missed message sends its worker to the
watch at once; a missed round is passed again on the next look. Word that
its role changed changes nothing in the worker's task: missed, it sends its
worker to no one and holds up nothing after it, and is looked for again on
each next look: found taken in its session file since, it arrived; not
found, it is typed again. So no look waits on it past Orca's window: it is
given no grace after it, as other messages are. And no
tick waits for its typing at all: the tick gives it to the outboxes of every
worker it is owed and goes on, and it is typed at them at once beside the
ticks, on a store connection of its own; the tick's `roles` answers each
worker told, its changed files and its delivery as it stood when the tick
ended (`queued` while it is typed), and the outbox records how it went. A
session typed at beside the ticks is not tried again by a look until that
ended; the daemon lets it end before it stops, and swaps to a new binary only once none runs
(factorium#311). A prompt Claude Code took is
read from its session file as typed, or in the block it writes a long paste
in (`<pasted_content id="…">`).
`factorium outbox [--worker <id>]` answers the record.

`issue comment` records the comment and reaches the live worker on the
issue at once, through the same outbox as `watch tell`, never stopping
its running turn: a working worker gets one line typed at its session
(`delivery: submitted`, `queued`, or `missed`); one whose brief is not yet handed
gets it in that brief (`brief`); a waiting one is not typed at, its watch
episode carries it (`watch`); with no worker, or when delivery fails
(`undelivered` says why), `none`. The next brief carries every comment.
Comments factorium writes itself (start-over's) are typed at no one.

An issue's comments are the one record of what is said about it, each
with who wrote it (`owner`, `worker <id>`, `watch`, `factorium`) and when;
`issue show` answers them, and `spent`, what the issue took, for whoever settles its cost (Guild): each attempt's time from its start to its end (a live one's, to now), in ms, and the tokens its session spent, given (cached ones counted) and written, read from the session files that record its turn: as it ends, and kept, since its harness may clear them (a live one's, as they say now); with their sums. Claude Code's files say (each assistant message's `usage` once, by its id), Codex's (its last `token_count`), pi's (each assistant message's `usage`) and agy's (each `PLANNER_RESPONSE` of its `transcript.jsonl`: `input_tokens` and `cache_read_tokens` given, `output_tokens` written); DSH's, Kimi Code's and Grok's are not read, their tokens `null`. Each attempt also answers the role it started with, as `role` (`dir`, its directory, and `from`, `issue` or `checkout`, as `role_from` says, §3), and as `role_hash` a hash of what that role gave it, taken at its start: its rules and every file under its skills, read through the link, each file by its path under the skills and its bytes. A role directory is linked live, so a role changed while a worker runs changes what that worker reads; the hash says which content an attempt started with. On each tick, a live worker whose role directory now holds other content than it last knew of (as it started, or as it was last told) is told so once, through its outbox as a comment is: one line, `factorium (your role changed): …`, naming each file by its path under the role's directory (`CLAUDE.md`, `.claude/skills/…`) that changed, was added or went, and asking it to re-read the role's core skill and those files before its next step; where the role's rules changed, its session's rules are written again with them. What it was told is recorded on its worker, so one change is told once; `role_hash` stays the hash at its start. One waiting for the watch, unless parked on its engine's limit, is told once it works again; a worker with no role, or whose role did not change, is told nothing. It is of the content alone and the same from one build to the next: factorium reads nothing else of the role (no rolesmith file, no rev). An attempt with no role answers `null` for both. A picker's (`worker pick-mutants`) is not among them: its cost is its own comment (§6). A worker's `escalate` text, and the text of
`watch tell` and `watch restart`, become comments when they are typed,
delivered or not. Each reaches the worker at most once: the line `tell` or
`restart` types is its delivery, nothing types it again, and a worker's
own escalation is typed back at no one. Comments made before authors and
times were kept read as the owner's (start-over's as factorium's), with no
time.

**Where an attempt's time went**: `issue show --steps`
answers in each attempt of `spent` its `steps`, read from what the database keeps of it and
written nowhere twice, each step with its start and end in ms. Its
`heavy`: each heavy command of its own (`worker gate`, `worker
exclusive`, a hold), when it asked (the place it took, §1.9), when the
queue granted it and when it ended, its wait and its run apart, and how
its steps ran (`green`, `red`, `timed_out`), kept on the queue's end of
it; the rows are the tracker's `heavy_runs`, one per queue entry, built
from the heavy queue's events. Its `landings`: each `done`, each check
that its branch merges while it waits its turn (§1.12), each fast gate of
its merge (asked at its turn in the line: the line's gate names the
worker it lands), and how it ended; a red with its kind (`no_merge`,
`gate_red`, `past_limit`, `regenerated`: red on a merge that took
proving's copy of regenerated records), the files it names (those that
conflict, or the records taken), and the message that told it with when
it reached the session. Its `waits` (each `worker waiting`) and `parks`
(behind what its issue waits for, with the block's why; on its engine's
limit, with its reset), from its worker's events and the times the log
took them. Its `messages`: each message passed to its session, by its
number in the outbox, which keeps the text (`outbox --worker <id>`), with
when it was given and when it reached it. `spent.ms` stays what it is.
Steps before factorium#308 read as they were kept: a red with no kind, a
park with "(no reason given)", a heavy command with no outcome.

**Where a checkout's issues spent their time**: `spent
<checkout> [N|FROM..TO] [--since TIME]` reads those steps of every
attempt of the issues named (all, open and closed, by default) and stores
nothing. Each attempt's time (from `--since`, if later than its start) is
cut into parts that do not overlap and sum to it: a moment two steps
share goes to the first of waiting for a landing (green, red by kind, or
other), a heavy command of its own running, one queued (each by command:
gate, exclusive, hold), a park, a declared wait (each by why); the rest is
the session's own. It answers the totals (attempts, ms, tokens, the wall
clock from the first start to the last landing), that split, the reds by
kind and the files that conflicted ranked by the landings they broke, and
one row per issue with its own split and reds.

The daemon runs as a launchd agent (`factorium service install`,
defined in `packaging/factorium.plist`) with the installing shell's PATH
declared in it, restarted whenever it exits; `factorium stop` unloads it.
A boot loads no Background agent into `user/<uid>`, at boot or at a login
(factorium#287), so any command but `run`, `stop` and `service` loads the
installed agent again when launchd does not hold it and no daemon runs,
and says so on stderr; not after `stop`, until `service install`.
A daemon Homebrew installed notes its formula's link at start; once the
link names another version (a cut by the daemon, by `factorium cut`, or
by hand), it execs the link between ticks, as the same pid, once that
binary is proven to open this host's database: it answers `doctor` on a
copy (SQLite's `VACUUM INTO`, one read transaction no writer restarts;
never the live file). Each binary is proven once on the host, whoever
asks: the processes that ask take turns on its lock, and how it went is
kept beside the copies for the next (a copy that failed, or a `doctor`
past its limit, is not kept). The daemon's tick only starts that proof, on
a thread of its own, and a later tick swaps once it passed: no tick waits
on it. A new event kind may be written first by any process of the
version that adds it, a CLI call before the daemon swapped among them: a
daemon of an older version whose store cannot read it fails its ticks,
and a failed tick still starts that proof and swaps once it passed (not
while a line move runs beside the ticks), since the newer binary alone
reads the store; a rollback's kept binary is gone back to only when no
link to swap to is proven. A cut of the formula proves
it right after the install (the daemon's cut beside its ticks, recorded on
the tick after the proof ended); one that fails is a failed cut, its why the
error and the copy's path (kept for a look), and the old binary runs on.
Which host installs factorium, and when: a host one of
whose checkouts declares the cut of the daemon's own formula (its base,
as last fetched, names it in `[cut]`) installs it at that cut, and only
then; every other host's daemon, Homebrew's, upgrades its formula from
the tap itself, each `upgrade_every` (an hour) since its last try, while
no line move runs beside the ticks: `brew update`, then `brew upgrade`,
on a thread beside the ticks, recorded on the tick after it ended. One
that fails is a finding (`daemon:upgrade`, brew's why) until a later try
works. What it links is swapped to by the rule above, once proven; a
linked binary whose proof failed is a finding naming its version and
the proof's why (`daemon:refused:<version>`) while the link names it,
and the old binary runs on (scenarios: `upgrade.rs`). So no session
stops it: the guard refuses `stop` to the watch, which lives under the
daemon, and to workers. At start it looks for
git, orca and the programs of the commands stablehand keeps (the
fallback's, `claude`, if it cannot say); a missing one is a finding. A failing
tick is recorded, and `tick_failures_max` of them in a row is a finding;
the watch still looks when a tick fails. All three live in the store: a
look that fails on the store itself (a binary that cannot read the
database) briefs the watch's session directly — at its terminal as the
last look that read the store kept it in the data directory — with the
error, the binary's version, when the failures began and how many, once
per episode of the same error and again each hour while it lasts, and
what to read without factorium's commands. `doctor` that cannot read the
store answers that as its one finding, naming the error and the data
directory, and exits 1.

No tick waits for a heavy run: every gate and step a tick starts runs
beside the ticks. Each tick is timed by its steps; daemon.log has each one
that did something or was slow, with how long it took and its longest
step. A slow one, past half the hang bound (five `tick_every`, never under
two minutes: a daemon silent that long is taken to hang), is kept, and for
an hour is a finding naming how long it took and that step (factorium#231).

A binary's first good tick keeps a copy of it under the data directory
(`binary/<version>/bin/`, one, replaced by the next). A binary Homebrew
links whose ticks fail `tick_failures_max` times in a row before its first
good one (a store that cannot read this database) execs the kept copy, of
another version, as the same pid, reading no database to decide it: it
hands the kept one the rollback in `rollback.json`. Once the kept one ticks
well it records the rollback (both versions and the error), a finding and
so a watch episode, and swaps to no link naming the version it left; a
newer one linked, it swaps, and that one's good first tick ends the
rollback.

The help in the binary is the manual (as in afkconvoy): every command's
`--help` says what it does and what it answers. Answers are JSON. It is
written with agent-help, as stablehand's and slimfit's is: each command's
and argument's doc comment, Markdown with each paragraph on one line, kept
as written (`verbatim_doc_comment`), each top-level command in the group of
who runs it; a test runs `agent_help::check` on it.

## 5. Crates

| Crate | Holds | Target (source lines) |
|---|---|---|
| `factorium-domain` | The pure half, on factum: the issue, worker, landing, promotion, cut, account and watch aggregates and processes; admission, the heavy-command queue's start rule, a worker's health (up, hot, hung, silent, quota) decided by one command from one observation; the disk floor as a pure function of readings; doctor's findings. No IO, no clock. | 6,240 |
| `factorium-harness` | The six harnesses: start line and brief, the hook's tool call, session files and turn lines, quota and refusal screens; where a session on an account keeps its files (the account itself is stablehand's). Moved from afkconvoy-harness, nothing GitHub in it, and only what factorium calls. | 8,000 |
| `factorium-host` | The effects: git (worktrees, merge, push requiring a value), orca (terminals), host readings (CPU ticks, memory, pressure, disk, process creation), process trees and sampling, the programs it reads the answers of (Homebrew, stablehand). | 4,000 |
| `factorium` | The binary: CLI, the daemon (factum's tick plus three workers — the starter, the gate runner, the lander), the watch's terminal, `doctor`'s text. | 4,830 |

A crate's target is a trend, not a gate (factorium#163): `./lines.sh`,
which `checks.sh` runs, prints each crate's lines against its target here
and an overage, and never fails. A crate over its target, or that grew at
each of its last five landings that changed it, is a task: the idle tasks
line runs `./lines.sh --tasks` after slimfit's `next`, and a `crate-lines`
issue names the crate and the files that grew most (§1.16). Its worker
deepens the crate; a target changes only with its reason in the issue that
changes it. fmt and clippy stay bounds.

**What a repo declares**, in `factorium.toml` at its root (§3); every
table may be left out:

```toml
[gate]
fast = "./gate.sh"      # what every landing needs, on its merge
fast_limit = 600        # seconds
slow = "./slow.sh"      # what fast lacks (§1.13)
slow_limit = 3600
regenerated = [".truefit/models/*.json"]  # git globs: records the gate rewrites (§1.12)

[cut]                   # a version from the base tip (§1.14)
steps = ["./release.sh $FACTORIUM_VERSION"]
formula = "tap/name"    # the Homebrew formula upgraded once cut
limit = 3600

[idle]                  # only while the host is idle (§1.16)
steps = ["./soak.sh"]
tasks = "slimfit --version && slimfit next --all"   # filed as issues (§1.16)
every = 86400           # at most once per this many seconds
limit = 14400           # seconds, all the steps together; past it, red
```

**For Guild.** Guild depends on `factorium-harness`, `factorium-host`
and the domain's neutral parts (admission, the queue's start rule,
hot-worker rules, landing onto a proving branch), as git dependencies
pinned to a revision. Those public items name no issue, worker, watch or
checkout: what only the caller knows it passes in, or says through a
trait it implements. They live in `factorium_domain::neutral`; factorium's
own modules wrap them with what it knows. A test from outside
(`crates/factorium/tests/neutral.rs`) checks they name none of those and
depend on nothing else of factorium.

## 6. Tests: only at the outermost layer

- **Scenarios through `App`.** `App` is the program's one entry: every
  CLI command is one method of it, and the daemon's tick is one too.
  Every scenario drives `App` as the CLI does and reads only what the CLI
  could print. No crate has tests of its own beneath it.
- **What is faked, and how.** Orca, Homebrew, stablehand, slimfit,
  rolesmith and launchd: the programs themselves. The app runs them through one port
  (run a program, get its answer), and a scenario stands in there with
  answers shaped as the real programs gave them — Orca's keeping terminals
  and what was sent to them — so what the app makes of their answers runs
  in every scenario. Each stand-in is held to its real program by a truefit
  contract (`truefit.toml`): the cases of what the app reads of it, run
  through one driver against the scenarios' own fakes in the fast gate
  (`truefit contract`) and against the real program by hand (`truefit
  contract --real <name>`), the last agreeing run kept in `.truefit/`.
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
- **Every scenario is held to account**, by two mechanisms and no
  whole-workspace mutation run (factorium#114): a change's mutants, and
  slimfit's coverage-plus-review task, which finds a scenario that only
  repeats another, or shares its setup, to delete or merge. Both rules are
  slimfit's ([`slimfit help mutants-scope`](https://github.com/sunfmin/slimfit/blob/main/src/help/mutants-scope.md); slimfit#31), not restated here.
- **Bounds.** No test sleeps; time moves when a scenario moves it.
  slimfit holds test time, not `gate.sh`: its `test_seconds` is the CPU
  time (user+sys) the tests' own processes spent, build excluded, and the
  gate fails when it rises past the baseline's tolerance (`slimfit check
  --help`; `slimfit help host-load`, [src/help/host-load.md](https://github.com/sunfmin/slimfit/blob/main/src/help/host-load.md)).
  CPU time, not wall time, because the bound must fail when the tests got
  slower and pass the same code on the same host
  however busy it is: the suite took 24s idle and 75-193s at load 55-73
  on 18 cores, while its CPU time held (factorium#103). Until
  factorium#123 `gate.sh` held a CPU bound of its own around the run;
  slimfit#43 made that the same fact twice, so it went. Nearly all of the
  tests' CPU is git's processes, about 50 a scenario, so a scenario's cost
  is the git it runs: git is run as its binary, not macOS's `/usr/bin/git`
  shim, which more than doubled each call; the scenarios' homes turn off
  git's auto maintenance; a world's repos are copied in-process; a
  scenario of several worlds runs them at once (`each`, `at_once!`), so it
  takes its slowest world's time, not their sum; and the binary's tests
  run in the scenarios' binary, beside them rather than after
  (factorium#98 halved the suite this way).
- **slimfit holds the suite, in the fast gate.** `./gate.sh` is
  `./checks.sh` (lines, §5; fmt, clippy) then `slimfit check`
  against the committed `.slimfit/`, in place of a plain `cargo test`, holding
  the tests' time as above, then `truefit contract`; the slow gate (`./slow.sh`)
  is the release build and the models. What
  `slimfit check` holds, a change's record of its mutants among it, is
  slimfit's ([`slimfit help mutants-scope`](https://github.com/sunfmin/slimfit/blob/main/src/help/mutants-scope.md)), not restated here; factorium
  holds it at the base, once per promotion (ADR 0006). The worker follows
  `slimfit next`; what is factorium's is the priority: a mutants run goes
  through `factorium worker exclusive --background`, a run whose baseline
  it ratchets through `factorium worker exclusive` (§1, row 9). And the
  second agent that picks a change's mutants ([`slimfit help picker`](https://github.com/sunfmin/slimfit/blob/main/src/help/picker.md)):
  `factorium worker pick-mutants` asks a fresh headless session, on the
  engine, model and account stablehand picks among the other engines
  that can be asked headless (else the worker's, anew; §1.7), within `pick_bound`, given only
  slimfit's goal, the diff from origin's proving and `slimfit check
  --mutants`; its time and tokens go on the issue (factorium#147). Claude
  Code, Codex, Grok and agy can be asked, each in a user directory of its
  own that holds only its login (agy's, its HOME, also an agent with no
  tools, factorium#200). One whose answer is no JSON object, or not one
  as asked, is asked once more in the same session (Claude Code and Grok
  by `--resume`, agy by `--conversation`; Codex's session is ephemeral,
  so it is not), told only what was wrong and the one form the answer
  takes; both asks' time and tokens count, and the record says it was
  "answered as asked when asked again" (factorium#226). One that fails,
  or answers off the form twice, hands the pick to the one stablehand
  picks among those left, the worker's own last, and the record names each
  that failed and why; when none is left the worker gets the last refusal,
  never a pick (factorium#154). When slimfit lists no candidates (an
  empty listing, or its refusal that the tool listed none, slimfit#77)
  of a change its listing's `landing` says adds no source line and removes
  no test, no picker is asked: it answers `none`, "slimfit lists no
  candidates", with no picker, and the issue keeps that line
  (factorium#178). Otherwise the gate still wants a record of none, so the
  picker is asked as ever, given what slimfit printed, and answers why
  none and the `ratchet --no-mutants` naming it (factorium#247). The second agent that judges a change's growth
  ([`slimfit help reviewer`](https://github.com/sunfmin/slimfit/blob/main/src/help/reviewer.md)):
  `factorium worker review-growth` asks one the same way, with the same
  fallback and bound, given only slimfit's goal and the diff; it answers
  `as_is` with why or `cut` with what, and the `slimfit ratchet` that
  writes the growth record naming it (`harness:session:model`) and the
  issue it serves; its time and tokens go on the issue (factorium#174).

## 7. From afkconvoy

Kept, as afkconvoy decided them: the session file is the turn; the kit
orders harnesses and a checkout may pin one; a login that refreshes is
bound to one machine (which account a worker starts on is stablehand's
pick now, §1.7); admission alone decides each start; the queue starts by the
host's CPU reading, never starves, and bounds a hold; hot workers and
their evidence; the disk floor and reclaiming `target/`; serial builds
(cargo's fine-grain locking opted out); the gate is local, declared, fast
and slow, recorded by tree; the proving branch and promotion; the fleet
cuts its own versions; the watch, its look, episodes, rounds and report;
guards, not identities; the help is the manual.

Not in factorium: teams, roles kept by factorium (an issue, or a checkout as its default, may name a role directory, §1.2–1.3, and that is all), workflows, stages, verdicts, rework,
consults, patrols, intake and flow labels; GitHub in every form (tracker,
pull requests, checks, branch protection, quota, Discussions); the
daybook; the panel; batched rounds and the pairing tree; several hosts
(remote claims, a shared proving branch, cross-host reports: `issue move`,
§4, is not that, for no state is shared between hosts and nothing is read
from the other host afterwards: one owner command hands one issue over by
filing it there); Jira
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
   steps promotes as soon as it lands.
3. **A landing red twice in a row goes to the watch**; no second worker in
   a new worktree. A merge red on a proving tip that moved since the last
   red's tip, which the branch holds, is not the second: the worker did
   what the first asked, and in one day eight watch rounds only said
   "merge the new tip" (factorium#171). `moved_reds_max` bounds how many
   in a row are let off.
4. **No Flux.** Scenarios and mutation hold the rules; afkconvoy's fast gate
   spent 65 of its 138 seconds in Flux.
5. **factorium is the one writer of a managed repo's base.** A repo
   factorium manages has its base moved only by promotion; anything else
   that moves the base turns promotion red.
6. **A landing waits for no review** (factorium#167). It lands on the fast
   gate alone and reaches only proving, which is promoted after the slow
   gate; verdicts and rework stay out of factorium (§7). Review comes after
   landing, from outside: Guild's warden, and control's job on a proving
   branch, file what they find as new issues. What would reopen it: a
   landing that harms before review can catch it (secrets, data loss,
   something externally visible) — then a finding that holds promotion
   (`factorium hold`), not an approval on every landing; or measured
   evidence that findings after landing cost more than review before it.
