use super::{Cursor, Window, orders::Order};
use crate::{
    CommaList, Delivery, Id, KeyedRows, Pair, UnixTime,
    spot::{AssetClass, Endpoint, PositionStatus, Private, RebaseMultiplier, sealed::Sealed},
};
use kraken_core::OrderSide;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// `POST /0/private/TradesHistory`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct TradesHistoryQuery {
    #[serde(rename = "type")]
    pub filter: Option<TradeFilter>,
    /// Include the fill ids related to each position.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub trades: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub consolidate_taker: bool,
    /// Include each fill's ledger ids.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ledgers: bool,
    #[serde(flatten)]
    pub window: Window,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

/// Which fills, by their relation to a margin position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TradeFilter {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "any position")]
    AnyPosition,
    #[serde(rename = "closed position")]
    ClosedPosition,
    #[serde(rename = "closing position")]
    ClosingPosition,
    #[serde(rename = "no position")]
    NoPosition,
}

/// `POST /0/private/QueryTrades`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize)]
pub struct QueryTradesQuery {
    pub txid: CommaList<Id<Fill>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub trades: bool,
    pub rebase_multiplier: Option<RebaseMultiplier>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradesHistory {
    pub trades: KeyedRows<Id<Fill>, Fill>,
    pub count: Option<u64>,
    pub cursor: Option<Cursor>,
}

pub type QueriedFills = KeyedRows<Id<Fill>, Fill>;

/// One of the account's own executions. `type` publishes as `side`; `vol`
/// keeps its wire name, unlike an order row's `volume`, because each
/// endpoint's published fields are pinned separately.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fill {
    pub ledgers: Option<Vec<String>>,
    pub ext_exec_id: Option<String>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub cprice: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub ccost: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub cfee: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub cvol: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub cmargin: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub net: Option<Decimal>,
    /// Fills that closed this position, when it is one.
    pub trades: Option<Vec<Id<Fill>>>,
    pub ordertxid: Id<Order>,
    pub postxid: Option<String>,
    pub posstatus: Option<PositionStatus>,
    pub pair: Pair,
    pub aclass: Option<AssetClass>,
    pub time: UnixTime,
    #[serde(rename(deserialize = "type"))]
    pub side: OrderSide,
    /// Kraken's own spelling; its API descriptions disagree on the token set,
    /// so it is published as sent rather than narrowed.
    pub ordertype: String,
    /// The type of the order that took this fill, as sent (see `ordertype`).
    pub tradeordertype: Option<String>,
    #[serde(with = "crate::decimal::number")]
    pub price: Decimal,
    #[serde(with = "crate::decimal::number")]
    pub cost: Decimal,
    #[serde(with = "crate::decimal::number")]
    pub fee: Decimal,
    #[serde(with = "crate::decimal::number")]
    pub vol: Decimal,
    #[serde(default, with = "crate::decimal::number::option")]
    pub margin: Option<Decimal>,
    #[serde(default, with = "crate::decimal::number::option")]
    pub leverage: Option<Decimal>,
    pub misc: String,
    pub trade_id: Option<u64>,
    pub maker: Option<bool>,
}

impl Sealed for TradesHistoryQuery {}
impl Endpoint for TradesHistoryQuery {
    const PATH: &'static str = "TradesHistory";
    type Access = Private;
    type Response = TradesHistory;

    fn delivery(&self) -> Delivery {
        Delivery::Retryable
    }
}

impl Sealed for QueryTradesQuery {}
impl Endpoint for QueryTradesQuery {
    const PATH: &'static str = "QueryTrades";
    type Access = Private;
    type Response = QueriedFills;

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
    fn a_fill_publishes_its_side() {
        let history: TradesHistory = decode_fixture("TradesHistory.json").unwrap();
        let fill = history.trades.values().next().unwrap();
        assert_eq!(fill.vol, dec!(0.02));
        assert_eq!(serde_json::to_value(fill).unwrap()["side"], "buy");
    }

    #[test]
    fn queried_fills_share_the_fill_shape() {
        let fills: QueriedFills = decode_fixture("QueryTrades.json").unwrap();
        assert_eq!(fills.len(), 2);
    }

    #[test]
    fn a_position_filter_keeps_its_spaces() {
        let query = TradesHistoryQuery {
            filter: Some(TradeFilter::ClosedPosition),
            ..TradesHistoryQuery::default()
        };
        assert_eq!(
            serde_json::to_string(&query).unwrap(),
            r#"{"type":"closed position"}"#
        );
    }
}
