//! The `kraken ws` command surface and dispatch.

use crate::error::InvalidRequest;
use clap::{Args, Subcommand};

/// Kraken caps the dead man's switch timeout below one day.
const MAX_CANCEL_AFTER_SECS: u64 = 86_400;

/// Kraken bounds `batch_add` to 2–15 orders per request.
const BATCH_ADD_ORDERS: std::ops::RangeInclusive<usize> = 2..=15;

/// Kraken bounds `batch_cancel` to 2–50 identifiers per request.
const BATCH_CANCEL_IDS: std::ops::RangeInclusive<usize> = 2..=50;

use kraken_core::{
    AddOrderParams, AmendOrderParams, BatchAddParams, BatchCancelParams, BatchOrderParams,
    BookDepth, CancelAllOrdersAfterParams, CancelOrderParams, EventTrigger, FeePreference,
    Level3Depth, OhlcInterval, OrderSide, OrderType, PriceType, StpType, TimeInForce,
    TriggerReference, Triggers, Users, WsSubscription,
};
use rust_decimal::Decimal;

/// The two WS v2 command families, flattened so the CLI surface stays one flat
/// list of subcommands. The split is the routing: which family a command joins
/// decides its driver.
// One value exists per process, parsed once and dispatched immediately, so the size gap
// between the fat order variants and the unit ones costs nothing worth boxing for.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(clippy::large_enum_variant)]
pub enum WsCommand {
    /// Streaming channel subscriptions.
    #[command(flatten)]
    Channel(ChannelCommand),
    /// One-shot request/response methods.
    #[command(flatten)]
    Method(MethodCommand),
}

/// The subscription half of the surface — every variant converts infallibly
/// into the [`WsSubscription`] it streams.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelCommand {
    /// Stream live ticker updates.
    Ticker(Ticker),
    /// Stream live trades.
    Trades(Trades),
    /// Stream order book updates.
    Book(Book),
    /// Stream OHLC candle updates.
    Ohlc(Ohlc),
    /// Stream instrument metadata updates.
    Instrument(Instrument),
    /// Stream trade executions (auth required).
    Executions(Executions),
    /// Stream balance updates (auth required).
    Balances(Balances),
    /// Stream Level 3 order book (auth required).
    Level3(Level3),
}

/// The request/response half of the surface — every variant converts (fallibly)
/// into the typed params of the WS v2 method it invokes.
// See `WsCommand`: one short-lived value per process, not worth boxing.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(clippy::large_enum_variant)]
pub enum MethodCommand {
    /// Place a new order via WebSocket (auth required).
    AddOrder(AddOrder),
    /// Amend an existing order via WebSocket (auth required).
    /// Provide --order-id or --cl-ord-id to identify the order.
    AmendOrder(AmendOrder),
    /// Cancel one or more orders via WebSocket (auth required).
    CancelOrder(CancelOrder),
    /// Cancel all open orders via WebSocket (auth required).
    CancelAll(CancelAll),
    /// Dead man's switch: cancel all orders after timeout (auth required).
    CancelAfter(CancelAfter),
    /// Batch add orders via WebSocket (2-15 orders, single pair, auth
    /// required).
    BatchAdd(BatchAdd),
    /// Batch cancel orders via WebSocket (2-50 orders, auth required).
    BatchCancel(BatchCancel),
    /// Verify the connection with an application-level ping/pong.
    Ping(Ping),
}

// Takes no arguments; a struct rather than a unit variant so every leaf of the
// tree has a type the tool surface can classify.
#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CancelAll {}

