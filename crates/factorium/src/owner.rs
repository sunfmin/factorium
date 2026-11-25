//! The owner's view and hold of the host: the checkout list, the workers,
//! the kit, the accounts, the settings, holds, and the heavy queue's unqueue.
//! The tracker is `issues.rs`; doctor is `doctor.rs` (docs/adr/0001).

use std::path::Path;

use factorium_domain::checkout::{
    Add, AddCheckout, NameCheckoutRole, NameRole, Named, RemoveCheckout, TurnCheckoutIdle, TurnIdle,
};
use factorium_domain::heavy;
use factorium_domain::kit::*;
use factorium_domain::line::{Hold, HoldLandings, Lines};
use factorium_domain::line_id;
use factorium_domain::settings::{Declare, DeclareResource, Resource, Set, SetSetting, Setting};
use factorium_domain::tracker::{self, shown};
use factorium_harness::Harness;
use factum::Uuid;
use serde_json::json;

use crate::{Answer, App, Refusal};

impl App {
    // ---- checkouts ----

    /// Puts a repo on the list. `base` is where promotions land, `proving`
    /// where landings land. The repo needs an `origin` holding `base`; a
    /// proving branch it lacks is made at `base`. `role`, its default role
    /// ([`App::checkout_name_role`]), is checked as an issue's is.
    pub async fn checkout_add(&self, name: &str, path: &Path, base: &str, proving: &str, role: Option<&str>) -> Answer {
        self.role_here(role)?;
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
            role: role.map(Into::into),
        };
        self.submit::<AddCheckout>(&add).await?;
        let mut added = json!(add);
        added["role"] = json!(add.role);
        Ok(added)
    }

    /// `checkout edit --role`/`--no-role`: names `role`, a directory by its
    /// absolute path, as the role a worker of the checkout's issues that
    /// name none starts with from now, or, `None`, none. An issue's own role
    /// wins; a live worker keeps the directory it started in.
    pub async fn checkout_name_role(&self, name: &str, role: Option<&str>) -> Answer {
        self.role_here(role)?;
        self.submit::<NameCheckoutRole>(NameRole { name: name.into(), role: role.map(String::from) }).await?;
        Ok(json!(self.checkout(name).await?))
    }

    /// `checkout edit`: names its role (`Some`) as [`App::checkout_name_role`]
    /// does, then turns its idle work on or off (`Some`) as
    /// [`App::checkout_turn_idle`] does; answers the checkout.
    pub async fn checkout_edit(&self, name: &str, role: Option<Option<&str>>, idle: Option<bool>) -> Answer {
        let mut answer = self.checkout(name).await.map(|c| json!(c));
        if let Some(role) = role {
            answer = self.checkout_name_role(name, role).await;
        }
        if let Some(on) = idle.filter(|_| answer.is_ok()) {
            answer = self.checkout_turn_idle(name, on).await;
        }
        answer
    }

    /// `checkout edit --no-idle`/`--idle`: turns the checkout's idle work
    /// off on this host, or back on. Off, the daemon starts no idle run of
    /// it here, the `tasks_bound` run neither, and files none of its tasks;
    /// `factorium idle` still runs it. The repo's `[idle]` is left as it is.
    async fn checkout_turn_idle(&self, name: &str, on: bool) -> Answer {
        self.submit::<TurnCheckoutIdle>(TurnIdle { name: name.into(), off: !on }).await?;
        Ok(json!(self.checkout(name).await?))
    }

    pub async fn checkout_remove(&self, name: &str) -> Answer {
        self.submit::<RemoveCheckout>(Named { name: name.into() }).await?;
        Ok(json!({ "name": name, "removed": true }))
    }

    pub async fn checkout_list(&self) -> Answer {
        Ok(shown(&self.rows(tracker::listed()).await?))
    }

    /// The workers, live ones unless `all`: each with the resources it
    /// holds (since, until it declared) and those it waits for.
    pub async fn workers(&self, all: bool) -> Answer {
        let mut v = shown(&self.rows(tracker::workers(None, all)).await?);
        let (q, order) = (self.state::<heavy::Heavy>(heavy::queue_id()).await?, self.settings().await?.order());
        let resources = q.resources(self.now(), order);
        for row in v.as_array_mut().into_iter().flatten() {
            let me = row["worker"].as_str().and_then(|w| w.parse::<Uuid>().ok());
            let mine = |hs: &[heavy::Holding]| hs.iter().any(|h| me.is_some() && h.worker == me);
            let holds: Vec<_> = resources.iter().filter(|r| mine(&r.holders)).collect();
            let waits: Vec<_> = resources.iter().filter(|r| mine(&r.waiting)).collect();
            (row["holds"], row["waits_for"]) = (json!(holds), json!(waits));
        }
        Ok(v)
    }

    // ---- the kit and the accounts ----

    /// The kit: its pins. Each engine's command is stablehand's
    /// (`stablehand command show`).
    pub async fn kit(&self) -> Answer {
        let kit = self.state::<Kits>(kit_id()).await?;
        Ok(json!({ "pins": kit.pins }))
    }

    /// Pins a checkout to one harness of the kit, or unpins it.
    pub async fn kit_pin(&self, checkout: &str, harness: Option<&str>) -> Answer {
        self.checkout(checkout).await?;
        harness.map(known).transpose()?;
        self.submit::<PinCheckout>(Pin { checkout: checkout.into(), harness: harness.map(Into::into) }).await?;
        self.kit().await
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

    /// Declares the host's named resource `name` (its slots and note), or with `remove` takes it
    /// away; answers every resource declared. With neither, it changes nothing.
    pub async fn resource_declare(&self, name: &str, declared: Option<Resource>, remove: bool) -> Answer {
        if declared.is_some() || remove {
            self.submit::<DeclareResource>(Declare { name: name.into(), resource: declared }).await?;
        }
        Ok(json!({ "resources": self.settings().await?.resources }))
    }

    /// Holds a checkout's landings, or with `None` releases them.
    pub async fn hold(&self, checkout: &str, why: Option<&str>) -> Answer {
        self.checkout(checkout).await?;
        self.submit::<HoldLandings>(Hold { checkout: checkout.into(), why: why.map(Into::into) }).await?;
        let l = self.state::<Lines>(line_id(checkout)).await?;
        Ok(json!({ "checkout": checkout, "held": l.held }))
    }

    /// `factorium unqueue`: takes entry `id` off the heavy queue, waiting or
    /// running. A running one's session is ended here, TERM then KILL
    /// (`ended`); one not yet noted, its waiter ends on seeing it gone.
    pub async fn unqueue(&self, id: Uuid) -> Answer {
        let q = self.state::<heavy::Heavy>(heavy::queue_id()).await?;
        let Some(entry) = q.entries.into_iter().find(|e| e.id == id) else {
            return Err(Refusal::new("no_entry", format!("the heavy queue holds no entry {id}")));
        };
        self.submit::<heavy::EndHeavy>(heavy::End { id, keeps: false, gone: None, outcome: None }).await?;
        if let Some(session) = entry.session {
            let end = tokio::task::spawn_blocking(move || factorium_host::end(session));
            end.await.map_err(|e| Refusal::new("internal", e.to_string()))?;
        }
        Ok(json!({ "unqueued": entry, "ended": entry.session.is_some() }))
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
