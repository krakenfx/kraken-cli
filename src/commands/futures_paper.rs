use std::collections::{HashMap, HashSet};

use clap::Subcommand;
use enum_dispatch::enum_dispatch;
use serde_json::{Value, json};

use super::Execute;
use crate::cli::AppContext;
use crate::client::FuturesClient;
use crate::errors::{KrakenError, Result};
use crate::futures_paper::{
    self, DEFAULT_FUTURES_TAKER_FEE_RATE, DEFAULT_MAINTENANCE_MARGIN_RATE, FuturesOrderType,
    FuturesPaperState, MAX_LEVERAGE, MarketSnapshot, OrderParams, PositionSide, TriggerSignal,
    compute_liquidation_price, compute_unrealized_pnl,
};
use crate::output::CommandOutput;

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum FuturesPaperCommand {
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

#[derive(Debug, clap::Args)]
pub(crate) struct Init {
    /// Starting collateral balance (default: 10000).
    #[arg(long, default_value = "10000")]
    balance: f64,
    /// Collateral currency (default: USD).
    #[arg(long, default_value = "USD")]
    currency: String,
    /// Fee rate as a decimal (default: 0.0005 = 0.05%).
    #[arg(long)]
    fee_rate: Option<f64>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Reset {
    #[arg(long)]
    balance: Option<f64>,
    #[arg(long)]
    currency: Option<String>,
    #[arg(long)]
    fee_rate: Option<f64>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Balance {}

#[derive(Debug, clap::Args)]
pub(crate) struct Status {}

// Buy and sell share one order surface; each side keeps a thin wrapper because
// enum_dispatch needs a distinct type per variant. The flatten field comes
// first so the positionals (symbol, size) keep their order.
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

#[derive(Debug, clap::Args)]
pub(crate) struct OrderRequest {
    /// Futures symbol (e.g. PF_XBTUSD).
    symbol: String,
    /// Order size.
    size: String,
    /// Order type (default: limit).
    #[arg(long, default_value = "limit")]
    r#type: FuturesOrderType,
    /// Limit price.
    #[arg(long)]
    price: Option<String>,
    /// Stop/trigger price.
    #[arg(long)]
    stop_price: Option<String>,
    /// Trigger signal: mark, index, or last.
    #[arg(long)]
    trigger_signal: Option<TriggerSignal>,
    /// Client order ID.
    #[arg(long)]
    client_order_id: Option<String>,
    /// Reduce-only flag.
    #[arg(long)]
    reduce_only: bool,
    /// Leverage override for this order.
    #[arg(long)]
    leverage: Option<String>,
    /// Trailing stop max deviation.
    #[arg(long)]
    trailing_stop_max_deviation: Option<String>,
    /// Trailing stop deviation unit (percent or quote_currency).
    #[arg(long)]
    trailing_stop_deviation_unit: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Orders {}

#[derive(Debug, clap::Args)]
pub(crate) struct OrderStatus {
    /// Order ID to query.
    order_id: String,
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

// order_id/cli_ord_id are a required_unless_present pair; clap resolves those
// by sibling field id, so both fields must stay in this one struct.
#[derive(Debug, clap::Args)]
pub(crate) struct Cancel {
    /// Exchange order ID.
    #[arg(long, required_unless_present = "cli_ord_id")]
    order_id: Option<String>,
    /// Client order ID.
    #[arg(long, required_unless_present = "order_id")]
    cli_ord_id: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct CancelAll {
    /// Filter by symbol.
    #[arg(long)]
    symbol: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct BatchOrder {
    /// Orders as JSON array or path to JSON file (prefix with @).
    orders_json: String,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Positions {}

#[derive(Debug, clap::Args)]
pub(crate) struct Fills {}

#[derive(Debug, clap::Args)]
pub(crate) struct History {}

#[derive(Debug, clap::Args)]
pub(crate) struct Leverage {
    #[arg(long)]
    symbol: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct SetLeverage {
    /// Futures symbol.
    symbol: String,
    /// Max leverage value.
    leverage: String,
}

// Only the leaves that fetch live market data build an HTTP client, so
// `init`/`reset` and pure state reads stay fully offline.
impl Execute for Init {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            balance,
            currency,
            fee_rate,
        } = self;
        execute_init(balance, &currency, fee_rate)
    }
}

impl Execute for Reset {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            balance,
            currency,
            fee_rate,
        } = self;
        execute_reset(balance, currency.as_deref(), fee_rate)
    }
}

impl Execute for Balance {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        execute_balance(ctx.futures()?).await
    }
}

impl Execute for Status {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        execute_status(ctx.futures()?).await
    }
}

impl Execute for Buy {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        execute_order(PositionSide::Long, self.order, ctx.futures()?).await
    }
}

impl Execute for Sell {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        execute_order(PositionSide::Short, self.order, ctx.futures()?).await
    }
}

impl Execute for Orders {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        execute_orders()
    }
}

impl Execute for OrderStatus {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        execute_order_status(&self.order_id)
    }
}

impl Execute for EditOrder {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            order_id,
            size,
            price,
            stop_price,
        } = self;
        execute_edit_order(&order_id, &size, &price, &stop_price)
    }
}

impl Execute for Cancel {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            order_id,
            cli_ord_id,
        } = self;
        execute_cancel(order_id.as_deref(), cli_ord_id.as_deref())
    }
}

impl Execute for CancelAll {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        execute_cancel_all(self.symbol.as_deref())
    }
}

impl Execute for BatchOrder {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        execute_batch_order(&self.orders_json, ctx.futures()?).await
    }
}

impl Execute for Positions {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        execute_positions(ctx.futures()?).await
    }
}

impl Execute for Fills {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        execute_fills()
    }
}

impl Execute for History {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        execute_history()
    }
}

impl Execute for Leverage {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        execute_leverage(self.symbol.as_deref())
    }
}

impl Execute for SetLeverage {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        let Self { symbol, leverage } = self;
        execute_set_leverage(&symbol, &leverage)
    }
}

fn fp_json(mut data: Value) -> Value {
    if let Some(obj) = data.as_object_mut() {
        obj.insert("mode".to_string(), json!("futures_paper"));
    }
    data
}

