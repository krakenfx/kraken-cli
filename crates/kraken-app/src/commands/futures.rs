//! Futures commands: public market data, private trading, and account
//! management.

use super::{Execute, rest_bridge};
use crate::{
    cli::AppContext,
    errors::{KrakenError, Result},
    output::{CommandOutput, json_field, published_json},
};
use itertools::Itertools as _;
use kraken_contract::{
    futures::{
        Accounts, AssignmentPrograms, BatchOrder, Buy, Cancel, CancelAfter, CancelAll, EditOrder,
        FeeScheduleVolumes, Feeschedules, Fills, FuturesCommand, FuturesMarketCommand,
        FuturesOrderDirection, FuturesTradingCommand, HistoricalFundingRates, History,
        HistoryAccountLogCsv, HistoryExecutions, HistoryOrders, HistoryTriggers, InstrumentStatus,
        Instruments, Leverage, Notifications, OpenOrders, OrderRequest, OrderStatus, Orderbook,
        PnlPreferences, Positions, Sell, SetLeverage, SetPnlPreference, SetSubaccountStatus,
        SubaccountStatus, Subaccounts, Ticker, Tickers, TradingInstruments, Transfer, Transfers,
        UnwindQueue, WalletTransfer,
    },
    value_types::FuturesOrderType,
};
use kraken_core::OrderSide;
use kraken_rest::futures::{
    account, history, market,
    trading::{self, FuturesOrderKind},
    transfers,
};
use serde_json::Value;

impl Execute for Instruments {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let instruments = ctx.futures().query(&market::InstrumentsQuery {}).await?;
        CommandOutput::published(&instruments)
    }
}

impl Execute for Tickers {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let tickers = ctx.futures().query(&market::TickersQuery {}).await?;
        Ok(parse_tickers(published_json(&tickers)?))
    }
}

impl Execute for Ticker {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = market::TickerQuery {
            symbol: rest_bridge::segment("symbol", self.symbol)?,
        };
        let ticker = ctx.futures().query(&query).await?;
        Ok(parse_ticker(published_json(&ticker)?))
    }
}

impl Execute for Orderbook {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = market::OrderbookQuery {
            symbol: rest_bridge::segment("symbol", self.symbol)?,
        };
        let book = ctx.futures().query(&query).await?;
        Ok(parse_orderbook(published_json(&book)?))
    }
}

impl Execute for History {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            symbol,
            since,
            before,
        } = self;
        let query = market::TradeHistoryQuery {
            symbol: rest_bridge::segment("symbol", symbol)?,
            since,
            before,
        };
        let history = ctx.futures().query(&query).await?;
        Ok(parse_trade_history(published_json(&history)?))
    }
}

impl Execute for Feeschedules {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let schedules = ctx.futures().query(&market::FeeSchedulesQuery {}).await?;
        Ok(parse_feeschedules(published_json(&schedules)?))
    }
}

impl Execute for InstrumentStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let futures = ctx.futures();
        let data = match self.symbol {
            Some(symbol) => {
                let query = market::InstrumentStatusQuery {
                    symbol: rest_bridge::segment("symbol", symbol)?,
                };
                published_json(&futures.query(&query).await?)?
            }
            None => published_json(&futures.query(&market::InstrumentStatusesQuery {}).await?)?,
        };
        Ok(CommandOutput::from_untyped(data))
    }
}

impl Execute for HistoricalFundingRates {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = market::FundingRatesQuery {
            symbol: rest_bridge::segment("symbol", self.symbol)?,
        };
        let rates = ctx.futures().query(&query).await?;
        CommandOutput::published(&rates)
    }
}

impl Execute for TradingInstruments {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = account::TradingInstrumentsQuery {
            contract_type: self.contract_type.map(rest_bridge::instrument_type),
        };
        let instruments = ctx.futures_account()?.sign(&query).await?;
        CommandOutput::published(&instruments)
    }
}

impl Execute for Accounts {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let accounts = ctx
            .futures_account()?
            .sign(&account::AccountsQuery {})
            .await?;
        Ok(parse_accounts(published_json(&accounts)?))
    }
}

