/// Futures commands: public market data, private trading, and account management.
use std::collections::HashMap;

use clap::Subcommand;
use enum_dispatch::enum_dispatch;
use kraken_core::OrderSide;
use serde_json::Value;

use super::Execute;
use super::value_types::{CliWire, ContractType, TrailingUnit};
use crate::cli::AppContext;
use crate::errors::{KrakenError, Result};
use crate::futures_paper::{FuturesOrderType, TriggerSignal};
use crate::output::CommandOutput;
use crate::output::json_field;

/// Boolean-like value for CLI args (accepts true/false, yes/no, 1/0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum BoolValue {
    #[value(aliases = ["yes", "1"])]
    True,
    #[value(aliases = ["no", "0"])]
    False,
}

impl BoolValue {
    /// Returns "true" or "false" for API request bodies.
    pub(crate) fn as_api_value(self) -> &'static str {
        match self {
            BoolValue::True => "true",
            BoolValue::False => "false",
        }
    }
}

/// The `kraken futures` surface: request/response commands (flattened), plus
/// the streaming WS subtree and the paper engine, which route outside the
/// shared executor — mirroring how top-level `ws` and `paper` are siblings of
/// the request/response tree.
#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum FuturesCommand {
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
#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum FuturesMarketCommand {
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

#[derive(Debug, clap::Args)]
pub(crate) struct Instruments {}

