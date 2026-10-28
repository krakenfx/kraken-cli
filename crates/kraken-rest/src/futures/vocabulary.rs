//! Futures vocabulary shared by several endpoints, from Kraken's API
//! description. The long status and error enums list the description's values
//! verbatim; a token outside them fails the decode.

use kraken_core::OrderSide;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// A Futures number, published as Kraken sent it: integer or float.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Number(#[serde(with = "crate::decimal::json")] pub Decimal);

/// An order as the matching engine reports it in an event.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineOrder {
    pub order_id: String,
    #[serialize_always]
    pub cli_ord_id: Option<String>,
    #[serde(rename = "type")]
    pub kind: EngineOrderType,
    pub symbol: String,
    pub side: OrderSide,
    #[serde(with = "crate::decimal::json")]
    pub quantity: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub filled: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub limit_price: Decimal,
    pub reduce_only: bool,
    pub timestamp: String,
    pub last_update_timestamp: String,
    pub algo_id: Option<String>,
}

/// A trigger (stop) order as the engine reports it in an event.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineTrigger {
    pub uid: String,
    #[serialize_always]
    pub client_id: Option<String>,
    #[serde(rename = "type")]
    pub kind: EngineOrderType,
    pub symbol: String,
    pub side: OrderSide,
    #[serialize_always]
    #[serde(default, with = "crate::decimal::json::option")]
    pub quantity: Option<Decimal>,
    #[serialize_always]
    #[serde(default, with = "crate::decimal::json::option")]
    pub limit_price: Option<Decimal>,
    #[serialize_always]
    #[serde(default, with = "crate::decimal::json::option")]
    pub trigger_price: Option<Decimal>,
    #[serialize_always]
    pub trigger_side: Option<TriggerSide>,
    #[serialize_always]
    pub trigger_signal: Option<EngineTriggerSignal>,
    pub reduce_only: bool,
    pub timestamp: String,
    pub last_update_timestamp: String,
    pub start_time: Option<String>,
}

/// One thing that happened to an order while the engine handled a request.
/// The trigger variants share the order variants' tags, so the fields, not the
/// tag, say which one this is.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderEvent {
    #[serde(rename = "type")]
    pub kind: OrderEventKind,
    pub uid: Option<String>,
    // Kinds that require a nullable field send `null`, the others omit it, so
    // these keep all three states: absent, `Some(None)` for null, a value.
    #[serde(default, with = "serde_with::rust::double_option")]
    pub order: Option<Option<EngineOrder>>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub order_trigger: Option<Option<EngineTrigger>>,
    pub old: Option<EngineOrder>,
    pub new: Option<EngineOrder>,
    pub reason: Option<OrderError>,
    pub execution_id: Option<String>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub price: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub amount: Option<Decimal>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub order_prior_edit: Option<Option<EngineOrder>>,
    pub order_prior_execution: Option<EngineOrder>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub reduced_quantity: Option<Option<Number>>,
    #[serde(default, with = "serde_with::rust::double_option")]
    pub taker_reduced_quantity: Option<Option<Number>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderEventKind {
    Place,
    Cancel,
    Edit,
    Reject,
    Execution,
}

/// The engine's order type in events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineOrderType {
    Lmt,
    Ioc,
    Post,
    Liquidation,
    Assignment,
    Stp,
    Unwind,
    Block,
    Fok,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSide {
    TriggerAbove,
    TriggerBelow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineTriggerSignal {
    MarkPrice,
    LastPrice,
    SpotPrice,
}

/// Option sensitivities.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Greeks {
    #[serde(with = "crate::decimal::json")]
    pub iv: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub delta: Decimal,
    #[serialize_always]
    #[serde(default, with = "crate::decimal::json::option")]
    pub gamma: Option<Decimal>,
    #[serialize_always]
    #[serde(default, with = "crate::decimal::json::option")]
    pub vega: Option<Decimal>,
    #[serialize_always]
    #[serde(default, with = "crate::decimal::json::option")]
    pub theta: Option<Decimal>,
    #[serialize_always]
    #[serde(default, with = "crate::decimal::json::option")]
    pub rho: Option<Decimal>,
}

/// A margin step: from `contracts` (or `numNonContractUnits`) up, these rates.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarginLevel {
    #[serde(default)]
    pub contracts: Option<i64>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub num_non_contract_units: Option<Decimal>,
    #[serde(with = "crate::decimal::json")]
    pub initial_margin: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub maintenance_margin: Decimal,
}

