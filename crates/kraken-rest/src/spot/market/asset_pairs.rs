use crate::{
    Asset, Delivery, KeyedRows,
    decimal::Wire,
    spot::{Endpoint, Public, sealed::Sealed, vocabulary::AssetClass},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeMap as _};
use serde_with::skip_serializing_none;

/// `GET /0/public/AssetPairs`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct AssetPairsQuery {
    /// Comma-separated pairs; every pair of the class when absent.
    pub pair: Option<String>,
    pub info: Option<PairInfo>,
    pub aclass: Option<AssetClass>,
}

/// Which slice of each pair's details to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairInfo {
    Info,
    Leverage,
    Fees,
    Margin,
}

/// Pair details under Kraken's wire keys (`XXBTZUSD`), published under
/// modern `BASE/QUOTE` keys (`BTC/USD`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(transparent)]
pub struct AssetPairs(KeyedRows<String, AssetPair>);

impl AssetPairs {
    /// Entries under their wire keys.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &AssetPair)> {
        self.0.iter().map(|(key, pair)| (key.as_str(), pair))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for AssetPairs {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (key, pair) in &self.0 {
            map.serialize_entry(&pair.published_key(key), pair)?;
        }
        map.end()
    }
}

/// One pair. The legacy aliases (`altname`, `wsname`, `base`, `quote`, `lot`)
/// are read but never published: the modern key states the pair's identity and
/// the aliases would contradict it. Every field is optional because `info`
/// selects a subset.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetPair {
    #[serde(skip_serializing)]
    pub altname: Option<String>,
    #[serde(skip_serializing)]
    pub wsname: Option<String>,
    #[serde(skip_serializing)]
    pub base: Option<String>,
    #[serde(skip_serializing)]
    pub quote: Option<String>,
    #[serde(skip_serializing)]
    pub lot: Option<String>,
    pub aclass_base: Option<AssetClass>,
    pub aclass_quote: Option<AssetClass>,
    pub cost_decimals: Option<u8>,
    pub pair_decimals: Option<u8>,
    pub lot_decimals: Option<u8>,
    pub lot_multiplier: Option<u64>,
    pub leverage_buy: Option<Vec<u32>>,
    pub leverage_sell: Option<Vec<u32>>,
    pub fees: Option<Vec<FeeTier>>,
    pub fees_maker: Option<Vec<FeeTier>>,
    pub fee_volume_currency: Option<Asset>,
    pub margin_call: Option<i32>,
    pub margin_stop: Option<i32>,
    pub margin_level: Option<i32>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub ordermin: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub costmin: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub tick_size: Option<Decimal>,
    pub status: Option<PairStatus>,
    pub execution_venue: Option<ExecutionVenue>,
    pub long_position_limit: Option<u64>,
    pub short_position_limit: Option<u64>,
    pub launch_time: Option<i64>,
}

impl AssetPair {
    /// An already-slashed key stays; a slashless one is rebuilt from the
    /// pair's legacy codes when both are present.
    fn published_key(&self, wire_key: &str) -> String {
        match (self.base.as_deref(), self.quote.as_deref()) {
            (Some(base), Some(quote))
                if !wire_key.contains('/') && !base.is_empty() && !quote.is_empty() =>
            {
                format!("{}/{}", Asset::normalise(base), Asset::normalise(quote))
            }
            _ => wire_key.to_owned(),
        }
    }
}

/// A fee schedule step: from `volume` (30-day, in the fee volume currency)
/// the fee is `fee_pct` percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FeeTier {
    pub volume: Decimal,
    pub fee_pct: Decimal,
}

/// On the wire a tier is the pair `[volume, fee_pct]`.
impl<'de> Deserialize<'de> for FeeTier {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (Wire(volume), Wire(fee_pct)) = Deserialize::deserialize(d)?;
        Ok(Self { volume, fee_pct })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairStatus {
    Online,
    CancelOnly,
    Delisted,
    PostOnly,
    LimitOnly,
    ReduceOnly,
    SellOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionVenue {
    International,
    KrakenDerivativesExchange,
    BitnomialExchange,
}

impl Sealed for AssetPairsQuery {}
impl Endpoint for AssetPairsQuery {
    const PATH: &'static str = "AssetPairs";
    type Access = Public;
    type Response = AssetPairs;

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
    fn slashless_keys_publish_as_modern_pairs() {
        let pairs: AssetPairs = decode_fixture("AssetPairs.json").unwrap();
        let published = serde_json::to_value(&pairs).unwrap();
        assert!(published.get("BTC/USD").is_some(), "{published}");
        assert!(published["BTC/USD"].get("altname").is_none());
        assert_eq!(
            pairs.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            ["XETHZUSD", "XXBTZUSD"]
        );
    }

    #[test]
    fn a_fee_tier_decodes_from_its_wire_pair() {
        let tier: FeeTier = serde_json::from_str("[50000, 0.24]").unwrap();
        assert_eq!(
            tier,
            FeeTier {
                volume: dec!(50000),
                fee_pct: dec!(0.24)
            }
        );
    }
}
