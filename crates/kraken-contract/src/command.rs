//! The command tree's root: every family, as one enum.
//!
//! This is the type a proposal carries — the ground truth of what the CLI can
//! execute — so a policy engine matches on it directly instead of on JSON.

use crate::{
    account::AccountCommand,
    auth::AuthCommand,
    earn::EarnCommand,
    explain::ExplainCommand,
    feedback::Feedback,
    funding::{DepositCommand, WalletTransfer, Withdraw, WithdrawalCommand},
    futures::FuturesCommand,
    lab::LabCommand,
    market::MarketCommand,
    order::OrderCommand,
    paper::PaperCommand,
    playground::PlaygroundCommand,
    plugin::PluginCommand,
    recipe::RecipeCommand,
    session::SessionCommand,
    streaming::StreamingCommand,
    subaccount::SubaccountCommand,
    tape::TapeCommand,
    workspace::WorkspaceCommand,
};
use clap::Subcommand;

/// Every command this CLI can execute, as one enum.
/// Every variant is a newtype so the executing crate can delegate to one impl
/// per variant type.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Command {
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
    ///
    /// Sent to Kraken over HTTPS. The body comes from `--input-string` or
    /// `--input-file` and is capped at 8192 UTF-8 bytes.
    // The structure recommendations live in `long_about`: `--help` shows them,
    // and a tool call's description does not pay for them.
    #[command(long_about = crate::feedback::LONG_ABOUT)]
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
    /// Spot paper trading (simulated, no real money). For futures paper
    /// trading, use `kraken futures paper`.
    #[command(subcommand)]
    Paper(PaperCommand),
    /// Scaffold client extensions over the CLI's JSON contract.
    #[command(subcommand)]
    Plugin(PluginCommand),
    /// Reusable recipes: package a finished session's setup for the next run.
    #[command(subcommand)]
    Recipe(RecipeCommand),
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
    /// Interactive/streaming commands: ws, streamd, record, replay, setup.
    /// (`kraken-shell` and `kraken-mcp` are separate binaries, not verbs here.)
    #[command(flatten)]
    Streaming(StreamingCommand),
    /// Sessions: recorded windows over the account journal.
    #[command(subcommand)]
    Session(SessionCommand),
    /// One-command demo: a recorded session in the `playground` paper
    /// workspace.
    Playground(PlaygroundCommand),
}

crate::tools! {
    /// The commands that sit at the root of the tree.
    fn root in [] of Command {
        Feedback          as "feedback"        => Feedback,  Local,     None, Tool
            where ExactlyOne(&["input_string", "input_file"])
            example "kraken feedback bug --input-string \"order buy rejected with unclear error\" --llm-model claude-opus-4 -o json";
        WalletTransfer    as "wallet-transfer" => Funding,   Dangerous, Spot, Tool
            example "kraken wallet-transfer USD 1000 --from Spot --to Futures";
        Withdraw          as "withdraw"        => Funding,   Dangerous, Spot, Tool
            example "kraken withdraw BTC my-btc-wallet 0.1";
        PlaygroundCommand as "playground"      => Workspace, Local,     None, Stream
            example "kraken playground";
    }
}