fn validate_symbol_path_segment(symbol: &str) -> Result<()> {
    if symbol.is_empty() {
        return Err(KrakenError::Validation("symbol cannot be empty".into()));
    }
    if symbol.contains('/') {
        return Err(KrakenError::Validation(
            "symbol must not contain '/'".into(),
        ));
    }
    if symbol.contains("..") {
        return Err(KrakenError::Validation(
            "symbol must not contain '..'".into(),
        ));
    }
    if symbol.contains(char::is_whitespace) {
        return Err(KrakenError::Validation(
            "symbol must not contain whitespace".into(),
        ));
    }
    if symbol.chars().any(|c| matches!(c, '?' | '#' | '%' | '\\')) {
        return Err(KrakenError::Validation(
            "symbol must not contain reserved URL characters (?, #, %, \\)".into(),
        ));
    }
    Ok(())
}

fn validate_symbol(symbol: &str) -> Result<()> {
    validate_symbol_path_segment(symbol)?;
    let upper = symbol.to_uppercase();
    if upper.starts_with("FI_") {
        return Err(KrakenError::Validation(format!(
            "Fixed-date contracts ('{symbol}') are not supported in futures paper. Only perpetual contracts (PF_*) are accepted."
        )));
    }
    if !upper.starts_with("PF_") && !upper.starts_with("PI_") {
        return Err(KrakenError::Validation(format!(
            "Unknown futures symbol '{symbol}'. Use perpetual symbols like PF_XBTUSD or PI_XBTUSD."
        )));
    }
    Ok(())
}

async fn fetch_known_perpetual_symbols(client: &FuturesClient) -> Result<HashSet<String>> {
    let data = client.public_get("instruments", &[]).await?;
    let instruments = data
        .get("instruments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| KrakenError::Parse("Missing instruments data".into()))?;

    let mut symbols = HashSet::new();
    for inst in instruments {
        if let Some(sym) = inst.get("symbol").and_then(|s| s.as_str()) {
            let upper = sym.to_uppercase();
            if upper.starts_with("PF_") || upper.starts_with("PI_") {
                symbols.insert(upper);
            }
        }
    }
    Ok(symbols)
}

fn validate_symbol_exists(symbol: &str, known_symbols: &HashSet<String>) -> Result<()> {
    if !known_symbols.contains(&symbol.to_uppercase()) {
        return Err(KrakenError::Validation(format!(
            "Unknown perpetual symbol '{symbol}'. Symbol not found in available futures instruments."
        )));
    }
    Ok(())
}

async fn fetch_ticker(client: &FuturesClient, symbol: &str) -> Result<MarketSnapshot> {
    validate_symbol(symbol)?;
    let endpoint = format!("tickers/{}", symbol.to_uppercase());
    let data = client.public_get(&endpoint, &[]).await?;
    let ticker = data
        .get("ticker")
        .ok_or_else(|| KrakenError::Parse("Missing ticker data".into()))?;

    let last = ticker.get("last").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let bid = ticker.get("bid").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let ask = ticker.get("ask").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let mark = ticker
        .get("markPrice")
        .and_then(|v| v.as_f64())
        .unwrap_or(last);
    let index = ticker
        .get("indexPrice")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    if last <= 0.0 && bid <= 0.0 && ask <= 0.0 {
        return Err(KrakenError::Parse(format!(
            "Ticker for {symbol} returned no valid prices"
        )));
    }

    Ok(MarketSnapshot {
        bid,
        ask,
        last,
        mark,
        index,
        ask_levels: Vec::new(),
        bid_levels: Vec::new(),
    })
}

async fn fetch_orderbook_levels(
    client: &FuturesClient,
    symbol: &str,
) -> (Vec<(f64, f64)>, Vec<(f64, f64)>) {
    let params = [("symbol", symbol)];
    match client.public_get("orderbook", &params).await {
        Ok(data) => {
            let mut asks = Vec::new();
            let mut bids = Vec::new();
            let book = data.get("orderBook").or_else(|| data.get("order_book"));
            if let Some(book) = book {
                fn parse_levels(book: &Value, key: &str) -> Vec<(f64, f64)> {
                    let mut out = Vec::new();
                    if let Some(arr) = book.get(key).and_then(|v| v.as_array()) {
                        for level in arr {
                            if let Some(arr) = level.as_array()
                                && arr.len() >= 2
                            {
                                let price = arr[0].as_f64().unwrap_or(0.0);
                                let size = arr[1].as_f64().unwrap_or(0.0);
                                out.push((price, size));
                            }
                        }
                    }
                    out
                }
                asks = parse_levels(book, "asks");
                bids = parse_levels(book, "bids");
            }
            (asks, bids)
        }
        Err(_) => (Vec::new(), Vec::new()),
    }
}

async fn fetch_funding_rate(client: &FuturesClient, symbol: &str) -> Result<f64> {
    let params = [("symbol", symbol)];
    let data = client
        .public_get("historical-funding-rates", &params)
        .await?;
    let rates = data
        .get("rates")
        .and_then(|v| v.as_array())
        .ok_or_else(|| KrakenError::Parse("Missing funding rates data".into()))?;

    rates
        .last()
        .and_then(|entry| entry.get("relativeFundingRate").and_then(|v| v.as_f64()))
        .ok_or_else(|| KrakenError::Parse("No funding rate entries".into()))
}

async fn reconcile_best_effort(state: &mut FuturesPaperState, client: &FuturesClient) {
    let symbols: Vec<String> = state
        .positions
        .iter()
        .map(|p| p.symbol.clone())
        .chain(state.open_orders.iter().map(|o| o.symbol.clone()))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    if symbols.is_empty() {
        return;
    }

    let mut mark_prices = HashMap::new();
    let mut last_prices = HashMap::new();
    let mut index_prices = HashMap::new();
    for sym in &symbols {
        if let Ok(snap) = fetch_ticker(client, sym).await {
            if snap.mark > 0.0 {
                mark_prices.insert(sym.clone(), snap.mark);
            }
            if snap.last > 0.0 {
                last_prices.insert(sym.clone(), snap.last);
            }
            if snap.index > 0.0 {
                index_prices.insert(sym.clone(), snap.index);
            }
        }
    }

    let position_symbols: Vec<String> = state
        .positions
        .iter()
        .map(|p| p.symbol.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let mut funding_rates = HashMap::new();
    for sym in &position_symbols {
        if let Ok(rate) = fetch_funding_rate(client, sym).await {
            funding_rates.insert(sym.clone(), rate);
        }
    }

    let mut maint_rates = HashMap::new();
    for sym in &symbols {
        maint_rates.insert(sym.clone(), DEFAULT_MAINTENANCE_MARGIN_RATE);
    }

    let _reconciled = state.reconcile(
        &mark_prices,
        &last_prices,
        &index_prices,
        &funding_rates,
        &maint_rates,
    );

    if let Err(e) = futures_paper::save_state(state) {
        tracing::warn!(error = %e, "could not save reconciled state");
    }
}

fn execute_init(balance: f64, currency: &str, fee_rate: Option<f64>) -> Result<CommandOutput> {
    if !balance.is_finite() || balance <= 0.0 {
        return Err(KrakenError::Validation(
            "Starting balance must be a finite positive number".into(),
        ));
    }

    let rate = fee_rate.unwrap_or(DEFAULT_FUTURES_TAKER_FEE_RATE);
    if !rate.is_finite() || !(0.0..=1.0).contains(&rate) {
        return Err(KrakenError::Validation(
            "Fee rate must be between 0.0 and 1.0".into(),
        ));
    }

    let _lock = futures_paper::StateLock::acquire()?;
    let path = futures_paper::futures_paper_state_path()?;
    if path.exists() {
        return Err(KrakenError::Validation(
            "Futures paper account already initialized. Use 'kraken futures paper reset' to start over.".into(),
        ));
    }
    let state = FuturesPaperState::new(balance, currency, rate);
    futures_paper::save_state(&state)?;

    let cur = currency.to_uppercase();
    let fee_pct = format!("{:.4}%", rate * 100.0);
    let pairs = vec![
        ("Mode".into(), "[FUTURES PAPER] Simulated Trading".into()),
        ("Action".into(), "Account initialized".into()),
        ("Starting Collateral".into(), format!("{balance:.2} {cur}")),
        ("Fee Rate".into(), fee_pct),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        fp_json(json!({
            "action": "initialized",
            "starting_collateral": balance,
            "currency": cur,
            "fee_rate": rate,
        })),
    ))
}

fn execute_reset(
    balance: Option<f64>,
    currency: Option<&str>,
    fee_rate: Option<f64>,
) -> Result<CommandOutput> {
    if let Some(b) = balance
        && (!b.is_finite() || b <= 0.0)
    {
        return Err(KrakenError::Validation(
            "Starting balance must be a finite positive number".into(),
        ));
    }
    if let Some(r) = fee_rate
        && (!r.is_finite() || !(0.0..=1.0).contains(&r))
    {
        return Err(KrakenError::Validation(
            "Fee rate must be between 0.0 and 1.0".into(),
        ));
    }

    let _lock = futures_paper::StateLock::acquire()?;
    let mut state = futures_paper::load_state()?;
    state.reset(balance, currency, fee_rate);
    futures_paper::save_state(&state)?;

    let pairs = vec![
        ("Mode".into(), "[FUTURES PAPER] Simulated Trading".into()),
        ("Action".into(), "Account reset".into()),
        (
            "Starting Collateral".into(),
            format!("{:.2} {}", state.starting_collateral, state.currency),
        ),
        ("Fee Rate".into(), format!("{:.4}%", state.fee_rate * 100.0)),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        fp_json(json!({
            "action": "reset",
            "starting_collateral": state.starting_collateral,
            "currency": state.currency,
            "fee_rate": state.fee_rate,
        })),
    ))
}

