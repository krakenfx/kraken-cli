use super::{OrderPrice, OrderRef};
use crate::{
    Pair,
    spot::{Endpoint, Private, sealed::Sealed},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/AmendOrder`: changes a resting order in place, keeping its
/// queue priority and ids. Never resent after a timeout.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AmendOrder {
    #[serde(flatten)]
    pub order: OrderRef,
    #[serde(default, with = "crate::decimal::string::option")]
    pub order_qty: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub display_qty: Option<Decimal>,
    pub limit_price: Option<OrderPrice>,
    pub trigger_price: Option<OrderPrice>,
    pub pair: Option<Pair>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub post_only: bool,
    pub deadline: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderAmended {
    pub amend_id: String,
}

impl Sealed for AmendOrder {}
impl Endpoint for AmendOrder {
    const PATH: &'static str = "AmendOrder";
    type Access = Private;
    type Response = OrderAmended;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;
    use serde_json::json;

    #[test]
    fn the_order_is_named_by_one_id() {
        let amend = AmendOrder {
            order: OrderRef::ClientOrderId("mine-1".into()),
            order_qty: None,
            display_qty: None,
            limit_price: Some("51000".parse().unwrap()),
            trigger_price: None,
            pair: None,
            post_only: false,
            deadline: None,
        };
        assert_eq!(
            serde_json::to_value(amend).unwrap(),
            json!({"cl_ord_id": "mine-1", "limit_price": "51000"})
        );
    }

    #[test]
    fn decodes_the_amend_id() {
        let amended: OrderAmended = decode_fixture("AmendOrder.json").unwrap();
        assert_eq!(amended.amend_id, "TEZA4R-DSDGT-IJBOJK");
    }
}
