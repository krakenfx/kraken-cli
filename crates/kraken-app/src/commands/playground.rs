//! One-command demo composed from ordinary workspaces and sessions.
//!
//! Reuse never re-funds the paper workspace, and every created artifact remains
//! manageable through the standard command surface.

use super::Execute;
use crate::{cli::AppContext, errors::Result, output::CommandOutput};
use kraken_contract::playground::PlaygroundCommand;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// The demo workspace's fixed identity: one well-known name, paper, seeded
/// once. Reuse never re-funds — the account's history is the demo's value.
const PLAYGROUND_WORKSPACE: &str = "playground";
/// The demo account's one-time seed, in [`PLAYGROUND_CURRENCY`].
const PLAYGROUND_CAPITAL: Decimal = dec!(10_000);
const PLAYGROUND_CURRENCY: &str = "USD";

impl Execute for PlaygroundCommand {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        demo(&self, ctx).await?;
        Ok(CommandOutput::inline())
    }

    /// A session start: it streams for the whole window.
    fn renders_inline(&self) -> bool {
        true
    }
}

/// Provision-or-reuse the demo workspace, say how to watch, and start the
/// demo session in it. Composes existing seams only: no new state, no engine.
pub(crate) async fn demo(args: &PlaygroundCommand, ctx: &AppContext) -> Result<()> {
    let name: kraken_workspace::WorkspaceName = PLAYGROUND_WORKSPACE.parse()?;
    super::workspace::provision_paper_workspace(
        ctx,
        &name,
        PLAYGROUND_CAPITAL,
        PLAYGROUND_CURRENCY,
    )?;

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

    let start = kraken_contract::session::Start::demo(
        args.symbols.clone(),
        args.template.clone(),
        args.window.clone(),
    );
    // The demo threads its workspace explicitly — no env required — by
    // rescoping the context exactly like a `--workspace playground` line.
    let scoped = ctx.rescope(Some(PLAYGROUND_WORKSPACE.to_string()));
    super::session::start(&start, &scoped).await
}
