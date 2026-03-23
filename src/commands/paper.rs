use std::collections::{HashMap, HashSet};

use clap::Subcommand;
use serde_json::{json, Value};

use crate::client::SpotClient;
use crate::errors::{KrakenError, Result};
use crate::output::{self, CommandOutput};
use crate::paper::{
    load_state, migrate_legacy_state, paper_state_path, parse_pair, save_state, OrderSide,
    PaperState, PaperTrade,
};
use crate::{build_spot_client, AppContext};

const DUST_THRESHOLD: f64 = 1e-12;

#[derive(Debug, Subcommand)]
pub(crate) enum PaperCommand {
    /// Initialize paper trading account.
    Init {
        /// Starting balance (default: 10000).
        #[arg(long, default_value = "10000")]
        balance: f64,
        /// Starting currency (default: USD).
        #[arg(long, default_value = "USD")]
        currency: String,
        /// Fee rate as a decimal (default: 0.0026 = 0.26% Kraken Starter tier).
        #[arg(long)]
        fee_rate: Option<f64>,
        /// Slippage rate as a decimal (default: 0.0 = no slippage simulation).
        #[arg(long)]
        slippage_rate: Option<f64>,
    },
    /// Reset paper account. Optionally override balance, currency, or fee rate.
    Reset {
        /// New starting balance (default: keep previous).
        #[arg(long)]
        balance: Option<f64>,
        /// New starting currency (default: keep previous).
        #[arg(long)]
        currency: Option<String>,
        /// New fee rate (default: keep previous).
        #[arg(long)]
        fee_rate: Option<f64>,
        /// Slippage rate as a decimal (default: keep previous).
        #[arg(long)]
        slippage_rate: Option<f64>,
    },
    /// Show paper balances.
    Balance,
    /// Place a paper buy order.
    Buy {
        /// Trading pair (e.g., BTCUSD).
        pair: String,
        /// Order volume.
        volume: String,
        /// Order type: market (default) or limit.
        #[arg(long, default_value = "market")]
        r#type: String,
        /// Limit price (required for limit orders).
        #[arg(long)]
        price: Option<String>,
    },
    /// Place a paper sell order.
    Sell {
        /// Trading pair (e.g., BTCUSD).
        pair: String,
        /// Order volume.
        volume: String,
        /// Order type: market (default) or limit.
        #[arg(long, default_value = "market")]
        r#type: String,
        /// Limit price (required for limit orders).
        #[arg(long)]
        price: Option<String>,
    },
    /// Show open paper limit orders.
    Orders,
    /// Cancel a paper limit order.
    Cancel {
        /// Order ID (e.g., PAPER-00001).
        order_id: String,
    },
    /// Cancel all paper limit orders.
    CancelAll,
    /// Show paper trade history.
    History,
    /// Show paper account summary with P&L.
    Status,
}

pub(crate) async fn execute(
    cmd: &PaperCommand,
    ctx: &AppContext,
    verbose: bool,
) -> Result<CommandOutput> {
    match cmd {
        PaperCommand::Init {
            balance,
            currency,
            fee_rate,
            slippage_rate,
        } => execute_init(*balance, currency, *fee_rate, *slippage_rate),
        PaperCommand::Reset {
            balance,
            currency,
            fee_rate,
            slippage_rate,
        } => {
            let client = build_spot_client(ctx)?;
            execute_reset(
                balance.as_ref().copied(),
                currency.as_deref(),
                fee_rate.as_ref().copied(),
                slippage_rate.as_ref().copied(),
                &client,
                verbose,
            )
            .await
        }
        PaperCommand::Balance => {
            let client = build_spot_client(ctx)?;
            execute_balance(&client, verbose).await
        }
        PaperCommand::Buy {
            pair,
            volume,
            r#type,
            price,
        } => {
            let client = build_spot_client(ctx)?;
            execute_trade(
                OrderSide::Buy,
                pair,
                volume,
                r#type,
                price,
                &client,
                verbose,
            )
            .await
        }
        PaperCommand::Sell {
            pair,
            volume,
            r#type,
            price,
        } => {
            let client = build_spot_client(ctx)?;
            execute_trade(
                OrderSide::Sell,
                pair,
                volume,
                r#type,
                price,
                &client,
                verbose,
            )
            .await
        }
        PaperCommand::Orders => {
            let client = build_spot_client(ctx)?;
            execute_orders(&client, verbose).await
        }
        PaperCommand::Cancel { order_id } => {
            let client = build_spot_client(ctx)?;
            execute_cancel(order_id, &client, verbose).await
        }
        PaperCommand::CancelAll => {
            let client = build_spot_client(ctx)?;
            execute_cancel_all(&client, verbose).await
        }
        PaperCommand::History => {
            let client = build_spot_client(ctx)?;
            execute_history(&client, verbose).await
        }
        PaperCommand::Status => {
            let client = build_spot_client(ctx)?;
            execute_status(&client, verbose).await
        }
    }
}

