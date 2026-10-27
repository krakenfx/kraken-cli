//! Private account data commands.

use crate::value_types::{
    AssetClass, Closetime, L3Depth, Pagination, RebaseOpts, ReportFormat, ReportType, TradeType,
};
use clap::Subcommand;
use std::path::PathBuf;

/// The `account` family: private reads of the configured Kraken account —
/// balances, orders, trade and ledger history, exports, and the L3 book.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AccountCommand {
    /// Get all cash balances.
    Balance(Balance),
    /// Get extended balances including credits and held amounts.
    ExtendedBalance(ExtendedBalance),
    /// Get L3 (per-order) order book via the authenticated Level3 endpoint.
    ///
    /// Authenticated market data: it needs credentials, so it lives with the
    /// account handlers rather than the unauthenticated `market` path.
    OrderbookL3(OrderbookL3),
    /// Get credit line details (VIP only).
    CreditLines(CreditLines),
    /// Get margin/equity trade balance summary.
    TradeBalance(TradeBalance),
    /// Get currently open orders.
    OpenOrders(OpenOrders),
    /// Get closed orders.
    ClosedOrders(ClosedOrders),
    /// Query specific orders by TXID.
    QueryOrders(QueryOrders),
    /// Get trade history.
    TradesHistory(TradesHistory),
    /// Query specific trades by TXID.
    QueryTrades(QueryTrades),
    /// Get open margin positions.
    Positions(Positions),
    /// Get ledger entries.
    Ledgers(Ledgers),
    /// Query specific ledger entries.
    QueryLedgers(QueryLedgers),
    /// Get trade volume and fee info.
    Volume(Volume),
    /// Request an export report.
    ExportReport(ExportReport),
    /// Check export report status.
    ExportStatus(ExportStatus),
    /// Download an export report.
    ExportRetrieve(ExportRetrieve),
    /// Delete an export report.
    ExportDelete(ExportDelete),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Balance {
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

impl Balance {
    /// The plain venue view, for `workspace balance` addressing the real
    /// account: no rebase, exactly the bare `kraken balance` request.
    pub fn plain() -> Self {
        Self {
            rebase: RebaseOpts {
                rebase_multiplier: None,
            },
        }
    }
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ExtendedBalance {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OrderbookL3 {
    /// Trading pair.
    pub pair: String,
    /// Number of price levels per side (0 for full book).
    #[arg(long, default_value = "100")]
    #[serde(default = "hundred_levels")]
    pub depth: L3Depth,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CreditLines {
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct TradeBalance {
    /// Base asset for balance calculation (default: ZUSD).
    #[arg(long)]
    pub asset: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

impl TradeBalance {
    /// The plain venue view, for `workspace status` addressing the real
    /// account: default asset, no rebase.
    pub fn plain() -> Self {
        Self {
            asset: None,
            rebase: RebaseOpts {
                rebase_multiplier: None,
            },
        }
    }
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OpenOrders {
    /// Include trade details.
    #[arg(long)]
    #[serde(default)]
    pub trades: bool,
    /// Filter by user reference ID.
    #[arg(long)]
    pub userref: Option<i32>,
    /// Filter by client order ID.
    #[arg(long)]
    pub cl_ord_id: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ClosedOrders {
    /// Include trade details.
    #[arg(long)]
    #[serde(default)]
    pub trades: bool,
    #[command(flatten)]
    #[serde(flatten)]
    pub page: Pagination,
    /// User reference filter.
    #[arg(long)]
    pub userref: Option<i32>,
    /// Filter by client order ID.
    #[arg(long)]
    pub cl_ord_id: Option<String>,
    /// Which time to use (open, close, both).
    #[arg(long)]
    pub closetime: Option<Closetime>,
    /// Consolidate trades by individual taker trades.
    #[arg(long)]
    #[serde(default)]
    pub consolidate_taker: bool,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct QueryOrders {
    /// Transaction IDs (comma-delimited, up to 50).
    #[arg(num_args = 1..)]
    #[serde(default)]
    pub txids: Vec<String>,
    /// Include trade details.
    #[arg(long)]
    #[serde(default)]
    pub trades: bool,
    /// User reference filter.
    #[arg(long)]
    pub userref: Option<i32>,
    /// Consolidate taker trades.
    #[arg(long)]
    #[serde(default)]
    pub consolidate_taker: bool,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct TradesHistory {
    /// Trade type filter.
    #[arg(long = "type")]
    pub trade_type: Option<TradeType>,
    /// Include trades related to position in output.
    #[arg(long)]
    #[serde(default)]
    pub trades: bool,
    /// Consolidate trades by individual taker trades.
    #[arg(long)]
    #[serde(default)]
    pub consolidate_taker: bool,
    /// Include related ledger IDs for each trade (slower).
    #[arg(long)]
    #[serde(default)]
    pub ledgers: bool,
    #[command(flatten)]
    #[serde(flatten)]
    pub page: Pagination,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct QueryTrades {
    /// Transaction IDs (comma-delimited, up to 20).
    #[arg(num_args = 1..)]
    #[serde(default)]
    pub txids: Vec<String>,
    /// Include trades related to position in output.
    #[arg(long)]
    #[serde(default)]
    pub trades: bool,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Positions {
    /// Filter by TXIDs.
    #[arg(long)]
    #[serde(default)]
    pub txid: Vec<String>,
    /// Include P&L calculations.
    #[arg(long)]
    #[serde(default)]
    pub show_pnl: bool,
    /// Consolidate positions by market/pair.
    #[arg(long)]
    pub consolidation: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Ledgers {
    /// Filter by asset or comma-delimited list of assets.
    #[arg(long)]
    pub asset: Option<String>,
    /// Type of ledger to retrieve.
    // Kraken's ledger-type set grows server-side (staking, dividends, NFT
    // rebates arrived after launch); constraining it here would break valid
    // filters, so the server validates.
    #[arg(long = "type")]
    pub ledger_type: Option<String>,
    /// Filter by asset class.
    #[arg(long, alias = "aclass")]
    pub asset_class: Option<AssetClass>,
    #[command(flatten)]
    #[serde(flatten)]
    pub page: Pagination,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct QueryLedgers {
    /// Ledger IDs (comma-delimited, up to 20).
    #[arg(num_args = 1..)]
    #[serde(default)]
    pub ids: Vec<String>,
    /// Include trades related to position in output.
    #[arg(long)]
    #[serde(default)]
    pub trades: bool,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Volume {
    /// Comma-delimited list of asset pairs for fee info.
    #[arg(long)]
    #[serde(default)]
    pub pair: Vec<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub rebase: RebaseOpts,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ExportReport {
    /// Type of data to export (trades or ledgers).
    #[arg(long)]
    pub report: ReportType,
    /// Description for the export.
    #[arg(long)]
    pub description: String,
    /// File format.
    #[arg(long, default_value = "CSV")]
    #[serde(default = "csv_export")]
    pub format: ReportFormat,
    /// Comma-delimited list of fields to include (default: all).
    #[arg(long)]
    pub fields: Option<String>,
    /// UNIX timestamp for report start time (default: 1st of current month).
    #[arg(long)]
    pub starttm: Option<String>,
    /// UNIX timestamp for report end time (default: now).
    #[arg(long)]
    pub endtm: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ExportStatus {
    /// Report type (trades or ledgers).
    #[arg(long)]
    pub report: ReportType,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ExportRetrieve {
    /// Report ID to retrieve.
    pub report_id: String,
    /// Output file path for the downloaded report.
    #[arg(long)]
    pub output_file: Option<PathBuf>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ExportDelete {
    /// Report ID to delete.
    pub report_id: String,
}

/// Level-3 book depth when the caller names none.
fn hundred_levels() -> L3Depth {
    L3Depth::D100
}

/// The export format when the caller names none.
fn csv_export() -> ReportFormat {
    ReportFormat::Csv
}

crate::tools! {
    /// The `kraken account` commands.
    fn account in ["account"] of AccountCommand {
        Balance         as "balance"          => Account, ReadOnly,  Spot, Tool
            example "kraken balance -o json";
        ExtendedBalance as "extended-balance" => Account, ReadOnly,  Spot, Tool
            example "kraken extended-balance -o json";
        OrderbookL3     as "orderbook-l3"     => Account, ReadOnly,  Spot, Tool
            example "kraken orderbook-l3 BTC/USD --depth 100";
        CreditLines     as "credit-lines"     => Account, ReadOnly,  Spot, Tool
            example "kraken credit-lines";
        TradeBalance    as "trade-balance"    => Account, ReadOnly,  Spot, Tool
            example "kraken trade-balance --asset ZUSD";
        OpenOrders      as "open-orders"      => Account, ReadOnly,  Spot, Tool
            example "kraken open-orders -o json";
        ClosedOrders    as "closed-orders"    => Account, ReadOnly,  Spot, Tool
            example "kraken closed-orders --start 2025-01-01 -o json";
        QueryOrders     as "query-orders"     => Account, ReadOnly,  Spot, Tool
            example "kraken query-orders OABC12-DEFG3-HIJKL4";
        TradesHistory   as "trades-history"   => Account, ReadOnly,  Spot, Tool
            example "kraken trades-history -o json";
        QueryTrades     as "query-trades"     => Account, ReadOnly,  Spot, Tool
            example "kraken query-trades TABC12-DEFG3-HIJKL4";
        Positions       as "positions"        => Account, ReadOnly,  Spot, Tool
            example "kraken positions --show-pnl -o json";
        Ledgers         as "ledgers"          => Account, ReadOnly,  Spot, Tool
            example "kraken ledgers --asset BTC --type trade -o json";
        QueryLedgers    as "query-ledgers"    => Account, ReadOnly,  Spot, Tool
            example "kraken query-ledgers LABC12-DEFG3-HIJKL4";
        Volume          as "volume"           => Account, ReadOnly,  Spot, Tool
            example "kraken volume --pair BTCUSD";
        ExportReport    as "export-report"    => Account, ReadOnly,  Spot, Tool
            example "kraken export-report --report trades --description \"Q1 trades\"";
        ExportStatus    as "export-status"    => Account, ReadOnly,  Spot, Tool
            example "kraken export-status --report trades";
        ExportRetrieve  as "export-retrieve"  => Account, Dangerous, Spot, Tool
            example "kraken export-retrieve ABCD";
        ExportDelete    as "export-delete"    => Account, Dangerous, Spot, Tool
            example "kraken export-delete ABCD";
    }
}
