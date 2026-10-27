use super::{OrderPrice, OrderTag, PlacementTimeInForce, SelfTradePrevention};
use crate::{
    CommaList, Delivery, Id, Pair,
    spot::{
        AssetClass, Endpoint, OrderFlag, OrderType, Private, Trigger, account::Order,
        sealed::Sealed,
    },
};
use kraken_core::OrderSide;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize, Serializer};
use serde_with::skip_serializing_none;

/// `POST /0/private/AddOrder`. Never resent after a timeout: the exchange may
/// already hold the order, and a resend would place a second one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddOrder {
    pub pair: Pair,
    pub side: OrderSide,
    /// In the base asset, or the quote asset under [`OrderFlag::Viqc`].
    pub volume: Decimal,
    pub kind: OrderKind,
    /// `2:1` style, or `none`.
    pub leverage: Option<String>,
    pub reduce_only: bool,
    pub time_in_force: Option<PlacementTimeInForce>,
    /// `0` (now), `+<seconds>` or a Unix time.
    pub starttm: Option<String>,
    pub expiretm: Option<String>,
    pub tag: Option<OrderTag>,
    pub oflags: CommaList<OrderFlag>,
    pub stptype: Option<SelfTradePrevention>,
    pub close: Option<CloseOrder>,
    /// RFC 3339 instant after which the matching engine rejects the order.
    pub deadline: Option<String>,
    /// Check and price the order without placing it.
    pub validate: bool,
    pub asset_class: Option<AssetClass>,
}

/// An order type with exactly the prices it needs. A limit order without a
/// price, or a stop with a second price it ignores, cannot be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderKind {
    Market,
    Limit {
        price: OrderPrice,
    },
    Iceberg {
        price: OrderPrice,
        display_volume: Decimal,
    },
    StopLoss {
        trigger_price: OrderPrice,
        trigger: Option<Trigger>,
    },
    TakeProfit {
        trigger_price: OrderPrice,
        trigger: Option<Trigger>,
    },
    StopLossLimit {
        trigger_price: OrderPrice,
        limit_price: OrderPrice,
        trigger: Option<Trigger>,
    },
    TakeProfitLimit {
        trigger_price: OrderPrice,
        limit_price: OrderPrice,
        trigger: Option<Trigger>,
    },
    TrailingStop {
        offset: OrderPrice,
        trigger: Option<Trigger>,
    },
    TrailingStopLimit {
        offset: OrderPrice,
        limit_offset: OrderPrice,
        trigger: Option<Trigger>,
    },
    /// Closes a margin position at the given limit.
    SettlePosition {
        price: OrderPrice,
    },
}

/// The order type and the prices it carries, in Kraken's flat wire fields.
struct Priced<'a> {
    ordertype: OrderType,
    price: Option<&'a OrderPrice>,
    price2: Option<&'a OrderPrice>,
    displayvol: Option<Decimal>,
    trigger: Option<Trigger>,
}

impl OrderKind {
    fn priced(&self) -> Priced<'_> {
        let (ordertype, price, price2, displayvol, trigger) = match self {
            Self::Market => (OrderType::Market, None, None, None, None),
            Self::Limit { price } => (OrderType::Limit, Some(price), None, None, None),
            Self::Iceberg {
                price,
                display_volume,
            } => (
                OrderType::Iceberg,
                Some(price),
                None,
                Some(*display_volume),
                None,
            ),
            Self::StopLoss {
                trigger_price,
                trigger,
            } => (
                OrderType::StopLoss,
                Some(trigger_price),
                None,
                None,
                *trigger,
            ),
            Self::TakeProfit {
                trigger_price,
                trigger,
            } => (
                OrderType::TakeProfit,
                Some(trigger_price),
                None,
                None,
                *trigger,
            ),
            Self::StopLossLimit {
                trigger_price,
                limit_price,
                trigger,
            } => (
                OrderType::StopLossLimit,
                Some(trigger_price),
                Some(limit_price),
                None,
                *trigger,
            ),
            Self::TakeProfitLimit {
                trigger_price,
                limit_price,
                trigger,
            } => (
                OrderType::TakeProfitLimit,
                Some(trigger_price),
                Some(limit_price),
                None,
                *trigger,
            ),
            Self::TrailingStop { offset, trigger } => {
                (OrderType::TrailingStop, Some(offset), None, None, *trigger)
            }
            Self::TrailingStopLimit {
                offset,
                limit_offset,
                trigger,
            } => (
                OrderType::TrailingStopLimit,
                Some(offset),
                Some(limit_offset),
                None,
                *trigger,
            ),
            Self::SettlePosition { price } => {
                (OrderType::SettlePosition, Some(price), None, None, None)
            }
        };
        Priced {
            ordertype,
            price,
            price2,
            displayvol,
            trigger,
        }
    }
}