fn execute_init(balance: f64, currency: &str, fee_rate: Option<f64>, slippage_rate: Option<f64>) -> Result<CommandOutput> {
    use crate::paper::{DEFAULT_FEE_RATE, DEFAULT_SLIPPAGE_RATE};

    let migrated = migrate_legacy_state()?;
    let path = paper_state_path()?;
    if path.exists() {
        let msg = if migrated {
            "Existing paper account found and migrated to new location. Use 'kraken paper reset' to start fresh."
        } else {
            "Paper account already initialized. Use 'kraken paper reset' to start over."
        };
        return Err(KrakenError::Validation(msg.into()));
    }
    if !balance.is_finite() || balance <= 0.0 {
        return Err(KrakenError::Validation(
            "Starting balance must be a finite positive number".into(),
        ));
    }

    let rate = fee_rate.unwrap_or(DEFAULT_FEE_RATE);
    if !rate.is_finite() || !(0.0..=1.0).contains(&rate) {
        return Err(KrakenError::Validation(
            "Fee rate must be between 0.0 and 1.0 (e.g., 0.0026 for 0.26%)".into(),
        ));
    }

    let slip = slippage_rate.unwrap_or(DEFAULT_SLIPPAGE_RATE);
    if !slip.is_finite() || !(0.0..=1.0).contains(&slip) {
        return Err(KrakenError::Validation(
            "Slippage rate must be between 0.0 and 1.0 (e.g., 0.001 for 0.1%)".into(),
        ));
    }

    let state = PaperState::with_fee_rate(balance, currency, rate, slip);
    save_state(&state)?;

    let cur = currency.to_uppercase();
    let fee_pct = format!("{:.2}%", rate * 100.0);
    let slip_pct = format!("{:.2}%", slip * 100.0);
    let pairs = vec![
        ("Mode".into(), "[PAPER] Simulated Trading".into()),
        ("Action".into(), "Account initialized".into()),
        ("Starting Balance".into(), format!("{balance:.2} {cur}")),
        ("Fee Rate".into(), fee_pct),
        ("Slippage Rate".into(), slip_pct),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        paper_json(json!({
            "action": "initialized",
            "starting_balance": balance,
            "starting_currency": cur,
            "fee_rate": rate,
            "slipage_rate": slip,
        })),
    ))
}

async fn reconcile_best_effort(
    state: &mut PaperState,
    client: &SpotClient,
    verbose: bool,
) -> Vec<PaperTrade> {
    match reconcile_and_persist(state, client, verbose).await {
        Ok(fills) => fills,
        Err(err) => {
            output::warn(&format!("Could not sync pending orders: {err}"));
            Vec::new()
        }
    }
}

