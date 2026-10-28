use super::pages::PairPage;
use crate::{
    Delivery, Pair, UnixTime,
    decimal::Wire,
    spot::{Endpoint, Public, sealed::Sealed, vocabulary::AssetClass},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::skip_serializing_none;

/// `GET /0/public/Spread`.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct SpreadsQuery {
    pub pair: Pair,
    /// Exclusive lower bound: a previous page's `last`.
    pub since: Option<String>,
    pub asset_class: Option<AssetClass>,
}

/// A page of best bid/ask snapshots. `pair` is Kraken's wire name until the
/// caller echoes its own spelling into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Spreads {
    pub last: UnixTime,
    pub pair: Pair,
    pub spreads: Vec<Spread>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Spread {
    /// Unix seconds.
    pub time: u64,
    pub bid: Decimal,
    pub ask: Decimal,
}

impl<'de> Deserialize<'de> for Spread {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (time, Wire(bid), Wire(ask)) = Deserialize::deserialize(d)?;
        Ok(Self { time, bid, ask })
    }
}

impl<'de> Deserialize<'de> for Spreads {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let PairPage { pair, rows, last } = PairPage::deserialize(d)?;
        Ok(Self {
            last,
            pair,
            spreads: rows,
        })
    }
}

impl Sealed for SpreadsQuery {}
impl Endpoint for SpreadsQuery {
    const PATH: &'static str = "Spread";
    type Access = Public;
    type Response = Spreads;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn the_integer_cursor_publishes_as_a_string() {
        let spreads: Spreads = decode_fixture("Spread.json").unwrap();
        assert_eq!(
            serde_json::to_value(&spreads).unwrap()["last"],
            "1792499885"
        );
        assert_eq!(spreads.spreads.len(), 3);
    }
}
