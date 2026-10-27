//! MCP front-end for kraken-cli: the command tree's own tool surface
//! ([`kraken_contract::tool`]) served over rmcp stdio, plus the gates and the
//! audit trail. Execution stays behind [`ToolHost`], the seam the host binary
//! implements — this crate never dispatches a command itself. The `kraken-cli`
//! package ships all three binaries, but only its `kraken-mcp` shim calls into
//! this crate, so the `kraken` binary links no rmcp code:
//! `the_kraken_binary_links_neither_rmcp_nor_rustyline` in `kraken-cli` checks
//! the built binaries.
//!
//! Two seams, in opposite directions. [`ToolHost`] runs a command; [`Operator`]
//! asks a person whether it should be run at all, which is what a call moving
//! real money is gated on. The `acknowledged` argument is the fallback for a
//! client that cannot reach anyone, not the mechanism.
//!
//! Nothing here describes a command. A tool's name, description, arguments and
//! schema are derived from the leaf type that declares it, so a renamed field
//! reshapes the tool and the two cannot drift.
//!
//! Scope honesty: this crate's own public API does expose rmcp model types
//! ([`KrakenMcpServer::dispatch`], [`ToolRegistry::tool_definitions`]), so its
//! semver tracks rmcp's. Confinement is a property of the host's dependency
//! graph, not of this crate's signatures.

// Tests assert with unwrap/panic by design; the workspace lints deny them elsewhere.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

mod catalog;
mod error;
mod registry;
mod server;
mod services;

pub use catalog::Catalog;
pub use error::{McpError, Result};
// The runtime surface a host wires up.
pub use registry::{GateMode, ToolEntry, ToolRegistry};
pub use server::{
    ActiveWorkspace, Approval, KrakenMcpServer, Operator, ServerConfig, ToolHost, ToolOutcome,
    serve_stdio,
};
pub use services::parse_services;

mod host;
pub use host::serve;

mod cli;
pub use cli::main;
