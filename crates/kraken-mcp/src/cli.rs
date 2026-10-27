//! `kraken-mcp`: the MCP server, as its own product.
//!
//! An MCP client spawns this directly. It takes the same global options as
//! every other kraken product, plus the two that are its own: which tool
//! services to expose, and whether dangerous tools may run without per-call
//! acknowledgment.
//!
//! The binary target is a shim in the `kraken-cli` package, so the release
//! installer ships it with `kraken`; everything it does lives here.

use clap::Parser;
use kraken_app::{AppContext, ConnectionArgs, GlobalArgs, output::OutputFormat};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "kraken-mcp",
    version,
    about = "Model Context Protocol server exposing kraken commands as tools"
)]
struct Cli {
    // The shared core alone: this server pins JSON output, needs no
    // confirmation prompt, runs no streaming command, and must not read its own
    // protocol channel for a secret.
    #[command(flatten)]
    connection: ConnectionArgs,

    /// Tool services to expose (comma-separated), or `all`.
    #[arg(short, long, default_value = "market,account,paper,workspace,feedback")]
    services: String,

    /// Allow dangerous tools without per-call acknowledgment.
    #[arg(long)]
    allow_dangerous: bool,

    /// Write the published command catalog to stdout and exit. The checked-in
    /// `agents/tool-catalog.json` is this output; a test asserts they match.
    #[arg(long)]
    emit_catalog: bool,
}

/// The `kraken-mcp` binary's `main`.
pub fn main() -> ExitCode {
    let cli = Cli::parse();
    kraken_app::logging::LogConfig::resolve(cli.connection.log_format, cli.connection.verbose)
        .init();

    // Before the runtime and before any credential resolution: the catalog is a
    // pure dump of the command tree, so it must not fail on an unrelated config
    // or key error, nor pay for a runtime it never uses.
    if cli.emit_catalog {
        return emit_catalog();
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        // Tool calls are served on worker tasks and `execute_command` builds a
        // large future (the command enum spans 150+ variants). Tokio's default
        // 2 MiB worker stack overflows on it; 8 MiB matches the main thread.
        .thread_stack_size(8 * 1024 * 1024)
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("kraken-mcp: failed to build tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run(cli))
}

/// Data to stdout, so `kraken-mcp --emit-catalog > agents/tool-catalog.json`
/// regenerates the published catalog.
fn emit_catalog() -> ExitCode {
    let catalog = crate::Catalog::generate(env!("CARGO_PKG_VERSION").to_owned());
    match serde_json::to_string_pretty(&catalog) {
        Ok(rendered) => {
            println!("{rendered}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("kraken-mcp: failed to render the catalog: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> ExitCode {
    let globals = GlobalArgs::connection_only(cli.connection);

    let ctx = match AppContext::from_globals(&globals) {
        Ok(ctx) => ctx,
        Err(e) => {
            // Pre-context, so the server's own JSON is the only honest format.
            kraken_app::output::render_error(OutputFormat::Json, &e);
            return ExitCode::FAILURE;
        }
    };
    match crate::serve(&ctx, &cli.services, cli.allow_dangerous).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            kraken_app::output::render_error(ctx.format, &e);
            ExitCode::FAILURE
        }
    }
}
