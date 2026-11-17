//! factorium's pure half, on factum: what the host's checkouts and issues
//! are, the commands that change them, and the tracker the CLI reads.
//! Nothing here reads a clock or touches the host.

pub mod checkout;
pub mod issue;
pub mod tracker;

use factum_core::Uuid;

/// The namespace of every id factorium derives.
const IDS: Uuid = Uuid::from_u128(0x6f1c_2a5e_9d3b_4c7a_8e21_5b6d_0f9a_3c47);

/// The id of the stream of the checkout named `name`.
pub fn checkout_id(name: &str) -> Uuid {
    Uuid::new_v5(&IDS, format!("checkout:{name}").as_bytes())
}

/// The id of the stream of issue `number` of the checkout named `checkout`.
pub fn issue_id(checkout: &str, number: u64) -> Uuid {
    Uuid::new_v5(&IDS, format!("issue:{checkout}#{number}").as_bytes())
}
