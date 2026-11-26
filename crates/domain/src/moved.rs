//! What `issue move` says when it stops part way (`issue move --help`): the step
//! that failed, that the issue stays open here, and the steps done before
//! it, so that a person knows what to undo or finish by hand.

/// The refusal of a move of `issue` (`web#1`) that stopped at a step which
/// said `said`, after the steps `done` (none: `nothing`).
pub fn stopped(said: &str, issue: &str, done: &[String]) -> String {
    let done = if done.is_empty() { "nothing".to_string() } else { done.join("; ") };
    format!("{said}. The move stopped there, {issue} open here; done: {done}")
}
