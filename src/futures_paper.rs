use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::errors::{KrakenError, Result};

pub(crate) const DEFAULT_FUTURES_TAKER_FEE_RATE: f64 = 0.0005;
pub(crate) const DEFAULT_MAINTENANCE_MARGIN_RATE: f64 = 0.02;
pub(crate) const MAX_LEVERAGE: f64 = 200.0;
const FUNDING_INTERVAL_HOURS: i64 = 8;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Side {
    Long,
    Short,
}

impl Side {
    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Long => Self::Short,
            Self::Short => Self::Long,
        }
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Long => f.write_str("long"),
            Self::Short => f.write_str("short"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FuturesOrderType {
    Market,
    Limit,
    Post,
    Stop,
    TakeProfit,
    Ioc,
    TrailingStop,
    Fok,
}

impl FuturesOrderType {
    pub(crate) fn from_str_cli(s: &str) -> Result<Self> {
        match s {
            "market" => Ok(Self::Market),
            "limit" => Ok(Self::Limit),
            "post" => Ok(Self::Post),
            "stop" => Ok(Self::Stop),
            "take-profit" => Ok(Self::TakeProfit),
            "ioc" => Ok(Self::Ioc),
            "trailing-stop" => Ok(Self::TrailingStop),
            "fok" => Ok(Self::Fok),
            _ => Err(KrakenError::Validation(format!(
                "Invalid order type '{s}'. Valid: limit, market, post, stop, take-profit, ioc, trailing-stop, fok"
            ))),
        }
    }
}

impl std::fmt::Display for FuturesOrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Market => "market",
            Self::Limit => "limit",
            Self::Post => "post",
            Self::Stop => "stop",
            Self::TakeProfit => "take-profit",
            Self::Ioc => "ioc",
            Self::TrailingStop => "trailing-stop",
            Self::Fok => "fok",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum OrderStatus {
    Open,
    Triggered,
    Filled,
    PartiallyFilled,
    Cancelled,
    Rejected,
}

impl std::fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => f.write_str("open"),
            Self::Triggered => f.write_str("triggered"),
            Self::Filled => f.write_str("filled"),
            Self::PartiallyFilled => f.write_str("partially_filled"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::Rejected => f.write_str("rejected"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TriggerSignal {
    Mark,
    Index,
    Last,
}

impl TriggerSignal {
    pub(crate) fn from_str_cli(s: &str) -> Result<Self> {
        match s {
            "mark" => Ok(Self::Mark),
            "index" => Ok(Self::Index),
            "last" => Ok(Self::Last),
            _ => Err(KrakenError::Validation(format!(
                "Invalid trigger signal '{s}'. Valid: mark, index, last"
            ))),
        }
    }
}

impl std::fmt::Display for TriggerSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mark => f.write_str("mark"),
            Self::Index => f.write_str("index"),
            Self::Last => f.write_str("last"),
        }
    }
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FuturesPaperOrder {
    pub(crate) id: String,
    pub(crate) symbol: String,
    pub(crate) side: Side,
    pub(crate) size: f64,
    pub(crate) filled_size: f64,
    pub(crate) order_type: FuturesOrderType,
    pub(crate) price: Option<f64>,
    pub(crate) stop_price: Option<f64>,
    pub(crate) trigger_signal: Option<TriggerSignal>,
    pub(crate) client_order_id: Option<String>,
    pub(crate) reduce_only: bool,
    pub(crate) leverage: f64,
    pub(crate) reserved_margin: f64,
    pub(crate) status: OrderStatus,
    pub(crate) trailing_stop_max_deviation: Option<f64>,
    pub(crate) trailing_stop_deviation_unit: Option<String>,
    pub(crate) trailing_anchor: Option<f64>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FuturesPaperPosition {
    pub(crate) symbol: String,
    pub(crate) side: Side,
    pub(crate) size: f64,
    pub(crate) entry_price: f64,
    pub(crate) leverage: f64,
    pub(crate) unrealized_funding: f64,
    pub(crate) last_funding_time: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FuturesPaperFill {
    pub(crate) id: String,
    pub(crate) order_id: String,
    pub(crate) symbol: String,
    pub(crate) side: Side,
    pub(crate) size: f64,
    pub(crate) price: f64,
    pub(crate) fee: f64,
    pub(crate) realized_pnl: Option<f64>,
    pub(crate) fill_type: String,
    pub(crate) client_order_id: Option<String>,
    pub(crate) filled_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FuturesPaperHistoryEvent {
    pub(crate) id: String,
    pub(crate) event_type: String,
    pub(crate) symbol: Option<String>,
    pub(crate) amount: f64,
    pub(crate) details: String,
    pub(crate) timestamp: String,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FuturesPaperState {
    pub(crate) collateral: f64,
    pub(crate) currency: String,
    pub(crate) fee_rate: f64,
    pub(crate) starting_collateral: f64,
    pub(crate) open_orders: Vec<FuturesPaperOrder>,
    pub(crate) positions: Vec<FuturesPaperPosition>,
    pub(crate) fills: Vec<FuturesPaperFill>,
    pub(crate) history: Vec<FuturesPaperHistoryEvent>,
    pub(crate) leverage_preferences: HashMap<String, f64>,
    #[serde(default = "default_next_id")]
    next_id: u64,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    #[serde(default)]
    pub(crate) last_reconciled_at: Option<String>,
    #[serde(default)]
    pub(crate) maintenance_margin_fallback_used: bool,
}

fn default_next_id() -> u64 {
    1
}

impl FuturesPaperState {
    pub(crate) fn new(collateral: f64, currency: &str, fee_rate: f64) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            collateral,
            currency: currency.to_uppercase(),
            fee_rate,
            starting_collateral: collateral,
            open_orders: Vec::new(),
            positions: Vec::new(),
            fills: Vec::new(),
            history: Vec::new(),
            leverage_preferences: HashMap::new(),
            next_id: 1,
            created_at: now.clone(),
            updated_at: now,
            last_reconciled_at: None,
            maintenance_margin_fallback_used: false,
        }
    }

    pub(crate) fn reset(
        &mut self,
        collateral: Option<f64>,
        currency: Option<&str>,
        fee_rate: Option<f64>,
    ) {
        let col = collateral.unwrap_or(self.starting_collateral);
        let cur = currency
            .map(|c| c.to_uppercase())
            .unwrap_or_else(|| self.currency.clone());
        let fee = fee_rate.unwrap_or(self.fee_rate);
        *self = Self::new(col, &cur, fee);
    }

    fn next_id(&mut self) -> String {
        let id = format!("FP-{:05}", self.next_id);
        self.next_id += 1;
        id
    }

    // -----------------------------------------------------------------------
    // Margin calculations
    // -----------------------------------------------------------------------

    pub(crate) fn position_margin(&self) -> f64 {
        self.positions
            .iter()
            .map(|p| (p.size * p.entry_price) / p.leverage)
            .sum()
    }

    pub(crate) fn reserved_order_margin(&self) -> f64 {
        self.open_orders.iter().map(|o| o.reserved_margin).sum()
    }

    pub(crate) fn unrealized_pnl(&self, mark_prices: &HashMap<String, f64>) -> f64 {
        self.positions
            .iter()
            .map(|p| {
                let mark = mark_prices.get(&p.symbol).copied().unwrap_or(p.entry_price);
                compute_unrealized_pnl(p, mark)
            })
            .sum()
    }

    pub(crate) fn used_margin(&self) -> f64 {
        self.position_margin() + self.reserved_order_margin()
    }

    pub(crate) fn available_margin(&self, mark_prices: &HashMap<String, f64>) -> f64 {
        let upnl = self.unrealized_pnl(mark_prices);
        (self.collateral + upnl - self.used_margin()).max(0.0)
    }

    // -----------------------------------------------------------------------
    // Leverage resolution
    // -----------------------------------------------------------------------

    pub(crate) fn resolve_leverage(
        &self,
        order_leverage: Option<f64>,
        symbol: &str,
    ) -> Result<f64> {
        if let Some(lev) = order_leverage {
            if !lev.is_finite() || !(1.0..=MAX_LEVERAGE).contains(&lev) {
                return Err(KrakenError::Validation(format!(
                    "Leverage must be between 1.0 and {MAX_LEVERAGE:.0}"
                )));
            }
            return Ok(lev);
        }
        if let Some(&pref) = self.leverage_preferences.get(symbol) {
            return Ok(pref);
        }
        Err(KrakenError::Validation(format!(
            "No leverage specified for {symbol}. Use --leverage or set a preference with 'futures paper set-leverage'."
        )))
    }

    // -----------------------------------------------------------------------
    // Order placement
    // -----------------------------------------------------------------------

    pub(crate) fn place_order(
        &mut self,
        params: OrderParams,
        market: &MarketSnapshot,
    ) -> Result<OrderPlacementResult> {
        validate_finite_positive(params.size, "size")?;
        if let Some(p) = params.price {
            validate_finite_positive(p, "price")?;
        }

        let leverage = self.resolve_leverage(params.leverage, &params.symbol)?;
        let empty = HashMap::new();

        match params.order_type {
            FuturesOrderType::Market => self.place_market_order(&params, leverage, market),
            FuturesOrderType::Limit => self.place_limit_order(&params, leverage, market, &empty),
            FuturesOrderType::Post => self.place_post_order(&params, leverage, market),
            FuturesOrderType::Ioc => self.place_ioc_order(&params, leverage, market),
            FuturesOrderType::Fok => self.place_fok_order(&params, leverage, market),
            FuturesOrderType::Stop
            | FuturesOrderType::TakeProfit
            | FuturesOrderType::TrailingStop => {
                self.place_triggered_order(&params, leverage, &empty)
            }
        }
    }

    fn place_market_order(
        &mut self,
        params: &OrderParams,
        leverage: f64,
        market: &MarketSnapshot,
    ) -> Result<OrderPlacementResult> {
        let fill_price = match params.side {
            Side::Long => market.ask,
            Side::Short => market.bid,
        };
        if fill_price <= 0.0 || !fill_price.is_finite() {
            return Err(KrakenError::Validation("Invalid market price".into()));
        }

        self.validate_reduce_only(params)?;
        let margin_delta = self.compute_margin_delta(params, fill_price, leverage);
        self.check_margin(margin_delta, &HashMap::new())?;

        let order_id = self.next_id();
        let fill_id = self.next_id();
        let fee = params.size * fill_price * self.fee_rate;

        let fill = self.apply_fill(
            &fill_id,
            &order_id,
            &params.symbol,
            params.side,
            params.size,
            fill_price,
            fee,
            params.client_order_id.clone(),
            "market",
            leverage,
        );

        self.collateral -= fee;
        self.updated_at = Utc::now().to_rfc3339();

        Ok(OrderPlacementResult {
            order_id,
            status: OrderStatus::Filled,
            fills: vec![fill],
            message: None,
        })
    }

    fn place_limit_order(
        &mut self,
        params: &OrderParams,
        leverage: f64,
        _market: &MarketSnapshot,
        mark_prices: &HashMap<String, f64>,
    ) -> Result<OrderPlacementResult> {
        let price = params
            .price
            .ok_or_else(|| KrakenError::Validation("Limit orders require --price".into()))?;

        self.validate_reduce_only(params)?;
        let margin_delta = self.compute_margin_delta(params, price, leverage);
        self.check_margin(margin_delta, mark_prices)?;

        let order_id = self.next_id();
        let reserved = margin_delta.max(0.0);

        let order = FuturesPaperOrder {
            id: order_id.clone(),
            symbol: params.symbol.clone(),
            side: params.side,
            size: params.size,
            filled_size: 0.0,
            order_type: FuturesOrderType::Limit,
            price: Some(price),
            stop_price: None,
            trigger_signal: None,
            client_order_id: params.client_order_id.clone(),
            reduce_only: params.reduce_only,
            leverage,
            reserved_margin: reserved,
            status: OrderStatus::Open,
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
            trailing_anchor: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };
        self.open_orders.push(order);
        self.updated_at = Utc::now().to_rfc3339();

        Ok(OrderPlacementResult {
            order_id,
            status: OrderStatus::Open,
            fills: Vec::new(),
            message: None,
        })
    }

    fn place_post_order(
        &mut self,
        params: &OrderParams,
        leverage: f64,
        market: &MarketSnapshot,
    ) -> Result<OrderPlacementResult> {
        let price = params
            .price
            .ok_or_else(|| KrakenError::Validation("Post-only orders require --price".into()))?;

        let crosses = match params.side {
            Side::Long => price >= market.ask,
            Side::Short => price <= market.bid,
        };
        if crosses {
            return Err(KrakenError::Validation(
                "Post-only order would cross the spread and be rejected".into(),
            ));
        }

        self.place_limit_order(params, leverage, market, &HashMap::new())
            .map(|mut r| {
                if let Some(o) = self.open_orders.iter_mut().find(|o| o.id == r.order_id) {
                    o.order_type = FuturesOrderType::Post;
                }
                r.message = Some("Post-only order placed".to_string());
                r
            })
    }

    fn place_ioc_order(
        &mut self,
        params: &OrderParams,
        leverage: f64,
        market: &MarketSnapshot,
    ) -> Result<OrderPlacementResult> {
        let price = params
            .price
            .ok_or_else(|| KrakenError::Validation("IOC orders require --price".into()))?;

        self.validate_reduce_only(params)?;

        let contra_levels = match params.side {
            Side::Long => &market.ask_levels,
            Side::Short => &market.bid_levels,
        };
        let executable_size =
            compute_executable_depth(contra_levels, params.side, price, params.size);

        if executable_size <= 0.0 {
            return Ok(OrderPlacementResult {
                order_id: self.next_id(),
                status: OrderStatus::Cancelled,
                fills: Vec::new(),
                message: Some("IOC: no executable depth at requested price".into()),
            });
        }

        let fill_price = match params.side {
            Side::Long => market.ask,
            Side::Short => market.bid,
        };

        let margin_delta =
            self.compute_margin_delta_for_size(params, fill_price, leverage, executable_size);
        self.check_margin(margin_delta, &HashMap::new())?;

        let order_id = self.next_id();
        let fill_id = self.next_id();
        let fee = executable_size * fill_price * self.fee_rate;

        let fill = self.apply_fill(
            &fill_id,
            &order_id,
            &params.symbol,
            params.side,
            executable_size,
            fill_price,
            fee,
            params.client_order_id.clone(),
            "ioc",
            leverage,
        );

        self.collateral -= fee;
        self.updated_at = Utc::now().to_rfc3339();

        let status = if (executable_size - params.size).abs() < 1e-12 {
            OrderStatus::Filled
        } else {
            OrderStatus::PartiallyFilled
        };

        Ok(OrderPlacementResult {
            order_id,
            status,
            fills: vec![fill],
            message: if status == OrderStatus::PartiallyFilled {
                Some(format!(
                    "IOC: partial fill {executable_size:.8} of {:.8}, remainder cancelled",
                    params.size
                ))
            } else {
                None
            },
        })
    }

    fn place_fok_order(
        &mut self,
        params: &OrderParams,
        leverage: f64,
        market: &MarketSnapshot,
    ) -> Result<OrderPlacementResult> {
        let price = params
            .price
            .ok_or_else(|| KrakenError::Validation("FOK orders require --price".into()))?;

        self.validate_reduce_only(params)?;

        let contra_levels = match params.side {
            Side::Long => &market.ask_levels,
            Side::Short => &market.bid_levels,
        };
        let executable_size =
            compute_executable_depth(contra_levels, params.side, price, params.size);

        if (executable_size - params.size).abs() > 1e-12 {
            return Ok(OrderPlacementResult {
                order_id: self.next_id(),
                status: OrderStatus::Rejected,
                fills: Vec::new(),
                message: Some(format!(
                    "FOK: insufficient depth ({executable_size:.8} available, {:.8} required)",
                    params.size
                )),
            });
        }

        let fill_price = match params.side {
            Side::Long => market.ask,
            Side::Short => market.bid,
        };

        let margin_delta = self.compute_margin_delta(params, fill_price, leverage);
        self.check_margin(margin_delta, &HashMap::new())?;

        let order_id = self.next_id();
        let fill_id = self.next_id();
        let fee = params.size * fill_price * self.fee_rate;

        let fill = self.apply_fill(
            &fill_id,
            &order_id,
            &params.symbol,
            params.side,
            params.size,
            fill_price,
            fee,
            params.client_order_id.clone(),
            "fok",
            leverage,
        );

        self.collateral -= fee;
        self.updated_at = Utc::now().to_rfc3339();

        Ok(OrderPlacementResult {
            order_id,
            status: OrderStatus::Filled,
            fills: vec![fill],
            message: None,
        })
    }

    fn place_triggered_order(
        &mut self,
        params: &OrderParams,
        leverage: f64,
        mark_prices: &HashMap<String, f64>,
    ) -> Result<OrderPlacementResult> {
        let stop_price = params.stop_price.ok_or_else(|| {
            KrakenError::Validation(format!("{} orders require --stop-price", params.order_type))
        })?;
        validate_finite_positive(stop_price, "stop-price")?;

        let resolved_trailing_unit = if params.order_type == FuturesOrderType::TrailingStop {
            let _max_dev = params.trailing_stop_max_deviation.ok_or_else(|| {
                KrakenError::Validation(
                    "Trailing-stop orders require --trailing-stop-max-deviation".into(),
                )
            })?;
            validate_finite_positive(_max_dev, "trailing-stop-max-deviation")?;
            let unit = params
                .trailing_stop_deviation_unit
                .as_deref()
                .unwrap_or("percent");
            if unit != "percent" && unit != "quote_currency" {
                return Err(KrakenError::Validation(format!(
                    "Invalid trailing-stop-deviation-unit '{unit}'. Use 'percent' or 'quote_currency'."
                )));
            }
            Some(unit.to_string())
        } else {
            params.trailing_stop_deviation_unit.clone()
        };

        self.validate_reduce_only(params)?;

        let notional = params.size * stop_price;
        let margin_delta = if params.reduce_only {
            0.0
        } else {
            notional / leverage
        };
        self.check_margin(margin_delta, mark_prices)?;

        let order_id = self.next_id();
        let trigger = params.trigger_signal.unwrap_or(TriggerSignal::Last);

        let trailing_anchor = if params.order_type == FuturesOrderType::TrailingStop {
            Some(stop_price)
        } else {
            None
        };

        let order = FuturesPaperOrder {
            id: order_id.clone(),
            symbol: params.symbol.clone(),
            side: params.side,
            size: params.size,
            filled_size: 0.0,
            order_type: params.order_type,
            price: params.price,
            stop_price: Some(stop_price),
            trigger_signal: Some(trigger),
            client_order_id: params.client_order_id.clone(),
            reduce_only: params.reduce_only,
            leverage,
            reserved_margin: margin_delta.max(0.0),
            status: OrderStatus::Open,
            trailing_stop_max_deviation: params.trailing_stop_max_deviation,
            trailing_stop_deviation_unit: resolved_trailing_unit,
            trailing_anchor,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };
        self.open_orders.push(order);
        self.updated_at = Utc::now().to_rfc3339();

        Ok(OrderPlacementResult {
            order_id,
            status: OrderStatus::Open,
            fills: Vec::new(),
            message: Some(format!(
                "{} order placed (trigger: {trigger})",
                params.order_type
            )),
        })
    }

    // -----------------------------------------------------------------------
    // Validation helpers
    // -----------------------------------------------------------------------

    fn validate_reduce_only(&self, params: &OrderParams) -> Result<()> {
        if !params.reduce_only {
            return Ok(());
        }
        let opposite_side = params.side.opposite();
        let pos = self
            .positions
            .iter()
            .find(|p| p.symbol == params.symbol && p.side == opposite_side);

        match pos {
            None => Err(KrakenError::Validation(
                "Reduce-only: no opposite-side position exists to reduce".into(),
            )),
            Some(p) if p.size < params.size => Err(KrakenError::Validation(format!(
                "Reduce-only: requested size {:.8} exceeds position size {:.8}",
                params.size, p.size
            ))),
            Some(_) => Ok(()),
        }
    }

    fn compute_margin_delta(&self, params: &OrderParams, price: f64, leverage: f64) -> f64 {
        self.compute_margin_delta_for_size(params, price, leverage, params.size)
    }

    fn compute_margin_delta_for_size(
        &self,
        params: &OrderParams,
        price: f64,
        leverage: f64,
        size: f64,
    ) -> f64 {
        if params.reduce_only {
            return 0.0;
        }

        let opposite_side = params.side.opposite();
        let opposite_pos_size = self
            .positions
            .iter()
            .find(|p| p.symbol == params.symbol && p.side == opposite_side)
            .map(|p| p.size)
            .unwrap_or(0.0);

        let net_new_exposure = (size - opposite_pos_size).max(0.0);
        (net_new_exposure * price) / leverage
    }

    fn check_margin(&self, margin_required: f64, mark_prices: &HashMap<String, f64>) -> Result<()> {
        if margin_required <= 0.0 {
            return Ok(());
        }
        let available = self.available_margin(mark_prices);
        if available < margin_required {
            return Err(KrakenError::Validation(format!(
                "Insufficient margin. Required: {margin_required:.2}, Available: {available:.2}"
            )));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Fill application and position netting
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn apply_fill(
        &mut self,
        fill_id: &str,
        order_id: &str,
        symbol: &str,
        side: Side,
        size: f64,
        price: f64,
        fee: f64,
        client_order_id: Option<String>,
        fill_type: &str,
        leverage: f64,
    ) -> FuturesPaperFill {
        let realized_pnl = self.net_position(symbol, side, size, price, leverage);

        let fill = FuturesPaperFill {
            id: fill_id.to_string(),
            order_id: order_id.to_string(),
            symbol: symbol.to_string(),
            side,
            size,
            price,
            fee,
            realized_pnl,
            fill_type: fill_type.to_string(),
            client_order_id,
            filled_at: Utc::now().to_rfc3339(),
        };
        self.fills.push(fill.clone());
        fill
    }

    fn net_position(
        &mut self,
        symbol: &str,
        fill_side: Side,
        fill_size: f64,
        fill_price: f64,
        leverage: f64,
    ) -> Option<f64> {
        let same_side_idx = self
            .positions
            .iter()
            .position(|p| p.symbol == symbol && p.side == fill_side);
        let opposite_side = fill_side.opposite();
        let opp_idx = self
            .positions
            .iter()
            .position(|p| p.symbol == symbol && p.side == opposite_side);

        match opp_idx {
            Some(idx) => {
                let opp = &self.positions[idx];
                let opp_size = opp.size;
                let opp_entry = opp.entry_price;

                if fill_size <= opp_size {
                    let pnl = compute_realized_pnl(opposite_side, opp_entry, fill_price, fill_size);
                    self.collateral += pnl;

                    let remaining = opp_size - fill_size;
                    if remaining < 1e-12 {
                        self.positions.remove(idx);
                    } else {
                        self.positions[idx].size = remaining;
                        self.positions[idx].updated_at = Utc::now().to_rfc3339();
                    }

                    self.record_history(
                        "realized_pnl",
                        Some(symbol),
                        pnl,
                        &format!("Closed {fill_size:.8} {opposite_side} at {fill_price:.2}"),
                    );

                    Some(pnl)
                } else {
                    let close_pnl =
                        compute_realized_pnl(opposite_side, opp_entry, fill_price, opp_size);
                    self.collateral += close_pnl;
                    self.positions.remove(idx);

                    self.record_history("realized_pnl", Some(symbol), close_pnl, &format!(
                        "Closed {opp_size:.8} {opposite_side} at {fill_price:.2}, flipping to {fill_side}"
                    ));

                    let remainder = fill_size - opp_size;
                    self.open_new_position(symbol, fill_side, remainder, fill_price, leverage);

                    Some(close_pnl)
                }
            }
            None => match same_side_idx {
                Some(idx) => {
                    let existing = &self.positions[idx];
                    let total_notional =
                        existing.size * existing.entry_price + fill_size * fill_price;
                    let total_size = existing.size + fill_size;
                    let new_entry = total_notional / total_size;

                    self.positions[idx].size = total_size;
                    self.positions[idx].entry_price = new_entry;
                    self.positions[idx].updated_at = Utc::now().to_rfc3339();
                    None
                }
                None => {
                    self.open_new_position(symbol, fill_side, fill_size, fill_price, leverage);
                    None
                }
            },
        }
    }

    fn open_new_position(
        &mut self,
        symbol: &str,
        side: Side,
        size: f64,
        price: f64,
        leverage: f64,
    ) {
        let now = Utc::now().to_rfc3339();
        self.positions.push(FuturesPaperPosition {
            symbol: symbol.to_string(),
            side,
            size,
            entry_price: price,
            leverage,
            unrealized_funding: 0.0,
            last_funding_time: Some(now.clone()),
            created_at: now.clone(),
            updated_at: now,
        });
    }

    // -----------------------------------------------------------------------
    // Order management
    // -----------------------------------------------------------------------

    pub(crate) fn edit_order(
        &mut self,
        order_id: &str,
        new_size: Option<f64>,
        new_price: Option<f64>,
        new_stop_price: Option<f64>,
    ) -> Result<()> {
        let idx = self.find_open_order_idx(order_id)?;
        if let Some(s) = new_size {
            validate_finite_positive(s, "size")?;
        }
        if let Some(p) = new_price {
            validate_finite_positive(p, "price")?;
        }
        if let Some(sp) = new_stop_price {
            validate_finite_positive(sp, "stop-price")?;
        }

        let order = &self.open_orders[idx];
        let old_size = order.size;
        let old_reserved = order.reserved_margin;

        let eff_size = new_size.unwrap_or(old_size);
        let eff_stop = new_stop_price.or(order.stop_price);
        let eff_price = new_price.or(order.price).or(eff_stop).unwrap_or(0.0);
        let new_reserved = if order.reduce_only {
            0.0
        } else {
            let params = OrderParams {
                symbol: order.symbol.clone(),
                side: order.side,
                size: eff_size,
                order_type: order.order_type,
                price: Some(eff_price),
                stop_price: eff_stop,
                trigger_signal: order.trigger_signal,
                client_order_id: order.client_order_id.clone(),
                reduce_only: false,
                leverage: Some(order.leverage),
                trailing_stop_max_deviation: order.trailing_stop_max_deviation,
                trailing_stop_deviation_unit: order.trailing_stop_deviation_unit.clone(),
            };
            self.compute_margin_delta_for_size(&params, eff_price, order.leverage, eff_size)
        };
        let margin_increase = new_reserved - old_reserved;

        if margin_increase > 0.0 {
            let available = self.available_margin(&HashMap::new());
            if available < margin_increase {
                return Err(KrakenError::Validation(format!(
                    "Edit would require {margin_increase:.2} additional margin, but only {available:.2} available"
                )));
            }
        }

        if let Some(s) = new_size {
            self.open_orders[idx].size = s;
        }
        if let Some(p) = new_price {
            self.open_orders[idx].price = Some(p);
        }
        if let Some(sp) = new_stop_price {
            self.open_orders[idx].stop_price = Some(sp);
        }
        self.open_orders[idx].reserved_margin = new_reserved;
        self.open_orders[idx].updated_at = Utc::now().to_rfc3339();
        self.updated_at = Utc::now().to_rfc3339();
        Ok(())
    }

    pub(crate) fn cancel_order(
        &mut self,
        order_id: Option<&str>,
        client_order_id: Option<&str>,
    ) -> Result<FuturesPaperOrder> {
        let idx = match (order_id, client_order_id) {
            (Some(_), Some(_)) => {
                return Err(KrakenError::Validation(
                    "Provide exactly one of --order-id or --cli-ord-id".into(),
                ))
            }
            (Some(oid), None) => self.find_open_order_idx(oid)?,
            (None, Some(cid)) => self
                .open_orders
                .iter()
                .position(|o| o.client_order_id.as_deref() == Some(cid))
                .ok_or_else(|| {
                    KrakenError::Validation(format!("No open order with client_order_id '{cid}'"))
                })?,
            (None, None) => {
                return Err(KrakenError::Validation(
                    "Provide --order-id or --cli-ord-id".into(),
                ))
            }
        };

        let order = self.open_orders.remove(idx);
        self.record_history(
            "order_cancelled",
            Some(&order.symbol),
            0.0,
            &format!("User cancelled order {}", order.id),
        );
        self.updated_at = Utc::now().to_rfc3339();
        Ok(order)
    }

    pub(crate) fn cancel_all(&mut self, symbol_filter: Option<&str>) -> Vec<FuturesPaperOrder> {
        let (cancelled, remaining): (Vec<_>, Vec<_>) = self.open_orders.drain(..).partition(|o| {
            symbol_filter
                .map(|s| o.symbol.eq_ignore_ascii_case(s))
                .unwrap_or(true)
        });
        self.open_orders = remaining;
        for order in &cancelled {
            self.record_history(
                "order_cancelled",
                Some(&order.symbol),
                0.0,
                &format!("User cancelled order {} (cancel-all)", order.id),
            );
        }
        self.updated_at = Utc::now().to_rfc3339();
        cancelled
    }

    pub(crate) fn batch_orders(
        &mut self,
        batch: Vec<OrderParams>,
        market_snapshots: &HashMap<String, MarketSnapshot>,
    ) -> Vec<BatchOrderResult> {
        let mut results = Vec::with_capacity(batch.len());
        for params in batch {
            let snapshot = market_snapshots.get(&params.symbol);
            let result = match snapshot {
                Some(snap) => match self.place_order(params.clone(), snap) {
                    Ok(r) => BatchOrderResult {
                        symbol: params.symbol,
                        success: true,
                        order_id: Some(r.order_id),
                        error: None,
                    },
                    Err(e) => BatchOrderResult {
                        symbol: params.symbol,
                        success: false,
                        order_id: None,
                        error: Some(e.to_string()),
                    },
                },
                None => BatchOrderResult {
                    symbol: params.symbol.clone(),
                    success: false,
                    order_id: None,
                    error: Some(format!("No market data for {}", params.symbol)),
                },
            };
            results.push(result);
        }
        results
    }

    fn find_open_order_idx(&self, order_id: &str) -> Result<usize> {
        self.open_orders
            .iter()
            .position(|o| o.id == order_id)
            .ok_or_else(|| {
                KrakenError::Validation(format!("Order '{order_id}' not found in open orders"))
            })
    }

    // -----------------------------------------------------------------------
    // Reconciliation: triggers, liquidation, funding
    // -----------------------------------------------------------------------

    pub(crate) fn reconcile(
        &mut self,
        mark_prices: &HashMap<String, f64>,
        last_prices: &HashMap<String, f64>,
        index_prices: &HashMap<String, f64>,
        funding_rates: &HashMap<String, f64>,
        maintenance_rates: &HashMap<String, f64>,
    ) -> ReconcileResult {
        let mut fills = Vec::new();
        let mut liquidations = Vec::new();
        let mut funding_events = Vec::new();

        self.reconcile_limit_orders(last_prices, mark_prices, &mut fills);
        self.reconcile_triggered_orders(mark_prices, last_prices, index_prices, &mut fills);
        self.reconcile_liquidations(mark_prices, maintenance_rates, &mut liquidations);
        self.reconcile_funding(funding_rates, mark_prices, &mut funding_events);

        self.last_reconciled_at = Some(Utc::now().to_rfc3339());
        self.updated_at = Utc::now().to_rfc3339();

        ReconcileResult {
            fills,
            liquidations,
            funding_events,
        }
    }

    fn reconcile_limit_orders(
        &mut self,
        last_prices: &HashMap<String, f64>,
        mark_prices: &HashMap<String, f64>,
        fills: &mut Vec<FuturesPaperFill>,
    ) {
        let mut matched_indices = Vec::new();

        for (i, order) in self.open_orders.iter().enumerate() {
            if order.order_type != FuturesOrderType::Limit
                && order.order_type != FuturesOrderType::Post
            {
                continue;
            }
            let limit_price = match order.price {
                Some(p) => p,
                None => continue,
            };
            let last = match last_prices
                .get(&order.symbol)
                .or_else(|| mark_prices.get(&order.symbol))
            {
                Some(&p) if p.is_finite() && p > 0.0 => p,
                _ => continue,
            };

            let is_marketable = match order.side {
                Side::Long => limit_price >= last,
                Side::Short => limit_price <= last,
            };
            if is_marketable {
                matched_indices.push(i);
            }
        }

        matched_indices.sort_unstable_by(|a, b| b.cmp(a));

        for idx in matched_indices {
            let order = self.open_orders.remove(idx);

            if order.order_type == FuturesOrderType::Post {
                self.record_history(
                    "order_cancelled",
                    Some(&order.symbol),
                    0.0,
                    &format!(
                        "Post-only order {} cancelled: would have taken liquidity",
                        order.id
                    ),
                );
                continue;
            }

            if order.reduce_only {
                let opposite_side = order.side.opposite();
                let has_position = self.positions.iter().any(|p| {
                    p.symbol == order.symbol && p.side == opposite_side && p.size >= order.size
                });
                if !has_position {
                    self.record_history(
                        "order_cancelled",
                        Some(&order.symbol),
                        0.0,
                        &format!(
                            "Reduce-only limit {} cancelled: no reducible position",
                            order.id
                        ),
                    );
                    continue;
                }
            }

            let fill_price = match order.price {
                Some(p) if p.is_finite() && p > 0.0 => p,
                _ => continue,
            };
            let fee = order.size * fill_price * self.fee_rate;

            let fill_id = self.next_id();
            let fill = self.apply_fill(
                &fill_id,
                &order.id,
                &order.symbol,
                order.side,
                order.size,
                fill_price,
                fee,
                order.client_order_id.clone(),
                "limit_fill",
                order.leverage,
            );
            self.collateral -= fee;
            fills.push(fill);
        }
    }

    fn reconcile_triggered_orders(
        &mut self,
        mark_prices: &HashMap<String, f64>,
        last_prices: &HashMap<String, f64>,
        index_prices: &HashMap<String, f64>,
        fills: &mut Vec<FuturesPaperFill>,
    ) {
        for order in &mut self.open_orders {
            if order.order_type != FuturesOrderType::TrailingStop {
                continue;
            }
            let signal_price = resolve_trigger_price(
                order.trigger_signal.unwrap_or(TriggerSignal::Last),
                &order.symbol,
                mark_prices,
                last_prices,
                index_prices,
            );
            let Some(current) = signal_price else {
                continue;
            };
            if let Some(ref mut anchor) = order.trailing_anchor {
                match order.side {
                    Side::Long => *anchor = anchor.min(current),
                    Side::Short => *anchor = anchor.max(current),
                }
            }
        }

        let mut triggered_indices = Vec::new();

        for (i, order) in self.open_orders.iter().enumerate() {
            let eval_price = resolve_trigger_price(
                order.trigger_signal.unwrap_or(TriggerSignal::Last),
                &order.symbol,
                mark_prices,
                last_prices,
                index_prices,
            );
            let Some(price) = eval_price else {
                continue;
            };

            let should_trigger = match order.order_type {
                FuturesOrderType::Stop => match order.side {
                    Side::Long => order.stop_price.map(|sp| price >= sp).unwrap_or(false),
                    Side::Short => order.stop_price.map(|sp| price <= sp).unwrap_or(false),
                },
                FuturesOrderType::TakeProfit => match order.side {
                    Side::Long => order.stop_price.map(|sp| price <= sp).unwrap_or(false),
                    Side::Short => order.stop_price.map(|sp| price >= sp).unwrap_or(false),
                },
                FuturesOrderType::TrailingStop => check_trailing_stop_trigger(order, price),
                _ => false,
            };

            if should_trigger {
                triggered_indices.push(i);
            }
        }

        triggered_indices.sort_unstable_by(|a, b| b.cmp(a));

        for idx in triggered_indices {
            let order = self.open_orders.remove(idx);

            let signal_price = resolve_trigger_price(
                order.trigger_signal.unwrap_or(TriggerSignal::Last),
                &order.symbol,
                mark_prices,
                last_prices,
                index_prices,
            );
            let fill_price = order
                .price
                .filter(|p| p.is_finite() && *p > 0.0)
                .or(signal_price)
                .or_else(|| {
                    mark_prices
                        .get(&order.symbol)
                        .copied()
                        .filter(|p| p.is_finite() && *p > 0.0)
                });
            let Some(fill_price) = fill_price else {
                self.open_orders.push(order);
                continue;
            };

            if order.reduce_only {
                let opposite_side = order.side.opposite();
                let has_position = self.positions.iter().any(|p| {
                    p.symbol == order.symbol && p.side == opposite_side && p.size >= order.size
                });
                if !has_position {
                    self.record_history(
                        "order_cancelled",
                        Some(&order.symbol),
                        0.0,
                        &format!(
                            "Reduce-only order {} cancelled at trigger: no reducible position",
                            order.id
                        ),
                    );
                    continue;
                }
            }

            let fee = order.size * fill_price * self.fee_rate;

            let fill_id = self.next_id();
            let fill = self.apply_fill(
                &fill_id,
                &order.id,
                &order.symbol,
                order.side,
                order.size,
                fill_price,
                fee,
                order.client_order_id.clone(),
                "triggered",
                order.leverage,
            );
            self.collateral -= fee;
            fills.push(fill);
        }
    }

    fn reconcile_liquidations(
        &mut self,
        mark_prices: &HashMap<String, f64>,
        maintenance_rates: &HashMap<String, f64>,
        liquidations: &mut Vec<FuturesPaperFill>,
    ) {
        let mut liq_indices = Vec::new();

        for (i, pos) in self.positions.iter().enumerate() {
            let mark = match mark_prices.get(&pos.symbol) {
                Some(&p) if p.is_finite() && p > 0.0 => p,
                _ => continue,
            };

            let maint_rate = match maintenance_rates.get(&pos.symbol) {
                Some(&r) if r.is_finite() && r > 0.0 => r,
                _ => {
                    self.maintenance_margin_fallback_used = true;
                    DEFAULT_MAINTENANCE_MARGIN_RATE
                }
            };

            let liq_price = compute_liquidation_price(pos, maint_rate);
            let is_liquidated = match pos.side {
                Side::Long => mark <= liq_price,
                Side::Short => mark >= liq_price,
            };

            if is_liquidated {
                liq_indices.push((i, mark));
            }
        }

        liq_indices.sort_unstable_by(|a, b| b.0.cmp(&a.0));

        for (idx, mark) in liq_indices {
            let pos = self.positions.remove(idx);
            let pnl = compute_realized_pnl(pos.side, pos.entry_price, mark, pos.size);
            self.collateral += pnl;

            let fill_id = self.next_id();
            let order_id = format!("LIQ-{}", &fill_id);

            let fill = FuturesPaperFill {
                id: fill_id,
                order_id: order_id.clone(),
                symbol: pos.symbol.clone(),
                side: pos.side.opposite(),
                size: pos.size,
                price: mark,
                fee: 0.0,
                realized_pnl: Some(pnl),
                fill_type: "liquidation".to_string(),
                client_order_id: None,
                filled_at: Utc::now().to_rfc3339(),
            };
            self.fills.push(fill.clone());
            liquidations.push(fill);

            self.record_history(
                "liquidation",
                Some(&pos.symbol),
                pnl,
                &format!(
                    "Liquidated {:.8} {} {} at {mark:.2}",
                    pos.size, pos.side, pos.symbol
                ),
            );
        }
    }

    fn reconcile_funding(
        &mut self,
        funding_rates: &HashMap<String, f64>,
        mark_prices: &HashMap<String, f64>,
        events: &mut Vec<FuturesPaperHistoryEvent>,
    ) {
        let now = Utc::now();
        let id_base = self.next_id;
        let mut id_offset = 0u64;

        for pos in &mut self.positions {
            let rate = match funding_rates.get(&pos.symbol) {
                Some(&r) if r.is_finite() => r,
                _ => continue,
            };

            let last_funding = pos
                .last_funding_time
                .as_ref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));

            let intervals_elapsed = match last_funding {
                Some(last) => {
                    let hours = (now - last).num_hours();
                    hours / FUNDING_INTERVAL_HOURS
                }
                None => 0,
            };

            if intervals_elapsed <= 0 {
                continue;
            }

            let mark = match mark_prices.get(&pos.symbol) {
                Some(&m) if m.is_finite() && m > 0.0 => m,
                _ => pos.entry_price,
            };
            let notional = pos.size * mark;
            let funding_per_interval = notional * rate;
            let total_funding = funding_per_interval * intervals_elapsed as f64;

            let amount = match pos.side {
                Side::Long => -total_funding,
                Side::Short => total_funding,
            };

            pos.unrealized_funding += amount;
            pos.last_funding_time = Some(now.to_rfc3339());
            pos.updated_at = now.to_rfc3339();

            let event_id = format!("FP-{:05}", id_base + id_offset);
            id_offset += 1;
            let event = FuturesPaperHistoryEvent {
                id: event_id,
                event_type: "funding".to_string(),
                symbol: Some(pos.symbol.clone()),
                amount,
                details: format!(
                    "{intervals_elapsed} interval(s) at rate {rate:.6}, notional {notional:.2}"
                ),
                timestamp: now.to_rfc3339(),
            };

            self.collateral += amount;
            events.push(event.clone());
            self.history.push(event);
        }
        self.next_id = id_base + id_offset;
    }

    fn record_history(
        &mut self,
        event_type: &str,
        symbol: Option<&str>,
        amount: f64,
        details: &str,
    ) {
        let id = self.next_id();
        self.history.push(FuturesPaperHistoryEvent {
            id,
            event_type: event_type.to_string(),
            symbol: symbol.map(|s| s.to_string()),
            amount,
            details: details.to_string(),
            timestamp: Utc::now().to_rfc3339(),
        });
    }
}

// ---------------------------------------------------------------------------
// Pure computation helpers
// ---------------------------------------------------------------------------

pub(crate) fn compute_unrealized_pnl(pos: &FuturesPaperPosition, mark_price: f64) -> f64 {
    match pos.side {
        Side::Long => (mark_price - pos.entry_price) * pos.size,
        Side::Short => (pos.entry_price - mark_price) * pos.size,
    }
}

pub(crate) fn compute_liquidation_price(
    pos: &FuturesPaperPosition,
    maintenance_margin_rate: f64,
) -> f64 {
    match pos.side {
        Side::Long => pos.entry_price * (1.0 - (1.0 / pos.leverage) + maintenance_margin_rate),
        Side::Short => pos.entry_price * (1.0 + (1.0 / pos.leverage) - maintenance_margin_rate),
    }
}

fn compute_realized_pnl(side: Side, entry: f64, exit: f64, size: f64) -> f64 {
    match side {
        Side::Long => (exit - entry) * size,
        Side::Short => (entry - exit) * size,
    }
}

fn compute_executable_depth(
    levels: &[(f64, f64)],
    side: Side,
    limit_price: f64,
    max_size: f64,
) -> f64 {
    if levels.is_empty() {
        return 0.0;
    }
    let mut filled = 0.0;
    for &(price, available) in levels {
        if !price.is_finite() || price <= 0.0 || !available.is_finite() || available < 0.0 {
            continue;
        }
        let acceptable = match side {
            Side::Long => price <= limit_price,
            Side::Short => price >= limit_price,
        };
        if !acceptable {
            break;
        }
        filled += available;
        if filled >= max_size {
            return max_size;
        }
    }
    filled.min(max_size)
}

fn resolve_trigger_price(
    signal: TriggerSignal,
    symbol: &str,
    mark_prices: &HashMap<String, f64>,
    last_prices: &HashMap<String, f64>,
    index_prices: &HashMap<String, f64>,
) -> Option<f64> {
    let raw = match signal {
        TriggerSignal::Mark => mark_prices.get(symbol).copied(),
        TriggerSignal::Last => last_prices
            .get(symbol)
            .or_else(|| mark_prices.get(symbol))
            .copied(),
        TriggerSignal::Index => index_prices
            .get(symbol)
            .or_else(|| mark_prices.get(symbol))
            .copied(),
    };
    raw.filter(|&p| p.is_finite() && p > 0.0)
}

fn check_trailing_stop_trigger(order: &FuturesPaperOrder, mark: f64) -> bool {
    let Some(anchor) = order.trailing_anchor else {
        return false;
    };
    let Some(max_dev) = order.trailing_stop_max_deviation else {
        return false;
    };

    let deviation = match order.trailing_stop_deviation_unit.as_deref() {
        Some("quote_currency") => (mark - anchor).abs(),
        _ => (mark - anchor).abs() / anchor * 100.0,
    };

    match order.side {
        Side::Long => mark >= anchor && deviation >= max_dev,
        Side::Short => mark <= anchor && deviation >= max_dev,
    }
}

fn validate_finite_positive(val: f64, name: &str) -> Result<()> {
    if !val.is_finite() || val <= 0.0 {
        return Err(KrakenError::Validation(format!(
            "{name} must be a finite positive number"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Parameters and results
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct OrderParams {
    pub(crate) symbol: String,
    pub(crate) side: Side,
    pub(crate) size: f64,
    pub(crate) order_type: FuturesOrderType,
    pub(crate) price: Option<f64>,
    pub(crate) stop_price: Option<f64>,
    pub(crate) trigger_signal: Option<TriggerSignal>,
    pub(crate) client_order_id: Option<String>,
    pub(crate) reduce_only: bool,
    pub(crate) leverage: Option<f64>,
    pub(crate) trailing_stop_max_deviation: Option<f64>,
    pub(crate) trailing_stop_deviation_unit: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct MarketSnapshot {
    pub(crate) bid: f64,
    pub(crate) ask: f64,
    pub(crate) last: f64,
    pub(crate) mark: f64,
    pub(crate) index: f64,
    pub(crate) ask_levels: Vec<(f64, f64)>,
    pub(crate) bid_levels: Vec<(f64, f64)>,
}

#[derive(Debug)]
pub(crate) struct OrderPlacementResult {
    pub(crate) order_id: String,
    pub(crate) status: OrderStatus,
    pub(crate) fills: Vec<FuturesPaperFill>,
    pub(crate) message: Option<String>,
}

#[derive(Debug)]
pub(crate) struct BatchOrderResult {
    pub(crate) symbol: String,
    pub(crate) success: bool,
    pub(crate) order_id: Option<String>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Default)]
#[allow(dead_code)]
pub(crate) struct ReconcileResult {
    pub(crate) fills: Vec<FuturesPaperFill>,
    pub(crate) liquidations: Vec<FuturesPaperFill>,
    pub(crate) funding_events: Vec<FuturesPaperHistoryEvent>,
}

// ---------------------------------------------------------------------------
// Persistence (isolated from spot paper)
// ---------------------------------------------------------------------------

pub(crate) fn futures_paper_state_path() -> Result<PathBuf> {
    Ok(config::config_dir()?
        .join("paper")
        .join("futures_state.json"))
}

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

fn lock_path() -> Result<PathBuf> {
    Ok(futures_paper_state_path()?.with_extension("json.lock"))
}

fn lock_token() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}:{timestamp}", std::process::id())
}

/// Advisory lock guard. Acquiring creates a `.lock` file atomically; dropping
/// removes it only if the guard still owns the lock file.
pub(crate) struct StateLock {
    path: PathBuf,
    token: String,
}

impl StateLock {
    pub(crate) fn acquire() -> Result<Self> {
        let path = lock_path()?;
        let token = lock_token();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let start = Instant::now();
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    if let Err(e) = f.write_all(token.as_bytes()).and_then(|()| f.sync_all()) {
                        let _ = fs::remove_file(&path);
                        return Err(KrakenError::Io(e));
                    }
                    return Ok(StateLock { path, token });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if start.elapsed() > LOCK_TIMEOUT {
                        return Err(KrakenError::Validation(
                            format!(
                                "Futures paper state is locked by another process. Try again shortly. If a previous command crashed, remove '{}'.",
                                path.display()
                            ),
                        ));
                    }
                    std::thread::sleep(LOCK_POLL_INTERVAL);
                }
                Err(e) => return Err(KrakenError::Io(e)),
            }
        }
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let still_owned = fs::read_to_string(&self.path)
            .ok()
            .as_deref()
            .is_some_and(|contents| contents == self.token);
        if still_owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn load_state() -> Result<FuturesPaperState> {
    let path = futures_paper_state_path()?;
    if !path.exists() {
        return Err(KrakenError::Validation(
            "Futures paper account not initialized. Run 'kraken futures paper init' first.".into(),
        ));
    }
    let data = fs::read_to_string(&path)?;
    let state: FuturesPaperState = serde_json::from_str(&data)?;
    Ok(state)
}

pub(crate) fn save_state(state: &FuturesPaperState) -> Result<()> {
    let path = futures_paper_state_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(state)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> FuturesPaperState {
        FuturesPaperState::new(10000.0, "USD", DEFAULT_FUTURES_TAKER_FEE_RATE)
    }

    fn market_snapshot(bid: f64, ask: f64) -> MarketSnapshot {
        MarketSnapshot {
            bid,
            ask,
            last: (bid + ask) / 2.0,
            mark: (bid + ask) / 2.0,
            index: (bid + ask) / 2.0,
            ask_levels: vec![(ask, 100.0)],
            bid_levels: vec![(bid, 100.0)],
        }
    }

    #[test]
    fn init_state_has_correct_defaults() {
        let s = test_state();
        assert_eq!(s.collateral, 10000.0);
        assert_eq!(s.currency, "USD");
        assert_eq!(s.fee_rate, DEFAULT_FUTURES_TAKER_FEE_RATE);
        assert!(s.open_orders.is_empty());
        assert!(s.positions.is_empty());
        assert!(s.fills.is_empty());
    }

    #[test]
    fn reset_clears_state() {
        let mut s = test_state();
        s.positions.push(FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            entry_price: 50000.0,
            leverage: 10.0,
            unrealized_funding: 0.0,
            last_funding_time: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        });
        s.reset(None, None, None);
        assert!(s.positions.is_empty());
        assert_eq!(s.collateral, 10000.0);
    }

    #[test]
    fn leverage_resolution_order_level_wins() {
        let s = test_state();
        let lev = s.resolve_leverage(Some(5.0), "PF_XBTUSD").unwrap();
        assert!((lev - 5.0).abs() < 1e-10);
    }

    #[test]
    fn leverage_resolution_preference_fallback() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let lev = s.resolve_leverage(None, "PF_XBTUSD").unwrap();
        assert!((lev - 10.0).abs() < 1e-10);
    }

    #[test]
    fn leverage_resolution_fails_when_unset() {
        let s = test_state();
        let result = s.resolve_leverage(None, "PF_XBTUSD");
        assert!(result.is_err());
    }

    #[test]
    fn market_buy_opens_long() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        assert_eq!(result.status, OrderStatus::Filled);
        assert_eq!(result.fills.len(), 1);
        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.positions[0].side, Side::Long);
        assert!((s.positions[0].size - 1.0).abs() < 1e-10);
    }

    #[test]
    fn market_sell_opens_short() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Short,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        assert_eq!(result.status, OrderStatus::Filled);
        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.positions[0].side, Side::Short);
    }

    #[test]
    fn same_side_aggregates_weighted_entry() {
        let mut s = FuturesPaperState::new(100000.0, "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);

        let snap1 = market_snapshot(49900.0, 50000.0);
        let params1 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params1, &snap1).unwrap();

        let snap2 = market_snapshot(51900.0, 52000.0);
        let params2 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params2, &snap2).unwrap();

        assert_eq!(s.positions.len(), 1);
        assert!((s.positions[0].size - 2.0).abs() < 1e-10);
        let expected_entry = (50000.0 + 52000.0) / 2.0;
        assert!((s.positions[0].entry_price - expected_entry).abs() < 1e-6);
    }

    #[test]
    fn opposite_side_reduces_and_closes() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);

        let snap1 = market_snapshot(49900.0, 50000.0);
        let params1 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Short,
            size: 2.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params1, &snap1).unwrap();
        assert_eq!(s.positions[0].side, Side::Short);
        assert!((s.positions[0].size - 2.0).abs() < 1e-10);

        let snap2 = market_snapshot(48900.0, 49000.0);
        let params2 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params2, &snap2).unwrap();

        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.positions[0].side, Side::Short);
        assert!((s.positions[0].size - 1.0).abs() < 1e-10);
        assert!(result.fills[0].realized_pnl.is_some());
        let pnl = result.fills[0].realized_pnl.unwrap();
        assert!(pnl > 0.0, "short closed at profit should be positive");
    }

    #[test]
    fn opposite_side_overfill_flips_direction() {
        let mut s = FuturesPaperState::new(100000.0, "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);

        let snap1 = market_snapshot(49900.0, 50000.0);
        let p1 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Short,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(p1, &snap1).unwrap();

        let snap2 = market_snapshot(47900.0, 48000.0);
        let p2 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 3.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(p2, &snap2).unwrap();

        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.positions[0].side, Side::Long);
        assert!((s.positions[0].size - 2.0).abs() < 1e-10);
        assert!(result.fills[0].realized_pnl.is_some());
    }

    #[test]
    fn post_only_rejects_crossing() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Post,
            price: Some(50100.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Post-only"));
    }

    #[test]
    fn fok_rejects_insufficient_depth() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = MarketSnapshot {
            bid: 49900.0,
            ask: 50000.0,
            last: 49950.0,
            mark: 49950.0,
            index: 49950.0,
            ask_levels: vec![(50000.0, 0.5)],
            bid_levels: vec![(49900.0, 100.0)],
        };
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Fok,
            price: Some(50000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        assert_eq!(result.status, OrderStatus::Rejected);
    }

    #[test]
    fn ioc_partially_fills() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = MarketSnapshot {
            bid: 49900.0,
            ask: 50000.0,
            last: 49950.0,
            mark: 49950.0,
            index: 49950.0,
            ask_levels: vec![(50000.0, 0.5)],
            bid_levels: vec![(49900.0, 100.0)],
        };
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Ioc,
            price: Some(50000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        assert_eq!(result.status, OrderStatus::PartiallyFilled);
        assert_eq!(result.fills.len(), 1);
        assert!((result.fills[0].size - 0.5).abs() < 1e-10);
    }

    #[test]
    fn margin_rejects_over_leveraged() {
        let mut s = FuturesPaperState::new(100.0, "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences.insert("PF_XBTUSD".into(), 2.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(2.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Insufficient margin"));
    }

    #[test]
    fn reduce_only_rejects_no_position() {
        let mut s = test_state();
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: true,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Reduce-only"));
    }

    #[test]
    fn client_order_id_propagates() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: Some("my-signal-123".into()),
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        assert_eq!(
            result.fills[0].client_order_id.as_deref(),
            Some("my-signal-123")
        );
    }

    #[test]
    fn cancel_by_order_id() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Limit,
            price: Some(48000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: Some("test-cancel".into()),
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        assert_eq!(s.open_orders.len(), 1);

        let cancelled = s.cancel_order(Some(&result.order_id), None).unwrap();
        assert_eq!(cancelled.id, result.order_id);
        assert!(s.open_orders.is_empty());
    }

    #[test]
    fn cancel_by_client_order_id() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Limit,
            price: Some(48000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: Some("test-cancel-cid".into()),
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params, &snap).unwrap();

        let cancelled = s.cancel_order(None, Some("test-cancel-cid")).unwrap();
        assert_eq!(
            cancelled.client_order_id.as_deref(),
            Some("test-cancel-cid")
        );
        assert!(s.open_orders.is_empty());
    }

    #[test]
    fn cancel_rejects_both_ids() {
        let mut s = test_state();
        let err = s.cancel_order(Some("ORD-1"), Some("CID-1"));
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("exactly one"),
            "Expected 'exactly one' error, got: {msg}"
        );
    }

    #[test]
    fn cancel_records_history_event() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Limit,
            price: Some(48000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        let history_before = s.history.len();

        s.cancel_order(Some(&result.order_id), None).unwrap();
        assert_eq!(s.history.len(), history_before + 1);
        let event = s.history.last().unwrap();
        assert_eq!(event.event_type, "order_cancelled");
        assert!(event.details.contains("User cancelled"));
    }

    #[test]
    fn cancel_all_records_history_events() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        for _ in 0..3 {
            let params = OrderParams {
                symbol: "PF_XBTUSD".into(),
                side: Side::Long,
                size: 0.1,
                order_type: FuturesOrderType::Limit,
                price: Some(48000.0),
                stop_price: None,
                trigger_signal: None,
                client_order_id: None,
                reduce_only: false,
                leverage: Some(10.0),
                trailing_stop_max_deviation: None,
                trailing_stop_deviation_unit: None,
            };
            s.place_order(params, &snap).unwrap();
        }
        assert_eq!(s.open_orders.len(), 3);
        let history_before = s.history.len();

        s.cancel_all(None);
        assert!(s.open_orders.is_empty());
        assert_eq!(s.history.len(), history_before + 3);
        assert!(s
            .history
            .iter()
            .rev()
            .take(3)
            .all(|h| h.event_type == "order_cancelled" && h.details.contains("cancel-all")));
    }

    #[test]
    fn cancel_all_with_symbol_filter() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        s.leverage_preferences.insert("PF_ETHUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);

        let p1 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Limit,
            price: Some(48000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(p1, &snap).unwrap();

        let p2 = OrderParams {
            symbol: "PF_ETHUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Limit,
            price: Some(3000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(p2, &snap).unwrap();

        let cancelled = s.cancel_all(Some("PF_XBTUSD"));
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].symbol, "PF_XBTUSD");
        assert_eq!(s.open_orders.len(), 1);
        assert_eq!(s.open_orders[0].symbol, "PF_ETHUSD");
    }

    #[test]
    fn edit_order_updates_fields() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Limit,
            price: Some(48000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();

        s.edit_order(&result.order_id, Some(2.0), Some(47000.0), None)
            .unwrap();
        assert!((s.open_orders[0].size - 2.0).abs() < 1e-10);
        assert!((s.open_orders[0].price.unwrap() - 47000.0).abs() < 1e-10);
    }

    #[test]
    fn edit_order_updates_stop_price() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let mark: HashMap<String, f64> = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Stop,
            price: None,
            stop_price: Some(48000.0),
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_triggered_order(&params, 10.0, &mark).unwrap();
        assert_eq!(s.open_orders[0].stop_price, Some(48000.0));

        s.edit_order(&s.open_orders[0].id.clone(), None, None, Some(47000.0))
            .unwrap();
        assert_eq!(s.open_orders[0].stop_price, Some(47000.0));
    }

    #[test]
    fn unrealized_pnl_sign_correct() {
        let long_pos = FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            entry_price: 50000.0,
            leverage: 10.0,
            unrealized_funding: 0.0,
            last_funding_time: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };

        assert!(compute_unrealized_pnl(&long_pos, 51000.0) > 0.0);
        assert!(compute_unrealized_pnl(&long_pos, 49000.0) < 0.0);

        let short_pos = FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: Side::Short,
            size: 1.0,
            entry_price: 50000.0,
            leverage: 10.0,
            unrealized_funding: 0.0,
            last_funding_time: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };

        assert!(compute_unrealized_pnl(&short_pos, 49000.0) > 0.0);
        assert!(compute_unrealized_pnl(&short_pos, 51000.0) < 0.0);
    }

    #[test]
    fn liquidation_price_computation() {
        let pos = FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            entry_price: 50000.0,
            leverage: 10.0,
            unrealized_funding: 0.0,
            last_funding_time: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };
        let liq = compute_liquidation_price(&pos, 0.02);
        let expected = 50000.0 * (1.0 - 0.1 + 0.02);
        assert!((liq - expected).abs() < 1e-6);
    }

    #[test]
    fn reconciliation_liquidates_breached_position() {
        let mut s = test_state();
        s.positions.push(FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            entry_price: 50000.0,
            leverage: 10.0,
            unrealized_funding: 0.0,
            last_funding_time: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        });

        let liq_price = compute_liquidation_price(&s.positions[0], 0.02);
        let mut mark_prices = HashMap::new();
        mark_prices.insert("PF_XBTUSD".to_string(), liq_price - 100.0);

        let mut maint = HashMap::new();
        maint.insert("PF_XBTUSD".to_string(), 0.02);

        let result = s.reconcile(
            &mark_prices,
            &mark_prices,
            &HashMap::new(),
            &HashMap::new(),
            &maint,
        );
        assert_eq!(result.liquidations.len(), 1);
        assert!(s.positions.is_empty());
    }

    #[test]
    fn batch_order_sequential_margin() {
        let mut s = FuturesPaperState::new(10000.0, "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences.insert("PF_XBTUSD".into(), 2.0);

        let snap = market_snapshot(49900.0, 50000.0);
        let mut snapshots = HashMap::new();
        snapshots.insert("PF_XBTUSD".to_string(), snap);

        let batch = vec![
            OrderParams {
                symbol: "PF_XBTUSD".into(),
                side: Side::Long,
                size: 0.3,
                order_type: FuturesOrderType::Market,
                price: None,
                stop_price: None,
                trigger_signal: None,
                client_order_id: None,
                reduce_only: false,
                leverage: Some(2.0),
                trailing_stop_max_deviation: None,
                trailing_stop_deviation_unit: None,
            },
            OrderParams {
                symbol: "PF_XBTUSD".into(),
                side: Side::Long,
                size: 100.0,
                order_type: FuturesOrderType::Market,
                price: None,
                stop_price: None,
                trigger_signal: None,
                client_order_id: None,
                reduce_only: false,
                leverage: Some(2.0),
                trailing_stop_max_deviation: None,
                trailing_stop_deviation_unit: None,
            },
        ];

        let results = s.batch_orders(batch, &snapshots);
        assert!(results[0].success);
        assert!(!results[1].success);
        assert!(results[1].error.as_ref().unwrap().contains("margin"));
    }

    #[test]
    fn default_order_type_is_limit() {
        let ot = FuturesOrderType::from_str_cli("limit").unwrap();
        assert_eq!(ot, FuturesOrderType::Limit);
    }

    #[test]
    fn state_path_isolated_from_spot() {
        let futures_path = futures_paper_state_path().unwrap();
        let spot_path = crate::paper::paper_state_path().unwrap();
        assert_ne!(futures_path, spot_path);
        assert!(futures_path.to_string_lossy().contains("futures_state"));
        assert!(spot_path.to_string_lossy().contains("state.json"));
    }

    #[test]
    fn fee_deducted_on_market_order() {
        let mut s = test_state();
        let initial = s.collateral;
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        let fee = result.fills[0].fee;
        assert!(fee > 0.0);
        assert!((s.collateral - (initial - fee)).abs() < 1e-6);
    }

    #[test]
    fn serialization_roundtrip() {
        let s = test_state();
        let json = serde_json::to_string_pretty(&s).unwrap();
        let loaded: FuturesPaperState = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.collateral, s.collateral);
        assert_eq!(loaded.currency, s.currency);
        assert!(!json.contains("api_key"));
        assert!(!json.contains("api_secret"));
    }

    #[test]
    fn empty_orderbook_returns_zero_depth() {
        let depth = compute_executable_depth(&[], Side::Long, 50000.0, 1.0);
        assert!((depth - 0.0).abs() < 1e-12);
    }

    #[test]
    fn depth_uses_correct_side_for_buy() {
        let ask_levels = vec![(50000.0, 0.5), (50100.0, 0.5)];
        let depth = compute_executable_depth(&ask_levels, Side::Long, 50100.0, 2.0);
        assert!((depth - 1.0).abs() < 1e-12);
    }

    #[test]
    fn depth_uses_correct_side_for_sell() {
        let bid_levels = vec![(49900.0, 0.5), (49800.0, 0.5)];
        let depth = compute_executable_depth(&bid_levels, Side::Short, 49800.0, 2.0);
        assert!((depth - 1.0).abs() < 1e-12);
    }

    #[test]
    fn leverage_threads_to_new_position() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 5.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params, &snap).unwrap();
        assert!((s.positions[0].leverage - 10.0).abs() < 1e-10);
    }

    #[test]
    fn flip_uses_order_leverage_not_old_position() {
        let mut s = FuturesPaperState::new(100000.0, "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);

        let snap1 = market_snapshot(49900.0, 50000.0);
        let p1 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Short,
            size: 1.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(p1, &snap1).unwrap();

        let snap2 = market_snapshot(47900.0, 48000.0);
        let p2 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 3.0,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(5.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(p2, &snap2).unwrap();

        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.positions[0].side, Side::Long);
        assert!((s.positions[0].leverage - 5.0).abs() < 1e-10);
    }

    #[test]
    fn edit_order_recalculates_margin() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Limit,
            price: Some(48000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        let old_margin = s.open_orders[0].reserved_margin;

        s.edit_order(&result.order_id, Some(2.0), None, None)
            .unwrap();
        let new_margin = s.open_orders[0].reserved_margin;
        assert!(new_margin > old_margin);
        assert!((new_margin - (2.0 * 48000.0 / 10.0)).abs() < 1e-6);
    }

    #[test]
    fn edit_order_rejects_if_margin_insufficient() {
        let mut s = FuturesPaperState::new(5000.0, "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49900.0, 50000.0);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 0.5,
            order_type: FuturesOrderType::Limit,
            price: Some(48000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();

        let edit_result = s.edit_order(&result.order_id, Some(100.0), None, None);
        assert!(edit_result.is_err());
        assert!((s.open_orders[0].size - 0.5).abs() < 1e-10);
    }

    #[test]
    fn trigger_signal_differentiation() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);

        s.open_orders.push(FuturesPaperOrder {
            id: "STOP-MARK".into(),
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            filled_size: 0.0,
            order_type: FuturesOrderType::Stop,
            price: None,
            stop_price: Some(51000.0),
            trigger_signal: Some(TriggerSignal::Mark),
            client_order_id: None,
            reduce_only: false,
            leverage: 10.0,
            reserved_margin: 5100.0,
            status: OrderStatus::Open,
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
            trailing_anchor: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        });

        let mut mark_prices = HashMap::new();
        mark_prices.insert("PF_XBTUSD".to_string(), 49000.0);
        let mut last_prices = HashMap::new();
        last_prices.insert("PF_XBTUSD".to_string(), 52000.0);

        let result = s.reconcile(
            &mark_prices,
            &last_prices,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            result.fills.is_empty(),
            "Mark=49000 < 51000, should not trigger"
        );
        assert_eq!(s.open_orders.len(), 1);

        mark_prices.insert("PF_XBTUSD".to_string(), 51500.0);
        let result = s.reconcile(
            &mark_prices,
            &last_prices,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(result.fills.len(), 1, "Mark=51500 >= 51000, should trigger");
        assert!(s.open_orders.is_empty());
    }

    #[test]
    fn trailing_stop_anchor_updates() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        s.positions.push(FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: Side::Short,
            size: 1.0,
            entry_price: 50000.0,
            leverage: 10.0,
            unrealized_funding: 0.0,
            last_funding_time: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        });

        s.open_orders.push(FuturesPaperOrder {
            id: "TS-1".into(),
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            filled_size: 0.0,
            order_type: FuturesOrderType::TrailingStop,
            price: None,
            stop_price: Some(49000.0),
            trigger_signal: Some(TriggerSignal::Last),
            client_order_id: None,
            reduce_only: false,
            leverage: 10.0,
            reserved_margin: 4900.0,
            status: OrderStatus::Open,
            trailing_stop_max_deviation: Some(500.0),
            trailing_stop_deviation_unit: Some("quote_currency".into()),
            trailing_anchor: Some(49000.0),
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        });

        let mut mark = HashMap::new();
        mark.insert("PF_XBTUSD".to_string(), 48000.0);
        let mut last = HashMap::new();
        last.insert("PF_XBTUSD".to_string(), 48000.0);

        s.reconcile(
            &mark,
            &last,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(s.open_orders.len(), 1);
        assert!(
            (s.open_orders[0].trailing_anchor.unwrap() - 48000.0).abs() < 1e-10,
            "Short trailing stop anchor should move down to 48000"
        );
    }

    #[test]
    fn reduce_only_cancelled_at_trigger_when_no_position() {
        let mut s = test_state();

        s.open_orders.push(FuturesPaperOrder {
            id: "STOP-RO".into(),
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            filled_size: 0.0,
            order_type: FuturesOrderType::Stop,
            price: None,
            stop_price: Some(51000.0),
            trigger_signal: Some(TriggerSignal::Last),
            client_order_id: None,
            reduce_only: true,
            leverage: 10.0,
            reserved_margin: 0.0,
            status: OrderStatus::Open,
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
            trailing_anchor: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        });

        let mut mark = HashMap::new();
        mark.insert("PF_XBTUSD".to_string(), 52000.0);
        let mut last = HashMap::new();
        last.insert("PF_XBTUSD".to_string(), 52000.0);

        let result = s.reconcile(
            &mark,
            &last,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(result.fills.is_empty());
        assert!(s.open_orders.is_empty());
        assert!(s.history.iter().any(|h| h.event_type == "order_cancelled"));
    }

    #[test]
    fn funding_accrual_applies_when_rate_provided() {
        let mut s = test_state();
        let past = (chrono::Utc::now() - chrono::Duration::hours(9)).to_rfc3339();
        s.positions.push(FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            entry_price: 50000.0,
            leverage: 10.0,
            unrealized_funding: 0.0,
            last_funding_time: Some(past),
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        });

        let mark = HashMap::new();
        let last = HashMap::new();
        let mut funding = HashMap::new();
        funding.insert("PF_XBTUSD".to_string(), 0.0001);

        let result = s.reconcile(&mark, &last, &HashMap::new(), &funding, &HashMap::new());
        assert_eq!(result.funding_events.len(), 1);
        assert!(result.funding_events[0].amount != 0.0);
    }

    #[test]
    fn reconcile_fills_marketable_limit_buy() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49000.0, 51000.0);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Limit,
            price: Some(50500.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params, &snap).unwrap();
        assert_eq!(s.open_orders.len(), 1);
        assert!(s.positions.is_empty());

        let mark: HashMap<String, f64> = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);
        let last = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);
        let result = s.reconcile(
            &mark,
            &last,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            result.fills.len(),
            1,
            "Limit buy at 50500 >= last 50000 should fill"
        );
        assert!(s.open_orders.is_empty());
        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.positions[0].entry_price, 50500.0);
    }

    #[test]
    fn reconcile_skips_non_marketable_limit() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49000.0, 51000.0);
        let mark: HashMap<String, f64> = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Limit,
            price: Some(45000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params, &snap).unwrap();

        let last = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);
        let result = s.reconcile(
            &mark,
            &last,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            result.fills.is_empty(),
            "Limit buy at 45000 < last 50000 should not fill"
        );
        assert_eq!(s.open_orders.len(), 1);
    }

    #[test]
    fn reconcile_fills_marketable_limit_sell() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49000.0, 51000.0);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Short,
            size: 1.0,
            order_type: FuturesOrderType::Limit,
            price: Some(49800.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params, &snap).unwrap();

        let mark: HashMap<String, f64> = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);
        let last = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);
        let result = s.reconcile(
            &mark,
            &last,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            result.fills.len(),
            1,
            "Limit sell at 49800 <= last 50000 should fill"
        );
        assert!(s.open_orders.is_empty());
        assert_eq!(s.positions.len(), 1);
    }

    #[test]
    fn reconcile_limit_skips_zero_last_price() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49000.0, 51000.0);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Limit,
            price: Some(55000.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params, &snap).unwrap();

        let empty_last: HashMap<String, f64> = HashMap::new();
        let zero_mark: HashMap<String, f64> = HashMap::from([("PF_XBTUSD".into(), 0.0)]);
        let result = s.reconcile(
            &zero_mark,
            &empty_last,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            result.fills.is_empty(),
            "Should not fill with no valid price data"
        );
        assert_eq!(s.open_orders.len(), 1);
    }

    #[test]
    fn post_only_cancelled_when_marketable() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let snap = market_snapshot(49000.0, 51000.0);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Post,
            price: Some(50500.0),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let collateral_before = s.collateral;
        s.place_order(params, &snap).unwrap();
        assert_eq!(s.open_orders.len(), 1);
        assert!(
            s.open_orders[0].reserved_margin > 0.0,
            "Margin should be reserved on the order"
        );

        let last = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);
        let mark: HashMap<String, f64> = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);
        let result = s.reconcile(
            &mark,
            &last,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            result.fills.is_empty(),
            "Post-only should not fill when marketable"
        );
        assert!(
            s.open_orders.is_empty(),
            "Post-only order should be removed"
        );
        assert_eq!(
            s.collateral, collateral_before,
            "Collateral must not be inflated by post-only cancellation"
        );
        let cancel_event = s.history.iter().find(|h| h.event_type == "order_cancelled");
        assert!(cancel_event.is_some(), "Should record cancellation event");
        let msg = &cancel_event.unwrap().details;
        assert!(
            msg.contains("Post-only"),
            "Cancellation note should mention Post-only, got: {msg}"
        );
    }

    #[test]
    fn index_trigger_uses_index_prices() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let mark = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::Stop,
            price: None,
            stop_price: Some(52000.0),
            trigger_signal: Some(TriggerSignal::Index),
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_triggered_order(&params, 10.0, &mark).unwrap();

        let last = HashMap::from([("PF_XBTUSD".into(), 53000.0)]);
        let index_low = HashMap::from([("PF_XBTUSD".into(), 51000.0)]);
        let result = s.reconcile(&mark, &last, &index_low, &HashMap::new(), &HashMap::new());
        assert!(
            result.fills.is_empty(),
            "Index=51000 < stop=52000, should not trigger"
        );
        assert_eq!(s.open_orders.len(), 1);

        let index_high = HashMap::from([("PF_XBTUSD".into(), 53000.0)]);
        let result = s.reconcile(&mark, &last, &index_high, &HashMap::new(), &HashMap::new());
        assert_eq!(
            result.fills.len(),
            1,
            "Index=53000 >= stop=52000, should trigger"
        );
        assert!(s.open_orders.is_empty());
    }

    #[test]
    fn resolve_trigger_price_filters_zero() {
        let mark = HashMap::from([("PF_XBTUSD".into(), 0.0)]);
        let last = HashMap::from([("PF_XBTUSD".into(), 0.0)]);
        let index = HashMap::from([("PF_XBTUSD".into(), 0.0)]);

        assert!(
            resolve_trigger_price(TriggerSignal::Mark, "PF_XBTUSD", &mark, &last, &index).is_none()
        );
        assert!(
            resolve_trigger_price(TriggerSignal::Last, "PF_XBTUSD", &mark, &last, &index).is_none()
        );
        assert!(
            resolve_trigger_price(TriggerSignal::Index, "PF_XBTUSD", &mark, &last, &index)
                .is_none()
        );
    }

    #[test]
    fn resolve_trigger_price_filters_infinity() {
        let mark = HashMap::from([("PF_XBTUSD".into(), f64::INFINITY)]);
        let last = HashMap::from([("PF_XBTUSD".into(), f64::NEG_INFINITY)]);
        let index = HashMap::from([("PF_XBTUSD".into(), f64::NAN)]);

        assert!(
            resolve_trigger_price(TriggerSignal::Mark, "PF_XBTUSD", &mark, &last, &index).is_none()
        );
        assert!(
            resolve_trigger_price(TriggerSignal::Last, "PF_XBTUSD", &mark, &last, &index).is_none()
        );
        assert!(
            resolve_trigger_price(TriggerSignal::Index, "PF_XBTUSD", &mark, &last, &index)
                .is_none()
        );
    }

    #[test]
    fn state_lock_drop_does_not_remove_replaced_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("futures_state.json.lock");
        fs::write(&path, "new-owner").unwrap();

        let lock = StateLock {
            path: path.clone(),
            token: "old-owner".into(),
        };
        drop(lock);

        assert_eq!(fs::read_to_string(&path).unwrap(), "new-owner");
    }

    #[test]
    fn trailing_stop_rejected_without_deviation() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let mark = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::TrailingStop,
            price: None,
            stop_price: Some(48000.0),
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let err = s.place_triggered_order(&params, 10.0, &mark).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("trailing-stop-max-deviation"),
            "Expected trailing-stop-max-deviation error, got: {msg}"
        );
    }

    #[test]
    fn trailing_stop_rejects_invalid_unit() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), 10.0);
        let mark = HashMap::from([("PF_XBTUSD".into(), 50000.0)]);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: Side::Long,
            size: 1.0,
            order_type: FuturesOrderType::TrailingStop,
            price: None,
            stop_price: Some(48000.0),
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(10.0),
            trailing_stop_max_deviation: Some(500.0),
            trailing_stop_deviation_unit: Some("percnt".into()),
        };
        let err = s.place_triggered_order(&params, 10.0, &mark).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("trailing-stop-deviation-unit"),
            "Expected trailing-stop-deviation-unit error, got: {msg}"
        );
    }

    #[test]
    fn leverage_rejects_above_max() {
        let s = test_state();
        let err = s.resolve_leverage(Some(300.0), "PF_XBTUSD");
        assert!(err.is_err(), "Leverage > 200 should be rejected");
    }
}
