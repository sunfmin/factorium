//! The engine accounts, as stablehand keeps them (sunfmin/stablehand, its
//! docs/adr/0001): their one home — which there are, how each logs in, and
//! the limits each met. factorium asks it which account one more session
//! starts on (`stablehand pick`, told how many sessions factorium runs on
//! each), what that session is handed of its login (`stablehand credential`), and tells it of
//! a limit a session's screen showed (`stablehand limit`). Each engine starts by the
//! command it keeps (`stablehand command show`). It keeps no
//! account of its own.

use std::collections::BTreeMap;
use std::path::PathBuf;

use factorium_domain::kit::{Ask, Kits, MoveCommands, Moved, Picked, Refused, Unpicked, kit_id};
use factorium_domain::watch::{Watches, watch_id};
use factorium_domain::worker::Workers;
use factorium_harness::Harness;
use serde_json::{Value, json};

use crate::{Answer, App, Refusal};

/// The command each engine starts with: the one stablehand keeps for it
/// (`kept`, `stablehand command show`), else its bare program.
pub(crate) fn engine_commands(kept: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let each = Harness::all().iter().map(|h| (h.key(), kept.get(h.key()).map_or(h.program(), String::as_str)));
    each.map(|(e, c)| (e.to_string(), c.to_string())).collect()
}

/// An account as stablehand keeps it, as far as a session needs it.
pub(crate) struct Shown {
    /// Its engine: a harness's key.
    pub(crate) engine: String,
    /// Its bound login's home, where that login is; `None`: a portable login.
    pub(crate) home: Option<PathBuf>,
    /// Why no session may start on it here, if none may.
    pub(crate) unusable: Option<String>,
}

/// What a session on an account is handed of its login.
pub(crate) struct Credential {
    /// A portable login's variables (its token), for the secrets file.
    pub(crate) env: Vec<(String, String)>,
    /// A bound login's files: where the session's home holds each, and the
    /// file in the account's home it links to.
    pub(crate) files: Vec<(String, PathBuf)>,
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

    /// What one more session may start on here, `running` sessions running
    /// on each account (`stablehand pick`), its walk narrowed by `ask`.
    pub(crate) fn picked(&self, ask: &Ask, running: &BTreeMap<String, u32>) -> Result<Picked, String> {
        let machine = factorium_harness::account::this_machine()?;
        let mut args = vec!["pick", "--machine", &machine];
        for engine in &ask.engines {
            args.extend(["--engine", engine]);
        }
        if let Some(model) = &ask.model {
            args.extend(["--model", model]);
        }
        let counts: Vec<String> = running.iter().map(|(a, n)| format!("{a}={n}")).collect();
        let counts = counts.join(",");
        if !counts.is_empty() {
            args.extend(["--running", &counts]);
        }
        let answer = match self.stablehand(&args) {
            Ok(answer) => answer,
            // An order not kept yet is stablehand's answer, not a fault: none starts.
            Err(r) if r.message.ends_with("(no_order)") => {
                return Ok(Picked::None { until: None, why: vec![Unpicked { account: None, why: r.message }] });
            }
            Err(r) => return Err(r.message),
        };
        let text = |v: &Value, k: &str| v[k].as_str().map(str::to_string);
        if let (Some(engine), Some(account)) = (text(&answer, "engine"), text(&answer, "account")) {
            return Ok(Picked::On { engine, model: text(&answer, "model"), account });
        }
        let why = answer["why"].as_array().into_iter().flatten();
        let why = why.map(|u| Unpicked { account: text(u, "account"), why: text(u, "why").unwrap_or_default() });
        Ok(Picked::None { until: text(&answer, "until"), why: why.collect() })
    }

