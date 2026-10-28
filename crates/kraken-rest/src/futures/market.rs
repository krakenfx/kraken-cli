//! Public market data.

use super::{
    Endpoint, Greeks, MarginLevel, Method, Number, PathSegment, Public, Reply, sealed::Sealed,
};
use crate::KeyedRows;
use kraken_core::OrderSide;
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeTuple as _};
use serde_with::skip_serializing_none;
use std::borrow::Cow;

/// `GET /instruments`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct InstrumentsQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instruments {
    pub instruments: Vec<Instrument>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instrument {
    pub symbol: String,
    pub pair: Option<String>,
    pub base: Option<String>,
    pub quote: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<InstrumentType>,
    pub underlying: Option<String>,
    pub underlying_future: Option<String>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub tick_size: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub contract_size: Option<Decimal>,
    pub tradeable: bool,
    #[serde(default, with = "crate::decimal::json::option")]
    pub impact_mid_size: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub max_position_size: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub max_open_interest_usd: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub max_open_interest_share: Option<Decimal>,
    pub opening_date: Option<String>,
    pub last_trading_time: Option<String>,
    pub margin_schedules: Option<KeyedRows<String, MarginSchedule>>,
    pub margin_levels: Option<Vec<MarginLevel>>,
    pub retail_margin_levels: Option<Vec<MarginLevel>>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub funding_rate_coefficient: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub max_relative_funding_rate: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub min_relative_funding_rate: Option<Decimal>,
    pub isin: Option<String>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub contract_value_trade_precision: Option<Decimal>,
    pub post_only: Option<bool>,
    pub suspended: Option<bool>,
    pub fee_schedule_uid: Option<String>,
    pub maker_protection_millis: Option<i64>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub tags: Option<Vec<String>>,
    /// Market-share level to rebate, both as decimals.
    pub rebate_levels: Option<KeyedRows<String, super::Number>>,
    pub tradfi: bool,
    pub mtf: Option<bool>,
    pub platforms_permitted: Vec<String>,
    pub countries_banned: Vec<String>,
    pub is_expired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarginSchedule {
    pub retail: Vec<MarginLevel>,
    pub professional: Vec<MarginLevel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentType {
    FlexibleFutures,
    FuturesInverse,
    FuturesVanilla,
    Options,
}

/// `GET /tickers`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TickersQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tickers {
    pub tickers: Vec<Ticker>,
}

/// `GET /tickers/{symbol}`.
#[derive(Debug, Clone, Serialize)]
pub struct TickerQuery {
    #[serde(skip)]
    pub symbol: PathSegment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SingleTicker {
    pub ticker: Ticker,
}

/// A contract's ticker, or an index's: an index has only a last value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Ticker {
    Market(Box<MarketTicker>),
    Index(IndexTicker),
}

/// Told apart by `markPrice`, which only a contract has. An untagged fallback
/// would read a contract that fails to decode as an index, dropping its fields.
impl<'de> Deserialize<'de> for Ticker {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        if value.get("markPrice").is_some() {
            MarketTicker::deserialize(value)
                .map(|ticker| Self::Market(Box::new(ticker)))
                .map_err(serde::de::Error::custom)
        } else {
            IndexTicker::deserialize(value)
                .map(Self::Index)
                .map_err(serde::de::Error::custom)
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketTicker {
    pub tag: ContractTag,
    pub pair: String,
    pub symbol: String,
    #[serde(with = "crate::decimal::json")]
    pub mark_price: Decimal,
    #[serde(default, with = "crate::decimal::json::option")]
    pub bid: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub bid_size: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub ask: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub ask_size: Option<Decimal>,
    #[serde(with = "crate::decimal::json")]
    pub vol24h: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub volume_quote: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub open_interest: Decimal,
    #[serde(default, with = "crate::decimal::json::option")]
    pub open24h: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub high24h: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub low24h: Option<Decimal>,
    /// Sent live, though not in the API description.
    #[serde(default, with = "crate::decimal::json::option")]
    pub vwap24h: Option<Decimal>,
    #[serde(with = "crate::decimal::json")]
    pub index_price: Decimal,
    #[serde(default, with = "crate::decimal::json::option")]
    pub last: Option<Decimal>,
    pub last_time: Option<String>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub last_size: Option<Decimal>,
    pub suspended: bool,
    #[serde(default, with = "crate::decimal::json::option")]
    pub funding_rate: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub funding_rate_prediction: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub relative_funding_rate: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub relative_funding_rate_prediction: Option<Decimal>,
    #[serde(default, with = "crate::decimal::json::option")]
    pub extrinsic_value: Option<Decimal>,
    pub post_only: bool,
    #[serde(with = "crate::decimal::json")]
    pub change24h: Decimal,
    pub greeks: Option<Greeks>,
    pub is_underlying_market_closed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexTicker {
    pub symbol: String,
    #[serde(with = "crate::decimal::json")]
    pub last: Decimal,
    pub last_time: String,
}

/// A contract's expiry grouping. Open: Kraken documents that it adds tags
/// without notice.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractTag {
    Perpetual,
    Week,
    Month,
    Quarter,
    Semiannual,
    #[serde(untagged, deserialize_with = "crate::primitives::unrecognized")]
    Unrecognized(String),
}

/// `GET /orderbook`.
#[derive(Debug, Clone, Serialize)]
pub struct OrderbookQuery {
    pub symbol: PathSegment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Orderbook {
    pub order_book: Book,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Book {
    pub asks: Vec<BookLevel>,
    pub bids: Vec<BookLevel>,
}

/// `[price, size]`, published as that pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookLevel {
    pub price: Decimal,
    pub size: Decimal,
}

impl Serialize for BookLevel {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut pair = s.serialize_tuple(2)?;
        pair.serialize_element(&Number(self.price))?;
        pair.serialize_element(&Number(self.size))?;
        pair.end()
    }
}

impl<'de> Deserialize<'de> for BookLevel {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (Number(price), Number(size)) = Deserialize::deserialize(d)?;
        Ok(Self { price, size })
    }
}

/// `GET /history`: a contract's recent public trades.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct TradeHistoryQuery {
    pub symbol: PathSegment,
    pub since: Option<String>,
    pub before: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeHistory {
    pub history: Vec<PublicTrade>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicTrade {
    pub time: String,
    pub trade_id: Option<i64>,
    #[serde(with = "crate::decimal::json")]
    pub price: Decimal,
    #[serde(default, with = "crate::decimal::json::option")]
    pub size: Option<Decimal>,
    pub side: Option<OrderSide>,
    #[serde(rename = "type")]
    pub kind: Option<PublicTradeType>,
    pub uid: Option<String>,
    pub sequence_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PublicTradeType {
    #[serde(rename = "fill")]
    Fill,
    #[serde(rename = "liquidation")]
    Liquidation,
    #[serde(rename = "partial liquidation")]
    PartialLiquidation,
    #[serde(rename = "assignment")]
    Assignment,
    #[serde(rename = "unwind")]
    Unwind,
    #[serde(rename = "rfq")]
    Rfq,
    #[serde(rename = "termination")]
    Termination,
    #[serde(rename = "block")]
    Block,
}

/// `GET /feeschedules`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FeeSchedulesQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeSchedules {
    pub fee_schedules: Vec<FeeSchedule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeSchedule {
    pub uid: String,
    pub name: String,
    pub tiers: Vec<FeeTier>,
}

/// From `usdVolume` (30 days), these fees in percent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeTier {
    #[serde(with = "crate::decimal::json")]
    pub maker_fee: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub taker_fee: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub usd_volume: Decimal,
}

/// `GET /instruments/status`: every contract's.
#[derive(Debug, Clone, Default, Serialize)]
pub struct InstrumentStatusesQuery {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstrumentStatuses {
    pub instrument_status: Vec<InstrumentStatus>,
}

/// `GET /instruments/{symbol}/status`: one contract's.
#[derive(Debug, Clone, Serialize)]
pub struct InstrumentStatusQuery {
    #[serde(skip)]
    pub symbol: PathSegment,
}

/// Whether a contract's price has broken away from its index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstrumentStatus {
    pub tradeable: String,
    pub experiencing_dislocation: bool,
    pub price_dislocation_direction: Option<DislocationDirection>,
    pub experiencing_extreme_volatility: bool,
    pub extreme_volatility_initial_margin_multiplier: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DislocationDirection {
    AboveUpperBound,
    BelowLowerBound,
}

/// `GET /historical-funding-rates`.
#[derive(Debug, Clone, Serialize)]
pub struct FundingRatesQuery {
    pub symbol: PathSegment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FundingRates {
    pub rates: Vec<FundingRate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FundingRate {
    pub timestamp: String,
    #[serde(with = "crate::decimal::json")]
    pub funding_rate: Decimal,
    #[serde(with = "crate::decimal::json")]
    pub relative_funding_rate: Decimal,
}

macro_rules! public {
    ($request:ty, $path:literal, $body:ty) => {
        impl Sealed for $request {}
        impl Endpoint for $request {
            const PATH: &'static str = $path;
            const METHOD: Method = Method::Get;
            type Access = Public;
            type Response = Reply<$body>;
        }
    };
}

public!(InstrumentsQuery, "instruments", Instruments);
public!(TickersQuery, "tickers", Tickers);
public!(OrderbookQuery, "orderbook", Orderbook);
public!(TradeHistoryQuery, "history", TradeHistory);
public!(FeeSchedulesQuery, "feeschedules", FeeSchedules);
public!(
    InstrumentStatusesQuery,
    "instruments/status",
    InstrumentStatuses
);
public!(FundingRatesQuery, "historical-funding-rates", FundingRates);

impl Sealed for TickerQuery {}
impl Endpoint for TickerQuery {
    const PATH: &'static str = "tickers/{symbol}";
    const METHOD: Method = Method::Get;
    type Access = Public;
    type Response = Reply<SingleTicker>;

    fn path(&self) -> Cow<'static, str> {
        format!("tickers/{}", self.symbol).into()
    }
}

impl Sealed for InstrumentStatusQuery {}
impl Endpoint for InstrumentStatusQuery {
    const PATH: &'static str = "instruments/{symbol}/status";
    const METHOD: Method = Method::Get;
    type Access = Public;
    type Response = Reply<InstrumentStatus>;

    fn path(&self) -> Cow<'static, str> {
        format!("instruments/{}/status", self.symbol).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::futures::envelope::{assert_round_trips, decode_fixture};

    #[test]
    fn live_public_replies_round_trip_unchanged() {
        assert_round_trips::<Reply<Instruments>>("trading.get_instruments.live.json");
        assert_round_trips::<Reply<Tickers>>("trading.get_tickers.live.json");
        assert_round_trips::<Reply<SingleTicker>>("trading.get_tickers_symbol.live.json");
        assert_round_trips::<Reply<Orderbook>>("trading.get_orderbook.live.json");
        assert_round_trips::<Reply<TradeHistory>>("trading.get_history.live.json");
        assert_round_trips::<Reply<FeeSchedules>>("trading.get_feeschedules.live.json");
        assert_round_trips::<Reply<InstrumentStatuses>>("trading.get_instruments_status.live.json");
        assert_round_trips::<Reply<InstrumentStatus>>(
            "trading.get_instruments_symbol_status.live.json",
        );
        assert_round_trips::<Reply<FundingRates>>("trading.get_historical-funding-rates.live.json");
    }

    #[test]
    fn a_contract_ticker_that_fails_to_decode_is_not_read_as_an_index() {
        let broken = serde_json::json!({
            "symbol": "PF_XBTUSD", "markPrice": 1, "last": 1, "lastTime": "t"
        });
        let err = serde_json::from_value::<Ticker>(broken).unwrap_err();
        assert!(err.to_string().contains("missing field"), "{err}");
    }

    #[test]
    fn decodes_the_public_replies() {
        let _: Reply<Instruments> = decode_fixture("trading.get_instruments.json").unwrap();
        let _: Reply<Orderbook> = decode_fixture("trading.get_orderbook.json").unwrap();
        let _: Reply<TradeHistory> = decode_fixture("trading.get_history.json").unwrap();
        let _: Reply<FeeSchedules> = decode_fixture("trading.get_feeschedules.json").unwrap();
        let _: Reply<InstrumentStatuses> =
            decode_fixture("trading.get_instruments_status.json").unwrap();
        let _: Reply<InstrumentStatus> =
            decode_fixture("trading.get_instruments_symbol_status.json").unwrap();
        let _: Reply<FundingRates> =
            decode_fixture("trading.get_historical-funding-rates.json").unwrap();
        let _: Reply<SingleTicker> = decode_fixture("trading.get_tickers_symbol.json").unwrap();
    }

    #[test]
    fn index_tickers_decode_beside_contract_tickers() {
        let tickers: Reply<Tickers> = decode_fixture("trading.get_tickers.json").unwrap();
        let index = tickers
            .body
            .tickers
            .iter()
            .filter(|t| matches!(t, Ticker::Index(_)))
            .count();
        assert_eq!(index, 2);
    }

    #[test]
    fn a_book_level_publishes_its_wire_numbers() {
        let level: BookLevel = serde_json::from_str("[81348, 0.3033]").unwrap();
        assert_eq!(serde_json::to_string(&level).unwrap(), "[81348,0.3033]");
    }

    #[test]
    fn a_path_symbol_cannot_reach_another_path() {
        assert!(PathSegment::new("symbol", "../accounts").is_err());
        assert!(PathSegment::new("symbol", "PF_XBTUSD?x=1").is_err());
        assert!(PathSegment::new("symbol", "PF_XBTUSD").is_ok());
    }
}