async fn execute_reset(
    balance: Option<f64>,
    currency: Option<&str>,
    fee_rate: Option<f64>,
    slippage_rate: Option<f64>,
    client: &SpotClient,
    verbose: bool,
) -> Result<CommandOutput> {
    if let Some(b) = balance {
        if !b.is_finite() || b <= 0.0 {
            return Err(KrakenError::Validation(
                "Starting balance must be a finite positive number".into(),
            ));
        }
    }
    if let Some(r) = fee_rate {
        if !r.is_finite() || !(0.0..=1.0).contains(&r) {
            return Err(KrakenError::Validation(
                "Fee rate must be between 0.0 and 1.0 (e.g., 0.0026 for 0.26%)".into(),
            ));
        }
    }
    if let Some(s) = slippage_rate {
        if !s.is_finite() || !(0.0..=1.0).contains(&s) {
            return Err(KrakenError::Validation(
                "Slippage rate must be between 0.0 and 1.0 (e.g., 0.001 for 0.1%)".into(),
            ))
        }
    }

    let mut state = load_state()?;
    reconcile_best_effort(&mut state, client, verbose).await;
    state.reset_with(balance, currency, fee_rate, slippage_rate);
    save_state(&state)?;

    let bal = state.starting_balance;
    let cur = state.starting_currency.clone();
    let fee_pct = format!("{:.2}%", state.fee_rate * 100.0);

    let pairs = vec![
        ("Mode".into(), "[PAPER] Simulated Trading".into()),
        ("Action".into(), "Account reset".into()),
        ("Starting Balance".into(), format!("{bal:.2} {cur}")),
        ("Fee Rate".into(), fee_pct),
        ("Slippage Rate".into(), format!("{:.2}%", state.slippage_rate * 100.0))
    ];

    Ok(CommandOutput::key_value(
        pairs,
        paper_json(json!({
            "action": "reset",
            "starting_balance": bal,
            "starting_currency": cur,
            "fee_rate": state.fee_rate,
            "slippage_rate": state.slippage_rate,
        })),
    ))
}

async fn execute_balance(client: &SpotClient, verbose: bool) -> Result<CommandOutput> {
    let mut state = load_state()?;
    reconcile_best_effort(&mut state, client, verbose).await;

    let mut sorted_assets: Vec<_> = state
        .balances
        .iter()
        .filter(|(_, &v)| v.abs() > DUST_THRESHOLD)
        .collect();
    sorted_assets.sort_by(|(a, _), (b, _)| a.cmp(b));

    let headers = vec![
        "[PAPER] Asset".into(),
        "Total".into(),
        "Reserved".into(),
        "Available".into(),
    ];
    let rows: Vec<Vec<String>> = sorted_assets
        .iter()
        .map(|(asset, &total)| {
            let reserved = state.reserved.get(*asset).copied().unwrap_or(0.0);
            let available = state.available_balance(asset);
            vec![
                asset.to_string(),
                format!("{total:.8}"),
                format!("{reserved:.8}"),
                format!("{available:.8}"),
            ]
        })
        .collect();

    let json_balances: HashMap<&str, Value> = sorted_assets
        .iter()
        .map(|(asset, &total)| {
            let reserved = state.reserved.get(*asset).copied().unwrap_or(0.0);
            let available = state.available_balance(asset);
            (
                asset.as_str(),
                json!({
                    "total": total,
                    "reserved": reserved,
                    "available": available,
                }),
            )
        })
        .collect();

    Ok(CommandOutput::new(
        paper_json(json!({ "balances": json_balances })),
        headers,
        rows,
    ))
}

fn validate_order_type(order_type_str: &str) -> Result<bool> {
    if order_type_str.eq_ignore_ascii_case("limit") {
        Ok(true)
    } else if order_type_str.eq_ignore_ascii_case("market") {
        Ok(false)
    } else {
        Err(KrakenError::Validation(format!(
            "Invalid order type '{order_type_str}'. Must be 'market' or 'limit'."
        )))
    }
}

