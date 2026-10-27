use crate::{
    Delivery, Pair,
    spot::{Endpoint, Private, sealed::Sealed},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/Level3`: the order-by-order book. Signed because Kraken
/// grants it per key.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct Level3Query {
    pub pair: Pair,
    /// Orders per side; 0 for the whole book.
    pub depth: Option<u32>,
}

/// Published as sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Level3 {
    pub pair: Pair,
    pub bids: Vec<Level3Order>,
    pub asks: Vec<Level3Order>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Level3Order {
    #[serde(with = "crate::decimal::string")]
    pub price: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub qty: Decimal,
    pub order_id: String,
    /// Nanoseconds; above 2^53, so never through an f64.
    pub timestamp: u64,
}

impl Sealed for Level3Query {}
impl Endpoint for Level3Query {
    const PATH: &'static str = "Level3";
    type Access = Private;
    type Response = Level3;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn nanosecond_timestamps_stay_exact() {
        let book: Level3 = decode_fixture("Level3.json").unwrap();
        assert_eq!(book.asks[1].timestamp, 1765622021013826600);
    }
}
