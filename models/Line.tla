-------------------------------- MODULE Line --------------------------------
(***************************************************************************)
(* One checkout's line (ADR 0012, ADR 0019 and ADR 0023), as factorium     *)
(* moves it.                                                               *)
(*                                                                         *)
(* The remote (GitHub) holds `proving` and `base`. A commit is named by    *)
(* what it holds: the workers' branches merged onto the start tip, in      *)
(* order (<<>> is the start tip, <<1, 2>> worker 1's merge then worker     *)
(* 2's; a 0 after them, that merge with the records its gate regenerated   *)
(* amended into it). So one commit is an ancestor of another when it is a  *)
(* prefix of it, and merging a branch that a commit holds already is that  *)
(* commit itself (`git merge --no-ff` of a merged branch is "Already up to *)
(* date").                                                                 *)
(*                                                                         *)
(* The landings are stacked (ADR 0019; crates/domain/src/stack.rs,         *)
(* crates/factorium/src/line/stack.rs): every worker that asks to land is  *)
(* merged onto the merge below it, the first onto the proving tip; their   *)
(* gates run at once, beside the ticks; one push moves proving to the      *)
(* deepest merge whose gate and every gate below it are green, and each    *)
(* worker in it lands; the red above them is told then, and leaves with    *)
(* the layers above it. The stack is the store's: the daemon lets it go as *)
(* it starts. Urgent landings, a held line, merges that conflict and       *)
(* branches that move are not modelled.                                    *)
(*                                                                         *)
(* The processes: the daemon (`factorium run`), and `factorium promote`    *)
(* from the CLI. `Daemons = {"d1"}` is the code: a daemon holds its pid    *)
(* file's lock for its life (crates/factorium/src/lock.rs), so a second is *)
(* refused, however close in time it starts; two (LineTwo.cfg) is what     *)
(* the lock rules out. A daemon's landings, promotion and cut run apart,   *)
(* each kind one at a time (ADR 0018; its tick's pass over the line:       *)
(* crates/factorium/src/line.rs, App::lines and App::carry). A gate and a  *)
(* cut's steps run beside the ticks, so any step of another move or        *)
(* process goes between; what a tick does once a gate ended (the push, the *)
(* records) it does whole before another of its moves (InTick). A daemon   *)
(* may die between any two steps (`Die`), and is started again with       *)
(* nothing in flight and its stacks let go; its tick may fail on a store   *)
(* write the disk refuses (`Fail`), what it had in flight dropped, the     *)
(* stack kept. Each push to the remote answers ok, refused (the tip it     *)
(* requires moved), or is lost: it took effect, and the daemon reads an    *)
(* error, as for a refusal.                                                *)
(*                                                                         *)
(* What the store records (crates/domain/src/line.rs, stack.rs, worker.rs, *)
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
    MaxDeaths,  \* how often a process dies or a tick fails
    MaxLost,    \* how many pushes' answers are lost
    EndSplit,   \* the worker's end recorded before its issue's close, as two steps
    Regenerates, \* a merge may take records its gate regenerates from the copy below
    Bug         \* "none", or a broken variant

CLI == "cli"
Procs == Daemons \cup {CLI}
None == <<0>>   \* no commit: no worker's slot is 0
Amended == 0    \* after a merge: its regenerated records amended into it

Range(s) == {s[i] : i \in DOMAIN s}
IsPrefix(a, b) == Len(a) <= Len(b) /\ SubSeq(b, 1, Len(a)) = a
Drop(s, x) == SelectSeq(s, LAMBDA y : y # x)
Below(s, i) == SubSeq(s, 1, i - 1)

VARIABLES
    proving,     \* the remote's proving tip
    base,        \* the remote's base tip
    fast,        \* the commits whose fast gate is recorded green (Gates)
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
    stack,       \* the checkout's landing stack, the bottom first
    push,        \* push[d]: daemon d's push of the stack, or its red, in flight
    promotion,   \* promotion[p]: process p's promotion in flight
    cutting,     \* cutting[d]: daemon d's cut in flight
    lock,        \* who holds the promotion lock ("": none)
    deaths,      \* processes that died, and ticks that failed
    lost,        \* pushes whose answer was lost
    failing,     \* the daemon's tick failed on a refused command
    told,        \* of each red told, the gates below its layer as it left
    ends         \* ends[w]: how often the code ended worker w landed

vars == <<proving, base, fast, slow, status, asks, dones, closed, unpromoted,
          uncut, promoted, red, cuts, stack, push, promotion, cutting, lock,
          deaths, lost, failing, told, ends>>

IdleD == [pc |-> "idle", ls |-> <<>>, k |-> 0, w |-> 0, below |-> {}]
IdleP == [pc |-> "idle", tip |-> None, base |-> None]
IdleC == [pc |-> "idle", commit |-> None]

Commit == Seq(Workers \cup {Amended})
Layer == [w : Workers, on : Commit, merged : Commit, regen : BOOLEAN,
          gate : {"runs", "green", "red"}, commit : Commit]

TypeOK ==
    /\ proving \in Commit /\ base \in Commit
    /\ status \in [Workers -> {"working", "landing", "landed"}]
    /\ closed \subseteq Workers
    /\ stack \in Seq(Layer)
    /\ push \in [Daemons -> [pc : {"idle", "pushed", "recorded", "ended", "landed", "stale", "red"},
                             ls : Seq(Layer), k : Nat, w : Workers \cup {0},
                             below : SUBSET {"runs", "green", "red"}]]
    /\ promotion \in [Procs -> [pc : {"idle", "read", "gated", "pushed"},
                                tip : Commit, base : Commit]]
    /\ lock \in Procs \cup {""}
    /\ ends \in [Workers -> Nat]

Init ==
    /\ proving = <<>> /\ base = <<>>
    /\ fast = {} /\ slow = {}
    /\ status = [w \in Workers |-> "working"]
    /\ asks = <<>> /\ dones = [w \in Workers |-> 0] /\ closed = {}
    /\ unpromoted = <<>> /\ uncut = <<>> /\ promoted = None /\ red = None
    /\ cuts = {}
    /\ stack = <<>>
    /\ push = [d \in Daemons |-> IdleD]
    /\ promotion = [p \in Procs |-> IdleP]
    /\ cutting = [d \in Daemons |-> IdleC]
    /\ lock = "" /\ deaths = 0 /\ lost = 0 /\ failing = FALSE
    /\ told = {} /\ ends = [w \in Workers |-> 0]

-----------------------------------------------------------------------------
(* A worker.                                                               *)

\* `factorium worker done` (AskLanding): it asks to land, after those asking.
Done(w) ==
    /\ status[w] = "working" /\ dones[w] < MaxDones
    /\ status' = [status EXCEPT ![w] = "landing"]
    /\ asks' = Append(asks, w)
    /\ dones' = [dones EXCEPT ![w] = @ + 1]
    /\ UNCHANGED <<proving, base, fast, slow, closed, unpromoted, uncut, promoted,
                   red, cuts, stack, push, promotion, cutting, lock, deaths, lost,
                   failing, told, ends>>

\* RedLanding: the worker is told red and goes on working.
Red(w) == /\ status' = [status EXCEPT ![w] = "working"]
          /\ asks' = Drop(asks, w)

-----------------------------------------------------------------------------
(* The landing stack (App::landings), daemon d's tick over it.             *)

\* Daemon d's tick is finishing a move whose gate ended: the push and the
\* records, or the red told, taken whole before any other move of the tick.
\* The gates and the other moves' steps run beside the ticks, so any of
\* them goes on meanwhile.
InTick(d) == push[d].pc # "idle" \/ promotion[d].pc \in {"gated", "pushed"}
Settle(d) == push' = [push EXCEPT ![d] = IdleD]

Stacked(w) == \E i \in DOMAIN stack : stack[i].w = w
\* Stack::landable: from the bottom, each layer whose gate is green.
Landable == LET n == IF \E i \in DOMAIN stack : stack[i].gate # "green"
                     THEN CHOOSE i \in DOMAIN stack : stack[i].gate # "green"
                                  /\ \A j \in 1..(i - 1) : stack[j].gate = "green"
                     ELSE Len(stack) + 1
            IN Below(stack, n)
\* Unstacked (App::unstack): the layer of w and every layer above it leave.
From(w) == IF Stacked(w) THEN Below(stack, CHOOSE i \in DOMAIN stack : stack[i].w = w) ELSE stack

\* App::stack_on (StackLanding): the first asking worker not stacked yet,
\* its branch merged onto the top's merge, or onto the proving tip it
\* fetched on an empty stack; its gate asked. Whether its merge took records
\* the gate regenerates from the copy below is the merge's: a layer above
\* one that took them takes them too.
StackOn(d) ==
    /\ ~InTick(d)
    /\ \E i \in DOMAIN asks : ~Stacked(asks[i])
    /\ LET w == asks[CHOOSE i \in DOMAIN asks : ~Stacked(asks[i]) /\ \A j \in 1..(i - 1) : Stacked(asks[j])]
           on == IF stack = <<>> \/ Bug = "same_tip" THEN proving ELSE stack[Len(stack)].merged
           merged == IF w \in Range(on) THEN on ELSE Append(on, w)
       IN \E regen \in IF Regenerates THEN BOOLEAN ELSE {FALSE} :
            stack' = Append(stack, [w |-> w, on |-> on, merged |-> merged,
                                    regen |-> regen \/ (stack # <<>> /\ stack[Len(stack)].regen),
                                    gate |-> "runs", commit |-> merged])
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted,
                   uncut, promoted, red, cuts, push, promotion, cutting, lock, deaths,
                   lost, failing, told, ends>>

\* App::gate (Kind::Fast) of a stacked merge, beside the ticks, and
\* App::layer_gated as a tick reads it ended: a tree green already is green
\* at once; else its gate runs, green (GreenLayer: it lands as its merge,
\* with its regenerated records amended) or red (RedLayer).
StackGate(d) ==
    /\ ~InTick(d)
    /\ \E i \in DOMAIN stack :
        LET l == stack[i]
            amended == IF l.regen THEN Append(l.merged, Amended) ELSE l.merged
            Ended(g, c) == stack' = [stack EXCEPT ![i].gate = g, ![i].commit = c]
        IN /\ l.gate = "runs"
           /\ \/ /\ l.merged \in fast
                 /\ Ended("green", l.merged) /\ UNCHANGED fast
              \/ /\ l.merged \notin fast
                 /\ fast' = fast \cup {amended} /\ Ended("green", amended)
              \/ /\ l.merged \notin fast
                 /\ IF Bug = "push_red" THEN Ended("green", amended) ELSE Ended("red", l.merged)
                 /\ UNCHANGED fast
    /\ UNCHANGED <<proving, base, slow, status, asks, dones, closed, unpromoted, uncut,
                   promoted, red, cuts, push, promotion, cutting, lock, deaths, lost,
                   failing, told, ends>>

\* App::push_stack: a landable layer whose worker no longer asks to land
\* leaves the stack, with those above it. Else proving is pushed to the
\* deepest landable commit, requiring the tip the bottom merged onto
\* (push_requiring). Refused, or its answer lost, the whole stack leaves
\* (every layer lands again on the new tip).
StackPush(d) ==
    LET L == Landable IN
    /\ ~InTick(d) /\ L # <<>>
    /\ IF Bug # "unchecked" /\ \E i \in DOMAIN L : status[L[i].w] # "landing"
       THEN /\ stack' = From(L[CHOOSE i \in DOMAIN L : status[L[i].w] # "landing"].w)
            /\ UNCHANGED <<proving, push, lost>>
       ELSE IF proving = L[1].on \/ Bug = "no_lease"
       THEN /\ proving' = L[Len(L)].commit
            /\ \/ /\ push' = [push EXCEPT ![d] = [IdleD EXCEPT !.pc = "pushed", !.ls = L, !.k = 1]]
                  /\ UNCHANGED <<stack, lost>>
               \/ lost < MaxLost /\ lost' = lost + 1 /\ stack' = <<>> /\ UNCHANGED push
       ELSE stack' = <<>> /\ UNCHANGED <<proving, push, lost>>
    /\ UNCHANGED <<base, fast, slow, status, asks, dones, closed, unpromoted, uncut,
                   promoted, red, cuts, promotion, cutting, lock, deaths, failing, told, ends>>

\* The layer of the push being recorded, and the commit it lands as: the
\* pushed commit for the deepest, its merge for each below.
At(d) == push[d].ls[push[d].k]
AsCommit(d) == IF push[d].k = Len(push[d].ls) THEN At(d).commit ELSE At(d).merged
\* The next layer of the push, or, all recorded, the stack's record.
NextLayer(d) == push' = [push EXCEPT ![d] = IF push[d].k < Len(push[d].ls)
                                            THEN [push[d] EXCEPT !.pc = "pushed", !.k = push[d].k + 1]
                                            ELSE [push[d] EXCEPT !.pc = "landed"]]

\* RecordLanding (line.landed): a commit the line holds already is landed
\* already, and is not recorded again (a daemon whose tick stopped after
\* recording it lands it again).
Landed(c) == c \in Range(unpromoted) \/ c \in Range(uncut) \/ c = promoted
LandRecord(d) ==
    /\ push[d].pc = "pushed"
    /\ LET c == AsCommit(d) IN
       IF Bug = "landed_refused" /\ c \in Range(unpromoted)
       THEN failing' = TRUE /\ Settle(d) /\ UNCHANGED unpromoted
       ELSE /\ unpromoted' = IF Landed(c) THEN unpromoted ELSE Append(unpromoted, c)
            /\ push' = [push EXCEPT ![d].pc = "recorded"] /\ UNCHANGED failing
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, uncut,
                   promoted, red, cuts, stack, promotion, cutting, lock, deaths, lost, told, ends>>

\* App::end_worker (life.rs `finish`): the worker recorded landed
\* (GreenLanding), then its issue closed (CloseIssue): one step, or two
\* (EndSplit) between which the daemon may die.
LandEnd(d) ==
    LET w == At(d).w IN
    /\ push[d].pc = "recorded"
    /\ ends' = [ends EXCEPT ![w] = @ + 1]
    /\ IF status[w] = "landed"
       \* GreenLanding refuses a worker that ended: the tick fails.
       THEN failing' = TRUE /\ Settle(d) /\ UNCHANGED <<status, asks, closed>>
       ELSE /\ status' = [status EXCEPT ![w] = "landed"]
            /\ asks' = Drop(asks, w)
            /\ IF EndSplit THEN push' = [push EXCEPT ![d].pc = "ended"] /\ UNCHANGED closed
               ELSE closed' = closed \cup {w} /\ NextLayer(d)
            /\ UNCHANGED failing
    /\ UNCHANGED <<proving, base, fast, slow, dones, unpromoted, uncut, promoted,
                   red, cuts, stack, promotion, cutting, lock, deaths, lost, told>>

LandClose(d) ==
    /\ push[d].pc = "ended"
    /\ closed' = closed \cup {At(d).w}
    /\ NextLayer(d)
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, unpromoted, uncut,
                   promoted, red, cuts, stack, promotion, cutting, lock, deaths, lost,
                   failing, told, ends>>

\* LandStack (stack.landed): the pushed layers leave the stack; only what
\* is landable lands, else it is refused. Layers left merged onto a commit
\* that is not the one pushed (it amended regenerated records) are next.
StackLanded(d) ==
    LET ls == push[d].ls
        top == ls[Len(ls)]
        n == CHOOSE i \in DOMAIN stack : stack[i].w = top.w
        rest == SubSeq(stack, n + 1, Len(stack))
    IN
    /\ push[d].pc = "landed"
    /\ IF \E i \in DOMAIN Landable : Landable[i].w = top.w
       THEN /\ stack' = rest
            /\ push' = [push EXCEPT ![d] = IF rest # <<>> /\ rest[1].on # top.commit
                                           THEN [IdleD EXCEPT !.pc = "stale", !.w = rest[1].w] ELSE IdleD]
            /\ UNCHANGED failing
       ELSE failing' = TRUE /\ Settle(d) /\ UNCHANGED stack
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted, uncut,
                   promoted, red, cuts, promotion, cutting, lock, deaths, lost, told, ends>>

\* Unstacked: the layers on the unamended merge leave, to stack again.
StackStale(d) ==
    /\ push[d].pc = "stale"
    /\ stack' = From(push[d].w)
    /\ Settle(d)
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted, uncut,
                   promoted, red, cuts, promotion, cutting, lock, deaths, lost, failing, told, ends>>

\* App::land_stack (Stack::red): the first layer not green, if its gate is
\* red, leaves with every layer above it (Unstacked); then its worker is
\* told (TellRed).
StackRed(d) ==
    /\ ~InTick(d)
    /\ \E i \in DOMAIN stack :
        /\ stack[i].gate = "red"
        /\ Bug = "red_early" \/ i = Len(Landable) + 1
        /\ stack' = Below(stack, i)
        /\ push' = [push EXCEPT ![d] = [IdleD EXCEPT !.pc = "red", !.w = stack[i].w,
                                         !.below = {stack[j].gate : j \in 1..(i - 1)}]]
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted, uncut,
                   promoted, red, cuts, promotion, cutting, lock, deaths, lost, failing, told, ends>>

\* RedLanding, of a worker that still asks to land.
TellRed(d) ==
    LET w == push[d].w IN
    /\ push[d].pc = "red"
    /\ IF status[w] = "landing" /\ Bug # "red_untold"
       THEN Red(w) /\ told' = told \cup {push[d].below}
       ELSE UNCHANGED <<status, asks, told>>
    /\ Settle(d)
    /\ UNCHANGED <<proving, base, fast, slow, dones, closed, unpromoted, uncut, promoted,
                   red, cuts, stack, promotion, cutting, lock, deaths, lost, failing, ends>>

\* App::close_landed: a tick closes an open issue whose worker ended
\* landed, so a daemon that died between GreenLanding and CloseIssue leaves
\* it closed by the next tick.
LandSweep(d) ==
    /\ ~InTick(d) /\ Bug # "unswept"
    /\ \E w \in Workers :
        /\ status[w] = "landed" /\ w \notin closed
        /\ closed' = closed \cup {w}
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, unpromoted, uncut,
                   promoted, red, cuts, stack, push, promotion, cutting, lock, deaths,
                   lost, failing, told, ends>>

-----------------------------------------------------------------------------
(* A promotion (App::promote_tip), the daemon's or `factorium promote`'s.  *)

Prom(p, f, v) == promotion' = [promotion EXCEPT ![p][f] = v]
\* The promotion ends, and its lock is released.
Release(p) == /\ promotion' = [promotion EXCEPT ![p] = IdleP]
              /\ lock' = IF lock = p THEN "" ELSE lock
\* A refused command: the daemon's tick fails; `factorium promote` answers the refusal.
Refused(p) == IF p = CLI THEN UNCHANGED failing ELSE failing' = TRUE

\* What a promotion's steps leave as it was.
PromKeeps == UNCHANGED <<status, asks, dones, closed, stack, push, cutting, told, ends>>

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
    /\ PromKeeps
    /\ UNCHANGED <<proving, base, fast, slow, unpromoted, uncut, promoted, red, cuts,
                   deaths, lost, failing>>

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
    /\ PromKeeps
    /\ UNCHANGED <<proving, base, fast, unpromoted, uncut, promoted, cuts, deaths, lost>>

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
    /\ PromKeeps
    /\ UNCHANGED <<proving, fast, slow, unpromoted, uncut, promoted, cuts, deaths>>

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
    /\ PromKeeps
    /\ UNCHANGED <<proving, base, fast, slow, cuts, deaths, lost>>

-----------------------------------------------------------------------------
(* A cut (App::cut_base), daemon d's: a version of the base tip it fetched *)

CutRead(d) ==
    /\ ~InTick(d) /\ cutting[d].pc = "idle" /\ uncut # <<>>
    /\ cutting' = [cutting EXCEPT ![d] = [pc |-> "read",
                    commit |-> IF Bug = "cut_proving" THEN proving ELSE base]]
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted,
                   uncut, promoted, red, cuts, stack, push, promotion, lock, deaths, lost,
                   failing, told, ends>>

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
                   promoted, red, stack, push, promotion, lock, deaths, lost, told, ends>>

-----------------------------------------------------------------------------
(* Faults.                                                                 *)

\* A process dies: what it had in flight is gone; the lock it held is
\* released with it (a file lock). A daemon is started again, and lets the
\* stacks go (App::clear_stacks): their gates ran in the process it replaces.
Die(p) ==
    /\ deaths < MaxDeaths /\ deaths' = deaths + 1
    /\ IF p \in Daemons
       THEN /\ push' = [push EXCEPT ![p] = IdleD]
            /\ cutting' = [cutting EXCEPT ![p] = IdleC]
            /\ stack' = <<>>
       ELSE UNCHANGED <<push, cutting, stack>>
    /\ promotion' = [promotion EXCEPT ![p] = IdleP]
    /\ lock' = IF lock = p THEN "" ELSE lock
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted,
                   uncut, promoted, red, cuts, lost, failing, told, ends>>

\* Daemon d's tick stops on a store write the disk refuses, in the middle of
\* its push's records or its red: the next tick goes on from the stack.
Fail(d) ==
    /\ deaths < MaxDeaths /\ deaths' = deaths + 1
    /\ push[d].pc # "idle"
    /\ Settle(d)
    /\ UNCHANGED <<proving, base, fast, slow, status, asks, dones, closed, unpromoted,
                   uncut, promoted, red, cuts, stack, promotion, cutting, lock, lost,
                   failing, told, ends>>

-----------------------------------------------------------------------------

\* What a daemon does next, as its tick goes on.
Daemon(d) ==
    \/ StackOn(d) \/ StackGate(d) \/ StackPush(d) \/ LandRecord(d) \/ LandEnd(d)
    \/ LandClose(d) \/ StackLanded(d) \/ StackStale(d) \/ StackRed(d) \/ TellRed(d)
    \/ LandSweep(d)
    \/ PromRead(d) \/ PromGate(d) \/ PromPush(d) \/ PromRecord(d)
    \/ CutRead(d) \/ CutMake(d)

\* `factorium promote`, once someone runs it: its steps to the end.
Promoting(p) == PromGate(p) \/ PromPush(p) \/ PromRecord(p)

Next ==
    \/ \E w \in Workers : Done(w)
    \/ \E d \in Daemons : Daemon(d) \/ Fail(d)
    \/ PromRead(CLI) \/ Promoting(CLI)
    \/ \E p \in Procs : Die(p)

\* Each daemon ticks on; a promotion run by hand runs to its end.
Fairness == /\ \A d \in Daemons : WF_vars(Daemon(d))
            /\ WF_vars(Promoting(CLI))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
(* The properties, in ADR 0012's and ADR 0019's words.                     *)

OnProving(w) == w \in Range(proving)

\* Proving only ever holds commits whose fast gate, or the gate of one above
\* it in the same push, ran on a tree with every commit below it: each
\* commit proving holds is held by one gated green that proving holds.
ProvingOnlyGated ==
    \A n \in 1..Len(proving) :
        \E g \in fast : IsPrefix(SubSeq(proving, 1, n), g) /\ IsPrefix(g, proving)

\* The base is always an ancestor of proving, and only a tree whose slow
\* gate passed (or the tip it started from).
BaseBehindProving == IsPrefix(base, proving) /\ (base = <<>> \/ base \in slow)

\* An issue closed by a landing has its branch's commits on proving; and
\* one whose push took effect is not sent back to work told red (the lost
\* answer: the daemon lands it again, merged already, green already).
ClosedOnProving == \A w \in Workers : w \in closed => OnProving(w)
PushedNotRed == \A w \in Workers : OnProving(w) => status[w] # "working"
ClosedIsLanded == ClosedOnProving /\ PushedNotRed

\* A worker told red had every gate below its layer green.
RedAfterGreenBelow == \A b \in told : b \subseteq {"green"}

\* No worker lands twice: the code ends each worker landed at most once.
LandsOnce == \A w \in Workers : ends[w] <= 1

\* At most one push, one promotion and one cut per checkout in flight.
OneAtATime ==
    /\ Cardinality({d \in Daemons : push[d].pc # "idle"}) <= 1
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
    push_red       |-> "ProvingOnlyGated",   \* a red fast gate is recorded green
    same_tip       |-> "ClosedOnProving",    \* each landing merged onto the proving tip, not the stack's top
    red_early      |-> "RedAfterGreenBelow", \* a red is told while a gate below it runs
    unchecked      |-> "LandsOnce",          \* a landable layer whose worker landed already is pushed again
    no_lease       |-> "ClosedOnProving",    \* a push requires no tip (two daemons)
    promote_red    |-> "BaseBehindProving",  \* a red slow gate still promotes
    no_lock        |-> "OneAtATime",         \* a second promotion beside the running one
    cut_proving    |-> "CutFromBase",        \* a cut of the proving tip
    landed_refused |-> "LandedCloses",       \* line.landed refuses a commit landed already
    red_untold     |-> "LandingMoves",       \* a red layer leaves the stack untold: gated again
    unswept        |-> "LandedCloses",       \* no tick closes an issue its landed worker holds
    cut_clears     |-> "PromotedCut"         \* a cut records every promotion cut, one made while it ran too
]
=============================================================================
