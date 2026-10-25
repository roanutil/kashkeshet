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
pub mod web;

mod events;
mod format;
mod mcp;
mod rules;

pub use mcp::DEFAULT_INSTRUCTIONS;