impl Execute for OpenOrders {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let orders = ctx
            .futures_account()?
            .sign(&account::OpenOrdersQuery {})
            .await?;
        Ok(parse_open_orders(published_json(&orders)?))
    }
}

impl Execute for OrderStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = trading::OrdersStatusQuery {
            order_ids: self.order_ids.into(),
        };
        let status = ctx.futures_account()?.sign(&query).await?;
        Ok(parse_order_status(published_json(&status)?))
    }
}

impl Execute for EditOrder {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            order_id,
            size,
            price,
            stop_price,
        } = self;
        let request = trading::EditOrder {
            order: trading::FuturesOrderRef::OrderId(order_id),
            size: rest_bridge::optional_amount("size", size.as_deref())?,
            limit_price: rest_bridge::optional_amount("--price", price.as_deref())?,
            stop_price: rest_bridge::optional_amount("--stop-price", stop_price.as_deref())?,
        };
        let edited = ctx.futures_account()?.sign(&request).await?;
        CommandOutput::published(&edited)
    }
}

impl Execute for Cancel {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            order_id,
            cli_ord_id,
        } = self;
        let request = match (order_id, cli_ord_id) {
            (Some(id), None) => trading::CancelOrder::OrderId(id),
            (None, Some(id)) => trading::CancelOrder::ClientOrderId(id),
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
        };
        let cancelled = ctx.futures_account()?.sign(&request).await?;
        CommandOutput::published(&cancelled)
    }
}

impl Execute for CancelAll {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let account = ctx.futures_account()?;
        ctx.confirm_destructive("Cancel ALL open futures orders?")?;
        let request = trading::CancelAllOrders {
            symbol: self.symbol,
        };
        let cancelled = account.sign(&request).await?;
        CommandOutput::published(&cancelled)
    }
}

impl Execute for CancelAfter {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let request = trading::CancelAllOrdersAfter {
            timeout: self.timeout,
        };
        let switch = ctx.futures_account()?.sign(&request).await?;
        CommandOutput::published(&switch)
    }
}

impl Execute for BatchOrder {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let account = ctx.futures_account()?;
        let Self { orders_json } = self;
        let json = if let Some(path) = orders_json.strip_prefix('@') {
            std::fs::read_to_string(path).map_err(|e| {
                KrakenError::Io(std::io::Error::new(
                    e.kind(),
                    format!("Failed to read batch order file '{path}': {e}"),
                ))
            })?
        } else {
            orders_json
        };
        let request = trading::BatchOrder {
            instructions: serde_json::from_str(&json)
                .map_err(|e| KrakenError::Validation(format!("Invalid batch order JSON: {e}")))?,
        };
        let placed = account.sign(&request).await?;
        CommandOutput::published(&placed)
    }
}

impl Execute for Positions {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let positions = ctx
            .futures_account()?
            .sign(&account::OpenPositionsQuery {})
            .await?;
        Ok(parse_positions(published_json(&positions)?))
    }
}

impl Execute for Fills {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = account::FillsQuery {
            last_fill_time: self.since,
        };
        let fills = ctx.futures_account()?.sign(&query).await?;
        Ok(parse_fills(published_json(&fills)?))
    }
}

impl Execute for Leverage {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = account::LeveragePreferencesQuery {
            symbol: self.symbol,
        };
        let preferences = ctx.futures_account()?.sign(&query).await?;
        Ok(parse_leverage_preferences(published_json(&preferences)?))
    }
}

impl Execute for SetLeverage {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { symbol, leverage } = self;
        let request = account::SetLeveragePreference {
            symbol,
            max_leverage: rest_bridge::optional_amount("leverage", leverage.as_deref())?,
        };
        let set = ctx.futures_account()?.sign(&request).await?;
        CommandOutput::published(&set)
    }
}

impl Execute for PnlPreferences {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let preferences = ctx
            .futures_account()?
            .sign(&account::PnlPreferencesQuery {})
            .await?;
        Ok(parse_pnl_preferences(published_json(&preferences)?))
    }
}

