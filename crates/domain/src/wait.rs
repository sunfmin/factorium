//! What `issue wait` waits for, and where each issue it names leaves it.

use serde::Serialize;

use crate::issue::{Issue, Status};

/// What a waiter waits for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Until {
    /// The issue lands, closes, or waits for a person.
    #[default]
    Any,
    /// The issue closes, landed or not: a wait for a person is waited past.
    /// A closed issue never opens again, so this is also "until it lands".
    Closed,
}

/// Where an issue leaves its waiter, the better first: a wait on several
/// issues answers the last of theirs. Each one's exit code is its
/// discriminant: 1 is a refusal's and 2 a command line's clap refused, so
/// the others start at 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Landed = 0,
    Closed = 3,
    WaitingForHuman = 4,
    Timeout = 5,
}

impl Outcome {
    /// The waiter's exit code.
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// Where `i` leaves a waiter waiting `until`: none while it waits on.
pub fn ended(i: &Issue, until: Until) -> Option<Outcome> {
    match (i.status, until) {
        (Status::Closed, _) if i.landed_as().is_some() => Some(Outcome::Landed),
        (Status::Closed, _) => Some(Outcome::Closed),
        (Status::WaitingForHuman, Until::Any) => Some(Outcome::WaitingForHuman),
        _ => None,
    }
}