async fn execute_trade(
    side: OrderSide,
    pair: &str,
    volume_str: &str,
    order_type_str: &str,
    price_str: &Option<String>,
    client: &SpotClient,
    verbose: bool,
) -> Result<CommandOutput> {
    let mut state = load_state()?;
    reconcile_best_effort(&mut state, client, verbose).await;

    let volume: f64 = volume_str
        .parse()
        .map_err(|_| KrakenError::Validation(format!("Invalid volume: {volume_str}")))?;

    let is_limit = validate_order_type(order_type_str)?;

    if is_limit {
        let price_val = price_str
            .as_ref()
            .ok_or_else(|| KrakenError::Validation("Limit orders require --price".into()))?;
        let price: f64 = price_val
            .parse()
            .map_err(|_| KrakenError::Validation(format!("Invalid price: {price_val}")))?;

        let order_id = state.place_limit_order(side, pair, volume, price)?;
        save_state(&state)?;

        let (normalized, _, _) = parse_pair(pair)?;
        let pairs = vec![
            ("Mode".into(), "[PAPER] Simulated Trading".into()),
            ("Action".into(), format!("Limit {side} order placed")),
            ("Order ID".into(), order_id.clone()),
            ("Pair".into(), normalized),
            ("Volume".into(), format!("{volume:.8}")),
            ("Price".into(), format!("{price:.2}")),
        ];

        Ok(CommandOutput::key_value(
            pairs,
            paper_json(json!({
                "action": "limit_order_placed",
                "order_id": order_id,
                "side": format!("{side}").to_lowercase(),
                "pair": pair.to_uppercase(),
                "volume": volume,
                "price": price,
            })),
        ))
    } else {
        let (normalized, _, _) = parse_pair(pair)?;
        let (ask, bid) = fetch_ticker_price(client, &normalized, verbose).await?;
        let trade = state.place_market_order(side, pair, volume, ask, bid)?;
        save_state(&state)?;

        let pairs = vec![
            ("Mode".into(), "[PAPER] Simulated Trading".into()),
            ("Action".into(), format!("Market {side} executed")),
            ("Trade ID".into(), trade.id.clone()),
            ("Pair".into(), trade.pair.clone()),
            ("Volume".into(), format!("{:.8}", trade.volume)),
            ("Price".into(), format!("{:.2}", trade.price)),
            ("Fee".into(), format!("{:.4}", trade.fee)),
            ("Cost".into(), format!("{:.2}", trade.cost)),
        ];

        Ok(CommandOutput::key_value(
            pairs,
            paper_json(json!({
                "action": "market_order_filled",
                "trade_id": trade.id,
                "order_id": trade.order_id,
                "side": format!("{side}").to_lowercase(),
                "pair": trade.pair,
                "volume": trade.volume,
                "price": trade.price,
                "fee": trade.fee,
                "cost": trade.cost,
            })),
        ))
    }
}

async fn execute_orders(client: &SpotClient, verbose: bool) -> Result<CommandOutput> {
    let mut state = load_state()?;
    reconcile_best_effort(&mut state, client, verbose).await;

    let headers = vec![
        "[PAPER] Order ID".into(),
        "Pair".into(),
        "Side".into(),
        "Type".into(),
        "Volume".into(),
        "Price".into(),
        "Reserved".into(),
        "Created".into(),
    ];

    let rows: Vec<Vec<String>> = if state.open_orders.is_empty() {
        vec![vec![
            "—".into(),
            "No open orders".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
        ]]
    } else {
        state
            .open_orders
            .iter()
            .map(|o| {
                vec![
                    o.id.clone(),
                    o.pair.clone(),
                    format!("{}", o.side),
                    format!("{}", o.order_type),
                    format!("{:.8}", o.volume),
                    format!("{:.2}", o.price),
                    format!("{:.8} {}", o.reserved_amount, o.reserved_asset),
                    o.created_at.clone(),
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
                "pair": o.pair,
                "side": format!("{}", o.side).to_lowercase(),
                "type": format!("{}", o.order_type).to_lowercase(),
                "volume": o.volume,
                "price": o.price,
                "reserved_amount": o.reserved_amount,
                "reserved_asset": o.reserved_asset,
                "created_at": o.created_at,
            })
        })
        .collect();

    Ok(CommandOutput::new(
        paper_json(json!({
            "open_orders": json_orders,
            "count": state.open_orders.len(),
        })),
        headers,
        rows,
    ))
}

