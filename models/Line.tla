-------------------------------- MODULE Line --------------------------------
(***************************************************************************)
(* One checkout's line (ADR 0012 and ADR 0023), as factorium moves it.     *)
(*                                                                         *)
(* The remote (GitHub) holds `proving` and `base`. A commit is named by    *)
(* what it holds: the workers' branches merged onto the start tip, in      *)
(* order (<<>> is the start tip, <<1, 2>> worker 1's merge then worker     *)
(* 2's). So one commit is an ancestor of another when it is a prefix of    *)
(* it, and merging a branch that proving holds already is proving itself   *)
(* (`git merge --no-ff` of a merged branch is "Already up to date").      *)
(*                                                                         *)
(* The processes: the daemon (`factorium run`), and `factorium promote`    *)
(* from the CLI. `Daemons = {"d1"}` is the code: a daemon holds its pid    *)
(* file's lock for its life (crates/factorium/src/lock.rs), so a second is *)
(* refused, however close in time it starts; two (LineTwo.cfg) is what     *)
(* the lock rules out. A daemon's landing, promotion and cut run apart,    *)
(* each kind one at a time (ADR 0018; its tick's pass over the line:       *)
(* crates/factorium/src/line.rs, App::lines and App::carry), each move as  *)
(* the steps App::land, App::promote_tip and App::cut_base take. A move's  *)
(* gate and a cut's steps run beside the ticks, so any step of another     *)
(* move or process goes between; what a tick does once a gate ended (the  *)
(* push, the records) it does whole before another of its moves (InTick). *)
(* A daemon may die between any two steps (`Die`), and is started again with *)
(* nothing in flight; what it recorded and pushed stays. Each push to the  *)
(* remote answers ok, refused (the tip it requires moved), or is lost: it  *)
(* took effect, and the daemon reads an error, as for a refusal.           *)
(*                                                                         *)
(* What the store records (crates/domain/src/line.rs, worker.rs,           *)
(* issue.rs, gate.rs) is one step each. A command the store refuses fails  *)
(* the daemon's tick (`failing`): doctor's finding of a tick that fails.   *)
(*                                                                         *)
(* `Bug` names a broken variant (BrokenVariants below); "none" is the code *)
(* as it is. `./models/line.sh broken` shows each property failing on its  *)
(* variant.                                                                *)
(***************************************************************************)
EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS
    Workers,    \* the workers of the checkout, by their slot
    Daemons,    \* the daemons that run at once (the code: one; two the lock rules out)
    MaxDones,   \* how often each worker runs `worker done`
    MaxDeaths,  \* how often a process dies
    MaxLost,    \* how many pushes' answers are lost
    EndSplit,   \* the worker's end recorded before its issue's close, as two steps
    Bug         \* "none", or a broken variant

CLI == "cli"
Procs == Daemons \cup {CLI}
None == <<0>>   \* no commit: no worker's slot is 0

Range(s) == {s[i] : i \in DOMAIN s}
IsPrefix(a, b) == Len(a) <= Len(b) /\ SubSeq(b, 1, Len(a)) = a
Drop(s, x) == SelectSeq(s, LAMBDA y : y # x)

VARIABLES
    proving,     \* the remote's proving tip
    base,        \* the remote's base tip
    fast,        \* the trees whose fast gate is recorded green (Gates)
    slow,        \* the trees whose slow gate is recorded green
    status,      \* status[w]: "working", "landing" (asks to land), "landed"
    asks,        \* the workers asking to land, the first to ask first
    dones,       \* how often each worker ran `worker done`
    closed,      \* the issues closed, by their worker
    unpromoted,  \* the line's landings not yet promoted, oldest first
    uncut,       \* the line's promotions not yet in a cut
    promoted,    \* the last promoted tip (None: none yet)
    red,         \* the proving tip whose slow gate went red (None: none)
    cuts,        \* the commits a cut made a version of
    landing,     \* landing[d]: daemon d's landing in flight
    promotion,   \* promotion[p]: process p's promotion in flight
    cutting,     \* cutting[d]: daemon d's cut in flight
    lock,        \* who holds the promotion lock ("": none)
    deaths,      \* processes that died
    lost,        \* pushes whose answer was lost
    failing      \* the daemon's tick failed on a refused command

vars == <<proving, base, fast, slow, status, asks, dones, closed, unpromoted,
          uncut, promoted, red, cuts, landing, promotion, cutting, lock,
          deaths, lost, failing>>

Idle == [pc |-> "idle", w |-> 0, tip |-> None, merge |-> None]
IdleP == [pc |-> "idle", tip |-> None, base |-> None]
IdleC == [pc |-> "idle", commit |-> None]

Commits == {s \in Seq(Workers) : Len(s) <= Cardinality(Workers)}

TypeOK ==
    /\ proving \in Seq(Workers) /\ base \in Seq(Workers)
    /\ status \in [Workers -> {"working", "landing", "landed"}]
    /\ closed \subseteq Workers
    /\ landing \in [Daemons -> [pc : {"idle", "merged", "gated", "pushed", "recorded", "ended"},
                                w : Workers \cup {0}, tip : Seq(Workers \cup {0}),
                                merge : Seq(Workers \cup {0})]]
    /\ promotion \in [Procs -> [pc : {"idle", "read", "gated", "pushed"},
                                tip : Seq(Workers \cup {0}), base : Seq(Workers \cup {0})]]
    /\ lock \in Procs \cup {""}

Init ==
    /\ proving = <<>> /\ base = <<>>
    /\ fast = {} /\ slow = {}
    /\ status = [w \in Workers |-> "working"]
    /\ asks = <<>> /\ dones = [w \in Workers |-> 0] /\ closed = {}
    /\ unpromoted = <<>> /\ uncut = <<>> /\ promoted = None /\ red = None
    /\ cuts = {}
    /\ landing = [d \in Daemons |-> Idle]
    /\ promotion = [p \in Procs |-> IdleP]
    /\ cutting = [d \in Daemons |-> IdleC]
    /\ lock = "" /\ deaths = 0 /\ lost = 0 /\ failing = FALSE

-----------------------------------------------------------------------------
(* A worker.                                                               *)

\* `factorium worker done` (AskLanding): it asks to land, after those asking.
Done(w) ==
    /\ status[w] = "working" /\ dones[w] < MaxDones
    /\ status' = [status EXCEPT ![w] = "landing"]
    /\ asks' = Append(asks, w)
    /\ dones' = [dones EXCEPT ![w] = @ + 1]
    /\ UNCHANGED <<proving, base, fast, slow, closed, unpromoted, uncut, promoted,
                   red, cuts, landing, promotion, cutting, lock, deaths, lost, failing>>

\* RedLanding: the worker is told red and goes on working.
Red(w) == /\ status' = [status EXCEPT ![w] = "working"]
          /\ asks' = Drop(asks, w)

-----------------------------------------------------------------------------
(* A landing (App::land), daemon d's.                                      *)

\* Daemon d's tick is finishing a move whose gate ended: the push and the
\* records, taken whole before any other move of the tick. Its moves'
\* gates and steps run beside the ticks, so any of them goes on meanwhile.
InTick(d) == landing[d].pc \in {"gated", "pushed", "recorded", "ended"} \/ promotion[d].pc \in {"gated", "pushed"}
Land(d, f, v) == landing' = [landing EXCEPT ![d][f] = v]
Settle(d) == landing' = [landing EXCEPT ![d] = Idle]

\* Line::landing: the first asking lands (one landing per checkout); its
\* branch merged onto the proving tip it fetched (`fresh`, `merge_from`).
LandRead(d) ==
    /\ ~InTick(d) /\ landing[d].pc = "idle" /\ asks # <<>>
    /\ LET w == Head(asks) IN
        landing' = [landing EXCEPT ![d] = [pc |-> "merged", w |-> w, tip |-> proving,
                     merge |-> IF w \in Range(proving) THEN proving ELSE Append(proving, w)]]
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted,
                   uncut, promoted, red, cuts, promotion, cutting, lock, deaths, lost, failing>>

\* App::gate (Kind::Fast): a tree green already is not gated again;
\* else it runs, and is recorded (RecordGate). Red: the worker is told.
LandGate(d) ==
    LET l == landing[d] IN
    /\ l.pc = "merged" /\ ~InTick(d)
    /\ \/ /\ l.merge \in fast
          /\ Land(d, "pc", "gated") /\ UNCHANGED <<fast, status, asks>>
       \/ /\ l.merge \notin fast
          /\ fast' = fast \cup {l.merge}
          /\ Land(d, "pc", "gated") /\ UNCHANGED <<status, asks>>
       \/ /\ l.merge \notin fast
          /\ IF Bug = "push_red" THEN Land(d, "pc", "gated") /\ UNCHANGED <<status, asks>>
             ELSE IF Bug = "red_untold" THEN Settle(d) /\ UNCHANGED <<status, asks>>
             ELSE Red(l.w) /\ Settle(d)
          /\ UNCHANGED fast
    /\ UNCHANGED <<proving, base, slow, dones, closed, unpromoted, uncut, promoted,
                   red, cuts, promotion, cutting, lock, deaths, lost, failing>>

\* push_requiring(merged, proving, tip): it takes effect only on the tip it
\* read. Refused, or its answer lost, the daemon lands again next tick.
LandPush(d) ==
    LET l == landing[d] IN
    /\ l.pc = "gated"
    /\ IF proving = l.tip \/ Bug = "no_lease"
       THEN /\ proving' = l.merge
            /\ \/ Land(d, "pc", "pushed") /\ UNCHANGED lost
               \/ lost < MaxLost /\ lost' = lost + 1 /\ Settle(d)
       ELSE Settle(d) /\ UNCHANGED <<proving, lost>>
    /\ UNCHANGED <<base, fast, slow, status, asks, dones, closed, unpromoted, uncut,
                   promoted, red, cuts, promotion, cutting, lock, deaths, failing>>

\* RecordLanding (line.landed): a commit the line holds already is landed
\* already, and is not recorded again (a daemon that died after recording
\* it lands it again).
Landed(c) == c \in Range(unpromoted) \/ c \in Range(uncut) \/ c = promoted
LandRecord(d) ==
    LET l == landing[d] IN
    /\ l.pc = "pushed"
    /\ IF Bug = "landed_refused" /\ l.merge \in Range(unpromoted)
       THEN failing' = TRUE /\ Settle(d) /\ UNCHANGED unpromoted
       ELSE /\ unpromoted' = IF Landed(l.merge) THEN unpromoted ELSE Append(unpromoted, l.merge)
            /\ Land(d, "pc", "recorded") /\ UNCHANGED failing
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, uncut,
                   promoted, red, cuts, promotion, cutting, lock, deaths, lost>>

\* App::end_worker (life.rs `finish`): the worker recorded landed
\* (GreenLanding), then its issue closed (CloseIssue): one step, or two
\* (EndSplit) between which the daemon may die.
LandEnd(d) ==
    LET l == landing[d] IN
    /\ l.pc = "recorded"
    /\ IF status[l.w] = "landed"
       \* GreenLanding refuses a worker that ended: the tick fails.
       THEN failing' = TRUE /\ Settle(d) /\ UNCHANGED <<status, asks, closed>>
       ELSE /\ status' = [status EXCEPT ![l.w] = "landed"]
            /\ asks' = Drop(asks, l.w)
            /\ IF EndSplit THEN Land(d, "pc", "ended") /\ UNCHANGED closed
               ELSE closed' = closed \cup {l.w} /\ Settle(d)
            /\ UNCHANGED failing
    /\ UNCHANGED <<proving, base, fast, slow, dones, unpromoted, uncut, promoted,
                   red, cuts, promotion, cutting, lock, deaths, lost>>

LandClose(d) ==
    /\ landing[d].pc = "ended"
    /\ closed' = closed \cup {landing[d].w}
    /\ Settle(d)
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, unpromoted, uncut,
                   promoted, red, cuts, promotion, cutting, lock, deaths, lost, failing>>

\* App::close_landed (factorium#192): a tick closes an open issue whose
\* worker ended landed, so a daemon that died between GreenLanding and
\* CloseIssue leaves it closed by the next tick.
LandSweep(d) ==
    /\ ~InTick(d) /\ Bug # "unswept"
    /\ \E w \in Workers :
        /\ status[w] = "landed" /\ w \notin closed
        /\ closed' = closed \cup {w}
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, unpromoted, uncut,
                   promoted, red, cuts, landing, promotion, cutting, lock, deaths, lost, failing>>

-----------------------------------------------------------------------------
(* A promotion (App::promote_tip), the daemon's or `factorium promote`'s.  *)

Prom(p, f, v) == promotion' = [promotion EXCEPT ![p][f] = v]
\* The promotion ends, and its lock is released.
Release(p) == /\ promotion' = [promotion EXCEPT ![p] = IdleP]
              /\ lock' = IF lock = p THEN "" ELSE lock
\* A refused command: the daemon's tick fails; `factorium promote` answers the refusal.
Refused(p) == IF p = CLI THEN UNCHANGED failing ELSE failing' = TRUE

\* Line::promotable, Line::on_tip: something landed since the last
\* promotion, on a tip not red (by hand: whatever the tip); then the lock
\* (`Running::take`: none while another runs), then the base fetched.
PromRead(p) ==
    /\ p \in Daemons => ~InTick(p)
    /\ promotion[p].pc = "idle"
    /\ unpromoted # <<>>
    /\ p # CLI => red # proving
    /\ lock = "" \/ Bug = "no_lock"
    /\ lock' = p
    /\ promotion' = [promotion EXCEPT ![p] = [pc |-> "read", tip |-> proving, base |-> base]]
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted,
                   uncut, promoted, red, cuts, landing, cutting, deaths, lost, failing>>

\* App::gate (Kind::Slow) on the tip: red is recorded (line.promote,
\* PromotionRed), and the promotion ends.
PromGate(p) ==
    LET r == promotion[p] IN
    /\ r.pc = "read" /\ (p \in Daemons => ~InTick(p))
    /\ \/ /\ r.tip \in slow
          /\ Prom(p, "pc", "gated") /\ UNCHANGED <<slow, red, lock, failing>>
       \/ /\ r.tip \notin slow
          /\ slow' = slow \cup {r.tip}
          /\ Prom(p, "pc", "gated") /\ UNCHANGED <<red, lock, failing>>
       \/ /\ r.tip \notin slow
          /\ IF Bug = "promote_red" THEN Prom(p, "pc", "gated") /\ UNCHANGED <<red, lock, failing>>
             ELSE /\ Release(p)
                  /\ IF unpromoted = <<>> THEN Refused(p) /\ UNCHANGED red
                     ELSE red' = r.tip /\ UNCHANGED failing
          /\ UNCHANGED slow
    /\ UNCHANGED <<proving, base, fast, status, asks, dones, closed, unpromoted, uncut,
                   promoted, cuts, landing, cutting, deaths, lost>>

\* A base that is not an ancestor of the tip moved outside factorium: red
\* (Promotion::red). Else push_requiring(tip, base, base read).
PromPush(p) ==
    LET r == promotion[p] IN
    /\ r.pc = "gated"
    /\ IF ~IsPrefix(r.base, r.tip)
       THEN /\ Release(p)
            /\ IF unpromoted = <<>> THEN Refused(p) /\ UNCHANGED red
               ELSE red' = r.tip /\ UNCHANGED failing
            /\ UNCHANGED <<base, lost>>
       ELSE IF base = r.base \/ Bug = "no_base_lease"
       THEN /\ base' = r.tip
            /\ \/ Prom(p, "pc", "pushed") /\ UNCHANGED <<lock, lost>>
               \/ lost < MaxLost /\ lost' = lost + 1 /\ Release(p)
            /\ UNCHANGED <<red, failing>>
       ELSE Release(p) /\ UNCHANGED <<base, red, lost, failing>>
    /\ UNCHANGED <<proving, fast, slow, status, asks, dones, closed, unpromoted, uncut,
                   promoted, cuts, landing, cutting, deaths>>

\* line.promote (Promoted): every landing up to the tip is promoted with
\* it; nothing landed since the last promotion is refused.
PromRecord(p) ==
    LET r == promotion[p]
        n == IF \E i \in DOMAIN unpromoted : unpromoted[i] = r.tip
             THEN CHOOSE i \in DOMAIN unpromoted : unpromoted[i] = r.tip
             ELSE Len(unpromoted)
    IN
    /\ r.pc = "pushed"
    /\ Release(p)
    /\ IF unpromoted = <<>>
       THEN Refused(p) /\ UNCHANGED <<unpromoted, uncut, promoted, red>>
       ELSE /\ uncut' = uncut \o SubSeq(unpromoted, 1, n)
            /\ unpromoted' = SubSeq(unpromoted, n + 1, Len(unpromoted))
            /\ promoted' = r.tip /\ red' = None
            /\ UNCHANGED failing
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, cuts,
                   landing, cutting, deaths, lost>>

-----------------------------------------------------------------------------
(* A cut (App::cut_base), daemon d's: a version of the base tip it fetched *)

CutRead(d) ==
    /\ ~InTick(d) /\ cutting[d].pc = "idle" /\ uncut # <<>>
    /\ cutting' = [cutting EXCEPT ![d] = [pc |-> "read",
                    commit |-> IF Bug = "cut_proving" THEN proving ELSE base]]
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted,
                   uncut, promoted, red, cuts, landing, promotion, lock, deaths, lost, failing>>

\* Its steps, the tag and the install, in one heavy turn beside the ticks;
\* recorded once (line.cut, CutMade): the promotions up to the base it read
\* are cut, one made while it ran is owed its own.
CutMake(d) ==
    /\ cutting[d].pc = "read" /\ ~InTick(d)
    /\ cutting' = [cutting EXCEPT ![d] = IdleC]
    /\ IF uncut = <<>> THEN failing' = TRUE /\ UNCHANGED <<cuts, uncut>>
       ELSE /\ cuts' = cuts \cup {cutting[d].commit}
            /\ uncut' = IF Bug = "cut_clears" THEN <<>>
                         ELSE SelectSeq(uncut, LAMBDA x : ~IsPrefix(x, cutting[d].commit))
            /\ UNCHANGED failing
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted,
                   promoted, red, landing, promotion, lock, deaths, lost>>

-----------------------------------------------------------------------------
(* Faults.                                                                 *)

\* A process dies: what it had in flight is gone; the lock it held is
\* released with it (a file lock). A daemon is started again.
Die(p) ==
    /\ deaths < MaxDeaths /\ deaths' = deaths + 1
    /\ IF p \in Daemons
       THEN /\ landing' = [landing EXCEPT ![p] = Idle]
            /\ cutting' = [cutting EXCEPT ![p] = IdleC]
       ELSE UNCHANGED <<landing, cutting>>
    /\ promotion' = [promotion EXCEPT ![p] = IdleP]
    /\ lock' = IF lock = p THEN "" ELSE lock
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted,
                   uncut, promoted, red, cuts, lost, failing>>

-----------------------------------------------------------------------------

\* What a daemon does next, as its tick goes on.
Daemon(d) ==
    \/ LandRead(d) \/ LandGate(d) \/ LandPush(d) \/ LandRecord(d) \/ LandEnd(d) \/ LandClose(d) \/ LandSweep(d)
    \/ PromRead(d) \/ PromGate(d) \/ PromPush(d) \/ PromRecord(d)
    \/ CutRead(d) \/ CutMake(d)

\* `factorium promote`, once someone runs it: its steps to the end.
Promoting(p) == PromGate(p) \/ PromPush(p) \/ PromRecord(p)

Next ==
    \/ \E w \in Workers : Done(w)
    \/ \E d \in Daemons : Daemon(d)
    \/ PromRead(CLI) \/ Promoting(CLI)
    \/ \E p \in Procs : Die(p)

\* Each daemon ticks on; a promotion run by hand runs to its end.
Fairness == /\ \A d \in Daemons : WF_vars(Daemon(d))
            /\ WF_vars(Promoting(CLI))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
(* The properties, in ADR 0012's words.                                    *)

OnProving(w) == w \in Range(proving)

\* Proving's tip is always a tree whose fast gate passed, or the tip it
\* started from.
ProvingOnlyGated == proving = <<>> \/ proving \in fast

\* The base is always an ancestor of proving, and only a tree whose slow
\* gate passed (or the tip it started from).
BaseBehindProving == IsPrefix(base, proving) /\ (base = <<>> \/ base \in slow)

\* An issue closed by a landing has its branch's commits on proving; and
\* one whose push took effect is not sent back to work told red (the lost
\* answer: the daemon lands it again, merged already, green already).
ClosedOnProving == \A w \in Workers : w \in closed => OnProving(w)
PushedNotRed == \A w \in Workers : OnProving(w) => status[w] # "working"
ClosedIsLanded == ClosedOnProving /\ PushedNotRed

\* At most one landing, one promotion and one cut per checkout in flight.
OneAtATime ==
    /\ Cardinality({d \in Daemons : landing[d].pc # "idle"}) <= 1
    /\ Cardinality({p \in Procs : promotion[p].pc # "idle"}) <= 1
    /\ Cardinality({d \in Daemons : cutting[d].pc # "idle"}) <= 1

\* A cut's version is a base commit.
CutFromBase == \A c \in cuts : IsPrefix(c, base)

\* Off the path, what the line owes is done, or is a finding: a landing
\* waiting is gated (landed or told red), or the tick fails; the oldest
\* unpromoted landing is promoted, or its tip is red, or the tick fails.
\* (Within the bounds: the daemon's promotion is due once the bound less
\* the hold bound passed, and its gate then goes ahead in the heavy queue,
\* which models/HeavyQueue.tla checks starts it within the hold bound;
\* doctor's findings are those of a move past its bound.)
LandingMoves == \A w \in Workers : (w \in Range(asks)) ~> (w \notin Range(asks) \/ failing)
PromotionMoves == (unpromoted # <<>>) ~> (unpromoted = <<>> \/ red # None \/ failing)

\* A branch whose push took effect has its issue closed.
LandedCloses == \A w \in Workers : OnProving(w) ~> (w \in closed)

\* A branch the base holds is in a cut, or the proving tip is red, or the
\* tick fails (findings, as for PromotionMoves): a promotion made while a
\* cut ran is cut by a later one.
PromotedCut == \A w \in Workers :
    (w \in Range(base)) ~> ((\E c \in cuts : w \in Range(c)) \/ red # None \/ failing)

\* The broken variants, each of the code as it could break, and the
\* property it breaks (`./models/line.sh broken`).
BrokenVariants == [
    push_red       |-> "ProvingOnlyGated",  \* a red fast gate still pushes
    promote_red    |-> "BaseBehindProving", \* a red slow gate still promotes
    no_lease       |-> "ClosedOnProving",    \* a landing's push requires no tip (two daemons)
    no_lock        |-> "OneAtATime",        \* a second promotion beside the running one (#72)
    cut_proving    |-> "CutFromBase",       \* a cut of the proving tip
    landed_refused |-> "LandedCloses",      \* line.landed refuses a commit landed already
    red_untold     |-> "LandingMoves",      \* a red landing is not recorded: gated again
    unswept        |-> "LandedCloses",      \* no tick closes an issue its landed worker holds
    cut_clears     |-> "PromotedCut"        \* a cut records every promotion cut, one made while it ran too
]
=============================================================================
