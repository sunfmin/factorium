//! The neutral parts: rules any program that runs agents on one host can
//! use. Guild depends on them, as on `factorium-harness` and
//! `factorium-host`, as git dependencies pinned to a revision. What only
//! the caller knows it passes in, or says through a trait it implements;
//! factorium's own modules wrap them. A test from outside
//! (`crates/factorium/tests/neutral.rs`) holds them to that.

pub mod admission;
pub mod hot;
pub mod landing;
pub mod queue;