// Takes no arguments; see `CancelAll`.
#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Ping {}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Ticker {
    /// Trading pairs.
    #[arg(required = true, num_args = 1..)]
    pub pairs: Vec<String>,
    /// Event trigger: trades (default) or bbo.
    #[arg(long)]
    pub event_trigger: Option<EventTrigger>,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    pub snapshot: Option<bool>,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Trades {
    /// Trading pairs.
    #[arg(required = true, num_args = 1..)]
    pub pairs: Vec<String>,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    pub snapshot: Option<bool>,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Book {
    /// Trading pairs.
    #[arg(required = true, num_args = 1..)]
    pub pairs: Vec<String>,
    /// Book depth (10, 25, 100, 500, 1000).
    #[arg(long, default_value = "10")]
    #[serde(default)]
    pub depth: BookDepth,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    pub snapshot: Option<bool>,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Ohlc {
    /// Trading pairs.
    #[arg(required = true, num_args = 1..)]
    pub pairs: Vec<String>,
    /// Candle interval in minutes (1, 5, 15, 30, 60, 240, 1440, 10080, 21600).
    #[arg(long, default_value = "1")]
    #[serde(default)]
    pub interval: OhlcInterval,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    pub snapshot: Option<bool>,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Instrument {
    /// Trading pairs (omit for all instruments).
    #[arg(num_args = 0..)]
    #[serde(default)]
    pub pairs: Vec<String>,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    pub snapshot: Option<bool>,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Executions {
    /// Include last 50 trade fills in snapshot.
    #[arg(long)]
    pub snap_trades: Option<bool>,
    /// Include open orders in snapshot (default: true).
    #[arg(long)]
    pub snap_orders: Option<bool>,
    /// Stream all status transitions (default: true). When false, only
    /// open/close transitions.
    #[arg(long)]
    pub order_status: Option<bool>,
    /// Display xstocks in terms of underlying equity (default: true).
    #[arg(long)]
    pub rebased: Option<bool>,
    /// Include rate-limit counter in stream.
    #[arg(long)]
    pub ratecounter: Option<bool>,
    /// Stream events for master and subaccounts (pass "all").
    #[arg(long)]
    pub users: Option<Users>,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Balances {
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    pub snapshot: Option<bool>,
    /// Display xstocks in terms of underlying equity (default: true).
    #[arg(long)]
    pub rebased: Option<bool>,
    /// Stream events for master and subaccounts (pass "all").
    #[arg(long)]
    pub users: Option<Users>,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Level3 {
    /// Trading pairs.
    #[arg(required = true, num_args = 1..)]
    pub pairs: Vec<String>,
    /// Number of price levels per side (10, 100, 1000).
    #[arg(long)]
    pub depth: Option<Level3Depth>,
    /// Request a snapshot on subscribe (default: true).
    #[arg(long)]
    pub snapshot: Option<bool>,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct AddOrder {
    /// Order type: limit, market, iceberg, stop-loss, stop-loss-limit,
    /// take-profit, take-profit-limit, trailing-stop, trailing-stop-limit,
    /// settle-position.
    #[arg(long)]
    pub order_type: OrderType,
    /// OrderSide: buy or sell.
    #[arg(long)]
    pub side: OrderSide,
    /// Order quantity in base asset (mutually exclusive with --cash-order-qty).
    #[arg(long)]
    pub order_qty: Option<Decimal>,
    /// Trading pair symbol (e.g. BTC/USD).
    #[arg(long)]
    pub symbol: String,
    /// Limit price.
    #[arg(long)]
    pub limit_price: Option<Decimal>,
    /// Time-in-force: gtc (default), gtd, ioc.
    #[arg(long)]
    pub time_in_force: Option<TimeInForce>,
    /// Fund on margin.
    #[arg(long)]
    #[serde(default)]
    pub margin: bool,
    /// Post-only order.
    #[arg(long)]
    #[serde(default)]
    pub post_only: bool,
    /// Reduce-only order.
    #[arg(long)]
    #[serde(default)]
    pub reduce_only: bool,
    /// Trigger reference price: last or index.
    #[arg(long)]
    pub trigger_reference: Option<TriggerReference>,
    /// Trigger price.
    #[arg(long)]
    pub trigger_price: Option<Decimal>,
    /// Trigger price type: static, pct, quote.
    #[arg(long)]
    pub trigger_price_type: Option<PriceType>,
    /// Effective time (RFC3339).
    #[arg(long)]
    pub effective_time: Option<String>,
    /// Expire time (RFC3339, for GTD orders).
    #[arg(long)]
    pub expire_time: Option<String>,
    /// Deadline (RFC3339, 500ms-60s from now).
    #[arg(long)]
    pub deadline: Option<String>,
    /// Client order ID.
    #[arg(long)]
    pub cl_ord_id: Option<String>,
    /// Numeric client order reference (signed 32-bit, per the API).
    #[arg(long)]
    pub order_userref: Option<i32>,
    /// Order volume in quote currency (buy market orders without margin).
    #[arg(long)]
    pub cash_order_qty: Option<Decimal>,
    /// Display quantity for iceberg orders.
    #[arg(long)]
    pub display_qty: Option<Decimal>,
    /// Fee preference: base or quote.
    #[arg(long)]
    pub fee_preference: Option<FeePreference>,
    /// Self-trade prevention: cancel_newest, cancel_oldest, cancel_both.
    #[arg(long)]
    pub stp_type: Option<StpType>,
    /// Sub-account/trader ID for granular STP (institutional accounts).
    #[arg(long)]
    pub sender_sub_id: Option<String>,
    /// Validate only, do not submit.
    #[arg(long)]
    #[serde(default)]
    pub validate: bool,
}

#[derive(Debug, Default, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct AmendOrder {
    /// Kraken order ID (required unless --cl-ord-id is given).
    #[arg(long)]
    pub order_id: Option<String>,
    /// New order quantity.
    #[arg(long)]
    pub order_qty: Option<Decimal>,
    /// New limit price.
    #[arg(long)]
    pub limit_price: Option<Decimal>,
    /// New trigger price (triggered order types).
    #[arg(long)]
    pub trigger_price: Option<Decimal>,
    /// Reject the amend if the new limit price cannot post passively.
    #[arg(long)]
    #[serde(default)]
    pub post_only: bool,
    /// New display quantity (iceberg).
    #[arg(long)]
    pub display_qty: Option<Decimal>,
    /// Deadline (RFC3339).
    #[arg(long)]
    pub deadline: Option<String>,
    /// Client order ID (alternative to --order-id).
    #[arg(long)]
    pub cl_ord_id: Option<String>,
    /// Symbol (required for non-crypto pairs).
    #[arg(long)]
    pub symbol: Option<String>,
}

#[derive(Debug, Default, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CancelOrder {
    /// Kraken order IDs to cancel.
    #[arg(long, num_args = 1..)]
    #[serde(default)]
    pub order_id: Vec<String>,
    /// Client order IDs to cancel.
    #[arg(long, num_args = 1..)]
    #[serde(default)]
    pub cl_ord_id: Vec<String>,
    /// Numeric client order refs to cancel (signed 32-bit, per the API).
    #[arg(long, num_args = 1..)]
    #[serde(default)]
    pub order_userref: Vec<i32>,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CancelAfter {
    /// Timeout in seconds (0 to disable, must be < 86400).
    pub timeout: u64,
}

#[derive(Debug, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BatchAdd {
    /// Trading pair symbol (e.g. BTC/USD).
    #[arg(long)]
    pub symbol: String,
    /// Orders as JSON array (see Kraken WS V2 batch_add docs for schema).
    #[arg(long)]
    pub orders: String,
    /// Validate only, do not submit.
    #[arg(long)]
    #[serde(default)]
    pub validate: bool,
    /// Deadline (RFC3339).
    #[arg(long)]
    pub deadline: Option<String>,
}

#[derive(Debug, Default, Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BatchCancel {
    /// Order IDs or user refs to cancel (required).
    #[arg(long, num_args = 1.., required = true)]
    pub orders: Vec<String>,
    /// Additional client order IDs to cancel.
    #[arg(long, num_args = 1..)]
    #[serde(default)]
    pub cl_ord_id: Vec<String>,
}

impl From<Ticker> for WsSubscription {
    fn from(args: Ticker) -> Self {
        let Ticker {
            pairs,
            event_trigger,
            snapshot,
        } = args;
        WsSubscription::Ticker {
            symbol: pairs,
            event_trigger,
            snapshot,
        }
    }
}

impl From<Trades> for WsSubscription {
    fn from(args: Trades) -> Self {
        let Trades { pairs, snapshot } = args;
        WsSubscription::Trade {
            symbol: pairs,
            snapshot,
        }
    }
}

impl From<Book> for WsSubscription {
    fn from(args: Book) -> Self {
        let Book {
            pairs,
            depth,
            snapshot,
        } = args;
        WsSubscription::Book {
            symbol: pairs,
            depth,
            snapshot,
        }
    }
}

impl From<Ohlc> for WsSubscription {
    fn from(args: Ohlc) -> Self {
        let Ohlc {
            pairs,
            interval,
            snapshot,
        } = args;
        WsSubscription::Ohlc {
            symbol: pairs,
            interval,
            snapshot,
        }
    }
}

impl From<Instrument> for WsSubscription {
    fn from(args: Instrument) -> Self {
        let Instrument { pairs, snapshot } = args;
        WsSubscription::Instrument {
            symbol: pairs,
            snapshot,
        }
    }
}

impl From<Executions> for WsSubscription {
    fn from(args: Executions) -> Self {
        let Executions {
            snap_trades,
            snap_orders,
            order_status,
            rebased,
            ratecounter,
            users,
        } = args;
        WsSubscription::Executions {
            snap_trades,
            snap_orders,
            order_status,
            rebased,
            ratecounter,
            users,
        }
    }
}

impl From<Balances> for WsSubscription {
    fn from(args: Balances) -> Self {
        let Balances {
            snapshot,
            rebased,
            users,
        } = args;
        WsSubscription::Balances {
            snapshot,
            rebased,
            users,
        }
    }
}

impl From<Level3> for WsSubscription {
    fn from(args: Level3) -> Self {
        let Level3 {
            pairs,
            depth,
            snapshot,
        } = args;
        WsSubscription::Level3 {
            symbol: pairs,
            depth,
            snapshot,
        }
    }
}

impl From<ChannelCommand> for WsSubscription {
    fn from(cmd: ChannelCommand) -> Self {
        match cmd {
            ChannelCommand::Ticker(args) => args.into(),
            ChannelCommand::Trades(args) => args.into(),
            ChannelCommand::Book(args) => args.into(),
            ChannelCommand::Ohlc(args) => args.into(),
            ChannelCommand::Instrument(args) => args.into(),
            ChannelCommand::Executions(args) => args.into(),
            ChannelCommand::Balances(args) => args.into(),
            ChannelCommand::Level3(args) => args.into(),
        }
    }
}

impl TryFrom<AddOrder> for AddOrderParams {
    type Error = InvalidRequest;

    fn try_from(args: AddOrder) -> Result<Self, Self::Error> {
        let AddOrder {
            order_type,
            side,
            order_qty,
            symbol,
            limit_price,
            time_in_force,
            margin,
            post_only,
            reduce_only,
            trigger_reference,
            trigger_price,
            trigger_price_type,
            effective_time,
            expire_time,
            deadline,
            cl_ord_id,
            order_userref,
            cash_order_qty,
            display_qty,
            fee_preference,
            stp_type,
            sender_sub_id,
            validate,
        } = args;
        order_qty_rule(order_qty, cash_order_qty, order_type, side)?;
        Ok(AddOrderParams {
            order_type,
            side,
            symbol,
            order_qty,
            cash_order_qty,
            limit_price,
            limit_price_type: None,
            time_in_force,
            // Wire defaults are false; send Some(true) only when the flag is
            // set so unset flags stay off the frame.
            margin: margin.then_some(true),
            post_only: post_only.then_some(true),
            reduce_only: reduce_only.then_some(true),
            // No CLI flags yet: conditional closes and no_mpp ride the --orders JSON
            // path (batch_add) or an SDK-constructed params value, not add-order args.
            no_mpp: None,
            triggers: triggers(trigger_reference, trigger_price, trigger_price_type)?,
            conditional: None,
            effective_time,
            expire_time,
            deadline,
            cl_ord_id,
            order_userref,
            display_qty,
            fee_preference,
            stp_type,
            sender_sub_id,
            validate: validate.then_some(true),
        })
    }
}

impl TryFrom<AmendOrder> for AmendOrderParams {
    type Error = InvalidRequest;

    fn try_from(args: AmendOrder) -> Result<Self, Self::Error> {
        let AmendOrder {
            order_id,
            order_qty,
            limit_price,
            trigger_price,
            post_only,
            display_qty,
            deadline,
            cl_ord_id,
            symbol,
        } = args;
        if order_id.is_none() && cl_ord_id.is_none() {
            return Err(InvalidRequest::new(
                "Either --order-id or --cl-ord-id is required for amend-order",
            ));
        }
        Ok(AmendOrderParams {
            order_id,
            cl_ord_id,
            order_qty,
            display_qty,
            limit_price,
            limit_price_type: None,
            post_only: post_only.then_some(true),
            trigger_price,
            trigger_price_type: None,
            deadline,
            symbol,
        })
    }
}

impl TryFrom<CancelOrder> for CancelOrderParams {
    type Error = InvalidRequest;

    /// Exactly one identifier kind, with at least one value — clap cannot see
    /// across the three lists, and the v2 contract forbids mixing them in
    /// one request.
    fn try_from(args: CancelOrder) -> Result<Self, Self::Error> {
        let CancelOrder {
            order_id,
            cl_ord_id,
            order_userref,
        } = args;
        match (
            order_id.is_empty(),
            cl_ord_id.is_empty(),
            order_userref.is_empty(),
        ) {
            (false, true, true) => Ok(CancelOrderParams::OrderIds { order_id }),
            (true, false, true) => Ok(CancelOrderParams::ClOrdIds { cl_ord_id }),
            (true, true, false) => Ok(CancelOrderParams::UserRefs { order_userref }),
            (true, true, true) => Err(InvalidRequest::new(
                "At least one --order-id, --cl-ord-id, or --order-userref is required",
            )),
            _ => Err(InvalidRequest::new(
                "--order-id, --cl-ord-id, and --order-userref are mutually exclusive",
            )),
        }
    }
}

impl TryFrom<CancelAfter> for CancelAllOrdersAfterParams {
    type Error = InvalidRequest;

    fn try_from(args: CancelAfter) -> Result<Self, Self::Error> {
        let CancelAfter { timeout } = args;
        if timeout >= MAX_CANCEL_AFTER_SECS {
            return Err(InvalidRequest::new(format!(
                "Timeout must be < {MAX_CANCEL_AFTER_SECS} seconds"
            )));
        }
        Ok(CancelAllOrdersAfterParams { timeout })
    }
}

impl TryFrom<BatchAdd> for BatchAddParams {
    type Error = InvalidRequest;

    /// Parses the `--orders` JSON array and enforces Kraken's batch bound.
    fn try_from(args: BatchAdd) -> Result<Self, Self::Error> {
        let BatchAdd {
            symbol,
            orders,
            validate,
            deadline,
        } = args;
        let orders: Vec<BatchOrderParams> = serde_json::from_str(&orders)
            .map_err(|e| InvalidRequest::new(format!("Invalid orders JSON: {e}")))?;
        if !BATCH_ADD_ORDERS.contains(&orders.len()) {
            return Err(InvalidRequest::new(format!(
                "Batch requires {}-{} orders, got {}",
                BATCH_ADD_ORDERS.start(),
                BATCH_ADD_ORDERS.end(),
                orders.len()
            )));
        }
        Ok(BatchAddParams {
            symbol,
            orders,
            validate: validate.then_some(true),
            deadline,
        })
    }
}

impl TryFrom<BatchCancel> for BatchCancelParams {
    type Error = InvalidRequest;

    /// The order-id and client-order-id lists count against one shared bound.
    fn try_from(args: BatchCancel) -> Result<Self, Self::Error> {
        let BatchCancel { orders, cl_ord_id } = args;
        let total = orders.len() + cl_ord_id.len();
        if !BATCH_CANCEL_IDS.contains(&total) {
            return Err(InvalidRequest::new(format!(
                "Batch cancel requires {}-{} identifiers, got {total}",
                BATCH_CANCEL_IDS.start(),
                BATCH_CANCEL_IDS.end(),
            )));
        }
        Ok(BatchCancelParams { orders, cl_ord_id })
    }
}

/// Cross-field quantity rules clap cannot express: exactly one of `--order-qty`
/// / `--cash-order-qty`, and the cash form only on market buys (the only shape
/// Kraken accepts it for).
fn order_qty_rule(
    order_qty: Option<Decimal>,
    cash_order_qty: Option<Decimal>,
    order_type: OrderType,
    side: OrderSide,
) -> Result<(), InvalidRequest> {
    match (order_qty, cash_order_qty) {
        (Some(_), Some(_)) => Err(InvalidRequest::new(
            "--order-qty and --cash-order-qty are mutually exclusive",
        )),
        (None, None) => Err(InvalidRequest::new(
            "Either --order-qty or --cash-order-qty is required",
        )),
        (None, Some(_)) if order_type != OrderType::Market || side != OrderSide::Buy => Err(
            InvalidRequest::new("--cash-order-qty is only available for market buy orders"),
        ),
        _ => Ok(()),
    }
}
/// Fold the three trigger flags into one wire object, or `None` when none was
/// given so the frame carries no `triggers` key at all. `--trigger-price`
/// anchors the group: the other two only qualify it, so they are rejected
/// without it rather than shipping a priceless trigger the server will bounce.
fn triggers(
    reference: Option<TriggerReference>,
    price: Option<Decimal>,
    price_type: Option<PriceType>,
) -> Result<Option<Triggers>, InvalidRequest> {
    match price {
        Some(price) => Ok(Some(Triggers {
            reference,
            price,
            price_type,
        })),
        None if reference.is_some() || price_type.is_some() => Err(InvalidRequest::new(
            "--trigger-reference/--trigger-price-type require --trigger-price",
        )),
        None => Ok(None),
    }
}

crate::tools! {
    /// The `kraken streaming ws cmd channel` commands.
    fn streaming_ws_cmd_channel in ["streaming", "ws", "cmd", "channel"] of ChannelCommand {
        Ticker     as "ticker"     => Websocket, ReadOnly, None, Stream
            example "kraken ws ticker BTC/USD ETH/USD";
        Trades     as "trades"     => Websocket, ReadOnly, None, Stream
            example "kraken ws trades BTC/USD";
        Book       as "book"       => Websocket, ReadOnly, None, Stream
            example "kraken ws book BTC/USD --depth 25";
        Ohlc       as "ohlc"       => Websocket, ReadOnly, None, Stream
            example "kraken ws ohlc BTC/USD --interval 5";
        Instrument as "instrument" => Websocket, ReadOnly, None, Stream
            example "kraken ws instrument BTC/USD";
        Executions as "executions" => Websocket, ReadOnly, Spot, Stream
            example "kraken ws executions";
        Balances   as "balances"   => Websocket, ReadOnly, Spot, Stream
            example "kraken ws balances";
        Level3     as "level3"     => Websocket, ReadOnly, Spot, Stream
            example "kraken ws level3 BTC/USD";
    }
}

crate::tools! {
    /// The `kraken streaming ws cmd method` commands.
    fn streaming_ws_cmd_method in ["streaming", "ws", "cmd", "method"] of MethodCommand {
        AddOrder    as "add-order"    => Websocket, Dangerous, Spot, Stream
            example "kraken ws add-order --order-type limit --side buy --symbol BTC/USD --order-qty 0.001 --limit-price 50000";
        AmendOrder  as "amend-order"  => Websocket, Dangerous, Spot, Stream
            example "kraken ws amend-order --order-id OABC12 --limit-price 51000";
        CancelOrder as "cancel-order" => Websocket, Dangerous, Spot, Stream
            example "kraken ws cancel-order --order-id OABC12";
        CancelAll   as "cancel-all"   => Websocket, Dangerous, Spot, Stream
            example "kraken ws cancel-all";
        CancelAfter as "cancel-after" => Websocket, Dangerous, Spot, Stream
            example "kraken ws cancel-after 60";
        BatchAdd    as "batch-add"    => Websocket, Dangerous, Spot, Stream
            example "kraken ws batch-add --symbol BTC/USD --orders \"[...]\"";
        BatchCancel as "batch-cancel" => Websocket, Dangerous, Spot, Stream
            example "kraken ws batch-cancel --orders OABC12 ODEF34";
        Ping        as "ping"         => Websocket, ReadOnly,  None, Stream
            example "kraken ws ping";
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal a caller turns into a `validation` envelope, checked by the
    /// phrase the user has to act on.
    fn expect_refusal<T: std::fmt::Debug>(result: Result<T, InvalidRequest>, needle: &str) {
        match result {
            Err(err) => assert!(
                err.to_string().contains(needle),
                "expected a refusal containing {needle:?}, got {err}"
            ),
            Ok(value) => panic!("expected a refusal containing {needle:?}, got {value:?}"),
        }
    }

    #[test]
    fn order_qty_rule_requires_exactly_one_quantity() {
        let (qty, cash) = (Decimal::ONE, Decimal::from(100));
        expect_refusal(
            order_qty_rule(Some(qty), Some(cash), OrderType::Limit, OrderSide::Buy),
            "mutually exclusive",
        );
        expect_refusal(
            order_qty_rule(None, None, OrderType::Limit, OrderSide::Buy),
            "required",
        );
        assert!(order_qty_rule(Some(qty), None, OrderType::Limit, OrderSide::Sell).is_ok());
    }

    #[test]
    fn cash_order_qty_is_market_buy_only() {
        let cash = Decimal::from(100);
        expect_refusal(
            order_qty_rule(None, Some(cash), OrderType::Limit, OrderSide::Buy),
            "market buy",
        );
        expect_refusal(
            order_qty_rule(None, Some(cash), OrderType::Market, OrderSide::Sell),
            "market buy",
        );
        assert!(order_qty_rule(None, Some(cash), OrderType::Market, OrderSide::Buy).is_ok());
    }

    #[test]
    fn triggers_fold_to_none_when_no_flag_is_given() {
        assert_eq!(triggers(None, None, None).expect("no flags is valid"), None);
        let price = Decimal::from(50_000);
        let some = triggers(None, Some(price), None)
            .expect("a price alone is a valid trigger")
            .expect("a price alone builds triggers");
        assert_eq!(some.price, price);
    }

    #[test]
    fn trigger_qualifiers_without_a_price_are_rejected() {
        expect_refusal(
            triggers(Some(TriggerReference::Last), None, None).map(|_| ()),
            "--trigger-price",
        );
    }

    #[test]
    fn amend_order_requires_an_identifier() {
        expect_refusal(
            AmendOrderParams::try_from(AmendOrder::default()),
            "--order-id or --cl-ord-id",
        );
    }

    #[test]
    fn cancel_after_rejects_timeout_at_or_above_one_day() {
        expect_refusal(
            CancelAllOrdersAfterParams::try_from(CancelAfter { timeout: 86_400 }),
            "86400",
        );
    }

    #[test]
    fn cancel_after_converts_below_the_limit() {
        let params = CancelAllOrdersAfterParams::try_from(CancelAfter { timeout: 60 })
            .expect("60s is in range");
        assert_eq!(params, CancelAllOrdersAfterParams { timeout: 60 });
    }

    #[test]
    fn cancel_order_requires_at_least_one_identifier() {
        expect_refusal(
            CancelOrderParams::try_from(CancelOrder::default()),
            "At least one",
        );
    }

    #[test]
    fn batch_cancel_requires_two_to_fifty_identifiers() {
        expect_refusal(
            BatchCancelParams::try_from(BatchCancel {
                orders: vec!["OID-1".into()],
                cl_ord_id: vec![],
            }),
            "2-50",
        );
    }

    #[test]
    fn batch_add_requires_two_to_fifteen_orders() {
        let one_order = BatchAdd {
            symbol: "BTC/USD".into(),
            orders: r#"[{"order_type":"limit","side":"buy","order_qty":1,"limit_price":1}]"#.into(),
            validate: false,
            deadline: None,
        };
        expect_refusal(BatchAddParams::try_from(one_order), "2-15");
    }

    #[test]
    fn batch_add_rejects_malformed_orders_json() {
        let bad_json = BatchAdd {
            symbol: "BTC/USD".into(),
            orders: "not json".into(),
            validate: false,
            deadline: None,
        };
        expect_refusal(BatchAddParams::try_from(bad_json), "Invalid orders JSON");
    }
}
