//! One-command demo composed from ordinary workspaces and sessions.
//!
//! Reuse never re-funds the paper workspace, and every created artifact remains
//! manageable through the standard command surface.

use kraken_paper::account::Origin;
use kraken_workspace::{CreateSpec, WorkspaceError, WorkspaceMode, Workspaces};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use super::Execute;
use crate::cli::AppContext;
use crate::config;
use crate::errors::{KrakenError, Result};
use crate::output::CommandOutput;

/// The demo workspace's fixed identity: one well-known name, paper, seeded
/// once. Reuse never re-funds — the account's history is the demo's value.
const PLAYGROUND_WORKSPACE: &str = "playground";
/// The demo account's one-time seed, in [`PLAYGROUND_CURRENCY`].
const PLAYGROUND_CAPITAL: Decimal = dec!(10_000);
const PLAYGROUND_CURRENCY: &str = "USD";

#[derive(Debug, clap::Args)]
pub(crate) struct PlaygroundCommand {
    /// Comma-separated trading pairs to record and demo against.
    #[arg(long, value_delimiter = ',', default_value = "BTC/USD")]
    symbols: Vec<String>,
    /// The template (recipe skill) driving the demo; recorded as the session's
    /// strategy label so scoring can attribute it.
    #[arg(long, default_value = "recipe-playground-dca")]
    template: String,
    /// How long the demo session records before closing itself.
    #[arg(long = "for", default_value = "1h")]
    window: String,
}

impl Execute for PlaygroundCommand {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        // `playground` streams (it is a session start); `dispatch` intercepts it,
        // so only the render-free executor (the MCP path) reaches this.
        Err(super::streaming::streaming_unsupported())
    }
}

/// Provision-or-reuse the demo workspace, say how to watch, and start the
/// demo session in it. Composes existing seams only: no new state, no engine.
pub(crate) async fn demo(args: &PlaygroundCommand, ctx: &AppContext) -> Result<()> {
    let name: kraken_workspace::WorkspaceName = PLAYGROUND_WORKSPACE.parse()?;
    let workspaces = Workspaces::new(config::config_dir()?);
    match workspaces.manifest(&name) {
        // Reuse as-is: never re-fund an account that exists. Only a paper
        // account qualifies — a live manifest under the demo's name (contract-
        // accepted by the manifest but not executable today) must refuse at
        // provision, not surface later as a session error.
        Ok(manifest) if manifest.mode == WorkspaceMode::Paper => {}
        Ok(manifest) => {
            return Err(KrakenError::Validation(format!(
                "the '{PLAYGROUND_WORKSPACE}' workspace exists in {} mode; the demo \
                 trades simulated fills only — remove or rename workspaces/{PLAYGROUND_WORKSPACE} to run it",
                manifest.mode
            )));
        }
        Err(WorkspaceError::NotFound(_)) => {
            workspaces.create(
                CreateSpec {
                    name: name.clone(),
                    capital: PLAYGROUND_CAPITAL,
                    currency: PLAYGROUND_CURRENCY.to_string(),
                    mode: WorkspaceMode::Paper,
                    fee_rate: kraken_paper::DEFAULT_FEE_RATE,
                    slippage_rate: kraken_paper::DEFAULT_SLIPPAGE_RATE,
                    allowed_pairs: None,
                },
                Origin::from_mcp_mode(ctx.mcp_mode),
            )?;
            eprintln!(
                "playground workspace created: {PLAYGROUND_CAPITAL} {PLAYGROUND_CURRENCY} paper capital"
            );
        }
        Err(err) => return Err(err.into()),
    }

    // The demo's guidance goes to stderr — stdout carries the session's own
    // machine-parseable stream: the lifecycle lines (session_started /
    // session_stopped) and the live tape echo between them. The trade example
    // names the first recorded symbol: a fill on an unrecorded pair would
    // leave the window without marks to score it by.
    let symbol = args.symbols.first().map_or("BTC/USD", String::as_str);
    eprintln!("watch it:   export KRAKEN_WORKSPACE={PLAYGROUND_WORKSPACE}");
    eprintln!("            kraken session show       # window, status, valuation");
    eprintln!("            kraken workspace status   # the account itself");
    eprintln!("trade it:   kraken order buy {symbol} 0.001 --type market --reason \"...\"");
    eprintln!("stop early: kraken session stop");

    let start = super::session::Start::demo(
        args.symbols.clone(),
        args.template.clone(),
        args.window.clone(),
    );
    // The demo threads its workspace explicitly — no env required — by
    // rescoping the context exactly like a `--workspace playground` line.
    let scoped = ctx.rescope(Some(PLAYGROUND_WORKSPACE.to_string()));
    super::session::start(&start, &scoped).await
}
