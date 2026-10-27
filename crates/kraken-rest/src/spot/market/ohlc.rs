use super::pages::PairPage;
use crate::{
    Delivery, Pair,
    decimal::Wire,
    spot::{Endpoint, Public, sealed::Sealed, vocabulary::AssetClass},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::skip_serializing_none;

/// `GET /0/public/OHLC`.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct OhlcQuery {
    pub pair: Pair,
    /// Candle width in minutes.
    pub interval: u32,
    /// Exclusive lower bound: a previous page's `last`.
    pub since: Option<String>,
    pub asset_class: Option<AssetClass>,
}

/// A page of candles. `pair` is Kraken's wire name until the caller echoes
/// its own spelling into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ohlc {
    pub last: u64,
    pub pair: Pair,
    pub candles: Vec<Candle>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candle {
    /// Unix seconds at the candle's open.
    pub time: u64,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub vwap: Decimal,
    pub volume: Decimal,
    pub count: u64,
}

impl<'de> Deserialize<'de> for Candle {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (time, Wire(open), Wire(high), Wire(low), Wire(close), Wire(vwap), Wire(volume), count) =
            Deserialize::deserialize(d)?;
        Ok(Self {
            time,
            open,
            high,
            low,
            close,
            vwap,
            volume,
            count,
        })
    }
}

impl<'de> Deserialize<'de> for Ohlc {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let PairPage { pair, rows, last } = PairPage::deserialize(d)?;
        Ok(Self {
            last,
            pair,
            candles: rows,
        })
    }
}

impl Sealed for OhlcQuery {}
impl Endpoint for OhlcQuery {
    const PATH: &'static str = "OHLC";
    type Access = Public;
    type Response = Ohlc;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn decodes_the_captured_page() {
        let ohlc: Ohlc = decode_fixture("OHLC.json").unwrap();
        assert_eq!(ohlc.pair.as_str(), "XXBTZUSD");
        assert_eq!(ohlc.last, 1792494000);
        assert_eq!(ohlc.candles.len(), 3);
        assert_eq!(ohlc.candles[0].count, 5532);
    }
}