/// The order placed when this one fills, closing the position it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseOrder {
    pub kind: CloseOrderKind,
    pub price: Option<OrderPrice>,
    pub price2: Option<OrderPrice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CloseOrderKind {
    Limit,
    Iceberg,
    StopLoss,
    TakeProfit,
    StopLossLimit,
    TakeProfitLimit,
    TrailingStop,
    TrailingStopLimit,
}

#[skip_serializing_none]
#[derive(Serialize)]
struct CloseBody<'a> {
    ordertype: CloseOrderKind,
    price: Option<&'a OrderPrice>,
    price2: Option<&'a OrderPrice>,
}

/// The request as Kraken's flat JSON body.
#[skip_serializing_none]
#[derive(Serialize)]
struct AddOrderBody<'a> {
    pair: &'a Pair,
    #[serde(rename = "type")]
    side: OrderSide,
    ordertype: OrderType,
    #[serde(with = "crate::decimal::string")]
    volume: Decimal,
    price: Option<&'a OrderPrice>,
    price2: Option<&'a OrderPrice>,
    #[serde(with = "crate::decimal::string::option")]
    displayvol: Option<Decimal>,
    trigger: Option<Trigger>,
    leverage: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    reduce_only: bool,
    timeinforce: Option<PlacementTimeInForce>,
    starttm: Option<&'a str>,
    expiretm: Option<&'a str>,
    userref: Option<i32>,
    cl_ord_id: Option<&'a str>,
    #[serde(skip_serializing_if = "is_empty")]
    oflags: &'a CommaList<OrderFlag>,
    stptype: Option<SelfTradePrevention>,
    close: Option<CloseBody<'a>>,
    deadline: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    validate: bool,
    asset_class: Option<&'a AssetClass>,
}

fn is_empty(flags: &&CommaList<OrderFlag>) -> bool {
    flags.is_empty()
}

impl<'a> From<&'a AddOrder> for AddOrderBody<'a> {
    fn from(order: &'a AddOrder) -> Self {
        // Exhaustive on purpose: a field added to the request must be placed
        // in the body, not dropped from a real order.
        let AddOrder {
            pair,
            side,
            volume,
            kind,
            leverage,
            reduce_only,
            time_in_force,
            starttm,
            expiretm,
            tag,
            oflags,
            stptype,
            close,
            deadline,
            validate,
            asset_class,
        } = order;
        let Priced {
            ordertype,
            price,
            price2,
            displayvol,
            trigger,
        } = kind.priced();
        let (userref, cl_ord_id) = match tag {
            Some(OrderTag::UserRef(r)) => (Some(*r), None),
            Some(OrderTag::ClientOrderId(id)) => (None, Some(id.as_str())),
            None => (None, None),
        };
        Self {
            pair,
            side: *side,
            ordertype,
            volume: *volume,
            price,
            price2,
            displayvol,
            trigger,
            leverage: leverage.as_deref(),
            reduce_only: *reduce_only,
            timeinforce: *time_in_force,
            starttm: starttm.as_deref(),
            expiretm: expiretm.as_deref(),
            userref,
            cl_ord_id,
            oflags,
            stptype: *stptype,
            close: close.as_ref().map(|c| CloseBody {
                ordertype: c.kind,
                price: c.price.as_ref(),
                price2: c.price2.as_ref(),
            }),
            deadline: deadline.as_deref(),
            validate: *validate,
            asset_class: asset_class.as_ref(),
        }
    }
}

impl Serialize for AddOrder {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        AddOrderBody::from(self).serialize(s)
    }
}

/// Kraken's acknowledgement, published as sent. A validate-only request
/// returns no txid.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderAdded {
    pub descr: PlacedDescription,
    pub txid: Option<Vec<Id<Order>>>,
}

/// The order and its attached close, in words.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedDescription {
    pub order: String,
    pub close: Option<String>,
}

impl Sealed for AddOrder {}
impl Endpoint for AddOrder {
    const PATH: &'static str = "AddOrder";
    type Access = Private;
    type Response = OrderAdded;