async fn execute_balance(client: &FuturesClient) -> Result<CommandOutput> {
    let _lock = futures_paper::StateLock::acquire()?;
    let mut state = futures_paper::load_state()?;
    reconcile_best_effort(&mut state, client).await;

    let mark_prices = build_mark_prices(&state, client).await;
    let upnl = state.unrealized_pnl(&mark_prices);
    let used = state.used_margin();
    let available = state.available_margin(&mark_prices);

    let pairs = vec![
        ("Mode".into(), "[FUTURES PAPER] Simulated Trading".into()),
        (
            "Collateral".into(),
            format!("{:.2} {}", state.collateral, state.currency),
        ),
        ("Unrealized PnL".into(), format!("{upnl:+.2}")),
        ("Used Margin".into(), format!("{used:.2}")),
        ("Available Margin".into(), format!("{available:.2}")),
        ("Open Orders".into(), state.open_orders.len().to_string()),
        ("Positions".into(), state.positions.len().to_string()),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        fp_json(json!({
            "collateral": state.collateral,
            "currency": state.currency,
            "unrealized_pnl": upnl,
            "used_margin": used,
            "available_margin": available,
            "open_orders": state.open_orders.len(),
            "positions": state.positions.len(),
            "last_reconciled_at": state.last_reconciled_at,
        })),
    ))
}

async fn execute_status(client: &FuturesClient) -> Result<CommandOutput> {
    let _lock = futures_paper::StateLock::acquire()?;
    let mut state = futures_paper::load_state()?;
    reconcile_best_effort(&mut state, client).await;

    let mark_prices = build_mark_prices(&state, client).await;
    let upnl = state.unrealized_pnl(&mark_prices);
    let equity = state.collateral + upnl;
    let pnl_from_start = equity - state.starting_collateral;
    let pnl_pct = if state.starting_collateral > 0.0 {
        (pnl_from_start / state.starting_collateral) * 100.0
    } else {
        0.0
    };

    let pairs = vec![
        ("Mode".into(), "[FUTURES PAPER] Simulated Trading".into()),
        (
            "Starting Collateral".into(),
            format!("{:.2} {}", state.starting_collateral, state.currency),
        ),
        (
            "Current Collateral".into(),
            format!("{:.2} {}", state.collateral, state.currency),
        ),
        ("Equity".into(), format!("{equity:.2} {}", state.currency)),
        (
            "P&L".into(),
            format!("{pnl_from_start:+.2} ({pnl_pct:+.2}%)"),
        ),
        ("Total Fills".into(), state.fills.len().to_string()),
        ("Open Orders".into(), state.open_orders.len().to_string()),
        ("Positions".into(), state.positions.len().to_string()),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        fp_json(json!({
            "starting_collateral": state.starting_collateral,
            "collateral": state.collateral,
            "equity": equity,
            "unrealized_pnl": upnl,
            "pnl": pnl_from_start,
            "pnl_pct": pnl_pct,
            "currency": state.currency,
            "total_fills": state.fills.len(),
            "open_orders": state.open_orders.len(),
            "positions": state.positions.len(),
            "last_reconciled_at": state.last_reconciled_at,
        })),
    ))
}