impl Execute for SetPnlPreference {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { symbol, preference } = self;
        let request = account::SetPnlPreference {
            symbol,
            pnl_preference: preference,
        };
        let set = ctx.futures_account()?.sign(&request).await?;
        CommandOutput::published(&set)
    }
}

impl Execute for Notifications {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let notifications = ctx
            .futures_account()?
            .sign(&account::NotificationsQuery {})
            .await?;
        Ok(parse_notifications(published_json(&notifications)?))
    }
}

impl Execute for HistoryExecutions {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = history::ExecutionsQuery {
            window: rest_bridge::history_window(self.filter),
        };
        let page = ctx.futures_account()?.sign(&query).await?;
        CommandOutput::published(&page)
    }
}

impl Execute for HistoryOrders {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = history::OrderEventsQuery {
            window: rest_bridge::history_window(self.filter),
        };
        let page = ctx.futures_account()?.sign(&query).await?;
        CommandOutput::published(&page)
    }
}

impl Execute for HistoryTriggers {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = history::TriggerEventsQuery {
            window: rest_bridge::history_window(self.filter),
        };
        let page = ctx.futures_account()?.sign(&query).await?;
        CommandOutput::published(&page)
    }
}

impl Execute for HistoryAccountLogCsv {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self { since, before } = self;
        let request = history::AccountLogCsv {
            window: history::HistoryWindow {
                since,
                before,
                sort: None,
            },
        };
        let csv_text = ctx.futures_account()?.download(&request).await?;
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

impl Execute for Transfers {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let transfers = ctx
            .futures_account()?
            .sign(&account::TransfersQuery::default())
            .await?;
        Ok(parse_transfers(published_json(&transfers)?))
    }
}

impl Execute for Transfer {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let account = ctx.futures_account()?;
        let Self { amount, currency } = self;
        ctx.confirm_destructive(&format!(
            "Transfer {amount} {currency} between spot and futures wallets?"
        ))?;
        let request = transfers::WithdrawToSpot {
            amount: rest_bridge::amount("amount", &amount)?,
            currency,
            source_wallet: None,
        };
        let withdrawn = account.sign(&request).await?;
        CommandOutput::published(&withdrawn)
    }
}

impl Execute for UnwindQueue {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let queue = ctx
            .futures_account()?
            .sign(&account::UnwindQueueQuery {})
            .await?;
        Ok(parse_unwind_queue(published_json(&queue)?))
    }
}

impl Execute for AssignmentPrograms {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let programs = ctx
            .futures_account()?
            .sign(&account::AssignmentProgramsQuery {})
            .await?;
        CommandOutput::published(&programs)
    }
}

impl Execute for FeeScheduleVolumes {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let volumes = ctx
            .futures_account()?
            .sign(&account::FeeScheduleVolumesQuery {})
            .await?;
        Ok(parse_fee_schedule_volumes(published_json(&volumes)?))
    }
}

impl Execute for Subaccounts {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let subaccounts = ctx
            .futures_account()?
            .sign(&account::SubaccountsQuery {})
            .await?;
        Ok(parse_subaccounts(published_json(&subaccounts)?))
    }
}

impl Execute for SubaccountStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let query = account::SubaccountTradingQuery {
            subaccount_uid: rest_bridge::segment("subaccount_uid", self.subaccount_uid)?,
        };
        let status = ctx.futures_account()?.sign(&query).await?;
        CommandOutput::published(&status)
    }
}

impl Execute for SetSubaccountStatus {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            subaccount_uid,
            trading_enabled,
        } = self;
        let request = account::SetSubaccountTrading {
            subaccount_uid: rest_bridge::segment("subaccount_uid", subaccount_uid)?,
            trading_enabled: rest_bridge::flag(trading_enabled),
        };
        let status = ctx.futures_account()?.sign(&request).await?;
        CommandOutput::published(&status)
    }
}