/// What happened to a placed order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderPlacementStatus {
    #[serde(rename = "placed")]
    Placed,
    #[serde(rename = "partiallyFilled")]
    PartiallyFilled,
    #[serde(rename = "filled")]
    Filled,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "edited")]
    Edited,
    #[serde(rename = "marketSuspended")]
    MarketSuspended,
    #[serde(rename = "marketInactive")]
    MarketInactive,
    #[serde(rename = "invalidPrice")]
    InvalidPrice,
    #[serde(rename = "invalidSize")]
    InvalidSize,
    #[serde(rename = "tooManySmallOrders")]
    TooManySmallOrders,
    #[serde(rename = "insufficientAvailableFunds")]
    InsufficientAvailableFunds,
    #[serde(rename = "wouldCauseLiquidation")]
    WouldCauseLiquidation,
    #[serde(rename = "clientOrderIdAlreadyExist")]
    ClientOrderIdAlreadyExist,
    #[serde(rename = "clientOrderIdTooBig")]
    ClientOrderIdTooBig,
    #[serde(rename = "maxPositionViolation")]
    MaxPositionViolation,
    #[serde(rename = "outsidePriceCollar")]
    OutsidePriceCollar,
    #[serde(rename = "wouldIncreasePriceDislocation")]
    WouldIncreasePriceDislocation,
    #[serde(rename = "notFound")]
    NotFound,
    #[serde(rename = "orderForEditNotAStop")]
    OrderForEditNotAStop,
    #[serde(rename = "orderForEditNotFound")]
    OrderForEditNotFound,
    #[serde(rename = "postWouldExecute")]
    PostWouldExecute,
    #[serde(rename = "iocWouldNotExecute")]
    IocWouldNotExecute,
    #[serde(rename = "selfFill")]
    SelfFill,
    #[serde(rename = "wouldNotReducePosition")]
    WouldNotReducePosition,
    #[serde(rename = "marketIsPostOnly")]
    MarketIsPostOnly,
    #[serde(rename = "tooManyOrders")]
    TooManyOrders,
    #[serde(rename = "fixedLeverageTooHigh")]
    FixedLeverageTooHigh,
    #[serde(rename = "clientOrderIdInvalid")]
    ClientOrderIdInvalid,
    #[serde(rename = "cannotEditTriggerPriceOfTrailingStop")]
    CannotEditTriggerPriceOfTrailingStop,
    #[serde(rename = "cannotEditLimitPriceOfTrailingStop")]
    CannotEditLimitPriceOfTrailingStop,
    #[serde(rename = "wouldProcessAfterSpecifiedTime")]
    WouldProcessAfterSpecifiedTime,
    #[serde(rename = "invalidDomainConfiguration")]
    InvalidDomainConfiguration,
    #[serde(rename = "editAlreadyHeld")]
    EditAlreadyHeld,
}

/// What happened to an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EditStatus {
    #[serde(rename = "edited")]
    Edited,
    #[serde(rename = "filled")]
    Filled,
    #[serde(rename = "invalidSize")]
    InvalidSize,
    #[serde(rename = "invalidPrice")]
    InvalidPrice,
    #[serde(rename = "insufficientAvailableFunds")]
    InsufficientAvailableFunds,
    #[serde(rename = "selfFill")]
    SelfFill,
    #[serde(rename = "tooManySmallOrders")]
    TooManySmallOrders,
    #[serde(rename = "outsidePriceCollar")]
    OutsidePriceCollar,
    #[serde(rename = "postWouldExecute")]
    PostWouldExecute,
    #[serde(rename = "iocWouldNotExecute")]
    IocWouldNotExecute,
    #[serde(rename = "wouldNotReducePosition")]
    WouldNotReducePosition,
    #[serde(rename = "orderForEditNotFound")]
    OrderForEditNotFound,
    #[serde(rename = "orderForEditNotAStop")]
    OrderForEditNotAStop,
    #[serde(rename = "editAlreadyHeld")]
    EditAlreadyHeld,
}

/// What happened to a cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CancelStatus {
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "filled")]
    Filled,
    #[serde(rename = "notFound")]
    NotFound,
}

/// What a cancel-all found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CancelAllStatus {
    #[serde(rename = "noOrdersToCancel")]
    NoOrdersToCancel,
    #[serde(rename = "cancelled")]
    Cancelled,
}

/// What happened to one batch instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BatchInstructionStatus {
    #[serde(rename = "placed")]
    Placed,
    #[serde(rename = "edited")]
    Edited,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "invalidOrderType")]
    InvalidOrderType,
    #[serde(rename = "invalidSide")]
    InvalidSide,
    #[serde(rename = "invalidSize")]
    InvalidSize,
    #[serde(rename = "invalidPrice")]
    InvalidPrice,
    #[serde(rename = "insufficientAvailableFunds")]
    InsufficientAvailableFunds,
    #[serde(rename = "selfFill")]
    SelfFill,
    #[serde(rename = "tooManySmallOrders")]
    TooManySmallOrders,
    #[serde(rename = "marketSuspended")]
    MarketSuspended,
    #[serde(rename = "marketInactive")]
    MarketInactive,
    #[serde(rename = "clientOrderIdAlreadyExist")]
    ClientOrderIdAlreadyExist,
    #[serde(rename = "clientOrderIdTooLong")]
    ClientOrderIdTooLong,
    #[serde(rename = "outsidePriceCollar")]
    OutsidePriceCollar,
    #[serde(rename = "postWouldExecute")]
    PostWouldExecute,
    #[serde(rename = "iocWouldNotExecute")]
    IocWouldNotExecute,
    #[serde(rename = "tooManyOrders")]
    TooManyOrders,
}

