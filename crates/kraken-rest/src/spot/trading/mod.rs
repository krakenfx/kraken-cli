//! Order entry: place, batch, amend, edit, cancel. Every request here moves or
//! withdraws real orders, so a lost reply is resent only when the request
//! provably never left (see [`Delivery::AtMostOnce`](crate::Delivery)).

mod add_order;
mod add_order_batch;
mod amend_order;
mod cancel;
mod edit_order;

use crate::{Id, spot::account::Order};
pub use add_order::{
    AddOrder, CloseOrder, CloseOrderKind, OrderAdded, OrderKind, PlacedDescription,
};
pub use add_order_batch::{AddOrderBatch, BatchClose, BatchOrder, BatchOrderResult, BatchPlaced};
pub use amend_order::{AmendOrder, OrderAmended};
pub use cancel::{
    CancelAll, CancelAllOrdersAfter, CancelOrder, CancelOrderBatch, Cancelled, DeadManSwitch,
};
pub use edit_order::{EditOrder, OrderEdited};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

/// A price as Kraken accepts it on an order: absolute (`50000.5`), relative to
/// the last trade (`+10`, `-5`, `#5` for either direction), or as a percentage
/// of it (`2%`, `+1.5%`). Validated once, sent verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderPrice(String);

impl OrderPrice {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a price string is not an order price.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a price: expected a decimal, optionally prefixed +, - or # and suffixed %")]
pub struct InvalidPrice(String);

impl FromStr for OrderPrice {
    type Err = InvalidPrice;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let body = raw.strip_prefix(['+', '-', '#']).unwrap_or(raw);
        let body = body.strip_suffix('%').unwrap_or(body);
        let (whole, fraction) = body.split_once('.').unwrap_or((body, "0"));
        let is_digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        if is_digits(whole) && is_digits(fraction) {
            Ok(Self(raw.to_owned()))
        } else {
            Err(InvalidPrice(raw.to_owned()))
        }
    }
}

impl fmt::Display for OrderPrice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for OrderPrice {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for OrderPrice {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// The caller's own label on an order: a numeric group tag or a unique client
/// order id, never both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderTag {
    UserRef(i32),
    ClientOrderId(String),
}

/// Which order a modification names: Kraken's txid or the caller's client id.
/// Flattened into the request as the one field it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum OrderRef {
    #[serde(rename = "txid")]
    Txid(Id<Order>),
    #[serde(rename = "cl_ord_id")]
    ClientOrderId(String),
}

/// Time-in-force on a new order. Kraken's request side takes it in capitals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PlacementTimeInForce {
    Gtc,
    Ioc,
    Gtd,
    Fok,
}

/// Self-trade prevention: which side of a crossing pair of own orders goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelfTradePrevention {
    CancelNewest,
    CancelOldest,
    CancelBoth,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn relative_and_percentage_prices_are_prices() {
        for price in ["50000", "50000.5", "+10", "-5.5", "#5", "2%", "+1.5%"] {
            assert!(price.parse::<OrderPrice>().is_ok(), "{price}");
        }
    }

    #[test]
    fn malformed_prices_are_refused() {
        for price in ["", "+", "1e5", "1,5", "5%%", "abc", "1.", ".5", "++5"] {
            assert!(price.parse::<OrderPrice>().is_err(), "{price}");
        }
    }

    proptest! {
        #[test]
        fn a_valid_price_is_sent_verbatim(
            sign in prop::sample::select(vec!["", "+", "-", "#"]),
            whole in 0u64..10_000_000,
            fraction in prop::option::of(0u32..100_000_000),
            pct in any::<bool>(),
        ) {
            let raw = format!(
                "{sign}{whole}{}{}",
                fraction.map(|f| format!(".{f}")).unwrap_or_default(),
                if pct { "%" } else { "" },
            );
            let price: OrderPrice = raw.parse().unwrap();
            prop_assert_eq!(serde_json::to_string(&price).unwrap(), format!("{raw:?}"));
        }
    }
}