async fn execute_order(
    side: PositionSide,
    order: OrderRequest,
    client: &FuturesClient,
) -> Result<CommandOutput> {
    let OrderRequest {
        symbol,
        size: size_str,
        r#type: order_type,
        price: price_str,
        stop_price: stop_price_str,
        trigger_signal,
        client_order_id,
        reduce_only,
        leverage: leverage_str,
        trailing_stop_max_deviation: trailing_max_str,
        trailing_stop_deviation_unit: trailing_unit,
    } = order;

    validate_symbol(&symbol)?;
    let known_symbols = fetch_known_perpetual_symbols(client).await?;
    validate_symbol_exists(&symbol, &known_symbols)?;

    let size: f64 = size_str
        .parse()
        .map_err(|_| KrakenError::Validation(format!("Invalid size: {size_str}")))?;
    let price: Option<f64> = price_str
        .as_ref()
        .map(|p| {
            p.parse()
                .map_err(|_| KrakenError::Validation(format!("Invalid price: {p}")))
        })
        .transpose()?;
    let stop_price: Option<f64> = stop_price_str
        .as_ref()
        .map(|p| {
            p.parse()
                .map_err(|_| KrakenError::Validation(format!("Invalid stop price: {p}")))
        })
        .transpose()?;
    let leverage: Option<f64> = leverage_str
        .as_ref()
        .map(|l| {
            l.parse()
                .map_err(|_| KrakenError::Validation(format!("Invalid leverage: {l}")))
        })
        .transpose()?;
    let trailing_max: Option<f64> = trailing_max_str
        .as_ref()
        .map(|v| {
            v.parse().map_err(|_| {
                KrakenError::Validation(format!("Invalid trailing stop deviation: {v}"))
            })
        })
        .transpose()?;

    if let Some(unit) = &trailing_unit
        && unit != "percent"
        && unit != "quote_currency"
    {
        return Err(KrakenError::Validation(format!(
            "Invalid trailing-stop-deviation-unit '{unit}'. Use 'percent' or 'quote_currency'."
        )));
    }

    if let Some(lev) = leverage
        && (!lev.is_finite() || !(1.0..=MAX_LEVERAGE).contains(&lev))
    {
        return Err(KrakenError::Validation(format!(
            "Leverage must be between 1.0 and {MAX_LEVERAGE:.0}"
        )));
    }

    let _lock = futures_paper::StateLock::acquire()?;
    let mut state = futures_paper::load_state()?;
    reconcile_best_effort(&mut state, client).await;

    let mut snapshot = fetch_ticker(client, &symbol).await?;
    let (ask_levels, bid_levels) = fetch_orderbook_levels(client, &symbol).await;
    snapshot.ask_levels = ask_levels;
    snapshot.bid_levels = bid_levels;

    let params = OrderParams {
        symbol: symbol.to_uppercase(),
        side,
        size,
        order_type,
        price,
        stop_price,
        trigger_signal,
        client_order_id,
        reduce_only,
        leverage,
        trailing_stop_max_deviation: trailing_max,
        trailing_stop_deviation_unit: trailing_unit,
    };

    let result = state.place_order(params, &snapshot)?;
    futures_paper::save_state(&state)?;

    let fill_json: Vec<Value> = result
        .fills
        .iter()
        .map(|f| {
            json!({
                "fill_id": f.id,
                "side": f.side.to_string(),
                "size": f.size,
                "price": f.price,
                "fee": f.fee,
                "realized_pnl": f.realized_pnl,
            })
        })
        .collect();

    let mut kvs = vec![
        ("Mode".into(), "[FUTURES PAPER] Simulated Trading".into()),
        ("Order ID".into(), result.order_id.clone()),
        ("Symbol".into(), symbol.to_uppercase()),
        ("PositionSide".into(), side.to_string()),
        ("Status".into(), result.status.to_string()),
    ];

    if let Some(msg) = &result.message {
        kvs.push(("Info".into(), msg.clone()));
    }

    for fill in &result.fills {
        kvs.push((
            "Fill".into(),
            format!(
                "{:.8} @ {:.2} (fee: {:.4})",
                fill.size, fill.price, fill.fee
            ),
        ));
    }

    Ok(CommandOutput::key_value(
        kvs,
        fp_json(json!({
            "action": "order_placed",
            "order_id": result.order_id,
            "symbol": symbol.to_uppercase(),
            "side": side.to_string(),
            "status": result.status.to_string(),
            "fills": fill_json,
            "message": result.message,
        })),
    ))
}

fn execute_orders() -> Result<CommandOutput> {
    let _lock = futures_paper::StateLock::acquire()?;
    let state = futures_paper::load_state()?;
    Ok(format_open_orders(&state))
}