/// Why the engine refused an order or trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderError {
    #[serde(rename = "MARKET_SUSPENDED")]
    MarketSuspended,
    #[serde(rename = "MARKET_NOT_FOUND")]
    MarketNotFound,
    #[serde(rename = "INVALID_PRICE")]
    InvalidPrice,
    #[serde(rename = "INVALID_QUANTITY")]
    InvalidQuantity,
    #[serde(rename = "SMALL_ORDER_LIMIT_EXCEEDED")]
    SmallOrderLimitExceeded,
    #[serde(rename = "INSUFFICIENT_MARGIN")]
    InsufficientMargin,
    #[serde(rename = "WOULD_CAUSE_LIQUIDATION")]
    WouldCauseLiquidation,
    #[serde(rename = "CLIENT_ORDER_ID_IN_USE")]
    ClientOrderIdInUse,
    #[serde(rename = "CLIENT_ORDER_ID_TOO_LONG")]
    ClientOrderIdTooLong,
    #[serde(rename = "MAX_POSITION_EXCEEDED")]
    MaxPositionExceeded,
    #[serde(rename = "PRICE_COLLAR")]
    PriceCollar,
    #[serde(rename = "PRICE_DISLOCATION")]
    PriceDislocation,
    #[serde(rename = "EDIT_HAS_NO_EFFECT")]
    EditHasNoEffect,
    #[serde(rename = "ORDER_FOR_CANCELLATION_NOT_FOUND")]
    OrderForCancellationNotFound,
    #[serde(rename = "ORDER_FOR_EDIT_NOT_FOUND")]
    OrderForEditNotFound,
    #[serde(rename = "ORDER_CANNOT_HAVE_TRIGGER_PRICE")]
    OrderCannotHaveTriggerPrice,
    #[serde(rename = "POST_WOULD_EXECUTE")]
    PostWouldExecute,
    #[serde(rename = "IOC_WOULD_NOT_EXECUTE")]
    IocWouldNotExecute,
    #[serde(rename = "WOULD_EXECUTE_SELF")]
    WouldExecuteSelf,
    #[serde(rename = "WOULD_NOT_REDUCE_POSITION")]
    WouldNotReducePosition,
    #[serde(rename = "REJECTED_AFTER_EXECUTION")]
    RejectedAfterExecution,
    #[serde(rename = "MARKET_IS_POST_ONLY")]
    MarketIsPostOnly,
    #[serde(rename = "ORDER_LIMIT_EXCEEDED")]
    OrderLimitExceeded,
    #[serde(rename = "FIXED_LEVERAGE_TOO_HIGH")]
    FixedLeverageTooHigh,
    #[serde(rename = "CANNOT_EDIT_TRIGGER_PRICE_OF_TRAILING_STOP")]
    CannotEditTriggerPriceOfTrailingStop,
    #[serde(rename = "CANNOT_EDIT_LIMIT_PRICE_OF_TRAILING_STOP")]
    CannotEditLimitPriceOfTrailingStop,
    #[serde(rename = "TRAILING_STOP_ORDER_LIMIT_EXCEEDED")]
    TrailingStopOrderLimitExceeded,
    #[serde(rename = "TRAILING_STOP_PERCENT_DEVIATION_EXCEEDS_MAX_DECIMAL_PLACES")]
    TrailingStopPercentDeviationExceedsMaxDecimalPlaces,
    #[serde(rename = "TRAILING_STOP_QUOTE_DEVIATION_NOT_MULTIPLE_OF_TICK_SIZE")]
    TrailingStopQuoteDeviationNotMultipleOfTickSize,
    #[serde(rename = "TRAILING_STOP_MAX_DEVIATION_TOO_LARGE")]
    TrailingStopMaxDeviationTooLarge,
    #[serde(rename = "TRAILING_STOP_MAX_DEVIATION_TOO_SMALL")]
    TrailingStopMaxDeviationTooSmall,
    #[serde(rename = "INSUFFICIENT_HEADROOM_AROUND_CURRENT_PRICE_TO_EDIT_TRAILING_STOP")]
    InsufficientHeadroomAroundCurrentPriceToEditTrailingStop,
    #[serde(rename = "NO_REFERENCE_PRICE_AVAILABLE_FOR_CALCULATING_TRAILING_STOP_TRIGGER_PRICE")]
    NoReferencePriceAvailableForCalculatingTrailingStopTriggerPrice,
    #[serde(rename = "INSUFFICIENT_CLOSING_MARGIN")]
    InsufficientClosingMargin,
    #[serde(rename = "LIMIT_PRICE_SET_AS_ABSOLUTE_AND_RELATIVE")]
    LimitPriceSetAsAbsoluteAndRelative,
    #[serde(rename = "LIMIT_PRICE_OFFSET_VALUE_INVALID")]
    LimitPriceOffsetValueInvalid,
    #[serde(rename = "LIMIT_PRICE_OFFSET_UNIT_INVALID")]
    LimitPriceOffsetUnitInvalid,
    #[serde(rename = "LIMIT_PRICE_OFFSET_MUST_HAVE_VALUE_AND_UNIT")]
    LimitPriceOffsetMustHaveValueAndUnit,
    #[serde(rename = "LIMIT_PRICE_OFFSET_QUOTE_CURRENCY_VALUE_MUST_BE_MULTIPLE_OF_TICK_SIZE")]
    LimitPriceOffsetQuoteCurrencyValueMustBeMultipleOfTickSize,
    #[serde(rename = "LIMIT_PRICE_OFFSET_PERCENT_VALUE_TOO_MANY_DECIMAL_PLACES")]
    LimitPriceOffsetPercentValueTooManyDecimalPlaces,
    #[serde(rename = "LIMIT_PRICE_OFFSET_TOO_HIGH")]
    LimitPriceOffsetTooHigh,
    #[serde(rename = "LIMIT_PRICE_OFFSET_TOO_LOW")]
    LimitPriceOffsetTooLow,
}

