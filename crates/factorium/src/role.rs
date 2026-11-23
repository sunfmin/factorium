//! A role changed under a live worker (factorium#273): its session read its
//! role's rules and skills once, at its start, and a role directory is
//! linked live. On each tick a worker whose role's directory now holds
//! other content than it last knew of is told once, through its outbox as
//! a comment is: which files changed, and to re-read the role's core skill
//! before its next step. Its session's rules are written again with the
//! role's new ones, and what it was told is recorded, so the same change is
//! told only once.

use std::path::Path;

use factorium_domain::outbox::Sender;
use factorium_domain::worker::{NoteRoleTold, TellRole, Workers};
use factum::Value;
use serde_json::json;

use crate::kept::WorkerBranch;
use crate::session::{read_role, write_role_rules};
use crate::{Answer, App, Refusal, Want};

impl App {
    /// Tells each live worker whose role changed since it last knew of it;
    /// one waiting for the watch, unless parked on its engine's limit, is
    /// told once it works again. Answers each one told.
    pub(crate) async fn tell_role_changes(&self) -> Answer<Vec<Value>> {
        let mut told = Vec::new();
        for row in self.live_workers().await? {
            let wk = self.state::<Workers>(row.worker).await?;
            let Some(known) = wk.role_known() else { continue };
            let st = wk.started.as_ref().expect("a worker with a role started");
            let dir = &st.role.as_ref().expect("it has a role").dir;
            let now = read_role(Path::new(dir)).map_err(|e| Refusal::new("io", format!("its role {dir}: {e}")))?;
            if now.hash == known.hash {
                continue;
            }
            let Some(live) = self.live_or_none(row.worker, Want::Any).await? else { continue };
            if live.waiting && !live.on_limit {
                continue;
            }
            let changed = now.changed_from(&known.files);
            let name = WorkerBranch { checkout: st.checkout.clone(), number: st.number, attempt: st.attempt }.worker();
            let session_dir = self.data().issue_role(&name);
            let rules = factorium_harness::RULES;
            let mut also = String::new();
            if changed.iter().any(|f| f == rules) {
                write_role_rules(&session_dir, Path::new(dir)).map_err(|e| Refusal::new("io", e.to_string()))?;
                also = format!(", and your own rules, written again with its new ones: `{}`", session_dir.join(rules).display());
            }
            let text = format!(
                "your role `{dir}` changed since you last read it: {}. Before your next step, re-read its core skill and each changed file{also}.",
                changed.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")
            );
            let delivery = self.tell(&live.session, Sender::Role, &text, false).await?;
            self.submit::<NoteRoleTold>(TellRole { worker: row.worker, read: now }).await?;
            told.push(json!({ "worker": row.worker, "changed": changed, "delivery": delivery.word() }));
        }
        Ok(told)
    }
}