fn format_open_orders(state: &FuturesPaperState) -> CommandOutput {
    let headers = vec![
        "[FP] Order ID".into(),
        "Symbol".into(),
        "PositionSide".into(),
        "Type".into(),
        "Size".into(),
        "Price".into(),
        "Stop Price".into(),
        "Trigger".into(),
        "Status".into(),
        "Client ID".into(),
    ];

    let rows: Vec<Vec<String>> = if state.open_orders.is_empty() {
        vec![vec!["—".into(); 10]]
    } else {
        state
            .open_orders
            .iter()
            .map(|o| {
                vec![
                    o.id.clone(),
                    o.symbol.clone(),
                    o.side.to_string(),
                    o.order_type.to_string(),
                    format!("{:.8}", o.size),
                    o.price
                        .map(|p| format!("{p:.2}"))
                        .unwrap_or_else(|| "—".into()),
                    o.stop_price
                        .map(|p| format!("{p:.2}"))
                        .unwrap_or_else(|| "—".into()),
                    o.trigger_signal
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "—".into()),
                    o.status.to_string(),
                    o.client_order_id.clone().unwrap_or_else(|| "—".into()),
                ]
            })
            .collect()
    };

    let json_orders: Vec<Value> = state
        .open_orders
        .iter()
        .map(|o| {
            json!({
                "id": o.id,
                "symbol": o.symbol,
                "side": o.side.to_string(),
                "type": o.order_type.to_string(),
                "size": o.size,
                "price": o.price,
                "stop_price": o.stop_price,
                "trigger_signal": o.trigger_signal.map(|t| t.to_string()),
                "status": o.status.to_string(),
                "client_order_id": o.client_order_id,
                "reduce_only": o.reduce_only,
                "leverage": o.leverage,
            })
        })
        .collect();

    CommandOutput::new(
        fp_json(json!({ "open_orders": json_orders, "count": state.open_orders.len() })),
        headers,
        rows,
    )
}

fn execute_order_status(order_id: &str) -> Result<CommandOutput> {
    let _lock = futures_paper::StateLock::acquire()?;
    let state = futures_paper::load_state()?;
    format_order_status(&state, order_id)
}

fn format_order_status(state: &FuturesPaperState, order_id: &str) -> Result<CommandOutput> {
    let order = state
        .open_orders
        .iter()
        .find(|o| o.id == order_id)
        .ok_or_else(|| {
            KrakenError::Validation(format!("Order '{order_id}' not found in open orders"))
        })?;

    let pairs = vec![
        ("Mode".into(), "[FUTURES PAPER] Simulated Trading".into()),
        ("Order ID".into(), order.id.clone()),
        ("Symbol".into(), order.symbol.clone()),
        ("PositionSide".into(), order.side.to_string()),
        ("Type".into(), order.order_type.to_string()),
        ("Size".into(), format!("{:.8}", order.size)),
        (
            "Price".into(),
            order
                .price
                .map(|p| format!("{p:.2}"))
                .unwrap_or_else(|| "—".into()),
        ),
        (
            "Stop Price".into(),
            order
                .stop_price
                .map(|p| format!("{p:.2}"))
                .unwrap_or_else(|| "—".into()),
        ),
        (
            "Trigger Signal".into(),
            order
                .trigger_signal
                .map(|t| t.to_string())
                .unwrap_or_else(|| "—".into()),
        ),
        ("Status".into(), order.status.to_string()),
        (
            "Client Order ID".into(),
            order.client_order_id.clone().unwrap_or_else(|| "—".into()),
        ),
        ("Leverage".into(), format!("{:.1}x", order.leverage)),
        ("Reduce Only".into(), order.reduce_only.to_string()),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        fp_json(json!({
            "order_id": order.id,
            "symbol": order.symbol,
            "side": order.side.to_string(),
            "type": order.order_type.to_string(),
            "size": order.size,
            "price": order.price,
            "stop_price": order.stop_price,
            "trigger_signal": order.trigger_signal.map(|t| t.to_string()),
            "status": order.status.to_string(),
            "client_order_id": order.client_order_id,
            "leverage": order.leverage,
            "reduce_only": order.reduce_only,
        })),
    ))
}

fn execute_edit_order(
    order_id: &str,
    size_str: &Option<String>,
    price_str: &Option<String>,
    stop_price_str: &Option<String>,
) -> Result<CommandOutput> {
    let new_size: Option<f64> = size_str
        .as_ref()
        .map(|s| {
            s.parse()
                .map_err(|_| KrakenError::Validation(format!("Invalid size: {s}")))
        })
        .transpose()?;
    let new_price: Option<f64> = price_str
        .as_ref()
        .map(|p| {
            p.parse()
                .map_err(|_| KrakenError::Validation(format!("Invalid price: {p}")))
        })
        .transpose()?;
    let new_stop_price: Option<f64> = stop_price_str
        .as_ref()
        .map(|sp| {
            sp.parse()
                .map_err(|_| KrakenError::Validation(format!("Invalid stop price: {sp}")))
        })
        .transpose()?;

    let _lock = futures_paper::StateLock::acquire()?;
    let mut state = futures_paper::load_state()?;
    state.edit_order(order_id, new_size, new_price, new_stop_price)?;
    futures_paper::save_state(&state)?;

    let pairs = vec![
        ("Mode".into(), "[FUTURES PAPER] Simulated Trading".into()),
        ("Action".into(), "Order edited".into()),
        ("Order ID".into(), order_id.to_string()),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        fp_json(json!({
            "action": "order_edited",
            "order_id": order_id,
        })),
    ))
}

fn execute_cancel(order_id: Option<&str>, cli_ord_id: Option<&str>) -> Result<CommandOutput> {
    let _lock = futures_paper::StateLock::acquire()?;
    let mut state = futures_paper::load_state()?;
    let cancelled = state.cancel_order(order_id, cli_ord_id)?;
    futures_paper::save_state(&state)?;

    let pairs = vec![
        ("Mode".into(), "[FUTURES PAPER] Simulated Trading".into()),
        ("Action".into(), "Order cancelled".into()),
        ("Order ID".into(), cancelled.id.clone()),
        ("Symbol".into(), cancelled.symbol.clone()),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        fp_json(json!({
            "action": "order_cancelled",
            "order_id": cancelled.id,
            "symbol": cancelled.symbol,
        })),
    ))
}

fn execute_cancel_all(symbol: Option<&str>) -> Result<CommandOutput> {
    let _lock = futures_paper::StateLock::acquire()?;
    let mut state = futures_paper::load_state()?;
    let cancelled = state.cancel_all(symbol);
    futures_paper::save_state(&state)?;

    let pairs = vec![
        ("Mode".into(), "[FUTURES PAPER] Simulated Trading".into()),
        ("Action".into(), "Orders cancelled".into()),
        ("Cancelled".into(), cancelled.len().to_string()),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        fp_json(json!({
            "action": "all_orders_cancelled",
            "cancelled_count": cancelled.len(),
        })),
    ))
}