async fn execute_cancel(
    order_id: &str,
    client: &SpotClient,
    verbose: bool,
) -> Result<CommandOutput> {
    let mut state = load_state()?;
    reconcile_best_effort(&mut state, client, verbose).await;

    let order = state.cancel_order(order_id)?;
    save_state(&state)?;

    let pairs = vec![
        ("Mode".into(), "[PAPER] Simulated Trading".into()),
        ("Action".into(), "Order cancelled".into()),
        ("Order ID".into(), order.id.clone()),
        ("Pair".into(), order.pair.clone()),
        ("Side".into(), format!("{}", order.side)),
        (
            "Released".into(),
            format!("{:.8} {}", order.reserved_amount, order.reserved_asset),
        ),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        paper_json(json!({
            "action": "order_cancelled",
            "order_id": order.id,
            "pair": order.pair,
            "side": format!("{}", order.side).to_lowercase(),
            "released_amount": order.reserved_amount,
            "released_asset": order.reserved_asset,
        })),
    ))
}

async fn execute_cancel_all(client: &SpotClient, verbose: bool) -> Result<CommandOutput> {
    let mut state = load_state()?;
    reconcile_best_effort(&mut state, client, verbose).await;

    let cancelled = state.cancel_all_orders();
    save_state(&state)?;

    let count = cancelled.len();
    let pairs = vec![
        ("Mode".into(), "[PAPER] Simulated Trading".into()),
        ("Action".into(), "All orders cancelled".into()),
        ("Cancelled".into(), count.to_string()),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        paper_json(json!({
            "action": "all_orders_cancelled",
            "cancelled_count": count,
        })),
    ))
}

async fn execute_history(client: &SpotClient, verbose: bool) -> Result<CommandOutput> {
    let mut state = load_state()?;
    reconcile_best_effort(&mut state, client, verbose).await;

    let headers = vec![
        "[PAPER] ID".into(),
        "Order ID".into(),
        "Status".into(),
        "Pair".into(),
        "Side".into(),
        "Volume".into(),
        "Price".into(),
        "Fee".into(),
        "Cost".into(),
        "Time".into(),
    ];

    let has_fills = !state.filled_trades.is_empty();
    let has_cancels = !state.cancelled_orders.is_empty();

    let mut rows: Vec<Vec<String>> = Vec::new();

    for t in &state.filled_trades {
        rows.push(vec![
            t.id.clone(),
            t.order_id.clone(),
            "Filled".into(),
            t.pair.clone(),
            format!("{}", t.side),
            format!("{:.8}", t.volume),
            format!("{:.2}", t.price),
            format!("{:.4}", t.fee),
            format!("{:.2}", t.cost),
            t.filled_at.clone(),
        ]);
    }

    for o in &state.cancelled_orders {
        rows.push(vec![
            o.id.clone(),
            "—".into(),
            "Cancelled".into(),
            o.pair.clone(),
            format!("{}", o.side),
            format!("{:.8}", o.volume),
            format!("{:.2}", o.price),
            "—".into(),
            "—".into(),
            o.created_at.clone(),
        ]);
    }

    if !has_fills && !has_cancels {
        rows.push(vec![
            "—".into(),
            "—".into(),
            "No history yet".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
        ]);
    }

    let json_trades: Vec<Value> = state
        .filled_trades
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "order_id": t.order_id,
                "status": "filled",
                "pair": t.pair,
                "side": format!("{}", t.side).to_lowercase(),
                "volume": t.volume,
                "price": t.price,
                "fee": t.fee,
                "cost": t.cost,
                "time": t.filled_at,
            })
        })
        .collect();

    let json_cancelled: Vec<Value> = state
        .cancelled_orders
        .iter()
        .map(|o| {
            json!({
                "id": o.id,
                "status": "cancelled",
                "pair": o.pair,
                "side": format!("{}", o.side).to_lowercase(),
                "volume": o.volume,
                "price": o.price,
                "time": o.created_at,
            })
        })
        .collect();

    Ok(CommandOutput::new(
        paper_json(json!({
            "trades": json_trades,
            "cancelled": json_cancelled,
            "filled_count": state.filled_trades.len(),
            "cancelled_count": state.cancelled_orders.len(),
        })),
        headers,
        rows,
    ))
}

