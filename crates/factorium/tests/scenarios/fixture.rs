//! The one fixture every scenario uses. It drives `App` as the CLI does and
//! reads only what the CLI would print (SPEC.md §6). Git is real — a bare
//! remote and a clone per repo, copied from one made per run. The readings
//! and the processes are stand-ins the scenario moves; orca, brew and
//! mytokens are programs whose answers it gives, so what the app makes of
//! them runs as for real. Time moves when the scenario moves it.
//!
//! What a scenario does is a step here, and what it reads a reader here,
//! each written once: a scenario file holds its tests and imports from no
//! other scenario file.

mod db;
mod fakes;
mod heavy;
mod kept_trace;
mod line_trace;
mod read;
mod sessions;
mod steps;
mod trace;
mod world;

pub use fakes::*;
pub use heavy::*;
pub use kept_trace::*;
pub use read::*;
pub use sessions::*;
pub use steps::*;
pub use world::*;
