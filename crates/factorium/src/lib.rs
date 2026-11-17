//! factorium's one entry. Every CLI command is one method of [`App`]; the
//! binary only parses arguments into a call and prints the answer as JSON.
//! Scenarios drive `App` the same way, so what they see is what the CLI
//! prints.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use factorium_domain::checkout::{Add, AddCheckout, Checkout, Checkouts, Named, RemoveCheckout};
use factorium_domain::issue::{
    Because, Block, BlockIssue, CloseIssue, Edit, EditIssue, Issue, Issues, Open, OpenIssue,
    ParkIssue, ReadyIssue, Ref, UnblockIssue,
};
use factorium_domain::tracker::{CHECKOUTS, ISSUES, TRACKER, Tracker};
use factorium_domain::{checkout_id, issue_id};
use factum::{By, Clock, Command, Engine, Model, Op, Query, StreamId, Submission, Uuid, Value};
use factum_sqlite::SqliteStore;
use serde::Serialize;
use serde_json::json;

/// What a command answers instead of its answer: a code a script can match
/// and a sentence a person can read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

impl Refusal {
    fn new(code: &str, message: impl Into<String>) -> Refusal {
        Refusal {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl From<factum::Error> for Refusal {
    fn from(e: factum::Error) -> Refusal {
        match e {
            factum::Error::Rejected(r) => Refusal {
                code: r.code,
                message: r.message,
            },
            other => Refusal::new("internal", other.to_string()),
        }
    }
}

pub type Answer<T = Value> = Result<T, Refusal>;

/// Where the database lives when nothing says otherwise.
pub fn default_db() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    data.join("factorium").join("factorium.db")
}

fn model() -> Model {
    Model::new()
        .aggregate::<Checkouts>()
        .command::<AddCheckout>()
        .command::<RemoveCheckout>()
        .aggregate::<Issues>()
        .command::<OpenIssue>()
        .command::<EditIssue>()
        .command::<BlockIssue>()
        .command::<UnblockIssue>()
        .command::<ParkIssue>()
        .command::<ReadyIssue>()
        .command::<CloseIssue>()
        .projection(Tracker)
}

pub struct App {
    engine: Engine<SqliteStore>,
    by: By,
}

impl App {
    /// Opens the database at `db`, making it if it is not there. `by` names
    /// who runs the commands, as the events will record it.
    pub fn open(db: &Path, clock: Arc<dyn Clock>, by: &str) -> Answer<App> {
        if let Some(dir) = db.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                Refusal::new("no_database", format!("cannot make {}: {e}", dir.display()))
            })?;
        }
        let store = SqliteStore::open(db).map_err(|e| {
            Refusal::new("no_database", format!("cannot open {}: {e}", db.display()))
        })?;
        let engine = Engine::new(Arc::new(store), Arc::new(model()), clock, by);
        Ok(App {
            engine,
            by: By::Principal(by.into()),
        })
    }

    async fn submit<C: Command>(&self, params: impl Serialize) -> Answer<()> {
        let submission = Submission::new(C::NAME, params, Uuid::now_v7(), self.by.clone());
        self.engine.submit(submission).await?;
        Ok(())
    }

    async fn rows(&self, query: Query) -> Answer<Vec<Value>> {
        self.engine.settle().await?;
        Ok(self
            .engine
            .query(TRACKER, &query)
            .await?
            .into_iter()
            .map(|(_, row)| row)
            .collect())
    }

    async fn checkout(&self, name: &str) -> Answer<Checkout> {
        let state = self
            .engine
            .state(&StreamId::new("checkout", checkout_id(name)))
            .await?;
        let c: Checkout = serde_json::from_value(state).unwrap_or_default();
        if c.listed {
            Ok(c)
        } else {
            Err(Refusal::new(
                "no_checkout",
                format!("{name} is not on the list"),
            ))
        }
    }

    async fn issue(&self, checkout: &str, number: u64) -> Answer<Issue> {
        let state = self
            .engine
            .state(&StreamId::new("issue", issue_id(checkout, number)))
            .await?;
        let i: Issue = serde_json::from_value(state).unwrap_or_default();
        if i.opened() {
            Ok(i)
        } else {
            Err(Refusal::new(
                "no_issue",
                format!("{checkout}#{number} does not exist"),
            ))
        }
    }

    // ---- checkouts ----

    /// Puts a repo on the list. `base` is where promotions land, `proving`
    /// where landings land.
    pub async fn checkout_add(&self, name: &str, path: &Path, base: &str, proving: &str) -> Answer {
        let path = std::fs::canonicalize(path)
            .map_err(|e| Refusal::new("no_path", format!("{}: {e}", path.display())))?;
        let path = path.to_string_lossy().into_owned();
        let add = Add {
            name: name.into(),
            path,
            base: base.into(),
            proving: proving.into(),
        };
        self.submit::<AddCheckout>(&add).await?;
        Ok(json!(add))
    }

    pub async fn checkout_remove(&self, name: &str) -> Answer {
        self.submit::<RemoveCheckout>(Named { name: name.into() })
            .await?;
        Ok(json!({ "name": name, "removed": true }))
    }

    pub async fn checkout_list(&self) -> Answer {
        let rows = self
            .rows(
                Query::table(CHECKOUTS)
                    .filter("listed", Op::Eq, true)
                    .order_by("name", false),
            )
            .await?;
        Ok(Value::Array(
            rows.into_iter().map(without(&["listed"])).collect(),
        ))
    }

    // ---- issues ----

    /// Makes an issue numbered one past the checkout's last.
    pub async fn issue_add(&self, checkout: &str, title: &str, body: &str, parked: bool) -> Answer {
        self.checkout(checkout).await?;
        loop {
            let last = self
                .rows(
                    Query::table(ISSUES)
                        .filter("checkout", Op::Eq, checkout)
                        .order_by("number", true)
                        .limit(1),
                )
                .await?;
            let number = last.first().and_then(|r| r["number"].as_u64()).unwrap_or(0) + 1;
            let open = Open {
                checkout: checkout.into(),
                number,
                title: title.into(),
                body: body.into(),
                parked,
            };
            match self.submit::<OpenIssue>(&open).await {
                // Another command took the number first: take the next.
                Err(r) if r.code == "number_taken" => continue,
                Err(r) => return Err(r),
                Ok(()) => return self.issue_show(checkout, number).await,
            }
        }
    }

    /// The issues of a checkout, or of every checkout; open ones unless
    /// `closed` says to list the closed ones too.
    pub async fn issue_list(&self, checkout: Option<&str>, closed: bool) -> Answer {
        let mut q = Query::table(ISSUES);
        if let Some(c) = checkout {
            q = q.filter("checkout", Op::Eq, c);
        }
        if !closed {
            q = q.filter("status", Op::Ne, "closed");
        }
        let rows = self
            .rows(q.order_by("checkout", false).order_by("number", false))
            .await?;
        Ok(Value::Array(rows))
    }

    pub async fn issue_show(&self, checkout: &str, number: u64) -> Answer {
        let i = self.issue(checkout, number).await?;
        Ok(json!({
            "checkout": i.checkout,
            "number": i.number,
            "title": i.title,
            "body": i.body,
            "status": i.status.as_str(),
            "blocked_by": i.blocked_by,
            "reason": i.reason,
        }))
    }

    pub async fn issue_edit(
        &self,
        checkout: &str,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Answer {
        let edit = Edit {
            checkout: checkout.into(),
            number,
            title: title.map(Into::into),
            body: body.map(Into::into),
        };
        self.submit::<EditIssue>(edit).await?;
        self.issue_show(checkout, number).await
    }

    pub async fn issue_close(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        self.submit::<CloseIssue>(Because {
            checkout: checkout.into(),
            number,
            reason: reason.into(),
        })
        .await?;
        self.issue_show(checkout, number).await
    }

    pub async fn issue_park(&self, checkout: &str, number: u64, reason: &str) -> Answer {
        self.submit::<ParkIssue>(Because {
            checkout: checkout.into(),
            number,
            reason: reason.into(),
        })
        .await?;
        self.issue_show(checkout, number).await
    }

    pub async fn issue_ready(&self, checkout: &str, number: u64) -> Answer {
        self.submit::<ReadyIssue>(Ref {
            checkout: checkout.into(),
            number,
        })
        .await?;
        self.issue_show(checkout, number).await
    }

    /// Makes issue `number` wait for issue `by` of the same checkout, or,
    /// with `undo`, stop waiting for it.
    pub async fn issue_block(&self, checkout: &str, number: u64, by: u64, undo: bool) -> Answer {
        let block = Block {
            checkout: checkout.into(),
            number,
            by,
        };
        if undo {
            self.submit::<UnblockIssue>(block).await?;
        } else {
            self.issue(checkout, by).await?;
            self.submit::<BlockIssue>(block).await?;
        }
        self.issue_show(checkout, number).await
    }
}

fn without(fields: &'static [&'static str]) -> impl Fn(Value) -> Value {
    move |mut row| {
        if let Some(o) = row.as_object_mut() {
            for f in fields {
                o.remove(*f);
            }
        }
        row
    }
}
