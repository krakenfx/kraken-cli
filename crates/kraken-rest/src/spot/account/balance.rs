use crate::{
    Asset, Delivery, KeyedRows,
    decimal::Wire,
    spot::{Endpoint, Private, RebaseMultiplier, sealed::Sealed},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/Balance`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct BalanceQuery {
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

/// Total balance per asset, keyed by modern asset code.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(transparent)]
pub struct Balances(KeyedRows<Asset, Decimal>);

impl Balances {
    pub fn iter(&self) -> impl Iterator<Item = (&Asset, &Decimal)> {
        self.0.iter().map(|(asset, amount)| (asset, amount))
    }

    pub fn get(&self, asset: &Asset) -> Option<Decimal> {
        self.0.get(asset).copied()
    }
}

impl<'de> Deserialize<'de> for Balances {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let rows = KeyedRows::<Asset, Wire>::deserialize(d)?;
        Ok(Self(
            rows.into_iter()
                .map(|(asset, Wire(amount))| (asset, amount))
                .collect(),
        ))
    }
}

/// `POST /0/private/BalanceEx`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExtendedBalanceQuery {
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

pub type ExtendedBalances = KeyedRows<Asset, ExtendedBalance>;

/// `credit` and `credit_used` are present only for credit-enabled keys.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtendedBalance {
    #[serde(with = "crate::decimal::number")]
    pub balance: Decimal,
    #[serde(with = "crate::decimal::number")]
    pub hold_trade: Decimal,
    #[serde(default, with = "crate::decimal::number::option")]
    pub credit: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub credit_used: Option<Decimal>,
}

impl Sealed for BalanceQuery {}
impl Endpoint for BalanceQuery {
    const PATH: &'static str = "Balance";
    type Access = Private;
    type Response = Balances;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for ExtendedBalanceQuery {}
impl Endpoint for ExtendedBalanceQuery {
    const PATH: &'static str = "BalanceEx";
    type Access = Private;
    type Response = ExtendedBalances;

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
    fn balances_key_modern_codes() {
        let balances: Balances = decode_fixture("Balance.json").unwrap();
        assert_eq!(
            balances.get(&Asset::from("XXBT")),
            Some(dec!(1011.19088779))
        );
        assert!(balances.get(&Asset::from("BTC")).is_some());
    }

    #[test]
    fn credit_fields_stay_absent_without_credit() {
        let balances: ExtendedBalances = decode_fixture("BalanceEx.json").unwrap();
        let published = serde_json::to_value(&balances).unwrap();
        assert!(published["USD"].get("credit").is_none(), "{published}");
    }
}
