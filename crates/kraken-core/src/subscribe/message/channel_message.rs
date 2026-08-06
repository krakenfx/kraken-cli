//! The typed channel-data frame: one shared envelope around a channel-tagged payload,
//! mirroring the reply side's `MethodResult`.

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use super::super::channel::{Channel, SubscribableChannel};
use super::balances::BalanceData;
use super::book::BookData;
use super::envelope::MessageType;
use super::executions::ExecutionData;
use super::instrument::InstrumentData;
use super::level3::Level3Data;
use super::ohlc::OhlcData;
use super::status::StatusData;
use super::ticker::TickerData;
use super::trade::TradeData;

/// A strictly-typed, parsed channel-data frame: the shared `{channel, type, data,
/// sequence?}` envelope, with the channel-tagged payload flattened in so a frame
/// serializes back to its natural wire shape. Account channels carry a `sequence`;
/// market channels do not. Unknown top-level fields are ignored so the payload stays
/// the single source of truth.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelMessage {
    #[serde(flatten)]
    pub body: ChannelData,
    #[serde(rename = "type")]
    pub message_type: MessageType,
    pub sequence: Option<u64>,
}

/// The wire `channel` name paired with its `data` payload — serde dispatches on the
/// frame's own `channel` field, so the channel names its payload schema and a payload
/// that no longer deserializes is a hard error: protocol drift surfaces loud, per
/// frame, instead of flowing on mistyped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "channel", content = "data", rename_all = "lowercase")]
pub enum ChannelData {
    Ticker(Vec<TickerData>),
    Trade(Vec<TradeData>),
    Book(Vec<BookData>),
    Ohlc(Vec<OhlcData>),
    /// The one object-shaped payload: a single reference-data catalogue, not entries.
    Instrument(InstrumentData),
    Executions(Vec<ExecutionData>),
    Balances(Vec<BalanceData>),
    Level3(Vec<Level3Data>),
    Status(Vec<StatusData>),
}

impl ChannelMessage {
    /// Decode a raw frame string — a test/diagnostic convenience over the full
    /// classifier ([`Inbound`](crate::Inbound)'s `FromStr`). `None` for
    /// anything that is not a channel-data frame.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.parse::<crate::Inbound>() {
            Ok(crate::Inbound::Channel(message)) => Some(message),
            _ => None,
        }
    }

    /// The wire channel this frame belongs to.
    pub fn channel(&self) -> Channel {
        self.body.channel()
    }

    /// Whether this is a `snapshot` frame — the initial backfill the server replays on
    /// subscribe and on every reconnect — rather than a live `update`. Snapshot timestamps
    /// describe historical data, so the live monitor excludes them from latency measurement.
    pub fn is_snapshot(&self) -> bool {
        self.message_type == MessageType::Snapshot
    }

    /// One representative event instant for the frame — see
    /// [`ChannelData::event_ts`] for the rule. `None` when the payload
    /// carries no per-entry event time.
    pub fn event_ts(&self) -> Option<&str> {
        self.body.event_ts()
    }
}

impl ChannelData {
    /// Whether any enum-typed field in this payload decoded through an `Unknown`
    /// fallback — vocabulary Kraken added after this crate's catalogue was pinned.
    /// The drift-observability hook: the connection actor warns on it, so tolerated
    /// growth is still logged, never silent. Each payload walks its own vocabulary;
    /// a new `Unknown` fallback must join its payload's walk.
    pub fn has_unknown_vocabulary(&self) -> bool {
        match self {
            Self::Ticker(_) | Self::Trade(_) | Self::Book(_) | Self::Ohlc(_) => false,
            Self::Instrument(data) => data.has_unknown_vocabulary(),
            Self::Balances(entries) => entries.iter().any(BalanceData::has_unknown_vocabulary),
            Self::Executions(entries) => entries.iter().any(ExecutionData::has_unknown_vocabulary),
            Self::Level3(entries) => entries.iter().any(Level3Data::has_unknown_vocabulary),
            Self::Status(entries) => entries.iter().any(StatusData::has_unknown_vocabulary),
        }
    }

