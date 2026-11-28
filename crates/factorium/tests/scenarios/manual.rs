//! The manual: every `--help` is agent-help's, assembled from the command
//! line's doc comments, and nothing in it names what is not there.

// The command line itself, as the binary compiles it: its `answer`s are
// the binary's to run, not this test's.
#[allow(dead_code)]
#[path = "../../src/cli.rs"]
mod cli;

use clap::CommandFactory;

#[test]
fn the_manual_is_whole() {
    cli::Cli::command().debug_assert();
    let manual = cli::manual();
    agent_help::check(&manual).unwrap();
    let all = manual.manual().unwrap();
    assert!(!all.contains("{const:"), "{all}");
}

/// A group's `--help` (`factorium issue --help`) lists every visible
/// subcommand of it with its summary, as the top level's lists its
/// commands, so none is found only by probing.
#[test]
fn every_group_s_help_lists_its_subcommands() {
    type Group = (Vec<String>, Vec<String>);
    fn groups(command: &clap::Command, path: &mut Vec<String>, found: &mut Vec<Group>) {
        let subs: Vec<_> = command.get_subcommands().filter(|c| !c.is_hide_set()).collect();
        if !path.is_empty() && !subs.is_empty() {
            found.push((path.clone(), subs.iter().map(|c| c.get_name().to_owned()).collect()));
        }
        for sub in subs {
            path.push(sub.get_name().to_owned());
            groups(sub, path, found);
            path.pop();
        }
    }
    let mut found = Vec::new();
    groups(&cli::Cli::command(), &mut Vec::new(), &mut found);
    assert!(found.iter().any(|(path, _)| path == &["issue"]), "{found:?}");
    let manual = cli::manual();
    for (path, subs) in found {
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        let help = manual.command_help(&path).unwrap();
        let group = format!("factorium {}", path.join(" "));
        for sub in subs {
            let entry = format!("- `{group} {sub}`: ");
            assert!(help.contains(&entry), "`{group} --help` lacks {entry:?}:\n{help}");
        }
    }
}
