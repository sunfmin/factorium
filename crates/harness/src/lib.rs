//! What factorium knows of each harness it starts sessions in — Claude Code,
//! pi, Codex, DSH, Kimi Code and Grok: how a session of each starts and is
//! handed its brief, how its hook is asked about a tool call, where it keeps
//! its session and which lines start or end a turn, which screen words mean
//! a quota or a refusal, and the engine accounts mytokens holds. Whoever
//! starts a session says what it is started with (`Start`); nothing here
//! knows why.

pub mod account;
mod brief;
mod harness;
mod line;
mod screen;
mod start;
mod turn;

pub use harness::{Call, Harness, Stop};
pub use line::{quoted, secrets_file};
pub use start::{Home, Kept, Launch, RULES, SKILLS, Start};
pub use turn::Turn;
