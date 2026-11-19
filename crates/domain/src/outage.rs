//! A store the daemon cannot read (SPEC §6): its looks fail, and with them
//! everything the store would carry to the watch. What reaches the watch
//! then is decided here, kept outside the store by the daemon: one episode
//! per error, one brief at its start and one each hour while it lasts.

use serde::{Deserialize, Serialize};

use crate::watch::Found;

/// How long an outage goes on before the watch is briefed of it again, ms.
pub const REPEAT: i64 = 3_600_000;

/// The watch's session as a look last read it from the store: where the
/// daemon reaches it when the store cannot be read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reach {
    pub terminal: String,
    pub harness: String,
    pub account: Option<String>,
}

/// Looks failing in a row on the same store error.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outage {
    pub error: String,
    /// When the first of them failed, ms.
    pub since: i64,
    pub failures: u64,
    /// When the watch was last briefed of it; `None`: not yet.
    pub briefed: Option<i64>,
}

impl Outage {
    /// The outage once one more look failed on `error` at `now`: the same
    /// error goes on, another starts its own episode.
    pub fn failed(before: Option<Outage>, error: &str, now: i64) -> Outage {
        match before {
            Some(o) if o.error == error => Outage { failures: o.failures + 1, ..o },
            _ => Outage { error: error.into(), since: now, failures: 1, briefed: None },
        }
    }

    /// Whether the watch is to be briefed of it at `now`.
    pub fn due(&self, now: i64) -> bool {
        self.briefed.is_none_or(|at| now - at >= REPEAT)
    }

    /// The brief typed at the watch's session: `version` the daemon's,
    /// `data` its data directory.
    pub fn brief(&self, version: &str, data: &str) -> String {
        format!(
            "factorium (the daemon, outside its store): every look has failed since {} ({} so far): {} — \
             factorium {version} cannot read its store, so no episode, round or `factorium` command works now. \
             Without them: read {data}/daemon.log, the data directory {data}, and Homebrew's factorium \
             (`brew list --versions factorium`: the versions it keeps; `brew info factorium`). The service runs \
             what Homebrew links, once restarted (`launchctl kickstart -k gui/$(id -u)/com.github.sunfmin.factorium`). Tell the owner what you found. \
             This comes again each hour while it lasts.",
            utc(self.since),
            self.failures,
            self.error
        )
    }
}

/// Doctor's one finding when it cannot read the store at all.
pub fn finding(error: &str, data: &str) -> Found {
    Found {
        key: "store".into(),
        what: format!("factorium cannot read its store in {data}: {error}"),
        fix: format!(
            "a binary and a database that do not match: see which factorium runs (`brew list --versions \
             factorium`) and {data}/daemon.log; link a version that reads this store, or migrate the store"
        ),
        heard: Vec::new(),
    }
}

/// `ms` since the epoch, as a UTC time a person reads.
pub fn utc(ms: i64) -> String {
    let (days, secs) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000) / 1000);
    // Days to a civil date (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", secs / 3600, secs / 60 % 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outage_is_briefed_once_and_again_each_hour_and_a_new_error_is_a_new_one() {
        let o = Outage::failed(None, "store: no such column", 1_000);
        assert!(o.due(1_000));
        let o = Outage { briefed: Some(1_000), ..Outage::failed(Some(o), "store: no such column", 61_000) };
        assert_eq!((o.since, o.failures), (1_000, 2));
        assert!(!o.due(1_000 + REPEAT - 1) && o.due(1_000 + REPEAT));
        let other = Outage::failed(Some(o), "store: disk I/O error", 70_000);
        assert_eq!((other.since, other.failures, other.briefed), (70_000, 1, None));
    }

    #[test]
    fn times_read_as_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(1_795_659_780_000), "2026-11-26 02:23:00 UTC");
        assert_eq!(utc(951_782_400_000), "2000-02-29 00:00:00 UTC");
    }
}
