//! Entry points: [`run`] for the binary, [`dispatch`] for parsed commands, and
//! the render-free [`execute_command`] for the MCP server.
//!
//! No routing lives here. Every command — streaming included — reaches its work
//! through [`Admission::admit`], which applies the gates and then hands off to
//! the [`Execute::execute`] chain.

use super::{Admission, Command, Execute, streaming};
use crate::{
    cli::AppContext,
    errors::Result,
    output::{CommandOutput, render},
};

/// Run the parsed CLI: dispatch the subcommand, or print help when none was
/// given. The binary's single entry point into the library; keeps [`Command`]
/// and [`dispatch`] crate-internal.
pub async fn run(ctx: &AppContext, cli: crate::Cli) -> Result<()> {
    match cli.command {
        Some(command) => dispatch(ctx, command).await,
        None => {
            use clap::CommandFactory;
            crate::Cli::command().print_help()?;
            println!();
            Ok(())
        }
    }
}

/// Central dispatch: admit the command, then render what it returns. Streaming
/// commands wrote their own output as they ran and return
/// [`CommandOutput::inline`], which [`render`] prints nothing for.
pub async fn dispatch(ctx: &AppContext, command: Command) -> Result<()> {
    let out = command.admit(ctx).await?;
    render(ctx.format, &out);
    Ok(())
}

/// The MCP server's render-free entry: admits a command and returns its
/// structured output.
///
/// Inline commands are refused *before* admission: they stream to stdout for as
/// long as they run, so a tool call must never start one.
pub async fn execute_command(ctx: &AppContext, command: Command) -> Result<CommandOutput> {
    if command.renders_inline() {
        return Err(streaming::streaming_unsupported());
    }
    command.admit(ctx).await
}
