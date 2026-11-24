//! The machine a session starts on, by the name an engine account's bound
//! login names it (`stablehand pick --machine`): the accounts themselves are
//! stablehand's, their one home.

/// This machine's name, as `hostname -s` says it: what a bound login's
/// machine names. The system's name up to its first `.`.
pub fn this_machine() -> Result<String, String> {
    let system = gethostname::gethostname();
    let system = system.to_string_lossy();
    let name = short_name(&system);
    if name.is_empty() {
        return Err(format!("the system names no machine ({system:?})"));
    }
    Ok(name.to_string())
}

/// `name` as `hostname -s` prints it: trimmed, and cut at its first `.`.
fn short_name(name: &str) -> &str {
    let name = name.trim();
    name.split_once('.').map_or(name, |(short, _)| short)
}
