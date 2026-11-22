-- factorium#95: "parked" is renamed "waiting for human". The code knows only
-- the new names, so a db written before must be rewritten once, with the
-- daemon stopped, once the binary that carries #95 is installed (the cut
-- that installs it swaps the daemon to it; until this runs, an issue whose
-- events or snapshot hold the old names does not load):
--
--   factorium service remove        # stops the daemon; launchd no longer restarts it
--   sqlite3 ~/.local/share/factorium/factorium.db ".backup $HOME/.local/share/factorium/factorium.db.before-95"
--   sqlite3 ~/.local/share/factorium/factorium.db < packaging/migrate-wait-for-human.sql
--   factorium service install       # starts the installed binary again
--
-- Running it twice changes nothing the second time.
BEGIN;

-- The event that leaves an issue to a person.
UPDATE events SET kind = 'IssueWaitingForHuman' WHERE kind = 'IssueParked';

-- An issue opened waiting: its flag.
UPDATE events
SET data = json_remove(
        json_set(data, '$.waiting_for_human',
                 json(CASE WHEN json_extract(data, '$.parked') THEN 'true' ELSE 'false' END)),
        '$.parked')
WHERE kind = 'IssueOpened' AND json_type(data, '$.parked') IS NOT NULL;

-- The command an event came from.
UPDATE events SET meta = json_set(meta, '$.command', 'issue.wait_for_human')
WHERE json_extract(meta, '$.command') = 'issue.park';

-- The tracker's issue rows (its projection stays at its version: rewritten, not rebuilt).
UPDATE "rows:tracker:issues" SET row = json_set(row, '$.status', 'waiting_for_human')
WHERE json_extract(row, '$.status') = 'parked';

-- Issue snapshots hold the old status; they are rebuilt from the events.
DELETE FROM snapshots WHERE stream LIKE 'issue/%';

-- No answers to rewrite: since factum#43 a store keeps nothing per key
-- beside the log (its step 7 drops the `answers` table).

COMMIT;
