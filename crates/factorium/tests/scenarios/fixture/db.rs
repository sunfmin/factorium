//! States of the database no `App` call can make: what an older binary or
//! older code left in it, a dead letter, a disk that refuses. Each is named
//! for the state it makes, and each is made through one tool, rusqlite.

use super::World;

impl World {
    /// The world's database, as one more program on it would open it.
    pub(super) fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.db).unwrap()
    }

    /// Runs `script` on the database: a migration as its owner would run it.
    pub fn migrate(&self, script: &str) {
        self.sql().execute_batch(script).unwrap();
    }

    /// The disk refuses to record events of `kind` while `refuses`, as a
    /// full disk or a failing one would: the write is aborted.
    pub fn disk_refuses(&self, kind: &str, refuses: bool) {
        let db = self.sql();
        db.execute_batch("DROP TRIGGER IF EXISTS refuses").unwrap();
        if refuses {
            let sql = format!(
                "CREATE TRIGGER refuses BEFORE INSERT ON events WHEN NEW.kind = '{kind}' \
                 BEGIN SELECT RAISE(ABORT, 'disk I/O error'); END"
            );
            db.execute_batch(&sql).unwrap();
        }
    }

    /// The store no longer reads: a column the code reads is not there, as
    /// on a database an older binary left that a newer one cannot read.
    /// `column` of the events is the one not there; any other gone before is back.
    pub fn store_reads_fail(&self, column: &str) {
        for c in ["data", "kind"] {
            let gone = format!("{c}_before");
            let there: bool = self.sql().prepare(&format!("SELECT {gone} FROM events LIMIT 0")).is_ok();
            if there {
                self.migrate(&format!("ALTER TABLE events RENAME COLUMN {gone} TO {c};"));
            }
        }
        self.migrate(&format!("ALTER TABLE events RENAME COLUMN {column} TO {column}_before;"));
    }

    /// A newer binary, a CLI call of it, appended an event of `kind`, which
    /// this code does not know, to an issue's stream (`DecidedNumbered`,
    /// v0.1.83, factorium#252).
    pub fn an_event_a_newer_binary_wrote(&self, kind: &str) {
        self.migrate(&format!(
            "INSERT INTO events (id, stream, version, kind, type_version, data, meta) \
             SELECT lower(hex(randomblob(16))), stream, MAX(version) + 1, '{kind}', 1, '{{}}', meta \
             FROM events WHERE stream = (SELECT stream FROM events WHERE kind = 'IssueOpened' LIMIT 1);"
        ));
    }

    /// Every stream has a snapshot older code left, of no use to this code.
    pub fn snapshots_older_code_left(&self) {
        self.migrate(
            "INSERT OR REPLACE INTO snapshots (stream, version, code, state) \
             SELECT stream, MAX(version), 'older', 'null' FROM events GROUP BY stream;",
        );
    }

    /// Each line has a snapshot kept by code `code` that says it last moved
    /// at `moved`, ms, and holds nothing.
    pub fn line_snapshots_kept_by(&self, code: &str, moved: i64) {
        self.migrate(&format!(
            "INSERT OR REPLACE INTO snapshots (stream, version, code, state) \
             SELECT stream, MAX(version), '{code}', json_object('unpromoted', json_array(), 'uncut', json_array(), \
                 'promoted', NULL, 'red', NULL, 'version', NULL, 'cut_failed', NULL, 'held', NULL, 'moved', {moved}) \
             FROM events WHERE stream IN (SELECT stream FROM events WHERE kind = 'Landed') GROUP BY stream;"
        ));
    }

    /// The issues' comments as the first version of their event wrote them:
    /// text alone, no author nor time.
    pub fn comments_as_their_first_version_wrote_them(&self) {
        self.migrate(
            "UPDATE events SET type_version = 1, data = json_object('text', json_extract(data, '$.text')) \
             WHERE kind = 'IssueCommented'; DELETE FROM snapshots;",
        );
    }

    /// Decided records as the code before factorium#243 wrote them: no
    /// numbers, and no sequence that gave any.
    pub fn records_as_code_before_243_wrote_them(&self) {
        self.migrate(
            "DELETE FROM events WHERE kind IN ('NumbersGiven', 'NumbersPlaced', 'DecidedNumbered'); \
             UPDATE events SET data = json_remove(data, '$.decided') \
             WHERE kind IN ('IssueOpened', 'IssueEdited', 'IssueCommented'); DELETE FROM snapshots;",
        );
    }

    /// What a waiting worker heard as the code before factorium#161 kept
    /// it: a copy of the comment `text` on the worker, by `worker.hear`.
    pub fn heard_as_code_before_161_kept_it(&self, text: &str) {
        let script = format!(
            "INSERT INTO events (id, stream, version, kind, type_version, data, meta) \
             SELECT lower(hex(randomblob(16))), stream, MAX(version) + 1, 'WorkerHeard', 1, \
                 json_object('text', '{text}', 'at', 0), json_set(meta, '$.command', 'worker.hear') \
             FROM events WHERE stream = (SELECT stream FROM events WHERE kind = 'WorkerWaits'); \
             DELETE FROM snapshots;"
        );
        self.migrate(&script);
    }

    /// The watch's start as an older binary recorded it: no rules.
    pub fn watch_started_as_an_older_binary_wrote_it(&self) {
        self.migrate(
            "UPDATE events SET data = json_remove(data, '$.rules', '$.data.rules') WHERE kind = 'WatchStarted'; \
             DELETE FROM snapshots;",
        );
    }

    /// Issues waiting for a human as the code before factorium#95 wrote
    /// them: `parked`, in their events, rows, snapshots and answers.
    pub fn issues_as_code_before_95_wrote_them(&self) {
        self.migrate(
            r#"
            UPDATE events SET kind = 'IssueParked' WHERE kind = 'IssueWaitingForHuman';
            UPDATE events SET data = json_remove(json_set(data, '$.parked', json(CASE WHEN
                json_extract(data, '$.waiting_for_human') THEN 'true' ELSE 'false' END)), '$.waiting_for_human')
                WHERE kind = 'IssueOpened';
            UPDATE events SET meta = json_set(meta, '$.command', 'issue.park')
                WHERE json_extract(meta, '$.command') = 'issue.wait_for_human';
            UPDATE "rows:tracker:issues" SET row = json_set(row, '$.status', 'parked')
                WHERE json_extract(row, '$.status') = 'waiting_for_human';
            UPDATE snapshots SET state = replace(state, '"waiting_for_human"', '"parked"');
            UPDATE answers SET answer = replace(replace(answer, '"IssueWaitingForHuman"', '"IssueParked"'),
                '"waiting_for_human":', '"parked":');"#,
        );
    }

    /// How many places still hold a name of the code before factorium#95.
    pub fn names_before_95(&self) -> i64 {
        let count = "SELECT (SELECT count(*) FROM events WHERE kind = 'IssueParked' OR data LIKE '%\"parked\":%' \
                     OR meta LIKE '%issue.park\"%') + (SELECT count(*) FROM \"rows:tracker:issues\" WHERE status = \
                     'parked') + (SELECT count(*) FROM answers WHERE answer LIKE '%parked%');";
        self.sql().query_row(count, [], |r| r.get(0)).unwrap()
    }

    /// How many events of `kind` the store holds.
    pub fn recorded(&self, kind: &str) -> i64 {
        self.sql().query_row("SELECT count(*) FROM events WHERE kind = ?1", [kind], |r| r.get(0)).unwrap()
    }

    /// An event the tracker cannot read: a checkout added with no data.
    pub fn a_dead_letter_for_the_tracker(&self) {
        let (event, stream) = (uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
        self.sql()
            .execute(
                "INSERT INTO events (id, stream, version, kind, type_version, data, meta) \
                 SELECT ?1, ?2, 1, 'CheckoutAdded', 1, '{}', meta FROM events LIMIT 1",
                [event.to_string(), format!("checkout/{stream}")],
            )
            .unwrap();
    }
}
