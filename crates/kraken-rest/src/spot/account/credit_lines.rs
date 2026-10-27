use crate::{
    Asset, Delivery, KeyedRows,
    spot::{Endpoint, Private, RebaseMultiplier, sealed::Sealed},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/CreditLines`. VIP accounts only.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct CreditLinesQuery {
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

/// Credit per asset and the account-wide limits. Published as sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditLines {
    pub asset_details: KeyedRows<Asset, CreditLineAsset>,
    pub limits_monitor: CreditLimits,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditLineAsset {
    #[serde(with = "crate::decimal::string")]
    pub balance: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub hold_trade: Decimal,
    #[serde(default, with = "crate::decimal::string::option")]
    pub credit: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub credit_used: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub collateral_value: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub rollover_fees: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub reserve_fees: Option<Decimal>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditLimits {
    #[serde(default, with = "crate::decimal::string::option")]
    pub total_credit_usd: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub total_credit_used_usd: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub total_collateral_value_usd: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub equity_usd: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub ongoing_balance: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub debt_to_equity: Option<Decimal>,
}

impl Sealed for CreditLinesQuery {}
impl Endpoint for CreditLinesQuery {
    const PATH: &'static str = "CreditLines";
    type Access = Private;
    type Response = CreditLines;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn amounts_publish_their_wire_digits() {
        let lines: CreditLines = decode_fixture("CreditLines.json").unwrap();
        let published = serde_json::to_value(&lines).unwrap();
        assert_eq!(published["asset_details"]["USD"]["balance"], "1000.5000");
    }
}
