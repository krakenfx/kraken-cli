//! Event vocabulary for merged session timelines.
//!
//! Payloads reuse persisted models and serialize flat, keeping merged output
//! sink-compatible without a parallel read model.

use crate::decision::Decision;
use chrono::{DateTime, Utc};
use kraken_core::ChannelMessage;
use kraken_paper::{AccountEvent, account::AccountRecord};
use kraken_recording::MarketEvent;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TimelineEvent {
    pub at: DateTime<Utc>,
    /// Within-track order: the DuckDB frame `seq`, or the track's line number.
    pub seq: i64,
    #[serde(flatten)]
    pub payload: EventPayload,
}

/// A persisted payload; boxing keeps the common market variant compact.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum EventPayload {
    /// A market frame (`market.duckdb` / `market.jsonl`).
    Market(ChannelMessage),
    /// A paper-account journal record (`events.jsonl`).
    Account(Box<AccountRecord>),
    /// A strategy/operator decision (`decisions.jsonl`).
    Decision(Decision),
}

/// Load-bearing tie-break order: market context, account effect, then
/// rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Track {
    Market,
    Account,
    Decision,
}

impl TimelineEvent {
    pub fn account(at: DateTime<Utc>, seq: i64, record: AccountRecord) -> Self {
        Self {
            at,
            seq,
            payload: EventPayload::Account(Box::new(record)),
        }
    }

    pub fn decision(at: DateTime<Utc>, seq: i64, decision: Decision) -> Self {
        Self {
            at,
            seq,
            payload: EventPayload::Decision(decision),
        }
    }

    pub fn track(&self) -> Track {
        match self.payload {
            EventPayload::Market(_) => Track::Market,
            EventPayload::Account(_) => Track::Account,
            EventPayload::Decision(_) => Track::Decision,
        }
    }

    /// Orders by instant and track before comparing unrelated per-track
    /// sequences.
    pub fn key(&self) -> (DateTime<Utc>, Track, i64) {
        (self.at, self.track(), self.seq)
    }

    /// The human render's line label: a market frame shows its channel
    /// (`trade`/`ticker`/…), the other tracks their track name.
    pub fn track_label(&self) -> String {
        match &self.payload {
            EventPayload::Market(frame) => frame.channel().to_string(),
            EventPayload::Account(_) => "account".to_string(),
            EventPayload::Decision(_) => "decision".to_string(),
        }
    }
}

/// Where the journal's final account epoch begins — the last
/// `Initialized`/`Reset` on the account track — and how many earlier epoch
/// starts precede it. `start: None` when the journal has no epoch at all.
pub struct EpochStart {
    pub start: Option<usize>,
    pub skipped: usize,
}

/// Finds the shared epoch boundary; order IDs restart at every boundary.
pub fn final_epoch_start(events: &[TimelineEvent]) -> EpochStart {
    let starts: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| {
            matches!(
                &event.payload,
                EventPayload::Account(record) if matches!(
                    record.event,
                    AccountEvent::Initialized(_)
                        | AccountEvent::Reset(_)
                        | AccountEvent::Attached(_)
                )
            )
        })
        .map(|(index, _)| index)
        .collect();
    EpochStart {
        start: starts.last().copied(),
        skipped: starts.len().saturating_sub(1),
    }
}

/// A market source item becomes a timeline event verbatim: the source already
/// clamped the instant and owns the `seq`.
impl From<MarketEvent> for TimelineEvent {
    fn from(event: MarketEvent) -> Self {
        Self {
            at: event.at,
            seq: event.seq,
            payload: EventPayload::Market(event.frame),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::DecisionKind;
    use kraken_paper::{
        PaperConfig,
        account::{Origin, RECORD_VERSION},
    };
    use kraken_recording::parse_instant;
    use rust_decimal_macros::dec;

    fn decision_at(ts: &str) -> TimelineEvent {
        TimelineEvent::decision(
            parse_instant(ts).unwrap(),
            0,
            Decision {
                timestamp: ts.parse().unwrap(),
                kind: DecisionKind::Skip,
                symbol: None,
                reason: "test".to_string(),
                order_id: None,
            },
        )
    }

    fn account_at(ts: &str) -> TimelineEvent {
        TimelineEvent::account(
            parse_instant(ts).unwrap(),
            0,
            AccountRecord {
                v: RECORD_VERSION,
                ts: ts.parse().unwrap(),
                origin: Origin::Cli,
                event: AccountEvent::Initialized(PaperConfig {
                    balance: dec!(10_000.0),
                    currency: "USD".to_string(),
                    fee_rate: dec!(0.0026),
                    slippage_rate: dec!(0.0),
                }),
            },
        )
    }

    fn market_at(ts: &str, seq: i64) -> TimelineEvent {
        let frame = ChannelMessage::parse(
            r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
               "price":1.0,"qty":1.0,"ord_type":"market","trade_id":1,"timestamp":"TS"}]}"#,
        )
        .unwrap();
        TimelineEvent::from(MarketEvent {
            at: parse_instant(ts).unwrap(),
            seq,
            frame,
        })
    }

    #[test]
    fn equal_instant_ties_break_market_account_decision() {
        // Market gets the highest seq: a pass proves track priority outranks seq.
        let ts = "2026-01-01T00:00:00Z";
        let market = market_at(ts, 9);
        let account = account_at(ts);
        let decision = decision_at(ts);
        assert!(market.key() < account.key(), "market precedes account");
        assert!(account.key() < decision.key(), "account precedes decision");
    }

    #[test]
    fn serializes_with_the_payload_flattened_to_its_wire_shape() {
        // Pins the NDJSON contract `kraken replay` emits verbatim.
        let event = market_at("2026-01-01T00:00:00Z", 7);
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["seq"], 7);
        assert_eq!(json["channel"], "trade");
        assert_eq!(json["data"][0]["price"], 1.0);
        assert!(json.get("payload").is_none(), "no discriminant wrapper");
    }

    #[test]
    fn track_labels_show_the_channel_for_market_and_the_track_otherwise() {
        let ts = "2026-01-01T00:00:00Z";
        assert_eq!(market_at(ts, 1).track_label(), "trade");
        assert_eq!(account_at(ts).track_label(), "account");
        assert_eq!(decision_at(ts).track_label(), "decision");
    }

    #[test]
    fn account_and_decision_payloads_serialize_flat_too() {
        // Pins the module's deepest serde stack: outer flatten → untagged enum
        // → Box → AccountRecord's own inner flatten.
        let account = serde_json::to_value(account_at("2026-01-01T00:00:00Z")).unwrap();
        assert_eq!(account["event"], "initialized");
        assert_eq!(account["origin"], "cli");
        // The journal's digit-exact string wire must survive the flatten
        // stack's `Content` buffering, digits included.
        assert_eq!(account["balance"], "10000.0");
        assert!(account.get("payload").is_none(), "no discriminant wrapper");

        let decision = serde_json::to_value(decision_at("2026-01-01T00:00:00Z")).unwrap();
        assert_eq!(decision["kind"], "skip");
        assert_eq!(decision["reason"], "test");
        assert!(decision.get("payload").is_none(), "no discriminant wrapper");
    }
}
