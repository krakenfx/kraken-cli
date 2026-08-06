//! Interactive/streaming commands: they render inline and return `Result<()>`,
//! so they fulfil [`Run`] for [`executor::dispatch`](super::executor::dispatch)
//! instead of producing a [`CommandOutput`] for the shared executor.

use std::num::NonZeroU64;

use clap::Subcommand;
use enum_dispatch::enum_dispatch;

use super::Execute;
use super::record::RecordCommand;
use super::replay::ReplayCommand;
use super::streamd::StreamdCommand;
use super::ws::WsCommand;
use crate::cli::AppContext;
use crate::errors::{KrakenError, Result};
use crate::output::{CommandOutput, render};

/// Streaming execution: render inline, return no structured output.
/// `#[enum_dispatch]` derives the [`StreamingCommand`]-level impl from the
/// per-variant impls below, mirroring [`Execute`] on the executor path.
#[enum_dispatch]
pub(super) trait Run {
    async fn run(self, ctx: &AppContext) -> Result<()>;
}

// One value exists per process, parsed once at startup, so the size gap
// between `Ws` and the argless variants costs nothing worth boxing for.
//
// `enum_dispatch` requires every variant to be a newtype over a [`Run`] type,
// so the argless commands (setup, shell) wrap empty Args structs.
#[allow(clippy::large_enum_variant)]
#[enum_dispatch(Run)]
#[derive(Subcommand)]
pub(crate) enum StreamingCommand {
    /// WebSocket streaming commands.
    Ws(Ws),
    /// Long-lived multi-channel, multi-symbol streaming daemon.
    #[command(subcommand)]
    Streamd(StreamdCommand),
    /// Capture live market data (trades/book/ohlc) into a local tape.
    Record(RecordCommand),
    /// Replay a recorded session's timeline to stdout at a chosen speed.
    Replay(ReplayCommand),
    /// Guided first-time setup wizard.
    Setup(Setup),
    /// Interactive REPL shell.
    Shell(Shell),
    /// Start a built-in MCP (Model Context Protocol) server over stdio.
    ///
    /// Default exposes read-only services (market, account, paper, workspace).
    /// Use `-s all` to include trade, funding, futures, earn, subaccount, and auth.
    Mcp(Mcp),
}

#[derive(Debug, clap::Args)]
pub(crate) struct Ws {
    #[command(subcommand)]
    pub(crate) cmd: WsCommand,
    /// Client request ID echoed back for reply correlation, > 0. When omitted,
    /// subscriptions send no ID and one-shot methods default to 1.
    #[arg(long, global = true)]
    pub(crate) req_id: Option<NonZeroU64>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Setup {}

#[derive(Debug, clap::Args)]
pub(crate) struct Shell {}

#[derive(Debug, clap::Args)]
pub(crate) struct Mcp {
    /// Comma-separated service groups, or "all". Default: market,account,paper,workspace,feedback.
    #[arg(short, long, default_value = "market,account,paper,workspace,feedback")]
    services: String,

    /// Skip per-call confirmation for dangerous tools. Use only when the
    /// calling agent is trusted and has been validated through paper trading.
    #[arg(long)]
    allow_dangerous: bool,
}

impl Run for Ws {
    async fn run(self, ctx: &AppContext) -> Result<()> {
        super::ws::execute(self.cmd, self.req_id, ctx).await
    }
}

impl Run for StreamdCommand {
    async fn run(self, ctx: &AppContext) -> Result<()> {
        super::streamd::execute(&self, ctx).await
    }
}

impl Run for RecordCommand {
    async fn run(self, ctx: &AppContext) -> Result<()> {
        super::record::execute(&self, ctx).await
    }
}

impl Run for ReplayCommand {
    async fn run(self, ctx: &AppContext) -> Result<()> {
        super::replay::execute(&self, ctx).await
    }
}

impl Run for Setup {
    async fn run(self, ctx: &AppContext) -> Result<()> {
        // Setup produces structured output like an executor-path command, but
        // it must stay on this inline path: the wizard prompts interactively
        // for API credentials, and living here keeps it behind the streaming
        // rejection so the render-free MCP executor can never reach a hidden
        // prompt. Renders exactly as the executor arm would.
        let out = super::setup::run().await?;
        render(ctx.format, &out);
        Ok(())
    }
}

impl Run for Shell {
    async fn run(self, ctx: &AppContext) -> Result<()> {
        crate::shell::run(ctx).await
    }
}

impl Run for Mcp {
    async fn run(self, ctx: &AppContext) -> Result<()> {
        crate::mcp::server::run_server(ctx, &self.services, self.allow_dangerous).await
    }
}

impl Execute for StreamingCommand {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        // `dispatch` handles streaming inline, so only the render-free executor
        // (the MCP path) can reach this rejection.
        Err(streaming_unsupported())
    }
}

/// Streaming/interactive commands render inline and return `Result<()>`; they
/// have no `CommandOutput` to hand the shared executor (CLI or MCP).
pub(super) fn streaming_unsupported() -> KrakenError {
    KrakenError::Validation("This command cannot be executed through the shared executor".into())
}
