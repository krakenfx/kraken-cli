use super::{Cursor, Window};
use crate::{
    Asset, CommaList, Delivery, Id, KeyedRows, UnixTime,
    spot::{AssetClass, Endpoint, LedgerType, Private, RebaseMultiplier, sealed::Sealed},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/Ledgers`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct LedgerQuery {
    /// Comma-separated asset codes.
    pub asset: Option<String>,
    pub aclass: Option<AssetClass>,
    /// Entry type filter, as Kraken spells it (`trade`, `deposit`, `all`).
    #[serde(rename = "type")]
    pub entry_type: Option<String>,
    #[serde(flatten)]
    pub window: Window,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

/// `POST /0/private/QueryLedgers`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct QueryLedgerQuery {
    pub id: CommaList<Id<LedgerEntry>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub trades: bool,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    pub ledger: KeyedRows<Id<LedgerEntry>, LedgerEntry>,
    pub count: Option<u64>,
    pub cursor: Option<Cursor>,
}

pub type QueriedLedger = KeyedRows<Id<LedgerEntry>, LedgerEntry>;

/// One movement of one asset. `type` publishes as `ledger_type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// The trade, deposit or withdrawal that caused the entry.
    pub refid: String,
    pub time: UnixTime,
    #[serde(rename(deserialize = "type"))]
    pub ledger_type: LedgerType,
    pub subtype: String,
    pub aclass: AssetClass,
    pub asset: Asset,
    #[serde(with = "crate::decimal::number")]
    pub amount: Decimal,
    #[serde(with = "crate::decimal::number")]
    pub fee: Decimal,
    /// The asset's balance after the entry.
    #[serde(with = "crate::decimal::number")]
    pub balance: Decimal,
}

impl Sealed for LedgerQuery {}
impl Endpoint for LedgerQuery {
    const PATH: &'static str = "Ledgers";
    type Access = Private;
    type Response = Ledger;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for QueryLedgerQuery {}
impl Endpoint for QueryLedgerQuery {
    const PATH: &'static str = "QueryLedgers";
    type Access = Private;
    type Response = QueriedLedger;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spot::envelope::decode_fixture;

    #[test]
    fn an_entry_publishes_its_type_and_modern_asset() {
        let ledger: Ledger = decode_fixture("Ledgers.json").unwrap();
        let entry = ledger.ledger.values().next().unwrap();
        assert_eq!(entry.ledger_type, LedgerType::Trade);
        let published = serde_json::to_value(entry).unwrap();
        assert_eq!(published["ledger_type"], "trade");
        assert_eq!(published["asset"], "GBP");
    }

    #[test]
    fn a_new_entry_type_passes_through_verbatim() {
        let entry: LedgerType = serde_json::from_str(r#""futurebonus""#).unwrap();
        assert_eq!(entry, LedgerType::Unrecognized("futurebonus".into()));
        assert_eq!(serde_json::to_string(&entry).unwrap(), r#""futurebonus""#);
    }
}
