//! The command tree, as a contract between the process that executes a command
//! and the process that judges one.
//!
//! Every type here carries both derives that matter: `clap` (so the CLI parses
//! straight into it) and `serde`. One declaration therefore serves the parse,
//! the `--help` text, the proposal payload, and the policy match
//! — there is no second definition to drift from.
//!
//! Deserialization is the reason this crate exists. The CLI alone only ever
//! serialized, so a `serde_json::Value` sufficed on the reading end and a
//! policy engine had to index into untyped JSON. With the tree here, both ends
//! name the same enum and an unreadable proposal fails to parse rather than
//! being half-understood — which is the fail-closed behaviour the gate already
//! promises.

pub mod account;
pub mod auth;
pub mod command;
pub mod domain_values;
pub mod earn;
pub mod error;
pub mod explain;
pub mod feedback;
pub mod funding;
pub mod futures;
pub mod futures_paper;
pub mod futures_ws;
pub mod lab;
pub mod market;
pub mod order;
pub mod paper;
pub mod playground;
pub mod plugin;
pub mod recipe;
pub mod record;
pub mod replay;
pub mod session;
pub mod streamd;
pub mod streaming;
pub mod subaccount;
pub mod subscription;
pub mod tape;
pub mod tool;
pub mod value_types;
pub mod workspace;
pub mod ws;
