/// Kraken CLI library crate.
///
/// Provides `AppContext`, the shared dispatcher, and all modules. This
/// intentional deviation from the spec layout ensures the exact same execution
/// path is used by CLI invocations, integration tests, and the interactive
/// REPL.
pub(crate) mod auth;
pub mod client;
pub(crate) mod commands;
pub mod config;
pub mod errors;
pub(crate) mod mcp;
pub mod output;
pub(crate) mod paper;
pub(crate) mod shell;
pub(crate) mod telemetry;

use clap::{Parser, Subcommand};

use client::{FuturesClient, SpotClient};
use commands::account::{self as account, AccountCommand};
use commands::auth::{self as auth_cmds, AuthCommand};
use commands::earn::{self as earn, EarnCommand};
use commands::funding::{self as funding, FundingCommand};
use commands::futures::{self as futures_cmd, FuturesCommand};
use commands::market::{self as market, MarketCommand};
use commands::paper::{self as paper_cmds, PaperCommand};
use commands::subaccount::{self as subaccount, SubaccountCommand};
use commands::trade::{self as trade, OrderCommand};
use commands::utility;
use commands::websocket::{self as ws, WsCommand};
use errors::Result;
use output::{render, CommandOutput, OutputFormat};

/// Runtime context assembled from global CLI flags and config.
pub struct AppContext {
    pub format: OutputFormat,
    pub verbose: bool,
    pub api_url: Option<String>,
    pub futures_url: Option<String>,
    pub ws_public_url: Option<String>,
    pub ws_auth_url: Option<String>,
    pub ws_l3_url: Option<String>,
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    pub otp: Option<String>,
    pub force: bool,
    /// True when the secret came from the raw `--api-secret` CLI flag (argv-visible).
    pub secret_from_flag: bool,
    /// True when running in MCP server mode. Disables all interactive prompts.
    pub mcp_mode: bool,
}

/// Kraken CLI — trade, query, and manage your Kraken account from the terminal.
#[derive(Parser)]
#[command(name = "kraken", version, about, long_about = None)]
pub struct Cli {
    /// Output format: table (default) or json.
    #[arg(short, long, value_enum, global = true)]
    pub output: Option<OutputFormat>,

    /// Show request/response details on stderr.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Override Spot API base URL.
    #[arg(long, global = true)]
    pub api_url: Option<String>,

    /// Override Futures API base URL.
    #[arg(long, global = true)]
    pub futures_url: Option<String>,

    /// API key override (takes precedence over env/config).
    #[arg(long, global = true)]
    pub api_key: Option<String>,

    /// API secret override (prefer --api-secret-stdin for security).
    #[arg(long, global = true)]
    pub api_secret: Option<String>,

    /// Read API secret from stdin (mutually exclusive with --api-secret and --api-secret-file).
    #[arg(long, global = true, conflicts_with_all = ["api_secret", "api_secret_file"])]
    pub api_secret_stdin: bool,

    /// Path to file containing API secret (mutually exclusive with --api-secret and --api-secret-stdin).
    #[arg(long, global = true, conflicts_with_all = ["api_secret", "api_secret_stdin"])]
    pub api_secret_file: Option<std::path::PathBuf>,

    /// OTP (two-factor authentication code).
    #[arg(long, global = true)]
    pub otp: Option<String>,