    /// The engines' commands stablehand keeps (`stablehand command show`),
    /// only those with one kept.
    pub(crate) fn kept_commands(&self) -> Answer<BTreeMap<String, String>> {
        let shown = self.stablehand(&["command", "show"])?;
        serde_json::from_value(shown["commands"].clone())
            .map_err(|e| Refusal::new("stablehand", format!("stablehand command show: {e}")))
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

    /// The home this host keeps for sessions of `harness` on `account`'s
    /// bound login, where its login files are linked; `None`: a portable
    /// login, or stablehand could not say.
    pub(crate) fn account_home(&self, harness: Harness, account: &str) -> Option<PathBuf> {
        self.kept_account(account).ok()?.home.map(|_| self.data().kept_on(harness.key(), account))
    }

    /// What a session on `account` is handed of its login (`stablehand
    /// credential --json`): a portable login's variables, or a bound
    /// login's files, each as (where the session's home holds it, the file
    /// in the account's home).
    pub(crate) fn credential(&self, account: &str) -> Answer<Credential> {
        let answer = self.stablehand(&["credential", account, "--json"])?;
        let env = answer["env"].as_object().into_iter().flatten();
        let env = env.filter_map(|(name, v)| Some((name.clone(), v.as_str()?.to_string())));
        let files = answer["files"].as_array().into_iter().flatten();
        let files = files.filter_map(|f| Some((f["as"].as_str()?.to_string(), PathBuf::from(f["path"].as_str()?))));
        Ok(Credential { env: env.collect(), files: files.collect() })
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

impl App {
    /// Why stablehand cannot say what a session starts on, if it cannot:
    /// not on the PATH, or its commands or its pick refused or failed, as a
    /// start would ask it.
    pub(crate) async fn stablehand_seen(&self) -> Answer<Option<factorium_domain::doctor::StablehandSeen>> {
        use factorium_domain::doctor::StablehandSeen;
        if !self.host.programs().found("stablehand") {
            return Ok(Some(StablehandSeen { found: false, why: "`stablehand` is not on the PATH".into() }));
        }
        let running = self.running().await?;
        let kept = self.kept_commands().map_err(|r| r.message);
        let why = kept.and_then(|_| self.picked(&Ask::default(), &running)).err();
        Ok(why.map(|why| StablehandSeen { found: true, why }))
    }

    /// The kit's commands stablehand refused when they moved to it, of the
    /// engines it keeps no command for now (all of them, if it cannot say).
    pub(crate) fn unmoved(&self, kit: &factorium_domain::kit::Kit) -> Vec<(String, factorium_domain::kit::Refused)> {
        let kept = self.kept_commands().unwrap_or_default();
        kit.refused.iter().filter(|(e, _)| !kept.contains_key(*e)).map(|(e, r)| (e.clone(), r.clone())).collect()
    }

    /// Moves the kit's commands to stablehand, once (factorium#306): each
    /// set there (`stablehand command set`) unless it keeps one for that
    /// engine already, then dropped from the kit; one it refuses
    /// (`bad_command`) is kept for doctor. Nothing moves while stablehand
    /// cannot be asked: the next start of the daemon tries again.
    pub(crate) async fn move_commands(&self) -> Answer {
        let kit = self.state::<Kits>(kit_id()).await?;
        if kit.commands.is_empty() || !self.host.programs().found("stablehand") {
            return Ok(json!(null));
        }
        let Ok(kept) = self.kept_commands() else { return Ok(json!(null)) };
        let (mut moved, mut refused) = (Vec::new(), BTreeMap::new());
        for (engine, command) in kit.commands.iter().filter(|(e, _)| !kept.contains_key(*e)) {
            let line = format!("{engine}={command}");
            match self.stablehand(&["command", "set", &line]) {
                Ok(_) => moved.push(engine.clone()),
                Err(r) if r.message.ends_with("(bad_command)") => {
                    refused.insert(engine.clone(), Refused { command: command.clone(), why: r.message });
                }
                Err(_) => return Ok(json!(null)),
            }
        }
        self.submit::<MoveCommands>(Moved { refused: refused.clone() }).await?;
        Ok(json!({ "moved": moved, "refused": refused }))
    }
}
