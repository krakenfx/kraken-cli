use crate::{
    Delivery, Pair,
    spot::{Endpoint, Public, sealed::Sealed},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `GET /0/public/GroupedBook`.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct GroupedBookQuery {
    pub pair: Pair,
    /// Price levels per side.
    pub depth: Option<u32>,
    /// Ticks merged into one level.
    pub grouping: Option<u32>,
}

/// An L2 book with levels merged into price bands. Published as sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupedBook {
    pub pair: Pair,
    pub bids: Vec<GroupedLevel>,
    pub asks: Vec<GroupedLevel>,
    pub grouping: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupedLevel {
    #[serde(with = "crate::decimal::string")]
    pub price: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub qty: Decimal,
}

impl Sealed for GroupedBookQuery {}
impl Endpoint for GroupedBookQuery {
    const PATH: &'static str = "GroupedBook";
    type Access = Public;
    type Response = GroupedBook;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn levels_publish_their_wire_digits() {
        let book: GroupedBook = decode_fixture("GroupedBook.json").unwrap();
        let published = serde_json::to_value(&book).unwrap();
        assert_eq!(published["bids"][0]["price"], "81401.30000");
        assert_eq!(book.grouping, 1);
    }
}
