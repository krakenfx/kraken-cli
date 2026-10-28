//! `kraken paper`.

use clap::Subcommand;
use rust_decimal::Decimal;

/// The `paper` family: the spot simulator. `init`/`reset`/`status`/`balance`
/// are aliases of the `workspace` verbs; only `trade` simulates fills.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PaperCommand {
    /// Create AND fund a paper workspace — `kraken workspace create --mode
    /// paper` under another name.
    ///
    /// One implementation, two spellings. Takes the workspace name positionally
    /// or from `KRAKEN_WORKSPACE`; with neither it funds the global paper
    /// account and says so on stderr. `--balance` is an alias for
    /// `--capital`.
    Init(Init),
    /// Return the workspace to its starting capital — `kraken workspace
    /// reset` under another name.
    Reset(Reset),
    /// Workspace balances — `kraken workspace balance` under another name.
    Balance(Balance),
    /// Workspace summary with P&L — `kraken workspace status` under another
    /// name.
    Status(Status),
    #[command(flatten)]
    Trade(PaperTradeCommand),
}

// The lifecycle verbs above are aliases kept for muscle memory: each Execute
// impl constructs the corresponding `workspace` command and runs it, so there
// is exactly one implementation. They live outside `Transact` deliberately —
// the workspace machinery opens the journal itself, and running it under the
// trade path's already-held writer lock would self-reject.

/// The simulator verbs that touch the journal. Grouped so the family can open
/// the account once and run them all inside that one transaction.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PaperTradeCommand {
    /// Place a paper buy order.
    Buy(Buy),
    /// Place a paper sell order.
    Sell(Sell),
    /// Show open paper limit orders.
    Orders(Orders),
    /// Cancel a paper limit order.
    Cancel(Cancel),
    /// Cancel all paper limit orders.
    CancelAll(CancelAll),
    /// Show paper trade history.
    History(History),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Init {
    /// Workspace name to create (defaults to the active workspace, else
    /// the global paper account).
    pub name: Option<String>,
    /// Starting capital (default: 10000).
    #[arg(long, alias = "balance", default_value = "10000")]
    #[serde(default = "starting_capital")]
    pub capital: Decimal,
    /// Capital currency.
    #[arg(long, default_value = "USD")]
    #[serde(default = "crate::value_types::usd")]
    pub currency: String,
    /// Fee rate as a decimal (default: 0.0026 = 0.26% Kraken Starter tier).
    #[arg(long)]
    pub fee_rate: Option<Decimal>,
    /// Slippage rate as a decimal (default: 0.0 = no slippage simulation).
    #[arg(long, alias = "slippage")]
    pub slippage_rate: Option<Decimal>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Reset {
    /// Workspace name (defaults to the active workspace, else the global
    /// paper account).
    pub name: Option<String>,
    /// Re-parameterize on reset: new starting capital (public-compatible
    /// alias: --balance).
    #[arg(long, alias = "balance")]
    pub capital: Option<Decimal>,
    /// New capital currency.
    #[arg(long)]
    pub currency: Option<String>,
    /// New fee rate as a decimal.
    #[arg(long)]
    pub fee_rate: Option<Decimal>,
    /// New slippage rate as a decimal.
    #[arg(long, alias = "slippage")]
    pub slippage_rate: Option<Decimal>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Balance {
    /// Workspace name (defaults to the active workspace, else the global
    /// paper account).
    pub name: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Status {
    /// Workspace name (defaults to the active workspace, else the global
    /// paper account).
    pub name: Option<String>,
}

// The Args structs below carry no struct-level rustdoc on purpose: clap folds
// type-level docs into `--help`, and the about lines must keep coming from the
// enum variants alone so the help text stays exactly as it was.

// Buy and sell take the same order parameters, but the executing crate
// implements its work trait per variant type, so it needs a distinct type per
// variant — so thin wrappers flatten this one definition.
#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct OrderRequest {
    /// Trading pair (e.g., BTCUSD).
    pub pair: String,
    /// Order volume.
    pub volume: String,
    /// Order type: market (default) or limit.
    #[arg(long, default_value = "market")]
    #[serde(default = "market_order")]
    pub r#type: String,
    /// Limit price (required for limit orders).
    #[arg(long)]
    pub price: Option<String>,
    /// Rationale to record in the decision log: the active session's when one
    /// is recording, else the workspace's.
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Buy {
    #[command(flatten)]
    #[serde(flatten)]
    pub trade: OrderRequest,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Sell {
    #[command(flatten)]
    #[serde(flatten)]
    pub trade: OrderRequest,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Orders {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Cancel {
    /// Order ID (e.g., PAPER-00001).
    pub order_id: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CancelAll {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct History {}

/// The capital a paper workspace starts with.
fn starting_capital() -> Decimal {
    Decimal::from(10_000)
}

/// The order type a paper trade assumes when the caller names none. Unlike the
/// real venue's `limit`, the simulator fills at the live price.
fn market_order() -> String {
    "market".to_owned()
}

crate::tools! {
    /// The `kraken paper` commands.
    fn paper in ["paper"] of PaperCommand {
        Init    as "init"    => Paper, Local,     None, Tool
            example "kraken paper init sandbox --capital 10000 -o json";
        Reset   as "reset"   => Paper, Dangerous, None, Tool
            example "kraken paper reset sandbox -o json";
        Balance as "balance" => Paper, ReadOnly,  None, Tool
            example "kraken paper balance sandbox -o json";
        Status  as "status"  => Paper, ReadOnly,  None, Tool
            example "kraken paper status sandbox -o json";
    }
}

crate::tools! {
    /// The `kraken paper trade` commands.
    fn paper_trade in ["paper", "trade"] of PaperTradeCommand {
        Buy       as "buy"        => Paper, Local,    None, Tool
            example "kraken paper buy BTC/USD 0.01 --type limit --price 50000";
        Sell      as "sell"       => Paper, Local,    None, Tool
            example "kraken paper sell BTC/USD 0.01 --type market";
        Orders    as "orders"     => Paper, ReadOnly, None, Tool
            example "kraken paper orders";
        Cancel    as "cancel"     => Paper, Local,    None, Tool
            example "kraken paper cancel abc123";
        CancelAll as "cancel-all" => Paper, Local,    None, Tool
            example "kraken paper cancel-all";
        History   as "history"    => Paper, ReadOnly, None, Tool
            example "kraken paper history -o json";
    }
}
