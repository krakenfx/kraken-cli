//! Order entry. Every request here but the status lookup is a write and is
//! never resent after a lost reply.

use super::{
    BatchInstructionStatus, CancelAllStatus, CancelStatus, Delivery, EditStatus, Endpoint, Method,
    OrderError, OrderEvent, OrderLifecycle, OrderPlacementStatus, OrderUpdateReason, Private,
    Reply, sealed::Sealed,
};
use crate::CommaList;
use kraken_core::OrderSide;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize, Serializer};
use serde_with::skip_serializing_none;

/// `POST /sendorder`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOrder {
    pub symbol: String,
    pub side: OrderSide,
    pub size: Decimal,
    pub kind: FuturesOrderKind,
    pub cli_ord_id: Option<String>,
    pub reduce_only: bool,
}

/// An order type with exactly the prices it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FuturesOrderKind {
    Market,
    Limit {
        price: Decimal,
    },
    /// Rests on the book or is cancelled; never takes.
    PostOnly {
        price: Decimal,
    },
    ImmediateOrCancel {
        price: Decimal,
    },
    FillOrKill {
        price: Decimal,
    },
    /// A stop: triggers at `stop_price`, then a limit at `limit_price` or a
    /// market order without one.
    Stop {
        stop_price: Decimal,
        limit_price: Option<Decimal>,
        trigger: Option<TriggerSignal>,
    },
    TakeProfit {
        stop_price: Decimal,
        limit_price: Option<Decimal>,
        trigger: Option<TriggerSignal>,
    },
    TrailingStop {
        max_deviation: Decimal,
        unit: DeviationUnit,
        trigger: Option<TriggerSignal>,
    },
}

/// The order type token Kraken takes on `sendorder` and in a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderTypeCode {
    Lmt,
    Post,
    Ioc,
    Mkt,
    Stp,
    TakeProfit,
    TrailingStop,
    Fok,
}

/// The price a stop watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSignal {
    Mark,
    Index,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeviationUnit {
    Percent,
    QuoteCurrency,
}

#[skip_serializing_none]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendOrderParams<'a> {
    order_type: OrderTypeCode,
    symbol: &'a str,
    side: OrderSide,
    #[serde(with = "crate::decimal::string")]
    size: Decimal,
    #[serde(with = "crate::decimal::string::option")]
    limit_price: Option<Decimal>,
    #[serde(with = "crate::decimal::string::option")]
    stop_price: Option<Decimal>,
    trigger_signal: Option<TriggerSignal>,
    cli_ord_id: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    reduce_only: bool,
    #[serde(with = "crate::decimal::string::option")]
    trailing_stop_max_deviation: Option<Decimal>,
    trailing_stop_deviation_unit: Option<DeviationUnit>,
}

impl<'a> From<&'a SendOrder> for SendOrderParams<'a> {
    fn from(order: &'a SendOrder) -> Self {
        // Exhaustive: a field added to the order must reach the request.
        let SendOrder {
            symbol,
            side,
            size,
            kind,
            cli_ord_id,
            reduce_only,
        } = order;
        let mut params = Self {
            order_type: OrderTypeCode::Mkt,
            symbol,
            side: *side,
            size: *size,
            limit_price: None,
            stop_price: None,
            trigger_signal: None,
            cli_ord_id: cli_ord_id.as_deref(),
            reduce_only: *reduce_only,
            trailing_stop_max_deviation: None,
            trailing_stop_deviation_unit: None,
        };
        match *kind {
            FuturesOrderKind::Market => {}
            FuturesOrderKind::Limit { price } => {
                (params.order_type, params.limit_price) = (OrderTypeCode::Lmt, Some(price))
            }
            FuturesOrderKind::PostOnly { price } => {
                (params.order_type, params.limit_price) = (OrderTypeCode::Post, Some(price))
            }
            FuturesOrderKind::ImmediateOrCancel { price } => {
                (params.order_type, params.limit_price) = (OrderTypeCode::Ioc, Some(price));
            }
            FuturesOrderKind::FillOrKill { price } => {
                (params.order_type, params.limit_price) = (OrderTypeCode::Fok, Some(price))
            }
            FuturesOrderKind::Stop {
                stop_price,
                limit_price,
                trigger,
            } => {
                params.order_type = OrderTypeCode::Stp;
                (params.stop_price, params.limit_price, params.trigger_signal) =
                    (Some(stop_price), limit_price, trigger);
            }
            FuturesOrderKind::TakeProfit {
                stop_price,
                limit_price,
                trigger,
            } => {
                params.order_type = OrderTypeCode::TakeProfit;
                (params.stop_price, params.limit_price, params.trigger_signal) =
                    (Some(stop_price), limit_price, trigger);
            }
            FuturesOrderKind::TrailingStop {
                max_deviation,
                unit,
                trigger,
            } => {
                params.order_type = OrderTypeCode::TrailingStop;
                params.trailing_stop_max_deviation = Some(max_deviation);
                params.trailing_stop_deviation_unit = Some(unit);
                params.trigger_signal = trigger;
            }
        }
        params
    }
}

