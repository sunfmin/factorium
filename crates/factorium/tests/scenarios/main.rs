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
mod beside;
mod binary;
mod body;
mod bounds;
mod bump;
mod comment;
mod daemon;
mod decided;
mod doing;
mod fits;
mod fixtures;
mod harnesses;
mod heavy;
mod idle;
mod in_line;
mod issue;
mod kept;
mod kit;
mod landing_first;
mod limit;
mod line;
mod lines;
mod manual;
mod moved;
mod mutants;
mod needs;
mod next;
mod not_before;
mod on;
mod one_cut;
mod one_harness;
mod order;
mod outage;
mod outbox;
mod parked;
mod path;
mod pick;
mod picked;
mod place;
mod priority;
mod program;
mod proof;
mod queue;
mod resource;
mod review;
mod spent;
mod stacked;
mod stale;
mod stall;
mod state;
mod steps;
mod terminals;
mod tracker;
mod unqueue;
mod upgrade;
mod urgent;
mod watch;
mod work_dirs;
