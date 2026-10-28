use super::orders::Order;
use crate::{
    CommaList, Delivery, Id, KeyedRows, Pair, UnixTime,
    primitives::ListOrObject,
    spot::{Endpoint, PositionStatus, Private, RebaseMultiplier, sealed::Sealed},
};
use kraken_core::OrderSide;
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/OpenPositions`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct OpenPositionsQuery {
    #[serde(skip_serializing_if = "CommaList::is_empty")]
    pub txid: CommaList<Id<Position>>,
    /// Include unrealised profit and loss (`value`, `net`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub docalcs: bool,
    /// `market` merges positions per pair into [`Positions::Consolidated`].
    pub consolidation: Option<String>,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

/// Open margin positions: one row per position, or one per pair when the query
/// asked for `consolidation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Positions {
    ByTxid(KeyedRows<Id<Position>, Position>),
    Consolidated(Vec<ConsolidatedPosition>),
}

impl<'de> Deserialize<'de> for Positions {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match ListOrObject::deserialize(d)? {
            ListOrObject::List(rows) => Self::Consolidated(rows),
            ListOrObject::Object(rows) => Self::ByTxid(rows),
        })
    }
}

/// `class` publishes as `asset_class`; `type` as `side`.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub ordertxid: Id<Order>,
    pub posstatus: PositionStatus,
    pub pair: Pair,
    #[serde(rename(deserialize = "class"))]
    pub asset_class: Option<String>,
    pub time: UnixTime,
    #[serde(rename(deserialize = "type"))]
    pub side: OrderSide,
    pub ordertype: String,
    #[serde(with = "crate::decimal::number")]
    pub cost: Decimal,
    #[serde(with = "crate::decimal::number")]
    pub fee: Decimal,
    #[serde(with = "crate::decimal::number")]
    pub vol: Decimal,
    #[serde(with = "crate::decimal::number")]
    pub vol_closed: Decimal,
    #[serde(with = "crate::decimal::number")]
    pub margin: Decimal,
    pub terms: String,
    /// The next rollover, as sent.
    pub rollovertm: String,
    pub misc: String,
    pub oflags: String,
    #[serde(default, with = "crate::decimal::number::option")]
    pub value: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub net: Option<Decimal>,
}

/// A pair's merged positions. Published as sent.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidatedPosition {
    pub pair: Pair,
    pub class: String,
    pub positions: String,
    #[serde(rename = "type")]
    pub side: OrderSide,
    pub leverage: String,
    #[serde(with = "crate::decimal::string")]
    pub cost: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub fee: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub vol: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub vol_closed: Decimal,
    #[serde(with = "crate::decimal::string")]
    pub margin: Decimal,
    #[serde(default, with = "crate::decimal::string::option")]
    pub value: Option<Decimal>,
    #[serde(default, with = "crate::decimal::string::option")]
    pub net: Option<Decimal>,
}

impl Sealed for OpenPositionsQuery {}
impl Endpoint for OpenPositionsQuery {
    const PATH: &'static str = "OpenPositions";
    type Access = Private;
    type Response = Positions;

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
    fn a_signed_net_decodes() {
        let Positions::ByTxid(rows) = decode_fixture("OpenPositions.json").unwrap() else {
            panic!("the fixture is keyed by txid");
        };
        let position = rows.values().next().unwrap();
        assert_eq!(position.net, Some(dec!(154186.9728)));
        assert_eq!(serde_json::to_value(position).unwrap()["side"], "buy");
    }

    #[test]
    fn a_consolidated_reply_decodes_as_its_array() {
        let json = r#"[{"pair":"XXBTZUSD","class":"margin","positions":"1","type":"buy","leverage":"2","cost":"1","fee":"0","vol":"1","vol_closed":"0","margin":"0.5"}]"#;
        assert!(matches!(
            serde_json::from_str(json).unwrap(),
            Positions::Consolidated(_)
        ));
    }

    #[test]
    fn a_drifted_row_names_its_field() {
        let json = r#"[{"pair":"XXBTZUSD","class":"margin","positions":"1","type":"sideways","leverage":"2","cost":"1","fee":"0","vol":"1","vol_closed":"0","margin":"0.5"}]"#;
        let err = serde_path_to_error::deserialize::<_, Positions>(
            &mut serde_json::Deserializer::from_str(json),
        )
        .unwrap_err();
        assert_eq!(err.path().to_string(), "[0].type");
    }
}
