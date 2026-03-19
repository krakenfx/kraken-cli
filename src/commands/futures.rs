/// Futures commands: public market data, private trading, and account management.
use std::collections::HashMap;

use clap::Subcommand;
use serde_json::Value;

use super::helpers::{confirm_destructive, jstr, parse_generic};
use crate::client::FuturesClient;
use crate::config::FuturesCredentials;
use crate::errors::{KrakenError, Result};
use crate::output::CommandOutput;

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

#[derive(Debug, Subcommand)]
pub(crate) enum FuturesCommand {
    /// List all futures instruments/contracts.
    Instruments,
    /// Get all futures tickers.
    Tickers,
    /// Get ticker for a single contract.
    Ticker {
        /// Futures symbol (e.g. PI_XBTUSD).
        symbol: String,
    },
    /// Get futures order book.
    Orderbook {
        /// Futures symbol.
        symbol: String,
    },
    /// Get recent futures trade history.
    History {
        /// Futures symbol.
        symbol: String,
        /// Fetch history since this timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Fetch history before this timestamp.
        #[arg(long)]
        before: Option<String>,
    },
    /// Get fee schedules (volumes and fees per contract).
    Feeschedules,
    /// Get instrument status (all or per symbol).
    InstrumentStatus {
        /// Specific symbol to query (omit for all).
        #[arg(long)]
        symbol: Option<String>,
    },
    /// Get trading instruments (optional filter by contract type) (auth required).
    TradingInstruments {
        /// Filter by contract type: futures_inverse, futures_vanilla, flexible_futures.
        #[arg(long)]
        contract_type: Option<String>,
    },
    /// Get historical funding rates.
    HistoricalFundingRates {
        /// Futures symbol (e.g. PI_XBTUSD).
        symbol: String,
    },

    // === Private commands ===
    /// Get futures account/wallet info (auth required).
    Accounts,
    /// Get open futures orders (auth required).
    OpenOrders,
    /// Get status of orders by ID (auth required).
    OrderStatus {
        /// Order ID to query.
        #[arg(required = true, num_args = 1.., value_name = "ORDER_ID")]
        order_ids: Vec<String>,
    },
    /// Place a futures order (auth required).
    Order {
        /// Direction: buy or sell.
        #[command(subcommand)]
        direction: FuturesOrderDirection,
    },
    /// Edit an existing futures order (auth required).
    EditOrder {
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
    },
    /// Cancel a futures order by order_id or client order id (auth required).
    Cancel {
        /// Exchange order ID.
        #[arg(long, required_unless_present = "cli_ord_id")]
        order_id: Option<String>,
        /// Client order ID.
        #[arg(long, required_unless_present = "order_id")]
        cli_ord_id: Option<String>,
    },
    /// Cancel all futures orders (auth required).
    CancelAll {
        /// Filter by symbol.
        #[arg(long)]
        symbol: Option<String>,
    },
    /// Dead man's switch: cancel all orders after timeout (auth required).
    CancelAfter {
        /// Timeout in seconds (0 to disable).
        timeout: u64,
    },
    /// Place a batch of futures orders (auth required).
    BatchOrder {
        /// Orders as JSON array or path to JSON file (prefix with @).
        orders_json: String,
    },
    /// Get open futures positions (auth required).
    Positions,
    /// Get recent fills (auth required).
    Fills {
        /// Filter since this timestamp.
        #[arg(long)]
        since: Option<String>,
    },
    /// Get current leverage preference (auth required).
    Leverage {
        /// Filter by symbol (omit for all).
        #[arg(long)]
        symbol: Option<String>,
    },
    /// Set leverage preference for a symbol (auth required).
    SetLeverage {
        /// Futures symbol.
        symbol: String,
        /// Max leverage value (omit to use cross margin).
        #[arg(required = false)]
        leverage: Option<String>,
    },
    /// Get PnL preferences (auth required).
    PnlPreferences,
    /// Set PnL preference for a symbol (auth required).
    SetPnlPreference {
        /// Futures symbol (e.g. PF_XBTUSD).
        symbol: String,
        /// PnL preference asset (e.g. USD).
        preference: String,
    },
    /// Get notifications (auth required).
    Notifications,
    /// Get execution history (auth required).
    HistoryExecutions {
        /// Filter since this timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Filter before this timestamp.
        #[arg(long)]
        before: Option<String>,
        /// Sort order (asc or desc).
        #[arg(long)]
        sort: Option<String>,
    },
    /// Get order history (auth required).
    HistoryOrders {
        /// Filter since this timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Filter before this timestamp.
        #[arg(long)]
        before: Option<String>,
        /// Sort order (asc or desc).
        #[arg(long)]
        sort: Option<String>,
    },
    /// Get trigger history (auth required).
    HistoryTriggers {
        /// Filter since this timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Filter before this timestamp.
        #[arg(long)]
        before: Option<String>,
        /// Sort order (asc or desc).
        #[arg(long)]
        sort: Option<String>,
    },
    /// Download account log as CSV (auth required).
    HistoryAccountLogCsv {
        /// Filter since this timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Filter before this timestamp.
        #[arg(long)]
        before: Option<String>,
    },
    /// Get futures transfer history (auth required).
    Transfers,
    /// Transfer funds between spot and futures wallets (auth required).
    Transfer {
        /// Amount.
        amount: String,
        /// Currency (e.g. USD, XBT).
        currency: String,
    },
    /// Get unwind queue (auth required).
    UnwindQueue,
    /// Get current assignment programs (auth required).
    AssignmentPrograms,
    /// Get fee schedule volumes (auth required).
    FeeScheduleVolumes,
    /// Get subaccounts (auth required).
    Subaccounts,
    /// Get trading-enabled status for a subaccount (auth required).
    SubaccountStatus {
        /// Subaccount UID.
        subaccount_uid: String,
    },
    /// Set trading-enabled for a subaccount (auth required).
    SetSubaccountStatus {
        /// Subaccount UID.
        subaccount_uid: String,
        /// Enable or disable trading (true/false, yes/no, 1/0).
        trading_enabled: BoolValue,
    },
    /// Transfer between wallets (auth required).
    WalletTransfer {
        /// Source account.
        from_account: String,
        /// Destination account.
        to_account: String,
        /// Currency unit (e.g. USD, XBT).
        unit: String,
        /// Amount to transfer.
        amount: String,
    },

