//! A worker's mutants, picked by a second agent: the one question it is
//! asked — slimfit's goal for the picker, the change's diff and its
//! candidates, as slimfit and git gave them — and what its answer must be
//! to be taken: picks by the exact names listed, each with one line of why,
//! or one line of why none is worth one. The rules of picking are slimfit's
//! (`slimfit help picker`), not restated here.

use serde::Deserialize;
use serde_json::{Value, json};

/// At most this many picks: slimfit runs no more of one change.
pub const MOST: usize = 5;

/// What the picker answered, as taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    /// Each pick's name, as listed, and why.
    Picks(Vec<(String, String)>),
    /// Why no mutant of the change is worth running.
    None(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Said {
    #[serde(default)]
    picks: Vec<Pick>,
    none: Option<String>,
}

#[derive(Deserialize)]
struct Pick {
    name: String,
    why: String,
}

/// The question the picker is asked: `goal` (slimfit's picker topic), the
/// change's `diff` and slimfit's `listing` (`check --mutants`), each as
/// given, then the one form its answer takes.
pub fn question(goal: &str, diff: &str, listing: &str) -> String {
    let how = how(&form());
    format!("{goal}\n\n# The diff\n\n```diff\n{diff}\n```\n\n# The candidates\n\n```json\n{listing}\n```\n\n{how}")
}

/// The one form the picker's answer takes.
pub fn form() -> String {
    format!(
        "{{\"picks\": [{{\"name\": \"<a candidate's exact name>\", \"why\": \"<one line>\"}}]}} with at most {MOST} \
         picks, or {{\"none\": \"<one line of why no mutant is worth running>\"}}."
    )
}

/// How a second agent answers: one JSON object of `form`.
pub fn how(form: &str) -> String {
    format!("# How to answer\n\nAnswer one JSON object and nothing else: {form}\n")
}

/// What a second agent whose answer was not taken is asked once more, in
/// the same session: only `why` not, and the one `form` its answer takes.
pub fn again(why: &str, form: &str) -> String {
    format!("Your answer was not taken: {why}.\n\n{}", how(form))
}

/// Why none is picked when slimfit lists no candidates of a change that
/// needs no record of its mutants: no picker is asked.
pub const UNLISTED: &str = "slimfit lists no candidates";

/// slimfit's `check --mutants`, as it answered (`ok`, what it printed):
/// what the picker is given — its listing, or, when it lists no candidate
/// (an empty one, or its refusal that the tool listed no mutants of the
/// change, slimfit#77), what it printed, since the gate still wants a
/// record of none when the change adds source lines (factorium#247); `None`
/// only when it lists none and its `landing` says the change adds no source
/// line and removes no test, so no record is due; else why it is not a
/// listing.
pub fn listing(ok: bool, out: &str) -> Result<Option<Value>, String> {
    let said: Value = serde_json::from_str(out).map_err(|e| format!("{e}: {out}"))?;
    let unlisted = |e: &Value| e["fault"].as_str().is_some_and(|f| f.contains("listed no mutants of the change"));
    let faults = said["error"].as_array().filter(|f| !f.is_empty());
    let due = due(&said["landing"]);
    match ok {
        true if names(&said).is_empty() && !due => Ok(None),
        true => Ok(Some(said)),
        false if faults.is_some_and(|f| f.iter().all(unlisted)) => Ok(Some(said)),
        false => Err(out.trim().to_string()),
    }
}

/// Whether slimfit's `landing` of a change, or of a range, wants a record of
/// its mutants: it adds source lines, or removes tests.
pub fn due(landing: &Value) -> bool {
    let removes = landing["removed_tests"].as_object().is_some_and(|r| r.values().any(|t| t != &json!([])));
    landing["lines"].as_u64() != Some(0) || removes
}

/// The worker's answer when slimfit lists no candidates of a change that
/// needs no record — none, why, no picker, `author` and the ratchet that
/// follows — and the line its issue keeps.
pub fn unlisted(author: &str) -> (Value, String) {
    let next = "factorium worker exclusive -- slimfit ratchet; commit .slimfit/ with the change";
    let said = json!({ "none": UNLISTED, "picker": null, "author": author, "next": next });
    (said, format!("pick-mutants: no picker asked; {UNLISTED}"))
}

/// The candidates' names in slimfit's `listing` (`{"candidates": {suite:
/// [{"name", …}]}}`).
fn names(listing: &Value) -> Vec<&str> {
    let suites = listing["candidates"].as_object().into_iter().flat_map(|s| s.values());
    suites.filter_map(Value::as_array).flatten().filter_map(|c| c["name"].as_str()).collect()
}

/// The one JSON object in the `text` a second agent answered as `role`
/// (fenced or not), from its first `{` to its last `}`, as `T`; or why
/// not, in one line, without the text.
pub fn object<T: serde::de::DeserializeOwned>(text: &str, role: &str) -> Result<T, String> {
    let (from, to) = (text.find('{'), text.rfind('}'));
    let object = from.zip(to).and_then(|(from, to)| text.get(from..=to));
    let object = object.ok_or_else(|| format!("the {role} answered no JSON object"))?;
    serde_json::from_str(object).map_err(|e| format!("the {role}'s answer is not as asked ({e})"))
}

/// The picker's answer `text` taken against slimfit's `listing`: its one
/// JSON object (fenced or not), picks of listed names only, each why one
/// line; or why it is not taken, in one line, without the text. A grouped
/// listing (`functions`) spells out only some of its candidates: a pick it
/// does not is taken when slimfit lists it in full, as `full` answers the
/// `--function` listing of the functions named (`<file>: <function>`).
pub fn read(text: &str, listing: &Value, full: impl Fn(&[String]) -> Result<Value, String>) -> Result<Picked, String> {
    let said: Said = object(text, "picker")?;
    let one_line = |why: &str| !why.trim().is_empty() && !why.contains('\n');
    let mut listed: Vec<String> = names(listing).into_iter().map(String::from).collect();
    let unspelled: Vec<&str> =
        said.picks.iter().map(|p| p.name.as_str()).filter(|n| !listed.iter().any(|l| l == n)).collect();
    let within = functions(listing, &unspelled);
    if !within.is_empty() {
        listed.extend(names(&full(&within)?).into_iter().map(String::from));
    }
    match (said.picks.is_empty(), said.none) {
        (true, Some(why)) if one_line(&why) => Ok(Picked::None(why.trim().into())),
        (false, None) if said.picks.len() <= MOST => {
            let mut picks = Vec::new();
            for p in said.picks {
                if !listed.contains(&p.name) {
                    return Err(format!("the picker picked `{}`, which slimfit does not list", p.name));
                }
                if !one_line(&p.why) {
                    return Err(format!("the picker gave `{}` no one line of why", p.name));
                }
                picks.push((p.name, p.why.trim().to_string()));
            }
            Ok(Picked::Picks(picks))
        }
        _ => Err(format!("the picker answered neither 1 to {MOST} picks nor one line of why none")),
    }
}

/// The functions of a grouped `listing` (`{"functions": {suite: [{"file",
/// "function", …}]}}`) that may hold the `picked` names, as `--function`
/// takes them: those in the file a name starts with (`<file>:<line>:…`).
fn functions(listing: &Value, picked: &[&str]) -> Vec<String> {
    let suites = listing["functions"].as_object().into_iter().flat_map(|s| s.values());
    let all = suites.filter_map(Value::as_array).flatten();
    let in_file =
        |f: &&Value| f["file"].as_str().is_some_and(|file| picked.iter().any(|n| n.starts_with(&format!("{file}:"))));
    let key = |f: &Value| Some(format!("{}: {}", f["file"].as_str()?, f["function"].as_str()?));
    all.filter(in_file).filter_map(key).collect()
}

/// What the line an issue keeps says of an answer taken on the second ask.
pub const AGAIN: &str = "answered as asked when asked again";

/// Who was asked and who made the change, and what asking took.
#[derive(Debug, Clone)]
pub struct Taken<'a> {
    /// The second agent's harness, session and model, as `slimfit ratchet
    /// --picker` (and `--reviewer`) takes it.
    pub harness: &'a str,
    pub session: &'a str,
    pub model: &'a str,
    /// The account it ran on; `None`: the host's own login.
    pub account: Option<&'a str>,
    /// The worker's id, as `--author` takes it.
    pub author: String,
    pub took_ms: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// The ones asked before it that failed, in turn: harness and why, of
    /// which the first line is kept.
    pub fell_back: &'a [(String, String)],
    /// Whether its first answer was not taken and the one it gave asked
    /// again was; what asking took is of both.
    pub asked_again: bool,
}

