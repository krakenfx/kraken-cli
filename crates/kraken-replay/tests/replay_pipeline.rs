//! One realistic flow through the public API only — the boundary proof for
//! the lab crate above: a fixture session explains and replays without
//! reaching past `pub` items.

use kraken_paper::OrderSide;
use kraken_paper::account::Origin;
use kraken_replay::{MAX_SPEED, PlaybackClock, explain_pnl, replay_events};
use kraken_session::timeline::{self, fixtures};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// The decomposition identity, restated through the public surface:
/// components sum exactly to the anchored total.
#[test]
fn components_sum_to_total_pnl() {
    let mut builder = fixtures::SessionBuilder::with_rates(dec!(10_000), dec!(0.0026), dec!(0.0));
    builder.ticker(
        "2026-01-01T00:00:01Z",
        "BTC/USD",
        dec!(50_000),
        dec!(50_100),
    );
    builder.market_fill(
        "2026-01-01T00:00:02Z",
        Origin::Cli,
        OrderSide::Buy,
        "BTCUSD",
        dec!(0.1),
        (dec!(50_100), dec!(50_000)),
    );
    builder.ticker(
        "2026-01-01T00:00:03Z",
        "BTC/USD",
        dec!(51_000),
        dec!(51_100),
    );

    let mut manifest = fixtures::manifest(vec![]);
    let final_value = dec!(10_087.6740); // 10_000 - cost - fee + 0.1 * end bid
    manifest.summary = Some(kraken_session::manifest::SessionOutcome {
        ended_at: "2026-01-01T00:00:04Z".parse().unwrap(),
        final_value,
        pnl: final_value - dec!(10_000),
    });
    let session = builder.session(None, manifest);

    let report = explain_pnl(&session).unwrap();
    let component_sum: Decimal = report
        .components
        .iter()
        .filter_map(|line| line.amount)
        .sum();
    assert_eq!(component_sum, report.anchor.total_pnl);
}

/// NDJSON out equals the timeline in: every emitted line parses back to the
/// event that produced it, in order.
#[tokio::test(start_paused = true)]
async fn replays_a_fixture_session_end_to_end_in_order() {
    let base = tempfile::tempdir().unwrap();
    let name = fixtures::provision(base.path(), vec![fixtures::jsonl_recording()]);
    fixtures::write_market(
        base.path(),
        &name,
        &["2026-01-01T00:00:01.000000Z", "2026-01-01T00:00:03.000000Z"],
    );
    fixtures::write_account(
        base.path(),
        &name,
        &[fixtures::account_record("2026-01-01T00:00:02+00:00")],
    );
    fixtures::write_decision(base.path(), &name, "2026-01-01T00:00:03+00:00");

    let session = timeline::read(base.path(), &name).unwrap();
    let clock = PlaybackClock::anchored(session.events[0].at, MAX_SPEED);
    let mut out = Vec::new();
    let mut emitted = 0;
    replay_events(
        &session.events,
        clock,
        &mut out,
        serde_json::to_string,
        &mut emitted,
    )
    .await
    .unwrap();

    let lines: Vec<serde_json::Value> = std::str::from_utf8(&out)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), session.events.len());
    assert_eq!(emitted, session.events.len() as u64);
    for (line, event) in lines.iter().zip(&session.events) {
        assert_eq!(line, &serde_json::to_value(event).unwrap());
    }
}

/// Recorded gaps are reproduced at speed 1 — proven under paused time from
/// outside the crate.
#[tokio::test(start_paused = true)]
async fn pacing_reproduces_recorded_gaps_at_speed_1() {
    let base = tempfile::tempdir().unwrap();
    let name = fixtures::provision(base.path(), vec![fixtures::jsonl_recording()]);
    fixtures::write_market(
        base.path(),
        &name,
        &["2026-01-01T00:00:00.000000Z", "2026-01-01T00:00:02.000000Z"],
    );

    let session = timeline::read(base.path(), &name).unwrap();
    let clock = PlaybackClock::anchored(session.events[0].at, 1.0);
    let started = tokio::time::Instant::now();
    let mut out = Vec::new();
    let mut emitted = 0;
    replay_events(
        &session.events,
        clock,
        &mut out,
        serde_json::to_string,
        &mut emitted,
    )
    .await
    .unwrap();

    assert_eq!(emitted, 2);
    assert_eq!(
        tokio::time::Instant::now() - started,
        std::time::Duration::from_secs(2),
        "the recorded 2 s gap is reproduced on the virtual clock"
    );
}
