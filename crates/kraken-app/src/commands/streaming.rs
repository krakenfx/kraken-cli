//! Interactive/streaming commands: they emit as they go, so their [`Execute`]
//! impls return [`CommandOutput::inline`] — the receipt of output already
//! written — and answer [`Execute::renders_inline`] with `true` so the
//! render-free MCP path refuses them before they start streaming.

use super::Execute;
use crate::{
    cli::AppContext,
    errors::{KrakenError, Result},
    output::{CommandOutput, render},
};
use kraken_contract::{
    record::RecordCommand,
    replay::ReplayCommand,
    streamd::StreamdCommand,
    streaming::{Setup, StreamingCommand, Ws},
};

/// Every leaf here streams or prompts and writes its own stdout, so each
/// returns the inline receipt and declares itself inline. The bodies are one
/// call each: the work lives in the per-command modules.
impl Execute for Ws {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        super::ws::execute(self.cmd, self.req_id, ctx).await?;
        Ok(CommandOutput::inline())
    }

    fn renders_inline(&self) -> bool {
        true
    }
}

impl Execute for StreamdCommand {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        super::streamd::execute(&self, ctx).await?;
        Ok(CommandOutput::inline())
    }

    fn renders_inline(&self) -> bool {
        true
    }
}

impl Execute for RecordCommand {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        super::record::execute(&self, ctx).await?;
        Ok(CommandOutput::inline())
    }

    fn renders_inline(&self) -> bool {
        true
    }
}

impl Execute for ReplayCommand {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        super::replay::execute(&self, ctx).await?;
        Ok(CommandOutput::inline())
    }

    fn renders_inline(&self) -> bool {
        true
    }
}

impl Execute for Setup {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        // Setup produces structured output like any executor-path command, but
        // it renders here and stays inline deliberately: the wizard prompts
        // interactively for API credentials, so declaring it inline keeps it
        // behind the MCP rejection where no hidden prompt can be reached.
        let out = super::setup::run().await?;
        render(ctx.format, &out);
        Ok(CommandOutput::inline())
    }

    fn renders_inline(&self) -> bool {
        true
    }
}

/// The refusal the render-free MCP path returns for an inline command: it
/// streams to stdout and has no structured payload a tool call could carry.
pub(super) fn streaming_unsupported() -> KrakenError {
    KrakenError::Validation("This command cannot be executed through the shared executor".into())
}

delegate_execute!(StreamingCommand {
    Ws,
    Streamd,
    Record,
    Replay,
    Setup
});