async fn execute_batch_order(orders_json: &str, client: &FuturesClient) -> Result<CommandOutput> {
    let json_str = if let Some(path) = orders_json.strip_prefix('@') {
        std::fs::read_to_string(path).map_err(|e| {
            KrakenError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read batch order file '{path}': {e}"),
            ))
        })?
    } else {
        orders_json.to_string()
    };

    let parsed: Vec<Value> = serde_json::from_str(&json_str)
        .map_err(|e| KrakenError::Validation(format!("Invalid batch order JSON: {e}")))?;

    let _lock = futures_paper::StateLock::acquire()?;
    let mut state = futures_paper::load_state()?;
    reconcile_best_effort(&mut state, client).await;

    let mut snapshots = HashMap::new();
    let symbols: Vec<String> = parsed
        .iter()
        .filter_map(|v| {
            v.get("symbol")
                .and_then(|s| s.as_str())
                .map(|s| s.to_uppercase())
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let known_symbols = fetch_known_perpetual_symbols(client).await?;
    for sym in &symbols {
        validate_symbol(sym)?;
        validate_symbol_exists(sym, &known_symbols)?;
    }

    for sym in &symbols {
        if let Ok(mut snap) = fetch_ticker(client, sym).await {
            let (ask_levels, bid_levels) = fetch_orderbook_levels(client, sym).await;
            snap.ask_levels = ask_levels;
            snap.bid_levels = bid_levels;
            snapshots.insert(sym.clone(), snap);
        }
    }

    let batch: Vec<OrderParams> = parsed
        .iter()
        .map(parse_batch_row)
        .collect::<Result<Vec<_>>>()?;

    let results = state.batch_orders(batch, &snapshots);
    futures_paper::save_state(&state)?;

    let json_results: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "symbol": r.symbol,
                "success": r.success,
                "order_id": r.order_id,
                "error": r.error,
            })
        })
        .collect();

    let headers = vec![
        "Symbol".into(),
        "Success".into(),
        "Order ID".into(),
        "Error".into(),
    ];
    let rows: Vec<Vec<String>> = results
        .iter()
        .map(|r| {
            vec![
                r.symbol.clone(),
                r.success.to_string(),
                r.order_id.clone().unwrap_or_else(|| "—".into()),
                r.error.clone().unwrap_or_else(|| "—".into()),
            ]
        })
        .collect();

    Ok(CommandOutput::new(
        fp_json(json!({ "results": json_results })),
        headers,
        rows,
    ))
}

fn parse_batch_f64(v: &Value, field: &str) -> Result<Option<f64>> {
    let n = match v.get(field) {
        None => return Ok(None),
        Some(val) => {
            if let Some(n) = val.as_f64() {
                n
            } else if let Some(s) = val.as_str() {
                s.parse::<f64>().map_err(|_| {
                    KrakenError::Validation(format!(
                        "Batch row field '{field}' is not a valid number: '{s}'"
                    ))
                })?
            } else {
                return Err(KrakenError::Validation(format!(
                    "Batch row field '{field}' has unexpected type"
                )));
            }
        }
    };
    if !n.is_finite() {
        return Err(KrakenError::Validation(format!(
            "Batch row field '{field}' must be a finite number"
        )));
    }
    Ok(Some(n))
}

fn parse_batch_row(v: &Value) -> Result<OrderParams> {
    let symbol = v
        .get("symbol")
        .and_then(|s| s.as_str())
        .ok_or_else(|| KrakenError::Validation("Batch row missing 'symbol'".into()))?
        .to_uppercase();
    validate_symbol(&symbol)?;
    let side_str = v
        .get("side")
        .and_then(|s| s.as_str())
        .ok_or_else(|| KrakenError::Validation("Batch row missing 'side'".into()))?;
    let side = match side_str {
        "buy" | "long" => PositionSide::Long,
        "sell" | "short" => PositionSide::Short,
        _ => {
            return Err(KrakenError::Validation(format!(
                "Invalid side '{side_str}'. Use buy/long or sell/short."
            )));
        }
    };
    let size = parse_batch_f64(v, "size")?
        .ok_or_else(|| KrakenError::Validation("Batch row missing 'size'".into()))?;
    let order_type_str = v.get("type").and_then(|s| s.as_str()).unwrap_or("limit");
    let order_type = FuturesOrderType::from_str_cli(order_type_str)?;
    let price = parse_batch_f64(v, "price")?;
    let leverage = parse_batch_f64(v, "leverage")?;
    let stop_price = parse_batch_f64(v, "stop_price")?;
    let trigger_signal = match v.get("trigger_signal") {
        Some(val) => match val.as_str() {
            Some(s) => Some(TriggerSignal::from_str_cli(s)?),
            None => {
                return Err(KrakenError::Validation(
                    "trigger_signal must be a string (mark, index, or last)".into(),
                ));
            }
        },
        None => None,
    };
    let trailing_stop_max_deviation = parse_batch_f64(v, "trailing_stop_max_deviation")?;
    let trailing_stop_deviation_unit = v
        .get("trailing_stop_deviation_unit")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    if let Some(ref unit) = trailing_stop_deviation_unit
        && unit != "percent"
        && unit != "quote_currency"
    {
        return Err(KrakenError::Validation(format!(
            "Invalid trailing_stop_deviation_unit '{unit}'. Use 'percent' or 'quote_currency'."
        )));
    }

    Ok(OrderParams {
        symbol,
        side,
        size,
        order_type,
        price,
        stop_price,
        trigger_signal,
        client_order_id: v
            .get("client_order_id")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        reduce_only: v
            .get("reduce_only")
            .and_then(|b| b.as_bool())
            .unwrap_or(false),
        leverage,
        trailing_stop_max_deviation,
        trailing_stop_deviation_unit,
    })
}

