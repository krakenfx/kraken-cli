use super::{Cursor, Window, trades::Fill};
use crate::{
    CommaList, Delivery, Id, KeyedRows, Pair, UnixTime,
    spot::{
        AssetClass, Endpoint, OrderFlag, OrderNote, OrderStatus, OrderType, Private,
        RebaseMultiplier, TimeInForce, Trigger, sealed::Sealed,
    },
};
use kraken_core::OrderSide;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/OpenOrders`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct OpenOrdersQuery {
    /// Include each order's fill ids.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub trades: bool,
    pub userref: Option<i32>,
    pub cl_ord_id: Option<String>,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

/// `POST /0/private/ClosedOrders`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct ClosedOrdersQuery {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub trades: bool,
    pub userref: Option<i32>,
    pub cl_ord_id: Option<String>,
    #[serde(flatten)]
    pub window: Window,
    pub closetime: Option<CloseTime>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub consolidate_taker: bool,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

/// Which of an order's times the window bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseTime {
    Open,
    Close,
    Both,
}

/// `POST /0/private/QueryOrders`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct QueryOrdersQuery {
    pub txid: CommaList<Id<Order>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub trades: bool,
    pub userref: Option<i32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub consolidate_taker: bool,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenOrders {
    pub open: KeyedRows<Id<Order>, Order>,
    pub cursor: Option<Cursor>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedOrders {
    pub closed: KeyedRows<Id<Order>, Order>,
    /// Absent when the query asked `without_count`.
    pub count: Option<u64>,
    pub cursor: Option<Cursor>,
}

pub type QueriedOrders = KeyedRows<Id<Order>, Order>;

/// One order, open or closed. Kraken's `vol`/`vol_exec` publish as
/// `volume`/`volume_executed`; times publish as their wire tokens.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    pub descr: OrderDescription,
    pub link_id: Option<String>,
    pub ext_ord_id: Option<String>,
    pub sender_sub_id: Option<String>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub displayvol: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub displayvolremain: Option<Decimal>,
    pub margin: Option<bool>,
    pub reduce_only: Option<bool>,
    /// The order that created this one, when one did. Kraken sends `"None"`
    /// as well as null for "none".
    pub refid: Option<String>,
    pub userref: Option<i64>,
    pub cl_ord_id: Option<String>,
    pub status: OrderStatus,
    pub opentm: UnixTime,
    pub closetm: Option<UnixTime>,
    pub starttm: Option<UnixTime>,
    pub expiretm: UnixTime,
    #[serde(rename(deserialize = "vol"), with = "crate::decimal::number")]
    pub volume: Decimal,
    #[serde(rename(deserialize = "vol_exec"), with = "crate::decimal::number")]
    pub volume_executed: Decimal,
    #[serde(with = "crate::decimal::number")]
    pub cost: Decimal,
    #[serde(with = "crate::decimal::number")]
    pub fee: Decimal,
    /// Average fill price.
    #[serde(with = "crate::decimal::number")]
    pub price: Decimal,
    #[serde(default, with = "crate::decimal::number::option")]
    pub stopprice: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub limitprice: Option<Decimal>,
    pub misc: CommaList<OrderNote>,
    pub oflags: CommaList<OrderFlag>,
    pub trades: Option<Vec<Id<Fill>>>,
    /// Why a closed order closed.
    pub reason: Option<String>,
    pub trigger: Option<Trigger>,
    #[serde(default)]
    pub time_in_force: TimeInForce,
}

/// The order as placed. `type` publishes as `side`.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderDescription {
    pub pair: Pair,
    pub aclass: Option<AssetClass>,
    #[serde(rename(deserialize = "type"))]
    pub side: OrderSide,
    pub ordertype: OrderType,
    #[serde(with = "crate::decimal::number")]
    pub price: Decimal,
    /// The second price of a two-price order type (`stop-loss-limit`).
    #[serde(with = "crate::decimal::number")]
    pub price2: Decimal,
    pub leverage: String,
    /// The order in words: `buy 1.25 XBTUSD @ limit 30010.0`.
    pub order: String,
    /// The conditional close attached to it, in words; empty when none.
    pub close: String,
}

impl Sealed for OpenOrdersQuery {}
impl Endpoint for OpenOrdersQuery {
    const PATH: &'static str = "OpenOrders";
    type Access = Private;
    type Response = OpenOrders;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for ClosedOrdersQuery {}
impl Endpoint for ClosedOrdersQuery {
    const PATH: &'static str = "ClosedOrders";
    type Access = Private;
    type Response = ClosedOrders;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for QueryOrdersQuery {}
impl Endpoint for QueryOrdersQuery {
    const PATH: &'static str = "QueryOrders";
    type Access = Private;
    type Response = QueriedOrders;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;
    use rust_decimal_macros::dec;

    #[test]
    fn an_open_order_decodes_and_renames() {
        let open: OpenOrders = decode_fixture("OpenOrders.json").unwrap();
        let order = open.open.values().next().unwrap();
        assert_eq!(order.volume_executed, dec!(0.375));
        assert_eq!(order.descr.side, OrderSide::Buy);
        assert!(order.oflags.0.contains(&OrderFlag::Fciq));
        let published = serde_json::to_value(order).unwrap();
        assert_eq!(published["descr"]["side"], "buy");
        assert!(published.get("vol").is_none());
    }

    #[test]
    fn an_absent_time_in_force_is_good_till_cancelled() {
        let closed: ClosedOrders = decode_fixture("ClosedOrders.json").unwrap();
        assert!(
            closed
                .closed
                .values()
                .all(|o| o.time_in_force == TimeInForce::Gtc)
        );
        assert_eq!(closed.count, Some(2));
    }

    #[test]
    fn queried_orders_share_the_order_shape() {
        let orders: QueriedOrders = decode_fixture("QueryOrders.json").unwrap();
        let order = orders.values().next().unwrap();
        assert_eq!(order.status, OrderStatus::Closed);
        assert_eq!(order.trigger, Some(Trigger::Index));
    }

    #[test]
    fn txids_join_into_one_parameter() {
        let query = QueryOrdersQuery {
            txid: vec!["OA".into(), "OB".into()].into(),
            ..QueryOrdersQuery::default()
        };
        assert_eq!(
            serde_json::to_string(&query).unwrap(),
            r#"{"txid":"OA,OB"}"#
        );
    }
}
