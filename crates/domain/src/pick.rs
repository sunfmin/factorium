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
    #[serde(default)]
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
    format!(
        "{goal}\n\n# The diff\n\n```diff\n{diff}\n```\n\n# The candidates\n\n```json\n{listing}\n```\n\n\
         # How to answer\n\nAnswer one JSON object and nothing else: \
         {{\"picks\": [{{\"name\": \"<a candidate's exact name>\", \"why\": \"<one line>\"}}]}} with at most {MOST} \
         picks, or {{\"none\": \"<one line of why no mutant is worth running>\"}}.\n"
    )
}

/// The candidates' names in slimfit's `listing` (`{"candidates": {suite:
/// [{"name", …}]}}`).
fn names(listing: &Value) -> Vec<&str> {
    let suites = listing["candidates"].as_object().into_iter().flat_map(|s| s.values());
    suites.filter_map(Value::as_array).flatten().filter_map(|c| c["name"].as_str()).collect()
}

/// The picker's answer `text` taken against slimfit's `listing`: its one
/// JSON object (fenced or not), picks of listed names only, each why one
/// line; or why it is not taken.
pub fn read(text: &str, listing: &Value) -> Result<Picked, String> {
    let (from, to) = (text.find('{'), text.rfind('}'));
    let object = match (from, to) {
        (Some(from), Some(to)) if from < to => &text[from..=to],
        _ => return Err(format!("the picker answered no JSON object: {text}")),
    };
    let said: Said =
        serde_json::from_str(object).map_err(|e| format!("the picker's answer is not as asked ({e}): {text}"))?;
    let one_line = |why: &str| !why.trim().is_empty() && !why.contains('\n');
    let listed = names(listing);
    match (said.picks.is_empty(), said.none) {
        (true, Some(why)) if one_line(&why) => Ok(Picked::None(why.trim().into())),
        (false, None) if said.picks.len() <= MOST => {
            let mut picks = Vec::new();
            for p in said.picks {
                if !listed.contains(&p.name.as_str()) {
                    return Err(format!("the picker picked `{}`, which slimfit does not list", p.name));
                }
                if !one_line(&p.why) {
                    return Err(format!("the picker gave `{}` no one line of why", p.name));
                }
                picks.push((p.name, p.why.trim().to_string()));
            }
            Ok(Picked::Picks(picks))
        }
        _ => Err(format!("the picker answered neither 1 to {MOST} picks nor one line of why none: {text}")),
    }
}

/// Who picked and made the change, and what picking took.
#[derive(Debug, Clone)]
pub struct Taken<'a> {
    /// The picker's harness, session and model, as `slimfit ratchet --picker` takes it.
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
    /// The pickers that failed before it, in turn: harness and why.
    pub fell_back: &'a [(&'a str, String)],
}

/// The worker's answer — what was picked, by whom, what it took, and the
/// commands that run the picks and record them, each word put in by
/// `quote` — and the line its issue keeps of what picking took.
pub fn answer(picked: &Picked, t: &Taken, quote: fn(&str) -> String) -> (Value, String) {
    let picker = format!("{}:{}:{}", t.harness, t.session, t.model);
    let ids = format!("--picker {} --author {}", quote(&picker), quote(&t.author));
    let ratchet = format!("factorium worker exclusive -- slimfit ratchet {ids}");
    let mut said = json!({ "picker": picker, "author": t.author, "harness": t.harness, "model": t.model,
        "account": t.account, "took_ms": t.took_ms, "tokens": { "in": t.tokens_in, "out": t.tokens_out } });
    let failed: Vec<Value> = t.fell_back.iter().map(|(harness, why)| json!({ "harness": harness, "why": why })).collect();
    said["fell_back"] = failed.into();
    let what = match picked {
        Picked::Picks(picks) => {
            let names: Vec<String> = picks.iter().map(|(n, _)| quote(n)).collect();
            let whys: Vec<String> = picks.iter().map(|(_, w)| quote(w)).collect();
            let (names, whys) = (names.join(" "), whys.join(" "));
            let run =
                format!("factorium worker exclusive --background -- slimfit check --mutants {names} --why {whys}");
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
    let on = t.account.unwrap_or("the host's own login");
    let secs = t.took_ms as f64 / 1000.0;
    let mut cost =
        format!("pick-mutants: {picker} on {on}, {secs:.1}s, {} tokens in, {} out; {what}", t.tokens_in, t.tokens_out);
    for (harness, why) in t.fell_back {
        cost.push_str(&format!("; the {harness} picker failed first, so {} picked: {why}", t.harness));
    }
    (said, cost)
}