async fn execute_positions(client: &FuturesClient) -> Result<CommandOutput> {
    let _lock = futures_paper::StateLock::acquire()?;
    let mut state = futures_paper::load_state()?;
    reconcile_best_effort(&mut state, client).await;

    let mark_prices = build_mark_prices(&state, client).await;

    let headers = vec![
        "[FP] Symbol".into(),
        "PositionSide".into(),
        "Size".into(),
        "Entry".into(),
        "Mark".into(),
        "Liq Price".into(),
        "Unrealized PnL".into(),
        "Funding".into(),
        "Leverage".into(),
    ];

    let rows: Vec<Vec<String>> = if state.positions.is_empty() {
        vec![vec!["—".into(); 9]]
    } else {
        state
            .positions
            .iter()
            .map(|p| {
                let mark = mark_prices.get(&p.symbol).copied().unwrap_or(p.entry_price);
                let upnl = compute_unrealized_pnl(p, mark);
                let liq = compute_liquidation_price(p, DEFAULT_MAINTENANCE_MARGIN_RATE);
                vec![
                    p.symbol.clone(),
                    p.side.to_string(),
                    format!("{:.8}", p.size),
                    format!("{:.2}", p.entry_price),
                    format!("{mark:.2}"),
                    format!("{liq:.2}"),
                    format!("{upnl:+.2}"),
                    format!("{:.4}", p.unrealized_funding),
                    format!("{:.1}x", p.leverage),
                ]
            })
            .collect()
    };

    let json_positions: Vec<Value> = state
        .positions
        .iter()
        .map(|p| {
            let mark = mark_prices.get(&p.symbol).copied().unwrap_or(p.entry_price);
            let upnl = compute_unrealized_pnl(p, mark);
            let liq = compute_liquidation_price(p, DEFAULT_MAINTENANCE_MARGIN_RATE);
            json!({
                "symbol": p.symbol,
                "side": p.side.to_string(),
                "size": p.size,
                "entry_price": p.entry_price,
                "mark_price": mark,
                "liquidation_price": liq,
                "unrealized_pnl": upnl,
                "unrealized_funding": p.unrealized_funding,
                "leverage": p.leverage,
            })
        })
        .collect();

    Ok(CommandOutput::new(
        fp_json(json!({
            "positions": json_positions,
            "count": state.positions.len(),
            "last_reconciled_at": state.last_reconciled_at,
            "maintenance_margin_fallback_used": state.maintenance_margin_fallback_used,
        })),
        headers,
        rows,
    ))
}

fn execute_fills() -> Result<CommandOutput> {
    let _lock = futures_paper::StateLock::acquire()?;
    let state = futures_paper::load_state()?;

    let headers = vec![
        "[FP] Fill ID".into(),
        "Order ID".into(),
        "Symbol".into(),
        "PositionSide".into(),
        "Size".into(),
        "Price".into(),
        "Fee".into(),
        "Realized PnL".into(),
        "Type".into(),
        "Time".into(),
    ];

    let rows: Vec<Vec<String>> = if state.fills.is_empty() {
        vec![vec!["—".into(); 10]]
    } else {
        state
            .fills
            .iter()
            .map(|f| {
                vec![
                    f.id.clone(),
                    f.order_id.clone(),
                    f.symbol.clone(),
                    f.side.to_string(),
                    format!("{:.8}", f.size),
                    format!("{:.2}", f.price),
                    format!("{:.4}", f.fee),
                    f.realized_pnl
                        .map(|p| format!("{p:+.2}"))
                        .unwrap_or_else(|| "—".into()),
                    f.fill_type.clone(),
                    kraken_recording::format_instant(f.filled_at),
                ]
            })
            .collect()
    };

    let json_fills: Vec<Value> = state
        .fills
        .iter()
        .map(|f| {
            json!({
                "id": f.id,
                "order_id": f.order_id,
                "symbol": f.symbol,
                "side": f.side.to_string(),
                "size": f.size,
                "price": f.price,
                "fee": f.fee,
                "realized_pnl": f.realized_pnl,
                "fill_type": f.fill_type,
                "client_order_id": f.client_order_id,
                "filled_at": f.filled_at,
            })
        })
        .collect();

    Ok(CommandOutput::new(
        fp_json(json!({ "fills": json_fills, "count": state.fills.len() })),
        headers,
        rows,
    ))
}

fn execute_history() -> Result<CommandOutput> {
    let _lock = futures_paper::StateLock::acquire()?;
    let state = futures_paper::load_state()?;

    let headers = vec![
        "[FP] ID".into(),
        "Event".into(),
        "Symbol".into(),
        "Amount".into(),
        "Details".into(),
        "Time".into(),
    ];

    let rows: Vec<Vec<String>> = if state.history.is_empty() {
        vec![vec!["—".into(); 6]]
    } else {
        state
            .history
            .iter()
            .map(|h| {
                vec![
                    h.id.clone(),
                    h.event_type.clone(),
                    h.symbol.clone().unwrap_or_else(|| "—".into()),
                    format!("{:+.4}", h.amount),
                    h.details.clone(),
                    kraken_recording::format_instant(h.timestamp),
                ]
            })
            .collect()
    };

    let json_history: Vec<Value> = state
        .history
        .iter()
        .map(|h| {
            json!({
                "id": h.id,
                "event_type": h.event_type,
                "symbol": h.symbol,
                "amount": h.amount,
                "details": h.details,
                "timestamp": h.timestamp,
            })
        })
        .collect();

    Ok(CommandOutput::new(
        fp_json(json!({ "history": json_history, "count": state.history.len() })),
        headers,
        rows,
    ))
}

fn execute_leverage(symbol: Option<&str>) -> Result<CommandOutput> {
    let _lock = futures_paper::StateLock::acquire()?;
    let state = futures_paper::load_state()?;

    let entries: Vec<(&String, &f64)> = match symbol {
        Some(s) => state
            .leverage_preferences
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(s))
            .collect(),
        None => state.leverage_preferences.iter().collect(),
    };

    let headers = vec!["[FP] Symbol".into(), "Max Leverage".into()];
    let rows: Vec<Vec<String>> = if entries.is_empty() {
        vec![vec!["—".into(), "No preferences set".into()]]
    } else {
        entries
            .iter()
            .map(|(sym, lev)| vec![sym.to_string(), format!("{lev:.1}x")])
            .collect()
    };

    let json_prefs: Value = entries
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect::<serde_json::Map<String, Value>>()
        .into();

    Ok(CommandOutput::new(
        fp_json(json!({ "leverage_preferences": json_prefs })),
        headers,
        rows,
    ))
}

