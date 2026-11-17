//! What factorium knows of each harness it starts sessions in — Claude Code,
//! pi, Codex, DSH, Kimi Code and Grok: how each starts, where it reads its
//! rules and skills, how a hook reaches it, where it keeps its session and
//! which lines start or end a turn, what its usage lines count, what it says
//! of its account's usage windows and limits, which screen words mean a quota
//! or a refusal, and the engine accounts mytokens holds. Whoever starts a
//! session says what it is started with (`Start`); nothing here knows why.

pub mod account;
pub mod brief;
mod harness;
pub mod line;
pub mod start;
pub mod status;
pub mod steps;
pub mod tokens;
pub mod turn;
pub mod window;

pub use harness::*;
pub use start::{DshProfile, Rules, Start, Told};