impl Execute for WalletTransfer {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let account = ctx.futures_account()?;
        let Self {
            from_account,
            to_account,
            unit,
            amount,
        } = self;
        ctx.confirm_destructive(&format!(
            "Transfer {amount} {unit} from {from_account} to {to_account}?"
        ))?;
        let request = transfers::WalletTransfer {
            amount: rest_bridge::amount("amount", &amount)?,
            from_account,
            to_account,
            unit,
        };
        let transfer = account.sign(&request).await?;
        CommandOutput::published(&transfer)
    }
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
    let account = ctx.futures_account()?;
    let sent = account.sign(&send_order(order, side)?).await?;
    Ok(parse_order_response(&published_json(&sent)?))
}

/// The command line's order as the one [`FuturesOrderKind`] its type allows:
/// a missing price, or a price the type does not take, never reaches Kraken.
fn send_order(order: OrderRequest, side: OrderSide) -> Result<trading::SendOrder> {
    use FuturesOrderType as T;
    let OrderRequest {
        symbol,
        size,
        r#type: order_type,
        price,
        stop_price,
        trigger_signal,
        client_order_id,
        reduce_only,
        trailing_stop_max_deviation: max_deviation,
        trailing_stop_deviation_unit: unit,
    } = order;
    let refuse = |given: bool, flag: &str, valid: &[T]| -> Result<()> {
        if given && !valid.contains(&order_type) {
            return Err(KrakenError::Validation(format!(
                "{flag} is only valid for {} orders",
                valid.iter().join(", ")
            )));
        }
        Ok(())
    };
    refuse(
        price.is_some(),
        "--price",
        &[T::Limit, T::Post, T::Ioc, T::Fok, T::Stop, T::TakeProfit],
    )?;
    refuse(
        stop_price.is_some(),
        "--stop-price",
        &[T::Stop, T::TakeProfit],
    )?;
    refuse(
        trigger_signal.is_some(),
        "--trigger-signal",
        &[T::Stop, T::TakeProfit, T::TrailingStop],
    )?;
    refuse(
        max_deviation.is_some() || unit.is_some(),
        "--trailing-stop-max-deviation / --trailing-stop-deviation-unit",
        &[T::TrailingStop],
    )?;
    let required = |flag: &str, raw: Option<&str>| {
        let raw = raw.ok_or_else(|| {
            KrakenError::Validation(format!("{flag} is required for {order_type} orders"))
        })?;
        rest_bridge::amount(flag, raw)
    };
    let limit = || required("--price", price.as_deref());
    let trigger = trigger_signal.map(rest_bridge::trigger_signal);
    let kind = match order_type {
        T::Market => FuturesOrderKind::Market,
        T::Limit => FuturesOrderKind::Limit { price: limit()? },
        T::Post => FuturesOrderKind::PostOnly { price: limit()? },
        T::Ioc => FuturesOrderKind::ImmediateOrCancel { price: limit()? },
        T::Fok => FuturesOrderKind::FillOrKill { price: limit()? },
        T::Stop => FuturesOrderKind::Stop {
            stop_price: required("--stop-price", stop_price.as_deref())?,
            limit_price: rest_bridge::optional_amount("--price", price.as_deref())?,
            trigger,
        },
        T::TakeProfit => FuturesOrderKind::TakeProfit {
            stop_price: required("--stop-price", stop_price.as_deref())?,
            limit_price: rest_bridge::optional_amount("--price", price.as_deref())?,
            trigger,
        },
        T::TrailingStop => FuturesOrderKind::TrailingStop {
            max_deviation: required("--trailing-stop-max-deviation", max_deviation.as_deref())?,
            unit: unit.map(rest_bridge::deviation_unit).ok_or_else(|| {
                KrakenError::Validation(format!(
                    "--trailing-stop-deviation-unit is required for {order_type} orders"
                ))
            })?,
            trigger,
        },
    };
    Ok(trading::SendOrder {
        symbol,
        side,
        size: rest_bridge::amount("size", &size)?,
        kind,
        cli_ord_id: client_order_id,
        reduce_only,
    })
}

