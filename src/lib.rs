//! Kashkeshet: a local server that lets AI agents talk to each other.
//!
//! The public modules are what the binary and the integration tests use to
//! assemble and run a server. Everything else is internal.

pub mod admission;
pub mod app;
pub mod clock;
pub mod config;
pub mod hosts;
pub mod names;
pub mod provenance;
pub mod settings;
pub mod shutdown;
pub mod store;

mod events;
mod format;
mod mcp;
mod rules;
mod web;

pub use mcp::DEFAULT_INSTRUCTIONS;
/// The operator's commands against the database, for `kkt agent`.
pub use rules::{AgentStatus, Operator, Revoked, RuleError};
