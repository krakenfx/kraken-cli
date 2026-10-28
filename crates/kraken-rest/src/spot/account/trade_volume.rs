use crate::{
    Asset, CommaList, Delivery, KeyedRows, Pair,
    spot::{AssetClass, Endpoint, Private, RebaseMultiplier, sealed::Sealed},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/TradeVolume`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct TradeVolumeQuery {
    /// Pairs whose fee tiers to include.
    #[serde(skip_serializing_if = "CommaList::is_empty")]
    pub pair: CommaList<Pair>,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

/// 30-day volume and the fee tier it buys per pair. The tiers publish as sent,
/// nulls included; only the headline volume and its currency are normalised.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeVolume {
    pub asset_class: Option<AssetClass>,
    pub inputs: Option<TradeVolumeInputs>,
    pub fees: Option<KeyedRows<String, FeeTierStatus>>,
    pub fees_maker: Option<KeyedRows<String, FeeTierStatus>>,
    pub currency: Asset,
    #[serde(with = "crate::decimal::number")]
    pub volume: Decimal,
}

/// What the volume figure is built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeVolumeInputs {
    #[serde(with = "crate::decimal::string")]
    pub domain_spot_volume_30d: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub domain_futures_volume_30d: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub domain_assets_on_platform: Decimal,
}

/// A pair's current fee and the next tier, in percent. `next*` is null at the
/// top tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeTierStatus {
    #[serde(with = "crate::decimal::string")]
    pub fee: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub minfee: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub maxfee: Decimal,
    #[serde(default, with = "crate::decimal::string::option")]
    pub nextfee: Option<Decimal>,
    #[serde(with = "crate::decimal::string")]
    pub tiervolume: Decimal,
    #[serde(default, with = "crate::decimal::string::option")]
    pub nextvolume: Option<Decimal>,
}

impl Sealed for TradeVolumeQuery {}
impl Endpoint for TradeVolumeQuery {
    const PATH: &'static str = "TradeVolume";
    type Access = Private;
    type Response = TradeVolume;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn the_top_tier_publishes_its_null_next_fee() {
        let volume: TradeVolume = decode_fixture("TradeVolume.json").unwrap();
        assert_eq!(volume.currency.as_str(), "USD");
        let published = serde_json::to_value(&volume).unwrap();
        assert!(published["fees"]["XXBTZUSD"]["nextfee"].is_null());
    }
}
