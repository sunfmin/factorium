//! Scenarios: each drives `App` as the CLI does and reads only what the CLI
//! would print, through the one fixture (`fixture.rs`). Each file holds its
//! tests.

/// Runs each `async` block at once, each on a thread and runtime of its own,
/// as [`fixture::each`] does.
macro_rules! at_once {
    ($($case:expr),+ $(,)?) => {
        std::thread::scope(|s| {
            let running = [$(s.spawn(|| $crate::fixture::block_on($case))),+];
            for r in running {
                if let Err(panic) = r.join() {
                    std::panic::resume_unwind(panic);
                }
            }
        })
    };
}

mod fixture;

mod accounts;
mod across;
mod admission;
mod answers;
mod base;
mod binary;
mod bounds;
mod bump;
mod comment;
mod daemon;
mod fixtures;
mod harnesses;
mod idle;
mod kept;
mod line;
mod lines;
mod on;
mod one_harness;
mod outage;
mod outbox;
mod parked;
mod path;
mod pick;
mod priority;
mod program;
mod queue;
mod stall;
mod terminals;
mod tracker;
mod unqueue;
mod watch;
