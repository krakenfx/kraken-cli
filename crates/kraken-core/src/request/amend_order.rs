//! `amend_order` — modify a resting order in place.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/amend_order>. The order is
//! addressed by `order_id` *or* `cl_ord_id` (hence both optional); the
//! remaining params are the fields being changed — `order_qty`, `display_qty`,
//! `limit_price` (`limit_price_type`), `post_only`, `trigger_price`
//! (`trigger_price_type`) — plus `deadline` and `symbol` (required for
//! non-crypto pairs). The reply's `amend_id` is the only always-present field.

use super::{decimal_wire, value::PriceType};
use crate::{MethodReply, Request};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `amend_order` request params (see the module schema).
#[skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AmendOrderParams {
    pub order_id: Option<String>,
    pub cl_ord_id: Option<String>,
    #[serde(serialize_with = "decimal_wire::opt")]
    pub order_qty: Option<Decimal>,
    #[serde(serialize_with = "decimal_wire::opt")]
    pub display_qty: Option<Decimal>,
    #[serde(serialize_with = "decimal_wire::opt")]
    pub limit_price: Option<Decimal>,
    pub limit_price_type: Option<PriceType>,
    pub post_only: Option<bool>,
    #[serde(serialize_with = "decimal_wire::opt")]
    pub trigger_price: Option<Decimal>,
    pub trigger_price_type: Option<PriceType>,
    pub deadline: Option<String>,
    pub symbol: Option<String>,
}

impl Request for AmendOrderParams {
    const METHOD: &'static str = "amend_order";
    type Response = AmendOrderResult;

    fn extract(reply: MethodReply) -> Option<AmendOrderResult> {
        reply.try_as_amend_order().flatten()
    }
}

/// `amend_order` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AmendOrderResult {
    pub amend_id: String,
    pub order_id: Option<String>,
    pub cl_ord_id: Option<String>,
    pub warnings: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Inbound, Wire};

    /// Pins the full outgoing frame: unset options stay off the wire and money
    /// serializes digit-exact past f64 precision — this is an order-mutating
    /// frame.
    #[test]
    fn amend_order_frame_pins_the_wire_shape_with_exact_decimals() {
        let params = AmendOrderParams {
            order_id: Some("OU22CG-KOAF2-D5B7YK".into()),
            order_qty: Some("0.123456789012345678".parse().expect("a valid decimal")),
            limit_price: Some("50000.1".parse().expect("a valid decimal")),
            post_only: Some(true),
            ..AmendOrderParams::default()
        };
        let json = serde_json::to_string(&Wire::new(
            AmendOrderParams::METHOD,
            Some("tok-1"),
            &params,
            Some(6),
        ))
        .expect("serializes");
        assert_eq!(
            json,
            r#"{"method":"amend_order","params":{"token":"tok-1","order_id":"OU22CG-KOAF2-D5B7YK","order_qty":0.123456789012345678,"limit_price":50000.1,"post_only":true},"req_id":6}"#
        );
    }

    /// The docs' success reply (2024-07 example): `amend_id` plus the echoed
    /// `cl_ord_id`, under `result`.
    #[test]
    fn a_docs_shaped_amend_reply_decodes_typed() {
        let reply = r#"{"method":"amend_order","result":{"amend_id":"TTW6PD-RC36L-ZZSWNU","cl_ord_id":"2c6be801-1f53-4f79-a0bb-4ea1c95dfae9"},"success":true,"time_in":"2024-07-26T13:39:04.922699Z","time_out":"2024-07-26T13:39:04.924912Z"}"#;
        let Inbound::Method(resp) = reply.parse::<Inbound>().expect("the frame decodes") else {
            panic!("expected a method reply");
        };
        let result = resp
            .into_result::<AmendOrderParams>()
            .expect("the reply projects into the amend result");
        assert_eq!(result.amend_id, "TTW6PD-RC36L-ZZSWNU");
        assert_eq!(
            result.cl_ord_id.as_deref(),
            Some("2c6be801-1f53-4f79-a0bb-4ea1c95dfae9")
        );
    }
}