/// Where an order is in the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderLifecycle {
    #[serde(rename = "ENTERED_BOOK")]
    EnteredBook,
    #[serde(rename = "FULLY_EXECUTED")]
    FullyExecuted,
    #[serde(rename = "REJECTED")]
    Rejected,
    #[serde(rename = "CANCELLED")]
    Cancelled,
    #[serde(rename = "TRIGGER_PLACED")]
    TriggerPlaced,
    #[serde(rename = "TRIGGER_ACTIVATION_FAILURE")]
    TriggerActivationFailure,
}

/// Why the engine last updated an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderUpdateReason {
    #[serde(rename = "LOADING_MARKET")]
    LoadingMarket,
    #[serde(rename = "NEW_USER_ORDER")]
    NewUserOrder,
    #[serde(rename = "LIQUIDATION_ORDER")]
    LiquidationOrder,
    #[serde(rename = "STOP_ORDER_TRIGGERED")]
    StopOrderTriggered,
    #[serde(rename = "LIMIT_FROM_STOP")]
    LimitFromStop,
    #[serde(rename = "PARTIAL_FILL")]
    PartialFill,
    #[serde(rename = "FULL_FILL")]
    FullFill,
    #[serde(rename = "CANCELLED_BY_USER")]
    CancelledByUser,
    #[serde(rename = "CONTRACT_EXPIRED")]
    ContractExpired,
    #[serde(rename = "NOT_ENOUGH_MARGIN")]
    NotEnoughMargin,
    #[serde(rename = "MARKET_INACTIVE")]
    MarketInactive,
    #[serde(rename = "DEAD_MAN_SWITCH")]
    DeadManSwitch,
    #[serde(rename = "CANCELLED_BY_ADMIN")]
    CancelledByAdmin,
    #[serde(rename = "POST_WOULD_EXECUTE_REASON")]
    PostWouldExecuteReason,
    #[serde(rename = "IOC_WOULD_NOT_EXECUTE_REASON")]
    IocWouldNotExecuteReason,
    #[serde(rename = "WOULD_EXECUTE_SELF_REASON")]
    WouldExecuteSelfReason,
    #[serde(rename = "WOULD_NOT_REDUCE_POSITION")]
    WouldNotReducePosition,
    #[serde(rename = "EDITED_BY_USER")]
    EditedByUser,
    #[serde(rename = "ORDER_FOR_EDIT_NOT_FOUND_REASON")]
    OrderForEditNotFoundReason,
    #[serde(rename = "EXPIRED")]
    Expired,
    #[serde(rename = "TRAILING_STOP_PRICE_UPDATED")]
    TrailingStopPriceUpdated,
    #[serde(rename = "TRAILING_STOP_CANCELLED_AND_REPLACED_BY_ADMIN")]
    TrailingStopCancelledAndReplacedByAdmin,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_required_null_survives_and_an_absent_field_stays_absent() {
        let cancel = json!({"type": "CANCEL", "uid": "u", "order": null});
        let event: OrderEvent = serde_json::from_value(cancel.clone()).unwrap();
        assert_eq!(serde_json::to_value(&event).unwrap(), cancel);
    }
}
