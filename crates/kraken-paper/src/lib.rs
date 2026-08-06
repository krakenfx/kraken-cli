#![doc = include_str!("../README.md")]

pub mod account;

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
pub use kraken_core::OrderSide;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

pub type Result<T, E = PaperError> = std::result::Result<T, E>;

/// The engine's failure modes. The binary maps each variant onto the JSON
/// envelope exactly once, in its `errors` module.
#[derive(Debug, thiserror::Error)]
pub enum PaperError {
    /// An order, config, or account state the engine refuses up front.
    /// Message text is a pinned agent-retry contract — user-facing sentences
    /// verbatim, never re-cased. → envelope `validation`.
    #[error("{0}")]
    Rejected(String),
    /// A journal written by a newer build (record v > `RECORD_VERSION`).
    /// → envelope `config`.
    #[error("{0}")]
    Incompatible(String),
    /// The journal store failed; the envelope mapping delegates to the
    /// recording bridge (Rejected→validation, Damaged→parse, Io/Engine→io).
    #[error(transparent)]
    Journal(#[from] kraken_recording::Error),
}

/// `remove_at`'s `std::fs::exists` is the one raw-io site; route it through
/// [`PaperError::Journal`] so `?` keeps working and the category stays `io`.
impl From<std::io::Error> for PaperError {
    fn from(e: std::io::Error) -> Self {
        Self::Journal(e.into())
    }
}

pub const DEFAULT_FEE_RATE: Decimal = dec!(0.0026);

pub const DEFAULT_SLIPPAGE_RATE: Decimal = Decimal::ZERO;

/// Paper order/trade ids are `PAPER-{seq:05}`; the fold re-derives the id
/// counter by parsing this prefix back off folded ids.
const ORDER_ID_PREFIX: &str = "PAPER-";

/// Ignores floating-point residue from journals written by older builds.
pub const BALANCE_DUST: Decimal = dec!(0.000000000001);

/// Caps operands so `Decimal` multiplication cannot overflow mid-command.
pub const MAX_MONEY: Decimal = dec!(1_000_000_000_000);

/// Reject a user-supplied money value outside `(0, MAX_MONEY]` — the one
/// gate that keeps engine arithmetic panic-free (see [`MAX_MONEY`]).
pub fn validate_money(value: Decimal, what: &str) -> Result<()> {
    if value <= Decimal::ZERO {
        return Err(PaperError::Rejected(format!(
            "{what} must be a positive number"
        )));
    }
    if value > MAX_MONEY {
        return Err(PaperError::Rejected(format!(
            "{what} must be at most {MAX_MONEY}"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperConfig {
    #[serde(with = "rust_decimal::serde::str")]
    pub balance: Decimal,
    pub currency: String,
    #[serde(default = "default_fee_rate", with = "rust_decimal::serde::str")]
    pub fee_rate: Decimal,
    #[serde(default = "default_slippage_rate", with = "rust_decimal::serde::str")]
    pub slippage_rate: Decimal,
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
pub struct PaperState {
    pub balances: HashMap<String, Decimal>,
    pub reserved: HashMap<String, Decimal>,
    pub open_orders: Vec<PaperOrder>,
    pub filled_trades: Vec<PaperTrade>,
    #[serde(default = "default_starting_balance")]
    pub starting_balance: Decimal,
    #[serde(default = "default_starting_currency")]
    pub starting_currency: String,
    #[serde(default = "default_fee_rate")]
    pub fee_rate: Decimal,
    #[serde(default = "default_slippage_rate")]
    pub slippage_rate: Decimal,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default = "default_next_order_id")]
    next_order_id: u64,
    #[serde(default)]
    pub cancelled_orders: Vec<PaperOrder>,
}

fn default_next_order_id() -> u64 {
    1
}

fn default_fee_rate() -> Decimal {
    DEFAULT_FEE_RATE
}

fn default_slippage_rate() -> Decimal {
    DEFAULT_SLIPPAGE_RATE
}

fn default_starting_balance() -> Decimal {
    dec!(10_000)
}

fn default_starting_currency() -> String {
    "USD".to_string()
}

/// A portfolio marked against one price snapshot. `unmarked` names every
/// held asset the snapshot could not price (sorted, so serialized output is
/// deterministic); completeness is derived, never stored alongside.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Valuation {
    pub value: Decimal,
    pub unmarked: BTreeSet<String>,
}

impl Valuation {
    pub fn is_complete(&self) -> bool {
        self.unmarked.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperOrder {
    pub id: String,
    pub pair: String,
    pub base: String,
    pub quote: String,
    pub side: OrderSide,
    #[serde(with = "rust_decimal::serde::str")]
    pub volume: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    pub order_type: PaperOrderType,
    pub reserved_asset: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub reserved_amount: Decimal,
    pub created_at: DateTime<Utc>,
}

impl PaperOrder {
    /// Whether `ask`/`bid` would fill this resting order — the engine's one
    /// crossing rule, shared with explain-pnl's recorded-stream
    /// counterfactual so the two can never drift.
    pub fn crosses(&self, ask: Decimal, bid: Decimal) -> bool {
        match self.side {
            OrderSide::Buy => ask <= self.price,
            OrderSide::Sell => bid >= self.price,
        }
    }
}

/// Money rides the digit-exact string wire wherever this type serializes —
/// see [`AccountEvent`] for why the journal payloads carry it everywhere.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperTrade {
    pub id: String,
    pub order_id: String,
    pub pair: String,
    pub base: String,
    pub quote: String,
    pub side: OrderSide,
    #[serde(with = "rust_decimal::serde::str")]
    pub volume: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub fee: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub cost: Decimal,
    pub filled_at: DateTime<Utc>,
    /// `None`: legacy trade, or no valid quote at fill time.
    pub reference_quote: Option<ReferenceQuote>,
}

/// Raw top-of-book at fill time, pre-slippage. For limit fills: the quote at
/// fill *detection*, which can lag the moment the market first crossed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ReferenceQuote {
    #[serde(with = "rust_decimal::serde::str")]
    pub ask: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub bid: Decimal,
}

impl ReferenceQuote {
    /// `None` unless both sides are positive: a zero or negative top-of-book
    /// is a broken frame, not a market.
    pub fn checked(ask: Decimal, bid: Decimal) -> Option<Self> {
        (ask > Decimal::ZERO && bid > Decimal::ZERO).then_some(Self { ask, bid })
    }

    pub fn mid(&self) -> Decimal {
        (self.ask + self.bid) / dec!(2)
    }
}

/// One event in an account's log (`events.jsonl`) — the account's single
/// source of truth: state is a fold of these via [`PaperState::apply`].
///
/// State-changing events record *outputs* (ids, prices, fees), not formulas,
/// so replaying an old log stays exact even after engine rules change.
/// `Command` and `OrderRejected` are observational: the fold ignores them.
///
/// Money in the state-changing payloads serializes as digit-exact strings
/// (`rust_decimal::serde::str`), not the workspace's f64 number wire — the
/// replay-stays-exact invariant needs every folded `Decimal` to round-trip
/// bit-for-bit. The attribute rides the payload types into every other
/// context they serialize in (state snapshots, lab fill ledgers), not only
/// this log. Observational entries keep the number wire: nothing folds
/// them, and their figures are display-grade echoes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AccountEvent {
    Initialized(PaperConfig),
    Reset(PaperConfig),
    OrderSubmitted {
        order: PaperOrder,
    },
    OrderFilled {
        trade: PaperTrade,
    },
    OrderCancelled {
        order: PaperOrder,
    },
    Command(CommandEntry),
    OrderRejected(OrderRejection),
    /// A live journal's anchor: the venue snapshot observed when the
    /// mirror first attached. Initializes the account like an epoch.
    Attached(VenueSnapshot),
    /// A venue snapshot closing a reconciliation window: the mirror
    /// adopts the venue's balances — the venue is the truth in live mode.
    Reconciled(VenueSnapshot),
    /// An event kind this build doesn't know — written by a newer build.
    #[serde(other)]
    Unknown,
}

/// The fixed reference a scoring window opens against: the
/// account's equity, currency, and cost rates at window-open, captured once
/// and never recomputed so the window's P&L and cost decomposition can't drift
/// if the account changes later. Carried as one unit because the four are only
/// ever meaningful together — the `Initialized`/`Reset` that set the rates and
/// the funding that set the equity both lie *before* the window, so the
/// window-opening snapshot is the only place to recover them.
#[serde_with::serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowAnchor {
    #[serde_as(as = "serde_with::DisplayFromStr")]
    pub equity: Decimal,
    pub currency: String,
    #[serde_as(as = "serde_with::DisplayFromStr")]
    pub fee_rate: Decimal,
    #[serde_as(as = "serde_with::DisplayFromStr")]
    pub slippage_rate: Decimal,
}

/// Per-asset balances as the venue reported them. Digit-exact on the wire
/// (string amounts) like every journaled money value, in stable key order;
/// `complete: false` discloses a snapshot the venue fetch could not fill —
/// never guessed.
#[serde_with::serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VenueSnapshot {
    #[serde_as(as = "std::collections::BTreeMap<_, serde_with::DisplayFromStr>")]
    pub balances: std::collections::BTreeMap<String, Decimal>,
    pub complete: bool,
    /// `Some` when this snapshot opens a scoring window; `None` for a
    /// live-venue mirror, which observes balances and has no paper anchor
    /// (a real account has a fee *tier*, not a scalar rate). One `Option`
    /// keeps the four anchor fields all-present-or-all-absent, so an
    /// equity anchor can never ship without its rates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<WindowAnchor>,
}

/// Audit record of one session-scoped command: allowlisted typed args, the
/// outcome, and post-command balances. The closed field set keeps free-form
/// prose (`--reason`) out of the log; identifiers the user typed (`pair`,
/// `order_id`) are recorded verbatim.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommandEntry {
    pub name: String,
    pub pair: Option<String>,
    pub side: Option<OrderSide>,
    pub volume: Option<Decimal>,
    pub order_type: Option<String>,
    pub price: Option<Decimal>,
    pub order_id: Option<String>,
    pub balance: Option<Decimal>,
    pub currency: Option<String>,
    pub fee_rate: Option<Decimal>,
    pub slippage_rate: Option<Decimal>,
    /// Ids the command produced, or the stable error category it failed with.
    pub outcome: CommandOutcome,
    /// Balances after the command (unchanged balances on failure).
    pub balances: Option<HashMap<String, Decimal>>,
    /// Run-window marker: set ONLY by the `run start` / `run
    /// stop` entries that cut a window into the journal. Every trading
    /// command leaves it `None` — trades attribute to runs by position
    /// between markers, never by stamping.
    pub session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CommandOutcome {
    Ok {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        order_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        trade_ids: Vec<String>,
    },
    Err {
        category: String,
    },
}

impl Default for CommandOutcome {
    fn default() -> Self {
        Self::Ok {
            order_ids: Vec::new(),
            trade_ids: Vec::new(),
        }
    }
}

/// An order the engine refused — the only durable trace a rejection leaves.
/// Carries the request shape and the error *category*, never the message
/// (engine messages embed balance figures as free text).
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRejection {
    pub side: OrderSide,
    pub pair: String,
    pub volume: Decimal,
    pub price: Option<Decimal>,
    pub category: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperOrderType {
    Market,
    Limit,
}

impl std::fmt::Display for PaperOrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Market => f.write_str("market"),
            Self::Limit => f.write_str("limit"),
        }
    }
}