impl Taken<'_> {
    /// The second agent's id, `harness:session:model`.
    pub fn id(&self) -> String {
        format!("{}:{}:{}", self.harness, self.session, self.model)
    }

    /// What the worker is answered of who was asked, as its `role`
    /// (`picker`, `reviewer`), and what it took.
    pub fn said(&self, role: &str) -> Value {
        let mut said = json!({ role: self.id(), "author": self.author, "harness": self.harness, "model": self.model,
            "account": self.account, "took_ms": self.took_ms, "tokens": { "in": self.tokens_in, "out": self.tokens_out },
            "asked_again": self.asked_again });
        said["fell_back"] = self.failed().map(|(harness, why)| json!({ "harness": harness, "why": why })).collect();
        said
    }

    /// The line the issue keeps of what `command` took and found (`what`),
    /// and of each `role` that failed before the one that `did` it.
    pub fn cost(&self, command: &str, role: &str, did: &str, what: &str) -> String {
        let (on, secs) = (self.account.unwrap_or("the host's own login"), self.took_ms as f64 / 1000.0);
        let (id, tin, tout) = (self.id(), self.tokens_in, self.tokens_out);
        let mut cost = format!("{command}: {id} on {on}, {secs:.1}s, {tin} tokens in, {tout} out; {what}");
        if self.asked_again {
            cost.push_str(&format!(", {AGAIN}"));
        }
        for (harness, why) in self.failed() {
            cost.push_str(&format!("; the {harness} {role} failed first, so {} {did}: {why}", self.harness));
        }
        cost
    }

    fn failed(&self) -> impl Iterator<Item = (&String, &str)> {
        self.fell_back.iter().map(|(h, why)| (h, why.lines().next().unwrap_or_default()))
    }
}

