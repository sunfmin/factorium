//! `issue wait`: looks at the issues it names on the host's clock until each
//! has ended as it waits for, or its timeout.

use factorium_domain::issue::{Issue, Outcome, Until};
use serde_json::json;

use crate::{Answer, App};

/// How often a waiter looks at its issues again: ms on the app's clock.
/// A look reads each issue's state, its snapshot and the events after it.
const LOOK: i64 = 1000;

impl App {
    /// Waits until each of `numbers` of `checkout` has ended as `until`
    /// says, or `timeout` ms have passed. Answers each issue's status,
    /// reason, commit (landed) and outcome, the worst of them as the
    /// answer's `outcome`, and its exit code as `code`.
    pub async fn issue_wait(&self, checkout: &str, numbers: &[u64], until: Until, timeout: Option<i64>) -> Answer {
        let deadline = timeout.map(|ms| self.now().saturating_add(ms));
        loop {
            let mut issues = Vec::with_capacity(numbers.len());
            for &n in numbers {
                issues.push(self.issue(checkout, n).await?);
            }
            let now = self.now();
            let timed_out = deadline.is_some_and(|d| now >= d);
            if timed_out || issues.iter().all(|i| i.ended(until).is_some()) {
                return Ok(answer(&issues, until));
            }
            let left = deadline.map_or(LOOK, |d| (d - now).min(LOOK));
            self.clock.sleep(left as u64).await;
        }
    }
}

/// What the waiter answers of `issues`: one not ended as `until` says timed out.
fn answer(issues: &[Issue], until: Until) -> serde_json::Value {
    let outcome = |i: &Issue| i.ended(until).unwrap_or(Outcome::Timeout);
    let worst = issues.iter().map(outcome).max().unwrap_or(Outcome::Landed);
    json!({
        "outcome": worst,
        "code": worst.code(),
        "issues": issues.iter().map(|i| json!({
            "checkout": i.checkout,
            "number": i.number,
            "title": i.title,
            "status": i.status,
            "reason": i.reason,
            "commit": i.landed_as(),
            "outcome": outcome(i),
        })).collect::<Vec<_>>(),
    })
}
