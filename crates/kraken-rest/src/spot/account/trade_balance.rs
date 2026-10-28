use crate::{
    Delivery,
    spot::{Endpoint, Private, RebaseMultiplier, sealed::Sealed},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/TradeBalance`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct TradeBalanceQuery {
    /// The asset the summary is stated in; Kraken defaults to USD.
    pub asset: Option<String>,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

/// The margin summary, Kraken's two-letter keys spelled out. The margin-only
/// fields are absent for an account with no open margin.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeBalance {
    #[serde(rename(deserialize = "eb"), with = "crate::decimal::number")]
    pub equivalent_balance: Decimal,
    #[serde(rename(deserialize = "tb"), with = "crate::decimal::number")]
    pub trade_balance: Decimal,
    #[serde(rename(deserialize = "m"), with = "crate::decimal::number")]
    pub margin_amount: Decimal,
    #[serde(rename(deserialize = "n"), with = "crate::decimal::number")]
    pub unrealized_pnl: Decimal,
    #[serde(rename(deserialize = "c"), with = "crate::decimal::number")]
    pub cost_basis: Decimal,
    #[serde(rename(deserialize = "v"), with = "crate::decimal::number")]
    pub floating_valuation: Decimal,
    #[serde(rename(deserialize = "e"), with = "crate::decimal::number")]
    pub equity: Decimal,
    #[serde(rename(deserialize = "mf"), with = "crate::decimal::number")]
    pub free_margin: Decimal,
    #[serde(
        rename(deserialize = "ml"),
        default,
        with = "crate::decimal::number::option"
    )]
    pub margin_level_pct: Option<Decimal>,
    #[serde(
        rename(deserialize = "uv"),
        default,
        with = "crate::decimal::number::option"
    )]
    pub unrealized_value: Option<Decimal>,
    #[serde(
        rename(deserialize = "mfo"),
        default,
        with = "crate::decimal::number::option"
    )]
    pub free_margin_original: Option<Decimal>,
}

impl Sealed for TradeBalanceQuery {}
impl Endpoint for TradeBalanceQuery {
    const PATH: &'static str = "TradeBalance";
    type Access = Private;
    type Response = TradeBalance;

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
    fn two_letter_keys_publish_spelled_out() {
        let balance: TradeBalance = decode_fixture("TradeBalance.json").unwrap();
        assert_eq!(balance.equivalent_balance, dec!(1101.3425));
        let published = serde_json::to_value(&balance).unwrap();
        assert!(published.get("eb").is_none());
        assert!(
            published.get("unrealized_value").is_none(),
            "absent stays absent"
        );
    }
}
