----------------------------- MODULE HeavyQueue -----------------------------
(***************************************************************************)
(* The host-wide heavy queue (SPEC.md §1 row 9), as factorium runs it.     *)
(*                                                                         *)
(* The queue is the `heavy` stream (crates/domain/src/heavy.rs): entries,  *)
(* the last start, and the newest rules that asked. Each command on it     *)
(* (heavy.queue, heavy.start, heavy.end, heavy.runs, heavy.rules) is one   *)
(* indivisible step, as the store runs one command at a time; its start    *)
(* rule is crates/domain/src/neutral/queue.rs. The processes that ask it   *)
(* (crates/factorium/src/heavy.rs) are the waiters: a CLI run that waits   *)
(* for its turn (`worker exclusive`, `heavy run`, a gate run by hand), or  *)
(* idle work that asks once and runs beside the ticks. Each waiter's own   *)
(* steps (looking at its running entry, answering) are steps of their own, *)
(* between which any other process may act. The owner's `unqueue` takes   *)
(* any entry off. A worker's or a program's entry that leaves unstarted   *)
(* (its waiter refused and failing to exec, or gone) keeps its place for   *)
(* the keep bound: its asker asking again takes it (factorium#193).        *)
(*                                                                         *)
(* Time: `now` is the time of the last step, in the config's units. A     *)
(* step happens at any time from `now` to `now + MaxStep`, up to Horizon.  *)
(*                                                                         *)
(* `Bug` names a broken variant (BrokenVariants below); "none" is the code *)
(* as it is. `./models/heavy.sh broken` shows each property failing on    *)
(* its variant.                                                            *)
(***************************************************************************)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Ids,        \* the entries' ids: their order breaks a tie of queued_at
    MaxRules,   \* rules are 1..MaxRules, oldest first; 0 is none yet
    Settle,     \* heavy_settle
    Hold,       \* hold_bound
    Horizon,    \* time runs 0..Horizon
    MaxStep,    \* how far one step may move time
    ReadLag,    \* how long before its ask a reading may be taken
    BoundLag,   \* how long after its arrival a line move falls due (0: none has a bound)
    Kinds,      \* the waiters asking: "wait" (a CLI run), "beside" (idle work)
    Unqueues,   \* whether the owner unqueues
    Keep,       \* heavy_keep (0: no waiter gives up nor asks again)
    Bug         \* "none", or a broken variant

None == -1
Max(a, b) == IF a >= b THEN a ELSE b

\* An entry of the queue, as heavy::Entry holds it (what the rules read):
\* `yields` is a worker's or a program's (worker.is_some() || program);
\* `noted` is whether its session is noted (session.is_some()).
Absent == [in |-> FALSE, queued_at |-> None, started_at |-> None,
           yields |-> FALSE, bound_at |-> None, noted |-> FALSE]

VARIABLES
    q,           \* q[i]: entry i (Absent when the queue holds none)
    last_start,  \* Queue::last_start (None: none started yet, the code's 0)
    rules,       \* Queue::rules: the newest rules that asked
    now,         \* the time of the last step
    waiter,      \* waiter[i]: the process that asks for entry i
    last         \* the last step and what it read: for the properties

vars == <<q, last_start, rules, now, waiter, last>>

Phases == {"none", "waits", "runs", "done"}
\* `left_at`, `place`: when its entry left unstarted with its place kept
\* (heavy::Left), and that place (its queued_at); None: none kept.
NoWaiter == [kind |-> "wait", rules |-> 0, yields |-> FALSE, bound |-> None,
             phase |-> "none", stop |-> "", answer |-> "", unqueued |-> "",
             left_at |-> None, place |-> None]

TypeOK ==
    /\ \A i \in Ids : q[i].in \/ q[i] = Absent
    /\ last_start \in Int /\ rules \in 0..MaxRules /\ now \in 0..Horizon
    /\ \A i \in Ids : waiter[i].kind \in Kinds \cup {"wait"} /\ waiter[i].phase \in Phases
                      /\ waiter[i].rules \in 0..MaxRules

Init ==
    /\ q = [i \in Ids |-> Absent]
    /\ last_start = None
    /\ rules = 0
    /\ now = 0
    /\ waiter = [i \in Ids |-> NoWaiter]
    /\ last = [act |-> "init", i |-> 0, t |-> 0, room |-> FALSE, read |-> 0]

-----------------------------------------------------------------------------
(* The queue as neutral/queue.rs reads it.                                 *)

Present == {i \in Ids : q[i].in}
Waiting == {i \in Present : q[i].started_at = None}
Running == {i \in Present : q[i].started_at # None}

\* Queued before: by (queued_at, id), as `oldest` orders them.
Older(a, b) == \/ q[a].queued_at < q[b].queued_at
               \/ q[a].queued_at = q[b].queued_at /\ a < b
OldestOf(S) == CHOOSE a \in S : \A b \in S \ {a} : Older(a, b)

\* `overdue`: a line move past its bound at t.
Overdue(i, t) == q[i].bound_at # None /\ t > q[i].bound_at

\* `next`: the oldest waiting, but one past its bound goes ahead of every
\* entry that yields, just before the oldest of them.
Yielding == {i \in Waiting : q[i].yields}
PlaceOf(i, t) ==
    IF Overdue(i, t) /\ Yielding # {} /\ Older(OldestOf(Yielding), i)
    THEN OldestOf(Yielding) ELSE i
KeyLess(i, j, t) ==
    LET p == PlaceOf(i, t)  r == PlaceOf(j, t) IN
    \/ p # r /\ Older(p, r)
    \/ p = r /\ Overdue(i, t) /\ ~Overdue(j, t)
    \/ p = r /\ Overdue(i, t) = Overdue(j, t) /\ Older(i, j)
NextOf(t) ==
    IF Bug = "no_line_ahead" THEN OldestOf(Waiting)
    ELSE IF Bug = "by_id" THEN CHOOSE i \in Waiting : \A j \in Waiting : i <= j
    ELSE CHOOSE i \in Waiting : \A j \in Waiting \ {i} : KeyLess(i, j, t)

\* `may_start`, after heavy.start's rules check: "start", or "wait".
MayStart(i, room, read, t) ==
    IF i # NextOf(t) THEN "wait"
    ELSE IF Bug = "room_first" /\ ~room THEN "wait"
    ELSE IF Running = {} THEN "start"
    ELSE IF ~room THEN "wait"
    ELSE IF Bug = "no_settle" THEN "start"
    ELSE IF Bug = "settle_at_ask" /\ t - last_start >= Settle THEN "start"
    ELSE IF last_start # None /\ (read <= last_start \/ read - last_start < Settle) THEN "wait"
    ELSE "start"

\* heavy.start: a binary older than the newest that asked is refused.
Decide(i, room, read, t) ==
    IF waiter[i].rules < rules /\ Bug # "no_stale" THEN "stale"
    ELSE MayStart(i, room, read, t)

\* `overheld`: the running i holds others up past the hold bound at t.
WaitingSince == LET w == Waiting IN
                IF w = {} THEN None ELSE q[OldestOf(w)].queued_at
Overheld(i, t) ==
    /\ Waiting # {}
    /\ IF Bug = "held_from_queued" THEN t - WaitingSince > Hold
       ELSE t - Max(q[i].started_at, WaitingSince) > Hold

-----------------------------------------------------------------------------
(* The steps.                                                              *)

Times == now..(IF now + MaxStep > Horizon THEN Horizon ELSE now + MaxStep)
Waiter(i, f, v) == waiter' = [waiter EXCEPT ![i][f] = v]
Did(act, i, t) == last' = [act |-> act, i |-> i, t |-> t, room |-> FALSE, read |-> 0]
Off(i) == q' = [q EXCEPT ![i] = Absent]
\* A waiter that answered: only its answer, and whether it was unqueued, are kept.
Ended(w, answer) == [NoWaiter EXCEPT !.phase = "done", !.answer = answer, !.unqueued = w.unqueued]
\* A waiter whose waiting entry i leaves at t, its place kept for its asker
\* if a worker's or a program's (EndHeavy's `keeps`): it answered.
Left(i, answer, t) ==
    LET w == Ended(waiter[i], answer) IN
    IF q[i].yields THEN [w EXCEPT !.left_at = t, !.place = q[i].queued_at] ELSE w

Entry(t, yields, bound) == [Absent EXCEPT !.in = TRUE, !.queued_at = t,
                                !.yields = yields, !.bound_at = bound]

\* A process asks for a heavy command, and its first take_turn queues it
\* (heavy.queue): a waiter (kind "wait") or idle work ("beside"), under
\* rules r; a worker's or a program's entry yields; a line move's (a gate,
\* a cut) has a bound or none.
Arrive(i, kind, r, yields, bound) ==
    /\ waiter[i].phase = "none"
    /\ kind = "beside" => ~yields /\ bound = None
    /\ yields => bound = None
    /\ \E t \in Times :
        /\ now' = t
        /\ q' = [q EXCEPT ![i] = Entry(t, yields, bound)]
        /\ waiter' = [waiter EXCEPT ![i] = [NoWaiter EXCEPT
              !.kind = kind, !.rules = r, !.yields = yields, !.bound = bound,
              !.phase = "waits"]]
        /\ Did("queue", i, t)
    /\ UNCHANGED <<last_start, rules>>

\* heavy.rules: a binary asks under rules newer than any before: the daemon
\* as it starts, or a waiter (take_turn's note_rules) before it queues.
NoteRules ==
    /\ \E r \in 1..MaxRules, t \in Times :
        rules < r /\ rules' = r /\ now' = t /\ Did("rules", 0, t)
    /\ UNCHANGED <<q, last_start, waiter>>

\* take_turn, each poll after its first ask: a waiter whose waiting entry
\* was taken off (an unqueue) answers `unqueued`, and is not queued again.
\* Broken (`requeues`), it queues it again, at the back (factorium#186).
Unqueued(i) ==
    /\ waiter[i].phase = "waits" /\ ~q[i].in
    /\ \E t \in Times :
        /\ now' = t
        /\ IF Bug = "requeues"
           THEN /\ q' = [q EXCEPT ![i] = Entry(t, waiter[i].yields, waiter[i].bound)]
                /\ Did("queue", i, t)
                /\ UNCHANGED waiter
           ELSE /\ waiter' = [waiter EXCEPT ![i] = Ended(waiter[i], "unqueued")]
                /\ Did("unqueued", i, t)
                /\ UNCHANGED q
    /\ UNCHANGED <<last_start, rules>>


\* heavy.start (take_turn): i asks on a reading taken at `read`, which
\* found room or not. Started, it runs. Told to wait, a waiter asks again
\* next poll; idle work leaves the queue. Refused as stale, a waiter execs
\* the installed binary, whose rules are the newest, keeping its entry, or
\* fails to and leaves it, its place kept; idle work leaves it (the daemon
\* execs after).
Ask(i) ==
    /\ waiter[i].phase = "waits" /\ q[i].in
    /\ \E t \in Times, room \in BOOLEAN : \E read \in Max(0, t - ReadLag)..t :
        LET d == Decide(i, room, read, t) IN
        /\ now' = t
        /\ last' = [act |-> "ask", i |-> i, t |-> t, room |-> room, read |-> read]
        /\ \/ /\ d = "start"
              /\ q' = [q EXCEPT ![i].started_at = t]
              /\ last_start' = t
              /\ Waiter(i, "phase", "runs")
           \/ /\ d = "wait" /\ waiter[i].kind = "wait"
              /\ UNCHANGED <<q, last_start, waiter>>
           \/ /\ d \in {"wait", "stale"} /\ waiter[i].kind = "beside"
              /\ Off(i) /\ UNCHANGED last_start
              /\ waiter' = [waiter EXCEPT ![i] = Ended(waiter[i], d)]
           \/ /\ d = "stale" /\ waiter[i].kind = "wait"
              /\ UNCHANGED last_start
              /\ \/ /\ IF Bug = "stale_requeues"
                       THEN q' = [q EXCEPT ![i].queued_at = t] ELSE UNCHANGED q
                    /\ Waiter(i, "rules", MaxRules)
                 \/ /\ Off(i)
                    /\ waiter' = [waiter EXCEPT ![i] = Left(i, "stale", t)]
    /\ UNCHANGED rules

\* A waiter gives up (killed, as a harness ends a command past its timeout):
\* another's take_turn forgets its waiting entry, its place kept.
GivesUp(i) ==
    /\ Keep > 0
    /\ waiter[i].phase = "waits" /\ waiter[i].kind = "wait"
    /\ q[i].in /\ q[i].started_at = None
    /\ \E t \in Times :
        /\ now' = t /\ Did("gaveup", i, t)
        /\ waiter' = [waiter EXCEPT ![i] = Left(i, "gaveup", t)]
    /\ Off(i)
    /\ UNCHANGED <<last_start, rules>>

\* Its asker asks for the same command again, as a new waiter under the
\* newest rules, within the keep bound of its entry leaving: queued
\* (heavy.queue, Queue::place) in the place that entry had. Broken
\* (`rejoins_at_back`), at the back.
Rejoin(i) ==
    /\ waiter[i].phase = "done" /\ waiter[i].left_at # None
    /\ \E t \in Times :
        /\ t - waiter[i].left_at <= Keep
        /\ now' = t /\ Did("rejoin", i, t)
        /\ q' = [q EXCEPT ![i] = Entry(IF Bug = "rejoins_at_back" THEN t ELSE waiter[i].place, TRUE, None)]
        /\ waiter' = [waiter EXCEPT ![i] = [NoWaiter EXCEPT
              !.rules = MaxRules, !.yields = TRUE, !.phase = "waits", !.place = waiter[i].place]]
    /\ UNCHANGED <<last_start, rules>>

\* heavy.runs: the running i's step runs in a session, noted on its entry.
NoteRun(i) ==
    /\ waiter[i].phase = "runs" /\ q[i].in /\ ~q[i].noted
    /\ \E t \in Times : now' = t /\ Did("runs", i, t)
    /\ q' = [q EXCEPT ![i].noted = TRUE]
    /\ UNCHANGED <<last_start, rules, waiter>>

\* `look_at`, each poll: the running i must end once unqueued, once it holds
\* others up past the hold bound, or (idle work) once anything else is
\* queued or the host is busy; its steps are then ended, whole.
Look(i) ==
    /\ waiter[i].phase = "runs" /\ waiter[i].stop = ""
    /\ \E t \in Times, busy \in BOOLEAN :
        LET why == IF ~q[i].in THEN "unqueued"
                   ELSE IF Overheld(i, t) /\ Bug # "no_held" THEN "held"
                   ELSE IF waiter[i].kind = "beside" /\ (busy \/ Present # {i}) THEN "gaveway"
                   ELSE IF Bug = "line_cuts_short" /\ \E j \in Waiting : Overdue(j, t) THEN "gaveway"
                   ELSE ""
        IN /\ now' = t /\ Did("look", i, t)
           /\ Waiter(i, "stop", why)
    /\ UNCHANGED <<q, last_start, rules>>

\* The running i's steps end (done, or ended), and its waiter answers how
\* they ran: `unqueued` if the queue no longer holds it (run_held; beside's
\* Ends::Unqueued), else held or ran; then it takes its entry off
\* (heavy.end). That an unqueue may land between the answer and the end
\* changes only which of the two takes it off.
Done(i) ==
    /\ waiter[i].phase = "runs"
    /\ \E t \in Times :
        LET answer == IF ~q[i].in /\ Bug # "answer_ran" THEN "unqueued"
                      ELSE IF waiter[i].stop \in {"held", "gaveway"} THEN waiter[i].stop
                      ELSE "ran"
        IN /\ now' = t /\ Did("done", i, t)
           /\ waiter' = [waiter EXCEPT ![i] = Ended(waiter[i], answer)]
    /\ IF q[i].in THEN Off(i) ELSE UNCHANGED q
    /\ UNCHANGED <<last_start, rules>>

\* `factorium unqueue`: heavy.end of any entry, waiting or running (its
\* session, if noted, is ended then too: its steps end, as Done).
Unqueue(i) ==
    /\ Unqueues /\ q[i].in
    /\ \E t \in Times : now' = t /\ Did("unqueue", i, t)
    /\ Off(i)
    /\ Waiter(i, "unqueued", IF q[i].started_at = None THEN "waiting" ELSE "running")
    /\ UNCHANGED <<last_start, rules>>

\* Any process, of any kind and rules, for any free id.
Arrives ==
    \E i \in Ids, kind \in Kinds, r \in 1..MaxRules, yields \in BOOLEAN,
       bound \in {None} \cup (now..(IF now + BoundLag - 1 > Horizon THEN Horizon ELSE now + BoundLag - 1)) :
        Arrive(i, kind, r, yields, bound)

\* Time passes, between the steps (the config's MaxStep 0: steps then take
\* none; a trace's steps each carry their own time, and take none of these).
Tick ==
    /\ now < Horizon
    /\ now' = now + 1 /\ Did("tick", 0, now + 1)
    /\ UNCHANGED <<q, last_start, rules, waiter>>

Next ==
    \/ Tick
    \/ Arrives
    \/ NoteRules
    \/ \E i \in Ids : Unqueued(i) \/ Ask(i) \/ NoteRun(i)
                      \/ Look(i) \/ Done(i) \/ Unqueue(i)
                      \/ GivesUp(i) \/ Rejoin(i)

Spec == Init /\ [][Next]_vars

\* What tells states apart: `last` only says which step reached one.
View == <<q, last_start, rules, now, waiter>>
\* A bound on the steps a model check explores (TLCGet("level")).
CONSTANT Depth
Shallow == TLCGet("level") < Depth

-----------------------------------------------------------------------------
(* The properties, in SPEC §1 row 9's words. Each is an action property:  *)
(* of every step, `last'` is that step and the unprimed state is before.   *)

Asked == last'.act = "ask"
Who == last'.i
At == last'.t
Started == Asked /\ q[Who].started_at = None /\ q'[Who].started_at # None

\* It never starves the oldest: with nothing running, the oldest waiting
\* entry starts at once when it asks under the newest rules, whatever the
\* reading (no line move past its bound waiting ahead of it).
AtOnce ==
    [][ (/\ Asked /\ Running = {} /\ Who = OldestOf(Waiting)
         /\ waiter[Who].rules >= rules
         /\ ~\E j \in Waiting : Overdue(j, At))
        => q'[Who].started_at # None ]_vars

\* Oldest first: an entry starts only when every entry queued before it
\* waits behind a worker's or a program's entry that a line move past its
\* bound (this one) goes ahead of.
InOrder ==
    [][ Started =>
        \A j \in Waiting \ {Who} :
            \/ Older(Who, j)
            \/ Overdue(Who, At) /\ \E y \in Waiting : q[y].yields /\ (y = j \/ Older(y, j)) ]_vars

\* A line move past its bound goes ahead of workers' and programs' waiting
\* entries: none of those starts while one waits.
LineAhead ==
    [][ Started /\ q[Who].yields => ~\E j \in Waiting : Overdue(j, At) ]_vars

\* It starts one beside what runs only when the host has room, on a reading
\* taken heavy_settle after the last start.
Settled ==
    [][ Started /\ Running # {} =>
        last'.room /\ last'.read > last_start /\ last'.read - last_start >= Settle ]_vars

\* A running entry is never cut short for a line move: a waiter's running
\* entry is ended only once unqueued, or once it holds an entry up past the
\* hold bound (counted from when both were there). Idle work gives way.
NotCutShort ==
    [][ \A i \in Ids :
        (waiter[i].kind = "wait" /\ waiter[i].stop = "" /\ waiter'[i].stop # "")
        => \/ waiter'[i].stop = "unqueued" /\ ~q[i].in
           \/ waiter'[i].stop = "held"
              /\ \E j \in Waiting : At - Max(q[i].started_at, q[j].queued_at) > Hold ]_vars

\* One that holds others up past the hold bound is ended: its waiter's
\* next look ends it.
HeldEnded ==
    [][ \A i \in Ids :
        (last'.act = "look" /\ last'.i = i /\ q[i].in
         /\ \E j \in Waiting : At - Max(q[i].started_at, q[j].queued_at) > Hold)
        => waiter'[i].stop # "" ]_vars

\* An unqueued run's waiter answers `unqueued`.
UnqueuedRunAnswers ==
    \A i \in Ids : waiter[i].unqueued = "running" =>
        waiter[i].answer \in {"", "unqueued"}

\* An unqueued entry's waiter answers `unqueued`, waiting or running: it is
\* not queued again, and never runs.
UnqueuedAnswers ==
    \A i \in Ids : waiter[i].unqueued # "" =>
        ~q[i].in /\ waiter[i].answer \in {"", "unqueued"}

\* A waiter of an older binary is refused, and keeps its place: its entry
\* does not start, and stays as it was while its waiter goes on.
StaleRefused ==
    [][ (Asked /\ waiter[Who].rules < rules) =>
        /\ q'[Who].started_at = None
        /\ waiter'[Who].phase = "waits" => q'[Who] = q[Who] ]_vars

\* An entry asked again keeps the place it had: rejoined, it is queued
\* before every waiting entry queued after its first ask.
KeptPlace ==
    [][ last'.act = "rejoin" =>
        \A j \in Waiting \ {Who} :
            q[j].queued_at > waiter[Who].place => q'[Who].queued_at < q[j].queued_at ]_vars

\* The broken variants, each of the code as it could break, and the
\* property it breaks (`./models/heavy.sh broken`).
BrokenVariants == [
    room_first       |-> "AtOnce",       \* the room read before "nothing runs"
    by_id            |-> "InOrder",      \* the first by id, not by when it was queued
    no_line_ahead    |-> "LineAhead",    \* `next` is just the oldest
    no_settle        |-> "Settled",      \* no settle interval beside what runs
    settle_at_ask    |-> "Settled",      \* settled at the ask, not the reading (#102)
    line_cuts_short  |-> "NotCutShort",  \* a line move past its bound ends what runs
    held_from_queued |-> "NotCutShort",  \* held counted from the waiter's queueing alone
    no_held          |-> "HeldEnded",    \* the hold bound never ends one
    answer_ran       |-> "UnqueuedRunAnswers", \* run_held does not look whether it is held
    requeues         |-> "UnqueuedAnswers",    \* take_turn queues an unqueued waiter again (#186)
    no_stale         |-> "StaleRefused", \* heavy.start takes any rules
    stale_requeues   |-> "StaleRefused", \* the exec'd waiter queues anew, at the back
    rejoins_at_back  |-> "KeptPlace"     \* asked again, it queues at the back (#193)
]
=============================================================================