impl Execute for Instruments {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.futures()?;
        let data = client.public_get("instruments", &[]).await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Tickers {}

impl Execute for Tickers {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.futures()?;
        let data = client.public_get("tickers", &[]).await?;
        Ok(parse_tickers(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Ticker {
    /// Futures symbol (e.g. PI_XBTUSD).
    symbol: String,
}

impl Execute for Ticker {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.futures()?;
        let Self { symbol } = self;
        validate_path_segment(&symbol, "symbol")?;
        let endpoint = format!("tickers/{symbol}");
        let data = client.public_get(&endpoint, &[]).await?;
        Ok(parse_ticker(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Orderbook {
    /// Futures symbol.
    symbol: String,
}

impl Execute for Orderbook {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.futures()?;
        let Self { symbol } = self;
        validate_path_segment(&symbol, "symbol")?;
        let data = client
            .public_get("orderbook", &[("symbol", symbol.as_str())])
            .await?;
        Ok(parse_orderbook(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct History {
    /// Futures symbol.
    symbol: String,
    /// Fetch history since this timestamp.
    #[arg(long)]
    since: Option<String>,
    /// Fetch history before this timestamp.
    #[arg(long)]
    before: Option<String>,
}

impl Execute for History {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.futures()?;
        let Self {
            symbol,
            since,
            before,
        } = self;
        validate_path_segment(&symbol, "symbol")?;
        let mut params: Vec<(&str, &str)> = vec![("symbol", symbol.as_str())];
        if let Some(s) = since.as_deref() {
            params.push(("since", s));
        }
        if let Some(b) = before.as_deref() {
            params.push(("before", b));
        }
        let data = client.public_get("history", &params).await?;
        Ok(parse_trade_history(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Feeschedules {}

impl Execute for Feeschedules {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.futures()?;
        let data = client.public_get("feeschedules", &[]).await?;
        Ok(parse_feeschedules(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct InstrumentStatus {
    /// Specific symbol to query (omit for all).
    #[arg(long)]
    symbol: Option<String>,
}

impl Execute for InstrumentStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.futures()?;
        let Self { symbol } = self;
        let endpoint = match symbol {
            Some(s) => {
                validate_path_segment(&s, "symbol")?;
                format!("instruments/{s}/status")
            }
            None => "instruments/status".to_string(),
        };
        let data = client.public_get(&endpoint, &[]).await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct HistoricalFundingRates {
    /// Futures symbol (e.g. PI_XBTUSD).
    symbol: String,
}

impl Execute for HistoricalFundingRates {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let client = ctx.futures()?;
        let Self { symbol } = self;
        validate_path_segment(&symbol, "symbol")?;
        let params = [("symbol", symbol.as_str())];
        let data = client
            .public_get("historical-funding-rates", &params)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

/// Authenticated futures REST: account data, orders, transfers, subaccounts.
#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum FuturesTradingCommand {
    /// Get trading instruments (optional filter by contract type) (auth required).
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

#[derive(Debug, clap::Args)]
pub(crate) struct TradingInstruments {
    /// Filter by contract type.
    #[arg(long)]
    contract_type: Option<ContractType>,
}

impl Execute for TradingInstruments {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { contract_type } = self;
        let mut params: Vec<(&str, &str)> = Vec::new();
        let ct_wire;
        if let Some(ct) = contract_type {
            ct_wire = ct.as_wire();
            params.push(("contractType", &ct_wire));
        }
        let data = client
            .private_get("trading/instruments", &params, creds)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Accounts {}

impl Execute for Accounts {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let data = client.private_get("accounts", &[], creds).await?;
        Ok(parse_accounts(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct OpenOrders {}

impl Execute for OpenOrders {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let data = client.private_get("openorders", &[], creds).await?;
        Ok(parse_open_orders(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct OrderStatus {
    /// Order ID to query.
    #[arg(required = true, num_args = 1.., value_name = "ORDER_ID")]
    order_ids: Vec<String>,
}

impl Execute for OrderStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { order_ids } = self;
        let mut params = HashMap::new();
        params.insert("orderIds".into(), order_ids.join(","));
        let data = client.private_post("orders/status", params, creds).await?;
        Ok(parse_order_status(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct EditOrder {
    /// Order ID to edit.
    #[arg(long)]
    order_id: String,
    /// New order size.
    #[arg(long)]
    size: Option<String>,
    /// New limit price.
    #[arg(long)]
    price: Option<String>,
    /// New stop price.
    #[arg(long)]
    stop_price: Option<String>,
}

impl Execute for EditOrder {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self {
            order_id,
            size,
            price,
            stop_price,
        } = self;
        let mut params = HashMap::new();
        params.insert("orderId".into(), order_id);
        if let Some(s) = size {
            params.insert("size".into(), s);
        }
        if let Some(p) = price {
            params.insert("limitPrice".into(), p);
        }
        if let Some(sp) = stop_price {
            params.insert("stopPrice".into(), sp);
        }
        let data = client.private_post("editorder", params, creds).await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

// `required_unless_present` on each side references the other, so both fields
// must stay in this one struct with these identifiers.
#[derive(Debug, clap::Args)]
pub(crate) struct Cancel {
    /// Exchange order ID.
    #[arg(long, required_unless_present = "cli_ord_id")]
    order_id: Option<String>,
    /// Client order ID.
    #[arg(long, required_unless_present = "order_id")]
    cli_ord_id: Option<String>,
}

impl Execute for Cancel {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self {
            order_id,
            cli_ord_id,
        } = self;
        let mut params = HashMap::new();
        match (order_id, cli_ord_id) {
            (Some(oid), None) => {
                params.insert("order_id".into(), oid);
            }
            (None, Some(cid)) => {
                params.insert("cliOrdId".into(), cid);
            }
            (Some(_), Some(_)) => {
                return Err(KrakenError::Validation(
                    "Provide exactly one of --order-id or --cli-ord-id".into(),
                ));
            }
            (None, None) => {
                return Err(KrakenError::Validation(
                    "One of --order-id or --cli-ord-id is required".into(),
                ));
            }
        }
        let data = client.private_post("cancelorder", params, creds).await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct CancelAll {
    /// Filter by symbol.
    #[arg(long)]
    symbol: Option<String>,
}

impl Execute for CancelAll {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { symbol } = self;
        ctx.confirm_destructive("Cancel ALL open futures orders?")?;
        let mut params = HashMap::new();
        if let Some(s) = symbol {
            params.insert("symbol".into(), s);
        }
        let data = client
            .private_post("cancelallorders", params, creds)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct CancelAfter {
    /// Timeout in seconds (0 to disable).
    timeout: u64,
}

impl Execute for CancelAfter {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { timeout } = self;
        let mut params = HashMap::new();
        params.insert("timeout".into(), timeout.to_string());
        let data = client
            .private_post("cancelallordersafter", params, creds)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct BatchOrder {
    /// Orders as JSON array or path to JSON file (prefix with @).
    orders_json: String,
}

impl Execute for BatchOrder {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { orders_json } = self;
        let json_str = if let Some(path) = orders_json.strip_prefix('@') {
            std::fs::read_to_string(path).map_err(|e| {
                KrakenError::Io(std::io::Error::new(
                    e.kind(),
                    format!("Failed to read batch order file '{path}': {e}"),
                ))
            })?
        } else {
            orders_json
        };
        let parsed: Value = serde_json::from_str(&json_str)
            .map_err(|e| KrakenError::Validation(format!("Invalid batch order JSON: {e}")))?;
        let data = client
            .private_post_json("batchorder", parsed, creds)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Positions {}

impl Execute for Positions {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let data = client.private_get("openpositions", &[], creds).await?;
        Ok(parse_positions(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Fills {
    /// Filter since this timestamp.
    #[arg(long)]
    since: Option<String>,
}

impl Execute for Fills {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { since } = self;
        let mut params: Vec<(&str, &str)> = Vec::new();
        if let Some(s) = since.as_deref() {
            params.push(("lastFillTime", s));
        }
        let data = client.private_get("fills", &params, creds).await?;
        Ok(parse_fills(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Leverage {
    /// Filter by symbol (omit for all).
    #[arg(long)]
    symbol: Option<String>,
}

impl Execute for Leverage {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { symbol } = self;
        let mut params: Vec<(&str, &str)> = Vec::new();
        if let Some(s) = symbol.as_deref() {
            params.push(("symbol", s));
        }
        let data = client
            .private_get("leveragepreferences", &params, creds)
            .await?;
        Ok(parse_leverage_preferences(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct SetLeverage {
    /// Futures symbol.
    symbol: String,
    /// Max leverage value (omit to use cross margin).
    #[arg(required = false)]
    leverage: Option<String>,
}

impl Execute for SetLeverage {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { symbol, leverage } = self;
        let mut params = HashMap::new();
        params.insert("symbol".into(), symbol);
        if let Some(lev) = leverage {
            params.insert("maxLeverage".into(), lev);
        }
        let data = client
            .private_put("leveragepreferences", params, creds)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct PnlPreferences {}

impl Execute for PnlPreferences {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let data = client.private_get("pnlpreferences", &[], creds).await?;
        Ok(parse_pnl_preferences(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct SetPnlPreference {
    /// Futures symbol (e.g. PF_XBTUSD).
    symbol: String,
    /// PnL preference asset (e.g. USD).
    preference: String,
}

impl Execute for SetPnlPreference {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { symbol, preference } = self;
        let mut params = HashMap::new();
        params.insert("symbol".into(), symbol);
        params.insert("pnlPreference".into(), preference);
        let data = client.private_put("pnlpreferences", params, creds).await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Notifications {}

impl Execute for Notifications {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let data = client.private_get("notifications", &[], creds).await?;
        Ok(parse_notifications(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct HistoryWindow {
    /// Filter since this timestamp.
    #[arg(long)]
    since: Option<String>,
    /// Filter before this timestamp.
    #[arg(long)]
    before: Option<String>,
    /// Sort order (asc or desc).
    #[arg(long)]
    sort: Option<String>,
}

// enum_dispatch needs a distinct type per variant, so each history command
// wraps the shared filter instead of reusing one Args struct three times.
#[derive(Debug, clap::Args)]
pub(crate) struct HistoryExecutions {
    #[command(flatten)]
    filter: HistoryWindow,
}

impl Execute for HistoryExecutions {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        fetch_history(self.filter, "executions", ctx).await
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct HistoryOrders {
    #[command(flatten)]
    filter: HistoryWindow,
}

impl Execute for HistoryOrders {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        fetch_history(self.filter, "orders", ctx).await
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct HistoryTriggers {
    #[command(flatten)]
    filter: HistoryWindow,
}

impl Execute for HistoryTriggers {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        fetch_history(self.filter, "triggers", ctx).await
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct HistoryAccountLogCsv {
    /// Filter since this timestamp.
    #[arg(long)]
    since: Option<String>,
    /// Filter before this timestamp.
    #[arg(long)]
    before: Option<String>,
}

impl Execute for HistoryAccountLogCsv {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { since, before } = self;
        let mut params: Vec<(&str, &str)> = Vec::new();
        if let Some(s) = since.as_deref() {
            params.push(("since", s));
        }
        if let Some(b) = before.as_deref() {
            params.push(("before", b));
        }
        let csv_text = client
            .private_get_raw("accountlogcsv", &params, creds)
            .await?;
        let json_data = serde_json::json!({ "csv": csv_text });
        let total_lines = csv_text.lines().count();
        let summary = if total_lines > 1 {
            format!("{} data rows", total_lines - 1)
        } else {
            "empty".to_string()
        };
        let first_line = csv_text.lines().next().unwrap_or("").to_string();
        Ok(CommandOutput::new(
            json_data,
            vec!["Headers".into(), "Rows".into()],
            vec![vec![first_line, summary]],
        ))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Transfers {}

impl Execute for Transfers {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let data = client.private_get("transfers", &[], creds).await?;
        Ok(parse_transfers(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Transfer {
    /// Amount.
    amount: String,
    /// Currency (e.g. USD, XBT).
    currency: String,
}

impl Execute for Transfer {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { amount, currency } = self;
        ctx.confirm_destructive(&format!(
            "Transfer {amount} {currency} between spot and futures wallets?"
        ))?;
        let mut params = HashMap::new();
        params.insert("amount".into(), amount);
        params.insert("currency".into(), currency);
        let data = client.private_post("withdrawal", params, creds).await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct UnwindQueue {}

impl Execute for UnwindQueue {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let data = client.private_get("unwindqueue", &[], creds).await?;
        Ok(parse_unwind_queue(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct AssignmentPrograms {}

impl Execute for AssignmentPrograms {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let data = client
            .private_get("assignmentprogram/current", &[], creds)
            .await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct FeeScheduleVolumes {}

impl Execute for FeeScheduleVolumes {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let data = client
            .private_get("feeschedules/volumes", &[], creds)
            .await?;
        Ok(parse_fee_schedule_volumes(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Subaccounts {}

impl Execute for Subaccounts {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let data = client.private_get("subaccounts", &[], creds).await?;
        Ok(parse_subaccounts(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct SubaccountStatus {
    /// Subaccount UID.
    subaccount_uid: String,
}

impl Execute for SubaccountStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self { subaccount_uid } = self;
        validate_path_segment(&subaccount_uid, "subaccount_uid")?;
        let endpoint = format!("subaccount/{subaccount_uid}/trading-enabled");
        let data = client.private_get(&endpoint, &[], creds).await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct SetSubaccountStatus {
    /// Subaccount UID.
    subaccount_uid: String,
    /// Enable or disable trading (true/false, yes/no, 1/0).
    trading_enabled: BoolValue,
}

impl Execute for SetSubaccountStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self {
            subaccount_uid,
            trading_enabled,
        } = self;
        validate_path_segment(&subaccount_uid, "subaccount_uid")?;
        let endpoint = format!("subaccount/{subaccount_uid}/trading-enabled");
        let mut params = HashMap::new();
        params.insert(
            "tradingEnabled".into(),
            trading_enabled.as_api_value().to_string(),
        );
        let data = client.private_put(&endpoint, params, creds).await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct WalletTransfer {
    /// Source account.
    from_account: String,
    /// Destination account.
    to_account: String,
    /// Currency unit (e.g. USD, XBT).
    unit: String,
    /// Amount to transfer.
    amount: String,
}

impl Execute for WalletTransfer {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let (client, creds) = ctx.futures_authed()?;
        let Self {
            from_account,
            to_account,
            unit,
            amount,
        } = self;
        ctx.confirm_destructive(&format!(
            "Transfer {amount} {unit} from {from_account} to {to_account}?"
        ))?;
        let mut params = HashMap::new();
        params.insert("fromAccount".into(), from_account);
        params.insert("toAccount".into(), to_account);
        params.insert("unit".into(), unit);
        params.insert("amount".into(), amount);
        let data = client.private_post("transfer", params, creds).await?;
        Ok(CommandOutput::from_untyped(&data))
    }
}

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum FuturesOrderDirection {
    /// Place a buy order.
    Buy(Buy),
    /// Place a sell order.
    Sell(Sell),
}

/// Order fields shared by buy and sell; the side comes from the variant.
#[derive(Debug, clap::Args)]
pub(crate) struct OrderRequest {
    /// Futures symbol.
    symbol: String,
    /// Order size.
    size: String,
    /// Order type.
    #[arg(long, default_value = "limit")]
    r#type: FuturesOrderType,
    /// Limit price.
    #[arg(long)]
    price: Option<String>,
    /// Stop price (for stop and take-profit orders).
    #[arg(long)]
    stop_price: Option<String>,
    /// Trigger signal: mark, index, or last.
    #[arg(long)]
    trigger_signal: Option<TriggerSignal>,
    /// Client order ID for correlation.
    #[arg(long)]
    client_order_id: Option<String>,
    /// Reduce-only flag.
    #[arg(long)]
    reduce_only: bool,
    /// Trailing stop max deviation.
    #[arg(long)]
    trailing_stop_max_deviation: Option<String>,
    /// Trailing stop deviation unit.
    #[arg(long)]
    trailing_stop_deviation_unit: Option<TrailingUnit>,
}

// enum_dispatch needs a distinct type per variant, so buy and sell each wrap
// the shared fields instead of reusing one Args struct twice.
#[derive(Debug, clap::Args)]
pub(crate) struct Buy {
    #[command(flatten)]
    order: OrderRequest,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Sell {
    #[command(flatten)]
    order: OrderRequest,
}

impl Execute for Buy {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        send_futures_order(self.order, OrderSide::Buy, ctx).await
    }
}

impl Execute for Sell {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        send_futures_order(self.order, OrderSide::Sell, ctx).await
    }
}

async fn send_futures_order(
    order: OrderRequest,
    side: OrderSide,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    let (client, creds) = ctx.futures_authed()?;
    let params = build_futures_order(order, side)?;
    let data = client.private_post("sendorder", params, creds).await?;
    Ok(parse_order_response(&data))
}

/// Validates a value for safe interpolation into a URL path segment.
/// Rejects empty strings, path traversal sequences, slashes, and whitespace.
fn validate_path_segment(value: &str, field_name: &str) -> Result<()> {
    if value.is_empty() {
        return Err(KrakenError::Validation(format!(
            "{field_name} cannot be empty"
        )));
    }
    if value.contains('/') {
        return Err(KrakenError::Validation(format!(
            "{field_name} must not contain '/'"
        )));
    }
    if value.contains("..") {
        return Err(KrakenError::Validation(format!(
            "{field_name} must not contain '..'"
        )));
    }
    if value.contains(char::is_whitespace) {
        return Err(KrakenError::Validation(format!(
            "{field_name} must not contain whitespace"
        )));
    }
    if value.chars().any(|c| matches!(c, '?' | '#' | '%' | '\\')) {
        return Err(KrakenError::Validation(format!(
            "{field_name} must not contain reserved URL characters (?, #, %, \\)"
        )));
    }
    Ok(())
}

fn build_futures_order(order: OrderRequest, side: OrderSide) -> Result<HashMap<String, String>> {
    let OrderRequest {
        symbol,
        size,
        r#type: order_type,
        price,
        stop_price,
        trigger_signal,
        client_order_id,
        reduce_only,
        trailing_stop_max_deviation: trailing_max,
        trailing_stop_deviation_unit: trailing_unit,
    } = order;

    let mut params = HashMap::new();
    params.insert("orderType".into(), order_type.as_api_wire().to_string());
    params.insert("symbol".into(), symbol);
    params.insert("side".into(), side.to_string());
    params.insert("size".into(), size);
    if let Some(p) = price {
        params.insert("limitPrice".into(), p);
    }
    if let Some(sp) = stop_price {
        params.insert("stopPrice".into(), sp);
    }
    if let Some(ts) = trigger_signal {
        params.insert("triggerSignal".into(), ts.to_string());
    }
    if let Some(coid) = client_order_id {
        params.insert("cliOrdId".into(), coid);
    }
    if reduce_only {
        params.insert("reduceOnly".into(), "true".into());
    }
    if let Some(tm) = trailing_max {
        params.insert("trailingStopMaxDeviation".into(), tm);
    }
    if let Some(tu) = trailing_unit {
        params.insert("trailingStopDeviationUnit".into(), tu.as_wire());
    }
    Ok(params)
}

/// Shared executor for the three `history-*` endpoints, which take identical filters.
async fn fetch_history(
    filter: HistoryWindow,
    endpoint: &str,
    ctx: &AppContext,
) -> Result<CommandOutput> {
    let (client, creds) = ctx.futures_authed()?;
    let params = build_history_params(
        filter.since.as_deref(),
        filter.before.as_deref(),
        filter.sort.as_deref(),
    );
    let data = client.private_get(endpoint, &params, creds).await?;
    Ok(CommandOutput::from_untyped(&data))
}

fn build_history_params<'a>(
    since: Option<&'a str>,
    before: Option<&'a str>,
    sort: Option<&'a str>,
) -> Vec<(&'a str, &'a str)> {
    let mut params = Vec::new();
    if let Some(s) = since {
        params.push(("since", s));
    }
    if let Some(b) = before {
        params.push(("before", b));
    }
    if let Some(s) = sort {
        params.push(("sort", s));
    }
    params
}

/// Parse orderbook response (GET orderbook) into OrderSide | Price | Size table (asks then bids).
fn parse_orderbook(data: &Value) -> CommandOutput {
    let headers = vec!["OrderSide".into(), "Price".into(), "Size".into()];
    let mut rows = Vec::new();
    let book = data
        .get("orderBook")
        .or_else(|| data.get("order_book"))
        .and_then(|v| v.as_object());
    if let Some(book) = book {
        if let Some(asks) = book.get("asks").and_then(|a| a.as_array()) {
            for level in asks {
                if let Some(arr) = level.as_array()
                    && arr.len() >= 2
                {
                    rows.push(vec!["Ask".into(), arr[0].to_string(), arr[1].to_string()]);
                }
            }
        }
        if let Some(bids) = book.get("bids").and_then(|b| b.as_array()) {
            for level in bids {
                if let Some(arr) = level.as_array()
                    && arr.len() >= 2
                {
                    rows.push(vec!["Bid".into(), arr[0].to_string(), arr[1].to_string()]);
                }
            }
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse order status response (POST orders/status) into order_id | status table.
fn parse_order_status(data: &Value) -> CommandOutput {
    let headers = vec!["order_id".into(), "status".into()];
    let mut rows = Vec::new();
    let orders = data.get("orders").and_then(|o| o.as_array());
    if let Some(orders) = orders {
        for item in orders {
            let order_id = item
                .get("order")
                .and_then(|o| o.get("orderId"))
                .map(value_to_string)
                .unwrap_or_else(|| "-".to_string());
            let status = item
                .get("status")
                .map(value_to_string)
                .unwrap_or_else(|| "-".to_string());
            rows.push(vec![order_id, status]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Format a JSON value as string for balance/amount display.
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Parse accounts response (GET accounts) into account_id | type | currency | balance table.
fn parse_accounts(data: &Value) -> CommandOutput {
    let headers = vec![
        "account_id".into(),
        "type".into(),
        "currency".into(),
        "balance".into(),
    ];
    let mut rows = Vec::new();
    let accounts = data.get("accounts").and_then(|a| a.as_object());
    if let Some(accounts) = accounts {
        for (account_id, account) in accounts {
            let type_str = json_field(account, "type");
            if let Some(balances) = account.get("balances").and_then(|b| b.as_object()) {
                for (currency, amount) in balances {
                    rows.push(vec![
                        account_id.clone(),
                        type_str.clone(),
                        currency.clone(),
                        value_to_string(amount),
                    ]);
                }
            } else if let Some(currencies) = account.get("currencies").and_then(|c| c.as_object()) {
                for (currency, obj) in currencies {
                    let val = obj
                        .get("value")
                        .map(value_to_string)
                        .unwrap_or_else(|| "-".to_string());
                    rows.push(vec![
                        account_id.clone(),
                        type_str.clone(),
                        currency.clone(),
                        val,
                    ]);
                }
            }
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse subaccounts response (GET subaccounts) into accountUid | email | fullName table.
fn parse_subaccounts(data: &Value) -> CommandOutput {
    let headers = vec!["accountUid".into(), "email".into(), "fullName".into()];
    let mut rows = Vec::new();
    let subaccounts = data.get("subaccounts").and_then(|s| s.as_array());
    if let Some(subaccounts) = subaccounts {
        for sa in subaccounts {
            rows.push(vec![
                json_field(sa, "accountUid"),
                json_field(sa, "email"),
                json_field(sa, "fullName"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse open orders response (GET openorders) into order_id | orderType | side | symbol | status table.
fn parse_open_orders(data: &Value) -> CommandOutput {
    let headers = vec![
        "order_id".into(),
        "orderType".into(),
        "side".into(),
        "symbol".into(),
        "status".into(),
    ];
    let mut rows = Vec::new();
    let orders = data
        .get("openOrders")
        .or_else(|| data.get("open_orders"))
        .and_then(|a| a.as_array());
    if let Some(orders) = orders {
        for order in orders {
            rows.push(vec![
                json_field(order, "order_id"),
                json_field(order, "orderType"),
                json_field(order, "side"),
                json_field(order, "symbol"),
                json_field(order, "status"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse public trade history response (GET history) into trade_id | time | side | price | size | type table.
fn parse_trade_history(data: &Value) -> CommandOutput {
    let headers = vec![
        "trade_id".into(),
        "time".into(),
        "side".into(),
        "price".into(),
        "size".into(),
        "type".into(),
    ];
    let mut rows = Vec::new();
    let history = data.get("history").and_then(|h| h.as_array());
    if let Some(history) = history {
        for trade in history {
            rows.push(vec![
                json_field(trade, "trade_id"),
                json_field(trade, "time"),
                json_field(trade, "side"),
                json_field(trade, "price"),
                json_field(trade, "size"),
                json_field(trade, "type"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse single-ticker response (GET tickers/{symbol}) into Symbol | Last | Bid | Ask | Vol24h table.
fn parse_ticker(data: &Value) -> CommandOutput {
    let headers = vec![
        "Symbol".into(),
        "Last".into(),
        "Bid".into(),
        "Ask".into(),
        "Vol24h".into(),
    ];
    let mut rows = Vec::new();
    if let Some(ticker) = data.get("ticker") {
        rows.push(vec![
            json_field(ticker, "symbol"),
            json_field(ticker, "last"),
            json_field(ticker, "bid"),
            json_field(ticker, "ask"),
            json_field(ticker, "vol24h"),
        ]);
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse tickers response (GET tickers) into Symbol | Last | Bid | Ask | Vol24h table.
fn parse_tickers(data: &Value) -> CommandOutput {
    let headers = vec![
        "Symbol".into(),
        "Last".into(),
        "Bid".into(),
        "Ask".into(),
        "Vol24h".into(),
    ];
    let mut rows = Vec::new();

    if let Some(tickers) = data.get("tickers").and_then(|t| t.as_array()) {
        for ticker in tickers {
            rows.push(vec![
                json_field(ticker, "symbol"),
                json_field(ticker, "last"),
                json_field(ticker, "bid"),
                json_field(ticker, "ask"),
                json_field(ticker, "vol24h"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse unwind queue response (GET unwindqueue) into symbol | percentile table.
fn parse_unwind_queue(data: &Value) -> CommandOutput {
    let headers = vec!["symbol".into(), "percentile".into()];
    let mut rows = Vec::new();
    let queue = data.get("queue").and_then(|q| q.as_array());
    if let Some(queue) = queue {
        for item in queue {
            rows.push(vec![
                json_field(item, "symbol"),
                json_field(item, "percentile"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse transfers response (GET transfers) into id | date | from | to | asset | amount | status table.
fn parse_transfers(data: &Value) -> CommandOutput {
    let headers = vec![
        "id".into(),
        "date".into(),
        "from".into(),
        "to".into(),
        "asset".into(),
        "amount".into(),
        "status".into(),
    ];
    let mut rows = Vec::new();
    let transfers = data.get("transfers").and_then(|t| t.as_array());
    if let Some(transfers) = transfers {
        for t in transfers {
            rows.push(vec![
                json_field(t, "id"),
                json_field(t, "date"),
                json_field(t, "from"),
                json_field(t, "to"),
                json_field(t, "asset"),
                json_field(t, "amount"),
                json_field(t, "status"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse fee schedule volumes response (GET feeschedules/volumes) into fee_schedule_uid | volume table.
fn parse_fee_schedule_volumes(data: &Value) -> CommandOutput {
    let headers = vec!["fee_schedule_uid".into(), "volume".into()];
    let mut rows = Vec::new();
    let volumes = data
        .get("volumesByFeeSchedule")
        .or_else(|| data.get("volumes_by_fee_schedule"))
        .and_then(|v| v.as_object());
    if let Some(volumes) = volumes {
        for (uid, vol) in volumes {
            let vol_str = match vol {
                Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            rows.push(vec![uid.clone(), vol_str]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse fee schedules response (GET feeschedules) into Name | Uid table.
fn parse_feeschedules(data: &Value) -> CommandOutput {
    let headers = vec!["Name".into(), "Uid".into()];
    let mut rows = Vec::new();

    if let Some(schedules) = data.get("feeSchedules").and_then(|s| s.as_array()) {
        for schedule in schedules {
            rows.push(vec![
                json_field(schedule, "name"),
                json_field(schedule, "uid"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse PnL preferences response (GET pnlpreferences) into symbol | pnlCurrency table.
fn parse_pnl_preferences(data: &Value) -> CommandOutput {
    let headers = vec!["symbol".into(), "pnlCurrency".into()];
    let mut rows = Vec::new();
    let prefs = data.get("preferences").and_then(|p| p.as_array());
    if let Some(prefs) = prefs {
        for pref in prefs {
            rows.push(vec![
                json_field(pref, "symbol"),
                json_field(pref, "pnlCurrency"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse leverage preferences response (GET leveragepreferences) into symbol | maxLeverage table.
fn parse_leverage_preferences(data: &Value) -> CommandOutput {
    let headers = vec!["symbol".into(), "maxLeverage".into()];
    let mut rows = Vec::new();
    let prefs = data
        .get("leveragePreferences")
        .or_else(|| data.get("leverage_preferences"))
        .and_then(|p| p.as_array());
    if let Some(prefs) = prefs {
        for pref in prefs {
            rows.push(vec![
                json_field(pref, "symbol"),
                json_field(pref, "maxLeverage"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse open positions response (GET openpositions) into symbol | side | size | price | fillTime | pnlCurrency | unrealizedFunding table.
fn parse_positions(data: &Value) -> CommandOutput {
    let headers = vec![
        "symbol".into(),
        "side".into(),
        "size".into(),
        "price".into(),
        "fillTime".into(),
        "pnlCurrency".into(),
        "unrealizedFunding".into(),
    ];
    let mut rows = Vec::new();
    let positions = data
        .get("openPositions")
        .or_else(|| data.get("open_positions"))
        .and_then(|p| p.as_array());
    if let Some(positions) = positions {
        for pos in positions {
            rows.push(vec![
                json_field(pos, "symbol"),
                json_field(pos, "side"),
                json_field(pos, "size"),
                json_field(pos, "price"),
                json_field(pos, "fillTime"),
                json_field(pos, "pnlCurrency"),
                json_field(pos, "unrealizedFunding"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse fills response (GET fills) into Fill ID | Symbol | OrderSide | Size | Price | Time table.
fn parse_fills(data: &Value) -> CommandOutput {
    let headers = vec![
        "Fill ID".into(),
        "Symbol".into(),
        "OrderSide".into(),
        "Size".into(),
        "Price".into(),
        "Time".into(),
    ];
    let mut rows = Vec::new();

    if let Some(fills) = data.get("fills").and_then(|f| f.as_array()) {
        for fill in fills {
            rows.push(vec![
                json_field(fill, "fill_id"),
                json_field(fill, "symbol"),
                json_field(fill, "side"),
                json_field(fill, "size"),
                json_field(fill, "price"),
                json_field(fill, "fillTime"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse notifications response (GET notifications) into Type | Priority | Note | Time table.
fn parse_notifications(data: &Value) -> CommandOutput {
    let headers = vec![
        "Type".into(),
        "Priority".into(),
        "Note".into(),
        "Time".into(),
    ];
    let mut rows = Vec::new();

    if let Some(notifications) = data.get("notifications").and_then(|n| n.as_array()) {
        for notif in notifications {
            rows.push(vec![
                json_field(notif, "type"),
                json_field(notif, "priority"),
                json_field(notif, "note"),
                json_field(notif, "effectiveTime"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse sendorder response into order_id | Result | Status table.
fn parse_order_response(data: &Value) -> CommandOutput {
    let result = json_field(data, "result");

    let (order_id, status) = data
        .get("sendStatus")
        .and_then(|v| {
            let obj = match v {
                Value::String(s) => serde_json::from_str(s).ok()?,
                Value::Object(_) => v.clone(),
                _ => return None,
            };
            Some((json_field(&obj, "order_id"), json_field(&obj, "status")))
        })
        .unwrap_or_else(|| ("-".to_string(), "-".to_string()));

    let headers = vec!["order_id".into(), "Result".into(), "Status".into()];
    let rows = vec![vec![order_id.clone(), result.clone(), status.clone()]];
    let json_data = serde_json::json!({
        "order_id": order_id,
        "result": result,
        "status": status,
    });
    CommandOutput::new(json_data, headers, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_path_segment_accepts_valid() {
        assert!(validate_path_segment("abc123", "test").is_ok());
        assert!(validate_path_segment("uid-with-dashes", "test").is_ok());
        assert!(validate_path_segment("PI_XBTUSD", "test").is_ok());
    }

    #[test]
    fn validate_path_segment_rejects_invalid() {
        assert!(validate_path_segment("", "test").is_err());
        assert!(validate_path_segment("path/with/slash", "test").is_err());
        assert!(validate_path_segment("..", "test").is_err());
        assert!(validate_path_segment("uid..suffix", "test").is_err());
        assert!(validate_path_segment("uid with space", "test").is_err());
        assert!(validate_path_segment("\ttab", "test").is_err());
        assert!(validate_path_segment("uid?query", "test").is_err());
        assert!(validate_path_segment("uid#fragment", "test").is_err());
        assert!(validate_path_segment("uid%2fsegment", "test").is_err());
        assert!(validate_path_segment("uid\\segment", "test").is_err());
    }

    #[test]
    fn build_futures_order_basic_buy() {
        let order = OrderRequest {
            symbol: "PI_XBTUSD".into(),
            size: "1".into(),
            r#type: FuturesOrderType::Limit,
            price: Some("50000".into()),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let params = build_futures_order(order, OrderSide::Buy).unwrap();
        assert_eq!(params.get("side").unwrap(), "buy");
        assert_eq!(params.get("symbol").unwrap(), "PI_XBTUSD");
        assert_eq!(params.get("orderType").unwrap(), "lmt");
        assert_eq!(params.get("limitPrice").unwrap(), "50000");
        assert!(!params.contains_key("reduceOnly"));
    }

    #[test]
    fn build_futures_order_with_stop_and_client_id() {
        let order = OrderRequest {
            symbol: "PI_ETHUSD".into(),
            size: "10".into(),
            r#type: FuturesOrderType::Stop,
            price: None,
            stop_price: Some("3000".into()),
            trigger_signal: Some(TriggerSignal::Mark),
            client_order_id: Some("my-order-123".into()),
            reduce_only: true,
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let params = build_futures_order(order, OrderSide::Sell).unwrap();
        assert_eq!(params.get("side").unwrap(), "sell");
        assert_eq!(params.get("stopPrice").unwrap(), "3000");
        assert_eq!(params.get("triggerSignal").unwrap(), "mark");
        assert_eq!(params.get("cliOrdId").unwrap(), "my-order-123");
        assert_eq!(params.get("reduceOnly").unwrap(), "true");
    }

    #[test]
    fn build_futures_order_trailing_stop() {
        let order = OrderRequest {
            symbol: "PI_XBTUSD".into(),
            size: "1".into(),
            r#type: FuturesOrderType::Stop,
            price: None,
            stop_price: Some("49000".into()),
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            trailing_stop_max_deviation: Some("5".into()),
            trailing_stop_deviation_unit: Some(TrailingUnit::Percent),
        };
        let params = build_futures_order(order, OrderSide::Buy).unwrap();
        assert_eq!(params.get("trailingStopMaxDeviation").unwrap(), "5");
        assert_eq!(params.get("trailingStopDeviationUnit").unwrap(), "percent");
    }

    #[test]
    fn build_history_params_all() {
        let params = build_history_params(Some("2024-01-01"), Some("2024-12-31"), Some("asc"));
        assert_eq!(params.len(), 3);
        assert!(params.contains(&("since", "2024-01-01")));
        assert!(params.contains(&("before", "2024-12-31")));
        assert!(params.contains(&("sort", "asc")));
    }

    #[test]
    fn build_history_params_empty() {
        let params = build_history_params(None, None, None);
        assert!(params.is_empty());
    }
}
