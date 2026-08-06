//! The CLI command tree: [`Command`] composes the per-domain subcommand enums
//! declared here, and [`Execute`] is the contract every variant fulfils. The
//! entry points live in [`executor`]; the inline-rendering commands and their
//! [`streaming::Run`] contract in [`streaming`].

use clap::Subcommand;
use enum_dispatch::enum_dispatch;

use crate::cli::AppContext;
use crate::errors::Result;
use crate::output::CommandOutput;

/// Uniform execution: each command resolves the resources it needs from the
/// context (clients, credentials, OTP, force) and runs, returning structured
/// output. `#[enum_dispatch]` derives every enum-level impl from the leaf
/// impls in the submodules, so no routing match exists on this path.
///
/// Consumes the command: leaves move their parsed args into request payloads,
/// which a `&self` receiver would force to clone.
///
/// Crate-internal + static-dispatch only, so native `async fn` in a trait
/// needs no `async-trait` crate.
//
// Declared ABOVE the `mod` declarations deliberately: enum_dispatch emits each
// enum's generated impl at whichever side of the trait/enum pair expands
// second. The trait must therefore expand first, so the impls land inside the
// child modules — where the leaf types are in scope — not here.
#[enum_dispatch]
trait Execute {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput>;
}

pub(crate) mod account;
pub(crate) mod auth;
pub(crate) mod earn;
mod executor;
pub(crate) mod explain;
pub(crate) mod feedback;
pub(crate) mod funding;
pub(crate) mod futures;
pub(crate) mod futures_paper;
pub(crate) mod futures_ws;
pub(crate) mod journal_mirror;
pub(crate) mod lab;
pub(crate) mod market;
pub(crate) mod paper;
pub(crate) mod playground;
pub(crate) mod record;
pub(crate) mod replay;
pub(crate) mod replay_venue;
pub(crate) mod schema;
pub(crate) mod session;
pub(crate) mod setup;
pub(crate) mod streamd;
mod streaming;
pub(crate) mod subaccount;
pub(crate) mod subscription;
pub(crate) mod tape;
pub(crate) mod trade;
pub(crate) mod value_types;
pub(crate) mod workspace;
pub(crate) mod workspace_guard;
pub(crate) mod ws;

use self::account::AccountCommand;
use self::auth::AuthCommand;
use self::earn::EarnCommand;
pub use self::executor::run;
pub(crate) use self::executor::{dispatch, execute_command};
use self::explain::ExplainCommand;
use self::feedback::Feedback;
use self::funding::{DepositCommand, WalletTransfer, Withdraw, WithdrawalCommand};
use self::futures::FuturesCommand;
use self::lab::LabCommand;
use self::market::MarketCommand;
use self::paper::PaperCommand;
use self::playground::PlaygroundCommand;
use self::session::SessionCommand;
use self::streaming::StreamingCommand;
use self::subaccount::SubaccountCommand;
use self::tape::TapeCommand;
use self::trade::OrderCommand;
use self::workspace::WorkspaceCommand;

// One value exists per process, parsed once at startup and dispatched immediately,
// so the size gap between the fat order variants and the unit variants costs
// nothing worth boxing for.
//
// `enum_dispatch` requires every variant to be a newtype over an [`Execute`]
// type — a struct variant here fails the macro, not the CLI surface. It also
// generates `From<variant>`/`TryInto<variant>` impls for every variant, so a
// hand-written conversion collides (E0119) with code you can't see.
#[allow(clippy::large_enum_variant)]
#[enum_dispatch(Execute)]
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Public market data: ticker, orderbook, ohlc, trades, spreads, status.
    #[command(flatten)]
    Market(MarketCommand),
    /// Private account data: balances, orders, history, exports, L3 book.
    #[command(flatten)]
    Account(AccountCommand),
    /// Manage API credentials.
    #[command(subcommand)]
    Auth(AuthCommand),
    /// Market memory: list every tape in the library.
    #[command(subcommand)]
    Tape(TapeCommand),
    /// Deposit methods and addresses.
    #[command(subcommand)]
    Deposit(DepositCommand),
    /// Earn/staking commands.
    #[command(subcommand)]
    Earn(EarnCommand),
    /// Explain a recorded session in plain terms.
    #[command(subcommand)]
    Explain(ExplainCommand),
    /// Submit product feedback (feature request, friction, or bug).
    #[command(long_about = feedback::LONG_ABOUT)]
    Feedback(Feedback),
    /// Futures trading and market data.
    #[command(subcommand)]
    Futures(FuturesCommand),
    /// Autoresearch Lab: score recorded sessions against their own tape.
    #[command(subcommand)]
    Lab(LabCommand),
    /// Place and manage spot orders.
    #[command(subcommand)]
    Order(OrderCommand),
    /// Spot paper trading (simulated, no real money). For futures paper trading, use `kraken futures paper`.
    #[command(subcommand)]
    Paper(PaperCommand),
    /// Subaccount management.
    #[command(subcommand)]
    Subaccount(SubaccountCommand),
    /// Transfer between wallets.
    WalletTransfer(WalletTransfer),
    /// Make a withdrawal.
    Withdraw(Withdraw),
    /// Withdrawal methods.
    #[command(subcommand)]
    Withdrawal(WithdrawalCommand),
    /// Strategy workspaces: the account, created and funded once.
    #[command(subcommand)]
    Workspace(WorkspaceCommand),
    /// Interactive/streaming commands: ws, streamd, record, replay, setup, shell, mcp.
    #[command(flatten)]
    Streaming(StreamingCommand),
    /// Sessions: recorded windows over the account journal.
    #[command(subcommand)]
    Session(SessionCommand),
    /// One-command demo: a recorded session in the `playground` paper workspace.
    Playground(PlaygroundCommand),
}

#[cfg(test)]
mod tests {
    /// The paper surface must stay auth-free: it trades no real funds, so a
    /// credentials reference appearing in it is a wiring bug. Scanned from a
    /// sibling module so the assertion needles never match themselves.
    #[test]
    fn test_paper_commands_no_auth_imports() {
        let source = include_str!("paper.rs");
        assert!(
            !source.contains("Credentials"),
            "paper.rs must not reference Credentials"
        );
        assert!(
            !source.contains("private_post"),
            "paper.rs must not reference private_post"
        );
        assert!(
            !source.contains("build_spot_authed"),
            "paper.rs must not reference build_spot_authed"
        );
        assert!(
            !source.contains("resolve_credentials"),
            "paper.rs must not reference resolve_credentials"
        );
        assert!(
            !source.contains("AuthConfig"),
            "paper.rs must not reference AuthConfig"
        );
    }
}
