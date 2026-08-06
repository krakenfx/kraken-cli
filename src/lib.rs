//! kraken-cli as a library: parse a [`Cli`], assemble an [`AppContext`], and
//! hand both to [`fn@run`]. The `kraken` binary is a thin shim over these three;
//! the MCP server reuses the same parse-and-execute path, so CLI and tool
//! behaviour cannot drift.

// Tests assert with unwrap/panic by design; the workspace lints deny them elsewhere.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub(crate) mod auth;
pub mod cli;
pub mod client;
pub(crate) mod commands;
pub mod config;
pub mod errors;
pub(crate) mod futures_paper;
pub mod logging;
pub(crate) mod mcp;
pub(crate) mod monitor;
pub mod output;
pub(crate) mod process;
pub(crate) mod record;
pub(crate) mod session;
pub(crate) mod shell;
pub(crate) mod sink;
pub(crate) mod stream;
pub(crate) mod telemetry;

pub use cli::{AppContext, Cli};
pub use commands::run;
