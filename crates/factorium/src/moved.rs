//! `issue move`: one issue handed to another host's factorium, over ssh
//! (SPEC §4, §7). Nothing is shared between the hosts and nothing is read
//! from the other afterwards: the issue stops here, its work goes to a
//! branch of its own on origin, and the other host files it as its own.

use factorium_domain::checkout::Checkout;
use factorium_domain::issue::{Author, Issue, Status};
use factorium_domain::when;
use factorium_harness::quoted;
use serde_json::{Value, json};

use crate::{Answer, App, Refusal, fail};

/// The work an issue carried: its branch on origin and that branch's tip.
struct Carried {
    branch: String,
    commit: String,
}

impl App {
    /// Moves issue `number` of `checkout` to the factorium `to` reaches by
    /// ssh: checks there first, changing nothing here until that passes;
    /// then stops it here as `issue wait-for-human` does, its worktree
    /// kept; pushes what its worktree holds that origin does not to a
    /// branch of its own; files it there; and closes it here, naming the
    /// issue there. A step that fails answers what was done before it, and
    /// leaves the issue here open.
    pub async fn issue_move(&self, checkout: &str, number: u64, to: &str) -> Answer {
        let (c, issue) = (self.checkout(checkout).await?, self.issue(checkout, number).await?);
        if issue.status == Status::Closed {
            return Err(Refusal::new("closed", format!("{checkout}#{number} is closed: there is nothing to move")));
        }
        let origin = self.host.git(&c.path).origin().map_err(fail("git"))?;
        self.holds_there(to, checkout, &origin)?;
        let here = factorium_harness::account::this_machine().unwrap_or_else(|_| "this host".into());
        let from = format!("{here} {checkout}#{number}");
        let mut done = Vec::new();
        let partly = |done: &[String], r: Refusal| {
            let done = if done.is_empty() { "nothing".to_string() } else { done.join("; ") };
            Refusal::new(
                &r.code,
                format!("{}. The move stopped there, {checkout}#{number} open here; done: {done}", r.message),
            )
        };

        let why = format!("it moves to {to}");
        self.issue_wait_for_human(checkout, number, &why).await.map_err(|r| partly(&done, r))?;
        done.push("its worker ended, its worktree kept, and it waits for a person here".to_string());

        let carried = self.carry_work(&c, &issue, to).await.map_err(|r| partly(&done, r))?;
        if let Some(k) = &carried {
            done.push(format!("its work pushed to origin's `{}` at {}", k.branch, k.commit));
        }

        let body = moved_body(&issue, &from, carried.as_ref(), self.now());
        let there = self.file_there(to, checkout, &issue.title, &body).map_err(|r| partly(&done, r))?;
        let named = format!("{to} {checkout}#{there}");
        done.push(format!("filed there as {named}"));

        let work = carried
            .as_ref()
            .map_or(String::new(), |k| format!(", its work on origin's `{}` at {}", k.branch, k.commit));
        let text = format!("Moved to `{named}`{work}: it goes on there.");
        self.note(checkout, number, Author::Factorium, &text).await.map_err(|r| partly(&done, r))?;
        let shown = self.close(checkout, number, &format!("moved to {named}"), true).await;
        let shown = shown.map_err(|r| partly(&done, r))?;
        Ok(json!({
            "moved_to": named,
            "host": to,
            "number": there,
            "branch": carried.as_ref().map(|k| &k.branch),
            "commit": carried.as_ref().map(|k| &k.commit),
            "issue": shown,
        }))
    }

    /// Refused unless the factorium `to` reaches lists a checkout named
    /// `checkout` whose origin is `origin`.
    fn holds_there(&self, to: &str, checkout: &str, origin: &str) -> Answer<()> {
        let listed = answer_of(&self.ssh(to, "factorium checkout list")?, to)?;
        let names: Vec<&str> = listed.as_array().into_iter().flatten().filter_map(|c| c["name"].as_str()).collect();
        let Some(path) = listed.as_array().into_iter().flatten().find(|c| c["name"] == checkout) else {
            let names = if names.is_empty() { "none".to_string() } else { names.join(", ") };
            let why = format!("{to}'s factorium has no checkout {checkout} (it lists: {names})");
            return Err(Refusal::new("no_checkout_there", why));
        };
        let path = path["path"].as_str().unwrap_or_default();
        let theirs = self.ssh(to, &format!("git -C {} remote get-url origin", quoted(path)))?;
        if same_origin(theirs.trim(), origin) {
            return Ok(());
        }
        let why = format!("{to}'s {checkout} ({path}) has origin {}, this one's is {origin}", theirs.trim());
        Err(Refusal::new("other_origin", why))
    }

