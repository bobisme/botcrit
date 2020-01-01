//! crit-core — domain logic for the crit distributed code review tool.
//!
//! This crate owns event model, append-log storage, projection queries,
//! SCM abstraction, and shared domain types.

pub mod critignore;
pub mod events;
pub mod jj;
pub mod log;
pub mod projection;
pub mod scm;
pub mod version;
