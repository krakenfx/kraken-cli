//! The account's event history: executions, order and trigger lifecycles, and
//! the account log, from the history API.

use super::{Api, Endpoint, Method, Private, sealed::Sealed};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// The window and order shared by every history query. Bounds are passed as
/// given; Kraken parses them.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct HistoryWindow {
    pub since: Option<String>,
    pub before: Option<String>,
    /// `asc` or `desc`.
    pub sort: Option<String>,
}

/// `GET /executions`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExecutionsQuery {
    #[serde(flatten)]
    pub window: HistoryWindow,
}

/// `GET /orders`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OrderEventsQuery {
    #[serde(flatten)]
    pub window: HistoryWindow,
}

/// `GET /triggers`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TriggerEventsQuery {
    #[serde(flatten)]
    pub window: HistoryWindow,
}

/// `GET /accountlogcsv`: the whole account log as CSV.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AccountLogCsv {
    #[serde(flatten)]
    pub window: HistoryWindow,
}

/// A page of history, newest first unless sorted otherwise.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage<E> {
    pub account_uid: String,
    pub len: u64,
    pub server_time: Option<String>,
    pub elements: Vec<HistoryElement<E>>,
    /// Pass back for the next page.
    pub continuation_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryElement<E> {
    pub uid: String,
    /// Unix milliseconds.
    pub timestamp: i64,
    pub event: E,
}