    /// A dry run places nothing, so it may be resent like a read.
    fn delivery(&self) -> Delivery {
        Delivery::once_if(!self.validate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;
    use proptest::prelude::*;
    use rust_decimal_macros::dec;
    use serde_json::{Value, json};

    fn order(kind: OrderKind) -> AddOrder {
        AddOrder {
            pair: "XBTUSD".into(),
            side: OrderSide::Buy,
            volume: dec!(0.00100),
            kind,
            leverage: None,
            reduce_only: false,
            time_in_force: None,
            starttm: None,
            expiretm: None,
            tag: None,
            oflags: CommaList::default(),
            stptype: None,
            close: None,
            deadline: None,
            validate: false,
            asset_class: None,
        }
    }

    fn price(raw: &str) -> OrderPrice {
        raw.parse().unwrap()
    }

    #[test]
    fn only_a_dry_run_may_be_resent() {
        let dry_run = AddOrder {
            validate: true,
            ..order(OrderKind::Market)
        };
        assert_eq!(dry_run.delivery(), Delivery::Retryable);
        assert_eq!(order(OrderKind::Market).delivery(), Delivery::AtMostOnce);
    }

    #[test]
    fn a_limit_order_sends_exactly_its_fields() {
        let body = serde_json::to_value(order(OrderKind::Limit {
            price: price("50000"),
        }))
        .unwrap();
        assert_eq!(
            body,
            json!({"pair": "XBTUSD", "type": "buy", "ordertype": "limit", "volume": "0.00100", "price": "50000"})
        );
    }

    #[test]
    fn a_close_order_nests_under_close() {
        let mut placed = order(OrderKind::Limit {
            price: price("50000"),
        });
        placed.close = Some(CloseOrder {
            kind: CloseOrderKind::StopLossLimit,
            price: Some(price("40000")),
            price2: Some(price("39000")),
        });
        let body = serde_json::to_value(placed).unwrap();
        assert_eq!(
            body["close"],
            json!({"ordertype": "stop-loss-limit", "price": "40000", "price2": "39000"})
        );
    }

    #[test]
    fn a_client_order_id_replaces_userref() {
        let mut placed = order(OrderKind::Market);
        placed.tag = Some(OrderTag::ClientOrderId("mine-1".into()));
        let body = serde_json::to_value(placed).unwrap();
        assert_eq!(body["cl_ord_id"], "mine-1");
        assert!(body.get("userref").is_none());
    }

    #[test]
    fn decodes_the_acknowledgement() {
        let added: OrderAdded = decode_fixture("AddOrder.json").unwrap();
        assert_eq!(added.txid.unwrap()[0].as_str(), "OUF4EM-FRGI2-MQMWZD");
    }

    fn kinds() -> impl Strategy<Value = OrderKind> {
        let p = || {
            prop::sample::select(vec!["50000", "+10", "#2.5", "3%"])
                .prop_map(|p| p.parse().unwrap())
        };
        let t = || prop::option::of(prop::sample::select(vec![Trigger::Last, Trigger::Index]));
        prop_oneof![
            Just(OrderKind::Market),
            p().prop_map(|price| OrderKind::Limit { price }),
            (p(), 1u32..100).prop_map(|(price, d)| OrderKind::Iceberg {
                price,
                display_volume: d.into()
            }),
            (p(), t()).prop_map(|(trigger_price, trigger)| OrderKind::StopLoss {
                trigger_price,
                trigger
            }),
            (p(), p(), t()).prop_map(|(trigger_price, limit_price, trigger)| {
                OrderKind::StopLossLimit {
                    trigger_price,
                    limit_price,
                    trigger,
                }
            }),
            (p(), t()).prop_map(|(offset, trigger)| OrderKind::TrailingStop { offset, trigger }),
            p().prop_map(|price| OrderKind::SettlePosition { price }),
        ]
    }

    proptest! {
        /// The body carries a second price only for the limit variants of a
        /// stop, and a price for every type but market.
        #[test]
        fn an_order_sends_only_the_prices_its_type_takes(kind in kinds()) {
            let body: Value = serde_json::to_value(order(kind.clone())).unwrap();
            let two_prices = matches!(kind, OrderKind::StopLossLimit { .. } | OrderKind::TakeProfitLimit { .. } | OrderKind::TrailingStopLimit { .. });
            prop_assert_eq!(body.get("price2").is_some(), two_prices);
            prop_assert_eq!(body.get("price").is_some(), kind != OrderKind::Market);
            prop_assert_eq!(body.get("displayvol").is_some(), matches!(kind, OrderKind::Iceberg { .. }));
        }
    }
}
