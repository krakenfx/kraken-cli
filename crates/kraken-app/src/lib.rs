//! The engine every kraken front-end is a thin adapter over.
//!
//! It owns the runtime context, the gates, and the work behind every command in
//! [`kraken_contract`]. A front-end's whole job is to turn its own input into a
//! [`Cli`]/`Command`, hand it to [`dispatch`] or [`execute_command`], and
//! render what comes back — so `kraken`, `kraken-shell`, and `kraken-mcp`
//! cannot drift in what a command means.
//!
//! It owns no front-end and cannot start one: `kraken`, `kraken-shell`, and
//! `kraken-mcp` are separate products over this crate, and nothing in the
//! command tree reaches back out to them.

// Tests assert with unwrap/panic by design; the workspace lints deny them elsewhere.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod cli;
pub(crate) mod commands;
pub mod config;
pub mod errors;
pub(crate) mod futures_paper;
pub mod logging;
pub(crate) mod monitor;
pub mod output;
pub(crate) mod process;
pub(crate) mod record;
pub(crate) mod session;
pub(crate) mod sink;
pub(crate) mod stream;
pub mod telemetry;
pub(crate) mod url_policy;
pub(crate) mod venue;
pub(crate) mod ws_token;

pub use cli::{
    AppContext, Cli, ConnectionArgs, GlobalArgs, InteractiveArgs, LineFlags, MonitorArgs,
};
// The three entry points a front-end needs, and the one read it needs to
// announce which account it is addressing. Everything else stays internal:
// this list is the whole contract between the engine and its adapters.
pub use commands::{active_manifest, dispatch, execute_command, run};
