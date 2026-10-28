use crate::{
    config,
    errors::{KrakenError, Result},
};
use chrono::{DateTime, Utc};
pub(crate) use kraken_contract::value_types::{FuturesOrderType, TriggerSignal};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, io::Write, path::PathBuf};

pub(crate) const DEFAULT_FUTURES_TAKER_FEE_RATE: Decimal = dec!(0.0005);
pub(crate) const DEFAULT_MAINTENANCE_MARGIN_RATE: Decimal = dec!(0.02);
pub(crate) const MAX_LEVERAGE: Decimal = dec!(200);
const FUNDING_INTERVAL_HOURS: i64 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PositionSide {
    Long,
    Short,
}

impl PositionSide {
    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Long => Self::Short,
            Self::Short => Self::Long,
        }
    }
}

impl std::fmt::Display for PositionSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Long => f.write_str("long"),
            Self::Short => f.write_str("short"),
        }
    }
}

/// Parse a CLI order-type token, naming the valid set on refusal.
///
/// A free function rather than an inherent method: the type is
/// `kraken_contract`'s now, and only its owning crate may add inherent impls.
pub(crate) fn order_type_from_cli(s: &str) -> Result<FuturesOrderType> {
    <FuturesOrderType as clap::ValueEnum>::from_str(s, false).map_err(|_| {
        KrakenError::Validation(format!(
            "Invalid order type '{s}'. Valid: {}",
            cli_tokens::<FuturesOrderType>()
        ))
    })
}

/// Parse a CLI trigger-signal token. Free for the same reason as above.
pub(crate) fn trigger_signal_from_cli(s: &str) -> Result<TriggerSignal> {
    <TriggerSignal as clap::ValueEnum>::from_str(s, false).map_err(|_| {
        KrakenError::Validation(format!(
            "Invalid trigger signal '{s}'. Valid: {}",
            cli_tokens::<TriggerSignal>()
        ))
    })
}