impl Default for PaperState {
    fn default() -> Self {
        Self::new(dec!(10_000), "USD")
    }
}

impl PaperState {
    pub fn new(balance: Decimal, currency: &str) -> Self {
        Self::with_config(PaperConfig {
            balance,
            currency: currency.to_string(),
            fee_rate: DEFAULT_FEE_RATE,
            slippage_rate: DEFAULT_SLIPPAGE_RATE,
        })
    }

    pub fn with_config(config: PaperConfig) -> Self {
        let now = Utc::now();
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
            created_at: now,
            updated_at: now,
            next_order_id: 1,
            cancelled_orders: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn reset(&mut self) {
        let config = self.reset_config(None, None, None, None);
        self.apply(Utc::now(), &AccountEvent::Reset(config));
    }

    /// The config a reset would start the next epoch with: explicit overrides
    /// where given, the current account's parameters otherwise.
    pub fn reset_config(
        &self,
        balance: Option<Decimal>,
        currency: Option<&str>,
        fee_rate: Option<Decimal>,
        slippage_rate: Option<Decimal>,
    ) -> PaperConfig {
        PaperConfig {
            balance: balance.unwrap_or(self.starting_balance),
            currency: currency
                .map(|c| c.to_uppercase())
                .unwrap_or_else(|| self.starting_currency.clone()),
            fee_rate: fee_rate.unwrap_or(self.fee_rate),
            slippage_rate: slippage_rate.unwrap_or(self.slippage_rate),
        }
    }

    #[cfg(test)]
    pub fn reset_with(
        &mut self,
        balance: Option<Decimal>,
        currency: Option<&str>,
        fee_rate: Option<Decimal>,
        slippage_rate: Option<Decimal>,
    ) {
        let config = self.reset_config(balance, currency, fee_rate, slippage_rate);
        self.apply(Utc::now(), &AccountEvent::Reset(config));
    }

    pub fn available_balance(&self, asset: &str) -> Decimal {
        let total = self.balances.get(asset).copied().unwrap_or(Decimal::ZERO);
        let reserved = self.reserved.get(asset).copied().unwrap_or(Decimal::ZERO);
        (total - reserved).max(Decimal::ZERO)
    }

    /// Business rules for a market order: validate, compute the fill.
    /// Mutates nothing — the returned trade becomes an [`AccountEvent::OrderFilled`]
    /// that [`Self::apply`] folds into state.
    pub fn decide_market_order(
        &self,
        side: OrderSide,
        pair: &str,
        volume: Decimal,
        ask: Decimal,
        bid: Decimal,
    ) -> Result<PaperTrade> {
        validate_money(volume, "Volume")?;
        validate_money(ask, "Ask")?;
        validate_money(bid, "Bid")?;

        let (normalized, base, quote) = parse_pair(pair)?;

        let (fill_price, cost, fee) = match side {
            OrderSide::Buy => {
                let fill_price = ask * (Decimal::ONE + self.slippage_rate);
                let cost = volume * fill_price;
                let fee = cost * self.fee_rate;
                let total_cost = cost + fee;
                let available = self.available_balance(&quote);
                if available < total_cost {
                    return Err(PaperError::Rejected(format!(
                        "Insufficient {quote} balance. Available: {available:.2}, Required: {total_cost:.2}"
                    )));
                }
                (fill_price, cost, fee)
            }
            OrderSide::Sell => {
                let available = self.available_balance(&base);
                if available < volume {
                    return Err(PaperError::Rejected(format!(
                        "Insufficient {base} balance. Available: {available:.8}, Required: {volume:.8}"
                    )));
                }
                let fill_price = bid * (Decimal::ONE - self.slippage_rate);
                let proceeds = volume * fill_price;
                let fee = proceeds * self.fee_rate;
                (fill_price, proceeds, fee)
            }
        };

        Ok(PaperTrade {
            id: self.peek_order_id(1),
            order_id: self.peek_order_id(0),
            pair: normalized,
            base,
            quote,
            side,
            volume,
            price: fill_price,
            fee,
            cost,
            filled_at: Utc::now(),
            reference_quote: Some(ReferenceQuote { ask, bid }),
        })
    }

    #[cfg(test)]
    pub fn place_market_order(
        &mut self,
        side: OrderSide,
        pair: &str,
        volume: Decimal,
        ask: Decimal,
        bid: Decimal,
    ) -> Result<PaperTrade> {
        let trade = self.decide_market_order(side, pair, volume, ask, bid)?;
        self.apply(
            trade.filled_at,
            &AccountEvent::OrderFilled {
                trade: trade.clone(),
            },
        );
        Ok(trade)
    }

    /// Business rules for a limit placement: validate, size the reservation.
    /// Mutates nothing — the returned order becomes an
    /// [`AccountEvent::OrderSubmitted`] that [`Self::apply`] folds into state.
    pub fn decide_limit_order(
        &self,
        side: OrderSide,
        pair: &str,
        volume: Decimal,
        price: Decimal,
    ) -> Result<PaperOrder> {
        validate_money(volume, "Volume")?;
        validate_money(price, "Price")?;

        let (normalized, base, quote) = parse_pair(pair)?;

        let (reserved_asset, reserved_amount) = match side {
            OrderSide::Buy => {
                let amount = volume * price * (Decimal::ONE + self.fee_rate);
                (quote.clone(), amount)
            }
            OrderSide::Sell => (base.clone(), volume),
        };

        let available = self.available_balance(&reserved_asset);
        if available < reserved_amount {
            return Err(PaperError::Rejected(format!(
                "Insufficient {reserved_asset} balance. Available: {available:.8}, Required: {reserved_amount:.8}"
            )));
        }

        Ok(PaperOrder {
            id: self.peek_order_id(0),
            pair: normalized,
            base,
            quote,
            side,
            volume,
            price,
            order_type: PaperOrderType::Limit,
            reserved_asset,
            reserved_amount,
            created_at: Utc::now(),
        })
    }

    #[cfg(test)]
    pub fn place_limit_order(
        &mut self,
        side: OrderSide,
        pair: &str,
        volume: Decimal,
        price: Decimal,
    ) -> Result<String> {
        let order = self.decide_limit_order(side, pair, volume, price)?;
        let id = order.id.clone();
        self.apply(order.created_at, &AccountEvent::OrderSubmitted { order });
        Ok(id)
    }

    /// The open order a cancel would remove, or a validation error.
    pub fn decide_cancel(&self, order_id: &str) -> Result<PaperOrder> {
        self.open_orders
            .iter()
            .find(|o| o.id == order_id)
            .cloned()
            .ok_or_else(|| {
                PaperError::Rejected(format!("Order {order_id} not found in open orders"))
            })
    }

    #[cfg(test)]
    pub fn cancel_order(&mut self, order_id: &str) -> Result<PaperOrder> {
        let order = self.decide_cancel(order_id)?;
        self.apply(
            Utc::now(),
            &AccountEvent::OrderCancelled {
                order: order.clone(),
            },
        );
        Ok(order)
    }

    #[cfg(test)]
    pub fn cancel_all_orders(&mut self) -> Vec<PaperOrder> {
        let orders = self.open_orders.clone();
        let ts = Utc::now();
        for order in &orders {
            self.apply(
                ts,
                &AccountEvent::OrderCancelled {
                    order: order.clone(),
                },
            );
        }
        orders
    }

    /// The fills the given quotes would trigger, in FIFO submission order.
    /// Mutates nothing — each trade becomes an [`AccountEvent::OrderFilled`].
    pub fn decide_pending_fills(
        &self,
        prices: &HashMap<String, (Decimal, Decimal)>,
    ) -> Vec<PaperTrade> {
        let mut fills = Vec::new();
        for order in &self.open_orders {
            let Some(&(ask, bid)) = prices.get(&order.pair) else {
                continue;
            };
            if !order.crosses(ask, bid) {
                continue;
            }
            let fill_price = order.price;
            let cost = order.volume * fill_price;
            let fee = cost * self.fee_rate;
            fills.push(PaperTrade {
                id: self.peek_order_id(fills.len() as u64),
                order_id: order.id.clone(),
                pair: order.pair.clone(),
                base: order.base.clone(),
                quote: order.quote.clone(),
                side: order.side,
                volume: order.volume,
                price: fill_price,
                fee,
                cost,
                filled_at: Utc::now(),
                reference_quote: ReferenceQuote::checked(ask, bid),
            });
        }
        fills
    }

    #[cfg(test)]
    pub fn check_pending_orders(
        &mut self,
        prices: &HashMap<String, (Decimal, Decimal)>,
    ) -> Vec<PaperTrade> {
        let fills = self.decide_pending_fills(prices);
        for trade in &fills {
            self.apply(
                trade.filled_at,
                &AccountEvent::OrderFilled {
                    trade: trade.clone(),
                },
            );
        }
        fills
    }

    /// Fold one event into state — the engine's only mutation path.
    ///
    /// Infallible pure bookkeeping from the event's recorded outputs; all
    /// validation happened in the `decide_*` that produced the event.
    /// Observational events (`Command`, `OrderRejected`, `Unknown`) are no-ops.
    pub fn apply(&mut self, ts: DateTime<Utc>, event: &AccountEvent) {
        match event {
            AccountEvent::Initialized(config) | AccountEvent::Reset(config) => {
                *self = Self::with_config(config.clone());
                self.created_at = ts;
                self.updated_at = ts;
            }
            AccountEvent::OrderSubmitted { order } => {
                *self
                    .reserved
                    .entry(order.reserved_asset.clone())
                    .or_insert(Decimal::ZERO) += order.reserved_amount;
                self.advance_order_seq(&order.id);
                self.updated_at = order.created_at;
                self.open_orders.push(order.clone());
            }
            AccountEvent::OrderFilled { trade } => {
                if let Some(pos) = self.open_orders.iter().position(|o| o.id == trade.order_id) {
                    let order = self.open_orders.remove(pos);
                    self.release_reservation(&order);
                }
                match trade.side {
                    OrderSide::Buy => {
                        *self
                            .balances
                            .entry(trade.quote.clone())
                            .or_insert(Decimal::ZERO) -= trade.cost + trade.fee;
                        *self
                            .balances
                            .entry(trade.base.clone())
                            .or_insert(Decimal::ZERO) += trade.volume;
                    }
                    OrderSide::Sell => {
                        *self
                            .balances
                            .entry(trade.base.clone())
                            .or_insert(Decimal::ZERO) -= trade.volume;
                        *self
                            .balances
                            .entry(trade.quote.clone())
                            .or_insert(Decimal::ZERO) += trade.cost - trade.fee;
                    }
                }
                self.advance_order_seq(&trade.order_id);
                self.advance_order_seq(&trade.id);
                self.updated_at = trade.filled_at;
                self.filled_trades.push(trade.clone());
            }
            AccountEvent::OrderCancelled { order } => {
                let Some(pos) = self.open_orders.iter().position(|o| o.id == order.id) else {
                    return;
                };
                let removed = self.open_orders.remove(pos);
                self.release_reservation(&removed);
                self.cancelled_orders.push(removed);
                self.updated_at = ts;
            }
            // The venue is the truth in live mode: the mirror adopts its
            // balances wholesale. Open orders and fill history are untouched —
            // they are the local record the snapshot reconciles against.
            AccountEvent::Attached(snapshot) => {
                self.balances = snapshot
                    .balances
                    .iter()
                    .map(|(asset, amount)| (asset.clone(), *amount))
                    .collect();
                // A window-opening snapshot re-anchors P&L at its own equity
                // and restores the account's rates so a folded scoring window
                // reports them (not the type defaults); a plain mirror attach
                // carries no anchor and leaves all of it alone.
                if let Some(anchor) = &snapshot.anchor {
                    self.starting_balance = anchor.equity;
                    self.starting_currency = anchor.currency.clone();
                    self.fee_rate = anchor.fee_rate;
                    self.slippage_rate = anchor.slippage_rate;
                }
                self.created_at = ts;
                self.updated_at = ts;
            }
            AccountEvent::Reconciled(snapshot) => {
                self.balances = snapshot
                    .balances
                    .iter()
                    .map(|(asset, amount)| (asset.clone(), *amount))
                    .collect();
                self.updated_at = ts;
            }
            AccountEvent::Command(_) | AccountEvent::OrderRejected(_) | AccountEvent::Unknown => {}
        }
    }

    /// The id `offset` allocations ahead of the counter, without advancing it.
    /// `decide_*` peeks ids; [`Self::apply`] advances the counter past any id
    /// it folds, so decided-then-applied ids never collide.
    fn peek_order_id(&self, offset: u64) -> String {
        format!("{ORDER_ID_PREFIX}{:05}", self.next_order_id + offset)
    }

    /// Advance the id counter past a folded id (`PAPER-{n}` → at least `n + 1`).
    fn advance_order_seq(&mut self, id: &str) {
        if let Some(seq) = id
            .strip_prefix(ORDER_ID_PREFIX)
            .and_then(|n| n.parse::<u64>().ok())
        {
            self.next_order_id = self.next_order_id.max(seq + 1);
        }
    }

    /// [`Self::valuation`] reduced to its historical `(value, complete)`
    /// shape for callers that don't need the unmarked assets by name.
    pub fn compute_portfolio_value(
        &self,
        prices: &HashMap<String, (Decimal, Decimal)>,
    ) -> (Decimal, bool) {
        let valuation = self.valuation(prices);
        let complete = valuation.is_complete();
        (valuation.value, complete)
    }

    /// Mark every balance from `prices` (keyed `"{asset}{starting_currency}"`,
    /// valued at the tuple's bid element). An asset with no usable mark is
    /// skipped — never a silent zero — and reported in
    /// [`Valuation::unmarked`]; the skip rules (dust, negative-at-face, cash
    /// 1:1) live only here.
    pub fn valuation(&self, prices: &HashMap<String, (Decimal, Decimal)>) -> Valuation {
        let mut valuation = Valuation::default();
        let sc = &self.starting_currency;
        // Summed in sorted key order. Decimal addition is exact for sane
        // magnitudes, but a 28-digit coefficient can still round at
        // pathological magnitude spreads — sorting keeps even that last
        // place identical every session (the Lab replay's byte-identity AC).
        let mut balances: Vec<(&String, &Decimal)> = self.balances.iter().collect();
        balances.sort_unstable_by(|a, b| a.0.cmp(b.0));
        for (asset, &amount) in balances {
            if amount.abs() < BALANCE_DUST {
                continue;
            }
            // Checked arithmetic throughout: [`validate_money`] bounds every
            // engine-produced value, but a journal is just a file — an
            // f64-era or hand-edited record can carry magnitudes whose
            // product overflows, and a read path must report an unvaluable
            // holding as unmarked, never panic.
            let contribution = if amount < Decimal::ZERO || asset == sc {
                Some(amount)
            } else {
                prices
                    .get(&format!("{asset}{sc}"))
                    .and_then(|&(_ask, bid)| amount.checked_mul(bid))
            };
            match contribution.and_then(|held| valuation.value.checked_add(held)) {
                Some(total) => valuation.value = total,
                None => {
                    valuation.unmarked.insert(asset.clone());
                }
            }
        }
        valuation
    }

    fn release_reservation(&mut self, order: &PaperOrder) {
        if let Some(reserved) = self.reserved.get_mut(&order.reserved_asset) {
            *reserved = (*reserved - order.reserved_amount).max(Decimal::ZERO);
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

pub fn parse_pair(pair: &str) -> Result<(String, String, String)> {
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
    Err(PaperError::Rejected(format!(
        "Unknown pair '{pair}'. Use slash format (e.g., BTC/USD) for uncommon pairs."
    )))
}

pub fn valid_quote(ask: Decimal, bid: Decimal) -> bool {
    ReferenceQuote::checked(ask, bid).is_some()
}

/// The canonical `(base, quote)` join key for a recorded symbol — never the
/// raw string: [`parse_pair`] canonicalizes XBT→BTC only in its base/quote
/// outputs, so string keys would not collide `XBTUSD` with `BTC/USD`.
pub fn canonical_key(symbol: &str) -> Option<(String, String)> {
    parse_pair(symbol)
        .ok()
        .map(|(_, base, quote)| (base, quote))
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn test_new_state_custom_balance() {
        let state = PaperState::new(dec!(5000.0), "EUR");
        assert_eq!(state.balances.get("EUR"), Some(&dec!(5000.0)));
        assert_eq!(state.starting_balance, dec!(5000.0));
        assert_eq!(state.starting_currency, "EUR");
        assert!(state.open_orders.is_empty());
        assert!(state.filled_trades.is_empty());
    }

    #[test]
    fn test_new_state_default() {
        let state = PaperState::default();
        assert_eq!(state.balances.get("USD"), Some(&dec!(10000.0)));
        assert_eq!(state.starting_balance, dec!(10000.0));
        assert_eq!(state.starting_currency, "USD");
    }

    #[test]
    fn test_reset_restores_init_params() {
        let mut state = PaperState::new(dec!(5000.0), "EUR");
        state
            .place_market_order(
                OrderSide::Buy,
                "BTCEUR",
                dec!(0.01),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap();
        assert!(state.balances.contains_key("BTC"));
        assert!(!state.filled_trades.is_empty());

        state.reset();
        assert_eq!(state.balances.get("EUR"), Some(&dec!(5000.0)));
        assert_eq!(state.starting_balance, dec!(5000.0));
        assert_eq!(state.starting_currency, "EUR");
        assert!(state.open_orders.is_empty());
        assert!(state.filled_trades.is_empty());
        assert!(!state.balances.contains_key("BTC"));
    }

    #[test]
    fn test_available_balance_with_reservation() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state.reserved.insert("USD".to_string(), dec!(3000.0));
        assert!((state.available_balance("USD") - dec!(7000.0)).abs() < dec!(0.0000000001));
    }

    #[test]
    fn test_market_buy() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        let trade = state
            .place_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap();
        assert_eq!(trade.side, OrderSide::Buy);
        assert_eq!(trade.volume, dec!(0.1));
        assert_eq!(trade.price, dec!(50000.0));
        let expected_cost = dec!(0.1) * dec!(50000.0);
        let expected_fee = expected_cost * DEFAULT_FEE_RATE;
        assert!((trade.fee - expected_fee).abs() < dec!(0.0000000001));
        assert!((trade.cost - expected_cost).abs() < dec!(0.0000000001));

        let usd = *state.balances.get("USD").unwrap();
        assert!((usd - (dec!(10000.0) - expected_cost - expected_fee)).abs() < dec!(0.0000000001));
        assert_eq!(*state.balances.get("BTC").unwrap(), dec!(0.1));
    }

    #[test]
    fn test_market_sell() {
        let mut state = PaperState::new(dec!(0.0), "USD");
        state.balances.insert("BTC".to_string(), dec!(1.0));
        let trade = state
            .place_market_order(
                OrderSide::Sell,
                "BTCUSD",
                dec!(0.5),
                dec!(50000.0),
                dec!(48000.0),
            )
            .unwrap();
        assert_eq!(trade.side, OrderSide::Sell);
        assert_eq!(trade.volume, dec!(0.5));
        assert_eq!(trade.price, dec!(48000.0));
        let expected_proceeds = dec!(0.5) * dec!(48000.0);
        let expected_fee = expected_proceeds * DEFAULT_FEE_RATE;
        assert!((trade.fee - expected_fee).abs() < dec!(0.0000000001));

        assert_eq!(*state.balances.get("BTC").unwrap(), dec!(0.5));
        let usd = *state.balances.get("USD").unwrap();
        assert!((usd - (expected_proceeds - expected_fee)).abs() < dec!(0.0000000001));
    }

    #[test]
    fn test_market_buy_insufficient_balance() {
        let mut state = PaperState::new(dec!(100.0), "USD");
        let result = state.place_market_order(
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            dec!(50000.0),
            dec!(49900.0),
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Insufficient USD"));
        assert_eq!(*state.balances.get("USD").unwrap(), dec!(100.0));
    }

    #[test]
    fn test_market_sell_insufficient_balance() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        let result = state.place_market_order(
            OrderSide::Sell,
            "BTCUSD",
            dec!(0.1),
            dec!(50000.0),
            dec!(48000.0),
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Insufficient BTC"));
    }

    #[test]
    fn test_fee_calculation() {
        let fee = dec!(1.0) * dec!(50_000.0) * DEFAULT_FEE_RATE;
        assert!((fee - dec!(130.0)).abs() < dec!(0.0000000001));
    }

    #[test]
    fn market_buy_applies_slippage() {
        let mut state = PaperState::with_config(PaperConfig {
            balance: dec!(10000.0),
            currency: "USD".into(),
            fee_rate: dec!(0.0),
            slippage_rate: dec!(0.001),
        });
        let trade = state
            .place_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap();
        // 50000 * 1.001 = 50050.0, volume 0.1 => cost 5005.0
        assert!((trade.price - dec!(50050.0)).abs() < dec!(0.000001));
    }

    #[test]
    fn market_sell_applies_slippage() {
        let mut state = PaperState::with_config(PaperConfig {
            balance: dec!(0.0),
            currency: "USD".into(),
            fee_rate: dec!(0.0),
            slippage_rate: dec!(0.001),
        });
        state.balances.insert("BTC".into(), dec!(1.0));
        let trade = state
            .place_market_order(
                OrderSide::Sell,
                "BTCUSD",
                dec!(1.0),
                dec!(50000.0),
                dec!(50000.0),
            )
            .unwrap();
        // 50000 * 0.999 = 49950.0 — clean number, no precision issue
        assert!((trade.price - dec!(49950.0)).abs() < dec!(0.000001));
    }

    #[test]
    fn zero_slippage_is_backward_compatible() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        let trade = state
            .place_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap();
        // no slippage, fill at exact ask
        assert!((trade.price - dec!(50000.0)).abs() < dec!(0.000001));
    }

    #[test]
    fn reset_with_preserves_slippage() {
        let mut state = PaperState::with_config(PaperConfig {
            balance: dec!(10000.0),
            currency: "USD".into(),
            fee_rate: dec!(0.0026),
            slippage_rate: dec!(0.001),
        });
        state.reset_with(Some(dec!(5000.0)), None, None, None);
        assert!((state.slippage_rate - dec!(0.001)).abs() < dec!(0.0000000001));
    }

    #[test]
    fn reset_with_updates_slippage() {
        let mut state = PaperState::with_config(PaperConfig {
            balance: dec!(10000.0),
            currency: "USD".into(),
            fee_rate: dec!(0.0026),
            slippage_rate: dec!(0.001),
        });
        state.reset_with(None, None, None, Some(dec!(0.002)));
        assert!((state.slippage_rate - dec!(0.002)).abs() < dec!(0.0000000001));
    }

    #[test]
    fn test_limit_buy_reserves_quote() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        let id = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45000.0))
            .unwrap();
        assert!(id.starts_with("PAPER-"));
        let expected_reserved = dec!(0.1) * dec!(45000.0) * (dec!(1.0) + DEFAULT_FEE_RATE);
        let reserved = state.reserved.get("USD").copied().unwrap_or(dec!(0.0));
        assert!((reserved - expected_reserved).abs() < dec!(0.0000000001));
        assert!(
            (state.available_balance("USD") - (dec!(10000.0) - expected_reserved)).abs()
                < dec!(0.0000000001)
        );
    }

