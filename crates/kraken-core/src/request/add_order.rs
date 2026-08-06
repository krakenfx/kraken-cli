//! `add_order` — place a single order. Owns the trigger and conditional-close
//! templates, plus [`AddOrderResult`], the per-order reply shared with
//! [`batch_add`](super::batch_add).
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/add_order>.
//!
//! Required: `order_type`, `side`, `symbol`; everything else optional and omitted when
//! unset. Quantity is `order_qty` (base) *or* `cash_order_qty` (quote, market buys),
//! hence both optional; `validate: true` dry-runs. Deprecated wire fields superseded by
//! `triggers` are omitted. The result carries `order_id` (absent in `validate` mode),
//! the echoed client ids, and optional `warnings`.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use super::decimal_wire;
use super::value::{FeePreference, PriceType, StpType, TimeInForce, TriggerReference};
use crate::{MethodReply, OrderSide, OrderType, Request};

/// Trigger price parameters for triggered order types. `price` is required by the type
/// — a priceless trigger is unrepresentable; `reference` defaults to `last` and
/// `price_type` to `static` server-side. Deserialized from user-supplied `batch_add`
/// JSON, where an unknown key is a typo that must fail loud.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Triggers {
    pub reference: Option<TriggerReference>,
    #[serde(with = "decimal_wire")]
    pub price: Decimal,
    pub price_type: Option<PriceType>,
}

/// The close-order template spawned once the primary order fills — the request-side
/// twin of the executions channel's `ContingentOrder` echo. `order_type` is required:
/// a template with no type is unrepresentable. The docs list only the trigger-capable
/// subset of order types; decoding with the full [`OrderType`] keeps one order-type
/// vocabulary. Deserialized from user-supplied `batch_add` JSON, where an unknown key
/// is a typo that must fail loud.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionalClose {
    pub order_type: OrderType,
    #[serde(
        default,
        serialize_with = "decimal_wire::opt",
        deserialize_with = "decimal_wire::de_opt"
    )]
    pub limit_price: Option<Decimal>,
    pub limit_price_type: Option<PriceType>,
    #[serde(
        default,
        serialize_with = "decimal_wire::opt",
        deserialize_with = "decimal_wire::de_opt"
    )]
    pub trigger_price: Option<Decimal>,
    pub trigger_price_type: Option<PriceType>,
}

/// `add_order` request params.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AddOrderParams {
    pub order_type: OrderType,
    pub side: OrderSide,
    pub symbol: String,
    #[serde(serialize_with = "decimal_wire::opt")]
    pub order_qty: Option<Decimal>,
    #[serde(serialize_with = "decimal_wire::opt")]
    pub cash_order_qty: Option<Decimal>,
    #[serde(serialize_with = "decimal_wire::opt")]
    pub limit_price: Option<Decimal>,
    pub limit_price_type: Option<PriceType>,
    pub time_in_force: Option<TimeInForce>,
    pub margin: Option<bool>,
    pub post_only: Option<bool>,
    pub reduce_only: Option<bool>,
    /// Disable market-price protection on a market order.
    pub no_mpp: Option<bool>,
    pub triggers: Option<Triggers>,
    pub conditional: Option<ConditionalClose>,
    pub effective_time: Option<String>,
    pub expire_time: Option<String>,
    pub deadline: Option<String>,
    pub cl_ord_id: Option<String>,
    /// `i32` across every path (requests, acks, executions ingest): the API defines a
    /// userref as a signed int32, so negatives set by other clients stay addressable.
    pub order_userref: Option<i32>,
    #[serde(serialize_with = "decimal_wire::opt")]
    pub display_qty: Option<Decimal>,
    pub fee_preference: Option<FeePreference>,
    pub stp_type: Option<StpType>,
    pub sender_sub_id: Option<String>,
    pub validate: Option<bool>,
}

impl Request for AddOrderParams {
    const METHOD: &'static str = "add_order";
    type Response = AddOrderResult;

    fn extract(reply: MethodReply) -> Option<AddOrderResult> {
        reply.try_as_add_order().flatten()
    }
}

/// `add_order` / `batch_add` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddOrderResult {
    pub order_id: Option<String>,
    pub cl_ord_id: Option<String>,
    pub order_userref: Option<i32>,
    /// Present only on `validate`-mode responses, in place of a real `order_id`.
    pub validation: Option<String>,
    pub warnings: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Wire;

    /// Pins the full outgoing frame: unset options stay off the wire, the token flattens
    /// in first, money serializes digit-exact past f64 precision, and a negative userref
    /// (legal int32 per the API) survives.
    #[test]
    fn add_order_frame_pins_the_wire_shape_with_exact_decimals() {
        let params = AddOrderParams {
            order_type: OrderType::Limit,
            side: OrderSide::Buy,
            symbol: "BTC/USD".into(),
            order_qty: Some("0.123456789012345678".parse().unwrap()),
            cash_order_qty: None,
            limit_price: Some("50000.1".parse().unwrap()),
            limit_price_type: None,
            time_in_force: Some(TimeInForce::Gtc),
            margin: None,
            post_only: Some(true),
            reduce_only: None,
            no_mpp: None,
            triggers: Some(Triggers {
                reference: None,
                price: "49000.5".parse().unwrap(),
                price_type: None,
            }),
            conditional: None,
            effective_time: None,
            expire_time: None,
            deadline: None,
            cl_ord_id: None,
            order_userref: Some(-7),
            display_qty: None,
            fee_preference: None,
            stp_type: None,
            sender_sub_id: None,
            validate: Some(true),
        };
        let json = serde_json::to_string(&Wire::new(
            AddOrderParams::METHOD,
            Some("tok-1"),
            &params,
            Some(5),
        ))
        .unwrap();
        assert_eq!(
            json,
            r#"{"method":"add_order","params":{"token":"tok-1","order_type":"limit","side":"buy","symbol":"BTC/USD","order_qty":0.123456789012345678,"limit_price":50000.1,"time_in_force":"gtc","post_only":true,"triggers":{"price":49000.5},"order_userref":-7,"validate":true},"req_id":5}"#
        );
    }
}