/// Comma-joined clap tokens of a value enum, for validation error messages.
fn cli_tokens<E: clap::ValueEnum>() -> String {
    E::value_variants()
        .iter()
        .filter_map(clap::ValueEnum::to_possible_value)
        .map(|v| v.get_name().to_owned())
        .collect::<Vec<_>>()
        .join(", ")
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FuturesPaperOrder {
    pub(crate) id: String,
    pub(crate) symbol: String,
    pub(crate) side: PositionSide,
    pub(crate) size: Decimal,
    pub(crate) filled_size: Decimal,
    pub(crate) order_type: FuturesOrderType,
    pub(crate) price: Option<Decimal>,
    pub(crate) stop_price: Option<Decimal>,
    pub(crate) trigger_signal: Option<TriggerSignal>,
    pub(crate) client_order_id: Option<String>,
    pub(crate) reduce_only: bool,
    pub(crate) leverage: Decimal,
    pub(crate) reserved_margin: Decimal,
    pub(crate) status: OrderStatus,
    pub(crate) trailing_stop_max_deviation: Option<Decimal>,
    pub(crate) trailing_stop_deviation_unit: Option<String>,
    pub(crate) trailing_anchor: Option<Decimal>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FuturesPaperPosition {
    pub(crate) symbol: String,
    pub(crate) side: PositionSide,
    pub(crate) size: Decimal,
    pub(crate) entry_price: Decimal,
    pub(crate) leverage: Decimal,
    pub(crate) unrealized_funding: Decimal,
    pub(crate) last_funding_time: Option<DateTime<Utc>>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FuturesPaperFill {
    pub(crate) id: String,
    pub(crate) order_id: String,
    pub(crate) symbol: String,
    pub(crate) side: PositionSide,
    pub(crate) size: Decimal,
    pub(crate) price: Decimal,
    pub(crate) fee: Decimal,
    pub(crate) realized_pnl: Option<Decimal>,
    pub(crate) fill_type: String,
    pub(crate) client_order_id: Option<String>,
    pub(crate) filled_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FuturesPaperHistoryEvent {
    pub(crate) id: String,
    pub(crate) event_type: String,
    pub(crate) symbol: Option<String>,
    pub(crate) amount: Decimal,
    pub(crate) details: String,
    pub(crate) timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FuturesPaperState {
    pub(crate) collateral: Decimal,
    pub(crate) currency: String,
    pub(crate) fee_rate: Decimal,
    pub(crate) starting_collateral: Decimal,
    pub(crate) open_orders: Vec<FuturesPaperOrder>,
    pub(crate) positions: Vec<FuturesPaperPosition>,
    pub(crate) fills: Vec<FuturesPaperFill>,
    pub(crate) history: Vec<FuturesPaperHistoryEvent>,
    pub(crate) leverage_preferences: HashMap<String, Decimal>,
    #[serde(default = "default_next_id")]
    next_id: u64,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
    pub(crate) last_reconciled_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) maintenance_margin_fallback_used: bool,
}

fn default_next_id() -> u64 {
    1
}

impl FuturesPaperState {
    pub(crate) fn new(collateral: Decimal, currency: &str, fee_rate: Decimal) -> Self {
        let now = Utc::now();
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
            created_at: now,
            updated_at: now,
            last_reconciled_at: None,
            maintenance_margin_fallback_used: false,
        }
    }

    pub(crate) fn reset(
        &mut self,
        collateral: Option<Decimal>,
        currency: Option<&str>,
        fee_rate: Option<Decimal>,
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

    pub(crate) fn position_margin(&self) -> Decimal {
        self.positions
            .iter()
            .map(|p| (p.size * p.entry_price) / p.leverage)
            .sum()
    }

    pub(crate) fn reserved_order_margin(&self) -> Decimal {
        self.open_orders.iter().map(|o| o.reserved_margin).sum()
    }

    pub(crate) fn unrealized_pnl(&self, mark_prices: &HashMap<String, Decimal>) -> Decimal {
        self.positions
            .iter()
            .map(|p| {
                let mark = mark_prices.get(&p.symbol).copied().unwrap_or(p.entry_price);
                compute_unrealized_pnl(p, mark)
            })
            .sum()
    }

    pub(crate) fn used_margin(&self) -> Decimal {
        self.position_margin() + self.reserved_order_margin()
    }

    pub(crate) fn available_margin(&self, mark_prices: &HashMap<String, Decimal>) -> Decimal {
        self.free_margin(mark_prices).max(Decimal::ZERO)
    }

    /// [`Self::available_margin`] before the clamp: negative when
    /// under-margined.
    fn free_margin(&self, mark_prices: &HashMap<String, Decimal>) -> Decimal {
        self.collateral + self.unrealized_pnl(mark_prices) - self.used_margin()
    }

    pub(crate) fn resolve_leverage(
        &self,
        order_leverage: Option<Decimal>,
        symbol: &str,
    ) -> Result<Decimal> {
        if let Some(lev) = order_leverage {
            if !(Decimal::ONE..=MAX_LEVERAGE).contains(&lev) {
                return Err(KrakenError::Validation(format!(
                    "Leverage must be between Decimal::ONE and {MAX_LEVERAGE:.0}"
                )));
            }
            return Ok(lev);
        }
        if let Some(&pref) = self.leverage_preferences.get(symbol) {
            return Ok(pref);
        }
        Err(KrakenError::Validation(format!(
            "No leverage specified for {symbol}. Use --leverage or set a preference with 'futures \
             paper set-leverage'."
        )))
    }

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
        leverage: Decimal,
        market: &MarketSnapshot,
    ) -> Result<OrderPlacementResult> {
        let fill_price = match params.side {
            PositionSide::Long => market.ask,
            PositionSide::Short => market.bid,
        };
        if fill_price <= Decimal::ZERO {
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
        self.updated_at = Utc::now();

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
        leverage: Decimal,
        _market: &MarketSnapshot,
        mark_prices: &HashMap<String, Decimal>,
    ) -> Result<OrderPlacementResult> {
        let price = params
            .price
            .ok_or_else(|| KrakenError::Validation("Limit orders require --price".into()))?;

        self.validate_reduce_only(params)?;
        let margin_delta = self.compute_margin_delta(params, price, leverage);
        self.check_margin(margin_delta, mark_prices)?;

        let order_id = self.next_id();
        let reserved = margin_delta.max(Decimal::ZERO);

        let order = FuturesPaperOrder {
            id: order_id.clone(),
            symbol: params.symbol.clone(),
            side: params.side,
            size: params.size,
            filled_size: Decimal::ZERO,
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
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        self.open_orders.push(order);
        self.updated_at = Utc::now();

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
        leverage: Decimal,
        market: &MarketSnapshot,
    ) -> Result<OrderPlacementResult> {
        let price = params
            .price
            .ok_or_else(|| KrakenError::Validation("Post-only orders require --price".into()))?;

        let crosses = match params.side {
            PositionSide::Long => price >= market.ask,
            PositionSide::Short => price <= market.bid,
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
        leverage: Decimal,
        market: &MarketSnapshot,
    ) -> Result<OrderPlacementResult> {
        let price = params
            .price
            .ok_or_else(|| KrakenError::Validation("IOC orders require --price".into()))?;

        self.validate_reduce_only(params)?;

        let contra_levels = match params.side {
            PositionSide::Long => &market.ask_levels,
            PositionSide::Short => &market.bid_levels,
        };
        let executable_size =
            compute_executable_depth(contra_levels, params.side, price, params.size);

        if executable_size <= Decimal::ZERO {
            return Ok(OrderPlacementResult {
                order_id: self.next_id(),
                status: OrderStatus::Cancelled,
                fills: Vec::new(),
                message: Some("IOC: no executable depth at requested price".into()),
            });
        }

        let fill_price = match params.side {
            PositionSide::Long => market.ask,
            PositionSide::Short => market.bid,
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
        self.updated_at = Utc::now();

        let status = if executable_size == params.size {
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
        leverage: Decimal,
        market: &MarketSnapshot,
    ) -> Result<OrderPlacementResult> {
        let price = params
            .price
            .ok_or_else(|| KrakenError::Validation("FOK orders require --price".into()))?;

        self.validate_reduce_only(params)?;

        let contra_levels = match params.side {
            PositionSide::Long => &market.ask_levels,
            PositionSide::Short => &market.bid_levels,
        };
        let executable_size =
            compute_executable_depth(contra_levels, params.side, price, params.size);

        if executable_size != params.size {
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
            PositionSide::Long => market.ask,
            PositionSide::Short => market.bid,
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
        self.updated_at = Utc::now();

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
        leverage: Decimal,
        mark_prices: &HashMap<String, Decimal>,
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
                    "Invalid trailing-stop-deviation-unit '{unit}'. Use 'percent' or \
                     'quote_currency'."
                )));
            }
            Some(unit.to_string())
        } else {
            params.trailing_stop_deviation_unit.clone()
        };

        self.validate_reduce_only(params)?;

        // Same netting as limit orders and edits, so an edit never reprices it.
        let eff_price = params.price.unwrap_or(stop_price);
        let margin_delta = self.compute_margin_delta(params, eff_price, leverage);
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
            filled_size: Decimal::ZERO,
            order_type: params.order_type,
            price: params.price,
            stop_price: Some(stop_price),
            trigger_signal: Some(trigger),
            client_order_id: params.client_order_id.clone(),
            reduce_only: params.reduce_only,
            leverage,
            reserved_margin: margin_delta.max(Decimal::ZERO),
            status: OrderStatus::Open,
            trailing_stop_max_deviation: params.trailing_stop_max_deviation,
            trailing_stop_deviation_unit: resolved_trailing_unit,
            trailing_anchor,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        self.open_orders.push(order);
        self.updated_at = Utc::now();

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

    fn compute_margin_delta(
        &self,
        params: &OrderParams,
        price: Decimal,
        leverage: Decimal,
    ) -> Decimal {
        self.compute_margin_delta_for_size(params, price, leverage, params.size)
    }

    fn compute_margin_delta_for_size(
        &self,
        params: &OrderParams,
        price: Decimal,
        leverage: Decimal,
        size: Decimal,
    ) -> Decimal {
        if params.reduce_only {
            return Decimal::ZERO;
        }
        self.net_exposure_margin(&params.symbol, params.side, size, price, leverage)
    }

    fn net_exposure_margin(
        &self,
        symbol: &str,
        side: PositionSide,
        size: Decimal,
        price: Decimal,
        leverage: Decimal,
    ) -> Decimal {
        let opposite_side = side.opposite();
        let opposite_pos_size = self
            .positions
            .iter()
            .find(|p| p.symbol == symbol && p.side == opposite_side)
            .map(|p| p.size)
            .unwrap_or(Decimal::ZERO);

        let net_new_exposure = (size - opposite_pos_size).max(Decimal::ZERO);
        (net_new_exposure * price) / leverage
    }

    fn check_margin(
        &self,
        margin_required: Decimal,
        mark_prices: &HashMap<String, Decimal>,
    ) -> Result<()> {
        if margin_required <= Decimal::ZERO {
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

    /// Rejects a triggered order whose netted reserve went stale: a fill since
    /// placement shrank the position it netted against. Call it after the
    /// order left `open_orders`, so its own reserve counts as available.
    fn check_stale_reserve(
        &self,
        order: &FuturesPaperOrder,
        mark_prices: &HashMap<String, Decimal>,
    ) -> Result<()> {
        // Priced as at placement, so a gap past the stop never cancels a
        // covered order.
        let reserved_at = order.price.or(order.stop_price).unwrap_or(Decimal::ZERO);
        let needed = self.net_exposure_margin(
            &order.symbol,
            order.side,
            order.size,
            reserved_at,
            order.leverage,
        );
        if needed <= order.reserved_margin {
            return Ok(());
        }
        // The fill closes the opposite position first, which frees its margin.
        let freed = self
            .positions
            .iter()
            .find(|p| p.symbol == order.symbol && p.side == order.side.opposite())
            .map(|p| order.size.min(p.size) * p.entry_price / p.leverage)
            .unwrap_or(Decimal::ZERO);
        let available = self.free_margin(mark_prices) + freed;
        if available < needed {
            return Err(KrakenError::Validation(format!(
                "Insufficient margin. Required: {needed:.2}, Available: {available:.2}"
            )));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_fill(
        &mut self,
        fill_id: &str,
        order_id: &str,
        symbol: &str,
        side: PositionSide,
        size: Decimal,
        price: Decimal,
        fee: Decimal,
        client_order_id: Option<String>,
        fill_type: &str,
        leverage: Decimal,
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
            filled_at: Utc::now(),
        };
        self.fills.push(fill.clone());
        fill
    }

    fn net_position(
        &mut self,
        symbol: &str,
        fill_side: PositionSide,
        fill_size: Decimal,
        fill_price: Decimal,
        leverage: Decimal,
    ) -> Option<Decimal> {
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
                    if remaining <= Decimal::ZERO {
                        self.positions.remove(idx);
                    } else {
                        self.positions[idx].size = remaining;
                        self.positions[idx].updated_at = Utc::now();
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

                    self.record_history(
                        "realized_pnl",
                        Some(symbol),
                        close_pnl,
                        &format!(
                            "Closed {opp_size:.8} {opposite_side} at {fill_price:.2}, flipping to \
                             {fill_side}"
                        ),
                    );

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
                    self.positions[idx].updated_at = Utc::now();
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
        side: PositionSide,
        size: Decimal,
        price: Decimal,
        leverage: Decimal,
    ) {
        let now = Utc::now();
        self.positions.push(FuturesPaperPosition {
            symbol: symbol.to_string(),
            side,
            size,
            entry_price: price,
            leverage,
            unrealized_funding: Decimal::ZERO,
            last_funding_time: Some(now),
            created_at: now,
            updated_at: now,
        });
    }

    pub(crate) fn edit_order(
        &mut self,
        order_id: &str,
        new_size: Option<Decimal>,
        new_price: Option<Decimal>,
        new_stop_price: Option<Decimal>,
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
        let eff_price = new_price
            .or(order.price)
            .or(eff_stop)
            .unwrap_or(Decimal::ZERO);
        let new_reserved = if order.reduce_only {
            Decimal::ZERO
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

        if margin_increase > Decimal::ZERO {
            let available = self.available_margin(&HashMap::new());
            if available < margin_increase {
                return Err(KrakenError::Validation(format!(
                    "Edit would require {margin_increase:.2} additional margin, but only \
                     {available:.2} available"
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
            let order = &mut self.open_orders[idx];
            order.stop_price = Some(sp);
            // A trailing stop triggers off its anchor, not `stop_price`.
            if order.order_type == FuturesOrderType::TrailingStop {
                order.trailing_anchor = Some(sp);
            }
        }
        self.open_orders[idx].reserved_margin = new_reserved;
        self.open_orders[idx].updated_at = Utc::now();
        self.updated_at = Utc::now();
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
                ));
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
                ));
            }
        };

        let order = self.open_orders.remove(idx);
        self.record_history(
            "order_cancelled",
            Some(&order.symbol),
            Decimal::ZERO,
            &format!("User cancelled order {}", order.id),
        );
        self.updated_at = Utc::now();
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
                Decimal::ZERO,
                &format!("User cancelled order {} (cancel-all)", order.id),
            );
        }
        self.updated_at = Utc::now();
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

    pub(crate) fn reconcile(
        &mut self,
        mark_prices: &HashMap<String, Decimal>,
        last_prices: &HashMap<String, Decimal>,
        index_prices: &HashMap<String, Decimal>,
        funding_rates: &HashMap<String, Decimal>,
        maintenance_rates: &HashMap<String, Decimal>,
    ) -> ReconcileResult {
        let mut fills = Vec::new();
        let mut liquidations = Vec::new();
        let mut funding_events = Vec::new();

        self.reconcile_limit_orders(last_prices, mark_prices, &mut fills);
        self.reconcile_triggered_orders(mark_prices, last_prices, index_prices, &mut fills);
        self.reconcile_liquidations(mark_prices, maintenance_rates, &mut liquidations);
        self.reconcile_funding(funding_rates, mark_prices, &mut funding_events);

        self.last_reconciled_at = Some(Utc::now());
        self.updated_at = Utc::now();

        ReconcileResult {
            fills,
            liquidations,
            funding_events,
        }
    }

    fn reconcile_limit_orders(
        &mut self,
        last_prices: &HashMap<String, Decimal>,
        mark_prices: &HashMap<String, Decimal>,
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
                Some(&p) if p > Decimal::ZERO => p,
                _ => continue,
            };

            let is_marketable = match order.side {
                PositionSide::Long => limit_price >= last,
                PositionSide::Short => limit_price <= last,
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
                    Decimal::ZERO,
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
                        Decimal::ZERO,
                        &format!(
                            "Reduce-only limit {} cancelled: no reducible position",
                            order.id
                        ),
                    );
                    continue;
                }
            }

            let fill_price = match order.price {
                Some(p) if p > Decimal::ZERO => p,
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
        mark_prices: &HashMap<String, Decimal>,
        last_prices: &HashMap<String, Decimal>,
        index_prices: &HashMap<String, Decimal>,
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
                    PositionSide::Long => *anchor = (*anchor).min(current),
                    PositionSide::Short => *anchor = (*anchor).max(current),
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
                    PositionSide::Long => order.stop_price.map(|sp| price >= sp).unwrap_or(false),
                    PositionSide::Short => order.stop_price.map(|sp| price <= sp).unwrap_or(false),
                },
                FuturesOrderType::TakeProfit => match order.side {
                    PositionSide::Long => order.stop_price.map(|sp| price <= sp).unwrap_or(false),
                    PositionSide::Short => order.stop_price.map(|sp| price >= sp).unwrap_or(false),
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
                .filter(|p| *p > Decimal::ZERO)
                .or(signal_price)
                .or_else(|| {
                    mark_prices
                        .get(&order.symbol)
                        .copied()
                        .filter(|p| *p > Decimal::ZERO)
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
                        Decimal::ZERO,
                        &format!(
                            "Reduce-only order {} cancelled at trigger: no reducible position",
                            order.id
                        ),
                    );
                    continue;
                }
            } else if let Err(e) = self.check_stale_reserve(&order, mark_prices) {
                self.record_history(
                    "order_cancelled",
                    Some(&order.symbol),
                    Decimal::ZERO,
                    &format!("Order {} cancelled at trigger: {e}", order.id),
                );
                continue;
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
        mark_prices: &HashMap<String, Decimal>,
        maintenance_rates: &HashMap<String, Decimal>,
        liquidations: &mut Vec<FuturesPaperFill>,
    ) {
        let mut liq_indices = Vec::new();

        for (i, pos) in self.positions.iter().enumerate() {
            let mark = match mark_prices.get(&pos.symbol) {
                Some(&p) if p > Decimal::ZERO => p,
                _ => continue,
            };

            let maint_rate = match maintenance_rates.get(&pos.symbol) {
                Some(&r) if r > Decimal::ZERO => r,
                _ => {
                    self.maintenance_margin_fallback_used = true;
                    DEFAULT_MAINTENANCE_MARGIN_RATE
                }
            };

            let liq_price = compute_liquidation_price(pos, maint_rate);
            let is_liquidated = match pos.side {
                PositionSide::Long => mark <= liq_price,
                PositionSide::Short => mark >= liq_price,
            };

            if is_liquidated {
                liq_indices.push((i, mark));
            }
        }

        liq_indices.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.0));

        for (idx, mark) in liq_indices {
            let pos = self.positions.remove(idx);
            let pnl = compute_realized_pnl(pos.side, pos.entry_price, mark, pos.size);
            self.collateral += pnl;

            let fill_id = self.next_id();
            let order_id = format!("LIQ-{}", fill_id);

            let fill = FuturesPaperFill {
                id: fill_id,
                order_id: order_id.clone(),
                symbol: pos.symbol.clone(),
                side: pos.side.opposite(),
                size: pos.size,
                price: mark,
                fee: Decimal::ZERO,
                realized_pnl: Some(pnl),
                fill_type: "liquidation".to_string(),
                client_order_id: None,
                filled_at: Utc::now(),
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
        funding_rates: &HashMap<String, Decimal>,
        mark_prices: &HashMap<String, Decimal>,
        events: &mut Vec<FuturesPaperHistoryEvent>,
    ) {
        let now = Utc::now();
        let id_base = self.next_id;
        let mut id_offset = 0u64;

        for pos in &mut self.positions {
            let rate = match funding_rates.get(&pos.symbol) {
                Some(&r) => r,
                _ => continue,
            };

            let intervals_elapsed = match pos.last_funding_time {
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
                Some(&m) if m > Decimal::ZERO => m,
                _ => pos.entry_price,
            };
            let notional = pos.size * mark;
            let funding_per_interval = notional * rate;
            let total_funding = funding_per_interval * Decimal::from(intervals_elapsed);

            let amount = match pos.side {
                PositionSide::Long => -total_funding,
                PositionSide::Short => total_funding,
            };

            pos.unrealized_funding += amount;
            pos.last_funding_time = Some(now);
            pos.updated_at = now;

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
                timestamp: now,
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
        amount: Decimal,
        details: &str,
    ) {
        let id = self.next_id();
        self.history.push(FuturesPaperHistoryEvent {
            id,
            event_type: event_type.to_string(),
            symbol: symbol.map(|s| s.to_string()),
            amount,
            details: details.to_string(),
            timestamp: Utc::now(),
        });
    }
}

pub(crate) fn compute_unrealized_pnl(pos: &FuturesPaperPosition, mark_price: Decimal) -> Decimal {
    match pos.side {
        PositionSide::Long => (mark_price - pos.entry_price) * pos.size,
        PositionSide::Short => (pos.entry_price - mark_price) * pos.size,
    }
}

pub(crate) fn compute_liquidation_price(
    pos: &FuturesPaperPosition,
    maintenance_margin_rate: Decimal,
) -> Decimal {
    match pos.side {
        PositionSide::Long => {
            pos.entry_price
                * (Decimal::ONE - (Decimal::ONE / pos.leverage) + maintenance_margin_rate)
        }
        PositionSide::Short => {
            pos.entry_price
                * (Decimal::ONE + (Decimal::ONE / pos.leverage) - maintenance_margin_rate)
        }
    }
}

fn compute_realized_pnl(
    side: PositionSide,
    entry: Decimal,
    exit: Decimal,
    size: Decimal,
) -> Decimal {
    match side {
        PositionSide::Long => (exit - entry) * size,
        PositionSide::Short => (entry - exit) * size,
    }
}

fn compute_executable_depth(
    levels: &[(Decimal, Decimal)],
    side: PositionSide,
    limit_price: Decimal,
    max_size: Decimal,
) -> Decimal {
    if levels.is_empty() {
        return Decimal::ZERO;
    }
    let mut filled = Decimal::ZERO;
    for &(price, available) in levels {
        if price <= Decimal::ZERO || available < Decimal::ZERO {
            continue;
        }
        let acceptable = match side {
            PositionSide::Long => price <= limit_price,
            PositionSide::Short => price >= limit_price,
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
    mark_prices: &HashMap<String, Decimal>,
    last_prices: &HashMap<String, Decimal>,
    index_prices: &HashMap<String, Decimal>,
) -> Option<Decimal> {
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
    raw.filter(|&p| p > Decimal::ZERO)
}

fn check_trailing_stop_trigger(order: &FuturesPaperOrder, mark: Decimal) -> bool {
    let Some(anchor) = order.trailing_anchor else {
        return false;
    };
    let Some(max_dev) = order.trailing_stop_max_deviation else {
        return false;
    };

    let deviation = match order.trailing_stop_deviation_unit.as_deref() {
        Some("quote_currency") => (mark - anchor).abs(),
        _ => (mark - anchor).abs() / anchor * dec!(100.0),
    };

    match order.side {
        PositionSide::Long => mark >= anchor && deviation >= max_dev,
        PositionSide::Short => mark <= anchor && deviation >= max_dev,
    }
}

fn validate_finite_positive(val: Decimal, name: &str) -> Result<()> {
    if val <= Decimal::ZERO {
        return Err(KrakenError::Validation(format!(
            "{name} must be a finite positive number"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct OrderParams {
    pub(crate) symbol: String,
    pub(crate) side: PositionSide,
    pub(crate) size: Decimal,
    pub(crate) order_type: FuturesOrderType,
    pub(crate) price: Option<Decimal>,
    pub(crate) stop_price: Option<Decimal>,
    pub(crate) trigger_signal: Option<TriggerSignal>,
    pub(crate) client_order_id: Option<String>,
    pub(crate) reduce_only: bool,
    pub(crate) leverage: Option<Decimal>,
    pub(crate) trailing_stop_max_deviation: Option<Decimal>,
    pub(crate) trailing_stop_deviation_unit: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct MarketSnapshot {
    pub(crate) bid: Decimal,
    pub(crate) ask: Decimal,
    pub(crate) last: Decimal,
    pub(crate) mark: Decimal,
    pub(crate) index: Decimal,
    pub(crate) ask_levels: Vec<(Decimal, Decimal)>,
    pub(crate) bid_levels: Vec<(Decimal, Decimal)>,
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

/// Advisory lock over the futures paper state file. A thin wrapper around the
/// shared [`FileLock`](kraken_recording::FileLock) that preserves the
/// no-argument `acquire()` call site used throughout the futures-paper
/// commands. Overlapping futures-paper commands are a conflict: the loser is
/// rejected with `validation`, not queued.
pub(crate) struct StateLock {
    // Held purely for its `Drop`; the inner `FileLock` owns the lock lifecycle.
    #[allow(dead_code)]
    inner: kraken_recording::FileLock,
}

impl StateLock {
    pub(crate) fn acquire() -> Result<Self> {
        let inner = kraken_recording::FileLock::acquire(
            &futures_paper_state_path()?,
            "Futures paper state",
        )?;
        Ok(Self { inner })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> FuturesPaperState {
        FuturesPaperState::new(dec!(10000.0), "USD", DEFAULT_FUTURES_TAKER_FEE_RATE)
    }

    fn market_snapshot(bid: Decimal, ask: Decimal) -> MarketSnapshot {
        MarketSnapshot {
            bid,
            ask,
            last: (bid + ask) / dec!(2.0),
            mark: (bid + ask) / dec!(2.0),
            index: (bid + ask) / dec!(2.0),
            ask_levels: vec![(ask, dec!(100.0))],
            bid_levels: vec![(bid, dec!(100.0))],
        }
    }

    #[test]
    fn reset_clears_state() {
        let mut s = test_state();
        s.positions.push(FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            entry_price: dec!(50000.0),
            leverage: dec!(10.0),
            unrealized_funding: Decimal::ZERO,
            last_funding_time: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });
        s.reset(None, None, None);
        assert!(s.positions.is_empty());
        assert_eq!(s.collateral, dec!(10000.0));
    }

    #[test]
    fn leverage_resolution_preference_fallback() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let lev = s.resolve_leverage(None, "PF_XBTUSD").unwrap();
        assert_eq!(lev, dec!(10.0));
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        assert_eq!(result.status, OrderStatus::Filled);
        assert_eq!(result.fills.len(), 1);
        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.positions[0].side, PositionSide::Long);
        assert_eq!(s.positions[0].size, Decimal::ONE);
    }

    #[test]
    fn same_side_aggregates_weighted_entry() {
        let mut s = FuturesPaperState::new(dec!(100000.0), "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));

        let snap1 = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params1 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params1, &snap1).unwrap();

        let snap2 = market_snapshot(dec!(51900.0), dec!(52000.0));
        let params2 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params2, &snap2).unwrap();

        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.positions[0].size, dec!(2.0));
        let expected_entry = (dec!(50000.0) + dec!(52000.0)) / dec!(2.0);
        assert_eq!(s.positions[0].entry_price, expected_entry);
    }

    #[test]
    fn opposite_side_reduces_and_closes() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));

        let snap1 = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params1 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Short,
            size: dec!(2.0),
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params1, &snap1).unwrap();
        assert_eq!(s.positions[0].side, PositionSide::Short);
        assert_eq!(s.positions[0].size, dec!(2.0));

        let snap2 = market_snapshot(dec!(48900.0), dec!(49000.0));
        let params2 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params2, &snap2).unwrap();

        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.positions[0].side, PositionSide::Short);
        assert_eq!(s.positions[0].size, Decimal::ONE);
        assert!(result.fills[0].realized_pnl.is_some());
        let pnl = result.fills[0].realized_pnl.unwrap();
        assert!(
            pnl > Decimal::ZERO,
            "short closed at profit should be positive"
        );
    }

    #[test]
    fn opposite_side_overfill_flips_direction() {
        let mut s = FuturesPaperState::new(dec!(100000.0), "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));

        let snap1 = market_snapshot(dec!(49900.0), dec!(50000.0));
        let p1 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Short,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(p1, &snap1).unwrap();

        let snap2 = market_snapshot(dec!(47900.0), dec!(48000.0));
        let p2 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: dec!(3.0),
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(p2, &snap2).unwrap();

        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.positions[0].side, PositionSide::Long);
        assert_eq!(s.positions[0].size, dec!(2.0));
        assert!(result.fills[0].realized_pnl.is_some());
    }

    #[test]
    fn post_only_rejects_crossing() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Post,
            price: Some(dec!(50100.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = MarketSnapshot {
            bid: dec!(49900.0),
            ask: dec!(50000.0),
            last: dec!(49950.0),
            mark: dec!(49950.0),
            index: dec!(49950.0),
            ask_levels: vec![(dec!(50000.0), dec!(0.5))],
            bid_levels: vec![(dec!(49900.0), dec!(100.0))],
        };
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Fok,
            price: Some(dec!(50000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        assert_eq!(result.status, OrderStatus::Rejected);
    }

    #[test]
    fn ioc_partially_fills() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = MarketSnapshot {
            bid: dec!(49900.0),
            ask: dec!(50000.0),
            last: dec!(49950.0),
            mark: dec!(49950.0),
            index: dec!(49950.0),
            ask_levels: vec![(dec!(50000.0), dec!(0.5))],
            bid_levels: vec![(dec!(49900.0), dec!(100.0))],
        };
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Ioc,
            price: Some(dec!(50000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        assert_eq!(result.status, OrderStatus::PartiallyFilled);
        assert_eq!(result.fills.len(), 1);
        assert_eq!(result.fills[0].size, dec!(0.5));
    }

    #[test]
    fn margin_rejects_over_leveraged() {
        let mut s = FuturesPaperState::new(dec!(100.0), "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences.insert("PF_XBTUSD".into(), dec!(2.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(2.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Insufficient margin")
        );
    }

    #[test]
    fn reduce_only_rejects_no_position() {
        let mut s = test_state();
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: true,
            leverage: Some(dec!(10.0)),
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: Some("my-signal-123".into()),
            reduce_only: false,
            leverage: Some(dec!(10.0)),
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(48000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: Some("test-cancel".into()),
            reduce_only: false,
            leverage: Some(dec!(10.0)),
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(48000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: Some("test-cancel-cid".into()),
            reduce_only: false,
            leverage: Some(dec!(10.0)),
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(48000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        for _ in 0..3 {
            let params = OrderParams {
                symbol: "PF_XBTUSD".into(),
                side: PositionSide::Long,
                size: dec!(0.1),
                order_type: FuturesOrderType::Limit,
                price: Some(dec!(48000.0)),
                stop_price: None,
                trigger_signal: None,
                client_order_id: None,
                reduce_only: false,
                leverage: Some(dec!(10.0)),
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
        assert!(
            s.history
                .iter()
                .rev()
                .take(3)
                .all(|h| h.event_type == "order_cancelled" && h.details.contains("cancel-all"))
        );
    }

    #[test]
    fn cancel_all_with_symbol_filter() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        s.leverage_preferences
            .insert("PF_ETHUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));

        let p1 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(48000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(p1, &snap).unwrap();

        let p2 = OrderParams {
            symbol: "PF_ETHUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(3000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(48000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();

        s.edit_order(&result.order_id, Some(dec!(2.0)), Some(dec!(47000.0)), None)
            .unwrap();
        assert_eq!(s.open_orders[0].size, dec!(2.0));
        assert_eq!(s.open_orders[0].price.unwrap(), dec!(47000.0));
    }

    #[test]
    fn editing_a_trailing_stops_stop_price_moves_its_anchor() {
        // A trailing stop triggers off its anchor, so an edit that left the
        // anchor behind changed nothing. Ported from GitHub PR #54.
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let mark: HashMap<String, Decimal> = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::TrailingStop,
            price: None,
            stop_price: Some(dec!(90000.0)),
            trigger_signal: Some(TriggerSignal::Last),
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: Some(dec!(2.0)),
            trailing_stop_deviation_unit: Some("percent".into()),
        };
        let placed = s.place_triggered_order(&params, dec!(10.0), &mark).unwrap();

        s.edit_order(&placed.order_id, None, None, Some(dec!(85000.0)))
            .unwrap();
        assert_eq!(s.open_orders[0].stop_price, Some(dec!(85000.0)));
        assert_eq!(s.open_orders[0].trailing_anchor, Some(dec!(85000.0)));
    }

    #[test]
    fn a_triggered_order_nets_its_margin_against_an_opposite_position() {
        // Limit orders and edits already netted; a stop reserved the full
        // notional, and its first edit dropped the reserve to zero. Ported
        // from GitHub PR #56.
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(82700.0), dec!(82800.0));
        let order = |side, order_type, stop_price| OrderParams {
            symbol: "PF_XBTUSD".into(),
            side,
            size: dec!(0.1),
            order_type,
            price: None,
            stop_price,
            trigger_signal: stop_price.map(|_| TriggerSignal::Last),
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(
            order(PositionSide::Short, FuturesOrderType::Market, None),
            &snap,
        )
        .unwrap();
        let used_after_short = s.used_margin();

        let placed = s
            .place_order(
                order(
                    PositionSide::Long,
                    FuturesOrderType::Stop,
                    Some(dec!(85000.0)),
                ),
                &snap,
            )
            .unwrap();
        assert_eq!(s.open_orders[0].reserved_margin, Decimal::ZERO);
        assert_eq!(s.used_margin(), used_after_short);

        // Only the 0.2 that exceeds the short is new exposure.
        s.edit_order(&placed.order_id, Some(dec!(0.3)), None, None)
            .unwrap();
        assert_eq!(
            s.open_orders[0].reserved_margin,
            dec!(0.2) * dec!(85000.0) / dec!(10.0)
        );
    }

    #[test]
    fn a_stop_limit_reserves_margin_at_its_limit_price() {
        let mut s = test_state();
        let snap = market_snapshot(dec!(82700.0), dec!(82800.0));
        let placed = s
            .place_order(
                OrderParams {
                    symbol: "PF_XBTUSD".into(),
                    side: PositionSide::Long,
                    size: Decimal::ONE,
                    order_type: FuturesOrderType::Stop,
                    price: Some(dec!(86000.0)),
                    stop_price: Some(dec!(85000.0)),
                    trigger_signal: Some(TriggerSignal::Last),
                    client_order_id: None,
                    reduce_only: false,
                    leverage: Some(dec!(10.0)),
                    trailing_stop_max_deviation: None,
                    trailing_stop_deviation_unit: None,
                },
                &snap,
            )
            .unwrap();
        let limit_margin = dec!(86000.0) / dec!(10.0);
        assert_eq!(s.open_orders[0].reserved_margin, limit_margin);

        s.edit_order(&placed.order_id, None, None, Some(dec!(85500.0)))
            .unwrap();
        assert_eq!(s.open_orders[0].reserved_margin, limit_margin);
    }

    #[test]
    fn a_triggered_order_is_cancelled_when_its_netted_position_is_gone() {
        // Both legs of a bracket net to zero against the short. Once the stop
        // closes it, the take-profit would open a long nobody reserved for.
        let mut s = FuturesPaperState::new(dec!(500.0), "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        let snap = market_snapshot(dec!(82700.0), dec!(82800.0));
        let order = |side, order_type, stop_price| OrderParams {
            symbol: "PF_XBTUSD".into(),
            side,
            size: dec!(0.1),
            order_type,
            price: None,
            stop_price,
            trigger_signal: stop_price.map(|_| TriggerSignal::Last),
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(20.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(
            order(PositionSide::Short, FuturesOrderType::Market, None),
            &snap,
        )
        .unwrap();
        for (order_type, stop) in [
            (FuturesOrderType::Stop, dec!(85000.0)),
            (FuturesOrderType::TakeProfit, dec!(80000.0)),
        ] {
            s.place_order(order(PositionSide::Long, order_type, Some(stop)), &snap)
                .unwrap();
        }
        assert_eq!(reconcile_at(&mut s, dec!(85000.0)).fills.len(), 1);
        assert!(s.positions.is_empty());

        // 0.1 * 80000 / 20 = 400 of margin, against under 300 of collateral.
        assert!(reconcile_at(&mut s, dec!(80000.0)).fills.is_empty());
        assert!(s.positions.is_empty());
        assert!(s.open_orders.is_empty());
        assert!(s.history.iter().any(
            |h| h.event_type == "order_cancelled" && h.details.contains("Insufficient margin")
        ));
    }

    fn reconcile_at(s: &mut FuturesPaperState, price: Decimal) -> ReconcileResult {
        let prices = HashMap::from([("PF_XBTUSD".to_string(), price)]);
        s.reconcile(
            &prices,
            &prices,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        )
    }

    fn xbt_order(
        side: PositionSide,
        size: Decimal,
        order_type: FuturesOrderType,
        stop_price: Option<Decimal>,
    ) -> OrderParams {
        OrderParams {
            symbol: "PF_XBTUSD".into(),
            side,
            size,
            order_type,
            price: None,
            stop_price,
            trigger_signal: stop_price.map(|_| TriggerSignal::Last),
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        }
    }

    #[test]
    fn a_stale_stop_fills_when_the_margin_its_close_frees_covers_the_reverse() {
        // Before the close, free margin is 294 after fees against the 380
        // the reverse needs; closing the 0.05 long frees its 400.
        let mut s = FuturesPaperState::new(dec!(900.0), "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        let snap = market_snapshot(dec!(80000.0), dec!(80000.0));
        s.place_order(
            xbt_order(
                PositionSide::Long,
                dec!(0.1),
                FuturesOrderType::Market,
                None,
            ),
            &snap,
        )
        .unwrap();
        s.place_order(
            xbt_order(
                PositionSide::Short,
                dec!(0.1),
                FuturesOrderType::Stop,
                Some(dec!(76000.0)),
            ),
            &snap,
        )
        .unwrap();
        s.place_order(
            xbt_order(
                PositionSide::Short,
                dec!(0.05),
                FuturesOrderType::Market,
                None,
            ),
            &snap,
        )
        .unwrap();

        assert_eq!(reconcile_at(&mut s, dec!(76000.0)).fills.len(), 1);
        assert!(s.open_orders.is_empty());
        let positions: Vec<_> = s.positions.iter().map(|p| (p.side, p.size)).collect();
        assert_eq!(positions, [(PositionSide::Short, dec!(0.05))]);
    }

    #[test]
    fn a_fully_reserved_stop_fills_when_the_price_gaps_past_it() {
        // The reserve was 8500 at the stop; at the 86000 fill a fresh check
        // would need 8600 of the 8550 collateral.
        let mut s = FuturesPaperState::new(dec!(8550.0), "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        let snap = market_snapshot(dec!(82700.0), dec!(82800.0));
        s.place_order(
            xbt_order(
                PositionSide::Long,
                Decimal::ONE,
                FuturesOrderType::Stop,
                Some(dec!(85000.0)),
            ),
            &snap,
        )
        .unwrap();
        assert_eq!(s.open_orders[0].reserved_margin, dec!(8500.0));

        assert_eq!(reconcile_at(&mut s, dec!(86000.0)).fills.len(), 1);
        assert_eq!(s.positions.len(), 1);
    }

    #[test]
    fn a_fully_reserved_stop_fills_after_another_position_loses_value() {
        // The ETH loss leaves 8200 free once the stop's 8500 reserve is
        // released; the reserve, not the free margin, covers the stop.
        let mut s = FuturesPaperState::new(dec!(9000.0), "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.positions.push(FuturesPaperPosition {
            symbol: "PF_ETHUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            entry_price: dec!(3000.0),
            leverage: dec!(10.0),
            unrealized_funding: Decimal::ZERO,
            last_funding_time: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });
        let snap = market_snapshot(dec!(82700.0), dec!(82800.0));
        s.place_order(
            xbt_order(
                PositionSide::Long,
                Decimal::ONE,
                FuturesOrderType::Stop,
                Some(dec!(85000.0)),
            ),
            &snap,
        )
        .unwrap();

        let prices = HashMap::from([
            ("PF_XBTUSD".to_string(), dec!(85000.0)),
            ("PF_ETHUSD".to_string(), dec!(2500.0)),
        ]);
        let result = s.reconcile(
            &prices,
            &prices,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(result.fills.len(), 1);
        assert!(s.positions.iter().any(|p| p.symbol == "PF_XBTUSD"));
    }

    #[test]
    fn edit_order_updates_stop_price() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let mark: HashMap<String, Decimal> = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Stop,
            price: None,
            stop_price: Some(dec!(48000.0)),
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_triggered_order(&params, dec!(10.0), &mark).unwrap();
        assert_eq!(s.open_orders[0].stop_price, Some(dec!(48000.0)));

        s.edit_order(
            &s.open_orders[0].id.clone(),
            None,
            None,
            Some(dec!(47000.0)),
        )
        .unwrap();
        assert_eq!(s.open_orders[0].stop_price, Some(dec!(47000.0)));
    }

    #[test]
    fn unrealized_pnl_sign_correct() {
        let long_pos = FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            entry_price: dec!(50000.0),
            leverage: dec!(10.0),
            unrealized_funding: Decimal::ZERO,
            last_funding_time: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        assert!(compute_unrealized_pnl(&long_pos, dec!(51000.0)) > Decimal::ZERO);
        assert!(compute_unrealized_pnl(&long_pos, dec!(49000.0)) < Decimal::ZERO);

        let short_pos = FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Short,
            size: Decimal::ONE,
            entry_price: dec!(50000.0),
            leverage: dec!(10.0),
            unrealized_funding: Decimal::ZERO,
            last_funding_time: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        assert!(compute_unrealized_pnl(&short_pos, dec!(49000.0)) > Decimal::ZERO);
        assert!(compute_unrealized_pnl(&short_pos, dec!(51000.0)) < Decimal::ZERO);
    }

    #[test]
    fn liquidation_price_computation() {
        let pos = FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            entry_price: dec!(50000.0),
            leverage: dec!(10.0),
            unrealized_funding: Decimal::ZERO,
            last_funding_time: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let liq = compute_liquidation_price(&pos, dec!(0.02));
        let expected = dec!(50000.0) * (Decimal::ONE - dec!(0.1) + dec!(0.02));
        assert_eq!(liq, expected);
    }

    #[test]
    fn reconciliation_liquidates_breached_position() {
        let mut s = test_state();
        s.positions.push(FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            entry_price: dec!(50000.0),
            leverage: dec!(10.0),
            unrealized_funding: Decimal::ZERO,
            last_funding_time: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });

        let liq_price = compute_liquidation_price(&s.positions[0], dec!(0.02));
        let mut mark_prices = HashMap::new();
        mark_prices.insert("PF_XBTUSD".to_string(), liq_price - dec!(100.0));

        let mut maint = HashMap::new();
        maint.insert("PF_XBTUSD".to_string(), dec!(0.02));

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
        let mut s = FuturesPaperState::new(dec!(10000.0), "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences.insert("PF_XBTUSD".into(), dec!(2.0));

        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let mut snapshots = HashMap::new();
        snapshots.insert("PF_XBTUSD".to_string(), snap);

        let batch = vec![
            OrderParams {
                symbol: "PF_XBTUSD".into(),
                side: PositionSide::Long,
                size: dec!(0.3),
                order_type: FuturesOrderType::Market,
                price: None,
                stop_price: None,
                trigger_signal: None,
                client_order_id: None,
                reduce_only: false,
                leverage: Some(dec!(2.0)),
                trailing_stop_max_deviation: None,
                trailing_stop_deviation_unit: None,
            },
            OrderParams {
                symbol: "PF_XBTUSD".into(),
                side: PositionSide::Long,
                size: dec!(100.0),
                order_type: FuturesOrderType::Market,
                price: None,
                stop_price: None,
                trigger_signal: None,
                client_order_id: None,
                reduce_only: false,
                leverage: Some(dec!(2.0)),
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
        let ot = order_type_from_cli("limit").unwrap();
        assert_eq!(ot, FuturesOrderType::Limit);
    }

    #[test]
    fn state_path_isolated_from_spot() {
        let futures_path = futures_paper_state_path().unwrap();
        let spot_path =
            crate::commands::paper::global_events_path(&crate::config::config_dir().unwrap());
        assert_ne!(futures_path, spot_path);
        assert!(futures_path.to_string_lossy().contains("futures_state"));
        assert!(spot_path.to_string_lossy().contains("journal.jsonl"));
    }

    #[test]
    fn fee_deducted_on_market_order() {
        let mut s = test_state();
        let initial = s.collateral;
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        let fee = result.fills[0].fee;
        assert!(fee > Decimal::ZERO);
        assert_eq!(s.collateral, (initial - fee));
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
        let depth = compute_executable_depth(&[], PositionSide::Long, dec!(50000.0), Decimal::ONE);
        assert_eq!(depth, Decimal::ZERO);
    }

    #[test]
    fn depth_uses_correct_side_for_buy() {
        let ask_levels = vec![(dec!(50000.0), dec!(0.5)), (dec!(50100.0), dec!(0.5))];
        let depth =
            compute_executable_depth(&ask_levels, PositionSide::Long, dec!(50100.0), dec!(2.0));
        assert_eq!(depth, Decimal::ONE);
    }

    #[test]
    fn depth_uses_correct_side_for_sell() {
        let bid_levels = vec![(dec!(49900.0), dec!(0.5)), (dec!(49800.0), dec!(0.5))];
        let depth =
            compute_executable_depth(&bid_levels, PositionSide::Short, dec!(49800.0), dec!(2.0));
        assert_eq!(depth, Decimal::ONE);
    }

    #[test]
    fn leverage_threads_to_new_position() {
        let mut s = test_state();
        s.leverage_preferences.insert("PF_XBTUSD".into(), dec!(5.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params, &snap).unwrap();
        assert_eq!(s.positions[0].leverage, dec!(10.0));
    }

    #[test]
    fn flip_uses_order_leverage_not_old_position() {
        let mut s = FuturesPaperState::new(dec!(100000.0), "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));

        let snap1 = market_snapshot(dec!(49900.0), dec!(50000.0));
        let p1 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Short,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(p1, &snap1).unwrap();

        let snap2 = market_snapshot(dec!(47900.0), dec!(48000.0));
        let p2 = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: dec!(3.0),
            order_type: FuturesOrderType::Market,
            price: None,
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(5.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(p2, &snap2).unwrap();

        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.positions[0].side, PositionSide::Long);
        assert_eq!(s.positions[0].leverage, dec!(5.0));
    }

    #[test]
    fn edit_order_recalculates_margin() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(48000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();
        let old_margin = s.open_orders[0].reserved_margin;

        s.edit_order(&result.order_id, Some(dec!(2.0)), None, None)
            .unwrap();
        let new_margin = s.open_orders[0].reserved_margin;
        assert!(new_margin > old_margin);
        assert_eq!(new_margin, dec!(2.0) * dec!(48000.0) / dec!(10.0));
    }

    #[test]
    fn edit_order_rejects_if_margin_insufficient() {
        let mut s = FuturesPaperState::new(dec!(5000.0), "USD", DEFAULT_FUTURES_TAKER_FEE_RATE);
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49900.0), dec!(50000.0));
        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: dec!(0.5),
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(48000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let result = s.place_order(params, &snap).unwrap();

        let edit_result = s.edit_order(&result.order_id, Some(dec!(100.0)), None, None);
        assert!(edit_result.is_err());
        assert_eq!(s.open_orders[0].size, dec!(0.5));
    }

    #[test]
    fn trigger_signal_differentiation() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));

        s.open_orders.push(FuturesPaperOrder {
            id: "STOP-MARK".into(),
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            filled_size: Decimal::ZERO,
            order_type: FuturesOrderType::Stop,
            price: None,
            stop_price: Some(dec!(51000.0)),
            trigger_signal: Some(TriggerSignal::Mark),
            client_order_id: None,
            reduce_only: false,
            leverage: dec!(10.0),
            reserved_margin: dec!(5100.0),
            status: OrderStatus::Open,
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
            trailing_anchor: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });

        let mut mark_prices = HashMap::new();
        mark_prices.insert("PF_XBTUSD".to_string(), dec!(49000.0));
        let mut last_prices = HashMap::new();
        last_prices.insert("PF_XBTUSD".to_string(), dec!(52000.0));

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

        mark_prices.insert("PF_XBTUSD".to_string(), dec!(51500.0));
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        s.positions.push(FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Short,
            size: Decimal::ONE,
            entry_price: dec!(50000.0),
            leverage: dec!(10.0),
            unrealized_funding: Decimal::ZERO,
            last_funding_time: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });

        s.open_orders.push(FuturesPaperOrder {
            id: "TS-1".into(),
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            filled_size: Decimal::ZERO,
            order_type: FuturesOrderType::TrailingStop,
            price: None,
            stop_price: Some(dec!(49000.0)),
            trigger_signal: Some(TriggerSignal::Last),
            client_order_id: None,
            reduce_only: false,
            leverage: dec!(10.0),
            reserved_margin: dec!(4900.0),
            status: OrderStatus::Open,
            trailing_stop_max_deviation: Some(dec!(500.0)),
            trailing_stop_deviation_unit: Some("quote_currency".into()),
            trailing_anchor: Some(dec!(49000.0)),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });

        let mut mark = HashMap::new();
        mark.insert("PF_XBTUSD".to_string(), dec!(48000.0));
        let mut last = HashMap::new();
        last.insert("PF_XBTUSD".to_string(), dec!(48000.0));

        s.reconcile(
            &mark,
            &last,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(s.open_orders.len(), 1);
        assert!(
            s.open_orders[0].trailing_anchor.unwrap() == dec!(48000.0),
            "Short trailing stop anchor should move down to 48000"
        );
    }

    #[test]
    fn reduce_only_cancelled_at_trigger_when_no_position() {
        let mut s = test_state();

        s.open_orders.push(FuturesPaperOrder {
            id: "STOP-RO".into(),
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            filled_size: Decimal::ZERO,
            order_type: FuturesOrderType::Stop,
            price: None,
            stop_price: Some(dec!(51000.0)),
            trigger_signal: Some(TriggerSignal::Last),
            client_order_id: None,
            reduce_only: true,
            leverage: dec!(10.0),
            reserved_margin: Decimal::ZERO,
            status: OrderStatus::Open,
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
            trailing_anchor: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });

        let mut mark = HashMap::new();
        mark.insert("PF_XBTUSD".to_string(), dec!(52000.0));
        let mut last = HashMap::new();
        last.insert("PF_XBTUSD".to_string(), dec!(52000.0));

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
        let past = chrono::Utc::now() - chrono::Duration::hours(9);
        s.positions.push(FuturesPaperPosition {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            entry_price: dec!(50000.0),
            leverage: dec!(10.0),
            unrealized_funding: Decimal::ZERO,
            last_funding_time: Some(past),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });

        let mark = HashMap::new();
        let last = HashMap::new();
        let mut funding = HashMap::new();
        funding.insert("PF_XBTUSD".to_string(), dec!(0.0001));

        let result = s.reconcile(&mark, &last, &HashMap::new(), &funding, &HashMap::new());
        assert_eq!(result.funding_events.len(), 1);
        assert!(result.funding_events[0].amount != Decimal::ZERO);
    }

    #[test]
    fn reconcile_fills_marketable_limit_buy() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49000.0), dec!(51000.0));

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(50500.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params, &snap).unwrap();
        assert_eq!(s.open_orders.len(), 1);
        assert!(s.positions.is_empty());

        let mark: HashMap<String, Decimal> = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);
        let last = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);
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
        assert_eq!(s.positions[0].entry_price, dec!(50500.0));
    }

    #[test]
    fn reconcile_skips_non_marketable_limit() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49000.0), dec!(51000.0));
        let mark: HashMap<String, Decimal> = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(45000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params, &snap).unwrap();

        let last = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49000.0), dec!(51000.0));

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Short,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(49800.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params, &snap).unwrap();

        let mark: HashMap<String, Decimal> = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);
        let last = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49000.0), dec!(51000.0));

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Limit,
            price: Some(dec!(55000.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_order(params, &snap).unwrap();

        let empty_last: HashMap<String, Decimal> = HashMap::new();
        let zero_mark: HashMap<String, Decimal> =
            HashMap::from([("PF_XBTUSD".into(), Decimal::ZERO)]);
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let snap = market_snapshot(dec!(49000.0), dec!(51000.0));

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Post,
            price: Some(dec!(50500.0)),
            stop_price: None,
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let collateral_before = s.collateral;
        s.place_order(params, &snap).unwrap();
        assert_eq!(s.open_orders.len(), 1);
        assert!(
            s.open_orders[0].reserved_margin > Decimal::ZERO,
            "Margin should be reserved on the order"
        );

        let last = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);
        let mark: HashMap<String, Decimal> = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);
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
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let mark = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::Stop,
            price: None,
            stop_price: Some(dec!(52000.0)),
            trigger_signal: Some(TriggerSignal::Index),
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        s.place_triggered_order(&params, dec!(10.0), &mark).unwrap();

        let last = HashMap::from([("PF_XBTUSD".into(), dec!(53000.0))]);
        let index_low = HashMap::from([("PF_XBTUSD".into(), dec!(51000.0))]);
        let result = s.reconcile(&mark, &last, &index_low, &HashMap::new(), &HashMap::new());
        assert!(
            result.fills.is_empty(),
            "Index=51000 < stop=52000, should not trigger"
        );
        assert_eq!(s.open_orders.len(), 1);

        let index_high = HashMap::from([("PF_XBTUSD".into(), dec!(53000.0))]);
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
        let mark = HashMap::from([("PF_XBTUSD".into(), Decimal::ZERO)]);
        let last = HashMap::from([("PF_XBTUSD".into(), Decimal::ZERO)]);
        let index = HashMap::from([("PF_XBTUSD".into(), Decimal::ZERO)]);

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
    fn trailing_stop_rejected_without_deviation() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let mark = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::TrailingStop,
            price: None,
            stop_price: Some(dec!(48000.0)),
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        let err = s
            .place_triggered_order(&params, dec!(10.0), &mark)
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("trailing-stop-max-deviation"),
            "Expected trailing-stop-max-deviation error, got: {msg}"
        );
    }

    #[test]
    fn trailing_stop_rejects_invalid_unit() {
        let mut s = test_state();
        s.leverage_preferences
            .insert("PF_XBTUSD".into(), dec!(10.0));
        let mark = HashMap::from([("PF_XBTUSD".into(), dec!(50000.0))]);

        let params = OrderParams {
            symbol: "PF_XBTUSD".into(),
            side: PositionSide::Long,
            size: Decimal::ONE,
            order_type: FuturesOrderType::TrailingStop,
            price: None,
            stop_price: Some(dec!(48000.0)),
            trigger_signal: None,
            client_order_id: None,
            reduce_only: false,
            leverage: Some(dec!(10.0)),
            trailing_stop_max_deviation: Some(dec!(500.0)),
            trailing_stop_deviation_unit: Some("percnt".into()),
        };
        let err = s
            .place_triggered_order(&params, dec!(10.0), &mark)
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("trailing-stop-deviation-unit"),
            "Expected trailing-stop-deviation-unit error, got: {msg}"
        );
    }

    #[test]
    fn leverage_rejects_above_max() {
        let s = test_state();
        let err = s.resolve_leverage(Some(dec!(300.0)), "PF_XBTUSD");
        assert!(err.is_err(), "Leverage > 200 should be rejected");
    }
}