    /// Futures WebSocket streaming commands.
    Ws {
        #[command(subcommand)]
        cmd: super::futures_ws::FuturesWsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum FuturesOrderDirection {
    /// Place a buy order.
    Buy {
        /// Futures symbol.
        symbol: String,
        /// Order size.
        size: String,
        /// Order type.
        #[arg(long, default_value = "limit", value_parser = ["limit", "post", "market", "stop", "take-profit", "ioc", "trailing-stop", "fok"])]
        r#type: String,
        /// Limit price.
        #[arg(long)]
        price: Option<String>,
        /// Stop price (for stop and take-profit orders).
        #[arg(long)]
        stop_price: Option<String>,
        /// Trigger signal: mark, index, or last.
        #[arg(long, value_parser = ["mark", "index", "last"])]
        trigger_signal: Option<String>,
        /// Client order ID for correlation.
        #[arg(long)]
        client_order_id: Option<String>,
        /// Reduce-only flag.
        #[arg(long)]
        reduce_only: bool,
        /// Trailing stop max deviation.
        #[arg(long)]
        trailing_stop_max_deviation: Option<String>,
        /// Trailing stop deviation unit (percent or quote_currency).
        #[arg(long)]
        trailing_stop_deviation_unit: Option<String>,
    },
    /// Place a sell order.
    Sell {
        /// Futures symbol.
        symbol: String,
        /// Order size.
        size: String,
        /// Order type.
        #[arg(long, default_value = "limit", value_parser = ["limit", "post", "market", "stop", "take-profit", "ioc", "trailing-stop", "fok"])]
        r#type: String,
        /// Limit price.
        #[arg(long)]
        price: Option<String>,
        /// Stop price (for stop and take-profit orders).
        #[arg(long)]
        stop_price: Option<String>,
        /// Trigger signal: mark, index, or last.
        #[arg(long, value_parser = ["mark", "index", "last"])]
        trigger_signal: Option<String>,
        /// Client order ID for correlation.
        #[arg(long)]
        client_order_id: Option<String>,
        /// Reduce-only flag.
        #[arg(long)]
        reduce_only: bool,
        /// Trailing stop max deviation.
        #[arg(long)]
        trailing_stop_max_deviation: Option<String>,
        /// Trailing stop deviation unit (percent or quote_currency).
        #[arg(long)]
        trailing_stop_deviation_unit: Option<String>,
    },
}

pub(crate) async fn execute(
    cmd: &FuturesCommand,
    client: &FuturesClient,
    creds: Option<&FuturesCredentials>,
    _otp: Option<&str>,
    force: bool,
    verbose: bool,
) -> Result<CommandOutput> {
    match cmd {
        FuturesCommand::Instruments => {
            let data = client.public_get("instruments", &[], verbose).await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::Tickers => {
            let data = client.public_get("tickers", &[], verbose).await?;
            Ok(parse_tickers(&data))
        }
        FuturesCommand::Ticker { symbol } => {
            validate_path_segment(symbol, "symbol")?;
            let endpoint = format!("tickers/{symbol}");
            let data = client.public_get(&endpoint, &[], verbose).await?;
            Ok(parse_ticker(&data))
        }
        FuturesCommand::Orderbook { symbol } => {
            validate_path_segment(symbol, "symbol")?;
            let data = client
                .public_get("orderbook", &[("symbol", symbol.as_str())], verbose)
                .await?;
            Ok(parse_orderbook(&data))
        }
        FuturesCommand::History {
            symbol,
            since,
            before,
        } => {
            validate_path_segment(symbol, "symbol")?;
            let mut params: Vec<(&str, &str)> = vec![("symbol", symbol.as_str())];
            let since_owned;
            if let Some(s) = since {
                since_owned = s.clone();
                params.push(("since", &since_owned));
            }
            let before_owned;
            if let Some(b) = before {
                before_owned = b.clone();
                params.push(("before", &before_owned));
            }
            let data = client.public_get("history", &params, verbose).await?;
            Ok(parse_trade_history(&data))
        }
        FuturesCommand::Feeschedules => {
            let data = client.public_get("feeschedules", &[], verbose).await?;
            Ok(parse_feeschedules(&data))
        }
        FuturesCommand::InstrumentStatus { symbol } => {
            let endpoint = match symbol {
                Some(s) => {
                    validate_path_segment(s, "symbol")?;
                    format!("instruments/{s}/status")
                }
                None => "instruments/status".to_string(),
            };
            let data = client.public_get(&endpoint, &[], verbose).await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::TradingInstruments { contract_type } => {
            let creds = require_creds(creds)?;
            if let Some(ref ct) = contract_type {
                validate_contract_type(ct)?;
            }
            let mut params: Vec<(&str, &str)> = Vec::new();
            let ct_owned;
            if let Some(ref ct) = contract_type {
                ct_owned = ct.clone();
                params.push(("contractType", &ct_owned));
            }
            let data = client
                .private_get("trading/instruments", &params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::HistoricalFundingRates { symbol } => {
            let params = [("symbol", symbol.as_str())];
            let data = client
                .public_get("historical-funding-rates", &params, verbose)
                .await?;
            Ok(parse_generic(&data))
        }

        // === Private commands ===
        FuturesCommand::Accounts => {
            let creds = require_creds(creds)?;
            let data = client.private_get("accounts", &[], creds, verbose).await?;
            Ok(parse_accounts(&data))
        }
        FuturesCommand::OpenOrders => {
            let creds = require_creds(creds)?;
            let data = client
                .private_get("openorders", &[], creds, verbose)
                .await?;
            Ok(parse_open_orders(&data))
        }
        FuturesCommand::OrderStatus { order_ids } => {
            let creds = require_creds(creds)?;
            let mut params = HashMap::new();
            params.insert("orderIds".into(), order_ids.join(","));
            let data = client
                .private_post("orders/status", params, creds, verbose)
                .await?;
            Ok(parse_order_status(&data))
        }
        FuturesCommand::Order { direction } => {
            let creds = require_creds(creds)?;
            let params = build_futures_order(direction)?;
            let data = client
                .private_post("sendorder", params, creds, verbose)
                .await?;
            Ok(parse_order_response(&data))
        }
        FuturesCommand::EditOrder {
            order_id,
            size,
            price,
            stop_price,
        } => {
            let creds = require_creds(creds)?;
            let mut params = HashMap::new();
            params.insert("orderId".into(), order_id.clone());
            if let Some(s) = size {
                params.insert("size".into(), s.clone());
            }
            if let Some(p) = price {
                params.insert("limitPrice".into(), p.clone());
            }
            if let Some(sp) = stop_price {
                params.insert("stopPrice".into(), sp.clone());
            }
            let data = client
                .private_post("editorder", params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::Cancel {
            order_id,
            cli_ord_id,
        } => {
            let creds = require_creds(creds)?;
            let mut params = HashMap::new();
            match (order_id, cli_ord_id) {
                (Some(oid), None) => {
                    params.insert("order_id".into(), oid.clone());
                }
                (None, Some(cid)) => {
                    params.insert("cliOrdId".into(), cid.clone());
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
            let data = client
                .private_post("cancelorder", params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::CancelAll { symbol } => {
            let creds = require_creds(creds)?;
            if !force {
                confirm_destructive("Cancel ALL open futures orders?")?;
            }
            let mut params = HashMap::new();
            if let Some(s) = symbol {
                params.insert("symbol".into(), s.clone());
            }
            let data = client
                .private_post("cancelallorders", params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::CancelAfter { timeout } => {
            let creds = require_creds(creds)?;
            let mut params = HashMap::new();
            params.insert("timeout".into(), timeout.to_string());
            let data = client
                .private_post("cancelallordersafter", params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::BatchOrder { orders_json } => {
            let creds = require_creds(creds)?;
            let json_str = if let Some(path) = orders_json.strip_prefix('@') {
                std::fs::read_to_string(path).map_err(|e| {
                    KrakenError::Io(std::io::Error::new(
                        e.kind(),
                        format!("Failed to read batch order file '{path}': {e}"),
                    ))
                })?
            } else {
                orders_json.clone()
            };
            let parsed: Value = serde_json::from_str(&json_str)
                .map_err(|e| KrakenError::Validation(format!("Invalid batch order JSON: {e}")))?;
            let data = client
                .private_post_json("batchorder", parsed, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::Positions => {
            let creds = require_creds(creds)?;
            let data = client
                .private_get("openpositions", &[], creds, verbose)
                .await?;
            Ok(parse_positions(&data))
        }
        FuturesCommand::Fills { since } => {
            let creds = require_creds(creds)?;
            let mut params: Vec<(&str, &str)> = Vec::new();
            let since_owned;
            if let Some(s) = since {
                since_owned = s.clone();
                params.push(("lastFillTime", &since_owned));
            }
            let data = client.private_get("fills", &params, creds, verbose).await?;
            Ok(parse_fills(&data))
        }
        FuturesCommand::Leverage { symbol } => {
            let creds = require_creds(creds)?;
            let mut params: Vec<(&str, &str)> = Vec::new();
            let sym_owned;
            if let Some(s) = symbol {
                sym_owned = s.clone();
                params.push(("symbol", &sym_owned));
            }
            let data = client
                .private_get("leveragepreferences", &params, creds, verbose)
                .await?;
            Ok(parse_leverage_preferences(&data))
        }
        FuturesCommand::SetLeverage { symbol, leverage } => {
            let creds = require_creds(creds)?;
            let mut params = HashMap::new();
            params.insert("symbol".into(), symbol.clone());
            if let Some(lev) = &leverage {
                params.insert("maxLeverage".into(), lev.clone());
            }
            let data = client
                .private_put("leveragepreferences", params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::PnlPreferences => {
            let creds = require_creds(creds)?;
            let data = client
                .private_get("pnlpreferences", &[], creds, verbose)
                .await?;
            Ok(parse_pnl_preferences(&data))
        }
        FuturesCommand::SetPnlPreference { symbol, preference } => {
            let creds = require_creds(creds)?;
            let mut params = HashMap::new();
            params.insert("symbol".into(), symbol.clone());
            params.insert("pnlPreference".into(), preference.clone());
            let data = client
                .private_put("pnlpreferences", params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::Notifications => {
            let creds = require_creds(creds)?;
            let data = client
                .private_get("notifications", &[], creds, verbose)
                .await?;
            Ok(parse_notifications(&data))
        }
        FuturesCommand::HistoryExecutions {
            since,
            before,
            sort,
        } => {
            let creds = require_creds(creds)?;
            let params = build_history_params(since.as_deref(), before.as_deref(), sort.as_deref());
            let data = client
                .private_get("executions", &params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::HistoryOrders {
            since,
            before,
            sort,
        } => {
            let creds = require_creds(creds)?;
            let params = build_history_params(since.as_deref(), before.as_deref(), sort.as_deref());
            let data = client
                .private_get("orders", &params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::HistoryTriggers {
            since,
            before,
            sort,
        } => {
            let creds = require_creds(creds)?;
            let params = build_history_params(since.as_deref(), before.as_deref(), sort.as_deref());
            let data = client
                .private_get("triggers", &params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::HistoryAccountLogCsv { since, before } => {
            let creds = require_creds(creds)?;
            let mut params: Vec<(&str, &str)> = Vec::new();
            let s_owned;
            if let Some(s) = since {
                s_owned = s.clone();
                params.push(("since", &s_owned));
            }
            let b_owned;
            if let Some(b) = before {
                b_owned = b.clone();
                params.push(("before", &b_owned));
            }
            let csv_text = client
                .private_get_raw("accountlogcsv", &params, creds, verbose)
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
        FuturesCommand::Transfers => {
            let creds = require_creds(creds)?;
            let data = client.private_get("transfers", &[], creds, verbose).await?;
            Ok(parse_transfers(&data))
        }
        FuturesCommand::Transfer { amount, currency } => {
            let creds = require_creds(creds)?;
            if !force {
                confirm_destructive(&format!(
                    "Transfer {amount} {currency} between spot and futures wallets?"
                ))?;
            }
            let mut params = HashMap::new();
            params.insert("amount".into(), amount.clone());
            params.insert("currency".into(), currency.clone());
            let data = client
                .private_post("withdrawal", params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::UnwindQueue => {
            let creds = require_creds(creds)?;
            let data = client
                .private_get("unwindqueue", &[], creds, verbose)
                .await?;
            Ok(parse_unwind_queue(&data))
        }
        FuturesCommand::AssignmentPrograms => {
            let creds = require_creds(creds)?;
            let data = client
                .private_get("assignmentprogram/current", &[], creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::FeeScheduleVolumes => {
            let creds = require_creds(creds)?;
            let data = client
                .private_get("feeschedules/volumes", &[], creds, verbose)
                .await?;
            Ok(parse_fee_schedule_volumes(&data))
        }
        FuturesCommand::Subaccounts => {
            let creds = require_creds(creds)?;
            let data = client
                .private_get("subaccounts", &[], creds, verbose)
                .await?;
            Ok(parse_subaccounts(&data))
        }
        FuturesCommand::SubaccountStatus { subaccount_uid } => {
            let creds = require_creds(creds)?;
            validate_path_segment(subaccount_uid, "subaccount_uid")?;
            let endpoint = format!("subaccount/{subaccount_uid}/trading-enabled");
            let data = client.private_get(&endpoint, &[], creds, verbose).await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::SetSubaccountStatus {
            subaccount_uid,
            trading_enabled,
        } => {
            let creds = require_creds(creds)?;
            validate_path_segment(subaccount_uid, "subaccount_uid")?;
            let endpoint = format!("subaccount/{subaccount_uid}/trading-enabled");
            let mut params = HashMap::new();
            params.insert(
                "tradingEnabled".into(),
                trading_enabled.as_api_value().to_string(),
            );
            let data = client
                .private_put(&endpoint, params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }
        FuturesCommand::WalletTransfer {
            from_account,
            to_account,
            unit,
            amount,
        } => {
            let creds = require_creds(creds)?;
            if !force {
                confirm_destructive(&format!(
                    "Transfer {amount} {unit} from {from_account} to {to_account}?"
                ))?;
            }
            let mut params = HashMap::new();
            params.insert("fromAccount".into(), from_account.clone());
            params.insert("toAccount".into(), to_account.clone());
            params.insert("unit".into(), unit.clone());
            params.insert("amount".into(), amount.clone());
            let data = client
                .private_post("transfer", params, creds, verbose)
                .await?;
            Ok(parse_generic(&data))
        }

        FuturesCommand::Ws { cmd } => super::futures_ws::execute(cmd, client, creds, verbose).await,
    }
}

/// Returns true for commands that require authentication.
pub(crate) fn requires_auth(cmd: &FuturesCommand) -> bool {
    matches!(
        cmd,
        FuturesCommand::Accounts
            | FuturesCommand::OpenOrders
            | FuturesCommand::OrderStatus { .. }
            | FuturesCommand::Order { .. }
            | FuturesCommand::EditOrder { .. }
            | FuturesCommand::Cancel { .. }
            | FuturesCommand::CancelAll { .. }
            | FuturesCommand::CancelAfter { .. }
            | FuturesCommand::BatchOrder { .. }
            | FuturesCommand::Positions
            | FuturesCommand::Fills { .. }
            | FuturesCommand::Leverage { .. }
            | FuturesCommand::SetLeverage { .. }
            | FuturesCommand::PnlPreferences
            | FuturesCommand::SetPnlPreference { .. }
            | FuturesCommand::Notifications
            | FuturesCommand::HistoryExecutions { .. }
            | FuturesCommand::HistoryOrders { .. }
            | FuturesCommand::HistoryTriggers { .. }
            | FuturesCommand::HistoryAccountLogCsv { .. }
            | FuturesCommand::Transfers
            | FuturesCommand::Transfer { .. }
            | FuturesCommand::UnwindQueue
            | FuturesCommand::AssignmentPrograms
            | FuturesCommand::FeeScheduleVolumes
            | FuturesCommand::Subaccounts
            | FuturesCommand::SubaccountStatus { .. }
            | FuturesCommand::SetSubaccountStatus { .. }
            | FuturesCommand::WalletTransfer { .. }
            | FuturesCommand::TradingInstruments { .. }
    )
}

/// Returns true for WS commands that require authentication.
pub(crate) fn ws_requires_auth(cmd: &FuturesCommand) -> bool {
    if let FuturesCommand::Ws { cmd } = cmd {
        super::futures_ws::requires_auth(cmd)
    } else {
        false
    }
}

fn require_creds(creds: Option<&FuturesCredentials>) -> Result<&FuturesCredentials> {
    creds.ok_or_else(|| {
        KrakenError::Auth(
            "Futures credentials required. Use `kraken setup` or set KRAKEN_FUTURES_API_KEY / KRAKEN_FUTURES_API_SECRET.".into(),
        )
    })
}

const VALID_TRAILING_UNITS: &[&str] = &["percent", "quote_currency"];
const VALID_CONTRACT_TYPES: &[&str] = &["futures_inverse", "futures_vanilla", "flexible_futures"];

fn validate_contract_type(ct: &str) -> Result<()> {
    if !VALID_CONTRACT_TYPES.contains(&ct) {
        return Err(KrakenError::Validation(format!(
            "Invalid contract type: {ct}. Valid values: {}",
            VALID_CONTRACT_TYPES.join(", ")
        )));
    }
    Ok(())
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

/// Map CLI order type to API orderType value.
fn map_order_type_to_api(ot: &str) -> String {
    match ot {
        "limit" => "lmt".to_string(),
        "market" => "mkt".to_string(),
        "stop" => "stp".to_string(),
        "take-profit" => "take_profit".to_string(),
        "trailing-stop" => "trailing_stop".to_string(),
        other => other.to_string(),
    }
}

fn build_futures_order(direction: &FuturesOrderDirection) -> Result<HashMap<String, String>> {
    let (
        side,
        symbol,
        size,
        order_type,
        price,
        stop_price,
        trigger_signal,
        client_order_id,
        reduce_only,
        trailing_max,
        trailing_unit,
    ) = match direction {
        FuturesOrderDirection::Buy {
            symbol,
            size,
            r#type,
            price,
            stop_price,
            trigger_signal,
            client_order_id,
            reduce_only,
            trailing_stop_max_deviation,
            trailing_stop_deviation_unit,
        } => (
            "buy",
            symbol,
            size,
            r#type,
            price,
            stop_price,
            trigger_signal,
            client_order_id,
            *reduce_only,
            trailing_stop_max_deviation,
            trailing_stop_deviation_unit,
        ),
        FuturesOrderDirection::Sell {
            symbol,
            size,
            r#type,
            price,
            stop_price,
            trigger_signal,
            client_order_id,
            reduce_only,
            trailing_stop_max_deviation,
            trailing_stop_deviation_unit,
        } => (
            "sell",
            symbol,
            size,
            r#type,
            price,
            stop_price,
            trigger_signal,
            client_order_id,
            *reduce_only,
            trailing_stop_max_deviation,
            trailing_stop_deviation_unit,
        ),
    };

    let order_type_api = map_order_type_to_api(order_type);

    if let Some(tu) = trailing_unit {
        if !VALID_TRAILING_UNITS.contains(&tu.as_str()) {
            return Err(KrakenError::Validation(format!(
                "Invalid trailing stop deviation unit: {tu}. Valid values: {}",
                VALID_TRAILING_UNITS.join(", ")
            )));
        }
    }

    let mut params = HashMap::new();
    params.insert("orderType".into(), order_type_api);
    params.insert("symbol".into(), symbol.to_string());
    params.insert("side".into(), side.to_string());
    params.insert("size".into(), size.to_string());
    if let Some(p) = price {
        params.insert("limitPrice".into(), p.clone());
    }
    if let Some(sp) = stop_price {
        params.insert("stopPrice".into(), sp.clone());
    }
    if let Some(ts) = trigger_signal {
        params.insert("triggerSignal".into(), ts.clone());
    }
    if let Some(coid) = client_order_id {
        params.insert("cliOrdId".into(), coid.clone());
    }
    if reduce_only {
        params.insert("reduceOnly".into(), "true".into());
    }
    if let Some(tm) = trailing_max {
        params.insert("trailingStopMaxDeviation".into(), tm.clone());
    }
    if let Some(tu) = trailing_unit {
        params.insert("trailingStopDeviationUnit".into(), tu.clone());
    }
    Ok(params)
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

/// Parse orderbook response (GET orderbook) into Side | Price | Size table (asks then bids).
fn parse_orderbook(data: &Value) -> CommandOutput {
    let headers = vec!["Side".into(), "Price".into(), "Size".into()];
    let mut rows = Vec::new();
    let book = data
        .get("orderBook")
        .or_else(|| data.get("order_book"))
        .and_then(|v| v.as_object());
    if let Some(book) = book {
        if let Some(asks) = book.get("asks").and_then(|a| a.as_array()) {
            for level in asks {
                if let Some(arr) = level.as_array() {
                    if arr.len() >= 2 {
                        rows.push(vec!["Ask".into(), arr[0].to_string(), arr[1].to_string()]);
                    }
                }
            }
        }
        if let Some(bids) = book.get("bids").and_then(|b| b.as_array()) {
            for level in bids {
                if let Some(arr) = level.as_array() {
                    if arr.len() >= 2 {
                        rows.push(vec!["Bid".into(), arr[0].to_string(), arr[1].to_string()]);
                    }
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
            let type_str = jstr(account, "type");
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
                jstr(sa, "accountUid"),
                jstr(sa, "email"),
                jstr(sa, "fullName"),
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
                jstr(order, "order_id"),
                jstr(order, "orderType"),
                jstr(order, "side"),
                jstr(order, "symbol"),
                jstr(order, "status"),
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
                jstr(trade, "trade_id"),
                jstr(trade, "time"),
                jstr(trade, "side"),
                jstr(trade, "price"),
                jstr(trade, "size"),
                jstr(trade, "type"),
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
            jstr(ticker, "symbol"),
            jstr(ticker, "last"),
            jstr(ticker, "bid"),
            jstr(ticker, "ask"),
            jstr(ticker, "vol24h"),
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
                jstr(ticker, "symbol"),
                jstr(ticker, "last"),
                jstr(ticker, "bid"),
                jstr(ticker, "ask"),
                jstr(ticker, "vol24h"),
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
            rows.push(vec![jstr(item, "symbol"), jstr(item, "percentile")]);
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
                jstr(t, "id"),
                jstr(t, "date"),
                jstr(t, "from"),
                jstr(t, "to"),
                jstr(t, "asset"),
                jstr(t, "amount"),
                jstr(t, "status"),
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
            rows.push(vec![jstr(schedule, "name"), jstr(schedule, "uid")]);
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
            rows.push(vec![jstr(pref, "symbol"), jstr(pref, "pnlCurrency")]);
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
            rows.push(vec![jstr(pref, "symbol"), jstr(pref, "maxLeverage")]);
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
                jstr(pos, "symbol"),
                jstr(pos, "side"),
                jstr(pos, "size"),
                jstr(pos, "price"),
                jstr(pos, "fillTime"),
                jstr(pos, "pnlCurrency"),
                jstr(pos, "unrealizedFunding"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse fills response (GET fills) into Fill ID | Symbol | Side | Size | Price | Time table.
fn parse_fills(data: &Value) -> CommandOutput {
    let headers = vec![
        "Fill ID".into(),
        "Symbol".into(),
        "Side".into(),
        "Size".into(),
        "Price".into(),
        "Time".into(),
    ];
    let mut rows = Vec::new();

    if let Some(fills) = data.get("fills").and_then(|f| f.as_array()) {
        for fill in fills {
            rows.push(vec![
                jstr(fill, "fill_id"),
                jstr(fill, "symbol"),
                jstr(fill, "side"),
                jstr(fill, "size"),
                jstr(fill, "price"),
                jstr(fill, "fillTime"),
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
                jstr(notif, "type"),
                jstr(notif, "priority"),
                jstr(notif, "note"),
                jstr(notif, "effectiveTime"),
            ]);
        }
    }
    CommandOutput::new(data.clone(), headers, rows)
}

/// Parse sendorder response into order_id | Result | Status table.
fn parse_order_response(data: &Value) -> CommandOutput {
    let result = jstr(data, "result");

    let (order_id, status) = data
        .get("sendStatus")
        .and_then(|v| {
            let obj = match v {
                Value::String(s) => serde_json::from_str(s).ok()?,
                Value::Object(_) => v.clone(),
                _ => return None,
            };
            Some((jstr(&obj, "order_id"), jstr(&obj, "status")))
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
        let dir = FuturesOrderDirection::Buy {
            symbol: "PI_XBTUSD".into(),
            size: "1".into(),
            r#type: "limit".into(),
            price: Some("50000".into()),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let params = build_futures_order(&dir).unwrap();
        assert_eq!(params.get("side").unwrap(), "buy");
        assert_eq!(params.get("symbol").unwrap(), "PI_XBTUSD");
        assert_eq!(params.get("orderType").unwrap(), "lmt");
        assert_eq!(params.get("limitPrice").unwrap(), "50000");
        assert!(!params.contains_key("reduceOnly"));
    }

    #[test]
    fn build_futures_order_with_stop_and_client_id() {
        let dir = FuturesOrderDirection::Sell {
            symbol: "PI_ETHUSD".into(),
            size: "10".into(),
            r#type: "stop".into(),
            price: None,
            stop_price: Some("3000".into()),
            trigger_signal: Some("mark".into()),
            client_order_id: Some("my-order-123".into()),
            reduce_only: true,
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let params = build_futures_order(&dir).unwrap();
        assert_eq!(params.get("side").unwrap(), "sell");
        assert_eq!(params.get("stopPrice").unwrap(), "3000");
        assert_eq!(params.get("triggerSignal").unwrap(), "mark");
        assert_eq!(params.get("cliOrdId").unwrap(), "my-order-123");
        assert_eq!(params.get("reduceOnly").unwrap(), "true");
    }

    #[test]
    fn build_futures_order_trailing_stop() {
        let dir = FuturesOrderDirection::Buy {
            symbol: "PI_XBTUSD".into(),
            size: "1".into(),
            r#type: "stop".into(),
            price: None,
            stop_price: Some("49000".into()),
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            trailing_stop_max_deviation: Some("5".into()),
            trailing_stop_deviation_unit: Some("percent".into()),
        };
        let params = build_futures_order(&dir).unwrap();
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
