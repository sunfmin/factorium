# 27. A session is told only through its outbox

Status: accepted

## Context

factorium types lines at agent sessions it does not control: a harness
may not be up yet, a turn may be running, a dialog may eat a line, Orca
may refuse. A line typed and lost leaves a worker waiting for word that
never comes; a line typed twice makes it act twice. A tick that waited
for the typing would hold every other step behind one slow terminal.

## Decision

- Every message to a session (a brief, a landing's red, a round, `watch
  tell` and `watch restart`, a comment, word that its role changed) goes
  through that session's outbox: one record of who it was from, the line
  typed, and how it went (`factorium outbox`).
- Nothing is typed before the harness is up, nor ahead of the brief.
- A message queued behind a running turn is found taken in the session
  file on a later look; not taken once the turn is over, it is typed again;
  still unread 10 min after it was given, it is missed. A missed message
  sends its worker to the watch at once; a missed round is passed again.
- A brief not typed is held and tried again on the next look; any other
  message not typed is refused to its sender, and a red no one hears sends
  its worker to the watch.
- Word that its role changed changes nothing in the task: missed, it holds
  nothing up and is looked for again on each look, given no grace after it.
- And no tick waits for its typing at all: the tick gives the message to the outboxes and goes on, and it is typed beside the ticks, on a store connection of its own.
- A session typed at beside the ticks is not tried again by a look until that ended; the daemon lets it end before it stops, and swaps to a new binary only once none runs.

## Consequences

- A prompt Claude Code took is read from its session file as typed, or in
  the block it writes a long paste in (`<pasted_content id="…">`).
- A tick's `roles` answers each worker told and its delivery as it stood
  when the tick ended (`queued` while it is typed).