/// Parse orderbook response (GET orderbook) into OrderSide | Price | Size table
/// (asks then bids).
fn parse_orderbook(data: Value) -> CommandOutput {
    let headers = vec!["OrderSide".into(), "Price".into(), "Size".into()];
    let mut rows = Vec::new();
    let book = data.get("orderBook").and_then(|v| v.as_object());
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
    CommandOutput::new(data, headers, rows)
}

/// Parse order status response (POST orders/status) into order_id | status
/// table.
fn parse_order_status(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
}

/// Format a JSON value as string for balance/amount display.
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Parse accounts response (GET accounts) into account_id | type | currency |
/// balance table.
fn parse_accounts(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
}

/// Parse subaccounts response (GET subaccounts) into accountUid | email |
/// fullName table.
fn parse_subaccounts(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
}

/// Parse open orders response (GET openorders) into order_id | orderType | side
/// | symbol | status table.
fn parse_open_orders(data: Value) -> CommandOutput {
    let headers = vec![
        "order_id".into(),
        "orderType".into(),
        "side".into(),
        "symbol".into(),
        "status".into(),
    ];
    let mut rows = Vec::new();
    let orders = data.get("openOrders").and_then(|a| a.as_array());
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
    CommandOutput::new(data, headers, rows)
}

/// Parse public trade history response (GET history) into trade_id | time |
/// side | price | size | type table.
fn parse_trade_history(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
}

/// Parse single-ticker response (GET tickers/{symbol}) into Symbol | Last | Bid
/// | Ask | Vol24h table.
fn parse_ticker(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
}

/// Parse tickers response (GET tickers) into Symbol | Last | Bid | Ask | Vol24h
/// table.
fn parse_tickers(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
}

/// Parse unwind queue response (GET unwindqueue) into symbol | percentile
/// table.
fn parse_unwind_queue(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
}

/// Parse transfers response (GET transfers) into id | date | from | to | asset
/// | amount | status table.
fn parse_transfers(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
}

