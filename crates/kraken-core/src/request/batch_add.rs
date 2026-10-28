//! `batch_add` — submit many orders for one symbol in a single frame. Each
//! entry is a [`BatchOrderParams`]; the reply is a `Vec<AddOrderResult>`.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/batch_add>. Top level: `symbol`,
//! `orders` (the docs allow 2–15 per batch, enforced server-side), and the
//! optional `validate`/`deadline`. Each order entry mirrors the
//! [`add_order`](super::add_order) params for a single pair.

use super::{
    add_order::{AddOrderResult, ConditionalClose, Triggers},
    decimal_wire,
    value::{FeePreference, PriceType, StpType, TimeInForce},
};
use crate::{MethodReply, OrderSide, OrderType, Request};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// A single order within a `batch_add` request: the
/// [`AddOrderParams`](super::AddOrderParams) fields for one pair, minus the
/// batch-level `symbol`/`validate`/`deadline`. Deserialized
/// from the user-supplied `--orders` JSON, so an unknown key is a typo that
/// must fail loud, never pass through unvalidated.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchOrderParams {
    pub order_type: OrderType,
    pub side: OrderSide,
    #[serde(
        default,
        serialize_with = "decimal_wire::opt",
        deserialize_with = "decimal_wire::de_opt"
    )]
    pub order_qty: Option<Decimal>,
    #[serde(
        default,
        serialize_with = "decimal_wire::opt",
        deserialize_with = "decimal_wire::de_opt"
    )]
    pub cash_order_qty: Option<Decimal>,
    #[serde(
        default,
        serialize_with = "decimal_wire::opt",
        deserialize_with = "decimal_wire::de_opt"
    )]
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
    pub cl_ord_id: Option<String>,
    pub order_userref: Option<i32>,
    #[serde(
        default,
        serialize_with = "decimal_wire::opt",
        deserialize_with = "decimal_wire::de_opt"
    )]
    pub display_qty: Option<Decimal>,
    pub fee_preference: Option<FeePreference>,
    pub stp_type: Option<StpType>,
    pub sender_sub_id: Option<String>,
}

/// `batch_add` request params.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BatchAddParams {
    pub symbol: String,
    pub orders: Vec<BatchOrderParams>,
    pub validate: Option<bool>,
    pub deadline: Option<String>,
}

impl Request for BatchAddParams {
    const METHOD: &'static str = "batch_add";
    type Response = Vec<AddOrderResult>;

    fn extract(reply: MethodReply) -> Option<Vec<AddOrderResult>> {
        reply.try_as_batch_add().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typo_in_a_batch_order_key_fails_loud() {
        let err = serde_json::from_str::<BatchOrderParams>(
            r#"{"order_type":"limit","side":"buy","order_qty":1,"limit_pricee":50000}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("limit_pricee"), "got: {err}");
    }

    #[test]
    fn a_conditional_close_batch_entry_decodes_and_keeps_its_wire_shape() {
        // The docs' close-order template plus no_mpp: previously-valid --orders JSON
        // that the typed field set must accept (deny_unknown_fields rejected
        // `conditional` outright before these fields existed).
        let order: BatchOrderParams = serde_json::from_str(
            r#"{"order_type":"limit","side":"buy","order_qty":1,"limit_price":50000,
                "no_mpp":true,
                "conditional":{"order_type":"stop-loss-limit","trigger_price":48000.5,
                               "limit_price":47900.1}}"#,
        )
        .expect("the documented conditional-close fields decode");
        assert_eq!(order.no_mpp, Some(true));
        let conditional = order.conditional.as_ref().expect("the template decodes");
        assert_eq!(conditional.order_type, OrderType::StopLossLimit);
        let json = serde_json::to_string(&order).expect("serializes");
        assert!(
            json.contains(
                r#""conditional":{"order_type":"stop-loss-limit","limit_price":47900.1,"trigger_price":48000.5}"#
            ),
            "the template must reach the wire in the documented shape: {json}"
        );
    }

    #[test]
    fn a_typo_inside_a_conditional_template_fails_loud() {
        let err = serde_json::from_str::<BatchOrderParams>(
            r#"{"order_type":"limit","side":"buy",
                "conditional":{"order_type":"stop-loss","trigger_pricee":48000}}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("trigger_pricee"), "got: {err}");
    }

    #[test]
    fn batch_order_money_decodes_exactly_never_through_f64() {
        // The exact-outbound-decimals guarantee must hold for the --orders JSON path
        // too: this quantity is the very value decimal_wire's serializer test pins,
        // and the f64 decode route would corrupt it to 0.12345678901234568 before the
        // exact serializer ever ran.
        let order: BatchOrderParams = serde_json::from_str(
            r#"{"order_type":"limit","side":"buy","order_qty":0.123456789012345678,"limit_price":1}"#,
        )
        .expect("a bare JSON number decodes");
        let json = serde_json::to_string(&order).expect("serializes");
        assert!(
            json.contains(r#""order_qty":0.123456789012345678"#),
            "user digits must reach the wire unaltered: {json}"
        );
    }

    #[test]
    fn a_full_batch_order_entry_deserializes_into_typed_fields() {
        let order: BatchOrderParams = serde_json::from_str(
            r#"{"order_type":"limit","side":"buy","order_qty":0.5,"limit_price":50000.1,
                "time_in_force":"gtc","post_only":true,"triggers":{"price":49000.5},
                "order_userref":-7}"#,
        )
        .expect("every documented field decodes typed");
        assert_eq!(order.time_in_force, Some(TimeInForce::Gtc));
        assert_eq!(order.order_userref, Some(-7));
        assert_eq!(
            order.triggers.map(|t| t.price),
            Some("49000.5".parse().unwrap())
        );
    }
}