/// An execution: `{"execution": {"execution": …, "takerReducedQuantity": …}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEvent {
    pub execution: ExecutionDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionDetails {
    pub execution: Execution,
    pub taker_reduced_quantity: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Execution {
    pub uid: String,
    pub order: HistoryOrder,
    pub timestamp: i64,
    #[serde(with = "crate::decimal::string")]
    pub quantity: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub price: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub mark_price: Decimal,
    pub execution_type: ExecutionType,
    pub limit_filled: bool,
    pub old_taker_order: Option<HistoryOrder>,
    #[serde(with = "crate::decimal::string")]
    pub usd_value: Decimal,
    #[serialize_always]
    pub order_data: Option<ExecutionOrderData>,
    pub partner_trade_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionType {
    Maker,
    Taker,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionOrderData {
    #[serde(with = "crate::decimal::string")]
    pub fee: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub position_size: Decimal,
    pub fee_calculation_info: FeeCalculation,
    #[serde(default, with = "crate::decimal::string::option")]
    pub realized_pnl: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeCalculation {
    #[serde(with = "crate::decimal::string")]
    pub percentage_fee: Decimal,
    #[serde(default, with = "crate::decimal::string::option")]
    pub user_fee_discount_applied: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub market_share_rebate_credited: Option<Decimal>,
}

/// An order as the history API records it. Decimals are strings here, unlike
/// the trading API's numbers.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryOrder {
    pub uid: String,
    pub account_uid: String,
    pub tradeable: String,
    pub position_uid: Option<String>,
    pub direction: Direction,
    #[serde(with = "crate::decimal::string")]
    pub quantity: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub filled: Decimal,
    pub timestamp: i64,
    #[serde(with = "crate::decimal::string")]
    pub limit_price: Decimal,
    pub order_type: HistoryOrderType,
    pub client_id: String,
    pub reduce_only: bool,
    pub last_update_timestamp: i64,
    pub spot_data: Option<SpotOrderData>,
    pub parent_of_group_id: Option<String>,
    pub member_of_group_id: Option<String>,
    pub partner_order_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    Buy,
    Sell,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HistoryOrderType {
    Limit,
    IoC,
    Post,
    Liquidation,
    Assignment,
    Stop,
    Unwind,
    Market,
    Block,
    CoveredLiquidation,
    HedgeImmediateOrCancel,
    HedgeAssignment,
    FillOrKill,
    Rfq,
    PartialLiquidation,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotOrderData {
    pub margin: bool,
    pub fee_preference: Option<BaseOrQuote>,
    pub quantity_preference: BaseOrQuote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BaseOrQuote {
    Base,
    Quote,
    Unknown,
}

/// One change to an order, tagged by Kraken's event name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderLifecycleEvent {
    OrderPlaced(OrderPlaced),
    OrderUpdated(OrderUpdated),
    OrderRejected(OrderRejected),
    OrderCancelled(OrderCancelled),
    OrderNotFound(OrderNotFound),
    OrderEditRejected(OrderEditRejected),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderPlaced {
    pub order: HistoryOrder,
    pub reason: String,
    pub reduced_quantity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderUpdated {
    pub old_order: HistoryOrder,
    pub new_order: HistoryOrder,
    pub reason: String,
    #[serde(with = "crate::decimal::string")]
    pub reduced_quantity: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderRejected {
    pub order: HistoryOrder,
    pub order_error: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderCancelled {
    pub order: HistoryOrder,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderNotFound {
    pub account_uid: String,
    pub order_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderEditRejected {
    pub old_order: HistoryOrder,
    pub attempted_order: HistoryOrder,
    pub order_error: String,
}

/// One change to a trigger (stop) order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerLifecycleEvent {
    OrderTriggerPlaced(TriggerPlaced),
    OrderTriggerCancelled(TriggerCancelled),
    OrderTriggerUpdated(TriggerUpdated),
    OrderTriggerActivated(TriggerActivated),
    OrderTriggerEditRejected(TriggerEditRejected),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerPlaced {
    pub order: HistoryTrigger,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerCancelled {
    pub order: HistoryTrigger,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerUpdated {
    pub old_order_trigger: HistoryTrigger,
    pub new_order_trigger: HistoryTrigger,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerActivated {
    pub order: HistoryTrigger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerEditRejected {
    pub attempted_order_trigger: HistoryTrigger,
    pub old_order_trigger: HistoryTrigger,
    pub reason: String,
    pub order_error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryTrigger {
    pub uid: String,
    pub account_id: u64,
    pub account_uid: String,
    pub tradeable: String,
    pub direction: Direction,
    #[serde(with = "crate::decimal::string")]
    pub quantity: Decimal,
    pub timestamp: i64,
    #[serde(with = "crate::decimal::string")]
    pub limit_price: Decimal,
    pub order_type: String,
    pub client_id: String,
    pub reduce_only: bool,
    pub last_update_timestamp: i64,
    pub trigger_options: HistoryTriggerOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryTriggerOptions {
    #[serde(with = "crate::decimal::string")]
    pub trigger_price: Decimal,
    pub trigger_signal: HistoryTriggerSignal,
    pub trigger_side: HistoryTriggerSide,
    pub trailing_stop_options: Offset,
    pub limit_price_offset: PriceOffset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HistoryTriggerSignal {
    MarkPrice,
    LastPrice,
    SpotPrice,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HistoryTriggerSide {
    Above,
    Below,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Offset {
    #[serde(with = "crate::decimal::string")]
    pub max_deviation: Decimal,
    pub unit: OffsetUnit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceOffset {
    #[serde(with = "crate::decimal::string")]
    pub price_offset: Decimal,
    pub unit: OffsetUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OffsetUnit {
    Percent,
    QuoteCurrency,
    Unknown,
}

macro_rules! history {
    ($request:ty, $path:literal, $response:ty) => {
        impl Sealed for $request {}
        impl Endpoint for $request {
            const PATH: &'static str = $path;
            const METHOD: Method = Method::Get;
            const API: Api = Api::History;
            type Access = Private;
            type Response = $response;
        }
    };
}

history!(ExecutionsQuery, "executions", HistoryPage<ExecutionEvent>);
history!(OrderEventsQuery, "orders", HistoryPage<OrderLifecycleEvent>);
history!(
    TriggerEventsQuery,
    "triggers",
    HistoryPage<TriggerLifecycleEvent>
);
// Never decoded: `FuturesAccount::download` returns the CSV text.
history!(AccountLogCsv, "accountlogcsv", ());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::futures::envelope::decode_fixture;

    #[test]
    fn decodes_the_history_pages() {
        let executions: HistoryPage<ExecutionEvent> =
            decode_fixture("history.get_executions.synthetic.json").unwrap();
        assert_eq!(
            executions.elements[0]
                .event
                .execution
                .execution
                .execution_type,
            ExecutionType::Taker
        );
        let orders: HistoryPage<OrderLifecycleEvent> =
            decode_fixture("history.get_orders.synthetic.json").unwrap();
        assert!(matches!(
            orders.elements[0].event,
            OrderLifecycleEvent::OrderPlaced(_)
        ));
        let _: HistoryPage<TriggerLifecycleEvent> =
            decode_fixture("history.get_triggers.synthetic.json").unwrap();
    }

    #[test]
    fn an_unknown_event_kind_fails_the_decode() {
        let err =
            serde_json::from_str::<OrderLifecycleEvent>(r#"{"OrderVanished":{}}"#).unwrap_err();
        assert!(err.to_string().contains("OrderVanished"), "{err}");
    }
}