    /// The wire channel this payload belongs to — the one payload → channel mapping.
    pub fn channel(&self) -> Channel {
        use SubscribableChannel as Sub;
        match self {
            Self::Ticker(_) => Channel::Subscribable(Sub::Ticker),
            Self::Trade(_) => Channel::Subscribable(Sub::Trade),
            Self::Book(_) => Channel::Subscribable(Sub::Book),
            Self::Ohlc(_) => Channel::Subscribable(Sub::Ohlc),
            Self::Instrument(_) => Channel::Subscribable(Sub::Instrument),
            Self::Executions(_) => Channel::Subscribable(Sub::Executions),
            Self::Balances(_) => Channel::Subscribable(Sub::Balances),
            Self::Level3(_) => Channel::Subscribable(Sub::Level3),
            Self::Status(_) => Channel::Status,
        }
    }

    /// One representative event instant for a payload whose entries each carry
    /// their own: the *last* entry's timestamp, RFC3339 as delivered (candles
    /// fall back per [`OhlcData::event_ts`]). The choice of entry is this
    /// crate's convention, not a wire guarantee — defined once so every
    /// consumer that needs a single instant per frame agrees on it. Only the
    /// four market channels stamp per-entry times; every other payload is `None`.
    pub fn event_ts(&self) -> Option<&str> {
        match self {
            Self::Ticker(entries) => entries.last().map(|t| t.timestamp.as_str()),
            Self::Trade(entries) => entries.last().map(|t| t.timestamp.as_str()),
            Self::Book(entries) => entries.last().map(|b| b.timestamp.as_str()),
            Self::Ohlc(entries) => entries.last().map(OhlcData::event_ts),
            Self::Instrument(_)
            | Self::Executions(_)
            | Self::Balances(_)
            | Self::Level3(_)
            | Self::Status(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::super::executions::{ExecType, LiquidityIndicator, OrderStatus};
    use super::super::level3::Level3Event;
    use super::super::status::SystemStatus;
    use super::*;
    use crate::PriceType;

    /// Decode `raw` and pin the round trip: the typed frame must re-serialize to
    /// the captured wire value.
    fn round_trips(raw: &str) -> ChannelMessage {
        let msg = ChannelMessage::parse(raw).expect("frame decodes");
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            serde_json::from_str::<Value>(raw).unwrap(),
            "typed frame must re-serialize to the captured wire value"
        );
        msg
    }

    #[test]
    fn parses_trade_into_typed_variant() {
        let raw = r#"{"channel":"trade","type":"update","data":[{
            "symbol":"BTC/USD","side":"buy","price":50000.1,"qty":0.5,
            "ord_type":"market","trade_id":42,"timestamp":"2026-01-01T00:00:00.000000Z"}]}"#;
        let ChannelData::Trade(data) = round_trips(raw).body else {
            panic!("expected Trade");
        };
        assert_eq!(data[0].price, "50000.1".parse().unwrap());
    }

    #[test]
    fn ticker_round_trips_the_captured_wire_shape() {
        // Field set as delivered live by wss://ws.kraken.com/v2 (2026-07).
        let raw = r#"{"channel":"ticker","type":"update","data":[{
            "symbol":"BTC/USD","bid":62710.4,"bid_qty":0.01,"ask":62710.5,
            "ask_qty":0.93111634,"last":62710.9,"volume":1691.31962925,"vwap":62538.9,
            "low":61700.0,"high":63200.0,"change":220.1,"change_pct":0.35,
            "timestamp":"2026-07-09T17:20:01.123456Z"}]}"#;
        let ChannelData::Ticker(data) = round_trips(raw).body else {
            panic!("expected Ticker");
        };
        assert_eq!(data[0].bid, "62710.4".parse().unwrap());
        assert_eq!(data[0].change_pct, "0.35".parse().unwrap());
    }

    #[test]
    fn book_update_round_trips_including_the_checksum() {
        // A checksummed update with a qty-0 removal level — both must survive the
        // round trip byte-exactly or checksum verification breaks downstream.
        let raw = r#"{"channel":"book","type":"update","data":[{
            "symbol":"BTC/USD",
            "bids":[{"price":62710.4,"qty":0.01},{"price":62709.1,"qty":0.0}],
            "asks":[{"price":62710.5,"qty":0.93111634}],
            "checksum":1361442827,"timestamp":"2026-07-09T17:20:01.123456Z"}]}"#;
        let ChannelData::Book(data) = round_trips(raw).body else {
            panic!("expected Book");
        };
        assert_eq!(data[0].checksum, 1_361_442_827);
        assert_eq!(data[0].bids[1].qty, "0".parse().unwrap());
    }

    #[test]
    fn ohlc_round_trips_with_and_without_the_deprecated_timestamp() {
        // https://docs.kraken.com/api/docs/websocket-v2/ohlc — `timestamp` is
        // deprecated wire-side; both shapes must round-trip.
        let with = r#"{"channel":"ohlc","type":"update","data":[{
            "symbol":"BTC/USD","open":62500.0,"high":62800.5,"low":62400.1,
            "close":62710.9,"vwap":62538.9,"trades":314,"volume":42.5,
            "interval_begin":"2026-07-09T17:15:00.000000Z","interval":5,
            "timestamp":"2026-07-09T17:20:00.000000Z"}]}"#;
        let ChannelData::Ohlc(data) = round_trips(with).body else {
            panic!("expected Ohlc");
        };
        assert_eq!(data[0].trades, 314);

        let without = r#"{"channel":"ohlc","type":"update","data":[{
            "symbol":"BTC/USD","open":62500.0,"high":62800.5,"low":62400.1,
            "close":62710.9,"vwap":62538.9,"trades":314,"volume":42.5,
            "interval_begin":"2026-07-09T17:15:00.000000Z","interval":5}]}"#;
        let ChannelData::Ohlc(data) = round_trips(without).body else {
            panic!("expected Ohlc");
        };
        assert_eq!(data[0].timestamp, None);
    }

    #[test]
    fn decodes_instrument_reference_data() {
        let raw = r#"{"channel":"instrument","type":"snapshot","data":{
            "assets":[{"id":"USD","status":"enabled","precision":4,"precision_display":2,
                "borrowable":true,"collateral_value":1.0,"margin_rate":0.015}],
            "pairs":[{"symbol":"BTC/USD","base":"BTC","quote":"USD","status":"online",
                "qty_precision":8,"qty_increment":1e-08,"price_precision":1,"cost_precision":5,
                "marginable":true,"has_index":true,"cost_min":0.5,"margin_initial":0.2,
                "position_limit_long":250,"position_limit_short":200,"tick_size":0.1,
                "price_increment":0.1,"qty_min":0.0001}]}}"#;
        let ChannelData::Instrument(data) = round_trips(raw).body else {
            panic!("expected Instrument");
        };
        assert_eq!(data.assets[0].id, "USD");
        assert_eq!(data.pairs[0].symbol, "BTC/USD");
    }

    #[test]
    fn decodes_execution_trade_event() {
        let raw = r#"{"channel":"executions","type":"update","data":[{
            "order_id":"OK4GJX-KSTLS-7DZZO5","order_userref":3,"exec_id":"TGBB7L-HT5LX-J3BZ4A",
            "exec_type":"trade","trade_id":62887576,"symbol":"BTC/USD","side":"sell",
            "last_qty":0.005,"last_price":26599.9,"liquidity_ind":"t","cost":132.9995,
            "order_type":"limit","timestamp":"2023-09-22T10:33:05.709993Z",
            "order_status":"partially_filled","cum_qty":0.005,"cum_cost":132.9995,
            "avg_price":26599.9,"fee_usd_equiv":0.3458,
            "fees":[{"asset":"USD","qty":0.3458}]}],"sequence":10}"#;
        let msg = round_trips(raw);
        assert_eq!(msg.sequence, Some(10));
        let ChannelData::Executions(data) = msg.body else {
            panic!("expected Executions");
        };
        assert_eq!(data[0].exec_type, ExecType::Trade);
        assert_eq!(data[0].order_status, Some(OrderStatus::PartiallyFilled));
        assert_eq!(data[0].liquidity_ind, Some(LiquidityIndicator::Taker));
        assert_eq!(data[0].last_price, Some("26599.9".parse().unwrap()));
    }

    #[test]
    fn decodes_execution_pending_new_event() {
        let raw = r#"{"channel":"executions","type":"update","data":[{
            "order_id":"OK4GJX-KSTLS-7DZZO5","order_userref":3,"symbol":"BTC/USD",
            "order_qty":0.005,"cum_cost":0.0,"time_in_force":"GTC",
            "exec_type":"pending_new","side":"sell","order_type":"limit",
            "limit_price_type":"static","limit_price":26500.0,"stop_price":0.0,
            "order_status":"pending_new","fee_usd_equiv":0.0,"fee_ccy_pref":"fciq",
            "timestamp":"2023-09-22T10:33:05.709950Z"}],"sequence":8}"#;
        let ChannelData::Executions(data) = round_trips(raw).body else {
            panic!("expected Executions");
        };
        assert_eq!(data[0].exec_type, ExecType::PendingNew);
        assert_eq!(data[0].order_status, Some(OrderStatus::PendingNew));
        assert_eq!(data[0].limit_price_type, Some(PriceType::Static));
    }

    #[test]
    fn decodes_balance_ledger_update() {
        let raw = r#"{"channel":"balances","type":"update","data":[{
            "ledger_id":"AAICKV-NMQSR-ZO5IJD","ref_id":"AGBB7L-HT5LX-J3BB4A",
            "timestamp":"2023-09-22T10:33:05.710082Z","type":"trade","asset":"BTC",
            "asset_class":"currency","category":"trade","wallet_type":"spot",
            "wallet_id":"main","amount":-0.005,"fee":0.0,"balance":0.005}],"sequence":9}"#;
        let ChannelData::Balances(data) = round_trips(raw).body else {
            panic!("expected Balances");
        };
        assert_eq!(data[0].asset, "BTC");
        assert_eq!(data[0].amount, Some("-0.005".parse().unwrap()));
        assert_eq!(
            data[0].event_type,
            Some(super::super::balances::LedgerEntryType::Trade)
        );
    }

    #[test]
    fn decodes_level3_order_events() {
        let raw = r#"{"channel":"level3","type":"update","data":[{
            "checksum":2841398499,"symbol":"MATIC/USD","bids":[],
            "asks":[{"event":"delete","order_id":"OOIATY-6EIWY-ACVIUN",
                "limit_price":0.5636,"order_qty":302.89736033,
                "timestamp":"2023-10-06T18:21:00.097010033Z"}]}]}"#;
        let ChannelData::Level3(data) = round_trips(raw).body else {
            panic!("expected Level3");
        };
        assert_eq!(data[0].asks[0].event, Some(Level3Event::Delete));
        assert!(data[0].bids.is_empty());
    }

    #[test]
    fn decodes_status_announcement() {
        let raw = r#"{"channel":"status","type":"update","data":[{
            "system":"online","api_version":"v2","connection_id":1,"version":"2.0"}]}"#;
        let ChannelData::Status(data) = round_trips(raw).body else {
            panic!("expected Status");
        };
        assert_eq!(data[0].system, SystemStatus::Online);
    }

    #[test]
    fn extra_field_on_a_typed_channel_is_not_dropped() {
        // Market payloads tolerate unknown fields on decode but do not carry them
        // (pinned hot-path schemas, no `extra` catch-all — see `ExtraFields`);
        // account/reference payloads preserve them, pinned below.
        let raw = r#"{"channel":"trade","type":"update","data":[{
            "symbol":"BTC/USD","side":"buy","price":50000.1,"qty":0.5,
            "ord_type":"market","trade_id":42,"timestamp":"TS","new_exchange_field":true}]}"#;
        assert!(matches!(
            ChannelMessage::parse(raw),
            Some(ChannelMessage {
                body: ChannelData::Trade(_),
                ..
            })
        ));
    }

    #[test]
    fn unknown_wire_fields_survive_to_the_output_on_account_channels() {
        // The verbatim guarantee main gave these channels, kept under typing: a field
        // Kraken adds decodes into the `extra` catch-all and re-serializes byte-equal
        // (`round_trips` pins that), at the entry level and nested alike — an agent
        // tailing `-o json` never sees a silently narrowed record.
        let raw = r#"{"channel":"executions","type":"update","data":[{
            "order_id":"O1","exec_type":"trade","timestamp":"TS",
            "new_fee_model":{"tier":1},
            "fees":[{"asset":"USD","qty":0.34,"rebate":true}]}],"sequence":3}"#;
        let msg = round_trips(raw);
        let ChannelData::Executions(data) = msg.body else {
            panic!("expected Executions");
        };
        assert_eq!(data[0].extra["new_fee_model"]["tier"], 1);
        assert_eq!(data[0].fees.as_ref().unwrap()[0].extra["rebate"], true);
    }

    #[test]
    fn extra_top_level_field_is_tolerated() {
        let raw = r#"{"channel":"status","type":"update","new_envelope_field":1,"data":[{
            "system":"online","api_version":"v2","connection_id":1,"version":"2.0"}]}"#;
        assert!(ChannelMessage::parse(raw).is_some());
    }

    #[test]
    fn unknown_vocabulary_in_one_entry_keeps_the_whole_frame() {
        let msg = ChannelMessage::parse(
            r#"{"channel":"executions","type":"update","data":[
                {"order_id":"O1","exec_type":"settled","timestamp":"T1"},
                {"order_id":"O2","exec_type":"trade","timestamp":"T2"}]}"#,
        )
        .expect("an unknown enum token degrades one field, never the frame");
        let ChannelData::Executions(entries) = msg.body else {
            panic!("expected Executions");
        };
        assert_eq!(entries[0].exec_type, ExecType::Unknown);
        assert_eq!(
            entries[1].exec_type,
            ExecType::Trade,
            "sibling entries survive typed"
        );
    }

    #[test]
    fn flags_only_frames_carrying_unknown_vocabulary() {
        let drifted = ChannelMessage::parse(
            r#"{"channel":"executions","type":"update","data":[
                {"order_id":"O1","exec_type":"settled","timestamp":"T1"}]}"#,
        )
        .expect("frame decodes");
        assert!(drifted.body.has_unknown_vocabulary());

        let pinned = ChannelMessage::parse(
            r#"{"channel":"executions","type":"update","data":[
                {"order_id":"O1","exec_type":"trade","timestamp":"T1"}]}"#,
        )
        .expect("frame decodes");
        assert!(!pinned.body.has_unknown_vocabulary());
    }

    #[test]
    fn schema_mismatch_on_a_typed_channel_is_a_hard_error() {
        let raw = r#"{"channel":"ticker","type":"update","data":[{"symbol":"BTC/USD",
            "last":"not-a-number"}]}"#;
        assert!(raw.parse::<crate::Inbound>().is_err());
    }

    #[test]
    fn skips_non_data_frames() {
        assert!(ChannelMessage::parse(r#"{"channel":"heartbeat"}"#).is_none());
        assert!(ChannelMessage::parse(r#"{"method":"subscribe","success":true}"#).is_none());
    }

    /// [`Channel`] and [`ChannelData`]'s tag strings are two vocabularies synced only by
    /// name: a channel without a payload counterpart would compile and first fail on a
    /// live frame. Pin the pairing here instead.
    #[test]
    fn every_channel_in_the_catalogue_dispatches_to_a_payload() {
        use strum::IntoEnumIterator;
        let subscribables = SubscribableChannel::iter().map(|sub| {
            let data = match sub {
                SubscribableChannel::Instrument => r#"{"assets":[],"pairs":[]}"#,
                _ => "[]",
            };
            (sub.to_string(), data)
        });
        // `status` carries data but is not subscribable; `heartbeat` is payload-less
        // and routed before decode.
        for (channel, data) in subscribables.chain([("status".into(), "[]")]) {
            let raw = format!(r#"{{"channel":"{channel}","type":"update","data":{data}}}"#);
            assert!(
                ChannelMessage::parse(&raw).is_some(),
                "channel `{channel}` has no ChannelData counterpart"
            );
        }
    }
}
