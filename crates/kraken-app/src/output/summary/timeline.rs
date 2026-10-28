//! `-o table` lines for the merged session-timeline payloads (`kraken replay`).

use super::{Summarize, SummaryLine};
use crate::session::{decision::Decision, timeline::event::EventPayload};
use kraken_paper::{AccountEvent, PaperConfig, account::AccountRecord};

impl Summarize for EventPayload {
    fn summary(&self) -> String {
        match self {
            Self::Market(frame) => frame.body.summary(),
            Self::Account(record) => account_summary(record),
            Self::Decision(decision) => decision_summary(decision),
        }
    }
}

fn account_summary(record: &AccountRecord) -> String {
    match &record.event {
        AccountEvent::Initialized(config) => epoch_summary("initialized", config),
        AccountEvent::Reset(config) => epoch_summary("reset", config),
        AccountEvent::OrderSubmitted { order } => SummaryLine::default()
            .field("event", "order_submitted")
            .field("order", &order.id)
            .field("side", order.side)
            .field("pair", &order.pair)
            .field("volume", order.volume)
            .field("price", order.price)
            .build(),
        AccountEvent::OrderFilled { trade } => SummaryLine::default()
            .field("event", "order_filled")
            .field("trade", &trade.id)
            .field("order", &trade.order_id)
            .field("side", trade.side)
            .field("pair", &trade.pair)
            .field("volume", trade.volume)
            .field("price", trade.price)
            .field("fee", trade.fee)
            .build(),
        AccountEvent::OrderCancelled { order } => SummaryLine::default()
            .field("event", "order_cancelled")
            .field("order", &order.id)
            .field("side", order.side)
            .field("pair", &order.pair)
            .field("volume", order.volume)
            .field("price", order.price)
            .build(),
        AccountEvent::Command(entry) => SummaryLine::default()
            .field("event", "command")
            .field("name", &entry.name)
            .build(),
        AccountEvent::OrderRejected(rejection) => SummaryLine::default()
            .field("event", "order_rejected")
            .field("side", rejection.side)
            .field("pair", &rejection.pair)
            .field("volume", rejection.volume)
            .field("category", &rejection.category)
            .build(),
        // `Unknown` — and, `AccountEvent` being `#[non_exhaustive]`, whatever
        // a newer build adds — summarizes as "unknown", never dropped.
        _ => SummaryLine::default().field("event", "unknown").build(),
    }
}

fn epoch_summary(event: &str, config: &PaperConfig) -> String {
    SummaryLine::default()
        .field("event", event)
        .field("balance", config.balance)
        .field("currency", &config.currency)
        .build()
}

fn decision_summary(decision: &Decision) -> String {
    SummaryLine::default()
        .field("kind", decision.kind)
        .opt("symbol", decision.symbol.as_deref())
        .field("reason", &decision.reason)
        .opt("order", decision.order_id.as_deref())
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::decision::DecisionKind;
    use kraken_core::ChannelMessage;
    use kraken_paper::account::{Origin, RECORD_VERSION};
    use rust_decimal_macros::dec;

    #[test]
    fn market_payload_summary_delegates_to_the_frame() {
        let frame = ChannelMessage::parse(
            r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"buy",
               "price":1.0,"qty":1.0,"ord_type":"market","trade_id":1,
               "timestamp":"2026-01-01T00:00:00Z"}]}"#,
        )
        .unwrap();
        let expected = frame.body.summary();
        assert_eq!(EventPayload::Market(frame).summary(), expected);
    }

    #[test]
    fn account_payload_summary_names_the_event_and_its_identifiers() {
        // The riskiest human line: a fill must surface its ids, pair, and fee
        // without the reader opening the JSON render.
        let mut state = kraken_paper::PaperState::default();
        state.apply(
            "2026-01-01T00:00:00Z".parse().unwrap(),
            &AccountEvent::Initialized(PaperConfig {
                balance: dec!(10_000.0),
                currency: "USD".to_string(),
                fee_rate: dec!(0.0026),
                slippage_rate: dec!(0.0),
            }),
        );
        let trade = state
            .decide_market_order(
                kraken_core::OrderSide::Buy,
                "BTCUSD",
                dec!(0.1),
                dec!(50_000.0),
                dec!(49_900.0),
            )
            .unwrap();
        let record = AccountRecord {
            v: RECORD_VERSION,
            ts: "2026-01-01T00:00:01Z".parse().unwrap(),
            origin: Origin::Cli,
            event: AccountEvent::OrderFilled { trade },
        };
        let summary = EventPayload::Account(Box::new(record)).summary();
        for needle in [
            "event:order_filled",
            "trade:PAPER-00002",
            "order:PAPER-00001",
            "pair:BTCUSD",
        ] {
            assert!(summary.contains(needle), "{needle} missing in {summary}");
        }
    }

    #[test]
    fn decision_payload_summary_carries_kind_reason_and_optional_refs() {
        let bare = EventPayload::Decision(Decision {
            timestamp: "2026-01-01T00:00:00Z".parse().unwrap(),
            kind: DecisionKind::Skip,
            symbol: None,
            reason: "test".to_string(),
            order_id: None,
        });
        assert_eq!(bare.summary(), "kind:skip reason:test");

        let full = EventPayload::Decision(Decision {
            timestamp: "2026-01-01T00:00:00Z".parse().unwrap(),
            kind: DecisionKind::Buy,
            symbol: Some("BTCUSD".to_string()),
            reason: "dip".to_string(),
            order_id: Some("PAPER-00001".to_string()),
        });
        assert_eq!(
            full.summary(),
            "kind:buy symbol:BTCUSD reason:dip order:PAPER-00001"
        );
    }
}
