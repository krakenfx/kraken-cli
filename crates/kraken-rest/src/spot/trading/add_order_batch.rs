use super::{OrderPrice, PlacementTimeInForce, SelfTradePrevention};
use crate::{
    CommaList, Delivery, Id, Pair,
    spot::{
        AssetClass, Endpoint, OrderFlag, OrderType, Private, Trigger, account::Order,
        sealed::Sealed,
    },
};
use kraken_core::OrderSide;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/AddOrderBatch`: up to 15 orders on one pair, placed or
/// rejected together. Never resent after a timeout.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddOrderBatch {
    pub orders: Vec<BatchOrder>,
    pub pair: Pair,
    pub asset_class: Option<AssetClass>,
    pub deadline: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub validate: bool,
}

/// One order of a batch, in Kraken's own field names: the shape of a batch
/// file the caller writes. Unknown fields are refused, so a misspelt field
/// fails here instead of being dropped from a real order.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchOrder {
    /// The batch's pair, when the file names it per order. Sent once, at the
    /// top level.
    #[serde(skip_serializing)]
    pub pair: Option<Pair>,
    pub ordertype: OrderType,
    #[serde(rename = "type")]
    pub side: OrderSide,
    #[serde(with = "crate::decimal::string")]
    pub volume: Decimal,
    #[serde(default, with = "crate::decimal::string::option")]
    pub displayvol: Option<Decimal>,
    pub price: Option<OrderPrice>,
    pub price2: Option<OrderPrice>,
    pub trigger: Option<Trigger>,
    pub leverage: Option<String>,
    pub reduce_only: Option<bool>,
    pub stptype: Option<SelfTradePrevention>,
    pub oflags: Option<CommaList<OrderFlag>>,
    pub timeinforce: Option<PlacementTimeInForce>,
    pub starttm: Option<String>,
    pub expiretm: Option<String>,
    pub userref: Option<i32>,
    pub cl_ord_id: Option<String>,
    pub close: Option<BatchClose>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchClose {
    pub ordertype: OrderType,
    pub price: Option<OrderPrice>,
    pub price2: Option<OrderPrice>,
}

/// Published as sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchPlaced {
    pub orders: Vec<BatchOrderResult>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchOrderResult {
    pub txid: Option<Id<Order>>,
    pub descr: Option<super::PlacedDescription>,
    pub error: Option<String>,
}

impl Sealed for AddOrderBatch {}
impl Endpoint for AddOrderBatch {
    const PATH: &'static str = "AddOrderBatch";
    type Access = Private;
    type Response = BatchPlaced;

    /// A dry run places nothing, so it may be resent like a read.
    fn delivery(&self) -> Delivery {
        Delivery::once_if(!self.validate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn a_misspelt_batch_field_is_refused() {
        let err = serde_json::from_str::<BatchOrder>(
            r#"{"ordertype":"limit","type":"buy","volume":"1","pirce":"5"}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown field `pirce`"), "{err}");
    }

    #[test]
    fn the_per_order_pair_stays_out_of_the_order() {
        let order: BatchOrder = serde_json::from_str(
            r#"{"pair":"XBTUSD","ordertype":"market","type":"sell","volume":0.5}"#,
        )
        .unwrap();
        let body = serde_json::to_value(&order).unwrap();
        assert!(body.get("pair").is_none());
        assert_eq!(body["volume"], "0.5");
    }

    #[test]
    fn decodes_the_batch_acknowledgement() {
        let placed: BatchPlaced = decode_fixture("AddOrderBatch.json").unwrap();
        assert_eq!(placed.orders.len(), 2);
    }
}
