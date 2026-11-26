----------------------------- MODULE HeavyQueue -----------------------------
(***************************************************************************)
(* The host-wide heavy queue (ADR 0010, ADR 0011), as factorium runs it.   *)
(*                                                                         *)
(* The queue is the `heavy` stream (crates/domain/src/heavy.rs): entries,  *)
(* the last start, and the rules of the daemon that runs. Each command on  *)
(* it (heavy.queue, heavy.start, heavy.end, heavy.runs, heavy.rules) is    *)
(* one indivisible step, as the store runs one command at a time; its      *)
(* start rule is crates/domain/src/neutral/queue.rs. The processes that    *)
(* ask it (crates/factorium/src/heavy.rs) are the waiters: a CLI run that  *)
(* waits for its turn (`worker exclusive`, `heavy run`, a gate run by      *)
(* hand), or idle work that asks once and runs beside the ticks. Each      *)
(* waiter's own steps (looking at its running entry, answering) are steps  *)
(* of their own, between which any other process may act. The owner's      *)
(* `unqueue` takes any entry off. A worker's or a program's entry that     *)
(* leaves unstarted (its waiter refused and failing to exec, or gone)      *)
(* keeps its place for the keep bound: its asker asking again takes it     *)
(* (factorium#193).                                                        *)
(*                                                                         *)
(* It is the queue of resources (factorium#284, #286): each entry asks for *)
(* one or more (`ask`), granted all at once: "host" by room, as above; a   *)
(* named one by its `Slots`. One waiting for a named resource whose slots  *)
(* are all held, or outside the window its hold named, is passed over in   *)
(* every queue, as a stale one is. Each resource's line puts one waiting   *)
(* past the starve bound first, then the shortest declared hold (a command *)
(* declares none, read as `Default`), oldest first. A worker's hold, once  *)
(* granted, stays held after its waiter answers, until released, and its   *)
(* worker's own commands asking for what it holds take no slot of theirs.  *)
(*                                                                         *)
(* An owner's urgent mark (docs/adr/0015): an entry of an urgent issue's *)
(* worker, or of its landing's gate, is marked so as it asks, and asked    *)
(* again while it waits it takes the mark as it is then. In each line it   *)
(* goes ahead of workers' and programs' entries not marked, behind a line  *)
(* move past its bound. While the tick finds urgent issues that halt the   *)
(* queue, every worker's or program's entry not urgent (marked, or of one  *)
(* of their live workers) is passed over in every queue, and such a        *)
(* command holding what an urgent one waits for is ended; a line move's    *)
(* entry is never halted.                                                  *)
(* A landing's fast gate goes ahead of workers' and programs' entries not  *)
(* marked, behind those marked (docs/adr/0010).                           *)
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
    SettleIdle, \* heavy_settle_idle: on a reading well under the busy limit
    Hold,       \* hold_bound
    Horizon,    \* time runs 0..Horizon
    MaxStep,    \* how far one step may move time
    ReadLag,    \* how long before its ask a reading may be taken
    BoundLag,   \* how long after its arrival a line move falls due (0: none has a bound)
    Kinds,      \* the waiters asking: "wait" (a CLI run), "beside" (idle work)
    Unqueues,   \* whether the owner unqueues
    Keep,       \* heavy_keep (0: no waiter gives up nor asks again)
    Named,      \* the named resources the host's settings declare
    Slots,      \* each named one's slots
    Asks,       \* the sets of resources an entry may ask for ("host" among them)
    Workers,    \* the workers whose entries may be a hold or inside one
    Holds,      \* the holds a worker's hold may declare (`--for`)
    Opens,      \* when a hold's window may open (`--between`); it stays open
    Starve,     \* starve_bound
    Default,    \* hold_default: what a command, which declares none, is read to hold
    Urgent,     \* whether issues are marked urgent, and halt the queue, and a
                \* landing's gate is told from other line moves
    Bug         \* "none", or a broken variant

None == -1
Max(a, b) == IF a >= b THEN a ELSE b

\* An entry of the queue, as heavy::Entry holds it (what the rules read):
\* `yields` is a worker's or a program's (worker.is_some() || program);
\* `noted` is whether its session is noted (session.is_some()).
Absent == [in |-> FALSE, queued_at |-> None, started_at |-> None,
           yields |-> FALSE, bound_at |-> None, noted |-> FALSE]

\* What entry i asks for, as heavy::Entry holds it besides: its resources,
\* its declared hold (None: a command), its window's opening (None: any
\* time), its worker (NoWorker: none), its urgent mark, and whether it is a
\* landing's gate (`lands`).
NoWorker == ""
NoAsk == [res |-> {"host"}, hold |-> None, win |-> None, worker |-> NoWorker, urgent |-> FALSE,
          lands |-> FALSE]

VARIABLES
    q,           \* q[i]: entry i (Absent when the queue holds none)
    last_start,  \* Queue::last_start (None: none started yet, the code's 0)
    rules,       \* Queue::rules: the rules of the daemon that runs
    now,         \* the time of the last step
    waiter,      \* waiter[i]: the process that asks for entry i
    ask,         \* ask[i]: what entry i asks for
    halt,        \* Queue::halt is not empty: urgent issues halt the queue
    halt_for,    \* Queue::halt_for: their live workers
    last         \* the last step, what it read, and the last start before it
                 \* (`since`): for the properties

vars == <<q, last_start, rules, now, waiter, ask, halt, halt_for, last>>

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
    /\ halt \in BOOLEAN /\ halt_for \subseteq Workers

Init ==
    /\ q = [i \in Ids |-> Absent]
    /\ last_start = None
    /\ rules = 0
    /\ now = 0
    /\ waiter = [i \in Ids |-> NoWaiter]
    /\ ask = [i \in Ids |-> NoAsk]
    /\ halt = FALSE
    /\ halt_for = {}
    /\ last = [act |-> "init", i |-> 0, t |-> 0, room |-> FALSE, idle |-> FALSE, read |-> 0, since |-> None]

-----------------------------------------------------------------------------
(* The queue as heavy.rs's Queue and neutral/queue.rs read it.            *)

Present == {i \in Ids : q[i].in}
Waiting == {i \in Present : q[i].started_at = None}
Running == {i \in Present : q[i].started_at # None}
Res(i) == ask[i].res
IsHold(i) == ask[i].hold # None
\* Waiting under rules the queue's replaced: passed over, it holds none up
\* (Queue::stale; factorium#241). The entry's rules are its waiter's, which
\* the code notes on it as the waiter asks.
Stale == {i \in Waiting : waiter[i].rules < rules}

\* `covered`: a worker's command asking for r inside its worker's running
\* hold of r: the hold holds it, so it takes no slot of its own.
Covered(i, r) ==
    /\ ~IsHold(i) /\ ask[i].worker # NoWorker
    /\ \E h \in Running : IsHold(h) /\ ask[h].worker = ask[i].worker /\ r \in Res(h)
\* `holders`: the running entries holding r, but those inside a hold of it.
Holders(r) == {j \in Running : r \in Res(j) /\ ~Covered(j, r)}
\* `full_for`: the waiting i asks for a named resource whose slots are all held.
Full(i) == \E r \in Res(i) \ {"host"} : ~Covered(i, r) /\ Cardinality(Holders(r)) >= Slots
\* `outside`: the waiting i's window is not open yet.
Opened(i, t) == ask[i].win # None /\ t >= ask[i].win
Outside(i, t) == ask[i].win # None /\ ~Opened(i, t)
\* `Queue::urgent`: marked so, or of a live worker of an issue that halts it.
UrgentOf(i) == ask[i].urgent \/ ask[i].worker \in halt_for
\* `halted`: a worker's or a program's entry not urgent while urgent issues
\* halt the queue: passed over. A line move's never is.
Halted(i) == halt /\ q[i].yields /\ ~UrgentOf(i)

\* What the code reads, a broken variant's way: `covered`, `full_for`,
\* `outside` and `stale`, and whether the waiting i is passed over in every
\* queue at t (`passed_over`). Broken (`stale_holds`, and `no_stale`, which
\* takes any rules), a stale one holds its place at the head; (`full_holds`)
\* one waiting for a full resource holds its place in its other queues;
\* (`no_slots`) none is ever full; (`no_window`) a window is never waited for;
\* (`no_cover`) a worker's command waits for a slot beside its own hold;
\* (`no_halt`) a halt passes none over.
CoveredC(i, r) == Bug # "no_cover" /\ Covered(i, r)
HoldersC(r) == {j \in Running : r \in Res(j) /\ ~CoveredC(j, r)}
FullC(i) ==
    /\ Bug # "no_slots"
    /\ \E r \in Res(i) \ {"host"} : ~CoveredC(i, r) /\ Cardinality(HoldersC(r)) >= Slots
PassedOver(i, t) ==
    \/ i \in Stale /\ Bug \notin {"stale_holds", "no_stale"}
    \/ FullC(i) /\ Bug # "full_holds"
    \/ Outside(i, t) /\ Bug # "no_window"
    \/ Halted(i) /\ Bug # "no_halt"

\* `queue_of`: resource r's queue at t: the entries asking for it but
\* those passed over, and commands inside their worker's hold of it.
QueueOf(r, t) == {j \in Present : r \in Res(j) /\ ~CoveredC(j, r)
                                  /\ (q[j].started_at # None \/ ~PassedOver(j, t))}
LineOf(r, t) == {j \in QueueOf(r, t) : q[j].started_at = None}
RunningIn(r, t) == QueueOf(r, t) \ LineOf(r, t)

\* Queued before: by (queued_at, id), as `oldest` orders them.
Older(a, b) == \/ q[a].queued_at < q[b].queued_at
               \/ q[a].queued_at = q[b].queued_at /\ a < b
OldestOf(S) == CHOOSE a \in S : \A b \in S \ {a} : Older(a, b)

\* `overdue`: a line move past its bound at t.
Overdue(i, t) == q[i].bound_at # None /\ t > q[i].bound_at

\* `Order`: `joined`, when it joined the line (its window's opening, if
\* later than its queueing); `starved`, waited past the starve bound;
\* `holds`, what it declared, or the default; `key`, where it stands by its
\* own: the starved first, oldest first; then the shortest hold, oldest
\* first. Broken (`no_starve`) none is starved; (`by_age`) holds are not read.
Joined(i, t) == IF Opened(i, t) THEN Max(q[i].queued_at, ask[i].win) ELSE q[i].queued_at
Starved(i, t) == Bug # "no_starve" /\ t - Joined(i, t) > Starve
HoldsOf(i) == IF ask[i].hold = None \/ Bug = "by_age" THEN Default ELSE ask[i].hold
Key(i, t) == IF Starved(i, t) THEN <<0, 0, q[i].queued_at, i>>
             ELSE <<1, HoldsOf(i), q[i].queued_at, i>>
KeyLt(a, b) == \/ a[1] < b[1]
               \/ a[1] = b[1] /\ a[2] < b[2]
               \/ a[1] = b[1] /\ a[2] = b[2] /\ a[3] < b[3]
               \/ a[1] = b[1] /\ a[2] = b[2] /\ a[3] = b[3] /\ a[4] < b[4]
FirstKey(S, t) == Key(CHOOSE a \in S : \A b \in S \ {a} : KeyLt(Key(a, t), Key(b, t)), t)

\* `line`, `next`: by key, but one past its bound goes ahead of every entry
\* that gives way (`gives`: one that yields, or a landing's gate), just
\* before the first of them; one marked urgent ahead of every entry that
\* gives way and is not marked, just before the first of them, behind those
\* past their bound; and a landing's gate there too, behind the marked
\* ones. Broken (`no_urgent`), the mark is not read; (`urgent_first`) a
\* marked one goes ahead of those past their bound; (`no_lands`) a
\* landing's gate is not told from another line move.
MarkedC(i) == ask[i].urgent /\ Bug # "no_urgent"
LandsC(i) == ask[i].lands /\ Bug # "no_lands"
Gives(i) == q[i].yields \/ LandsC(i)
\* Its rank: past its bound 0, marked 1, a landing's gate 2, else 3.
Rank(i, t) == IF Overdue(i, t) THEN IF MarkedC(i) /\ Bug = "urgent_first" THEN 1 ELSE 0
              ELSE IF MarkedC(i) THEN IF Bug = "urgent_first" THEN 0 ELSE 1
              ELSE IF LandsC(i) THEN 2
              ELSE 3
PlaceOf(i, L, t) ==
    LET y == IF Rank(i, t) = 0 THEN {j \in L : Gives(j)}
             ELSE IF Rank(i, t) \in {1, 2} THEN {j \in L : Gives(j) /\ ~MarkedC(j)}
             ELSE {} IN
    IF y # {} /\ KeyLt(FirstKey(y, t), Key(i, t)) THEN FirstKey(y, t) ELSE Key(i, t)
KeyLess(i, j, L, t) ==
    LET p == PlaceOf(i, L, t)  r == PlaceOf(j, L, t) IN
    \/ p # r /\ KeyLt(p, r)
    \/ p = r /\ Rank(i, t) < Rank(j, t)
    \/ p = r /\ Rank(i, t) = Rank(j, t) /\ KeyLt(Key(i, t), Key(j, t))
NextIn(r, t) ==
    LET L == LineOf(r, t) IN
    IF Bug = "no_line_ahead" THEN OldestOf(L)
    ELSE IF Bug = "by_id" THEN CHOOSE i \in L : \A j \in L : i <= j
    ELSE CHOOSE i \in L : \A j \in L \ {i} : KeyLess(i, j, L, t)

\* neutral `may_start` on "host"'s queue, after heavy.start's rules check:
\* the settle is SettleIdle on a reading well under the busy limit (`idle`).
SettleOf(idle) == IF idle \/ Bug = "idle_near" THEN SettleIdle ELSE Settle
HostMay(i, room, idle, read, t) ==
    IF i # NextIn("host", t) THEN FALSE
    ELSE IF Bug = "room_first" /\ ~room THEN FALSE
    ELSE IF RunningIn("host", t) = {} THEN TRUE
    ELSE IF ~room THEN FALSE
    ELSE IF Bug = "no_settle" THEN TRUE
    ELSE IF Bug = "settle_at_ask" /\ t - last_start >= SettleOf(idle) THEN TRUE
    ELSE IF last_start # None /\ (read <= last_start \/ read - last_start < SettleOf(idle)) THEN FALSE
    ELSE TRUE

\* Queue::may_start: "start", or "wait": not passed over, and next in each
\* of its resources' queues it is not covered in, all at once, by each
\* one's rule: "host"'s room; a named one's slots (a full one is passed over).
MayStart(i, room, idle, read, t) ==
    IF PassedOver(i, t) \/ (Bug = "full_holds" /\ FullC(i)) THEN "wait"
    ELSE IF \A r \in Res(i) : CoveredC(i, r)
                \/ (r = "host" /\ HostMay(i, room, idle, read, t))
                \/ (r # "host" /\ NextIn(r, t) = i)
    THEN "start" ELSE "wait"

\* heavy.start: a binary older than the daemon's is refused.
Decide(i, room, idle, read, t) ==
    IF waiter[i].rules < rules /\ Bug # "no_stale" THEN "stale"
    ELSE MayStart(i, room, idle, read, t)

\* `overheld`: the running i, holding "host", holds others in its queue up
\* past the hold bound at t. A named resource is never cut short.
WaitingSince(t) == LET w == LineOf("host", t) IN
                   IF w = {} THEN None ELSE q[OldestOf(w)].queued_at
Overheld(i, t) ==
    /\ "host" \in Res(i)
    /\ LineOf("host", t) # {}
    /\ IF Bug = "held_from_queued" THEN t - WaitingSince(t) > Hold
       ELSE t - Max(q[i].started_at, WaitingSince(t)) > Hold

-----------------------------------------------------------------------------
(* The steps.                                                              *)

Times == now..(IF now + MaxStep > Horizon THEN Horizon ELSE now + MaxStep)
Waiter(i, f, v) == waiter' = [waiter EXCEPT ![i][f] = v]
Did(act, i, t) == last' = [act |-> act, i |-> i, t |-> t, room |-> FALSE, idle |-> FALSE, read |-> 0, since |-> last_start]
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
\* a cut) has a bound or none. It asks for `a`: its resources, all at once
\* (idle work and a line move ask for "host"); a worker's hold (`worker
\* hold`) declares how long it holds, asks for named ones alone, and may
\* name its window. A line move may be a landing's gate (`lands`). A
\* worker's entry (one that yields) or a landing's gate is marked urgent
\* if its issue is.
Arrive(i, kind, r, yields, bound, a) ==
    /\ waiter[i].phase = "none"
    /\ kind = "beside" => ~yields /\ bound = None
    /\ yields => bound = None
    /\ a.worker # NoWorker => yields /\ kind = "wait"
    /\ (kind = "beside" \/ bound # None) => a.res = {"host"}
    /\ a.hold # None => a.worker # NoWorker /\ "host" \notin a.res
    /\ a.win # None => a.hold # None
    /\ a.lands => Urgent /\ kind = "wait" /\ ~yields
    /\ a.urgent => Urgent /\ (yields \/ a.lands)
    /\ \E t \in Times :
        /\ now' = t
        /\ q' = [q EXCEPT ![i] = Entry(t, yields, bound)]
        /\ ask' = [ask EXCEPT ![i] = a]
        /\ waiter' = [waiter EXCEPT ![i] = [NoWaiter EXCEPT
              !.kind = kind, !.rules = r, !.yields = yields, !.bound = bound,
              !.phase = "waits"]]
        /\ Did("queue", i, t)
    /\ UNCHANGED <<last_start, rules, halt, halt_for>>

\* heavy.rules: a daemon starts under other rules than the queue's: a newer
\* binary swapped to, or an older one a rollback went back to. Only the
\* daemon: a newer binary that asks, whose proof may have failed, does not
\* (factorium#241).
NoteRules ==
    /\ \E r \in 1..MaxRules, t \in Times :
        rules # r /\ rules' = r /\ now' = t /\ Did("rules", 0, t)
    /\ UNCHANGED <<q, last_start, waiter, ask, halt, halt_for>>

\* take_turn, each poll, asks again (heavy.queue) with its issue's mark as
\* it is now: its waiting entry, queued under its rules, takes it
\* (HeavyMarked). Its issue's mark changed since.
Marks(i) ==
    /\ Urgent
    /\ waiter[i].phase = "waits" /\ q[i].in /\ q[i].started_at = None
    /\ q[i].yields \/ ask[i].lands
    /\ \E t \in Times :
        /\ now' = t /\ Did("mark", i, t)
        /\ ask' = [ask EXCEPT ![i].urgent = ~ask[i].urgent]
    /\ UNCHANGED <<q, last_start, rules, waiter, halt, halt_for>>

\* heavy.halt: the tick notes which urgent issues halt the queue (none:
\* it runs as usual), and their live workers.
NoteHalt ==
    /\ Urgent
    /\ \E h \in BOOLEAN, w \in SUBSET Workers, t \in Times :
        /\ w # {} => h
        /\ <<h, w>> # <<halt, halt_for>>
        /\ halt' = h /\ halt_for' = w /\ now' = t /\ Did("halt", 0, t)
    /\ UNCHANGED <<q, last_start, rules, waiter, ask>>

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
    /\ UNCHANGED <<last_start, rules, ask, halt, halt_for>>


\* What a reading finds: room or not, and well under the busy limit or not;
\* one well under it without room (memory, disk) is told as one without;
\* under a config whose two settles are one, being well under tells nothing.
Readings == {<<FALSE, FALSE>>, <<TRUE, FALSE>>} \cup (IF SettleIdle # Settle THEN {<<TRUE, TRUE>>} ELSE {})

\* heavy.start (take_turn): i asks on a reading taken at `read`, which
\* found room or not, and was well under the busy limit (`idle`) or not.
\* Started, it runs. Told to wait, a waiter asks again next poll; idle
\* work leaves the queue. Refused as stale, a waiter execs
\* the installed binary, whose rules are the newest, keeping its entry, or
\* fails to and leaves it, its place kept; idle work leaves it (the daemon
\* execs after). A worker's hold, granted, runs nothing: its waiter answers
\* with it, and it stays held (App::heavy). A refusal is a step of its own
\* (`stale`), as heavy.start answers it: what an ask leaves tells the two
\* apart.
Ask(i) ==
    /\ waiter[i].phase = "waits" /\ q[i].in
    /\ \E t \in Times, rd \in Readings : \E read \in Max(0, t - ReadLag)..t :
        LET room == rd[1]
            idle == rd[2]
            d == Decide(i, room, idle, read, t) IN
        /\ now' = t
        /\ last' = [act |-> IF d = "stale" THEN "stale" ELSE "ask",
                    i |-> i, t |-> t, room |-> room, idle |-> idle, read |-> read, since |-> last_start]
        /\ \/ /\ d = "start"
              /\ q' = [q EXCEPT ![i].started_at = t]
              /\ last_start' = IF "host" \in Res(i) THEN t ELSE last_start
              /\ IF IsHold(i) THEN waiter' = [waiter EXCEPT ![i] = Ended(waiter[i], "held")]
                 ELSE Waiter(i, "phase", "runs")
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
    /\ UNCHANGED <<rules, ask, halt, halt_for>>

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
    /\ UNCHANGED <<last_start, rules, ask, halt, halt_for>>

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
    /\ UNCHANGED <<last_start, rules, ask, halt, halt_for>>

\* heavy.runs: the running i's step runs in a session, noted on its entry.
NoteRun(i) ==
    /\ waiter[i].phase = "runs" /\ q[i].in /\ ~q[i].noted
    /\ \E t \in Times : now' = t /\ Did("runs", i, t)
    /\ q' = [q EXCEPT ![i].noted = TRUE]
    /\ UNCHANGED <<last_start, rules, waiter, ask, halt, halt_for>>

\* `look_at`, each poll: the running i must end once unqueued, once it is a
\* command not urgent holding what an urgent waiting entry asks for while
\* urgent issues halt the queue, once it holds others up past the hold
\* bound, or (idle work) once anything else asking for "host" is queued or
\* the host is busy; its steps are then ended, whole.
Wanted(i) == halt /\ \E u \in Waiting : UrgentOf(u) /\ Res(u) \cap Res(i) # {}
Look(i) ==
    /\ waiter[i].phase = "runs" /\ waiter[i].stop = ""
    /\ \E t \in Times, busy \in BOOLEAN :
        LET why == IF ~q[i].in THEN "unqueued"
                   ELSE IF Halted(i) /\ ~IsHold(i) /\ Wanted(i) THEN "halted"
                   ELSE IF Overheld(i, t) /\ Bug # "no_held" THEN "held"
                   ELSE IF waiter[i].kind = "beside" /\ (busy \/ \E j \in Present \ {i} : "host" \in Res(j)) THEN "gaveway"
                   ELSE IF Bug = "line_cuts_short" /\ \E j \in Waiting : Overdue(j, t) THEN "gaveway"
                   ELSE ""
        IN /\ now' = t /\ Did("look", i, t)
           /\ Waiter(i, "stop", why)
    /\ UNCHANGED <<q, last_start, rules, ask, halt, halt_for>>

\* The running i's steps end (done, or ended), and its waiter answers how
\* they ran: `unqueued` if the queue no longer holds it (run_held; beside's
\* Ends::Unqueued), else held or ran; then it takes its entry off
\* (heavy.end). That an unqueue may land between the answer and the end
\* changes only which of the two takes it off.
Done(i) ==
    /\ waiter[i].phase = "runs"
    /\ \E t \in Times :
        LET answer == IF ~q[i].in /\ Bug # "answer_ran" THEN "unqueued"
                      ELSE IF waiter[i].stop \in {"held", "gaveway", "halted"} THEN waiter[i].stop
                      ELSE "ran"
        IN /\ now' = t /\ Did("done", i, t)
           /\ waiter' = [waiter EXCEPT ![i] = Ended(waiter[i], answer)]
    /\ IF q[i].in THEN Off(i) ELSE UNCHANGED q
    /\ UNCHANGED <<last_start, rules, ask, halt, halt_for>>

\* `factorium unqueue`: heavy.end of any entry, waiting or running (its
\* session, if noted, is ended then too: its steps end, as Done). A granted
\* hold's waiter answered long since.
Unqueue(i) ==
    /\ Unqueues /\ q[i].in
    /\ \E t \in Times : now' = t /\ Did("unqueue", i, t)
    /\ Off(i)
    /\ IF waiter[i].phase = "done" THEN UNCHANGED waiter
       ELSE Waiter(i, "unqueued", IF q[i].started_at = None THEN "waiting" ELSE "running")
    /\ UNCHANGED <<last_start, rules, ask, halt, halt_for>>

\* A granted hold is given back (heavy.end): `worker release`, or its
\* worker ended (`give_back`).
Release(i) ==
    /\ q[i].in /\ q[i].started_at # None /\ IsHold(i)
    /\ \E t \in Times : now' = t /\ Did("release", i, t)
    /\ Off(i)
    /\ UNCHANGED <<last_start, rules, waiter, ask, halt, halt_for>>

\* What an entry may ask for; marked urgent, or a landing's gate, only under Urgent.
Flags == IF Urgent THEN BOOLEAN ELSE {FALSE}
AskOf == [res : Asks, hold : {None} \cup Holds, win : {None} \cup Opens,
          worker : {NoWorker} \cup Workers, urgent : Flags, lands : Flags]

\* Any process, of any kind and rules, asking for anything, for any free id.
Arrives ==
    \E i \in Ids, kind \in Kinds, r \in 1..MaxRules, yields \in BOOLEAN,
       bound \in {None} \cup (now..(IF now + BoundLag - 1 > Horizon THEN Horizon ELSE now + BoundLag - 1)),
       a \in AskOf :
        Arrive(i, kind, r, yields, bound, a)

\* Time passes, between the steps (the config's MaxStep 0: steps then take
\* none; a trace's steps each carry their own time, and take none of these).
Tick ==
    /\ now < Horizon
    /\ now' = now + 1 /\ Did("tick", 0, now + 1)
    /\ UNCHANGED <<q, last_start, rules, waiter, ask, halt, halt_for>>

Next ==
    \/ Tick
    \/ Arrives
    \/ NoteRules
    \/ NoteHalt
    \/ \E i \in Ids : Unqueued(i) \/ Ask(i) \/ NoteRun(i)
                      \/ Look(i) \/ Done(i) \/ Unqueue(i) \/ Marks(i)
                      \/ GivesUp(i) \/ Rejoin(i) \/ Release(i)

Spec == Init /\ [][Next]_vars

\* What tells states apart: `last` only says which step reached one.
View == <<q, last_start, rules, now, waiter, ask, halt, halt_for>>
\* Under a config whose invariants read `last`: which step reached a state,
\* and whose, tell it apart too (its time is `now`; what it read, only the
\* action properties). Under View, a step that changes nothing else (an
\* ask told to wait) reaches a state already seen, whose invariants TLC
\* does not check again.
StepView == <<q, last_start, rules, now, waiter, ask, halt, halt_for, last.act, last.i>>
\* A bound on the steps a model check explores (TLCGet("level")).
CONSTANT Depth
Shallow == TLCGet("level") < Depth

-----------------------------------------------------------------------------
(* The properties, in ADR 0010's words. An action property reads,          *)
(* of every step, `last'` as that step and the unprimed state as before;   *)
(* an invariant reads `last` as the step that reached the state.           *)

Asked == last'.act \in {"ask", "stale"}
Who == last'.i
At == last'.t
Started == Asked /\ q[Who].started_at = None /\ q'[Who].started_at # None

\* Resource r's line at t: the entries waiting for it, but those inside
\* their worker's hold of it, and those passed over in every queue: under
\* replaced rules, waiting for a resource whose slots are all held,
\* outside their window, or not urgent while urgent issues halt the queue.
InLine(r, t) == {j \in Waiting : r \in Res(j) /\ ~Covered(j, r) /\ j \notin Stale
                                 /\ ~Full(j) /\ ~Outside(j, t) /\ ~Halted(j)}
HostIdle == ~\E j \in Running : "host" \in Res(j)

\* What an ask leaves (its `last`): a waiter's ask changes no entry but its
\* own, and that one only by starting it; told to wait it changes nothing.
\* So the queue it read is the queue after it, but for its own start, and
\* the last start before it is `last.since`. Of a waiter's ask: idle work,
\* which leaves the queue on any answer but start, asks under none of the
\* configs that check these (Kinds = {"wait"}).
AskedBy(i) == last.act = "ask" /\ last.i = i
StartedBy(i) == AskedBy(i) /\ q[i].started_at # None
\* Nothing but i holds "host".
IdleBut(i) == ~\E j \in Running \ {i} : "host" \in Res(j)

\* It never starves the oldest: with nothing holding "host", the oldest
\* entry in its line, asking for it alone, starts at once when it asks,
\* whatever the reading (no line move past its bound waiting ahead of it,
\* nor, unless it is marked urgent, one marked so, nor, unless it is
\* marked or a landing's gate, a landing's gate):
\* a stale one ahead of it holds it up no more than one gone (factorium#241),
\* nor does one waiting for a full resource (factorium#284). Told to wait,
\* it is still in the line it read.
AtOnce ==
    \A i \in Ids :
        ~(/\ AskedBy(i) /\ i \in Waiting
          /\ Res(i) = {"host"} /\ HostIdle /\ waiter[i].rules >= rules
          /\ i \in InLine("host", last.t) /\ i = OldestOf(InLine("host", last.t))
          /\ ~\E j \in InLine("host", last.t) : Overdue(j, last.t)
          /\ ~\E j \in InLine("host", last.t) \ {i} : ask[j].urgent /\ ~ask[i].urgent
          /\ ~\E j \in InLine("host", last.t) \ {i} : ask[j].lands /\ ~ask[i].urgent /\ ~ask[i].lands)

\* Before in a resource's line, in ADR 0011's words: one waiting past the
\* starve bound (since it joined the line) before all that have not, oldest
\* first; then the shortest declared hold (a command's, the default),
\* oldest first.
StarvedAt(i, t) == t - Joined(i, t) > Starve
Declared(i) == IF ask[i].hold = None THEN Default ELSE ask[i].hold
Prior(a, b, t) ==
    IF StarvedAt(a, t) # StarvedAt(b, t) THEN StarvedAt(a, t)
    ELSE IF ~StarvedAt(a, t) /\ Declared(a) # Declared(b) THEN Declared(a) < Declared(b)
    ELSE Older(a, b)

\* In order: an entry starts only when, in the line of each resource it
\* asks for (but those its worker's hold holds), every entry before it
\* waits behind an entry that gives way (a worker's or a program's, or a
\* landing's gate) that a line move past its bound (this one) goes ahead
\* of; or, not past its bound itself, behind one giving way and not marked
\* urgent that an urgent one (this one) goes ahead of; or, neither past its
\* bound nor marked itself, behind one giving way and not marked that a
\* landing's gate (this one) goes ahead of.
GivesWay(i) == q[i].yields \/ ask[i].lands
InOrder ==
    [][ Started =>
        \A r \in Res(Who) : Covered(Who, r) \/
            \A j \in InLine(r, At) \ {Who} :
                \/ Prior(Who, j, At)
                \/ Overdue(Who, At) /\ \E y \in InLine(r, At) : GivesWay(y) /\ (y = j \/ Prior(y, j, At))
                \/ /\ ask[Who].urgent /\ ~Overdue(j, At)
                   /\ \E y \in InLine(r, At) : GivesWay(y) /\ ~ask[y].urgent /\ (y = j \/ Prior(y, j, At))
                \/ /\ ask[Who].lands /\ ~Overdue(j, At) /\ ~ask[j].urgent
                   /\ \E y \in InLine(r, At) : GivesWay(y) /\ ~ask[y].urgent /\ (y = j \/ Prior(y, j, At)) ]_vars

\* A line move past its bound goes ahead of workers' and programs' waiting
\* entries and landings' gates: none of those starts while one waits in
\* its line. (A line move asks for "host" alone, so the start beside it
\* leaves it in the line.)
LineAhead ==
    \A i \in Ids :
        (StartedBy(i) /\ GivesWay(i) /\ ~Overdue(i, last.t)) =>
            \A r \in Res(i) : Covered(i, r) \/ ~\E j \in InLine(r, last.t) : Overdue(j, last.t)

\* An urgent entry goes ahead of workers' and programs' waiting entries
\* and landings' gates not marked so: none of those starts while one
\* marked waits in its line.
UrgentAhead ==
    \A i \in Ids :
        (StartedBy(i) /\ GivesWay(i) /\ ~ask[i].urgent /\ ~Overdue(i, last.t)) =>
            \A r \in Res(i) : Covered(i, r) \/ ~\E j \in InLine(r, last.t) : ask[j].urgent

\* A landing's gate goes ahead of workers' and programs' waiting entries
\* not marked urgent: none of those starts while one waits in its line.
LandsAhead ==
    \A i \in Ids :
        (StartedBy(i) /\ q[i].yields /\ ~ask[i].urgent) =>
            \A r \in Res(i) : Covered(i, r) \/ ~\E j \in InLine(r, last.t) : ask[j].lands

\* While urgent issues halt the queue, no worker's or program's entry but
\* an urgent one starts.
HaltHolds ==
    [][ Started /\ halt /\ q[Who].yields => UrgentOf(Who) ]_vars

\* It starts one beside what holds "host" only when the host has room, on a
\* reading taken heavy_settle after the last start of one holding it, or
\* heavy_settle_idle after it on a reading well under the busy limit.
Settled ==
    \A i \in Ids :
        (StartedBy(i) /\ "host" \in Res(i) /\ ~IdleBut(i)) =>
            /\ last.room /\ last.read > last.since
            /\ last.read - last.since >= IF last.idle THEN SettleIdle ELSE Settle

\* `slots = n`: n holders at most. A start takes a slot of each named
\* resource it asks for (but those its worker's hold holds) only while one
\* is free.
SlotsKept == \A r \in Named : Cardinality(Holders(r)) <= Slots

\* A hold with a window starts only inside it.
InWindow ==
    [][ Started /\ ask[Who].win # None => At >= ask[Who].win ]_vars

\* A worker's own command asking only for what its hold holds runs inside
\* it: it takes no slot of its own, so it starts at once when it asks.
InsideHold ==
    [][ (/\ Asked /\ waiter[Who].rules >= rules
         /\ \A r \in Res(Who) : Covered(Who, r))
        => q'[Who].started_at # None ]_vars

\* A running entry is never cut short for a line move: a waiter's running
\* entry is ended only once unqueued, or once it holds "host" with an entry
\* of its line held up past the hold bound (counted from when both were
\* there). Idle work gives way.
\* An invariant of the state a look leaves (its `last`): a look is the one
\* step that ends an entry's steps, and it changes no entry of the queue,
\* so the queue it read is the queue after it.
Looked(i) == last.act = "look" /\ last.i = i
NotCutShort ==
    \A i \in Ids :
        (Looked(i) /\ waiter[i].kind = "wait" /\ waiter[i].stop # "")
        => \/ waiter[i].stop = "unqueued" /\ ~q[i].in
           \/ waiter[i].stop = "held" /\ "host" \in Res(i)
              /\ \E j \in InLine("host", last.t) : last.t - Max(q[i].started_at, q[j].queued_at) > Hold

\* One holding "host" that holds others in its line up past the hold
\* bound is ended: its waiter's next look ends it.
HeldEnded ==
    \A i \in Ids :
        (Looked(i) /\ q[i].in /\ "host" \in Res(i)
         /\ \E j \in InLine("host", last.t) : last.t - Max(q[i].started_at, q[j].queued_at) > Hold)
        => waiter[i].stop # ""

\* An unqueued run's waiter answers `unqueued`.
UnqueuedRunAnswers ==
    \A i \in Ids : waiter[i].unqueued = "running" =>
        waiter[i].answer \in {"", "unqueued"}

\* An unqueued entry's waiter answers `unqueued`, waiting or running: it is
\* not queued again, and never runs.
UnqueuedAnswers ==
    \A i \in Ids : waiter[i].unqueued # "" =>
        ~q[i].in /\ waiter[i].answer \in {"", "unqueued"}

\* A stale entry holds none up: with nothing running, the oldest waiting
\* entry under the daemon's rules starts at once when it asks, a stale one
\* waiting ahead of it (factorium#241). An invariant of the state an ask
\* leaves (its `last`): an ask that leaves its entry waiting changes
\* nothing else (a refused one's is a `stale` step), so the queue it read
\* is the queue after it. Of a waiter's ask: idle work, which leaves the
\* queue on any answer but start, asks under AtOnce (HeavyStarts.cfg).
StaleHoldsNone ==
    \A i \in Ids :
        ~(/\ AskedBy(i) /\ i \in Waiting
          /\ Res(i) = {"host"} /\ HostIdle /\ waiter[i].rules >= rules /\ Stale # {}
          /\ i \in InLine("host", last.t) /\ i = OldestOf(InLine("host", last.t))
          /\ ~\E j \in InLine("host", last.t) : Overdue(j, last.t))

\* A waiter of an older binary is refused, and keeps its place: its entry
\* does not start, and stays as it was while its waiter goes on.
StaleRefused ==
    [][ (Asked /\ waiter[Who].rules < rules) =>
        /\ q'[Who].started_at = None
        /\ waiter'[Who].phase = "waits" => q'[Who] = q[Who] ]_vars

\* An entry asked again keeps the place it had: rejoined, it is queued
\* before every waiting entry queued after its first ask. An invariant of
\* the state a rejoin leaves (its `last`): a rejoin changes no entry but
\* its own, and its waiter keeps the place.
KeptPlace ==
    \A i \in Ids :
        (last.act = "rejoin" /\ last.i = i) =>
            \A j \in Waiting \ {i} :
                q[j].queued_at > waiter[i].place => q[i].queued_at < q[j].queued_at

\* Under a config whose invariants of an ask read what it read (`room`,
\* `read`, `since`), which View and StepView leave out: whether each holds
\* tells states apart too. Where none fails it is View, state for state; a
\* state that breaks one is never taken for one seen that does not.
StartView == <<View, AtOnce, LineAhead, UrgentAhead, LandsAhead, Settled>>

\* The broken variants, each of the code as it could break, and the
\* property it breaks (`./models/heavy.sh broken`).
BrokenVariants == [
    room_first       |-> "AtOnce",       \* the room read before "nothing runs"
    by_id            |-> "InOrder",      \* the first by id, not by when it was queued
    no_line_ahead    |-> "LineAhead",    \* `next` is just the oldest
    no_settle        |-> "Settled",      \* no settle interval beside what runs
    settle_at_ask    |-> "Settled",      \* settled at the ask, not the reading (#102)
    idle_near        |-> "Settled",      \* the idle settle on a reading near the limit
    line_cuts_short  |-> "NotCutShort",  \* a line move past its bound ends what runs
    held_from_queued |-> "NotCutShort",  \* held counted from the waiter's queueing alone
    no_held          |-> "HeldEnded",    \* the hold bound never ends one
    answer_ran       |-> "UnqueuedRunAnswers", \* run_held does not look whether it is held
    requeues         |-> "UnqueuedAnswers",    \* take_turn queues an unqueued waiter again (#186)
    no_stale         |-> "StaleRefused", \* heavy.start takes any rules
    stale_requeues   |-> "StaleRefused", \* the exec'd waiter queues anew, at the back
    stale_holds      |-> "StaleHoldsNone", \* a stale entry keeps the head (#241)
    rejoins_at_back  |-> "KeptPlace",    \* asked again, it queues at the back (#193)
    full_holds       |-> "AtOnce",       \* one waiting for a full resource keeps its place (#284)
    no_slots         |-> "SlotsKept",    \* a named resource's slots are never full
    no_cover         |-> "InsideHold",   \* a worker's command waits for a slot beside its hold
    no_window        |-> "InWindow",     \* a hold's window is not waited for (#286)
    by_age           |-> "InOrder",      \* declared holds are not read: oldest first
    no_starve        |-> "InOrder",      \* none goes ahead for waiting past the starve bound
    no_halt          |-> "HaltHolds",    \* a halt passes none over
    no_urgent        |-> "UrgentAhead",  \* the urgent mark is not read in the line
    urgent_first     |-> "LineAhead",    \* an urgent one goes ahead of a line move past its bound
    no_lands         |-> "LandsAhead"    \* a landing's gate is ranked as another line move
]
=============================================================================
