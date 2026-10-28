//! `kraken-shell`: the interactive shell, as its own product.
//!
//! Takes the same global options as every other kraken product — they are the
//! only thing the three must agree on — and nothing else: this binary runs a
//! prompt, and the lines typed at it are what name commands.
//!
//! The binary target is a shim in the `kraken-cli` package, so the release
//! installer ships it with `kraken`; everything it does lives here.

use clap::Parser;
use kraken_app::{
    AppContext, ConnectionArgs, GlobalArgs, InteractiveArgs, MonitorArgs, output::OutputFormat,
};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "kraken-shell",
    version,
    about = "Interactive Kraken shell: type commands at a prompt against one account"
)]
struct Cli {
    #[command(flatten)]
    connection: ConnectionArgs,

    #[command(flatten)]
    interactive: InteractiveArgs,

    // A line typed here can start `ws` or `record`, so the monitor applies.
    #[command(flatten)]
    monitor: MonitorArgs,
}

/// The `kraken-shell` binary's `main`.
pub fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("kraken-shell: failed to build tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(start())
}

async fn start() -> ExitCode {
    let cli = Cli::parse();
    kraken_app::logging::LogConfig::resolve(cli.connection.log_format, cli.connection.verbose)
        .init();
    let format = cli.interactive.output.unwrap_or(OutputFormat::Table);
    let globals = GlobalArgs::new(cli.connection, cli.interactive, cli.monitor);

    let ctx = match AppContext::from_globals(&globals) {
        Ok(ctx) => ctx,
        Err(e) => {
            kraken_app::output::render_error(format, &e);
            return ExitCode::FAILURE;
        }
    };
    match crate::run(&ctx).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            kraken_app::output::render_error(ctx.format, &e);
            ExitCode::FAILURE
        }
    }
}
