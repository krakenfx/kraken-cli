use super::pages::PairPage;
use crate::{
    Delivery, Pair, UnixTime,
    decimal::Wire,
    spot::{Endpoint, Public, sealed::Sealed, vocabulary::AssetClass},
};
use kraken_core::OrderSide;
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use serde_with::skip_serializing_none;

/// `GET /0/public/Trades`.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct TradesQuery {
    pub pair: Pair,
    /// Exclusive lower bound: a previous page's `last`.
    pub since: Option<String>,
    pub count: Option<u32>,
    pub asset_class: Option<AssetClass>,
}

/// A page of public trades, oldest first. `pair` is Kraken's wire name until
/// the caller echoes its own spelling into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Trades {
    /// Paging cursor, nanoseconds as a string.
    pub last: String,
    pub pair: Pair,
    pub trades: Vec<Trade>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Trade {
    pub price: Decimal,
    pub volume: Decimal,
    pub time: UnixTime,
    /// The taker's side.
    pub side: OrderSide,
    pub order_type: TradeOrderType,
    pub misc: String,
    pub trade_id: u64,
}

/// The taker order's type, published as Kraken's letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeOrderType {
    #[serde(rename = "l")]
    Limit,
    #[serde(rename = "m")]
    Market,
}

/// `b`/`s` on the wire.
#[derive(Deserialize)]
enum WireSide {
    #[serde(rename = "b")]
    Buy,
    #[serde(rename = "s")]
    Sell,
}

impl From<WireSide> for OrderSide {
    fn from(side: WireSide) -> Self {
        match side {
            WireSide::Buy => Self::Buy,
            WireSide::Sell => Self::Sell,
        }
    }
}

impl<'de> Deserialize<'de> for Trade {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (Wire(price), Wire(volume), time, side, order_type, misc, trade_id): (
            Wire,
            Wire,
            UnixTime,
            WireSide,
            TradeOrderType,
            String,
            u64,
        ) = Deserialize::deserialize(d)?;
        Ok(Self {
            price,
            volume,
            time,
            side: side.into(),
            order_type,
            misc,
            trade_id,
        })
    }
}

impl<'de> Deserialize<'de> for Trades {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let PairPage { pair, rows, last } = PairPage::deserialize(d)?;
        Ok(Self {
            last,
            pair,
            trades: rows,
        })
    }
}

impl Sealed for TradesQuery {}
impl Endpoint for TradesQuery {
    const PATH: &'static str = "Trades";
    type Access = Public;
    type Response = Trades;

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
        let trades: Trades = decode_fixture("Trades.json").unwrap();
        let first = &trades.trades[0];
        assert_eq!(first.side, OrderSide::Sell);
        assert_eq!(first.order_type, TradeOrderType::Market);
        assert_eq!(first.time.as_str(), "1792499896.0352738");
        assert_eq!(trades.last, "1792499896035273824");
    }

    #[test]
    fn an_unknown_side_letter_fails_the_decode() {
        let err = serde_json::from_str::<Trade>(r#"["1","1",1.5,"x","l","",1]"#).unwrap_err();
        assert!(err.to_string().contains("unknown variant `x`"), "{err}");
    }
}
