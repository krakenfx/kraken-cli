use crate::{
    Delivery, KeyedRows, Pair,
    decimal::Wire,
    spot::{Endpoint, Public, sealed::Sealed, vocabulary::AssetClass},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_with::skip_serializing_none;

/// `GET /0/public/Depth`.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct DepthQuery {
    pub pair: Pair,
    /// Levels per side.
    pub count: Option<u32>,
    pub asset_class: Option<AssetClass>,
}

/// One pair's L2 book. Kraken nests it under the wire pair name; with one pair
/// per request the wrapper carries nothing and is dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Depth {
    pub asks: Vec<Level>,
    pub bids: Vec<Level>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Level {
    pub price: Decimal,
    pub volume: Decimal,
    /// Unix seconds of the level's last change.
    pub timestamp: u64,
}

impl<'de> Deserialize<'de> for Level {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (Wire(price), Wire(volume), timestamp) = Deserialize::deserialize(d)?;
        Ok(Self {
            price,
            volume,
            timestamp,
        })
    }
}

#[derive(Deserialize)]
struct WireBook {
    asks: Vec<Level>,
    bids: Vec<Level>,
}

impl<'de> Deserialize<'de> for Depth {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let books = KeyedRows::<String, WireBook>::deserialize(d)?;
        let Some((_, WireBook { asks, bids })) = books.into_iter().next() else {
            return Err(de::Error::invalid_length(0, &"one pair's book"));
        };
        Ok(Self { asks, bids })
    }
}

impl Sealed for DepthQuery {}
impl Endpoint for DepthQuery {
    const PATH: &'static str = "Depth";
    type Access = Public;
    type Response = Depth;

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
    fn the_pair_wrapper_is_dropped() {
        let book: Depth = decode_fixture("Depth.json").unwrap();
        assert_eq!(book.asks[0].price, dec!(81401.40000));
        assert_eq!(book.bids.len(), 3);
    }

    #[test]
    fn an_empty_reply_is_not_a_book() {
        assert!(serde_json::from_str::<Depth>("{}").is_err());
    }
}