/// Parse fee schedule volumes response (GET feeschedules/volumes) into
/// fee_schedule_uid | volume table.
fn parse_fee_schedule_volumes(data: Value) -> CommandOutput {
    let headers = vec!["fee_schedule_uid".into(), "volume".into()];
    let mut rows = Vec::new();
    let volumes = data.get("volumesByFeeSchedule").and_then(|v| v.as_object());
    if let Some(volumes) = volumes {
        for (uid, vol) in volumes {
            let vol_str = match vol {
                Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            rows.push(vec![uid.clone(), vol_str]);
        }
    }
    CommandOutput::new(data, headers, rows)
}

/// Parse fee schedules response (GET feeschedules) into Name | Uid table.
fn parse_feeschedules(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
}

/// Parse PnL preferences response (GET pnlpreferences) into symbol |
/// pnlCurrency table.
fn parse_pnl_preferences(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
}

/// Parse leverage preferences response (GET leveragepreferences) into symbol |
/// maxLeverage table.
fn parse_leverage_preferences(data: Value) -> CommandOutput {
    let headers = vec!["symbol".into(), "maxLeverage".into()];
    let mut rows = Vec::new();
    let prefs = data.get("leveragePreferences").and_then(|p| p.as_array());
    if let Some(prefs) = prefs {
        for pref in prefs {
            rows.push(vec![
                json_field(pref, "symbol"),
                json_field(pref, "maxLeverage"),
            ]);
        }
    }
    CommandOutput::new(data, headers, rows)
}

/// Parse open positions response (GET openpositions) into symbol | side | size
/// | price | fillTime | pnlCurrency | unrealizedFunding table.
fn parse_positions(data: Value) -> CommandOutput {
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
    let positions = data.get("openPositions").and_then(|p| p.as_array());
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
    CommandOutput::new(data, headers, rows)
}

/// Parse fills response (GET fills) into Fill ID | Symbol | OrderSide | Size |
/// Price | Time table.
fn parse_fills(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
}

/// Parse notifications response (GET notifications) into Type | Priority | Note
/// | Time table.
fn parse_notifications(data: Value) -> CommandOutput {
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
    CommandOutput::new(data, headers, rows)
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
    use kraken_contract::value_types::{TrailingUnit, TriggerSignal};
    use rust_decimal_macros::dec;

    fn order(order_type: FuturesOrderType) -> OrderRequest {
        OrderRequest {
            symbol: "PI_XBTUSD".into(),
            size: "1".into(),
            r#type: order_type,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        }
    }

    fn refusal(order: OrderRequest) -> String {
        send_order(order, OrderSide::Buy).unwrap_err().to_string()
    }

    #[test]
    fn a_stop_carries_its_trigger_and_optional_limit() {
        let sent = send_order(
            OrderRequest {
                stop_price: Some("3000".into()),
                trigger_signal: Some(TriggerSignal::Mark),
                client_order_id: Some("my-order-123".into()),
                reduce_only: true,
                ..order(FuturesOrderType::Stop)
            },
            OrderSide::Sell,
        )
        .unwrap();
        assert_eq!(
            sent.kind,
            FuturesOrderKind::Stop {
                stop_price: dec!(3000),
                limit_price: None,
                trigger: Some(trading::TriggerSignal::Mark),
            }
        );
        assert_eq!(sent.cli_ord_id.as_deref(), Some("my-order-123"));
        assert!(sent.reduce_only);
    }

    #[test]
    fn a_trailing_stop_needs_both_deviation_flags() {
        let sent = send_order(
            OrderRequest {
                trailing_stop_max_deviation: Some("5".into()),
                trailing_stop_deviation_unit: Some(TrailingUnit::Percent),
                ..order(FuturesOrderType::TrailingStop)
            },
            OrderSide::Buy,
        )
        .unwrap();
        assert!(matches!(sent.kind, FuturesOrderKind::TrailingStop { .. }));
        let err = refusal(OrderRequest {
            trailing_stop_max_deviation: Some("5".into()),
            ..order(FuturesOrderType::TrailingStop)
        });
        assert!(
            err.contains("--trailing-stop-deviation-unit is required"),
            "{err}"
        );
    }

    #[test]
    fn a_limit_without_a_price_is_refused() {
        let err = refusal(order(FuturesOrderType::Limit));
        assert!(
            err.contains("--price is required for limit orders"),
            "{err}"
        );
    }

    #[test]
    fn a_price_the_type_does_not_take_is_refused() {
        let err = refusal(OrderRequest {
            price: Some("50000".into()),
            ..order(FuturesOrderType::Market)
        });
        assert!(err.contains("--price is only valid for"), "{err}");
        let err = refusal(OrderRequest {
            stop_price: Some("49000".into()),
            trailing_stop_max_deviation: Some("5".into()),
            ..order(FuturesOrderType::Stop)
        });
        assert!(err.contains("only valid for trailing-stop orders"), "{err}");
    }

    #[test]
    fn a_size_that_is_not_a_decimal_is_refused() {
        let err = refusal(OrderRequest {
            size: "one".into(),
            ..order(FuturesOrderType::Market)
        });
        assert!(err.contains("size \"one\" is not a decimal"), "{err}");
    }
}

delegate_execute!(FuturesOrderDirection { Buy, Sell });
delegate_execute!(FuturesCommand {
    Market,
    Trading,
    Ws,
    Paper
});
delegate_execute!(FuturesMarketCommand {
    Instruments,
    Tickers,
    Ticker,
    Orderbook,
    History,
    Feeschedules,
    InstrumentStatus,
    HistoricalFundingRates
});
delegate_execute!(FuturesTradingCommand {
    TradingInstruments,
    Accounts,
    OpenOrders,
    OrderStatus,
    Order,
    EditOrder,
    Cancel,
    CancelAll,
    CancelAfter,
    BatchOrder,
    Positions,
    Fills,
    Leverage,
    SetLeverage,
    PnlPreferences,
    SetPnlPreference,
    Notifications,
    HistoryExecutions,
    HistoryOrders,
    HistoryTriggers,
    HistoryAccountLogCsv,
    Transfers,
    Transfer,
    UnwindQueue,
    AssignmentPrograms,
    FeeScheduleVolumes,
    Subaccounts,
    SubaccountStatus,
    SetSubaccountStatus,
    WalletTransfer
});
