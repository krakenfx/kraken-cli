use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::errors::{KrakenError, Result};

pub(crate) const DEFAULT_FEE_RATE: f64 = 0.0026;

pub(crate) const DEFAULT_SLIPPAGE_RATE: f64 = 0.0;

#[derive(Debug, Clone)]
pub(crate) struct PaperConfig {
    pub(crate) balance: f64,
    pub(crate) currency: String,
    pub(crate) fee_rate: f64,
    pub(crate) slippage_rate: f64,
}

const KNOWN_QUOTES: &[&str] = &[
    "USDT", "USDC", "USD", "EUR", "GBP", "CAD", "AUD", "JPY", "CHF", "ETH", "BTC", "DAI",
];

const Z_FIAT_QUOTES: &[&str] = &["ZUSD", "ZEUR", "ZGBP", "ZCAD", "ZJPY", "ZAUD", "ZCHF"];

const CANON_MAP: &[(&str, &str)] = &[
    ("XBT", "BTC"),
    ("XXBT", "BTC"),
    ("XETH", "ETH"),
    ("XLTC", "LTC"),
    ("XXRP", "XRP"),
    ("XDOGE", "DOGE"),
    ("XXLM", "XLM"),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PaperState {
    pub(crate) balances: HashMap<String, f64>,
    pub(crate) reserved: HashMap<String, f64>,
    pub(crate) open_orders: Vec<PaperOrder>,
    pub(crate) filled_trades: Vec<PaperTrade>,
    #[serde(default = "default_starting_balance")]
    pub(crate) starting_balance: f64,
    #[serde(default = "default_starting_currency")]
    pub(crate) starting_currency: String,
    #[serde(default = "default_fee_rate")]
    pub(crate) fee_rate: f64,
    #[serde(default = "default_slippage_rate")]
    pub(crate) slippage_rate: f64,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    #[serde(default = "default_next_order_id")]
    next_order_id: u64,
    #[serde(default)]
    pub(crate) cancelled_orders: Vec<PaperOrder>,
}

fn default_next_order_id() -> u64 {
    1
}

fn default_fee_rate() -> f64 {
    DEFAULT_FEE_RATE
}

fn default_slippage_rate() -> f64 {
    DEFAULT_SLIPPAGE_RATE
}

fn default_starting_balance() -> f64 {
    10_000.0
}

fn default_starting_currency() -> String {
    "USD".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PaperOrder {
    pub(crate) id: String,
    pub(crate) pair: String,
    pub(crate) base: String,
    pub(crate) quote: String,
    pub(crate) side: OrderSide,
    pub(crate) volume: f64,
    pub(crate) price: f64,
    pub(crate) order_type: PaperOrderType,
    pub(crate) reserved_asset: String,
    pub(crate) reserved_amount: f64,
    pub(crate) created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PaperTrade {
    pub(crate) id: String,
    pub(crate) order_id: String,
    pub(crate) pair: String,
    pub(crate) base: String,
    pub(crate) quote: String,
    pub(crate) side: OrderSide,
    pub(crate) volume: f64,
    pub(crate) price: f64,
    pub(crate) fee: f64,
    pub(crate) cost: f64,
    pub(crate) filled_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum OrderSide {
    Buy,
    Sell,
}

impl std::fmt::Display for OrderSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Buy => f.write_str("Buy"),
            Self::Sell => f.write_str("Sell"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PaperOrderType {
    Market,
    Limit,
}

impl std::fmt::Display for PaperOrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Market => f.write_str("Market"),
            Self::Limit => f.write_str("Limit"),
        }
    }
}

impl Default for PaperState {
    fn default() -> Self {
        Self::new(10_000.0, "USD")
    }
}

impl PaperState {
    pub(crate) fn new(balance: f64, currency: &str) -> Self {
        Self::with_config(PaperConfig {
            balance,
            currency: currency.to_string(),
            fee_rate: DEFAULT_FEE_RATE,
            slippage_rate: DEFAULT_SLIPPAGE_RATE,
        })
    }

    pub(crate) fn with_config(config: PaperConfig) -> Self {
        let now = Utc::now().to_rfc3339();
        let cur = config.currency.to_uppercase();
        let mut balances = HashMap::new();
        balances.insert(cur.clone(), config.balance);
        Self {
            balances,
            reserved: HashMap::new(),
            open_orders: Vec::new(),
            filled_trades: Vec::new(),
            starting_balance: config.balance,
            starting_currency: cur,
            fee_rate: config.fee_rate,
            slippage_rate: config.slippage_rate,
            created_at: now.clone(),
            updated_at: now,
            next_order_id: 1,
            cancelled_orders: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn reset(&mut self) {
        *self = Self::with_config(PaperConfig {
            balance: self.starting_balance,
            currency: self.starting_currency.clone(),
            fee_rate: self.fee_rate,
            slippage_rate: self.slippage_rate,
        });
    }

    pub(crate) fn reset_with(
        &mut self,
        balance: Option<f64>,
        currency: Option<&str>,
        fee_rate: Option<f64>,
        slippage_rate: Option<f64>,
    ) {
        *self = Self::with_config(PaperConfig {
            balance: balance.unwrap_or(self.starting_balance),
            currency: currency
                .map(|c| c.to_uppercase())
                .unwrap_or_else(|| self.starting_currency.clone()),
            fee_rate: fee_rate.unwrap_or(self.fee_rate),
            slippage_rate: slippage_rate.unwrap_or(self.slippage_rate),
        });
    }

    pub(crate) fn available_balance(&self, asset: &str) -> f64 {
        let total = self.balances.get(asset).copied().unwrap_or(0.0);
        let reserved = self.reserved.get(asset).copied().unwrap_or(0.0);
        (total - reserved).max(0.0)
    }

    fn next_id(&mut self) -> String {
        let id = format!("PAPER-{:05}", self.next_order_id);
        self.next_order_id += 1;
        id
    }

    pub(crate) fn place_market_order(
        &mut self,
        side: OrderSide,
        pair: &str,
        volume: f64,
        ask: f64,
        bid: f64,
    ) -> Result<PaperTrade> {
        if !volume.is_finite() || volume <= 0.0 {
            return Err(KrakenError::Validation(
                "Volume must be a finite positive number".into(),
            ));
        }
        if !ask.is_finite() || ask <= 0.0 || !bid.is_finite() || bid <= 0.0 {
            return Err(KrakenError::Validation(
                "Ask and bid must be finite positive numbers".into(),
            ));
        }

        let (normalized, base, quote) = parse_pair(pair)?;

        match side {
            OrderSide::Buy => {
                let fill_price = ask * (1.0 + self.slippage_rate);
                let cost = volume * fill_price;
                let fee = cost * self.fee_rate;
                let total_cost = cost + fee;
                let available = self.available_balance(&quote);
                if available < total_cost {
                    return Err(KrakenError::Validation(format!(
                        "Insufficient {quote} balance. Available: {available:.2}, Required: {total_cost:.2}"
                    )));
                }
                *self.balances.entry(quote.clone()).or_insert(0.0) -= total_cost;
                *self.balances.entry(base.clone()).or_insert(0.0) += volume;
                let order_id = self.next_id();
                let trade_id = self.next_id();
                let trade = PaperTrade {
                    id: trade_id,
                    order_id,
                    pair: normalized,
                    base,
                    quote,
                    side,
                    volume,
                    price: fill_price,
                    fee,
                    cost,
                    filled_at: Utc::now().to_rfc3339(),
                };
                self.filled_trades.push(trade.clone());
                self.updated_at = Utc::now().to_rfc3339();
                Ok(trade)
            }
            OrderSide::Sell => {
                let available = self.available_balance(&base);
                if available < volume {
                    return Err(KrakenError::Validation(format!(
                        "Insufficient {base} balance. Available: {available:.8}, Required: {volume:.8}"
                    )));
                }
                let fill_price = bid * (1.0 - self.slippage_rate);
                let proceeds = volume * fill_price;
                let fee = proceeds * self.fee_rate;
                let net_proceeds = proceeds - fee;
                *self.balances.entry(base.clone()).or_insert(0.0) -= volume;
                *self.balances.entry(quote.clone()).or_insert(0.0) += net_proceeds;
                let order_id = self.next_id();
                let trade_id = self.next_id();
                let trade = PaperTrade {
                    id: trade_id,
                    order_id,
                    pair: normalized,
                    base,
                    quote,
                    side,
                    volume,
                    price: fill_price,
                    fee,
                    cost: proceeds,
                    filled_at: Utc::now().to_rfc3339(),
                };
                self.filled_trades.push(trade.clone());
                self.updated_at = Utc::now().to_rfc3339();
                Ok(trade)
            }
        }
    }

    pub(crate) fn place_limit_order(
        &mut self,
        side: OrderSide,
        pair: &str,
        volume: f64,
        price: f64,
    ) -> Result<String> {
        if !volume.is_finite() || volume <= 0.0 {
            return Err(KrakenError::Validation(
                "Volume must be a finite positive number".into(),
            ));
        }
        if !price.is_finite() || price <= 0.0 {
            return Err(KrakenError::Validation(
                "Price must be a finite positive number".into(),
            ));
        }

        let (normalized, base, quote) = parse_pair(pair)?;

        let (reserved_asset, reserved_amount) = match side {
            OrderSide::Buy => {
                let amount = volume * price * (1.0 + self.fee_rate);
                (quote.clone(), amount)
            }
            OrderSide::Sell => (base.clone(), volume),
        };

        let available = self.available_balance(&reserved_asset);
        if available < reserved_amount {
            return Err(KrakenError::Validation(format!(
                "Insufficient {reserved_asset} balance. Available: {available:.8}, Required: {reserved_amount:.8}"
            )));
        }

        *self.reserved.entry(reserved_asset.clone()).or_insert(0.0) += reserved_amount;

        let id = self.next_id();
        let order = PaperOrder {
            id: id.clone(),
            pair: normalized,
            base,
            quote,
            side,
            volume,
            price,
            order_type: PaperOrderType::Limit,
            reserved_asset,
            reserved_amount,
            created_at: Utc::now().to_rfc3339(),
        };
        self.open_orders.push(order);
        self.updated_at = Utc::now().to_rfc3339();
        Ok(id)
    }

    pub(crate) fn cancel_order(&mut self, order_id: &str) -> Result<PaperOrder> {
        let pos = self
            .open_orders
            .iter()
            .position(|o| o.id == order_id)
            .ok_or_else(|| {
                KrakenError::Validation(format!("Order {order_id} not found in open orders"))
            })?;

        let order = self.open_orders.remove(pos);
        self.release_reservation(&order);
        self.cancelled_orders.push(order.clone());
        self.updated_at = Utc::now().to_rfc3339();
        Ok(order)
    }

    pub(crate) fn cancel_all_orders(&mut self) -> Vec<PaperOrder> {
        let orders: Vec<PaperOrder> = self.open_orders.drain(..).collect();
        for order in &orders {
            self.release_reservation(order);
        }
        self.cancelled_orders.extend(orders.clone());
        self.updated_at = Utc::now().to_rfc3339();
        orders
    }

    pub(crate) fn check_pending_orders(
        &mut self,
        prices: &HashMap<String, (f64, f64)>,
    ) -> Vec<PaperTrade> {
        let mut fills = Vec::new();
        let mut i = 0;
        while i < self.open_orders.len() {
            let should_fill = {
                let order = &self.open_orders[i];
                if let Some(&(ask, bid)) = prices.get(&order.pair) {
                    match order.side {
                        OrderSide::Buy => ask <= order.price,
                        OrderSide::Sell => bid >= order.price,
                    }
                } else {
                    false
                }
            };

            if should_fill {
                let order = self.open_orders.remove(i);
                self.release_reservation(&order);

                let fill_price = order.price;
                match order.side {
                    OrderSide::Buy => {
                        let cost = order.volume * fill_price;
                        let fee = cost * self.fee_rate;
                        *self.balances.entry(order.quote.clone()).or_insert(0.0) -= cost + fee;
                        *self.balances.entry(order.base.clone()).or_insert(0.0) += order.volume;
                        let trade_id = self.next_id();
                        fills.push(PaperTrade {
                            id: trade_id,
                            order_id: order.id,
                            pair: order.pair,
                            base: order.base,
                            quote: order.quote,
                            side: order.side,
                            volume: order.volume,
                            price: fill_price,
                            fee,
                            cost,
                            filled_at: Utc::now().to_rfc3339(),
                        });
                    }
                    OrderSide::Sell => {
                        let proceeds = order.volume * fill_price;
                        let fee = proceeds * self.fee_rate;
                        *self.balances.entry(order.base.clone()).or_insert(0.0) -= order.volume;
                        *self.balances.entry(order.quote.clone()).or_insert(0.0) += proceeds - fee;
                        let trade_id = self.next_id();
                        fills.push(PaperTrade {
                            id: trade_id,
                            order_id: order.id,
                            pair: order.pair,
                            base: order.base,
                            quote: order.quote,
                            side: order.side,
                            volume: order.volume,
                            price: fill_price,
                            fee,
                            cost: proceeds,
                            filled_at: Utc::now().to_rfc3339(),
                        });
                    }
                }
            } else {
                i += 1;
            }
        }

        self.filled_trades.extend(fills.clone());
        if !fills.is_empty() {
            self.updated_at = Utc::now().to_rfc3339();
        }
        fills
    }

    pub(crate) fn compute_portfolio_value(
        &self,
        prices: &HashMap<String, (f64, f64)>,
    ) -> (f64, bool) {
        let mut total = 0.0;
        let mut complete = true;
        let sc = &self.starting_currency;
        for (asset, &amount) in &self.balances {
            if amount.abs() < 1e-12 {
                continue;
            }
            if amount < 0.0 {
                total += amount;
                continue;
            }
            if asset == sc {
                total += amount;
                continue;
            }
            let pair_key = format!("{asset}{sc}");
            if let Some(&(_ask, bid)) = prices.get(&pair_key) {
                total += amount * bid;
            } else {
                complete = false;
            }
        }
        (total, complete)
    }

    fn release_reservation(&mut self, order: &PaperOrder) {
        if let Some(reserved) = self.reserved.get_mut(&order.reserved_asset) {
            *reserved = (*reserved - order.reserved_amount).max(0.0);
        }
    }
}

fn canonicalize(symbol: &str) -> String {
    for &(from, to) in CANON_MAP {
        if symbol.eq_ignore_ascii_case(from) {
            return to.to_string();
        }
    }
    symbol.to_string()
}

fn normalize_kraken_pair(s: &str) -> String {
    for &zq in Z_FIAT_QUOTES {
        if let Some(pos) = s.rfind(zq) {
            if pos == 0 {
                continue;
            }
            let raw_base = &s[..pos];
            let raw_quote = &s[pos + 1..]; // strip the leading 'Z'
            let stripped_base = raw_base.strip_prefix('X').unwrap_or(raw_base);
            return format!("{stripped_base}{raw_quote}");
        }
    }
    s.to_string()
}

pub(crate) fn parse_pair(pair: &str) -> Result<(String, String, String)> {
    let upper = pair.to_uppercase();
    let api_pair = upper.replace('/', "");
    let extraction = normalize_kraken_pair(&api_pair);
    for &q in KNOWN_QUOTES {
        if extraction.ends_with(q) && extraction.len() > q.len() {
            let base_raw = &extraction[..extraction.len() - q.len()];
            let base = canonicalize(base_raw);
            let quote = canonicalize(q);
            return Ok((api_pair, base, quote));
        }
    }
    Err(KrakenError::Validation(format!(
        "Unknown pair '{pair}'. Use slash format (e.g., BTC/USD) for uncommon pairs."
    )))
}

pub(crate) fn paper_state_path() -> Result<PathBuf> {
    Ok(config::config_dir()?.join("paper").join("state.json"))
}

pub(crate) fn legacy_state_path() -> Result<PathBuf> {
    Ok(config::config_dir()?.join("paper.json"))
}

pub(crate) fn migrate_legacy_state() -> Result<bool> {
    let new_path = paper_state_path()?;
    if new_path.exists() {
        return Ok(false);
    }
    let legacy = legacy_state_path()?;
    if !legacy.exists() {
        return Ok(false);
    }
    if let Some(parent) = new_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&legacy, &new_path)?;
    Ok(true)
}

pub(crate) fn load_state() -> Result<PaperState> {
    migrate_legacy_state()?;
    let path = paper_state_path()?;
    if !path.exists() {
        return Err(KrakenError::Validation(
            "Paper account not initialized. Run 'kraken paper init' first.".into(),
        ));
    }
    let data = fs::read_to_string(&path)?;
    let state: PaperState = serde_json::from_str(&data)?;
    Ok(state)
}

pub(crate) fn save_state(state: &PaperState) -> Result<()> {
    let path = paper_state_path()?;
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

    #[test]
    fn test_new_state_custom_balance() {
        let state = PaperState::new(5000.0, "EUR");
        assert_eq!(state.balances.get("EUR"), Some(&5000.0));
        assert_eq!(state.starting_balance, 5000.0);
        assert_eq!(state.starting_currency, "EUR");
        assert!(state.open_orders.is_empty());
        assert!(state.filled_trades.is_empty());
    }

    #[test]
    fn test_new_state_default() {
        let state = PaperState::default();
        assert_eq!(state.balances.get("USD"), Some(&10000.0));
        assert_eq!(state.starting_balance, 10000.0);
        assert_eq!(state.starting_currency, "USD");
    }

    #[test]
    fn test_reset_restores_init_params() {
        let mut state = PaperState::new(5000.0, "EUR");
        state
            .place_market_order(OrderSide::Buy, "BTCEUR", 0.01, 50000.0, 49900.0)
            .unwrap();
        assert!(state.balances.contains_key("BTC"));
        assert!(!state.filled_trades.is_empty());

        state.reset();
        assert_eq!(state.balances.get("EUR"), Some(&5000.0));
        assert_eq!(state.starting_balance, 5000.0);
        assert_eq!(state.starting_currency, "EUR");
        assert!(state.open_orders.is_empty());
        assert!(state.filled_trades.is_empty());
        assert!(!state.balances.contains_key("BTC"));
    }

    #[test]
    fn test_available_balance_with_reservation() {
        let mut state = PaperState::new(10000.0, "USD");
        state.reserved.insert("USD".to_string(), 3000.0);
        assert!((state.available_balance("USD") - 7000.0).abs() < 1e-10);
    }

    #[test]
    fn test_market_buy() {
        let mut state = PaperState::new(10000.0, "USD");
        let trade = state
            .place_market_order(OrderSide::Buy, "BTCUSD", 0.1, 50000.0, 49900.0)
            .unwrap();
        assert_eq!(trade.side, OrderSide::Buy);
        assert_eq!(trade.volume, 0.1);
        assert_eq!(trade.price, 50000.0);
        let expected_cost = 0.1 * 50000.0;
        let expected_fee = expected_cost * DEFAULT_FEE_RATE;
        assert!((trade.fee - expected_fee).abs() < 1e-10);
        assert!((trade.cost - expected_cost).abs() < 1e-10);

        let usd = *state.balances.get("USD").unwrap();
        assert!((usd - (10000.0 - expected_cost - expected_fee)).abs() < 1e-10);
        assert_eq!(*state.balances.get("BTC").unwrap(), 0.1);
    }

    #[test]
    fn test_market_sell() {
        let mut state = PaperState::new(0.0, "USD");
        state.balances.insert("BTC".to_string(), 1.0);
        let trade = state
            .place_market_order(OrderSide::Sell, "BTCUSD", 0.5, 50000.0, 48000.0)
            .unwrap();
        assert_eq!(trade.side, OrderSide::Sell);
        assert_eq!(trade.volume, 0.5);
        assert_eq!(trade.price, 48000.0);
        let expected_proceeds = 0.5 * 48000.0;
        let expected_fee = expected_proceeds * DEFAULT_FEE_RATE;
        assert!((trade.fee - expected_fee).abs() < 1e-10);

        assert_eq!(*state.balances.get("BTC").unwrap(), 0.5);
        let usd = *state.balances.get("USD").unwrap();
        assert!((usd - (expected_proceeds - expected_fee)).abs() < 1e-10);
    }

    #[test]
    fn test_market_buy_insufficient_balance() {
        let mut state = PaperState::new(100.0, "USD");
        let result = state.place_market_order(OrderSide::Buy, "BTCUSD", 0.1, 50000.0, 49900.0);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Insufficient USD"));
        assert_eq!(*state.balances.get("USD").unwrap(), 100.0);
    }

    #[test]
    fn test_market_sell_insufficient_balance() {
        let mut state = PaperState::new(10000.0, "USD");
        let result = state.place_market_order(OrderSide::Sell, "BTCUSD", 0.1, 50000.0, 48000.0);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Insufficient BTC"));
    }

    #[test]
    fn test_fee_calculation() {
        let fee = 1.0 * 50_000.0 * DEFAULT_FEE_RATE;
        assert!((fee - 130.0).abs() < 1e-10);
    }

    #[test]
    fn market_buy_applies_slippage() {
        let mut state = PaperState::with_config(PaperConfig { balance: 10000.0, currency: "USD".into(), fee_rate: 0.0, slippage_rate: 0.001 });
        let trade = state
            .place_market_order(OrderSide::Buy, "BTCUSD", 0.1, 50000.0, 49900.0)
            .unwrap();
        // 50000 * 1.001 = 50050.0, volume 0.1 => cost 5005.0
        assert!((trade.price - 50050.0).abs() < 1e-6);
    }

    #[test]
    fn market_sell_applies_slippage() {
        let mut state = PaperState::with_config(PaperConfig { balance: 0.0, currency: "USD".into(), fee_rate: 0.0, slippage_rate: 0.001 });
        state.balances.insert("BTC".into(), 1.0);
        let trade = state
            .place_market_order(OrderSide::Sell, "BTCUSD", 1.0, 50000.0, 50000.0)
            .unwrap();
        // 50000 * 0.999 = 49950.0 — clean number, no precision issue
        assert!((trade.price - 49950.0).abs() < 1e-6);
    }

    #[test]
    fn zero_slippage_is_backward_compatible() {
        let mut state = PaperState::new(10000.0, "USD");
        let trade = state
            .place_market_order(OrderSide::Buy, "BTCUSD", 0.1, 50000.0, 49900.0)
            .unwrap();
        // no slippage, fill at exact ask
        assert!((trade.price - 50000.0).abs() < 1e-6);
    }

    #[test]
    fn reset_with_preserves_slippage() {
        let mut state = PaperState::with_config(PaperConfig { balance: 10000.0, currency: "USD".into(), fee_rate: 0.0026, slippage_rate: 0.001 });
        state.reset_with(Some(5000.0), None, None, None);
        assert!((state.slippage_rate - 0.001).abs() < 1e-10);
    }

    #[test]
    fn reset_with_updates_slippage() {
        let mut state = PaperState::with_config(PaperConfig { balance: 10000.0, currency: "USD".into(), fee_rate: 0.0026, slippage_rate: 0.001 });
        state.reset_with(None, None, None, Some(0.002));
        assert!((state.slippage_rate - 0.002).abs() < 1e-10);
    }

    #[test]
    fn test_limit_buy_reserves_quote() {
        let mut state = PaperState::new(10000.0, "USD");
        let id = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 45000.0)
            .unwrap();
        assert!(id.starts_with("PAPER-"));
        let expected_reserved = 0.1 * 45000.0 * (1.0 + DEFAULT_FEE_RATE);
        let reserved = state.reserved.get("USD").copied().unwrap_or(0.0);
        assert!((reserved - expected_reserved).abs() < 1e-10);
        assert!((state.available_balance("USD") - (10000.0 - expected_reserved)).abs() < 1e-10);
    }

    #[test]
    fn test_limit_sell_reserves_base() {
        let mut state = PaperState::new(10000.0, "USD");
        state.balances.insert("BTC".to_string(), 1.0);
        let id = state
            .place_limit_order(OrderSide::Sell, "BTCUSD", 0.5, 55000.0)
            .unwrap();
        assert!(id.starts_with("PAPER-"));
        let reserved = state.reserved.get("BTC").copied().unwrap_or(0.0);
        assert!((reserved - 0.5).abs() < 1e-10);
        assert!((state.available_balance("BTC") - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_limit_over_commit_rejected() {
        let mut state = PaperState::new(10000.0, "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 45000.0)
            .unwrap();
        let result = state.place_limit_order(OrderSide::Buy, "BTCUSD", 0.2, 45000.0);
        assert!(result.is_err());
        assert_eq!(state.open_orders.len(), 1);
    }

    #[test]
    fn test_cancel_order_releases_reservation() {
        let mut state = PaperState::new(10000.0, "USD");
        let id = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 45000.0)
            .unwrap();
        assert!(state.reserved.get("USD").copied().unwrap_or(0.0) > 0.0);

        state.cancel_order(&id).unwrap();
        assert!(state.open_orders.is_empty());
        assert!(state.reserved.get("USD").copied().unwrap_or(0.0) < 1e-10);
        assert!((state.available_balance("USD") - 10000.0).abs() < 1e-10);
    }

    #[test]
    fn test_cancel_nonexistent_order() {
        let mut state = PaperState::new(10000.0, "USD");
        let result = state.cancel_order("PAPER-99999");
        assert!(result.is_err());
    }

    #[test]
    fn test_cancel_all_releases_all() {
        let mut state = PaperState::new(10000.0, "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.01, 40000.0)
            .unwrap();
        state
            .place_limit_order(OrderSide::Buy, "ETHUSD", 0.1, 3000.0)
            .unwrap();
        assert_eq!(state.open_orders.len(), 2);

        let cancelled = state.cancel_all_orders();
        assert_eq!(cancelled.len(), 2);
        assert!(state.open_orders.is_empty());
        assert!((state.available_balance("USD") - 10000.0).abs() < 1e-10);
    }

    #[test]
    fn test_check_pending_buy_fills() {
        let mut state = PaperState::new(10000.0, "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 45000.0)
            .unwrap();
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (44500.0, 44400.0));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].price, 45000.0);
        assert!(state.open_orders.is_empty());
        assert!(*state.balances.get("BTC").unwrap() > 0.0);
    }

    #[test]
    fn test_check_pending_sell_fills() {
        let mut state = PaperState::new(10000.0, "USD");
        state.balances.insert("BTC".to_string(), 1.0);
        state
            .place_limit_order(OrderSide::Sell, "BTCUSD", 0.5, 55000.0)
            .unwrap();
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (55500.0, 55500.0));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].price, 55000.0);
        assert!(state.open_orders.is_empty());
    }

    #[test]
    fn test_check_pending_no_fill() {
        let mut state = PaperState::new(10000.0, "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 45000.0)
            .unwrap();
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (46000.0, 45900.0));
        let fills = state.check_pending_orders(&prices);
        assert!(fills.is_empty());
        assert_eq!(state.open_orders.len(), 1);
    }

    #[test]
    fn test_check_pending_fifo_order() {
        let mut state = PaperState::new(100000.0, "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 45000.0)
            .unwrap();
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.2, 44000.0)
            .unwrap();
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (43000.0, 42900.0));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 2);
        assert_eq!(fills[0].volume, 0.1);
        assert_eq!(fills[1].volume, 0.2);
    }

    #[test]
    fn test_reconcile_then_cancel() {
        let mut state = PaperState::new(100000.0, "USD");
        let _id_a = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 45000.0)
            .unwrap();
        let id_b = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 30000.0)
            .unwrap();

        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (44000.0, 43900.0));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 1);

        let cancelled = state.cancel_order(&id_b).unwrap();
        assert_eq!(cancelled.id, id_b);
        assert!(state.open_orders.is_empty());
    }

    #[test]
    fn test_reconcile_then_cancel_all() {
        let mut state = PaperState::new(100000.0, "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 45000.0)
            .unwrap();
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 30000.0)
            .unwrap();

        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (44000.0, 43900.0));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 1);
        assert_eq!(state.open_orders.len(), 1);

        let cancelled = state.cancel_all_orders();
        assert_eq!(cancelled.len(), 1);
        assert!(state.open_orders.is_empty());
    }

    #[test]
    fn test_parse_pair_standard() {
        let (pair, base, quote) = parse_pair("BTCUSD").unwrap();
        assert_eq!(pair, "BTCUSD");
        assert_eq!(base, "BTC");
        assert_eq!(quote, "USD");
    }

    #[test]
    fn test_parse_pair_slash() {
        let (pair, base, quote) = parse_pair("ETH/USD").unwrap();
        assert_eq!(pair, "ETHUSD");
        assert_eq!(base, "ETH");
        assert_eq!(quote, "USD");
    }

    #[test]
    fn test_parse_pair_lowercase() {
        let (pair, base, quote) = parse_pair("solusd").unwrap();
        assert_eq!(pair, "SOLUSD");
        assert_eq!(base, "SOL");
        assert_eq!(quote, "USD");
    }

    #[test]
    fn test_parse_pair_usdt() {
        let (pair, base, quote) = parse_pair("BTCUSDT").unwrap();
        assert_eq!(pair, "BTCUSDT");
        assert_eq!(base, "BTC");
        assert_eq!(quote, "USDT");
    }

    #[test]
    fn test_parse_pair_usdc_usd() {
        let (pair, base, quote) = parse_pair("USDCUSD").unwrap();
        assert_eq!(pair, "USDCUSD");
        assert_eq!(base, "USDC");
        assert_eq!(quote, "USD");
    }

    #[test]
    fn test_parse_pair_eth_quote() {
        let (pair, base, quote) = parse_pair("SOLETH").unwrap();
        assert_eq!(pair, "SOLETH");
        assert_eq!(base, "SOL");
        assert_eq!(quote, "ETH");
    }

    #[test]
    fn test_parse_pair_unknown() {
        let result = parse_pair("XYZABC");
        assert!(result.is_err());
    }

    #[test]
    fn test_order_id_format() {
        let mut state = PaperState::new(100000.0, "USD");
        let id1 = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.01, 40000.0)
            .unwrap();
        let id2 = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.01, 39000.0)
            .unwrap();
        assert_eq!(id1, "PAPER-00001");
        assert_eq!(id2, "PAPER-00002");
    }

    #[test]
    fn test_compute_portfolio_value_complete() {
        let mut state = PaperState::new(5000.0, "USD");
        state.balances.insert("BTC".to_string(), 0.1);
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (50100.0, 50000.0));
        let (value, complete) = state.compute_portfolio_value(&prices);
        assert!(complete);
        assert!((value - 10000.0).abs() < 1e-10);
    }

    #[test]
    fn test_compute_portfolio_value_partial() {
        let mut state = PaperState::new(5000.0, "USD");
        state.balances.insert("BTC".to_string(), 0.1);
        state.balances.insert("ETH".to_string(), 1.0);
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (50100.0, 50000.0));
        let (value, complete) = state.compute_portfolio_value(&prices);
        assert!(!complete);
        assert!((value - 10000.0).abs() < 1e-10);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paper.json");
        let state = PaperState::new(5000.0, "EUR");
        let data = serde_json::to_string_pretty(&state).unwrap();
        fs::write(&path, &data).unwrap();
        let loaded: PaperState = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.starting_balance, 5000.0);
        assert_eq!(loaded.starting_currency, "EUR");
        assert_eq!(loaded.balances.get("EUR"), Some(&5000.0));
    }

    #[test]
    fn test_state_serialization_no_secrets() {
        let state = PaperState::new(10000.0, "USD");
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("api_key"));
        assert!(!json.contains("api_secret"));
        assert!(!json.contains("password"));
    }

    #[test]
    fn test_paper_commands_no_auth_imports() {
        let source = include_str!("commands/paper.rs");
        assert!(
            !source.contains("SpotCredentials"),
            "paper.rs must not reference SpotCredentials"
        );
        assert!(
            !source.contains("private_post"),
            "paper.rs must not reference private_post"
        );
        assert!(
            !source.contains("build_spot_authed"),
            "paper.rs must not reference build_spot_authed"
        );
        assert!(
            !source.contains("resolve_spot_credentials"),
            "paper.rs must not reference resolve_spot_credentials"
        );
        assert!(
            !source.contains("AuthConfig"),
            "paper.rs must not reference AuthConfig"
        );
    }

    #[test]
    fn test_canonicalize_xbt_to_btc() {
        assert_eq!(canonicalize("XBT"), "BTC");
    }

    #[test]
    fn test_canonicalize_passthrough() {
        assert_eq!(canonicalize("SOL"), "SOL");
    }

    #[test]
    fn test_normalize_kraken_pair_xxbtzusd() {
        assert_eq!(normalize_kraken_pair("XXBTZUSD"), "XBTUSD");
    }

    #[test]
    fn test_normalize_kraken_pair_xethzeur() {
        assert_eq!(normalize_kraken_pair("XETHZEUR"), "ETHEUR");
    }

    #[test]
    fn test_normalize_kraken_pair_passthrough() {
        assert_eq!(normalize_kraken_pair("BTCUSD"), "BTCUSD");
    }

    #[test]
    fn test_parse_pair_xbt_yields_btc() {
        let (api, base, quote) = parse_pair("XBTUSD").unwrap();
        assert_eq!(api, "XBTUSD");
        assert_eq!(base, "BTC");
        assert_eq!(quote, "USD");
    }

    #[test]
    fn test_parse_pair_xxbtzusd() {
        let (api, base, quote) = parse_pair("XXBTZUSD").unwrap();
        assert_eq!(api, "XXBTZUSD");
        assert_eq!(base, "BTC");
        assert_eq!(quote, "USD");
    }

    #[test]
    fn test_parse_pair_xethzeur() {
        let (api, base, quote) = parse_pair("XETHZEUR").unwrap();
        assert_eq!(api, "XETHZEUR");
        assert_eq!(base, "ETH");
        assert_eq!(quote, "EUR");
    }

    #[test]
    fn test_parse_pair_xxrpzusd() {
        let (api, base, quote) = parse_pair("XXRPZUSD").unwrap();
        assert_eq!(api, "XXRPZUSD");
        assert_eq!(base, "XRP");
        assert_eq!(quote, "USD");
    }

    #[test]
    fn test_canonicalization_buy_sell_same_key() {
        let mut state = PaperState::new(100000.0, "USD");
        state
            .place_market_order(OrderSide::Buy, "BTCUSD", 0.1, 50000.0, 49900.0)
            .unwrap();
        assert!(state.balances.contains_key("BTC"));
        assert!(!state.balances.contains_key("XBT"));

        state
            .place_market_order(OrderSide::Sell, "XBTUSD", 0.05, 50000.0, 49900.0)
            .unwrap();
        assert!(state.balances.contains_key("BTC"));
        assert!(!state.balances.contains_key("XBT"));
    }

    #[test]
    fn test_canonicalization_xxbt_buy_btc_sell() {
        let mut state = PaperState::new(100000.0, "USD");
        state
            .place_market_order(OrderSide::Buy, "XXBTZUSD", 0.1, 50000.0, 49900.0)
            .unwrap();
        assert!(state.balances.contains_key("BTC"));
        assert!(!state.balances.contains_key("XBT"));
        assert!(!state.balances.contains_key("XXBT"));

        let btc_before = *state.balances.get("BTC").unwrap();
        state
            .place_market_order(OrderSide::Sell, "BTCUSD", 0.05, 50000.0, 49900.0)
            .unwrap();
        let btc_after = *state.balances.get("BTC").unwrap();
        assert!((btc_before - btc_after - 0.05).abs() < 1e-10);
    }

    #[test]
    fn test_canonicalization_slash_and_xbt() {
        let (_, base1, _) = parse_pair("BTC/USD").unwrap();
        let (_, base2, _) = parse_pair("XBTUSD").unwrap();
        assert_eq!(base1, "BTC");
        assert_eq!(base2, "BTC");
    }

    #[test]
    fn test_check_pending_empty_prices_no_fill() {
        let mut state = PaperState::new(100000.0, "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 45000.0)
            .unwrap();
        let prices: HashMap<String, (f64, f64)> = HashMap::new();
        let fills = state.check_pending_orders(&prices);
        assert!(fills.is_empty());
        assert_eq!(state.open_orders.len(), 1);
    }

    #[test]
    fn test_state_path_in_paper_subdir() {
        let path = paper_state_path().unwrap();
        let components: Vec<_> = path.components().collect();
        let names: Vec<&str> = components
            .iter()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();
        assert!(
            names
                .windows(2)
                .any(|w| w[0] == "paper" && w[1] == "state.json"),
            "Path must contain paper/state.json, got: {path:?}"
        );
    }

    #[test]
    fn test_migration_legacy_to_new() {
        let dir = tempfile::tempdir().unwrap();
        let kraken_dir = dir.path().join("kraken");
        fs::create_dir_all(&kraken_dir).unwrap();

        let state = PaperState::new(7500.0, "EUR");
        let data = serde_json::to_string_pretty(&state).unwrap();
        let legacy_path = kraken_dir.join("paper.json");
        fs::write(&legacy_path, &data).unwrap();

        let new_dir = kraken_dir.join("paper");
        let new_path = new_dir.join("state.json");
        assert!(!new_path.exists());

        fs::create_dir_all(&new_dir).unwrap();
        fs::rename(&legacy_path, &new_path).unwrap();

        assert!(new_path.exists());
        assert!(!legacy_path.exists());
        let loaded: PaperState =
            serde_json::from_str(&fs::read_to_string(&new_path).unwrap()).unwrap();
        assert_eq!(loaded.starting_balance, 7500.0);
        assert_eq!(loaded.starting_currency, "EUR");
    }

    #[test]
    fn test_normalize_kraken_pair_xxrpzusd() {
        assert_eq!(normalize_kraken_pair("XXRPZUSD"), "XRPUSD");
    }

    #[test]
    fn test_normalize_kraken_pair_zchf() {
        assert_eq!(normalize_kraken_pair("XXBTZCHF"), "XBTCHF");
    }

    #[test]
    fn test_parse_pair_btcusdt() {
        let (api, base, quote) = parse_pair("BTCUSDT").unwrap();
        assert_eq!(api, "BTCUSDT");
        assert_eq!(base, "BTC");
        assert_eq!(quote, "USDT");
    }

    #[test]
    fn test_market_order_zero_volume_rejected() {
        let mut state = PaperState::new(10000.0, "USD");
        let result = state.place_market_order(OrderSide::Buy, "BTCUSD", 0.0, 50000.0, 49900.0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("positive"));
    }

    #[test]
    fn test_market_order_negative_volume_rejected() {
        let mut state = PaperState::new(10000.0, "USD");
        let result = state.place_market_order(OrderSide::Buy, "BTCUSD", -1.0, 50000.0, 49900.0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("positive"));
    }

    #[test]
    fn test_limit_order_zero_volume_rejected() {
        let mut state = PaperState::new(10000.0, "USD");
        let result = state.place_limit_order(OrderSide::Buy, "BTCUSD", 0.0, 50000.0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("positive"));
    }

    #[test]
    fn test_limit_order_zero_price_rejected() {
        let mut state = PaperState::new(10000.0, "USD");
        let result = state.place_limit_order(OrderSide::Buy, "BTCUSD", 0.1, 0.0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("positive"));
    }

    #[test]
    fn test_parse_pair_alias_mapping_for_ticker() {
        let (_, base1, quote1) = parse_pair("BTCUSD").unwrap();
        let (_, base2, quote2) = parse_pair("XXBTZUSD").unwrap();
        assert_eq!(
            base1, base2,
            "BTCUSD and XXBTZUSD must canonicalize to same base"
        );
        assert_eq!(
            quote1, quote2,
            "BTCUSD and XXBTZUSD must canonicalize to same quote"
        );
        assert_eq!(base1, "BTC");
        assert_eq!(quote1, "USD");

        let (_, base3, quote3) = parse_pair("ETHUSD").unwrap();
        let (_, base4, quote4) = parse_pair("XETHZUSD").unwrap();
        assert_eq!(
            base3, base4,
            "ETHUSD and XETHZUSD must canonicalize to same base"
        );
        assert_eq!(
            quote3, quote4,
            "ETHUSD and XETHZUSD must canonicalize to same quote"
        );
        assert_eq!(base3, "ETH");
        assert_eq!(quote3, "USD");
    }

    #[test]
    fn test_status_degraded_valuation_components() {
        let mut state = PaperState::new(5000.0, "USD");
        state.balances.insert("BTC".to_string(), 0.1);
        let empty_prices: HashMap<String, (f64, f64)> = HashMap::new();
        let (value, complete) = state.compute_portfolio_value(&empty_prices);
        assert!(
            !complete,
            "valuation must be incomplete when prices are missing"
        );
        assert!(
            (value - 5000.0).abs() < 1e-10,
            "value must be quote-currency only"
        );
    }

    #[test]
    fn market_order_rejects_nan_volume() {
        let mut state = PaperState::new(10000.0, "USD");
        let err = state.place_market_order(OrderSide::Buy, "BTCUSD", f64::NAN, 50000.0, 49990.0);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("finite"));
    }

    #[test]
    fn market_order_rejects_infinity_volume() {
        let mut state = PaperState::new(10000.0, "USD");
        let err =
            state.place_market_order(OrderSide::Buy, "BTCUSD", f64::INFINITY, 50000.0, 49990.0);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("finite"));
    }

    #[test]
    fn market_order_rejects_nan_ask() {
        let mut state = PaperState::new(10000.0, "USD");
        let err = state.place_market_order(OrderSide::Buy, "BTCUSD", 0.001, f64::NAN, 49990.0);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("finite"));
    }

    #[test]
    fn market_order_rejects_zero_bid() {
        let mut state = PaperState::new(10000.0, "USD");
        state.balances.insert("BTC".into(), 1.0);
        let err = state.place_market_order(OrderSide::Sell, "BTCUSD", 0.001, 50000.0, 0.0);
        assert!(err.is_err());
    }

    #[test]
    fn limit_order_rejects_nan_volume() {
        let mut state = PaperState::new(10000.0, "USD");
        let err = state.place_limit_order(OrderSide::Buy, "BTCUSD", f64::NAN, 50000.0);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("finite"));
    }

    #[test]
    fn limit_order_rejects_nan_price() {
        let mut state = PaperState::new(10000.0, "USD");
        let err = state.place_limit_order(OrderSide::Buy, "BTCUSD", 0.001, f64::NAN);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("finite"));
    }

    #[test]
    fn limit_order_rejects_infinity_price() {
        let mut state = PaperState::new(10000.0, "USD");
        let err = state.place_limit_order(OrderSide::Buy, "BTCUSD", 0.001, f64::INFINITY);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("finite"));
    }

    #[test]
    fn deserialize_state_missing_next_order_id() {
        let json = r#"{
            "balances": {"USD": 10000.0},
            "reserved": {},
            "open_orders": [],
            "filled_trades": [],
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;
        let state: PaperState =
            serde_json::from_str(json).expect("must deserialize without next_order_id");
        assert_eq!(state.next_order_id, 1);
    }

    #[test]
    fn deserialize_state_missing_slippage_rate_defaults_to_zero() {
        let json = r#"{
            "balances": {"USD": 10000.0},
            "reserved": {},
            "open_orders": [],
            "filled_trades": [],
            "fee_rate": 0.0026,
            "starting_balance": 10000.0,
            "starting_currency": "USD",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;
        let state: PaperState =
            serde_json::from_str(json).expect("must deserialize without slippage_rate");
        assert_eq!(state.slippage_rate, 0.0);
    }
}
