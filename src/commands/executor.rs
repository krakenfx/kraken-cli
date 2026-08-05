//! Entry points: [`run`] for the binary, [`dispatch`] for parsed commands, and
//! the render-free [`execute_command`] for the MCP server.

use super::futures::FuturesCommand;
use super::session::{self, SessionCommand};
use super::streaming::Run;
use super::{Command, Execute, futures_ws};
use crate::cli::AppContext;
use crate::errors::Result;
use crate::output::{CommandOutput, render};

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

/// Central dispatch: streaming commands run inline through [`Run`]; everything
/// else executes and renders its structured output.
pub(crate) async fn dispatch(ctx: &AppContext, command: Command) -> Result<()> {
    // Before the match: the streaming arms below bypass `Execute`, and the
    // guard must cover them too (a ws order method is still a money command).
    super::workspace_guard::enforce(ctx, &command)?;
    match command {
        Command::Streaming(cmd) => cmd.run(ctx).await?,
        // `futures ws` is a streaming command, handled here alongside top-level
        // `ws` rather than through the request/response executor.
        Command::Futures(FuturesCommand::Ws(cmd)) => {
            let creds = if futures_ws::requires_auth(&cmd) {
                Some(ctx.require_credentials(crate::config::Engine::Futures)?)
            } else {
                ctx.futures_credentials.as_ref()
            };
            futures_ws::execute(
                &cmd,
                creds,
                ctx.ws_futures_url.as_deref(),
                ctx.accept_invalid_certs,
            )
            .await?;
        }
        // Only `session start` (and bare `playground`, its demo composition)
        // streams; the request/response session verbs fall through to the
        // shared executor like any other command.
        Command::Session(SessionCommand::Start(args)) => {
            session::start(&args, ctx).await?;
        }
        Command::Playground(args) => {
            super::playground::demo(&args, ctx).await?;
        }
        other => {
            let out = other.execute(ctx).await?;
            render(ctx.format, &out);
        }
    }

    Ok(())
}

/// The MCP server's render-free entry: executes a command and returns its
/// structured output. Routing lives in the `enum_dispatch`-derived [`Execute`]
/// impls; this facade exists so the trait stays sealed inside `commands`.
pub(crate) async fn execute_command(ctx: &AppContext, command: Command) -> Result<CommandOutput> {
    super::workspace_guard::enforce(ctx, &command)?;
    command.execute(ctx).await
}