impl Serialize for SendOrder {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        SendOrderParams::from(self).serialize(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sent {
    pub send_status: SendStatus,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendStatus {
    #[serde(rename = "order_id")]
    pub order_id: Option<String>,
    pub cli_ord_id: Option<String>,
    pub status: OrderPlacementStatus,
    pub received_time: Option<String>,
    pub order_events: Option<Vec<OrderEvent>>,
}

/// Which order a modification names: Kraken's id or the caller's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum FuturesOrderRef {
    #[serde(rename = "orderId")]
    OrderId(String),
    #[serde(rename = "cliOrdId")]
    ClientOrderId(String),
}

/// `POST /editorder`.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOrder {
    #[serde(flatten)]
    pub order: FuturesOrderRef,
    #[serde(with = "crate::decimal::string::option")]
    pub size: Option<Decimal>,
    #[serde(with = "crate::decimal::string::option")]
    pub limit_price: Option<Decimal>,
    #[serde(with = "crate::decimal::string::option")]
    pub stop_price: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edited {
    pub edit_status: EditOutcome,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditOutcome {
    pub status: EditStatus,
    pub order_id: Option<String>,
    pub cli_ord_id: Option<String>,
    pub received_time: Option<String>,
    pub order_events: Vec<OrderEvent>,
}

/// `POST /cancelorder`. The order id goes as `order_id`, unlike the edit's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum CancelOrder {
    #[serde(rename = "order_id")]
    OrderId(String),
    #[serde(rename = "cliOrdId")]
    ClientOrderId(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cancelled {
    pub cancel_status: CancelOutcome,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelOutcome {
    pub status: CancelStatus,
    #[serde(rename = "order_id")]
    pub order_id: Option<String>,
    pub cli_ord_id: Option<String>,
    pub received_time: Option<String>,
    pub order_events: Option<Vec<OrderEvent>>,
}

/// `POST /cancelallorders`: every open order, or a contract's.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CancelAllOrders {
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllCancelled {
    pub cancel_status: CancelAllOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelAllOutcome {
    pub received_time: String,
    pub cancel_only: String,
    pub status: CancelAllStatus,
    pub cancelled_orders: Vec<CancelledOrder>,
    pub order_events: Vec<OrderEvent>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelledOrder {
    #[serde(rename = "order_id")]
    pub order_id: String,
    pub cli_ord_id: Option<String>,
}

/// `POST /cancelallordersafter`: the dead man's switch; `0` disarms it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelAllOrdersAfter {
    pub timeout: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadManSwitch {
    pub status: DeadManSwitchTimes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeadManSwitchTimes {
    pub current_time: String,
    pub trigger_time: String,
}

/// `POST /batchorder`: sends, edits and cancels in one request, as Kraken's
/// `json` parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchOrder {
    pub instructions: BatchInstructions,
}

impl Serialize for BatchOrder {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Params {
            json: String,
        }
        let json = serde_json::to_string(&self.instructions).map_err(serde::ser::Error::custom)?;
        Params { json }.serialize(s)
    }
}

/// The batch as the caller writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchInstructions {
    pub batch_order: Vec<BatchInstruction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "order", rename_all = "snake_case")]
pub enum BatchInstruction {
    Send(BatchSend),
    Edit(BatchEdit),
    Cancel(BatchCancel),
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSend {
    /// Ties this instruction to its result in the reply.
    #[serde(rename = "order_tag")]
    pub order_tag: String,
    pub order_type: OrderTypeCode,
    pub symbol: String,
    pub side: OrderSide,
    #[serde(with = "crate::decimal::json")]
    pub size: Decimal,
    #[serde(default, with = "crate::decimal::json::option")]
    pub limit_price: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub stop_price: Option<Decimal>,
    pub cli_ord_id: Option<String>,
    pub trigger_signal: Option<TriggerSignal>,
    pub reduce_only: Option<bool>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub trailing_stop_max_deviation: Option<Decimal>,
    pub trailing_stop_deviation_unit: Option<DeviationUnit>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchEdit {
    #[serde(rename = "order_id")]
    pub order_id: Option<String>,
    pub cli_ord_id: Option<String>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub size: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub limit_price: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub stop_price: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub trailing_stop_max_deviation: Option<Decimal>,
    pub trailing_stop_deviation_unit: Option<DeviationUnit>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchCancel {
    #[serde(rename = "order_id")]
    pub order_id: Option<String>,
    pub cli_ord_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchPlaced {
    pub batch_status: Vec<BatchResult>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchResult {
    pub status: BatchInstructionStatus,
    #[serde(rename = "order_tag")]
    pub order_tag: Option<String>,
    #[serde(rename = "order_id")]
    pub order_id: Option<String>,
    pub cli_ord_id: Option<String>,
    pub date_time_received: Option<String>,
    pub order_events: Vec<OrderEvent>,
}

/// `POST /orders/status`: up to 100 orders by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrdersStatusQuery {
    pub order_ids: CommaList<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdersStatus {
    pub orders: Vec<OrderStatusDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderStatusDetails {
    pub order: CachedOrder,
    pub status: OrderLifecycle,
    pub update_reason: Option<OrderUpdateReason>,
    pub error: Option<OrderError>,
}

/// An order as the engine's order cache holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedOrder {
    #[serde(rename = "type")]
    pub kind: CachedOrderKind,
    pub order_id: String,
    pub cli_ord_id: Option<String>,
    pub symbol: String,
    pub side: OrderSide,
    #[serde(default, with = "crate::decimal::json::option")]
    pub quantity: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub filled: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub limit_price: Option<Decimal>,
    pub reduce_only: bool,
    pub timestamp: String,
    pub last_update_timestamp: String,
    pub algo_id: Option<String>,
    pub price_trigger_options: Option<PriceTriggerOptions>,
    pub trigger_time: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CachedOrderKind {
    TriggerOrder,
    Order,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceTriggerOptions {
    #[serde(with = "crate::decimal::json")]
    pub trigger_price: Decimal,
    pub trigger_side: TriggerDirection,
    pub trigger_signal: CachedTriggerSignal,
    pub trailing_stop_options: Option<TrailingStopOptions>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TriggerDirection {
    TriggerAbove,
    TriggerBelow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CachedTriggerSignal {
    MarkPrice,
    LastPrice,
    SpotPrice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrailingStopOptions {
    #[serde(with = "crate::decimal::json")]
    pub max_deviation: Decimal,
    pub unit: DeviationUnit,
}

macro_rules! order_entry {
    ($request:ty, $path:literal, $response:ty) => {
        impl Sealed for $request {}
        impl Endpoint for $request {
            const PATH: &'static str = $path;
            const METHOD: Method = Method::Post;
            type Access = Private;
            type Response = Reply<$response>;
        }
    };
}

order_entry!(SendOrder, "sendorder", Sent);
order_entry!(EditOrder, "editorder", Edited);
order_entry!(CancelOrder, "cancelorder", Cancelled);
order_entry!(CancelAllOrders, "cancelallorders", AllCancelled);
order_entry!(CancelAllOrdersAfter, "cancelallordersafter", DeadManSwitch);
order_entry!(BatchOrder, "batchorder", BatchPlaced);

impl Sealed for OrdersStatusQuery {}
impl Endpoint for OrdersStatusQuery {
    const PATH: &'static str = "orders/status";
    const METHOD: Method = Method::Post;
    type Access = Private;
    type Response = Reply<OrdersStatus>;

    /// A read Kraken happens to take as a POST.
    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::futures::{Delivery, envelope::decode_fixture};
    use rust_decimal_macros::dec;

    #[test]
    fn decodes_the_trading_replies() {
        let sent: Reply<Sent> = decode_fixture("trading.post_sendorder.json").unwrap();
        assert_eq!(sent.body.send_status.status, OrderPlacementStatus::Placed);
        let _: Reply<Edited> = decode_fixture("trading.post_editorder.json").unwrap();
        let _: Reply<Cancelled> = decode_fixture("trading.post_cancelorder.json").unwrap();
        let _: Reply<AllCancelled> = decode_fixture("trading.post_cancelallorders.json").unwrap();
        let _: Reply<DeadManSwitch> =
            decode_fixture("trading.post_cancelallordersafter.json").unwrap();
        let _: Reply<BatchPlaced> = decode_fixture("trading.post_batchorder.json").unwrap();
        let _: Reply<OrdersStatus> =
            decode_fixture("trading.post_orders_status.synthetic.json").unwrap();
    }

    #[test]
    fn a_limit_order_sends_exactly_its_fields() {
        let order = SendOrder {
            symbol: "PF_XBTUSD".into(),
            side: OrderSide::Buy,
            size: dec!(1),
            kind: FuturesOrderKind::Limit { price: dec!(50000) },
            cli_ord_id: None,
            reduce_only: false,
        };
        assert_eq!(
            crate::futures::form::encode(&order).unwrap(),
            "orderType=lmt&symbol=PF_XBTUSD&side=buy&size=1&limitPrice=50000"
        );
        assert_eq!(order.delivery(), Delivery::AtMostOnce);
    }

    #[test]
    fn a_trailing_stop_sends_its_deviation_in_capitals() {
        let order = SendOrder {
            symbol: "PF_XBTUSD".into(),
            side: OrderSide::Sell,
            size: dec!(1),
            kind: FuturesOrderKind::TrailingStop {
                max_deviation: dec!(2),
                unit: DeviationUnit::Percent,
                trigger: Some(TriggerSignal::Mark),
            },
            cli_ord_id: None,
            reduce_only: true,
        };
        let form = crate::futures::form::encode(&order).unwrap();
        assert!(form.contains("trailingStopDeviationUnit=PERCENT"), "{form}");
        assert!(form.contains("triggerSignal=mark"), "{form}");
    }

    #[test]
    fn a_misspelt_batch_key_is_refused() {
        let err = serde_json::from_str::<BatchInstructions>(r#"{"batchOrders":[]}"#).unwrap_err();
        assert!(err.to_string().contains("batchOrders"), "{err}");
    }

    #[test]
    fn a_batch_is_sent_as_its_json_parameter() {
        let batch = BatchOrder {
            instructions: serde_json::from_str(
                r#"{"batchOrder":[{"order":"cancel","order_id":"abc"}]}"#,
            )
            .unwrap(),
        };
        let form = crate::futures::form::encode(&batch).unwrap();
        assert!(form.starts_with("json=%7B%22batchOrder%22"), "{form}");
    }
}
