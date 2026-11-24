//! The engine accounts, as stablehand keeps them (sunfmin/stablehand, its
//! docs/adr/0001): their one home — which there are, how each logs in, and
//! the limits each met. factorium asks it which account one more session
//! starts on (`stablehand pick`, told how many sessions factorium runs on
//! each), what that session starts with (`stablehand env`), and tells it of
//! a limit a session's screen showed (`stablehand limit`). It keeps no
//! account of its own.

use std::collections::BTreeMap;
use std::path::PathBuf;

use factorium_domain::kit::{Picked, Unpicked};
use factorium_domain::watch::{Watches, watch_id};
use factorium_domain::worker::Workers;
use factorium_harness::Harness;
use serde_json::Value;

use crate::{Answer, App, Refusal};

/// An account as stablehand keeps it, as far as a session needs it.
pub(crate) struct Shown {
    /// Its engine: a harness's key.
    pub(crate) engine: String,
    /// Its bound login's home, where that login is; `None`: a portable login.
    pub(crate) home: Option<PathBuf>,
    /// Why no session may start on it here, if none may.
    pub(crate) unusable: Option<String>,
}

impl App {
    /// What `stablehand <args>` answered, one JSON object; refused
    /// `stablehand` with its refusal's text and code, or why it could not
    /// be run or read.
    fn stablehand(&self, args: &[&str]) -> Answer<Value> {
        let said = self.host.programs().run("stablehand", args);
        let line = format!("stablehand {}", args.join(" "));
        let out: Option<Value> = serde_json::from_str(&said.out).ok();
        match out {
            Some(v) if said.ok && v.get("error").is_none() => Ok(v),
            Some(v) if v.get("error").is_some() => {
                let (error, code) = (v["error"].as_str().unwrap_or_default(), v["code"].as_str().unwrap_or_default());
                Err(Refusal::new("stablehand", format!("{line}: {error} ({code})")))
            }
            _ => Err(Refusal::new("stablehand", format!("{line}: {}", said.err.trim()))),
        }
    }

    /// How many sessions factorium runs on each account now: its live
    /// workers' and its watch's.
    pub(crate) async fn running(&self) -> Answer<BTreeMap<String, u32>> {
        let mut on = BTreeMap::new();
        for row in self.live_workers().await? {
            let account = self.state::<Workers>(row.worker).await?.started.and_then(|st| st.account);
            *on.entry(account).or_insert(0) += 1;
        }
        let watch = self.state::<Watches>(watch_id()).await?;
        if watch.terminal.is_some() {
            *on.entry(watch.account).or_insert(0) += 1;
        }
        Ok(on.into_iter().filter_map(|(a, n)| Some((a?, n))).collect())
    }

    /// Which account of the engine `engine` one more session may start on
    /// here, `running` sessions running on each (`stablehand pick`).
    pub(crate) fn picked(&self, engine: &str, running: &BTreeMap<String, u32>) -> Result<Picked, String> {
        let machine = factorium_harness::account::this_machine()?;
        let mut args = vec!["pick", "--engine", engine, "--machine", &machine];
        let counts: Vec<String> = running.iter().map(|(a, n)| format!("{a}={n}")).collect();
        let counts = counts.join(",");
        if !counts.is_empty() {
            args.extend(["--running", &counts]);
        }
        let answer = self.stablehand(&args).map_err(|r| r.message)?;
        if let Some(account) = answer["account"].as_str() {
            return Ok(Picked::On(account.to_string()));
        }
        let text = |u: &Value, k: &str| u[k].as_str().unwrap_or_default().to_string();
        let why = answer["why"].as_array().into_iter().flatten();
        Ok(Picked::None(
            why.map(|u| Unpicked { account: text(u, "account"), reason: text(u, "reason"), why: text(u, "why") })
                .collect(),
        ))
    }

    /// The account `name` as stablehand keeps it (`stablehand account
    /// show`), and whether a session may start on it here.
    pub(crate) fn kept_account(&self, name: &str) -> Answer<Shown> {
        let shown = self.stablehand(&["account", "show", name])?;
        let a = &shown["account"];
        let engine = a["engine"].as_str().unwrap_or_default().to_string();
        let bound = a["login"] == "bound";
        let home = a["home"].as_str().filter(|_| bound).map(PathBuf::from);
        let here = factorium_harness::account::this_machine().ok();
        let machine = a["machine"].as_str();
        let unusable = if Harness::read(&engine).is_none() {
            Some(format!("its engine `{engine}` is no harness factorium starts"))
        } else if !a["off"].is_null() {
            Some(format!("it is off ({})", a["off"]))
        } else if let Some(m) = machine.filter(|m| here.as_deref().is_none_or(|h| !m.eq_ignore_ascii_case(h))) {
            Some(format!("its login is bound to `{m}`, not this machine"))
        } else if shown["logged_in"] != true {
            Some(format!("it is not logged in: `stablehand account login {name}`"))
        } else {
            None
        };
        Ok(Shown { engine, home, unusable })
    }

    /// The home of `account`'s bound login, where its sessions keep their
    /// files; `None`: a portable login, or stablehand could not say.
    pub(crate) fn account_home(&self, account: &str) -> Option<PathBuf> {
        self.kept_account(account).ok()?.home
    }

    /// What a session on `account` starts with (`stablehand env --json`),
    /// but the variable that names its home, `home`: the session's home is
    /// set where its harness starts it.
    pub(crate) fn account_env(&self, account: &str, home: Option<&std::path::Path>) -> Answer<Vec<(String, String)>> {
        let answer = self.stablehand(&["env", account, "--json"])?;
        let home = home.map(|h| h.to_string_lossy().into_owned());
        let env = answer["env"].as_object().into_iter().flatten();
        let values = env.filter_map(|(name, v)| Some((name.clone(), v.as_str()?.to_string())));
        Ok(values.filter(|(_, v)| Some(v) != home.as_ref()).collect())
    }

    /// Tells stablehand a session on `account` met its limit
    /// (`stablehand limit`): until `resets_at` (ms), else until stablehand
    /// next reads the account. `Err`: why it could not be told.
    pub(crate) fn report_limit(&self, account: &str, resets_at: Option<i64>) -> Result<(), String> {
        // Rounded up to the second, so the time named is never past.
        let at = resets_at.map(|ms| factorium_domain::when::utc(ms + 999));
        let mut args = vec!["limit", account];
        if let Some(at) = &at {
            args.extend(["--resets-at", at]);
        }
        self.stablehand(&args).map(|_| ()).map_err(|r| r.message)
    }

    /// The engine accounts as stablehand's books say now (`stablehand
    /// status`): its `accounts`.
    pub async fn accounts(&self) -> Answer {
        let mut status = self.stablehand(&["status"])?;
        Ok(status["accounts"].take())
    }
}
