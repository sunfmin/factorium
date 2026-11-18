//! The owner's view and hold of the host: the checkout list, the workers,
//! the kit, the accounts, the settings, holds, and the heavy queue's unqueue.
//! The tracker is `issues.rs`; doctor is `doctor.rs` (docs/adr/0001).

use std::path::Path;

use factorium_domain::checkout::{Add, AddCheckout, Named, RemoveCheckout};
use factorium_domain::heavy;
use factorium_domain::kit::*;
use factorium_domain::line::{Hold, HoldLandings, Lines};
use factorium_domain::line_id;
use factorium_domain::settings::{Set, SetSetting, Setting};
use factorium_domain::tracker::{self, shown};
use factorium_harness::Harness;
use factum::{Uuid, Value};
use serde_json::json;

use crate::{Answer, App, Refusal};

impl App {
    // ---- checkouts ----

    /// Puts a repo on the list. `base` is where promotions land, `proving`
    /// where landings land. The repo needs an `origin` holding `base`; a
    /// proving branch it lacks is made at `base`.
    pub async fn checkout_add(&self, name: &str, path: &Path, base: &str, proving: &str) -> Answer {
        let path =
            std::fs::canonicalize(path).map_err(|e| Refusal::new("no_path", format!("{}: {e}", path.display())))?;
        let git = self.host.git(&path);
        let base_tip = git.fetch(base).map_err(|e| Refusal::new("no_base", format!("origin has no {base}: {e}")))?;
        if git.fetch(proving).is_err() {
            git.push_requiring(&base_tip, proving, None).map_err(crate::fail("no_proving"))?;
        }
        let add = Add {
            name: name.into(),
            path: path.to_string_lossy().into_owned(),
            base: base.into(),
            proving: proving.into(),
        };
        self.submit::<AddCheckout>(&add).await?;
        Ok(json!(add))
    }

    pub async fn checkout_remove(&self, name: &str) -> Answer {
        self.submit::<RemoveCheckout>(Named { name: name.into() }).await?;
        Ok(json!({ "name": name, "removed": true }))
    }

    pub async fn checkout_list(&self) -> Answer {
        Ok(shown(&self.rows(tracker::listed()).await?))
    }

    /// The workers, live ones unless `all`.
    pub async fn workers(&self, all: bool) -> Answer {
        Ok(shown(&self.rows(tracker::workers(None, all)).await?))
    }

    // ---- the kit and the accounts ----

    pub async fn kit(&self) -> Answer {
        let mut kit = self.state::<Kits>(kit_id()).await?;
        // The command that will run, as a launch makes it.
        for e in &mut kit.entries {
            if let Some(h) = factorium_harness::Harness::read(&e.harness) {
                e.command = h.command(&e.command);
            }
        }
        Ok(json!({ "entries": kit.entries, "pins": kit.pins }))
    }

    /// Sets the kit: `harness=command[@model]` entries, in priority order.
    pub async fn kit_set(&self, entries: &[String]) -> Answer {
        let parse = |e: &String| -> Answer<Entry> {
            let (h, rest) =
                e.split_once('=').ok_or_else(|| Refusal::new("bad_entry", format!("{e}: harness=command[@model]")))?;
            let (command, model) = match rest.rsplit_once('@') {
                Some((c, m)) if !m.contains(' ') => (c, Some(m.to_string())),
                _ => (rest, None),
            };
            Ok(Entry { harness: h.trim().into(), command: command.trim().into(), model })
        };
        let entries = entries.iter().map(parse).collect::<Answer<Vec<_>>>()?;
        for e in &entries {
            known(&e.harness)?;
        }
        self.submit::<SetTheKit>(SetKit { entries }).await?;
        self.kit().await
    }

    /// Pins a checkout to one harness of the kit, or unpins it.
    pub async fn kit_pin(&self, checkout: &str, harness: Option<&str>) -> Answer {
        self.checkout(checkout).await?;
        harness.map(known).transpose()?;
        self.submit::<PinCheckout>(Pin { checkout: checkout.into(), harness: harness.map(Into::into) }).await?;
        self.kit().await
    }

    /// The engine accounts mytokens holds, as this machine can use them,
    /// with what each has left.
    pub async fn accounts(&self) -> Answer {
        let kit = self.state::<Kits>(kit_id()).await?;
        let now = self.now();
        let none = Account::default();
        let rows: Vec<Value> = self
            .accounts_here()
            .usable
            .into_iter()
            .map(|u| {
                let a = kit.accounts.get(&u.name).unwrap_or(&none);
                json!({ "name": u.name, "harness": u.harness.key(), "left": a.left(now), "limited_until": a.limited_until })
            })
            .collect();
        Ok(Value::Array(rows))
    }

    // ---- settings and holds ----

    /// The settings, or, given a value, sets one.
    pub async fn settings_set(&self, name: Option<&str>, value: Option<u64>) -> Answer {
        if let (Some(name), Some(value)) = (name, value) {
            self.submit::<SetSetting>(Set { name: name.into(), value }).await?;
        }
        let s = self.settings().await?;
        let row = |t: Setting| json!({ "name": t.name(), "value": s.get(t), "unit": t.unit().name(), "default": t.default(), "what": t.what() });
        Ok(match name {
            Some(n) => row(Setting::named(n).ok_or_else(|| Refusal::new("no_setting", n))?),
            None => Setting::all().map(row).collect(),
        })
    }

    /// Holds a checkout's landings, or with `None` releases them.
    pub async fn hold(&self, checkout: &str, why: Option<&str>) -> Answer {
        self.checkout(checkout).await?;
        self.submit::<HoldLandings>(Hold { checkout: checkout.into(), why: why.map(Into::into) }).await?;
        let l = self.state::<Lines>(line_id(checkout)).await?;
        Ok(json!({ "checkout": checkout, "held": l.held }))
    }

    /// `factorium unqueue`: takes entry `id` off the heavy queue, waiting or
    /// running; its owner, if it still asks, queues again behind the rest.
    pub async fn unqueue(&self, id: Uuid) -> Answer {
        let q = self.state::<heavy::Heavy>(heavy::queue_id()).await?;
        let Some(entry) = q.entries.into_iter().find(|e| e.id == id) else {
            return Err(Refusal::new("no_entry", format!("the heavy queue holds no entry {id}")));
        };
        self.submit::<heavy::EndHeavy>(heavy::End { id }).await?;
        Ok(json!({ "unqueued": entry }))
    }
}

/// Refuses `harness` unless it names a harness factorium starts.
fn known(harness: &str) -> Answer<()> {
    if Harness::read(harness).is_some() {
        return Ok(());
    }
    let keys: Vec<&str> = Harness::all().iter().map(|h| h.key()).collect();
    Err(Refusal::new("no_harness", format!("{harness} is none of {}", keys.join(", "))))
}
