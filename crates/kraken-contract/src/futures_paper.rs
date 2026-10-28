//! `kraken futures_paper`.

use crate::value_types::{FuturesOrderType, TriggerSignal};
use clap::Subcommand;

/// The `futures paper` family: the futures simulator's own verbs, over state
/// that is still global rather than workspace-scoped.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FuturesPaperCommand {
    /// Initialize futures paper trading account.
    Init(Init),
    /// Reset futures paper account.
    Reset(Reset),
    /// Show futures paper collateral balance.
    Balance(Balance),
    /// Show futures paper account summary.
    Status(Status),
    /// Place a futures paper buy (long) order.
    Buy(Buy),
    /// Place a futures paper sell (short) order.
    Sell(Sell),
    /// Show open futures paper orders.
    Orders(Orders),
    /// Get status of a specific order.
    OrderStatus(OrderStatus),
    /// Edit a resting futures paper order.
    EditOrder(EditOrder),
    /// Cancel a futures paper order.
    Cancel(Cancel),
    /// Cancel all futures paper orders.
    CancelAll(CancelAll),
    /// Place a batch of futures paper orders (JSON array).
    BatchOrder(BatchOrder),
    /// Show open futures paper positions.
    Positions(Positions),
    /// Show futures paper fill history.
    Fills(Fills),
    /// Show futures paper account history.
    History(History),
    /// Get current leverage preferences.
    Leverage(Leverage),
    /// Set leverage preference for a symbol.
    SetLeverage(SetLeverage),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Init {
    /// Starting collateral balance (default: 10000).
    #[arg(long, default_value = "10000")]
    #[serde(default = "starting_balance")]
    pub balance: rust_decimal::Decimal,
    /// Collateral currency (default: USD).
    #[arg(long, default_value = "USD")]
    #[serde(default = "crate::value_types::usd")]
    pub currency: String,
    /// Fee rate as a decimal (default: 0.0005 = 0.05%).
    #[arg(long)]
    pub fee_rate: Option<rust_decimal::Decimal>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Reset {
    #[arg(long)]
    pub balance: Option<rust_decimal::Decimal>,
    #[arg(long)]
    pub currency: Option<String>,
    #[arg(long)]
    pub fee_rate: Option<rust_decimal::Decimal>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Balance {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Status {}

// Buy and sell share one order surface; each side keeps a thin wrapper because
// enum_dispatch needs a distinct type per variant. The flatten field comes
// first so the positionals (symbol, size) keep their order.
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

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OrderRequest {
    /// Futures symbol (e.g. PF_XBTUSD).
    pub symbol: String,
    /// Order size.
    pub size: String,
    /// Order type (default: limit).
    #[arg(long, default_value = "limit")]
    #[serde(default = "crate::value_types::limit_order")]
    pub r#type: FuturesOrderType,
    /// Limit price.
    #[arg(long)]
    pub price: Option<String>,
    /// Stop/trigger price.
    #[arg(long)]
    pub stop_price: Option<String>,
    /// Trigger signal: mark, index, or last.
    #[arg(long)]
    pub trigger_signal: Option<TriggerSignal>,
    /// Client order ID.
    #[arg(long)]
    pub client_order_id: Option<String>,
    /// Reduce-only flag.
    #[arg(long)]
    #[serde(default)]
    pub reduce_only: bool,
    /// Leverage override for this order.
    #[arg(long)]
    pub leverage: Option<String>,
    /// Trailing stop max deviation.
    #[arg(long)]
    pub trailing_stop_max_deviation: Option<String>,
    /// Trailing stop deviation unit (percent or quote_currency).
    #[arg(long)]
    pub trailing_stop_deviation_unit: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Orders {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OrderStatus {
    /// Order ID to query.
    pub order_id: String,
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

// order_id/cli_ord_id are a required_unless_present pair; clap resolves those
// by sibling field id, so both fields must stay in this one struct.
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
pub struct BatchOrder {
    /// Orders as JSON array or path to JSON file (prefix with @). Each row
    /// takes `side` (buy or sell) and the `futures paper buy` arguments by
    /// name: `symbol`, `size`, `type`, `price`, `leverage` and so on.
    pub orders_json: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Positions {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Fills {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct History {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Leverage {
    #[arg(long)]
    pub symbol: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SetLeverage {
    /// Futures symbol.
    pub symbol: String,
    /// Max leverage value.
    pub leverage: String,
}

/// The collateral a futures paper account starts with.
fn starting_balance() -> rust_decimal::Decimal {
    rust_decimal::Decimal::from(10_000)
}

crate::tools! {
    /// The `kraken futures paper` commands.
    fn futures_paper in ["futures", "paper"] of FuturesPaperCommand {
        Init        as "init"         => FuturesPaper, Local,    None, Tool
            example "kraken futures paper init --balance 10000 --currency USD";
        Reset       as "reset"        => FuturesPaper, Local,    None, Tool
            example "kraken futures paper reset";
        Balance     as "balance"      => FuturesPaper, ReadOnly, None, Tool
            example "kraken futures paper balance";
        Status      as "status"       => FuturesPaper, ReadOnly, None, Tool
            example "kraken futures paper status";
        Buy         as "buy"          => FuturesPaper, Local,    None, Tool
            example "kraken futures paper buy PF_XBTUSD 1 --leverage 10 --type market";
        Sell        as "sell"         => FuturesPaper, Local,    None, Tool
            example "kraken futures paper sell PF_ETHUSD 5 --leverage 20 --type market";
        Orders      as "orders"       => FuturesPaper, ReadOnly, None, Tool
            example "kraken futures paper orders";
        OrderStatus as "order-status" => FuturesPaper, ReadOnly, None, Tool
            example "kraken futures paper order-status abc123";
        EditOrder   as "edit-order"   => FuturesPaper, Local,    None, Tool
            example "kraken futures paper edit-order --order-id abc123 --size 2 --price 51000";
        Cancel      as "cancel"       => FuturesPaper, Local,    None, Tool
            where AtLeastOne(&["order_id", "cli_ord_id"])
            example "kraken futures paper cancel --order-id abc123";
        CancelAll   as "cancel-all"   => FuturesPaper, Local,    None, Tool
            example "kraken futures paper cancel-all --symbol PF_XBTUSD";
        BatchOrder  as "batch-order"  => FuturesPaper, Local,    None, Tool
            example "kraken futures paper batch-order '[{\"symbol\":\"PF_XBTUSD\",\"side\":\"buy\",\"size\":1,\"type\":\"limit\",\"price\":50000,\"leverage\":10}]'";
        Positions   as "positions"    => FuturesPaper, ReadOnly, None, Tool
            example "kraken futures paper positions";
        Fills       as "fills"        => FuturesPaper, ReadOnly, None, Tool
            example "kraken futures paper fills";
        History     as "history"      => FuturesPaper, ReadOnly, None, Tool
            example "kraken futures paper history";
        Leverage    as "leverage"     => FuturesPaper, ReadOnly, None, Tool
            example "kraken futures paper leverage";
        SetLeverage as "set-leverage" => FuturesPaper, Local,    None, Tool
            example "kraken futures paper set-leverage PF_XBTUSD 10";
    }
}
