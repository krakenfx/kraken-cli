//! `kraken futures`.

use crate::value_types::{ContractType, FuturesOrderType, TrailingUnit, TriggerSignal};
use clap::Subcommand;

/// Boolean-like value for CLI args (accepts true/false, yes/no, 1/0).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum BoolValue {
    #[value(aliases = ["yes", "1"])]
    True,
    #[value(aliases = ["no", "0"])]
    False,
}

/// The `kraken futures` surface: request/response commands (flattened), plus
/// the streaming WS subtree and the paper engine, which route outside the
/// shared executor — mirroring how top-level `ws` and `paper` are siblings of
/// the request/response tree.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FuturesCommand {
    #[command(flatten)]
    Market(FuturesMarketCommand),

    #[command(flatten)]
    Trading(FuturesTradingCommand),

    /// Futures WebSocket streaming commands.
    #[command(subcommand)]
    Ws(super::futures_ws::FuturesWsCommand),

    /// Futures paper trading (simulated, no real money).
    #[command(subcommand)]
    Paper(super::futures_paper::FuturesPaperCommand),
}

/// Public futures market data (no credentials required).
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FuturesMarketCommand {
    /// List all futures instruments/contracts.
    Instruments(Instruments),
    /// Get all futures tickers.
    Tickers(Tickers),
    /// Get ticker for a single contract.
    Ticker(Ticker),
    /// Get futures order book.
    Orderbook(Orderbook),
    /// Get recent futures trade history.
    History(History),
    /// Get fee schedules (volumes and fees per contract).
    Feeschedules(Feeschedules),
    /// Get instrument status (all or per symbol).
    InstrumentStatus(InstrumentStatus),
    /// Get historical funding rates.
    HistoricalFundingRates(HistoricalFundingRates),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Instruments {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Tickers {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Ticker {
    /// Futures symbol (e.g. PI_XBTUSD).
    pub symbol: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Orderbook {
    /// Futures symbol.
    pub symbol: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct History {
    /// Futures symbol.
    pub symbol: String,
    /// Fetch history since this timestamp.
    #[arg(long)]
    pub since: Option<String>,
    /// Fetch history before this timestamp.
    #[arg(long)]
    pub before: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Feeschedules {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct InstrumentStatus {
    /// Specific symbol to query (omit for all).
    #[arg(long)]
    pub symbol: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct HistoricalFundingRates {
    /// Futures symbol (e.g. PI_XBTUSD).
    pub symbol: String,
}

/// Authenticated futures REST: account data, orders, transfers, subaccounts.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FuturesTradingCommand {
    /// Get trading instruments (optional filter by contract type) (auth
    /// required).
    TradingInstruments(TradingInstruments),
    /// Get futures account/wallet info (auth required).
    Accounts(Accounts),
    /// Get open futures orders (auth required).
    OpenOrders(OpenOrders),
    /// Get status of orders by ID (auth required).
    OrderStatus(OrderStatus),
    /// Place a futures order (auth required).
    #[command(subcommand)]
    Order(FuturesOrderDirection),
    /// Edit an existing futures order (auth required).
    EditOrder(EditOrder),
    /// Cancel a futures order by order_id or client order id (auth required).
    Cancel(Cancel),
    /// Cancel all futures orders (auth required).
    CancelAll(CancelAll),
    /// Dead man's switch: cancel all orders after timeout (auth required).
    CancelAfter(CancelAfter),
    /// Place a batch of futures orders (auth required).
    BatchOrder(BatchOrder),
    /// Get open futures positions (auth required).
    Positions(Positions),
    /// Get recent fills (auth required).
    Fills(Fills),
    /// Get current leverage preference (auth required).
    Leverage(Leverage),
    /// Set leverage preference for a symbol (auth required).
    SetLeverage(SetLeverage),
    /// Get PnL preferences (auth required).
    PnlPreferences(PnlPreferences),
    /// Set PnL preference for a symbol (auth required).
    SetPnlPreference(SetPnlPreference),
    /// Get notifications (auth required).
    Notifications(Notifications),
    /// Get execution history (auth required).
    HistoryExecutions(HistoryExecutions),
    /// Get order history (auth required).
    HistoryOrders(HistoryOrders),
    /// Get trigger history (auth required).
    HistoryTriggers(HistoryTriggers),
    /// Download account log as CSV (auth required).
    HistoryAccountLogCsv(HistoryAccountLogCsv),
    /// Get futures transfer history (auth required).
    Transfers(Transfers),
    /// Transfer funds between spot and futures wallets (auth required).
    Transfer(Transfer),
    /// Get unwind queue (auth required).
    UnwindQueue(UnwindQueue),
    /// Get current assignment programs (auth required).
    AssignmentPrograms(AssignmentPrograms),
    /// Get fee schedule volumes (auth required).
    FeeScheduleVolumes(FeeScheduleVolumes),
    /// Get subaccounts (auth required).
    Subaccounts(Subaccounts),
    /// Get trading-enabled status for a subaccount (auth required).
    SubaccountStatus(SubaccountStatus),
    /// Set trading-enabled for a subaccount (auth required).
    SetSubaccountStatus(SetSubaccountStatus),
    /// Transfer between wallets (auth required).
    WalletTransfer(WalletTransfer),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct TradingInstruments {
    /// Filter by contract type.
    #[arg(long)]
    pub contract_type: Option<ContractType>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Accounts {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OpenOrders {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OrderStatus {
    /// Order ID to query.
    #[arg(required = true, num_args = 1.., value_name = "ORDER_ID")]
    pub order_ids: Vec<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct EditOrder {
    /// Order ID to edit.
    #[arg(long)]
    pub order_id: String,
    /// New order size.
    #[arg(long)]
    pub size: Option<String>,
    /// New limit price.
    #[arg(long)]
    pub price: Option<String>,
    /// New stop price.
    #[arg(long)]
    pub stop_price: Option<String>,
}

// `required_unless_present` on each side references the other, so both fields
// must stay in this one struct with these identifiers.
#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Cancel {
    /// Exchange order ID.
    #[arg(long, required_unless_present = "cli_ord_id")]
    pub order_id: Option<String>,
    /// Client order ID.
    #[arg(long, required_unless_present = "order_id")]
    pub cli_ord_id: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CancelAll {
    /// Filter by symbol.
    #[arg(long)]
    pub symbol: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CancelAfter {
    /// Timeout in seconds (0 to disable).
    pub timeout: u64,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BatchOrder {
    /// Orders as JSON array or path to JSON file (prefix with @).
    pub orders_json: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Positions {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Fills {
    /// Filter since this timestamp.
    #[arg(long)]
    pub since: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Leverage {
    /// Filter by symbol (omit for all).
    #[arg(long)]
    pub symbol: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SetLeverage {
    /// Futures symbol.
    pub symbol: String,
    /// Max leverage value (omit to use cross margin).
    #[arg(required = false)]
    pub leverage: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct PnlPreferences {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SetPnlPreference {
    /// Futures symbol (e.g. PF_XBTUSD).
    pub symbol: String,
    /// PnL preference asset (e.g. USD).
    pub preference: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Notifications {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct HistoryWindow {
    /// Filter since this timestamp.
    #[arg(long)]
    pub since: Option<String>,
    /// Filter before this timestamp.
    #[arg(long)]
    pub before: Option<String>,
    /// Sort order (asc or desc).
    #[arg(long)]
    pub sort: Option<String>,
}

// the executing crate implements its work trait per variant type, so each needs
// a distinct one, so each history command wraps the shared filter instead of
// reusing one Args struct three times.
#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct HistoryExecutions {
    #[command(flatten)]
    #[serde(flatten)]
    pub filter: HistoryWindow,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct HistoryOrders {
    #[command(flatten)]
    #[serde(flatten)]
    pub filter: HistoryWindow,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct HistoryTriggers {
    #[command(flatten)]
    #[serde(flatten)]
    pub filter: HistoryWindow,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct HistoryAccountLogCsv {
    /// Filter since this timestamp.
    #[arg(long)]
    pub since: Option<String>,
    /// Filter before this timestamp.
    #[arg(long)]
    pub before: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Transfers {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Transfer {
    /// Amount.
    pub amount: String,
    /// Currency (e.g. USD, XBT).
    pub currency: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct UnwindQueue {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct AssignmentPrograms {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct FeeScheduleVolumes {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Subaccounts {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SubaccountStatus {
    /// Subaccount UID.
    pub subaccount_uid: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SetSubaccountStatus {
    /// Subaccount UID.
    pub subaccount_uid: String,
    /// Enable or disable trading: `true` or `false` (the command line also
    /// accepts yes/no and 1/0, which a tool call's schema does not).
    pub trading_enabled: BoolValue,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct WalletTransfer {
    /// Source account.
    pub from_account: String,
    /// Destination account.
    pub to_account: String,
    /// Currency unit (e.g. USD, XBT).
    pub unit: String,
    /// Amount to transfer.
    pub amount: String,
}

/// Which side a futures order takes. A nested enum rather than a flag so the
/// two sides are distinct types with their own work impls.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FuturesOrderDirection {
    /// Place a buy order.
    Buy(Buy),
    /// Place a sell order.
    Sell(Sell),
}

// Shared by buy and sell; the side comes from the variant.
#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OrderRequest {
    /// Futures symbol.
    pub symbol: String,
    /// Order size.
    pub size: String,
    /// Order type.
    #[arg(long, default_value = "limit")]
    #[serde(default = "crate::value_types::limit_order")]
    pub r#type: FuturesOrderType,
    /// Limit price.
    #[arg(long)]
    pub price: Option<String>,
    /// Stop price (for stop and take-profit orders).
    #[arg(long)]
    pub stop_price: Option<String>,
    /// Trigger signal: mark, index, or last.
    #[arg(long)]
    pub trigger_signal: Option<TriggerSignal>,
    /// Client order ID for correlation.
    #[arg(long)]
    pub client_order_id: Option<String>,
    /// Reduce-only flag.
    #[arg(long)]
    #[serde(default)]
    pub reduce_only: bool,
    /// Trailing stop max deviation.
    #[arg(long)]
    pub trailing_stop_max_deviation: Option<String>,
    /// Trailing stop deviation unit.
    #[arg(long)]
    pub trailing_stop_deviation_unit: Option<TrailingUnit>,
}

// the executing crate implements its work trait per variant type, so each needs
// a distinct one, so buy and sell each wrap the shared fields instead of
// reusing one Args struct twice.
#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Buy {
    #[command(flatten)]
    #[serde(flatten)]
    pub order: OrderRequest,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Sell {
    #[command(flatten)]
    #[serde(flatten)]
    pub order: OrderRequest,
}

crate::tools! {
    /// The `kraken futures market` commands.
    fn futures_market in ["futures", "market"] of FuturesMarketCommand {
        Instruments            as "instruments"              => Futures, ReadOnly, None, Tool
            example "kraken futures instruments -o json";
        Tickers                as "tickers"                  => Futures, ReadOnly, None, Tool
            example "kraken futures tickers";
        Ticker                 as "ticker"                   => Futures, ReadOnly, None, Tool
            example "kraken futures ticker PF_XBTUSD";
        Orderbook              as "orderbook"                => Futures, ReadOnly, None, Tool
            example "kraken futures orderbook PF_XBTUSD";
        History                as "history"                  => Futures, ReadOnly, None, Tool
            example "kraken futures history PF_XBTUSD";
        Feeschedules           as "feeschedules"             => Futures, ReadOnly, None, Tool
            example "kraken futures feeschedules -o json";
        InstrumentStatus       as "instrument-status"        => Futures, ReadOnly, None, Tool
            example "kraken futures instrument-status --symbol PF_XBTUSD";
        HistoricalFundingRates as "historical-funding-rates" => Futures, ReadOnly, None, Tool
            example "kraken futures historical-funding-rates PI_XBTUSD";
    }
}

crate::tools! {
    /// The `kraken futures trading` commands.
    fn futures_trading in ["futures", "trading"] of FuturesTradingCommand {
        TradingInstruments   as "trading-instruments"     => Futures, ReadOnly,  Futures, Tool
            example "kraken futures trading-instruments --contract-type futures_inverse";
        Accounts             as "accounts"                => Futures, ReadOnly,  Futures, Tool
            example "kraken futures accounts -o json";
        OpenOrders           as "open-orders"             => Futures, ReadOnly,  Futures, Tool
            example "kraken futures open-orders";
        OrderStatus          as "order-status"            => Futures, ReadOnly,  Futures, Tool
            where NonEmpty("order_ids")
            example "kraken futures order-status abc123 def456";
        EditOrder            as "edit-order"              => Futures, Dangerous, Futures, Tool
            example "kraken futures edit-order --order-id abc123 --price 52000";
        Cancel               as "cancel"                  => Futures, Dangerous, Futures, Tool
            where AtLeastOne(&["order_id", "cli_ord_id"])
            example "kraken futures cancel --order-id abc123";
        CancelAll            as "cancel-all"              => Futures, Dangerous, Futures, Tool
            example "kraken futures cancel-all --yes";
        CancelAfter          as "cancel-after"            => Futures, Dangerous, Futures, Tool
            example "kraken futures cancel-after 120";
        BatchOrder           as "batch-order"             => Futures, Dangerous, Futures, Tool
            example "kraken futures batch-order futures-orders.json";
        Positions            as "positions"               => Futures, ReadOnly,  Futures, Tool
            example "kraken futures positions -o json";
        Fills                as "fills"                   => Futures, ReadOnly,  Futures, Tool
            example "kraken futures fills --since 2025-01-01";
        Leverage             as "leverage"                => Futures, ReadOnly,  Futures, Tool
            example "kraken futures leverage --symbol PF_XBTUSD";
        SetLeverage          as "set-leverage"            => Futures, Dangerous, Futures, Tool
            example "kraken futures set-leverage PF_XBTUSD 5";
        PnlPreferences       as "pnl-preferences"         => Futures, ReadOnly,  Futures, Tool
            example "kraken futures pnl-preferences";
        SetPnlPreference     as "set-pnl-preference"      => Futures, Dangerous, Futures, Tool
            example "kraken futures set-pnl-preference PF_XBTUSD USD";
        Notifications        as "notifications"           => Futures, ReadOnly,  Futures, Tool
            example "kraken futures notifications";
        HistoryExecutions    as "history-executions"      => Futures, ReadOnly,  Futures, Tool
            example "kraken futures history-executions --since 2025-01-01";
        HistoryOrders        as "history-orders"          => Futures, ReadOnly,  Futures, Tool
            example "kraken futures history-orders --since 2025-01-01";
        HistoryTriggers      as "history-triggers"        => Futures, ReadOnly,  Futures, Tool
            example "kraken futures history-triggers";
        HistoryAccountLogCsv as "history-account-log-csv" => Futures, ReadOnly,  Futures, Tool
            example "kraken futures history-account-log-csv --since 2025-01-01";
        Transfers            as "transfers"               => Futures, ReadOnly,  Futures, Tool
            example "kraken futures transfers -o json";
        Transfer             as "transfer"                => Futures, Dangerous, Futures, Tool
            example "kraken futures transfer 1000 USD";
        UnwindQueue          as "unwind-queue"            => Futures, ReadOnly,  Futures, Tool
            example "kraken futures unwind-queue";
        AssignmentPrograms   as "assignment-programs"     => Futures, ReadOnly,  Futures, Tool
            example "kraken futures assignment-programs";
        FeeScheduleVolumes   as "fee-schedule-volumes"    => Futures, ReadOnly,  Futures, Tool
            example "kraken futures fee-schedule-volumes";
        Subaccounts          as "subaccounts"             => Futures, ReadOnly,  Futures, Tool
            example "kraken futures subaccounts";
        SubaccountStatus     as "subaccount-status"       => Futures, ReadOnly,  Futures, Tool
            example "kraken futures subaccount-status abc123";
        SetSubaccountStatus  as "set-subaccount-status"   => Futures, Dangerous, Futures, Tool
            example "kraken futures set-subaccount-status abc123 true";
        WalletTransfer       as "wallet-transfer"         => Futures, Dangerous, Futures, Tool
            example "kraken futures wallet-transfer main flex USD 100";
    }
}

crate::tools! {
    /// The `kraken futures trading order` commands.
    fn futures_trading_order in ["futures", "trading", "order"] of FuturesOrderDirection {
        Buy  as "buy"  => Futures, Dangerous, Futures, Tool
            example "kraken futures order buy PF_XBTUSD 1 --type limit --price 50000";
        Sell as "sell" => Futures, Dangerous, Futures, Tool
            example "kraken futures order sell PF_XBTUSD 1 --type limit --price 100000";
    }
}