async fn execute_status(client: &SpotClient, verbose: bool) -> Result<CommandOutput> {
    let mut state = load_state()?;
    reconcile_best_effort(&mut state, client, verbose).await;

    let sc = state.starting_currency.clone();

    let val_pairs: Vec<String> = state
        .balances
        .iter()
        .filter(|(a, &amt)| *a != &sc && amt > DUST_THRESHOLD)
        .map(|(a, _)| format!("{a}{sc}"))
        .collect();

    let (portfolio_value, valuation_complete) = if val_pairs.is_empty() {
        (state.balances.get(&sc).copied().unwrap_or(0.0), true)
    } else {
        match fetch_ticker_prices(client, &val_pairs, verbose).await {
            Ok(prices) => state.compute_portfolio_value(&prices),
            Err(err) => {
                output::warn(&format!("Could not fetch prices for valuation: {err}"));
                (state.balances.get(&sc).copied().unwrap_or(0.0), false)
            }
        }
    };

    let pnl = portfolio_value - state.starting_balance;
    let pnl_pct = if state.starting_balance > 0.0 {
        (pnl / state.starting_balance) * 100.0
    } else {
        0.0
    };

    let partial_marker = if valuation_complete { "" } else { " (partial)" };

    let pairs = vec![
        ("Mode".into(), "[PAPER] Simulated Trading".into()),
        (
            "Starting Balance".into(),
            format!("{:.2} {sc}", state.starting_balance),
        ),
        (
            "Current Value".into(),
            format!("{portfolio_value:.2} {sc}{partial_marker}"),
        ),
        (
            "Unrealized P&L".into(),
            format!("{pnl:+.2} {sc} ({pnl_pct:+.2}%){partial_marker}"),
        ),
        ("Total Trades".into(), state.filled_trades.len().to_string()),
        ("Open Orders".into(), state.open_orders.len().to_string()),
        (
            "Assets Held".into(),
            state
                .balances
                .iter()
                .filter(|(_, &v)| v > DUST_THRESHOLD)
                .count()
                .to_string(),
        ),
    ];

    Ok(CommandOutput::key_value(
        pairs,
        paper_json(json!({
            "starting_balance": state.starting_balance,
            "starting_currency": sc,
            "current_value": portfolio_value,
            "unrealized_pnl": pnl,
            "unrealized_pnl_pct": pnl_pct,
            "valuation_complete": valuation_complete,
            "total_trades": state.filled_trades.len(),
            "open_orders": state.open_orders.len(),
        })),
    ))
}

fn paper_json(mut data: Value) -> Value {
    if let Some(obj) = data.as_object_mut() {
        obj.insert("mode".to_string(), json!("paper"));
    }
    data
}

async fn fetch_ticker_price(client: &SpotClient, pair: &str, verbose: bool) -> Result<(f64, f64)> {
    let data = client
        .public_get("Ticker", &[("pair", pair)], verbose)
        .await?;
    let obj = data
        .as_object()
        .ok_or_else(|| KrakenError::Parse(format!("Unexpected ticker response for {pair}")))?;
    let (_, val) = obj
        .iter()
        .next()
        .ok_or_else(|| KrakenError::Parse(format!("Empty ticker response for {pair}")))?;
    let ask = extract_price(val, "a", pair)?;
    let bid = extract_price(val, "b", pair)?;
    Ok((ask, bid))
}

fn extract_price(val: &Value, field: &str, pair: &str) -> Result<f64> {
    val.get(field)
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| KrakenError::Parse(format!("Missing {field} price in ticker for {pair}")))?
        .parse::<f64>()
        .map_err(|e| KrakenError::Parse(format!("Invalid {field} price in ticker for {pair}: {e}")))
}

