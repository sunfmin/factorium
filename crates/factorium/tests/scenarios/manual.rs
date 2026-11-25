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
    agent_help::check(&cli::manual()).unwrap();
    let all = cli::manual().manual().unwrap();
    assert!(!all.contains("{const:"), "{all}");
}
