//! factorium's pure half, on factum: the host's checkouts, issues, workers,
//! proving lines, watch, kit and settings; the commands that change them;
//! the tracker the CLI reads; and the rules that decide from readings —
//! admission, the heavy-command queue, workers' health, and doctor's
//! findings.
//! Nothing here reads a clock or touches the host.

pub mod admission;
pub mod bump;
pub mod checkout;
pub mod daemon;
pub mod decided;
pub mod declared;
pub mod doctor;
pub mod gate;
pub mod health;
pub mod heavy;
pub mod idle;
pub mod issue;
pub mod kit;
pub mod line;
pub mod neutral;
pub mod outage;
pub mod outbox;
pub mod pick;
pub mod review;
pub mod settings;
pub mod tracker;
pub mod watch;
pub mod when;
pub mod worker;

use factum_core::Uuid;

/// The namespace of every id factorium derives.
const IDS: Uuid = Uuid::from_u128(0x6f1c_2a5e_9d3b_4c7a_8e21_5b6d_0f9a_3c47);

/// An id derived from `what`: the same words, the same id.
pub fn id(what: &str) -> Uuid {
    Uuid::new_v5(&IDS, what.as_bytes())
}

/// The id of the stream of the checkout named `name`.
pub fn checkout_id(name: &str) -> Uuid {
    id(&format!("checkout:{name}"))
}

/// The id of the stream of issue `number` of the checkout named `checkout`.
pub fn issue_id(checkout: &str, number: u64) -> Uuid {
    id(&format!("issue:{checkout}#{number}"))
}

/// The id of attempt `attempt` at issue `number` of `checkout`.
pub fn worker_id(checkout: &str, number: u64, attempt: u32) -> Uuid {
    id(&format!("worker:{checkout}#{number}/{attempt}"))
}

/// The id of the proving line of `checkout`: its landings, promotions, cuts.
pub fn line_id(checkout: &str) -> Uuid {
    id(&format!("line:{checkout}"))
}

/// Defines a command whose stream is `$stream` of its params.
#[macro_export]
macro_rules! command {
    ($(#[$m:meta])* $command:ident, $name:literal, $agg:ty, $params:ty, |$p0:ident| $stream:expr,
     |$s:ident, $p:ident, $now:ident| $decide:expr) => {
        $(#[$m])*
        pub struct $command;

        impl factum_core::Command for $command {
            const NAME: &'static str = $name;
            type Aggregate = $agg;
            type Params = $params;

            fn stream($p0: &$params) -> factum_core::Uuid {
                $stream
            }

            #[allow(unused_variables)]
            fn decide(
                $s: &<$agg as factum_core::Aggregate>::State,
                $p: &$params,
                _: &factum_core::By,
                $now: factum_core::Instant,
            ) -> Result<
                factum_core::Decision<<$agg as factum_core::Aggregate>::Event>,
                factum_core::Rejection,
            > {
                let decided = || -> Result<Vec<<$agg as factum_core::Aggregate>::Event>, factum_core::Rejection> { $decide };
                decided().map(factum_core::Decision::from)
            }
        }
    };
}

/// `event` alone if `yes`, else none: a command that may find nothing to do.
pub fn only<E>(yes: bool, event: E) -> Result<Vec<E>, factum_core::Rejection> {
    Ok(if yes { vec![event] } else { Vec::new() })
}

/// A refusal.
pub fn no<T>(code: &str, message: impl Into<String>) -> Result<T, factum_core::Rejection> {
    Err(factum_core::Rejection::new(code, message))
}

/// The name serde gives `v`, a unit variant: the one spelling of a status
/// or a kind, in rows, answers and words alike.
pub fn named(v: impl serde::Serialize) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}