async fn fetch_ticker_prices(
    client: &SpotClient,
    pairs: &[String],
    verbose: bool,
) -> Result<HashMap<String, (f64, f64)>> {
    if pairs.is_empty() {
        return Ok(HashMap::new());
    }
    let pair_param = pairs.join(",");
    let data = client
        .public_get("Ticker", &[("pair", pair_param.as_str())], verbose)
        .await?;
    let obj = data
        .as_object()
        .ok_or_else(|| KrakenError::Parse("Unexpected ticker response format".into()))?;

    let mut response_prices: Vec<(String, String, f64, f64)> = Vec::new();
    for (key, val) in obj {
        let ask = extract_price(val, "a", key)?;
        let bid = extract_price(val, "b", key)?;
        if let Ok((_, base, quote)) = parse_pair(key) {
            response_prices.push((base, quote, ask, bid));
        }
    }

    let mut prices = HashMap::new();
    for input_pair in pairs {
        let (_, input_base, input_quote) = parse_pair(input_pair)?;
        let matched = response_prices
            .iter()
            .find(|(b, q, _, _)| *b == input_base && *q == input_quote);
        if let Some((_, _, ask, bid)) = matched {
            prices.insert(input_pair.clone(), (*ask, *bid));
        } else {
            return Err(KrakenError::Parse(format!(
                "No ticker data for {input_pair} in batch response"
            )));
        }
    }
    Ok(prices)
}

async fn reconcile_and_persist(
    state: &mut PaperState,
    client: &SpotClient,
    verbose: bool,
) -> Result<Vec<PaperTrade>> {
    if state.open_orders.is_empty() {
        return Ok(Vec::new());
    }
    let unique_pairs: Vec<String> = state
        .open_orders
        .iter()
        .map(|o| o.pair.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let prices = fetch_ticker_prices(client, &unique_pairs, verbose).await?;
    let fills = state.check_pending_orders(&prices);
    if !fills.is_empty() {
        save_state(state)?;
        if verbose {
            output::verbose(&format!("{} limit order(s) filled", fills.len()));
        }
    }
    Ok(fills)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_price_valid() {
        let val = json!({
            "a": ["50000.00", "1", "1.000"],
            "b": ["49999.00", "1", "1.000"],
        });
        let ask = extract_price(&val, "a", "BTCUSD").unwrap();
        let bid = extract_price(&val, "b", "BTCUSD").unwrap();
        assert!((ask - 50000.0).abs() < 1e-10);
        assert!((bid - 49999.0).abs() < 1e-10);
    }

    #[test]
    fn test_extract_price_missing_field() {
        let val = json!({ "b": ["49999.00"] });
        let result = extract_price(&val, "a", "BTCUSD");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing a price"));
    }

    #[test]
    fn test_extract_price_non_numeric() {
        let val = json!({ "a": ["not_a_number"] });
        let result = extract_price(&val, "a", "BTCUSD");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid a price"));
    }

    #[test]
    fn test_extract_price_empty_array() {
        let val = json!({ "a": [] });
        let result = extract_price(&val, "a", "BTCUSD");
        assert!(result.is_err());
    }

    #[test]
    fn test_paper_json_injects_mode() {
        let data = paper_json(json!({"foo": "bar"}));
        assert_eq!(data.get("mode").and_then(|v| v.as_str()), Some("paper"));
        assert_eq!(data.get("foo").and_then(|v| v.as_str()), Some("bar"));
    }

    #[test]
    fn test_validate_order_type_rejects_garbage() {
        let result = validate_order_type("garbage");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Invalid order type"),
            "error must describe the problem"
        );
        assert!(
            err.contains("garbage"),
            "error must include the invalid value"
        );
        assert!(err.contains("market"), "error must list valid values");
        assert!(err.contains("limit"), "error must list valid values");
    }

    #[test]
    fn test_validate_order_type_accepts_valid() {
        assert!(!validate_order_type("market").unwrap());
        assert!(validate_order_type("limit").unwrap());
        assert!(!validate_order_type("MARKET").unwrap());
        assert!(validate_order_type("Limit").unwrap());
        assert!(validate_order_type("LIMIT").unwrap());
        assert!(!validate_order_type("Market").unwrap());
    }

    #[test]
    fn test_status_partial_marker_formatting() {
        let valuation_complete_false = false;
        let partial_marker = if valuation_complete_false {
            ""
        } else {
            " (partial)"
        };
        assert_eq!(partial_marker, " (partial)");

        let valuation_complete_true = true;
        let no_marker = if valuation_complete_true {
            ""
        } else {
            " (partial)"
        };
        assert_eq!(no_marker, "");

        let mode_label = "[PAPER] Simulated Trading";
        assert!(mode_label.starts_with("[PAPER]"));
    }
}
