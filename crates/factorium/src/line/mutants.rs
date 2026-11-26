//! A promotion's mutants, where the checkout's `slimfit.toml` holds their
//! record at the base (`[check] mutants_target`): before its slow gate, the
//! promotion asks slimfit's listing of the range whether it owes one; if it
//! does, it files one issue for it, which starts ahead of the ordinary ones,
//! and waits for it within `mutants_bound` of its filing. The rules are
//! [`factorium_domain::mutants`]'s.

use factorium_domain::checkout::Checkout;
use factorium_domain::issue::{Open, Status};
use factorium_domain::line::{self, Lines};
use factorium_domain::line_id;
use factorium_domain::mutants::{self, Before};
use factorium_domain::settings::Time;
use factorium_domain::when::utc;
use factorium_host::Git;
use serde_json::{Value, json};

use super::Did;
use crate::{Answer, App};

impl App {
    /// What the promotion of `c`'s proving `tip`, checked out in the clone
    /// `here`, does about its mutants before its slow gate: `None`, it goes
    /// on to it; else what it did instead: filed the issue that owes them,
    /// or waits for it. A listing slimfit refuses holds nothing back: the
    /// slow gate runs slimfit too.
    pub(super) async fn mutants_first(&self, c: &Checkout, here: &Git, tip: &str) -> Answer<Option<Did>> {
        let Some(target) = here.shown(tip, "slimfit.toml").and_then(|t| mutants::held_at(&t)) else {
            return Ok(None);
        };
        let l = self.state::<Lines>(line_id(&c.name)).await?;
        let (mut open, mut startable) = (None, false);
        if let Some(filed) = l.awaited {
            let i = self.issue(&c.name, filed.number).await?;
            if i.status != Status::Closed {
                open = Some(filed);
                startable = i.worker.is_some() || self.choice(&c.name, &i.on).await.is_ok();
            }
        }
        let bound = self.settings().await?.ms(Time::MutantsBound);
        match mutants::before(open, startable, self.now(), bound) {
            Before::Wait { number, until } => {
                return Ok(Some(Err(format!(
                    "waits for its mutants issue {}#{number}, at most until {}",
                    c.name,
                    utc(until)
                ))));
            }
            Before::Without { .. } => return Ok(None),
            Before::Ask => {}
        }
        if self.memory.lock().unwrap().owes_none.get(&c.name).is_some_and(|t| t == tip) {
            return Ok(None);
        }
        let dir = here.dir.to_string_lossy();
        let said = self.host.programs().run("slimfit", &["check", "--repo", &dir, "--mutants", "--target", &target]);
        let Some(listing) = serde_json::from_str::<Value>(&said.out).ok().filter(|_| said.ok) else {
            return Ok(None);
        };
        if !mutants::owes(&listing) {
            self.memory.lock().unwrap().owes_none.insert(c.name.clone(), tip.to_string());
            return Ok(None);
        }
        let from = listing["landing"]["from"].as_str().unwrap_or(&target).to_string();
        let (title, body, task) = mutants::issue(&c.proving, &target, &from, tip);
        let open = Open { checkout: c.name.clone(), title, body, task: Some(task), ..Open::default() };
        let number = self.open_issue(open).await?;
        self.submit::<line::AwaitMutants>(line::Await { checkout: c.name.clone(), number }).await?;
        let filed = format!("{}#{number}", c.name);
        Ok(Some(Ok(json!({ "promotion": c.name, "mutants": filed, "range": format!("{from}..{tip}") }))))
    }
}