/// The worker's answer — what was picked, by whom, what it took, and the
/// commands that run the picks and record them, each word put in by
/// `quote`, each taken against `target` when given (a range, `--target`) —
/// and the line its issue keeps of what picking took.
pub fn answer(picked: &Picked, t: &Taken, target: Option<&str>, quote: fn(&str) -> String) -> (Value, String) {
    let on = target.map_or(String::new(), |r| format!(" --target {}", quote(r)));
    let ids = format!("--picker {} --author {}", quote(&t.id()), quote(&t.author));
    let ratchet = format!("factorium worker exclusive -- slimfit ratchet{on} {ids}");
    let mut said = t.said("picker");
    let what = match picked {
        Picked::Picks(picks) => {
            let names: Vec<String> = picks.iter().map(|(n, _)| quote(n)).collect();
            let whys: Vec<String> = picks.iter().map(|(_, w)| quote(w)).collect();
            let (names, whys) = (names.join(" "), whys.join(" "));
            let run =
                format!("factorium worker exclusive --background -- slimfit check{on} --mutants {names} --why {whys}");
            said["picks"] = picks.iter().map(|(name, why)| json!({ "name": name, "why": why })).collect();
            said["next"] = format!("{run}; kill any survivor, then {ratchet}; commit .slimfit/ with the change").into();
            format!("{} picked", picks.len())
        }
        Picked::None(why) => {
            said["none"] = why.as_str().into();
            said["next"] = format!("{ratchet} --no-mutants {}; commit .slimfit/ with the change", quote(why)).into();
            "none picked".into()
        }
    };
    (said, t.cost("pick-mutants", "picker", "picked", &what))
}
