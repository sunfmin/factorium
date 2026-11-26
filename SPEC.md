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
and (factorium#201) for agy: the same, its tokens read from its real
transcript, and a real headless ask read; codex, pi and grok have had no
such run on this host. Phase 7 waits
on the owner: which repo moves first, and how Guild moves its gate and landing dependencies, since factorium has no
counterpart for afkconvoy-gate and afkconvoy-landing. What afkconvoy had
and factorium does not is in `docs/dropped.md`.

## 1. Concepts

factorium has these concepts and no others. Adding one needs the owner's
approval and an entry here.

| # | Concept | What it is |
|---|---|---|
| 1 | **Host** | The one machine and its one user account. One daemon (`factorium run`) and one SQLite database hold everything factorium knows of it. |
| 2 | **Checkout** | A repo on the host's checkout list: its path, base branch, proving branch, its **default role**, if any, and what the repo declares (§4). The default role is a role directory as for an issue (§1.3), by its absolute path, kept in the host's list and not in the repo's `factorium.toml`, since the path holds only on this host; it is named at `checkout add --role`, changed with `checkout edit --role` or taken away with `checkout edit --no-role`, each refused unless it is such a directory (factorium#249). |
| 3 | **Issue** | A piece of work in the host's tracker: title, body, the files it names, what blocks it, comments, and whether it is ready, waiting for a human (only a person's `issue ready` moves it again) or closed. It may name the **role** its worker plays and the **account** it starts on, as whoever filed it decided (Guild, which keeps roles and accounts' quotas across hosts; guild ADR 0051): a role is a directory, by its absolute path, holding the role's rules (`CLAUDE.md`) and skills (`.claude/skills`), say lore checked out at a rev; its worker starts in a directory of its own whose rules are factorium's worker rules and then the role's, and whose skills are the role's, linked, the role's directory read and never written. An issue that names no role takes its checkout's default role (§1.2); an issue's own role always wins over it, and `issue show` answers the role the next worker starts with as `role`, and as `role_from` whether it is the issue's own (`issue`) or the checkout's (`checkout`). The role is named at `issue add --role` or at `issue edit --role` (changed, or taken away with `--no-role`), each refused unless it is such a directory; a worker already started keeps the directory it started in, each attempt after the edit starts with the role as edited, and the edit is a comment on the issue saying from what to what (factorium#248). A role directory that holds a `rolesmith.toml` is checked before a worker starts with it, each directory once a tick however many of its issues are ready (factorium#277): the daemon runs `rolesmith doctor --project <dir>`, and a program it lacks is installed first (`rolesmith tool install --project <dir>`, not again while the same lack stands) and the role checked again. While the role still lacks anything (a program not installed, a mytokens entry) no worker starts with it: its issues wait behind ready issues that can start, and doctor finds it (`role:<dir>`), naming each thing it lacks and the fix rolesmith gives, so the watch is handed it; the first check that passes starts them. A role directory without `rolesmith.toml`, or no role, starts with no check. An account is one of §1.7, refused at `issue add` unless stablehand keeps it (`stablehand account show <name>`) and a session may start on it here: its engine a harness factorium starts, not off, logged in, and its login portable or bound to this machine. What an issue took, `issue show` answers as `spent` (§4). |
| 4 | **Worker** | One harness session working one issue in its own worktree, branched from the proving tip. It ends with `factorium worker done`, `factorium worker escalate`, or by failing. |
| 5 | **Harness** | A coding agent program factorium starts: Claude Code, Codex, pi, DSH, Kimi Code, Grok, Antigravity CLI (`agy`). How each starts, where it keeps its session and which lines start and end a turn are its own module's to say. The session file is the turn. |
| 6 | **Kit** | The checkouts pinned to one engine (`kit pin`). The split (factorium#291, stablehand#24, factorium#306): which engine, model and account a session starts on is stablehand's, its order (`stablehand order set`) and its pick its one home, and so is each engine's command, its program and the flags it always takes (`stablehand command set\|show`, stablehand#30); factorium adds its own arguments (headless, streaming, resume), the guard and the session itself. An engine stablehand keeps no command for starts on its bare program with its stock arguments. With stablehand missing or failing, claude-code alone starts, by `claude --dangerously-skip-permissions` (built in, for that case only), on the host user's own login. The kit keeps no order, no model and no command: the commands it once kept move to stablehand when the daemon starts, once, and one stablehand refuses is a doctor finding (`kit:unmoved:<engine>`) until it keeps one for that engine. |
| 7 | **Account** | An engine subscription, kept in stablehand (sunfmin/stablehand, its docs/adr/0001), its one home: how it logs in, where it may run, its usage windows and the limits it met. factorium keeps no account of its own. A start asks `stablehand pick --running <account>=<n>,… --machine <this host>` once, `--running` counting factorium's own live sessions on each (its workers' and the watch's), and starts the engine it answers, on the model it answers (`null`: the engine's own), on the account it answers, by the kit's command for that engine (§1.6). A checkout's pin narrows it to that engine (`--engine <pinned>`); an issue's own model (`issue add --model`) passes `--model <m>`. A worker whose issue names an account starts on it, its pick narrowed to that account's engine (refused when a pin names another engine), on the model the pick answers there, unless the pick passed that account over. An engine with no account kept is skipped by the pick: nothing starts on a login stablehand does not keep. A session on an account is handed its login as `stablehand credential <account>` gives it, and factorium builds the session's home itself: a portable login's token variable, by a file the session reads first and removes, off its command line; a bound login's files, each linked into the home this host keeps for sessions on that account (`kept/<harness>@<account>`, one per account, so no two accounts share a home), in place of the host user's login and of any link of the harness's own that holds one or lies inside one (kimi's `credentials` directory). Nothing else of the account's home reaches the session. A harness of which this host has no account links the host user's own login, as its kept links name it. While the pick answers none for an issue (`none`, or its named account passed over), its issue waits, behind ready issues that can start, and holds admission (`kit: …`, the pick's reasons) only when none can; each tick asks again, so it starts once the pick's `until` passes or stablehand reads the account again. A worker whose screen says its account is used up ends and its issue starts again, and stablehand is told (`stablehand limit <account>`, no reset: until stablehand next reads it). A screen that says when its limit resets parks the worker until then instead (§2), told as `stablehand limit <account> --resets-at <time>`. One stablehand cannot take is said in the tick's answer (`unbooked`), and the worker's end does not wait on it. `factorium accounts` answers `stablehand status`'s accounts. When stablehand cannot be run (not found) or its pick answers a fault, factorium starts on claude-code alone, on the host user's own Claude login, by the kit's claude-code command (where a pin allows it), on the issue's own model if it names one, and on no other engine, since without stablehand it knows no other engine's accounts or windows; an issue that names an account waits. doctor finds it (`stablehand`), naming what failed and the fix (`brew install sunfmin/tap/stablehand`, or the fault's own words), so the watch is handed it. Once stablehand answers again, starts go through it as before. |
| 8 | **Admission** | Whether one more worker may start now, from the host's readings alone: CPU busy share (less what runs in started heavy slots, the queue's to bound), free memory, the disk floor, factorium's own share caps, the accounts' windows (as stablehand's pick judges them, §1.7), and the heavy queue keeping up — no heavy command waiting its turn past `queue_wait_bound` (the queue starts beside what runs only while the host has room, so it holds the busy share near the limit, and the load it holds back shows in no reading but that wait: factorium#194). No concurrency number. |
| 9 | **Resource** and **heavy command** | A resource is a name and a capacity rule, with one queue each, the rule of the neutral queue (`neutral/queue.rs`): the shortest declared hold first — a hold's `--for`; a command declares none and is read as `hold_default` — oldest first among equals; one waiting past `starve_bound` goes ahead of all that have not, oldest first, so a long one never starves. Nobody knows how long an issue takes before it starts; a holder knows how long it holds when it asks, so the order is the resource queue's, never the issue's: it replaces hand-ordered `issue block` chains between issues that share a device. `host`'s entries declare no hold, so its queue is oldest first; only the room test differs by rule, and a new kind of lock is a new rule and nothing else. `slots = n`: n holders at most (1 is a mutex): a named resource, declared in the host's settings (`settings --resource <name> --slots <n> [--note <what it is>]`, `--remove`; a phone serves every checkout on the host, so no checkout's `factorium.toml` names it). `room`: as many as the host's readings allow: the resource `host`, which is never declared — the heavy queue, as below. A heavy command asks for one or more (`worker exclusive --resource <name> …`, `heavy run --resource <name> …`; none named is `host`), granted all at once or not at all: one waiting for a resource whose slots are all held is passed over in every queue, as a stale one is, so it holds none up and never holds one while it waits for another. A worker's hold across commands (`worker hold <name> … --for <duration>`, a soak) waits as a command does; with `--between 22:00-07:00` (the host's local time, every day; an overnight run on the charger) it is passed over outside its window, joining the line only inside it, its starve bound counted from the window's opening; granted, it outlives its waiter until `worker release <name>` or its worker ends, and the worker's own commands asking for what it holds run inside it, taking no slot of their own. A hold is never cut short, whatever the hold bound; past its declared until it is a finding for the watch, as a silent worker is. A named resource is given back by one path: its command ends, `release`, or its holder is gone — a command's waiter died, a hold's worker ended — which the next tick gives back, kept on record (doctor's finding for an hour). What doctor's `resources` and `workers` show of each is their `--help`'s. A worker waiting for a resource is waiting, not silent. Promotion and cut (one per checkout, which the daemon orders itself), the issue claim and the accounts' windows are not resources: each could become a rule when a second need shows (factorium#284). The resource `host`: a command a worker runs through `factorium worker exclusive -- <cmd>`, one a program of the owner's that is not a worker (Guild's instances, guildd) runs through `factorium heavy run [--background] -- <cmd>` (its entry marked `program`), and every gate run. Its queue starts each when the host has room — by the same readings as admission, nothing else — never starves the oldest (it starts at once when nothing runs), starts one beside what runs only on a reading taken `heavy_settle` after the last start (so the load a start brings is read before the next), and ends one that holds others up past the hold bound (only one holding `host`: what waits for a named resource alone is held up by none of its queue). A run is ended whole, by one path whoever ends it (the hold bound, its limit, `unqueue`): each step runs in a session of its own, noted on its entry, so a process that left its process group or its parent is ended too (only one that starts a session of its own leaves it) — TERM, then KILL after a short grace; an unqueued run's waiter answers `unqueued`. Gates first, and a time a gate compares measured at the gate's priority: a gate (fast or slow, a worker's or the line's) and a worker's `worker exclusive` (a program's `heavy run`) run at the priority factorium runs at, so a baseline a worker records (`slimfit ratchet`) is measured as the gate measures it; heavy work whose time no gate compares — a cut, idle work, a worker's `worker exclusive --background` (a mutation run), a program's `heavy run --background` — runs at background priority, its whole process tree: a nice of 10, so a gate gets the CPU first when they compete, and the run has every core and full disk when the host has room (not Darwin's background band: efficiency cores and throttled disk even when idle). A line move (promotion, cut) past its bound goes ahead of workers' and programs' waiting entries, taking the next free slot; a running entry is never cut short for it. Behind it, an urgent issue's (§1.17), as its issue is marked at each ask. Then a landing's fast gate (short, one per checkout), oldest first. The rules are those of the daemon that runs, set as it starts (a newer binary swapped to, or an older one a rollback went back to), never by another process that asks: a newer binary whose proof failed does not run the daemon, which would else be refused its own gates (factorium#241). A waiter of an older binary is refused, and execs the installed one (its own binary found before it waits, so an upgrade meanwhile does not hide it), keeping its place; an entry waiting under replaced rules is passed over until its asker asks again under the daemon's, so it holds none up. A worker's or a program's entry that leaves unstarted without the owner taking it off (its waiter refused and failing to exec, or gone) keeps its place for `heavy_keep`: the same worker (or program) asking the same command again within it takes that place, not the back; so a daemon restart or a client that gives up and asks again loses no turn (factorium#193). |
| 10 | **Hot worker** | A worker whose processes take more than their share of CPU, memory, disk written, disk held or process creation, or that stalled holding build locks. Its evidence is taken at once; it is a finding for the watch. |
| 11 | **Gate** | The repo's own fast and slow commands, each with its limit. A green run is recorded against the tree it ran on, so the same tree is never gated twice. |
| 12 | **Landing** | A worker's `done`: its branch is merged onto the proving tip, the fast gate runs on the merge, and the proving branch is moved to it by a push that requires the tip it read. One landing per checkout at a time. The daemon runs the fast gate beside its ticks, as a promotion's slow gate (§1.13), and the first tick after it ended records it and pushes — unless the worker no longer asks to land, or its branch moved since the merge: then it lands again from its branch as it is (factorium#231). Green closes the issue and ends the worker. A merge whose only conflicts are records the gate regenerates (`regenerated`, §5: slimfit's baseline, truefit's models) is not red: it takes proving's copy of each, and once the fast gate is green on that merge, the land commit holds them as the gate left them (a record the gate does not rewrite, such as slimfit's baseline, stays proving's) and its message names them. Any other conflict is red, naming every conflicting file (factorium#225). **When the merge is checked** (factorium#307): not only at its turn. The first tick after `done`, and each tick after that while it waits its turn on which its branch or the proving tip moved since it last merged, checks the merge of every branch waiting in line onto the proving tip, without a work tree or a gate (`git merge-tree`), by the same rules (a conflict only in `regenerated` records merges). One that does not merge is red on that tick, in the words of the red at its turn (the conflicting files, and what landed since, §2 step 5), and leaves the line; one that merges keeps its place. A landing whose merge runs is not checked again. |
| 13 | **Promotion** | One at a time per checkout: its gate on the proving tip — the slow steps alone on a green landing's commit (none: `slow` absent or `fast`'s), else the fast gate first — then the base branch fast-forwarded to it by a push that requires the base it read. Started when the host is idle, or once the oldest unpromoted landing has waited the promotion bound less the hold bound: its gate then goes ahead of workers' waiting heavy commands, so one running then, held at most the hold bound, cannot push it past its bound. The daemon runs its gate beside its ticks, so workers start and are watched while it runs, and the first tick after it ended records how it ended and pushes; while a move of a checkout runs so, none of its other moves (a landing, a promotion, a cut) is carried, and the daemon does not restart onto a new binary. The checkout's local base follows: fast-forwarded to the promoted tip, and on every tick to origin's base as last fetched, never further — in the tree it is checked out in, if that tree is clean. One with commits of its own, or uncommitted changes, is never reset: it is left, a doctor finding naming the checkout and how far behind it is. |
| 14 | **Cut** | A version made from the base branch when the checkout is idle, or once its oldest uncut landing has waited the cut bound less the hold bound, ahead of workers' heavy commands as a promotion is: set, push its own branch, gate, tag, publish, install. Its steps run beside the daemon's ticks as a promotion's slow gate does; the tag and the install follow on the tick after they ended, and the installed binary's proof (§4) runs beside the ticks too: the cut is recorded, made or failed with the proof's why, on the first tick after the proof ended, and until then none of the checkout's other moves is carried (factorium#242). A failed cut holds every later cut of its checkout until one of three: each tick reads it back and records it made once its tag and install hold; a cut by hand tries again; or a promotion moves the base to a commit other than the one it was cut from (the fix reaches the base only so), and the cut is owed again by the rule above. One that fails again is a failed cut again, a new finding; a promotion of the same commit retries nothing (factorium#251). |
| 15 | **Watch** | One resident agent session on the host. A **look** (no model) every five minutes, and on the next tick once a worker starts waiting for the watch, reads `doctor`'s findings; each condition is an **episode**, and an episode starts a **round**: a brief, an hour (`round_bound`), and a report the watch files with `factorium watch report`. No finding stays standing after its round with nobody looking at it: an episode still found `round_again` (30 min) after its round ended — reported, or past its bound — starts another, whose brief carries the last report ("last time you reported …; the finding is still there"), up to `rounds_max` (3) rounds; past them it starts none, and doctor lists it under `unresolved`, with its rounds and last report, for the owner, while it stands. A worker's `done` refused because it still waits leaves its finding standing, so it gets the same next round. `watch tell` to a worker that waits for the watch is `watch restart`: it ends the wait, so a word that sends the worker back to work never leaves it waiting, its `done` refused (factorium#268). Escalations, failed workers, hot workers, red promotions, red idle work, failed cuts and resource holds all go to it. A worker the watch parks behind other issues — it blocks the worker's issue on them (`issue block`) — is no finding while any of them is unmet (doctor lists it as `parked`, with what it waits for), and goes back to work by itself on the first tick that finds all of them met (§2), which ends its episode; one whose issue has nothing left to wait for stays the watch's. It acts with the operator's commands. It works by the rules of the factorium that runs: its session records which rules (their hash) it was briefed with, and a look that finds them other than the running binary's — a cut installed a version that changed them — closes that session between rounds, never in one, and starts a fresh one briefed with the new rules before the next round. |
| 16 | **Idle work** | What a checkout declares under `[idle]` (§5): steps too slow for any gate (a long soak or load test), run in their own clone at the base's tip while the host is idle — no live worker works (each waits for the watch, or none lives), the heavy queue holds nothing, and the host has room — at most once per `every` (from the last run's start) and within `limit` for all the steps. It takes a heavy slot after the lines' moves and the tick's start, so it never goes ahead of a promotion, a cut, a worker's start or a waiting heavy command; it runs beside the ticks, through the heavy queue's one path (its sessions noted on its entry, so `unqueue` ends it whole as it does any running entry, unrecorded), and the first tick that finds the host no longer idle (a worker at work, anything else queued) ends it, unrecorded, to run again in the next idle window, so a worker that wants room waits at most a tick. A run that ends is recorded with its base tip and tail; past `limit` it ends red. A red run is a doctor finding, and so a watch episode, until a later run is green. One runs at a time on the host. **Tasks**: its `tasks` line, run after the steps, prints what read them and then slimfit's `next --all` answer (its rules are slimfit's: `slimfit help`), and any other answer of its shape that names what read it in `by` (this repo's crates' lines, §5). Before the run is recorded, the first task of each kind is filed as an issue of the checkout, carrying the task as slimfit gave it and the version and base tip it was read at, unless an issue filed for that kind is open, or one filed for the same task (kind and files) closed by a landing (`landed as <sha>`) that is not an ancestor of the tip the run read. Promotion fast-forwards the base (§1.13), so that landing reaches the base unchanged; once it is in the tip read, a task the tool still lists is work not done and is filed again. An issue closed without a landing (`issue close`) holds nothing back: a verdict to leave a task as it is is the tool's keep in the repo (`slimfit keep`, `truefit promise --mark <hash> not --why`), where the tool reads it. So each kind has at most one open issue, no kind waits on another, and once one closes the next run files the next of its kind. Its worker does that one task by its steps, accepts it as the task says, and lands; its judgment is accepted (slimfit#44). An answer that is no tasks makes the run red. A host may stay busy for days, so tasks have a bound: once a checkout's last run started more than `every` plus `tasks_bound` (a setting, a day by default) ago, or it never ran, its tasks line runs without an idle host, alone (not the steps), as a heavy command at background priority that yields to nothing but the hold bound; it is recorded as any run is. **By hand**: `factorium idle <checkout>` runs it now, as `promote` and `cut` move the line now: the steps, then the tasks line, in its own clone at the base's tip, without waiting for an idle host or `every`. It takes the heavy queue's one path at background priority and, as the `tasks_bound` run, yields to nothing but the hold bound: a tick that finds the host busy does not end it, and `unqueue` ends it whole. It is recorded as any run is, its start beginning `every` again, and its tasks are filed by the same rule, so it never files a second open issue of a kind. The command waits for the run and answers its record. It is refused when the base declares no `[idle]`, and while idle work of any checkout runs or waits on the host, naming that entry and `factorium unqueue <id>`. **Off on a host**: `[idle]` lives in the repo, so every host with a checkout of it would run it and file the same tasks into its own tracker. `checkout edit --no-idle` turns a checkout's idle work off on this host alone, without changing the repo (`--idle`, back on): off, no idle run of it starts here, neither in an idle window nor past `tasks_bound`, so none of its tasks is filed here, and its red last run is no doctor finding. `factorium idle <checkout>` still runs and files as it does with idle work on. |
| 17 | **Urgent** | The owner's mark on an issue (`issue add --urgent`, `issue edit --urgent`/`--no-urgent`; never `worker file`). Blocks still hold it. Among urgent ones the ordinary order holds. Once its worker starts, until it closes or is unmarked, nothing else starts, and what runs is cut short once it waits for it (the halt). What shows it is `issue add --help`'s. |

`doctor` is not a concept of its own: it is the findings computed from the
database and the host's readings, each with how to fix it. It is the
watch's look and the operator's view. The operator's view first answers
whether a daemon runs: its pid file names a live process that holds its
lock, and that one ticked (`daemon.tick`) within five `tick_every`, never
under two minutes. When none does, that is the first finding, with its fix;
and every other command but `run`, `stop` and `service` says so in one line
on stderr, so whoever touches factorium next sees it. The watch's look
leaves it out: the daemon runs the look.

## 2. The issue's path

1. `factorium issue add` makes an issue, ready unless it says otherwise,
   with the issues it waits for (`--blocked-by N`, repeatable) recorded
   with it, so no tick sees it ready without them. One of another
   checkout is `--blocked-by <checkout>#N`, met once it is closed (landed);
   `<checkout>#N@cut` is met only once a cut of that checkout holds its
   landing, and installed it where its `[cut]` names a formula, for a tool
   used from another repo. `issue block` takes the same names. Every
   block says why (factorium#285): `--blocked-by` and `issue block --by`
   are refused without `--why <text>` (one why serves all the
   `--blocked-by` of one `add`), kept with the block; `issue show`
   answers each thing it waits for in `waits`, with its why and how it
   was met, and doctor's `parked` names each unmet one with its why. A
   block made before whys were asked for reads "(no reason given)". Two
   issues that only use one device or host do not block each other: that
   is a resource (§1.9), which the watch turns such a block into. An issue
   can wait until a time too (factorium#229): `issue add --not-before
   <time>` (RFC 3339 with its offset, or ms; `worker file` takes it as
   well), or on an existing one `issue block --until <time>`, which
   replaces the time it waited for (`--until --undo`: none). It is met on
   the first tick at or after that time, as a met block is; `issue show`
   answers it (`not_before`, in ms and UTC), and doctor lists a ready issue
   waiting for its time under `not_before`, with the time.
2. A tick admits one start per reading, of the ready issues unblocked
   and untaken: urgent (§1.17) first, held only by the memory and disk
   floors; then fewest live workers in its checkout, ready longest,
   checkout, number.
3. The worker resumes in the worktree its issue's last attempt kept, if
   that one holds work that is nowhere else (commits not on proving, or
   changes not committed), and its brief says what is there; else its
   worktree is branched from the proving tip. Its harness and
   engine, model and account are stablehand's pick, started by the command stablehand keeps for that engine (§1.6, §1.7),
   and its account's login read (`stablehand credential`); the guard is installed; its brief is sent; the
   session file says when its turn starts and ends. Text is typed at a
   session only once its harness's own screen is up there: typed at a
   shell, a line is a command it runs. A question the harness opens on its
   own mid-session (a dialog that would take the line) is dismissed first
   with the harness's own key.
4. The worker checks what it changed (fmt, clippy, its crates' tests,
   through `worker exclusive`), then runs `factorium worker done`: a
   change's one full gate is its landing's, on the merge (§1.12);
   `worker gate` is allowed, not required. An issue whose work changes no
   code (a publish, a measurement) it finishes, its branch holding no
   commits, with `factorium worker done --no-landing "<evidence>"`: the
   daemon closes the issue with it, running no gate (factorium#282). What it wants kept (a measurement)
   it records with `factorium worker comment`, a comment on its issue
   that changes nothing else and goes to no one (decisions go to the
   watch: `worker escalate`). A follow-up its issue asks for
   it files with `factorium worker file <checkout> <title> --body-file <f>
   [--blocked-by …]`, as `issue add` does; on its own checkout the filed one also waits for its issue
   to land, unless `--independent`
   (factorium#184). Both keep the record as its comments: its issue says
   "filed <checkout>#<n>", the filed one which issue and worker filed it.
   It changes no other issue.
5. Landing (§1.12), the urgent first (§1.17). Red: the worker is told why and goes on; red twice in
   a row goes to the watch. Its merge is checked from `done` on, not only
   at its turn (§1.12, factorium#307). A branch that does not merge onto the proving
   tip is not the second red when the red before was one onto an older
   tip, which the branch now holds: proving moved since the worker merged
   what it was told (factorium#171). It is told as for a first red, with
   what landed since that tip and the files that conflict; past
   `moved_reds_max` of these in a row, it counts. A red that does not reach the session's prompt
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
   issue stays ready or waiting for a human as it was.
   A start and the commands that abandon a worktree (start-over, close,
   discard) each claim the issue, one at a time: a start from taking it to
   its worker recorded started (or undone), a command from its read to its
   act's end. A start skips a claimed issue, and a command is refused while
   one is under way, so none acts on what it read while another changed
   it; a claim whose process is gone is cleared by the next tick (or the
   next command's claim).
   Closing an issue unlanded (`issue close`, by the owner or the watch)
   removes the worktree its last attempt left on disk the same way: its
   uncommitted changes are committed, its branch is kept as
   `<branch>-abandoned`, named in a comment written before the close (a
   closed issue takes none), and the worktree goes with its Orca
   terminals. An open issue owns one worktree: the one its last started
   attempt works in, which is attempt 1's, on attempt 1's branch, when a
   later attempt resumed it (step 3); the attempt a branch names does not
   decide it. A worker's worktree (on a branch
   `factorium/<checkout>-<n>-<attempt>`) that no open issue owns — its
   issue closed or unknown, or not its last started attempt's — is a
   doctor finding, fixed by `discard <worktree>`, which removes it the
   same way.
   A git worktree registered inside a worker's worktree (slimfit's
   comparison worktree, `.slimfit/out/before`) goes first when it holds
   no work: clean and detached at a commit on the proving or base branch.
   One that holds work is kept where it is: the removal (landed, closed,
   started over or discarded) changes nothing, a discard or close is
   refused naming it and why, and doctor's finding names it.
7. Promotion and cut follow on their own (§1.13, §1.14); idle work runs
   when nothing else does (§1.16).
8. A checkout whose proving tip pins another checkout by git rev in its
   `Cargo.toml` gets one issue, on the first tick that finds the pin behind
   that checkout's base tip as last fetched: "Bump <dep> to its base tip
   (<sha>)", listing the landings since the pin. No second while one is
   open (factorium#127).

Off the path, everything goes to the watch: `worker escalate`, a worker
whose session died, never came up (five minutes after it started, a fixed bound), went
silent past its bound (not while its issue waits for anything unmet, its
silence counted from when all of it is met, factorium#283; nor within a wait it declared on a long command of
its own, `worker waiting --why … --until …`, held to two hours, which doctor
shows; past it, the finding names what it said), or hung past it (its turn open, its processes next
to idle, its screen unchanged), a hot worker, a red promotion, red idle work, a failed cut (until the next promotion of a newer commit owes it again, §1.14), admission held
for one reason past the host's span. So does a line that stopped:
whatever it owes is done within its bound or is a finding — a heavy
queue that starts nothing past `stall_bound` (or whose oldest waits past
`hold_bound` and `stall_bound` more), a landing no gate runs for on a
line that has not moved for `stall_bound` (no landing, promotion, cut or
line job started on its checkout in that time, nor its own ask: one that
waits its turn behind landings that move is no stall, and `doctor` lists
it under `landing` with its place in its checkout's line), the oldest unpromoted landing past `promotion_bound` with
no promotion running, the oldest uncut promotion past `cut_bound` with no
cut running. A worker whose issue the watch blocks (on issues, or
until a time) is parked, waiting or working: once it is idle — it waits,
for any reason, or is in a wait it declared (`worker waiting`) — the first
tick that finds everything its issue waits for met, as step 1 defines met,
met before it went idle or after, sends it back to work (ending a wait it
declared) as `watch restart` sends it, told (in a comment on its
issue by `factorium`, typed at it) which blockers were met and at which
landing (their issues' close), and to merge the proving tip before it gates
again. No round runs for it, and nobody has to notice the landing.
A worker whose screen says its engine is at its limit, with when it resets
(agy's quota), is parked the same way until then: what is passed to it is
held, not typed; it is no finding (doctor lists it as `limited`, with the
reset); its account (stablehand told the reset, `stablehand limit
--resets-at`), or its harness on the host's own login, starts no
worker before the reset; and the first tick past the reset sends it back,
what was held typed first. Its quota still on its screen after that is no
new limit. A worker waiting for the watch is read for these two as well
(a used-up account, a limit with its reset): its finding, hung or a word
that did not reach it, is the spent engine's doing. A used-up account is
told to stablehand (`stablehand limit <account>`), which holds it until it
next reads it; the host's own login, used up, rests its harness the hour,
a fixed bound, not a setting.
The watch fixes, restarts, re-briefs, leaves an issue to a person (`issue wait-for-human`, for a decision only a person can make) or closes, and reports.
Only spending money is left to the owner, in the report's `human` part,
which `doctor` shows. Beside it doctor answers `asks`: every issue of every checkout that
waits for a person, answered as `doctor --help` says.
Everything else is decided without the owner, and each decision is kept
once, in the issue it belongs to (its body when filed, a comment after), as
a **Decided record**:

```
Decided (<who>, for Felix): <what, one line>
Options: A) …  B) …
Chose: B, because …
Undo: <how to take it back>
```

A worker writes one (`worker comment`) when it makes a choice its issue did
not settle or does other than its issue says; the watch, when it decides in
a round (restart, re-brief, park, close); a second agent (picker, reviewer)
writes none, its answer's why being its record. Their briefs say so.
Doctor answers `decided`: every record, in any issue body or comment
(a comment is one only when its first line starts `Decided (`; a line
inside a fenced code block, ``` or ~~~, quotes the shape and is none) of any
checkout, made since the owner last ran `decided --read`, newest first,
each with its checkout and number, who, what, options, chose, undo and when.
`factorium decided` answers those of the last 7 days, read or not; `--read`
(the owner's: the watch's brief says it never runs it) marks every one until now read. Nothing copies a record
out of its issue.
Each record has a number, `D<n>`, for the owner to say: one sequence for the
host (all checkouts), 1, 2, 3, …, kept on a stream of its own that gives
each number once. A record takes the next when it enters its issue — a
comment that is one (its event holds the number; a comment that is a record
and has none is refused), or a body's at `issue add` and `issue edit` (the
event that sets the body holds a mark for each of its records: the number
and the record's head, who and what). An edit keeps the number of each
record whose head stays as it was, whatever else of it changes, and gives
the next to each one new; a number whose record an edit removed is not given
again, and no other moves. Records made before numbers were given take
theirs once, oldest first by when they were made (a body's, when its issue
was filed), before any new one takes the next: an open issue's as an event
of its issue; a closed one takes no event (its stream has ended), so the
sequence's stream places them, each named by its issue and its place in it
(a comment's index, a body record's head), never copied. Doctor's
`decided` and `decided` answer each record's `id`; `decided D17` answers one
(`decided --help`). `issue show` answers `decided` on each
comment that is a record, and the body's records' numbers as `decided`.

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
daemon moves to the newer binary by itself (factorium#270). The log is
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
given no grace after it, as other messages are (factorium#281). And no
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
`issue show` answers them, and `spent`, what the issue took, for whoever settles its cost (Guild): each attempt's time from its start to its end (a live one's, to now), in ms, and the tokens its session spent, given (cached ones counted) and written, read from the session files that record its turn: as it ends, and kept, since its harness may clear them (a live one's, as they say now); with their sums. Claude Code's files say (each assistant message's `usage` once, by its id), Codex's (its last `token_count`), pi's (each assistant message's `usage`) and agy's (each `PLANNER_RESPONSE` of its `transcript.jsonl`: `input_tokens` and `cache_read_tokens` given, `output_tokens` written); DSH's, Kimi Code's and Grok's are not read, their tokens `null`. Each attempt also answers the role it started with, as `role` (`dir`, its directory, and `from`, `issue` or `checkout`, as `role_from` says, §3), and as `role_hash` a hash of what that role gave it, taken at its start: its rules and every file under its skills, read through the link, each file by its path under the skills and its bytes. A role directory is linked live, so a role changed while a worker runs changes what that worker reads; the hash says which content an attempt started with. On each tick, a live worker whose role directory now holds other content than it last knew of (as it started, or as it was last told) is told so once, through its outbox as a comment is: one line, `factorium (your role changed): …`, naming each file by its path under the role's directory (`CLAUDE.md`, `.claude/skills/…`) that changed, was added or went, and asking it to re-read the role's core skill and those files before its next step; where the role's rules changed, its session's rules are written again with them. What it was told is recorded on its worker, so one change is told once; `role_hash` stays the hash at its start. One waiting for the watch, unless parked on its engine's limit, is told once it works again; a worker with no role, or whose role did not change, is told nothing (factorium#273). It is of the content alone and the same from one build to the next: factorium reads nothing else of the role (no rolesmith file, no rev). An attempt with no role answers `null` for both (factorium#250). A picker's (`worker pick-mutants`) is not among them: its cost is its own comment (§6). A worker's `escalate` text, and the text of
`watch tell` and `watch restart`, become comments when they are typed,
delivered or not. Each reaches the worker at most once: the line `tell` or
`restart` types is its delivery, nothing types it again, and a worker's
own escalation is typed back at no one. Comments made before authors and
times were kept read as the owner's (start-over's as factorium's), with no
time.

**Where an attempt's time went** (factorium#308): `issue show --steps`
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
link to swap to is proven (factorium#252). A cut of the formula proves
it right after the install (the daemon's cut beside its ticks, recorded on
the tick after the proof ended); one that fails is a failed cut, its why the
error and the copy's path (kept for a look), and the old binary runs on.
Which host installs factorium, and when (factorium#269): a host one of
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
