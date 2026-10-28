use super::OrderRef;
use crate::{
    Id,
    spot::{Endpoint, Private, account::Order, sealed::Sealed},
};
use serde::{Deserialize, Serialize, Serializer};
use serde_with::skip_serializing_none;

/// `POST /0/private/CancelOrder`: one order. Never resent after a timeout,
/// because its reply is the only proof of which order closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelOrder {
    #[serde(flatten)]
    pub order: OrderRef,
}

/// `POST /0/private/CancelOrderBatch`: up to 50 orders, by txid and client id.
/// Never resent after a timeout, like every other order write.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CancelOrderBatch {
    #[serde(serialize_with = "txid_objects", skip_serializing_if = "Vec::is_empty")]
    pub orders: Vec<Id<Order>>,
    #[serde(
        serialize_with = "client_id_objects",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub cl_ord_ids: Vec<String>,
}

/// Kraken names each order as an object, `{"txid": …}`, not as a bare string.
fn txid_objects<S: Serializer>(txids: &[Id<Order>], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(txids.iter().cloned().map(OrderRef::Txid))
}

fn client_id_objects<S: Serializer>(ids: &[String], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(ids.iter().cloned().map(OrderRef::ClientOrderId))
}

/// `POST /0/private/CancelAll`: every open order on the account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CancelAll {}

/// `POST /0/private/CancelAllOrdersAfter`: the dead man's switch. Every call
/// re-arms it to `timeout` seconds from now; `0` disarms it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelAllOrdersAfter {
    pub timeout: u64,
}

/// Published as sent.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cancelled {
    pub count: u32,
    /// The cancel is accepted but not yet confirmed.
    pub pending: Option<bool>,
}

/// When the switch fires. Published as sent, Kraken's camelCase included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadManSwitch {
    #[serde(rename = "currentTime")]
    pub current_time: String,
    #[serde(rename = "triggerTime")]
    pub trigger_time: String,
}

impl Sealed for CancelOrder {}
impl Endpoint for CancelOrder {
    const PATH: &'static str = "CancelOrder";
    type Access = Private;
    type Response = Cancelled;
}

impl Sealed for CancelOrderBatch {}
impl Endpoint for CancelOrderBatch {
    const PATH: &'static str = "CancelOrderBatch";
    type Access = Private;
    type Response = Cancelled;
}

impl Sealed for CancelAll {}
impl Endpoint for CancelAll {
    const PATH: &'static str = "CancelAll";
    type Access = Private;
    type Response = Cancelled;
}

impl Sealed for CancelAllOrdersAfter {}
impl Endpoint for CancelAllOrdersAfter {
    const PATH: &'static str = "CancelAllOrdersAfter";
    type Access = Private;
    type Response = DeadManSwitch;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;
    use serde_json::json;

    #[test]
    fn a_batch_names_each_order_as_an_object() {
        let batch = CancelOrderBatch {
            orders: vec!["OA".into()],
            cl_ord_ids: vec!["mine-1".into()],
        };
        assert_eq!(
            serde_json::to_value(batch).unwrap(),
            json!({"orders": [{"txid": "OA"}], "cl_ord_ids": [{"cl_ord_id": "mine-1"}]})
        );
    }

    #[test]
    fn a_cancel_names_its_order_by_txid() {
        let cancel = CancelOrder {
            order: OrderRef::Txid("OA".into()),
        };
        assert_eq!(serde_json::to_value(cancel).unwrap(), json!({"txid": "OA"}));
    }

    #[test]
    fn decodes_the_replies() {
        let cancelled: Cancelled = decode_fixture("CancelOrder.json").unwrap();
        assert_eq!(cancelled.count, 1);
        let switch: DeadManSwitch = decode_fixture("CancelAllOrdersAfter.json").unwrap();
        assert_eq!(switch.trigger_time, "2023-03-24T17:42:56Z");
    }
}
