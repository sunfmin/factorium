------------------------------ MODULE Worktrees ------------------------------
(***************************************************************************)
(* An issue's attempts and their worktrees (SPEC §2 steps 3 and 6), as    *)
(* factorium keeps them: crates/factorium/src/kept.rs decides what a      *)
(* worktree holds, which one a start resumes, which one an open issue     *)
(* owns and how one is abandoned; life.rs starts and ends a worker;       *)
(* issues.rs runs `issue start-over` and `issue close`; line.rs lands.    *)
(*                                                                         *)
(* One issue, up to A attempts. A worktree is named by the attempt that   *)
(* made it (`factorium/<checkout>-<n>-<t>`): tree t. Work is tokens, each *)
(* made once by a worker (a change in its worktree), then committed on    *)
(* its branch, merged onto proving, or kept on `<branch>-abandoned`.      *)
(* What the database holds (the issue, its attempts) and what the disk    *)
(* and the remote hold (worktrees, branches, the proving tip, readable or *)
(* not) are one state `s`; the processes acting on it are the daemon (a   *)
(* tick's start; the landing, its line), and the CLI commands start-over, *)
(* close and discard, each its own process. Each step a process takes is  *)
(* one action, named as the code names it; any process may die between   *)
(* two of its steps (`Dies` names where), and comes back idle.            *)
(*                                                                         *)
(* `Races`: whether a CLI command's steps may interleave with the         *)
(* daemon's and each other's (they are other processes, so in the code    *)
(* they may: models/worktrees.sh finding). A worker's own steps (its      *)
(* changes and commits, failing) interleave with everything either way.   *)
(*                                                                         *)
(* `Bug` names a broken variant (BrokenVariants below); "none" is the     *)
(* code as it is. `models/worktrees.sh broken` shows each property        *)
(* failing on its variant.                                                *)
(*                                                                         *)
(* Each step is an operator from a state to the next (a guard `...G`, and *)
(* `...F`, what it does), so a trace's records, each a whole command or a *)
(* whole tick, are those steps in order with no other between (TraceNext).*)
(***************************************************************************)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    A,          \* attempts: 1..A, and so trees 1..A
    W,          \* work tokens a worker makes: 1..W
    Races,      \* whether CLI commands interleave with the daemon and each other
    Dies,       \* the points (a process's `at`) it may die at
    Bug,        \* "none", or a broken variant
    Depth       \* a bound on the steps a model check explores

Attempts == 1..A
Trees == 1..A
Procs == {"daemon", "land", "over", "close", "discard"}
CLIs == {"over", "close", "discard"}
Idle == [at |-> "idle", a |-> 0, t |-> 0]
None == [t \in Trees |-> {}]
No == [t \in Trees |-> FALSE]

\* The state before anything happens: an issue, ready, never taken.
Start0 == [
    status   |-> "ready",                     \* the issue: ready, human (waits for a person), closed
    taken    |-> 0,                           \* issue.worker: the attempt whose worker holds it
    attempts |-> 0,                           \* issue.attempts
    started  |-> [a \in Attempts |-> 0],      \* Started.worktree of attempt a: its tree (0: none recorded)
    live     |-> [a \in Attempts |-> FALSE],  \* attempt a's worker is at work (its session up)
    disk     |-> No,                          \* tree t's worktree is on disk
    dirty    |-> None,                        \* its changes not committed
    hasbr    |-> No,                          \* its branch exists
    br       |-> None,                        \* what its branch holds
    kept     |-> None,                        \* what `<its branch>-abandoned` holds
    proving  |-> {},                          \* what the proving tip holds
    tipok    |-> TRUE,                        \* the proving tip can be read (fetched)
    made     |-> {},                          \* every token a worker made (no code holds this)
    pc       |-> [p \in Procs |-> Idle]]      \* where each process is: its step, attempt, tree

VARIABLES
    s,      \* the state, as above
    last    \* the last step and what it read: for the properties

vars == <<s, last>>

TypeOK ==
    /\ s.status \in {"ready", "human", "closed"}
    /\ s.taken \in 0..A /\ s.attempts \in 0..A
    /\ \A a \in Attempts : s.started[a] \in 0..a
    /\ \A t \in Trees : s.dirty[t] # {} => s.disk[t]
    /\ \A t \in Trees : s.disk[t] => s.hasbr[t]

Init == s = Start0 /\ last = [act |-> "init", p |-> "", t |-> 0, tip |-> TRUE, removed |-> FALSE]

-----------------------------------------------------------------------------
(* What kept.rs reads.                                                     *)

\* last_started: the last attempt recorded started (0: none).
LastStarted(S) ==
    LET st == {a \in Attempts : S.started[a] # 0}
    IN IF st = {} THEN 0 ELSE CHOOSE a \in st : \A b \in st : b <= a
\* The tree it works in (0: none).
LastTree(S) == IF LastStarted(S) = 0 THEN 0 ELSE S.started[LastStarted(S)]

\* holds_own: commits of its branch not on the proving tip, the tip read or
\* not (`tip`: whether it was), or changes not committed. What cannot be
\* read is held: an unread tip, a branch that is gone.
Committed(S, t, tip) ==
    IF ~tip THEN Bug # "release_unknown"
    ELSE ~S.hasbr[t] \/ S.br[t] \ S.proving # {}
HoldsOwn(S, t, tip) ==
    \/ Committed(S, t, tip)
    \/ S.disk[t] /\ S.dirty[t] # {} /\ Bug # "ignore_dirty"

\* orphaned: tree t on disk, on its branch, that no open issue's attempt
\* owns: its issue closed; or its last started attempt works in another,
\* and t is no newer attempt's, still starting.
Orphaned(S, t) ==
    /\ S.disk[t] /\ S.hasbr[t]
    /\ \/ S.status = "closed" /\ Bug # "closed_unfound"
       \/ /\ S.status # "closed"
          /\ t # (IF Bug = "orphan_by_attempt" THEN LastStarted(S) ELSE LastTree(S))
          /\ ~(t > LastStarted(S) /\ S.taken = t)

\* SPEC §2.6: an open issue owns one worktree, its last started attempt's.
Owner(S, t) == S.status # "closed" /\ t # 0 /\ t = LastTree(S)

\* remove_worktree: Orca removes it with its terminals, git forgets it and
\* deletes its branch.
Remove(S, t) == [S EXCEPT !.disk[t] = FALSE, !.dirty[t] = {}, !.hasbr[t] = FALSE, !.br[t] = {}]

\* Under ~Races, a command begins only while no other process is in one.
Alone(S, p) == Races \/ \A q \in Procs \ {p} : S.pc[q].at = "idle"
At(S, p, at) == [S.pc[p] EXCEPT !.at = at]

-----------------------------------------------------------------------------
(* The daemon's start (life.rs start_worker, kept.rs next_worktree).       *)

\* The tick takes the issue: the proving tip is fetched first (unread, the
\* start refuses and takes nothing), then TakeIssue counts an attempt.
TakeG(S) == /\ S.pc.daemon.at = "idle" /\ Alone(S, "daemon")
            /\ S.status = "ready" /\ S.taken = 0 /\ S.attempts < A
            /\ (S.tipok \/ Bug = "start_unknown")
TakeF(S) == LET a == S.attempts + 1 IN
            [S EXCEPT !.attempts = a, !.taken = a, !.pc.daemon = [at |-> "taken", a |-> a, t |-> 0]]

\* next_worktree: the worktree its last attempt kept (on disk, holding work
\* that is nowhere else against the tip it read), else one Orca makes on a
\* new branch from the tip.
DecideG(S) == S.pc.daemon.at = "taken"
DecideF(S) ==
    LET a == S.pc.daemon.a  lt == LastTree(S) IN
    IF lt # 0 /\ S.disk[lt] /\ HoldsOwn(S, lt, TRUE)
    THEN [S EXCEPT !.pc.daemon = [at |-> "opened", a |-> a, t |-> lt]]
    ELSE [S EXCEPT !.disk[a] = TRUE, !.hasbr[a] = TRUE, !.br[a] = S.proving, !.dirty[a] = {},
                   !.pc.daemon = [at |-> "opened", a |-> a, t |-> a]]

\* Its session opens and it is recorded started (StartWorker): at work.
RecordG(S) == S.pc.daemon.at = "opened"
RecordF(S) == LET a == S.pc.daemon.a IN
              [S EXCEPT !.started[a] = S.pc.daemon.t, !.live[a] = TRUE, !.pc.daemon = Idle]

\* Its session does not open: undo_start frees the issue and releases the
\* worktree (finish: it goes unless it holds work of its own, against the
\* tip read now).
UndoRm(S) == ~HoldsOwn(S, S.pc.daemon.t, S.tipok)
UndoF(S) == LET t == S.pc.daemon.t
                S1 == IF UndoRm(S) THEN Remove(S, t) ELSE S
            IN [S1 EXCEPT !.taken = 0, !.pc.daemon = Idle]

-----------------------------------------------------------------------------
(* A worker's own steps, in the worktree its attempt works in.             *)

Tree(S, a) == S.started[a]
EditG(S, a) == S.live[a] /\ S.disk[Tree(S, a)] /\ Cardinality(S.made) < W
EditF(S, a) == LET t == Tree(S, a)  n == Cardinality(S.made) + 1 IN
               [S EXCEPT !.dirty[t] = @ \cup {n}, !.made = @ \cup {n}]
CommitG(S, a) == S.live[a] /\ S.disk[Tree(S, a)] /\ S.dirty[Tree(S, a)] # {}
CommitF(S, a) == LET t == Tree(S, a) IN
                 [S EXCEPT !.br[t] = @ \cup S.dirty[t], !.dirty[t] = {}]
\* Its session dies (or it escalates): no longer at work, its issue still held.
FailG(S, a) == S.live[a]
FailF(S, a) == [S EXCEPT !.live[a] = FALSE]

\* It ends keeping its worktree, its issue free again: `issue
\* wait-for-human` (waiting for a person), `watch restart` of one whose
\* session is gone (ready).
EndsG(S, a) == S.taken = a /\ S.started[a] # 0 /\ S.status # "closed" /\ Alone(S, "")
EndsF(S, a, st) == [S EXCEPT !.live[a] = FALSE, !.taken = 0, !.status = st]
\* `issue ready`.
ReadyG(S) == S.status = "human" /\ Alone(S, "")
ReadyF(S) == [S EXCEPT !.status = "ready"]

\* Landing (line.rs): `worker done`; the branch merged onto the proving
\* tip, gated green, pushed; the worker ends landed, its issue closes.
LandG(S, a) == /\ S.live[a] /\ S.taken = a /\ S.tipok
               /\ S.pc.land.at = "idle" /\ Alone(S, "land")
LandF(S, a) == LET t == Tree(S, a) IN
               [S EXCEPT !.proving = @ \cup S.br[t], !.live[a] = FALSE, !.status = "closed",
                         !.taken = 0, !.pc.land = [at |-> "landed", a |-> a, t |-> t]]
\* Then its worktree goes, unless it holds work of its own (release_worker).
ReleaseG(S) == S.pc.land.at = "landed"
ReleaseRm(S) == ~HoldsOwn(S, S.pc.land.t, S.tipok)
ReleaseF(S) == LET S1 == IF ReleaseRm(S) THEN Remove(S, S.pc.land.t) ELSE S IN
               [S1 EXCEPT !.pc.land = Idle]

-----------------------------------------------------------------------------
(* The CLI commands (issues.rs, kept.rs).                                  *)

\* `issue start-over`: refused while a worker is on the issue; else the
\* worktree its last attempt kept (kept_now: against the tip as read now,
\* unread meaning held) is abandoned.
OverG(S) == LET lt == LastTree(S) IN
            /\ S.pc.over.at = "idle" /\ Alone(S, "over")
            /\ (S.taken = 0 \/ Bug = "start_over_live")
            /\ lt # 0 /\ S.disk[lt] /\ HoldsOwn(S, lt, S.tipok)
OverF(S) == [S EXCEPT !.pc.over = [at |-> "commit", a |-> 0, t |-> LastTree(S)]]

\* `issue close`: the worktree its last attempt left on disk (left), if
\* any, is abandoned once the issue closes. With none, it just closes
\* (a worker on it ends, its worktree released: there is none on disk).
CloseG(S) == S.pc.close.at = "idle" /\ Alone(S, "close") /\ S.status # "closed"
Left(S) == IF LastTree(S) # 0 /\ S.disk[LastTree(S)] THEN LastTree(S) ELSE 0
Shut(S) == [S EXCEPT !.status = "closed", !.taken = 0, !.live = [a \in Attempts |-> FALSE]]
CloseF(S) == IF Left(S) = 0 THEN Shut(S)
             ELSE [S EXCEPT !.pc.close = [at |-> "close", a |-> 0, t |-> Left(S)]]
\* abandon's first steps for a close: its comment, then the close (a worker
\* on it ends, keeping its worktree).
ShutG(S) == S.pc.close.at = "close"
ShutF(S) == [Shut(S) EXCEPT !.pc.close.at = IF Bug = "close_no_keep" THEN "remove" ELSE "commit"]

\* `discard <worktree>`: one doctor finds that no open issue owns.
DiscardG(S, t) == S.pc.discard.at = "idle" /\ Alone(S, "discard") /\ Orphaned(S, t)
DiscardF(S, t) == [S EXCEPT !.pc.discard = [at |-> "commit", a |-> 0, t |-> t]]

\* abandon, by p: its uncommitted changes committed (refused if the
\* worktree is gone), its branch kept as `-abandoned` at HEAD, the
\* worktree removed with its branch.
AbCommitG(S, p) == S.pc[p].at = "commit"
AbCommitF(S, p) ==
    LET t == S.pc[p].t IN
    IF ~S.disk[t] THEN [S EXCEPT !.pc[p] = Idle]
    ELSE IF Bug = "abandon_no_commit" THEN [S EXCEPT !.pc[p].at = "keep"]
    ELSE [S EXCEPT !.br[t] = @ \cup S.dirty[t], !.dirty[t] = {}, !.pc[p].at = "keep"]
AbKeepG(S, p) == S.pc[p].at = "keep"
AbKeepF(S, p) == LET t == S.pc[p].t IN
                 IF ~S.disk[t] THEN [S EXCEPT !.pc[p] = Idle]     \* its HEAD cannot be read
                 ELSE [S EXCEPT !.kept[t] = S.br[t], !.pc[p].at = "remove"]
AbRemoveG(S, p) == S.pc[p].at = "remove"
AbRemoveF(S, p) == [Remove(S, S.pc[p].t) EXCEPT !.pc[p] = Idle]

-----------------------------------------------------------------------------
(* The steps, each with what it read for the properties.                   *)

Did(S2, act, p, t, tip, removed) ==
    s' = S2 /\ last' = [act |-> act, p |-> p, t |-> t, tip |-> tip, removed |-> removed]
Plain(S2, act, p, t) == Did(S2, act, p, t, TRUE, FALSE)

Take == TakeG(s) /\ Did(TakeF(s), "take", "daemon", 0, s.tipok, FALSE)
Decide == DecideG(s) /\ Plain(DecideF(s), "decide", "daemon", DecideF(s).pc.daemon.t)
Record == RecordG(s) /\ Plain(RecordF(s), "record", "daemon", s.pc.daemon.t)
Undo == RecordG(s) /\ Did(UndoF(s), "undo", "daemon", s.pc.daemon.t, s.tipok, UndoRm(s))
Edit(a) == EditG(s, a) /\ Plain(EditF(s, a), "edit", "worker", Tree(s, a))
Commit(a) == CommitG(s, a) /\ Plain(CommitF(s, a), "commit", "worker", Tree(s, a))
Fail(a) == FailG(s, a) /\ Plain(FailF(s, a), "fail", "worker", Tree(s, a))
Ends(a) == EndsG(s, a) /\ \E st \in {"ready", "human"} : Plain(EndsF(s, a, st), "ends", "cli", Tree(s, a))
Ready == ReadyG(s) /\ Plain(ReadyF(s), "ready", "cli", 0)
Land(a) == LandG(s, a) /\ Plain(LandF(s, a), "land", "land", Tree(s, a))
Release == ReleaseG(s) /\ Did(ReleaseF(s), "release", "land", s.pc.land.t, s.tipok, ReleaseRm(s))
Over == OverG(s) /\ Plain(OverF(s), "over", "over", LastTree(s))
Close == CloseG(s) /\ Plain(CloseF(s), "close", "close", Left(s))
ShutStep == ShutG(s) /\ Plain(ShutF(s), "shut", "close", s.pc.close.t)
Discard(t) == DiscardG(s, t) /\ Plain(DiscardF(s, t), "discard", "discard", t)
Abandon(p) ==
    \/ AbCommitG(s, p) /\ Plain(AbCommitF(s, p), "ab_commit", p, s.pc[p].t)
    \/ AbKeepG(s, p) /\ Plain(AbKeepF(s, p), "ab_keep", p, s.pc[p].t)
    \/ AbRemoveG(s, p) /\ Plain(AbRemoveF(s, p), "ab_remove", p, s.pc[p].t)

\* The proving tip becomes readable, or not (the remote, the network).
Tip == Plain([s EXCEPT !.tipok = ~@], "tip", "", 0)

\* A process dies where `Dies` allows, and comes back idle: the daemon
\* (its start and its landing alike), or a CLI command.
Die ==
    \/ /\ s.pc.daemon.at \in Dies \/ s.pc.land.at \in Dies
       /\ Plain([s EXCEPT !.pc.daemon = Idle, !.pc.land = Idle], "die", "daemon", 0)
    \/ \E p \in CLIs : s.pc[p].at \in Dies /\ Plain([s EXCEPT !.pc[p] = Idle], "die", p, 0)

Next ==
    \/ Take \/ Decide \/ Record \/ Undo \/ Release \/ Ready
    \/ Over \/ Close \/ ShutStep \/ Tip \/ Die
    \/ \E a \in Attempts : Edit(a) \/ Commit(a) \/ Fail(a) \/ Ends(a) \/ Land(a)
    \/ \E t \in Trees : Discard(t)
    \/ \E p \in CLIs : Abandon(p)

Spec == Init /\ [][Next]_vars

View == s
Shallow == TLCGet("level") < Depth

-----------------------------------------------------------------------------
(* A trace's steps: each record is one whole command, tick or worker's     *)
(* step, the code's steps of it in order with nothing between, from idle   *)
(* to idle (crates/factorium/tests/scenarios/fixture/kept_trace.rs).       *)

Whole(S2) == s' = S2 /\ last' = last
\* A record's list, as a set (truefit.toml's mapping).
Set(q) == {q[i] : i \in DOMAIN q}
Abandoned(S, p) == AbRemoveF(AbKeepF(AbCommitF(S, p), p), p)
TraceNext ==
    \/ TakeG(s) /\ DecideG(TakeF(s)) /\ RecordG(DecideF(TakeF(s)))
       /\ \/ Whole(RecordF(DecideF(TakeF(s))))
          \/ Whole(UndoF(DecideF(TakeF(s))))
    \/ \E a \in Attempts :
        \/ EditG(s, a) /\ Whole(EditF(s, a))
        \/ CommitG(s, a) /\ Whole(CommitF(s, a))
        \/ FailG(s, a) /\ Whole(FailF(s, a))
        \/ EndsG(s, a) /\ \E st \in {"ready", "human"} : Whole(EndsF(s, a, st))
        \/ LandG(s, a) /\ Whole(ReleaseF(LandF(s, a)))
    \/ ReadyG(s) /\ Whole(ReadyF(s))
    \/ OverG(s) /\ Whole(Abandoned(OverF(s), "over"))
    \/ CloseG(s) /\ LET S1 == CloseF(s) IN
        IF S1.pc.close.at = "idle" THEN Whole(S1) ELSE Whole(Abandoned(ShutF(S1), "close"))
    \/ \E t \in Trees : DiscardG(s, t) /\ Whole(Abandoned(DiscardF(s, t), "discard"))
    \/ Whole([s EXCEPT !.tipok = ~@])

-----------------------------------------------------------------------------
(* The properties, in SPEC §2's words.                                     *)

\* Work is never lost: what a worker made, and that is nowhere else, is in
\* a worktree on disk, on a branch, or on a kept `-abandoned` branch.
WorkNeverLost ==
    s.made \subseteq s.proving
                     \cup UNION {s.dirty[t] : t \in {u \in Trees : s.disk[u]}}
                     \cup UNION {s.br[t] : t \in {u \in Trees : s.hasbr[u]}}
                     \cup UNION {s.kept[t] : t \in Trees}

\* An open issue owns one worktree, its last started attempt's (attempt
\* 1's when a later one resumed it), whatever attempt its branch names:
\* doctor never names that one, and a worker at work works in it.
OneOwner ==
    s.status # "closed" /\ LastTree(s) # 0 =>
        /\ ~Orphaned(s, LastTree(s))
        /\ \A a \in Attempts : s.live[a] => Tree(s, a) = LastTree(s)

\* When the proving tip cannot be read, what a worktree holds is unknown,
\* so it is held: no worktree goes on the test that it holds nothing (a
\* worker that ends, a start undone), and a start takes nothing. (An
\* abandon removes one with its branch kept, whatever the tip.)
HeldWhenUnknown ==
    [][ ~last'.tip => last'.act # "take" /\ ~last'.removed ]_vars

\* Start-over only while no worker is on the issue; neither it nor discard
\* touches a worktree a worker is on (at work, or starting in it).
Busy(S, t) == \E a \in Attempts :
    \/ (S.taken = a \/ S.live[a]) /\ S.started[a] = t
    \/ S.taken = a /\ S.pc.daemon.a = a /\ S.pc.daemon.t = t
NoStartOverUnderWorker ==
    [][ last'.act # "die" =>
        /\ last'.p = "over" => s.taken = 0
        /\ last'.p \in {"over", "discard"} /\ last'.t # 0 => ~Busy(s, last'.t) ]_vars

\* Every worktree no open issue owns is a doctor finding once the tick is
\* over: between ticks, with no start of the daemon's under way.
DoctorFinds ==
    s.pc.daemon.at = "idle" =>
        \A t \in Trees : s.disk[t] /\ ~Owner(s, t) => Orphaned(s, t)

\* The broken variants, each of the code as it could break, and the
\* property it breaks (`models/worktrees.sh broken`).
BrokenVariants == [
    close_no_keep     |-> "WorkNeverLost",          \* close removes the worktree, its branch not kept (#124)
    abandon_no_commit |-> "WorkNeverLost",          \* abandon keeps the branch, its changes not committed
    ignore_dirty      |-> "WorkNeverLost",          \* holds_own reads no changes not committed
    orphan_by_attempt |-> "OneOwner",               \* owned by the attempt its branch names (#130)
    release_unknown   |-> "HeldWhenUnknown",        \* an unread tip counts as nothing committed
    start_unknown     |-> "HeldWhenUnknown",        \* a start takes the issue with the tip unread
    start_over_live   |-> "NoStartOverUnderWorker", \* start-over not refused under a worker
    closed_unfound    |-> "DoctorFinds"             \* doctor passes over a closed issue's worktree
]
=============================================================================