    #[test]
    fn test_limit_sell_reserves_base() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state.balances.insert("BTC".to_string(), dec!(1.0));
        let id = state
            .place_limit_order(OrderSide::Sell, "BTCUSD", dec!(0.5), dec!(55000.0))
            .unwrap();
        assert!(id.starts_with("PAPER-"));
        let reserved = state.reserved.get("BTC").copied().unwrap_or(dec!(0.0));
        assert!((reserved - dec!(0.5)).abs() < dec!(0.0000000001));
        assert!((state.available_balance("BTC") - dec!(0.5)).abs() < dec!(0.0000000001));
    }

    #[test]
    fn test_limit_over_commit_rejected() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45000.0))
            .unwrap();
        let result = state.place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.2), dec!(45000.0));
        assert!(result.is_err());
        assert_eq!(state.open_orders.len(), 1);
    }

    #[test]
    fn test_cancel_order_releases_reservation() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        let id = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45000.0))
            .unwrap();
        assert!(state.reserved.get("USD").copied().unwrap_or(dec!(0.0)) > dec!(0.0));

        state.cancel_order(&id).unwrap();
        assert!(state.open_orders.is_empty());
        assert!(state.reserved.get("USD").copied().unwrap_or(dec!(0.0)) < dec!(0.0000000001));
        assert!((state.available_balance("USD") - dec!(10000.0)).abs() < dec!(0.0000000001));
    }

    #[test]
    fn test_cancel_nonexistent_order() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        let result = state.cancel_order("PAPER-99999");
        assert!(result.is_err());
    }

    #[test]
    fn test_cancel_all_releases_all() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.01), dec!(40000.0))
            .unwrap();
        state
            .place_limit_order(OrderSide::Buy, "ETHUSD", dec!(0.1), dec!(3000.0))
            .unwrap();
        assert_eq!(state.open_orders.len(), 2);

        let cancelled = state.cancel_all_orders();
        assert_eq!(cancelled.len(), 2);
        assert!(state.open_orders.is_empty());
        assert!((state.available_balance("USD") - dec!(10000.0)).abs() < dec!(0.0000000001));
    }

    #[test]
    fn test_check_pending_buy_fills() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45000.0))
            .unwrap();
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (dec!(44500.0), dec!(44400.0)));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].price, dec!(45000.0));
        assert!(state.open_orders.is_empty());
        assert!(*state.balances.get("BTC").unwrap() > dec!(0.0));
    }

    #[test]
    fn test_check_pending_sell_fills() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state.balances.insert("BTC".to_string(), dec!(1.0));
        state
            .place_limit_order(OrderSide::Sell, "BTCUSD", dec!(0.5), dec!(55000.0))
            .unwrap();
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (dec!(55500.0), dec!(55500.0)));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].price, dec!(55000.0));
        assert!(state.open_orders.is_empty());
    }

    #[test]
    fn test_check_pending_no_fill() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45000.0))
            .unwrap();
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (dec!(46000.0), dec!(45900.0)));
        let fills = state.check_pending_orders(&prices);
        assert!(fills.is_empty());
        assert_eq!(state.open_orders.len(), 1);
    }

    #[test]
    fn test_check_pending_fifo_order() {
        let mut state = PaperState::new(dec!(100000.0), "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45000.0))
            .unwrap();
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.2), dec!(44000.0))
            .unwrap();
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (dec!(43000.0), dec!(42900.0)));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 2);
        assert_eq!(fills[0].volume, dec!(0.1));
        assert_eq!(fills[1].volume, dec!(0.2));
    }

    #[test]
    fn test_reconcile_then_cancel() {
        let mut state = PaperState::new(dec!(100000.0), "USD");
        let _id_a = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45000.0))
            .unwrap();
        let id_b = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(30000.0))
            .unwrap();

        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (dec!(44000.0), dec!(43900.0)));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 1);

        let cancelled = state.cancel_order(&id_b).unwrap();
        assert_eq!(cancelled.id, id_b);
        assert!(state.open_orders.is_empty());
    }

    #[test]
    fn test_reconcile_then_cancel_all() {
        let mut state = PaperState::new(dec!(100000.0), "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45000.0))
            .unwrap();
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(30000.0))
            .unwrap();

        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (dec!(44000.0), dec!(43900.0)));
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
        let mut state = PaperState::new(dec!(100000.0), "USD");
        let id1 = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.01), dec!(40000.0))
            .unwrap();
        let id2 = state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.01), dec!(39000.0))
            .unwrap();
        assert_eq!(id1, "PAPER-00001");
        assert_eq!(id2, "PAPER-00002");
    }

    #[test]
    fn test_compute_portfolio_value_complete() {
        let mut state = PaperState::new(dec!(5000.0), "USD");
        state.balances.insert("BTC".to_string(), dec!(0.1));
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (dec!(50100.0), dec!(50000.0)));
        let (value, complete) = state.compute_portfolio_value(&prices);
        assert!(complete);
        assert!((value - dec!(10000.0)).abs() < dec!(0.0000000001));
    }

    #[test]
    fn test_compute_portfolio_value_partial() {
        let mut state = PaperState::new(dec!(5000.0), "USD");
        state.balances.insert("BTC".to_string(), dec!(0.1));
        state.balances.insert("ETH".to_string(), dec!(1.0));
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (dec!(50100.0), dec!(50000.0)));
        let (value, complete) = state.compute_portfolio_value(&prices);
        assert!(!complete);
        assert!((value - dec!(10000.0)).abs() < dec!(0.0000000001));
    }

    #[test]
    fn test_state_serialization_no_secrets() {
        let state = PaperState::new(dec!(10000.0), "USD");
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("api_key"));
        assert!(!json.contains("api_secret"));
        assert!(!json.contains("password"));
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
        let mut state = PaperState::new(dec!(100000.0), "USD");
        state
            .place_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap();
        assert!(state.balances.contains_key("BTC"));
        assert!(!state.balances.contains_key("XBT"));

        state
            .place_market_order(
                OrderSide::Sell,
                "XBTUSD",
                dec!(0.05),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap();
        assert!(state.balances.contains_key("BTC"));
        assert!(!state.balances.contains_key("XBT"));
    }

    #[test]
    fn test_canonicalization_xxbt_buy_btc_sell() {
        let mut state = PaperState::new(dec!(100000.0), "USD");
        state
            .place_market_order(
                OrderSide::Buy,
                "XXBTZUSD",
                dec!(0.1),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap();
        assert!(state.balances.contains_key("BTC"));
        assert!(!state.balances.contains_key("XBT"));
        assert!(!state.balances.contains_key("XXBT"));

        let btc_before = *state.balances.get("BTC").unwrap();
        state
            .place_market_order(
                OrderSide::Sell,
                "BTCUSD",
                dec!(0.05),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap();
        let btc_after = *state.balances.get("BTC").unwrap();
        assert!((btc_before - btc_after - dec!(0.05)).abs() < dec!(0.0000000001));
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
        let mut state = PaperState::new(dec!(100000.0), "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45000.0))
            .unwrap();
        let prices: HashMap<String, (Decimal, Decimal)> = HashMap::new();
        let fills = state.check_pending_orders(&prices);
        assert!(fills.is_empty());
        assert_eq!(state.open_orders.len(), 1);
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
        let mut state = PaperState::new(dec!(10000.0), "USD");
        let result = state.place_market_order(
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.0),
            dec!(50000.0),
            dec!(49900.0),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("positive"));
    }

    #[test]
    fn test_market_order_negative_volume_rejected() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        let result = state.place_market_order(
            OrderSide::Buy,
            "BTCUSD",
            -dec!(1.0),
            dec!(50000.0),
            dec!(49900.0),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("positive"));
    }

    #[test]
    fn test_limit_order_zero_volume_rejected() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        let result = state.place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.0), dec!(50000.0));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("positive"));
    }

    #[test]
    fn test_limit_order_zero_price_rejected() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        let result = state.place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(0.0));
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
        let mut state = PaperState::new(dec!(5000.0), "USD");
        state.balances.insert("BTC".to_string(), dec!(0.1));
        let empty_prices: HashMap<String, (Decimal, Decimal)> = HashMap::new();
        let (value, complete) = state.compute_portfolio_value(&empty_prices);
        assert!(
            !complete,
            "valuation must be incomplete when prices are missing"
        );
        assert!(
            (value - dec!(5000.0)).abs() < dec!(0.0000000001),
            "value must be quote-currency only"
        );
    }

    // The old NaN/infinity rejection tests are gone with f64: `Decimal`
    // cannot represent either, so the boundary parser enforces what the
    // engine used to guard. Zero/negative gates remain testable.

    #[test]
    fn market_order_rejects_zero_bid() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state.balances.insert("BTC".into(), dec!(1.0));
        let err = state.place_market_order(
            OrderSide::Sell,
            "BTCUSD",
            dec!(0.001),
            dec!(50000.0),
            dec!(0.0),
        );
        assert!(err.is_err());
    }

    #[test]
    fn market_order_rejects_zero_ask() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        let err = state.place_market_order(
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.001),
            dec!(0.0),
            dec!(49990.0),
        );
        assert!(err.is_err());
    }

    // Regression pins for the Decimal-overflow panics: a magnitude past
    // MAX_MONEY must reject at the gate, never reach a multiplication.

    #[test]
    fn market_order_rejects_volume_above_max_money() {
        let state = PaperState::new(dec!(1000.0), "USD");
        let err = state
            .decide_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(9_000_000_000_000_000_000_000_000_000),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap_err();
        assert!(err.to_string().contains("at most"), "got: {err}");
    }

    #[test]
    fn limit_order_rejects_price_above_max_money() {
        let state = PaperState::new(dec!(1000.0), "USD");
        let err = state
            .decide_limit_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(10_000_000_000_000_000),
                dec!(10_000_000_000_000_000),
            )
            .unwrap_err();
        assert!(err.to_string().contains("at most"), "got: {err}");
    }

    #[test]
    fn valuation_reports_an_overflowing_holding_as_unmarked_not_panic() {
        // A journal is just a file: an f64-era or hand-edited record can
        // hold magnitudes the gates never saw. Valuing it must degrade to
        // an unmarked holding, never kill a read path.
        let mut state = PaperState::new(dec!(1000.0), "USD");
        state
            .balances
            .insert("BTC".into(), dec!(60_000_000_000_000_000_000_000_000));
        let prices = HashMap::from([(
            "BTCUSD".to_string(),
            (dec!(2_000_000_000_000), dec!(2_000_000_000_000)),
        )]);
        let valuation = state.valuation(&prices);
        assert!(!valuation.is_complete());
        assert!(valuation.unmarked.contains("BTC"));
        assert_eq!(valuation.value, dec!(1000.0), "cash still counted");
    }

    #[test]
    fn market_fill_records_reference_quote_before_slippage() {
        let mut state = PaperState::with_config(PaperConfig {
            balance: dec!(10000.0),
            currency: "USD".into(),
            fee_rate: dec!(0.0),
            slippage_rate: dec!(0.001),
        });
        let trade = state
            .place_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap();
        // nonzero slippage: a blended value leaking into the quote would differ
        // from the raw book, so the equality below pins the raw capture
        let quote = trade
            .reference_quote
            .expect("market fill records the quote");
        assert_eq!(quote.ask, dec!(50000.0));
        assert_eq!(quote.bid, dec!(49900.0));
    }

    #[test]
    fn market_sell_records_reference_quote_before_slippage() {
        let mut state = PaperState::with_config(PaperConfig {
            balance: dec!(0.0),
            currency: "USD".into(),
            fee_rate: dec!(0.0),
            slippage_rate: dec!(0.001),
        });
        state.balances.insert("BTC".to_string(), dec!(1.0));
        let trade = state
            .place_market_order(
                OrderSide::Sell,
                "BTCUSD",
                dec!(0.5),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap();
        let quote = trade
            .reference_quote
            .expect("market fill records the quote");
        assert_eq!(quote.ask, dec!(50000.0));
        assert_eq!(quote.bid, dec!(49900.0));
    }

    #[test]
    fn limit_fill_records_market_quote_not_limit_price() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45000.0))
            .unwrap();
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (dec!(44500.0), dec!(44400.0)));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].price, dec!(45000.0));
        let quote = fills[0]
            .reference_quote
            .expect("limit fill records the quote");
        assert_eq!(quote.ask, dec!(44500.0));
        assert_eq!(quote.bid, dec!(44400.0));
    }

    #[test]
    fn limit_sell_fill_records_market_quote() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state.balances.insert("BTC".to_string(), dec!(1.0));
        state
            .place_limit_order(OrderSide::Sell, "BTCUSD", dec!(0.5), dec!(55000.0))
            .unwrap();
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (dec!(55500.0), dec!(55450.0)));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].price, dec!(55000.0));
        let quote = fills[0]
            .reference_quote
            .expect("limit fill records the quote");
        assert_eq!(quote.ask, dec!(55500.0));
        assert_eq!(quote.bid, dec!(55450.0));
    }

    #[test]
    fn limit_fill_with_degenerate_uncompared_side_records_no_quote() {
        // A buy's crossing predicate only inspects the ask; a zero bid is a
        // broken frame, and `ReferenceQuote::checked` must refuse to stamp it
        // on the fill even though the fill itself is legitimate.
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state
            .place_limit_order(OrderSide::Buy, "BTCUSD", dec!(0.1), dec!(45000.0))
            .unwrap();
        let mut prices = HashMap::new();
        prices.insert("BTCUSD".to_string(), (dec!(44500.0), Decimal::ZERO));
        let fills = state.check_pending_orders(&prices);
        assert_eq!(fills.len(), 1);
        assert!(fills[0].reference_quote.is_none());
        let json = serde_json::to_string(&state).unwrap();
        let reloaded: PaperState =
            serde_json::from_str(&json).expect("state with an unquoted fill must round-trip");
        assert!(reloaded.filled_trades[0].reference_quote.is_none());
    }

    #[test]
    fn deserialize_trade_missing_reference_quote_defaults_to_none() {
        let json = r#"{
            "balances": {"USD": 5000.0, "BTC": 0.1},
            "reserved": {},
            "open_orders": [],
            "filled_trades": [{
                "id": "PAPER-00002",
                "order_id": "PAPER-00001",
                "pair": "BTCUSD",
                "base": "BTC",
                "quote": "USD",
                "side": "buy",
                "volume": "0.1",
                "price": "50000.0",
                "fee": "13.0",
                "cost": "5000.0",
                "filled_at": "2026-01-01T00:00:00Z"
            }],
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;
        let state: PaperState =
            serde_json::from_str(json).expect("a trade without reference_quote must keep loading");
        assert!(state.filled_trades[0].reference_quote.is_none());
    }

    #[test]
    fn reference_quote_survives_serde_roundtrip() {
        let mut state = PaperState::new(dec!(10000.0), "USD");
        state
            .place_market_order(
                OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                dec!(50000.0),
                dec!(49900.0),
            )
            .unwrap();
        let json = serde_json::to_string(&state).unwrap();
        let loaded: PaperState = serde_json::from_str(&json).unwrap();
        let quote = loaded.filled_trades[0]
            .reference_quote
            .expect("quote survives persistence");
        assert_eq!(quote.ask, dec!(50000.0));
        assert_eq!(quote.bid, dec!(49900.0));
    }
}
