//! The neutral parts (SPEC.md §5, "For Guild"): rules any program that runs
//! agents on one host can use. What only the caller knows it passes in, or
//! says through a trait it implements; factorium's own modules wrap them.
//! A test from outside (`crates/factorium/tests/neutral.rs`) holds them to
//! that.

pub mod admission;
pub mod hot;
pub mod landing;
pub mod queue;