    /// Skip confirmation prompts for destructive operations.
    #[arg(long, alias = "force", global = true)]
    pub yes: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[expect(private_interfaces)]
#[derive(Subcommand)]
pub enum Command {
    /// Get asset info.
    Assets {
        /// Comma-separated asset list.
        #[arg(long)]
        asset: Option<String>,
        /// Asset class filter.
        #[arg(long)]
        aclass: Option<String>,
    },
    /// Manage API credentials.
    Auth {
        #[command(subcommand)]
        cmd: AuthCommand,
    },
    /// Get all cash balances.
    Balance {
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get closed orders.
    ClosedOrders {
        /// Include trades.
        #[arg(long)]
        trades: bool,
        /// Start unix timestamp or order tx ID (exclusive).
        #[arg(long)]
        start: Option<String>,
        /// End unix timestamp or order tx ID (inclusive).
        #[arg(long)]
        end: Option<String>,
        /// Result offset for pagination.
        #[arg(long)]
        offset: Option<u64>,
        /// User reference filter.
        #[arg(long)]
        userref: Option<String>,
        /// Filter by client order ID.
        #[arg(long)]
        cl_ord_id: Option<String>,
        /// Which time to use (open, close, both).
        #[arg(long)]
        closetime: Option<String>,
        /// Consolidate trades by individual taker trades.
        #[arg(long)]
        consolidate_taker: bool,
        /// Omit page count from result (faster for large histories).
        #[arg(long)]
        without_count: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get credit line details (VIP only).
    CreditLines {
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Deposit methods and addresses.
    #[command(name = "deposit")]
    Deposit {
        #[command(subcommand)]
        cmd: DepositSubcommand,
    },
    /// Earn/staking commands.
    Earn {
        #[command(subcommand)]
        cmd: EarnCommand,
    },
    /// Delete export report.
    ExportDelete {
        /// Report ID.
        report_id: String,
    },
    /// Request an export report.
    ExportReport {
        /// Type of data to export (trades or ledgers).
        #[arg(long)]
        report: String,
        /// Description for the export.
        #[arg(long)]
        description: String,
        /// File format (CSV or TSV).
        #[arg(long, default_value = "CSV")]
        format: String,
        /// Comma-delimited list of fields to include (default: all).
        #[arg(long)]
        fields: Option<String>,
        /// UNIX timestamp for report start time (default: 1st of current month).
        #[arg(long)]
        starttm: Option<String>,
        /// UNIX timestamp for report end time (default: now).
        #[arg(long)]
        endtm: Option<String>,
    },
    /// Download export report.
    ExportRetrieve {
        /// Report ID.
        report_id: String,
        /// Output file path.
        #[arg(long)]
        output_file: Option<std::path::PathBuf>,
    },
    /// Check export report status.
    ExportStatus {
        /// Report type (trades or ledgers).
        #[arg(long)]
        report: String,
    },
    /// Get extended balances (balance, credit, credit_used, hold_trade).
    ExtendedBalance,
    /// Futures trading and market data.
    Futures {
        #[command(subcommand)]
        cmd: FuturesCommand,
    },
    /// Get ledger entries.
    Ledgers {
        /// Filter by asset or comma-delimited list of assets.
        #[arg(long)]
        asset: Option<String>,
        /// Type of ledger to retrieve.
        #[arg(long = "type")]
        ledger_type: Option<String>,
        /// Starting unix timestamp or ledger ID (exclusive).
        #[arg(long)]
        start: Option<String>,
        /// Ending unix timestamp or ledger ID (inclusive).
        #[arg(long)]
        end: Option<String>,
        /// Filter by asset class.
        #[arg(long)]
        aclass: Option<String>,
        /// Result offset for pagination.
        #[arg(long)]
        offset: Option<u64>,
        /// Omit count from result (faster for large ledgers).
        #[arg(long)]
        without_count: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Start a built-in MCP (Model Context Protocol) server over stdio.
    ///
    /// Default exposes read-only services (market, account, paper).
    /// Use `-s all` to include trade, funding, futures, earn, subaccount, and auth.
    Mcp {
        /// Comma-separated service groups, or "all". Default: market,account,paper.
        #[arg(short, long, default_value = "market,account,paper")]
        services: String,

        /// Skip per-call confirmation for dangerous tools. Use only when the
        /// calling agent is trusted and has been validated through paper trading.
        #[arg(long)]
        allow_dangerous: bool,
    },
    /// Get OHLC candle data.
    Ohlc {
        /// Trading pair.
        pair: String,
        /// Interval in minutes.
        #[arg(long, default_value = "60")]
        interval: u32,
        /// Fetch since timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Asset class filter.
        #[arg(long, value_parser = ["tokenized_asset", "forex"])]
        asset_class: Option<String>,
    },
    /// Get open orders.
    OpenOrders {
        /// Include trades.
        #[arg(long)]
        trades: bool,
        /// User reference filter.
        #[arg(long)]
        userref: Option<String>,
        /// Filter by client order ID.
        #[arg(long)]
        cl_ord_id: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Place and manage spot orders.
    Order {
        #[command(subcommand)]
        cmd: OrderCommand,
    },
    /// Get grouped order book.
    OrderbookGrouped {
        /// Trading pair.
        pair: String,
        /// Number of price levels per side.
        #[arg(long, default_value = "10", value_parser = ["10", "25", "100", "250", "1000"])]
        depth: String,
        /// Tick levels within each price level (bids rounded down, asks up).
        #[arg(long, default_value = "1", value_parser = ["1", "5", "10", "25", "50", "100", "250", "500", "1000"])]
        grouping: String,
    },
    /// Get L2 order book.
    Orderbook {
        /// Trading pair.
        pair: String,
        /// Number of price levels.
        #[arg(long, default_value = "25")]
        count: u32,
        /// Asset class filter.
        #[arg(long, value_parser = ["tokenized_asset"])]
        asset_class: Option<String>,
    },
    /// Get L3 order book (authenticated).
    OrderbookL3 {
        /// Trading pair.
        pair: String,
        /// Number of price levels per side (0 for full book).
        #[arg(long, default_value = "100", value_parser = ["0", "10", "25", "100", "250", "1000"])]
        depth: String,
    },
    /// Get tradable asset pairs.
    Pairs {
        /// Comma-separated pairs.
        #[arg(long)]
        pair: Option<String>,
        /// Info level.
        #[arg(long)]
        info: Option<String>,
        /// Asset class filter.
        #[arg(long)]
        aclass: Option<String>,
    },
    /// Paper trading (simulated, no real money).
    Paper {
        #[command(subcommand)]
        cmd: PaperCommand,
    },
    /// Get open margin positions.
    Positions {
        /// Filter by TXIDs.
        #[arg(long)]
        txid: Vec<String>,
        /// Include P&L calculations.
        #[arg(long)]
        show_pnl: bool,
        /// Consolidate positions by market/pair.
        #[arg(long)]
        consolidation: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Query specific ledger entries.
    QueryLedgers {
        /// Ledger IDs (comma-delimited, up to 20).
        #[arg(num_args = 1..)]
        ids: Vec<String>,
        /// Include trades related to position in output.
        #[arg(long)]
        trades: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Query specific orders.
    QueryOrders {
        /// Transaction IDs (comma-delimited, up to 50).
        #[arg(num_args = 1..)]
        txids: Vec<String>,
        /// Include trades.
        #[arg(long)]
        trades: bool,
        /// User reference filter.
        #[arg(long)]
        userref: Option<String>,
        /// Consolidate taker trades.
        #[arg(long)]
        consolidate_taker: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Query specific trades.
    QueryTrades {
        /// Transaction IDs (comma-delimited, up to 20).
        #[arg(num_args = 1..)]
        txids: Vec<String>,
        /// Include trades related to position in output.
        #[arg(long)]
        trades: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get server time.
    ServerTime,
    /// Guided first-time setup wizard.
    Setup,
    /// Interactive REPL shell.
    Shell,
    /// Get recent spreads.
    Spreads {
        /// Trading pair.
        pair: String,
        /// Fetch since timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Asset class filter.
        #[arg(long, value_parser = ["tokenized_asset"])]
        asset_class: Option<String>,
    },
    /// Get system status and trading mode.
    Status,
    /// Subaccount management.
    Subaccount {
        #[command(subcommand)]
        cmd: SubaccountCommand,
    },
    /// Get ticker information.
    Ticker {
        /// Trading pairs.
        #[arg(num_args = 1..)]
        pairs: Vec<String>,
        /// Asset class filter.
        #[arg(long, value_parser = ["tokenized_asset", "forex"])]
        asset_class: Option<String>,
    },
    /// Get margin/equity trade balance.
    TradeBalance {
        /// Base asset (default: ZUSD).
        #[arg(long)]
        asset: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get recent trades.
    Trades {
        /// Trading pair.
        pair: String,
        /// Fetch since timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Max trades.
        #[arg(long, default_value = "1000")]
        count: u32,
        /// Asset class filter.
        #[arg(long, value_parser = ["tokenized_asset"])]
        asset_class: Option<String>,
    },
    /// Get trade history.
    TradesHistory {
        /// Trade type filter (all, any position, closed position, closing position, no position).
        #[arg(long = "type")]
        trade_type: Option<String>,
        /// Starting unix timestamp or trade tx ID (exclusive).
        #[arg(long)]
        start: Option<String>,
        /// Ending unix timestamp or trade tx ID (inclusive).
        #[arg(long)]
        end: Option<String>,
        /// Include trades related to position in output.
        #[arg(long)]
        trades: bool,
        /// Result offset for pagination.
        #[arg(long)]
        offset: Option<u64>,
        /// Consolidate trades by individual taker trades.
        #[arg(long)]
        consolidate_taker: bool,
        /// Omit count from result (faster for large histories).
        #[arg(long)]
        without_count: bool,
        /// Include related ledger IDs for each trade (slower).
        #[arg(long)]
        ledgers: bool,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get trade volume and fees.
    Volume {
        /// Comma-delimited list of asset pairs for fee info.
        #[arg(long)]
        pair: Vec<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Transfer between wallets.
    WalletTransfer {
        /// Asset.
        asset: String,
        /// Amount.
        amount: String,
        /// Source wallet.
        #[arg(long)]
        from: String,
        /// Destination wallet.
        #[arg(long)]
        to: String,
    },
    /// Make a withdrawal.
    Withdraw {
        /// Asset being withdrawn.
        asset: String,
        /// Withdrawal key name, as set up on your account.
        key: String,
        /// Amount to withdraw.
        amount: String,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long)]
        aclass: Option<String>,
        /// Crypto address to confirm it matches the key.
        #[arg(long)]
        address: Option<String>,
        /// Max fee — fails if processed fee exceeds this.
        #[arg(long)]
        max_fee: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Withdrawal methods.
    #[command(name = "withdrawal")]
    Withdrawal {
        #[command(subcommand)]
        cmd: WithdrawalSubcommand,
    },
    /// WebSocket streaming commands.
    Ws {
        #[command(subcommand)]
        cmd: WsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum DepositSubcommand {
    /// Get deposit methods for an asset.
    Methods {
        asset: String,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long)]
        aclass: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get deposit addresses.
    Addresses {
        asset: String,
        method: String,
        #[arg(long)]
        new: bool,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long)]
        aclass: Option<String>,
        /// Amount to deposit (required for Bitcoin Lightning).
        #[arg(long)]
        amount: Option<String>,
    },
    /// Get deposit status.
    Status {
        #[arg(long)]
        asset: Option<String>,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long)]
        aclass: Option<String>,
        #[arg(long)]
        method: Option<String>,
        /// Start timestamp (deposits before this are excluded).
        #[arg(long)]
        start: Option<String>,
        /// End timestamp (deposits after this are excluded).
        #[arg(long)]
        end: Option<String>,
        /// Enable pagination (true/false) or cursor for next page.
        #[arg(long)]
        cursor: Option<String>,
        /// Number of results per page.
        #[arg(long)]
        limit: Option<u32>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum WithdrawalSubcommand {
    /// Get withdrawal methods.
    Methods {
        #[arg(long)]
        asset: Option<String>,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long)]
        aclass: Option<String>,
        /// Filter by network.
        #[arg(long)]
        network: Option<String>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Get withdrawal addresses.
    Addresses {
        #[arg(long)]
        asset: Option<String>,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long)]
        aclass: Option<String>,
        /// Filter by withdrawal method.
        #[arg(long)]
        method: Option<String>,
        /// Find address by withdrawal key name.
        #[arg(long)]
        key: Option<String>,
        /// Filter by verification status.
        #[arg(long)]
        verified: Option<bool>,
    },
    /// Get withdrawal fee info.
    Info {
        asset: String,
        key: String,
        amount: String,
    },
    /// Get withdrawal status.
    Status {
        #[arg(long)]
        asset: Option<String>,
        /// Asset class (currency or tokenized_asset for xstocks).
        #[arg(long)]
        aclass: Option<String>,
        /// Filter by withdrawal method name.
        #[arg(long)]
        method: Option<String>,
        /// Start timestamp (withdrawals before this are excluded).
        #[arg(long)]
        start: Option<String>,
        /// End timestamp (withdrawals after this are excluded).
        #[arg(long)]
        end: Option<String>,
        /// Enable pagination (true/false) or cursor for next page.
        #[arg(long)]
        cursor: Option<String>,
        /// Number of results per page.
        #[arg(long)]
        limit: Option<u32>,
        /// Rebase multiplier for xstocks data (rebased or base).
        #[arg(long)]
        rebase_multiplier: Option<String>,
    },
    /// Cancel a pending withdrawal.
    Cancel {
        /// Asset being withdrawn.
        asset: String,
        /// Withdrawal reference ID.
        refid: String,
    },
}

/// Shared render-free executor: routes a parsed command to its handler and
/// returns the structured output without rendering. Used by both CLI dispatch
/// and MCP tool execution.
pub(crate) async fn execute_command(ctx: &AppContext, command: Command) -> Result<CommandOutput> {
    match command {
        // === Public market commands ===
        Command::Status => {
            let client = build_spot_client(ctx)?;
            market::execute(&MarketCommand::Status, &client, ctx.verbose).await
        }
        Command::ServerTime => {
            let client = build_spot_client(ctx)?;
            market::execute(&MarketCommand::ServerTime, &client, ctx.verbose).await
        }
        Command::Assets { asset, aclass } => {
            let client = build_spot_client(ctx)?;
            market::execute(
                &MarketCommand::Assets { asset, aclass },
                &client,
                ctx.verbose,
            )
            .await
        }
        Command::Pairs { pair, info, aclass } => {
            let client = build_spot_client(ctx)?;
            market::execute(
                &MarketCommand::Pairs { pair, info, aclass },
                &client,
                ctx.verbose,
            )
            .await
        }
        Command::Ticker { pairs, asset_class } => {
            let client = build_spot_client(ctx)?;
            market::execute(
                &MarketCommand::Ticker { pairs, asset_class },
                &client,
                ctx.verbose,
            )
            .await
        }
        Command::Ohlc {
            pair,
            interval,
            since,
            asset_class,
        } => {
            let client = build_spot_client(ctx)?;
            market::execute(
                &MarketCommand::Ohlc {
                    pair,
                    interval,
                    since,
                    asset_class,
                },
                &client,
                ctx.verbose,
            )
            .await
        }
        Command::Orderbook {
            pair,
            count,
            asset_class,
        } => {
            let client = build_spot_client(ctx)?;
            market::execute(
                &MarketCommand::Orderbook {
                    pair,
                    count,
                    asset_class,
                },
                &client,
                ctx.verbose,
            )
            .await
        }
        Command::OrderbookL3 { pair, depth } => {
            let (client, creds) = build_spot_authed(ctx)?;
            let mut body = serde_json::Map::new();
            body.insert("pair".into(), serde_json::Value::String(pair));
            let depth_int: u64 = depth.parse().map_err(|_| {
                errors::KrakenError::Validation(format!("invalid depth value: '{depth}'"))
            })?;
            body.insert("depth".into(), serde_json::Value::Number(depth_int.into()));
            let data = client
                .private_post_json(
                    "Level3",
                    body,
                    &creds,
                    ctx.otp.as_deref(),
                    true,
                    ctx.verbose,
                )
                .await?;
            Ok(CommandOutput::new(
                data.clone(),
                vec!["Data".into()],
                vec![vec!["L3 orderbook data returned (see JSON output)".into()]],
            ))
        }
        Command::OrderbookGrouped {
            pair,
            depth,
            grouping,
        } => {
            let client = build_spot_client(ctx)?;
            market::execute(
                &MarketCommand::OrderbookGrouped {
                    pair,
                    depth,
                    grouping,
                },
                &client,
                ctx.verbose,
            )
            .await
        }
        Command::Trades {
            pair,
            since,
            count,
            asset_class,
        } => {
            let client = build_spot_client(ctx)?;
            market::execute(
                &MarketCommand::Trades {
                    pair,
                    since,
                    count,
                    asset_class,
                },
                &client,
                ctx.verbose,
            )
            .await
        }
        Command::Spreads {
            pair,
            since,
            asset_class,
        } => {
            let client = build_spot_client(ctx)?;
            market::execute(
                &MarketCommand::Spreads {
                    pair,
                    since,
                    asset_class,
                },
                &client,
                ctx.verbose,
            )
            .await
        }

        // === Account commands (private) ===
        Command::Balance { rebase_multiplier } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::Balance { rebase_multiplier },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::ExtendedBalance => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::ExtendedBalance,
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::CreditLines { rebase_multiplier } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::CreditLines { rebase_multiplier },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::TradeBalance {
            asset,
            rebase_multiplier,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::TradeBalance {
                    asset,
                    rebase_multiplier,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::OpenOrders {
            trades,
            userref,
            cl_ord_id,
            rebase_multiplier,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::OpenOrders {
                    trades,
                    userref,
                    cl_ord_id,
                    rebase_multiplier,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::ClosedOrders {
            trades,
            start,
            end,
            offset,
            userref,
            cl_ord_id,
            closetime,
            consolidate_taker,
            without_count,
            rebase_multiplier,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::ClosedOrders {
                    trades,
                    start,
                    end,
                    offset,
                    userref,
                    cl_ord_id,
                    closetime,
                    consolidate_taker,
                    without_count,
                    rebase_multiplier,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::QueryOrders {
            txids,
            trades,
            userref,
            consolidate_taker,
            rebase_multiplier,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::QueryOrders {
                    txids,
                    trades,
                    userref,
                    consolidate_taker,
                    rebase_multiplier,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::TradesHistory {
            trade_type,
            start,
            end,
            trades,
            offset,
            consolidate_taker,
            without_count,
            ledgers,
            rebase_multiplier,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::TradesHistory {
                    trade_type,
                    start,
                    end,
                    trades,
                    offset,
                    consolidate_taker,
                    without_count,
                    ledgers,
                    rebase_multiplier,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::QueryTrades {
            txids,
            trades,
            rebase_multiplier,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::QueryTrades {
                    txids,
                    trades,
                    rebase_multiplier,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::Positions {
            txid,
            show_pnl,
            consolidation,
            rebase_multiplier,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::Positions {
                    txid,
                    show_pnl,
                    consolidation,
                    rebase_multiplier,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::Ledgers {
            asset,
            ledger_type,
            start,
            end,
            aclass,
            offset,
            without_count,
            rebase_multiplier,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::Ledgers {
                    asset,
                    ledger_type,
                    start,
                    end,
                    aclass,
                    offset,
                    without_count,
                    rebase_multiplier,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::QueryLedgers {
            ids,
            trades,
            rebase_multiplier,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::QueryLedgers {
                    ids,
                    trades,
                    rebase_multiplier,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::Volume {
            pair,
            rebase_multiplier,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::Volume {
                    pair,
                    rebase_multiplier,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::ExportReport {
            report,
            description,
            format,
            fields,
            starttm,
            endtm,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::ExportReport {
                    report,
                    description,
                    format,
                    fields,
                    starttm,
                    endtm,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::ExportStatus { report } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::ExportStatus { report },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::ExportRetrieve {
            report_id,
            output_file,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::ExportRetrieve {
                    report_id,
                    output_file,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }
        Command::ExportDelete { report_id } => {
            let (client, creds) = build_spot_authed(ctx)?;
            account::execute(
                &AccountCommand::ExportDelete { report_id },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.verbose,
            )
            .await
        }

        // === Trading ===
        Command::Order { cmd } => {
            let (client, creds) = build_spot_authed(ctx)?;
            trade::execute(
                &cmd,
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.force,
                ctx.verbose,
            )
            .await
        }

        // === Funding ===
        Command::Deposit { cmd } => {
            let (client, creds) = build_spot_authed(ctx)?;
            let funding_cmd = match cmd {
                DepositSubcommand::Methods {
                    asset,
                    aclass,
                    rebase_multiplier,
                } => FundingCommand::DepositMethods {
                    asset,
                    aclass,
                    rebase_multiplier,
                },
                DepositSubcommand::Addresses {
                    asset,
                    method,
                    new,
                    aclass,
                    amount,
                } => FundingCommand::DepositAddresses {
                    asset,
                    method,
                    new,
                    aclass,
                    amount,
                },
                DepositSubcommand::Status {
                    asset,
                    aclass,
                    method,
                    start,
                    end,
                    cursor,
                    limit,
                    rebase_multiplier,
                } => FundingCommand::DepositStatus {
                    asset,
                    aclass,
                    method,
                    start,
                    end,
                    cursor,
                    limit,
                    rebase_multiplier,
                },
            };
            funding::execute(
                &funding_cmd,
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.force,
                ctx.verbose,
            )
            .await
        }
        Command::Withdraw {
            asset,
            key,
            amount,
            aclass,
            address,
            max_fee,
            rebase_multiplier,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            funding::execute(
                &FundingCommand::Withdraw {
                    asset,
                    key,
                    amount,
                    aclass,
                    address,
                    max_fee,
                    rebase_multiplier,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.force,
                ctx.verbose,
            )
            .await
        }
        Command::Withdrawal { cmd } => {
            let (client, creds) = build_spot_authed(ctx)?;
            let funding_cmd = match cmd {
                WithdrawalSubcommand::Methods {
                    asset,
                    aclass,
                    network,
                    rebase_multiplier,
                } => FundingCommand::WithdrawalMethods {
                    asset,
                    aclass,
                    network,
                    rebase_multiplier,
                },
                WithdrawalSubcommand::Addresses {
                    asset,
                    aclass,
                    method,
                    key,
                    verified,
                } => FundingCommand::WithdrawalAddresses {
                    asset,
                    aclass,
                    method,
                    key,
                    verified,
                },
                WithdrawalSubcommand::Info { asset, key, amount } => {
                    FundingCommand::WithdrawalInfo { asset, key, amount }
                }
                WithdrawalSubcommand::Status {
                    asset,
                    aclass,
                    method,
                    start,
                    end,
                    cursor,
                    limit,
                    rebase_multiplier,
                } => FundingCommand::WithdrawalStatus {
                    asset,
                    aclass,
                    method,
                    start,
                    end,
                    cursor,
                    limit,
                    rebase_multiplier,
                },
                WithdrawalSubcommand::Cancel { asset, refid } => {
                    FundingCommand::WithdrawalCancel { asset, refid }
                }
            };
            funding::execute(
                &funding_cmd,
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.force,
                ctx.verbose,
            )
            .await
        }
        Command::WalletTransfer {
            asset,
            amount,
            from,
            to,
        } => {
            let (client, creds) = build_spot_authed(ctx)?;
            funding::execute(
                &FundingCommand::WalletTransfer {
                    asset,
                    amount,
                    from,
                    to,
                },
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.force,
                ctx.verbose,
            )
            .await
        }

        // === Earn ===
        Command::Earn { cmd } => {
            let (client, creds) = build_spot_authed(ctx)?;
            earn::execute(
                &cmd,
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.force,
                ctx.verbose,
            )
            .await
        }

        // === Subaccount ===
        Command::Subaccount { cmd } => {
            let (client, creds) = build_spot_authed(ctx)?;
            subaccount::execute(
                &cmd,
                &client,
                &creds,
                ctx.otp.as_deref(),
                ctx.force,
                ctx.verbose,
            )
            .await
        }

        // === Futures ===
        Command::Futures { cmd } => {
            let futures_client = build_futures_client(ctx)?;
            let needs_auth =
                futures_cmd::requires_auth(&cmd) || futures_cmd::ws_requires_auth(&cmd);
            let futures_creds = if needs_auth {
                Some(config::resolve_futures_credentials(
                    ctx.api_key.as_deref(),
                    ctx.api_secret.as_deref(),
                )?)
            } else {
                config::resolve_futures_credentials(
                    ctx.api_key.as_deref(),
                    ctx.api_secret.as_deref(),
                )
                .ok()
            };
            futures_cmd::execute(
                &cmd,
                &futures_client,
                futures_creds.as_ref(),
                ctx.otp.as_deref(),
                ctx.force,
                ctx.verbose,
            )
            .await
        }

        // === Paper Trading ===
        Command::Paper { cmd } => paper_cmds::execute(&cmd, ctx, ctx.verbose).await,

        // === Auth ===
        Command::Auth { cmd } => auth_cmds::execute(&cmd, ctx).await,

        // Interactive/streaming commands not supported in render-free mode
        Command::Ws { .. } | Command::Setup | Command::Shell | Command::Mcp { .. } => {
            Err(errors::KrakenError::Validation(
                "This command cannot be executed through the shared executor".into(),
            ))
        }
    }
}

/// Central dispatch: routes a parsed command to the appropriate handler
/// and renders output. Delegates to `execute_command` for all non-interactive
/// commands; handles interactive/streaming commands (Ws, Setup, Shell, Mcp)
/// directly since they return `Result<()>` rather than `CommandOutput`.
pub async fn dispatch(ctx: &AppContext, command: Command) -> Result<()> {
    match command {
        Command::Ws { cmd } => {
            let urls = ws::WsUrls {
                public: ctx.ws_public_url.as_deref(),
                auth: ctx.ws_auth_url.as_deref(),
                l3: ctx.ws_l3_url.as_deref(),
            };
            let needs_auth = !matches!(
                cmd,
                WsCommand::Ticker { .. }
                    | WsCommand::Trades { .. }
                    | WsCommand::Book { .. }
                    | WsCommand::Ohlc { .. }
                    | WsCommand::Instrument { .. }
            );
            if needs_auth {
                let (client, creds) = build_spot_authed(ctx)?;
                ws::execute(
                    &cmd,
                    ctx.format,
                    Some((&client, &creds)),
                    ctx.otp.as_deref(),
                    ctx.verbose,
                    &urls,
                )
                .await?;
            } else {
                ws::execute(&cmd, ctx.format, None, None, ctx.verbose, &urls).await?;
            }
        }
        Command::Setup => {
            let out = utility::setup(ctx.verbose).await?;
            render(ctx.format, &out);
        }
        Command::Shell => {
            shell::run(ctx).await?;
        }
        Command::Mcp {
            services,
            allow_dangerous,
        } => {
            mcp::server::run_server(&services, allow_dangerous).await?;
        }
        other => {
            let out = execute_command(ctx, other).await?;
            render(ctx.format, &out);
        }
    }

    Ok(())
}

pub(crate) fn build_spot_client(ctx: &AppContext) -> Result<SpotClient> {
    SpotClient::new(ctx.api_url.as_deref())
}

fn build_spot_authed(ctx: &AppContext) -> Result<(SpotClient, config::SpotCredentials)> {
    let client = build_spot_client(ctx)?;
    let creds =
        config::resolve_spot_credentials(ctx.api_key.as_deref(), ctx.api_secret.as_deref())?;
    Ok((client, creds))
}

fn build_futures_client(ctx: &AppContext) -> Result<FuturesClient> {
    FuturesClient::new(ctx.futures_url.as_deref())
}
