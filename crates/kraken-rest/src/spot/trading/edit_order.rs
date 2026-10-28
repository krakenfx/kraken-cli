use super::{OrderPrice, PlacedDescription};
use crate::{
    Id, Pair,
    spot::{Endpoint, Private, account::Order, sealed::Sealed},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/EditOrder`: cancels an order and places its replacement
/// under a new txid. Never resent after a timeout.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditOrder {
    pub txid: Id<Order>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub volume: Option<Decimal>,
    pub price: Option<OrderPrice>,
    pub pair: Option<Pair>,
}

/// Published as sent.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderEdited {
    pub status: Option<String>,
    /// The replacement's txid.
    pub txid: Id<Order>,
    pub originaltxid: Option<Id<Order>>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub volume: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub price: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub price2: Option<Decimal>,
    pub orders_cancelled: Option<u32>,
    pub newuserref: Option<String>,
    pub olduserref: Option<String>,
    pub error_message: Option<String>,
    pub descr: PlacedDescription,
}

impl Sealed for EditOrder {}
impl Endpoint for EditOrder {
    const PATH: &'static str = "EditOrder";
    type Access = Private;
    type Response = OrderEdited;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn decodes_the_replacement() {
        let edited: OrderEdited = decode_fixture("EditOrder.json").unwrap();
        assert_eq!(edited.txid.as_str(), "OFVXHJ-KPQ3B-VS7ELA");
        assert_eq!(edited.orders_cancelled, Some(1));
        let minimal: OrderEdited = decode_fixture("EditOrder.schema-example.json").unwrap();
        assert!(minimal.originaltxid.is_none());
    }
}
