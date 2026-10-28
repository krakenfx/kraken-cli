use crate::{
    Asset, Delivery, KeyedRows,
    spot::{Endpoint, Public, sealed::Sealed, vocabulary::AssetClass},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `GET /0/public/Assets`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct AssetsQuery {
    /// Comma-separated asset codes; all assets of the class when absent.
    pub asset: Option<String>,
    pub aclass: Option<AssetClass>,
}

/// Asset details keyed by modern asset code.
pub type Assets = KeyedRows<Asset, AssetInfo>;

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetInfo {
    pub aclass: AssetClass,
    pub altname: String,
    pub decimals: u8,
    pub display_decimals: u8,
    #[serde(default, with = "crate::decimal::number::option")]
    pub collateral_value: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub max_collateral_balance_usd_value: Option<Decimal>,
    pub status: Option<AssetStatus>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub margin_rate: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub token_multiplier: Option<Decimal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetStatus {
    Enabled,
    DepositOnly,
    WithdrawalOnly,
    FundingTemporarilyDisabled,
    ExternalFundingOnly,
    SellOnly,
    Delisted,
}

impl Sealed for AssetsQuery {}
impl Endpoint for AssetsQuery {
    const PATH: &'static str = "Assets";
    type Access = Public;
    type Response = Assets;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn legacy_codes_key_their_modern_names() {
        let assets: Assets = decode_fixture("Assets.json").unwrap();
        let keys: Vec<&str> = assets.keys().map(Asset::as_str).collect();
        assert_eq!(keys, ["ETH", "BTC", "EUR", "USD"]);
    }

    #[test]
    fn an_unknown_asset_status_fails_the_decode() {
        let err = serde_json::from_str::<AssetInfo>(
            r#"{"aclass":"currency","altname":"X","decimals":1,"display_decimals":1,"status":"frozen"}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("frozen"), "{err}");
    }
}