    /// Pushes what `issue`'s worktree holds that origin does not, its
    /// changes not committed as one commit on top, to its branch of its
    /// own on origin; `None`: no worktree holds any.
    async fn carry_work(&self, c: &Checkout, issue: &Issue, to: &str) -> Answer<Option<Carried>> {
        let Some(st) = self.kept_now(c, issue).await? else { return Ok(None) };
        let tree = self.host.git(&st.worktree);
        let message = format!("Work in progress of {}#{}, not gated: moved to {to}", issue.checkout, issue.number);
        let commit = tree.snapshot(&message).map_err(fail("git"))?;
        let branch = format!("factorium/move/{}-{}", issue.checkout, issue.number);
        let was = tree.origin_tip(&branch).map_err(fail("git"))?;
        tree.push_requiring(&commit, &branch, was.as_deref()).map_err(fail("git"))?;
        Ok(Some(Carried { branch, commit }))
    }

    /// Files an issue of `checkout` on the factorium `to` reaches; answers its number there.
    fn file_there(&self, to: &str, checkout: &str, title: &str, body: &str) -> Answer<u64> {
        let line = format!("factorium issue add {} {} --body={}", quoted(checkout), quoted(title), quoted(body));
        let filed = answer_of(&self.ssh(to, &line)?, to)?;
        let why = || Refusal::new("bad_answer", format!("{to}'s factorium answered no issue number: {filed}"));
        filed["number"].as_u64().ok_or_else(why)
    }

    /// What `line` printed, run on `to` over ssh in its user's login shell,
    /// so that its PATH is the one a person there has (a shell that is not
    /// a login one finds no `factorium` where Homebrew is set up by the
    /// profile).
    fn ssh(&self, to: &str, line: &str) -> Answer<String> {
        if to.starts_with('-') || to.trim().is_empty() {
            return Err(Refusal::new("bad_target", format!("`{to}` names no ssh target")));
        }
        let remote = format!("exec \"$SHELL\" -lc {}", quoted(line));
        let said = self.host.programs().run("ssh", &["-o", "BatchMode=yes", to, &remote]);
        if said.ok {
            return Ok(said.out);
        }
        let printed = [said.err.trim(), said.out.trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>();
        let command = line.split(" --body=").next().unwrap_or(line);
        Err(Refusal::new("ssh", format!("`{command}` on {to} failed: {}", printed.join("; "))))
    }
}

/// The JSON `to`'s factorium answered in `out`.
fn answer_of(out: &str, to: &str) -> Answer<Value> {
    serde_json::from_str(out)
        .map_err(|e| Refusal::new("bad_answer", format!("{to}'s factorium answered no JSON ({e}): {out}")))
}

/// Whether two origin URLs name one repo: the same host and path, whatever
/// the scheme and user (`https://`, `ssh://git@`, scp-like `git@host:path`,
/// as a host's `insteadOf` may rewrite one into another), and with or
/// without a trailing `/` or `.git`.
fn same_origin(a: &str, b: &str) -> bool {
    repo_of(a) == repo_of(b)
}

/// The host (lowercased, its user and port taken off) and path an origin
/// URL names; a local path's host is empty.
fn repo_of(url: &str) -> (String, String) {
    let url = url.trim();
    let (host, path) = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p)),
        // scp-like `[user@]host:path`: a colon before any slash.
        None => match url.split_once(':') {
            Some((h, p)) if !h.contains('/') => (h, p),
            _ => ("", url),
        },
    };
    let host = host.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default().to_lowercase();
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_start_matches('/');
    (host, path.to_string())
}

/// The body the issue is filed with there: its body here, then where it
/// came from and the work to continue from, then what of it did not travel
/// (its role, account and blockers are this host's), then its comments,
/// each with who and when, quoted so that a Decided record among them is
/// not one there.
fn moved_body(issue: &Issue, from: &str, carried: Option<&Carried>, now: i64) -> String {
    let mut b = format!(
        "{}\n\n---\n\nMoved here from `{from}` by `factorium issue move`, {}.\n",
        issue.body.trim_end(),
        when::utc(now)
    );
    match carried {
        Some(k) => {
            b += &format!(
                "\nIts work so far is on origin's branch `{}`, at {}: merge it into your branch first; its last commit is work in progress, not gated.\n",
                k.branch, k.commit
            )
        }
        None => b += "\nIt carried no work: no worktree there held any that origin lacks.\n",
    }
    let mut stayed = Vec::new();
    if let Some(role) = &issue.on.role {
        stayed.push(format!("role `{role}`"));
    }
    if let Some(account) = &issue.on.account {
        stayed.push(format!("account `{account}`"));
    }
    let waits: Vec<String> = issue.waits_for().into_iter().map(|(name, _)| name).collect();
    if !waits.is_empty() {
        stayed.push(format!("blocked by {}", waits.join(", ")));
    }
    if let Some(at) = issue.not_before {
        stayed.push(format!("not before {}", when::utc(at)));
    }
    if !stayed.is_empty() {
        b += &format!(
            "\nThere it named, none of which travelled (its paths and numbers are that host's): {}.\n",
            stayed.join("; ")
        );
    }
    if !issue.comments.is_empty() {
        b += "\n## Its comments there\n";
        for c in &issue.comments {
            let at = c.at.map_or("at a time not kept".to_string(), when::utc);
            let text: Vec<String> = c.text.lines().map(|l| format!("> {l}").trim_end().to_string()).collect();
            b += &format!("\n{}, {at}:\n\n{}\n", c.by, text.join("\n"));
        }
    }
    b
}