fn execute_set_leverage(symbol: &str, leverage_str: &str) -> Result<CommandOutput> {
    validate_symbol(symbol)?;
    let leverage: f64 = leverage_str
        .parse()
        .map_err(|_| KrakenError::Validation(format!("Invalid leverage: {leverage_str}")))?;

    if !leverage.is_finite() || !(1.0..=MAX_LEVERAGE).contains(&leverage) {
        return Err(KrakenError::Validation(format!(
            "Leverage must be between 1.0 and {MAX_LEVERAGE:.0}"
        )));
    }

    let _lock = futures_paper::StateLock::acquire()?;
    let mut state = futures_paper::load_state()?;
    state
        .leverage_preferences
        .insert(symbol.to_uppercase(), leverage);
    futures_paper::save_state(&state)?;

    let pairs = vec![
        ("Mode".into(), "[FUTURES PAPER] Simulated Trading".into()),
        ("Action".into(), "Leverage preference set".into()),
        ("Symbol".into(), symbol.to_uppercase()),
        ("Leverage".into(), format!("{leverage:.1}x")),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        fp_json(json!({
            "action": "leverage_set",
            "symbol": symbol.to_uppercase(),
            "leverage": leverage,
        })),
    ))
}

async fn build_mark_prices(
    state: &FuturesPaperState,
    client: &FuturesClient,
) -> HashMap<String, f64> {
    let mut prices = HashMap::new();
    let symbols: Vec<String> = state
        .positions
        .iter()
        .map(|p| p.symbol.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    for sym in symbols {
        if let Ok(snap) = fetch_ticker(client, &sym).await
            && snap.mark > 0.0
        {
            prices.insert(sym, snap.mark);
        }
    }
    prices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_perpetual_symbol_accepted() {
        assert!(validate_symbol("PF_XBTUSD").is_ok());
        assert!(validate_symbol("PI_XBTUSD").is_ok());
    }

    #[test]
    fn validate_fixed_date_rejected() {
        let result = validate_symbol("FI_XBTUSD_260327");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Fixed-date"));
    }

    #[test]
    fn validate_unknown_symbol_rejected() {
        let result = validate_symbol("BTCUSD");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unknown futures symbol")
        );
    }

    #[test]
    fn validate_symbol_rejects_reserved_path_chars() {
        for symbol in ["PF_XBT/USD", "PF_XBTUSD%2f", "PF_XBTUSD extra"] {
            let err = validate_symbol(symbol).unwrap_err().to_string();
            assert!(
                err.contains("must not contain"),
                "Expected path validation error for {symbol}, got: {err}"
            );
        }
    }

    #[test]
    fn validate_nonexistent_perpetual_rejected() {
        let known: HashSet<String> = ["PF_XBTUSD", "PF_ETHUSD", "PI_XBTUSD"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let result = validate_symbol_exists("PF_FAKEUSD", &known);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unknown perpetual symbol")
        );
    }

    #[test]
    fn validate_existing_perpetual_accepted() {
        let known: HashSet<String> = ["PF_XBTUSD", "PF_ETHUSD", "PI_XBTUSD"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(validate_symbol_exists("PF_XBTUSD", &known).is_ok());
        assert!(validate_symbol_exists("pf_xbtusd", &known).is_ok());
        assert!(validate_symbol_exists("PI_XBTUSD", &known).is_ok());
    }

    #[test]
    fn validate_nonexistent_perpetual_in_batch_context() {
        let known: HashSet<String> = ["PF_XBTUSD", "PF_ETHUSD"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(validate_symbol("PF_NOSUCHASSET").is_ok());
        let result = validate_symbol_exists("PF_NOSUCHASSET", &known);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not found in available futures instruments")
        );
    }

    #[test]
    fn fp_json_injects_mode() {
        let data = fp_json(json!({"foo": "bar"}));
        assert_eq!(
            data.get("mode").and_then(|v| v.as_str()),
            Some("futures_paper")
        );
    }

    #[test]
    fn orders_and_order_status_surface_stop_price_and_trigger_signal() {
        let mut state = FuturesPaperState::new(10000.0, "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        let snap = MarketSnapshot {
            bid: 82700.0,
            ask: 82800.0,
            last: 82750.0,
            mark: 82750.0,
            index: 82750.0,
            ask_levels: vec![(82800.0, 10.0)],
            bid_levels: vec![(82700.0, 10.0)],
        };
        let placed = state
            .place_order(
                OrderParams {
                    symbol: "PF_XBTUSD".into(),
                    side: PositionSide::Long,
                    size: 0.1,
                    order_type: FuturesOrderType::Stop,
                    price: None,
                    stop_price: Some(85000.0),
                    trigger_signal: Some(TriggerSignal::Mark),
                    client_order_id: None,
                    reduce_only: false,
                    leverage: Some(10.0),
                    trailing_stop_max_deviation: None,
                    trailing_stop_deviation_unit: None,
                },
                &snap,
            )
            .unwrap();

        let orders_out = format_open_orders(&state);
        let first_order = &orders_out.data["open_orders"][0];
        assert_eq!(first_order["stop_price"], json!(85000.0));
        assert_eq!(first_order["trigger_signal"], json!("mark"));
        assert_eq!(orders_out.rows[0][6], "85000.00");
        assert_eq!(orders_out.rows[0][7], "mark");

        let status_out = format_order_status(&state, &placed.order_id).unwrap();
        assert_eq!(status_out.data["stop_price"], json!(85000.0));
        assert_eq!(status_out.data["trigger_signal"], json!("mark"));
        assert!(
            status_out
                .rows
                .iter()
                .any(|r| r[0] == "Stop Price" && r[1] == "85000.00")
        );
        assert!(
            status_out
                .rows
                .iter()
                .any(|r| r[0] == "Trigger Signal" && r[1] == "mark")
        );
    }
}
