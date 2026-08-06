//! Snapshot pins for the crate's serialized command contracts. The shapes of
//! `Scorecard`, `Replay`, and `Verdict` are what agents (and the future SDK)
//! parse; a field rename, reorder, or added key is a contract change, so it
//! must land as a reviewable `.snap` diff rather than slip through unseen.
//!
//! Every fixture uses literal timestamps and fixed rates, so the serialized
//! output is deterministic — no redactions needed. Regenerate intentionally
//! with `INSTA_UPDATE=always cargo test -p kraken-lab --test snapshots` (or
//! `cargo insta review`) and commit the updated snapshots.

// Fixtures build on known-good literals; a failed unwrap here is a test bug,
// so the workspace's `unwrap_used` deny is lifted for this test file (matching
// the sibling `*_pipeline` tests).
#![allow(clippy::unwrap_used)]

use insta::assert_json_snapshot;
use kraken_lab::{Criteria, Replay, Scorecard, replay, score};
use kraken_paper::OrderSide;
use kraken_paper::account::Origin;
use kraken_session::manifest::{SessionOutcome, SessionSource};
use kraken_session::timeline::SessionTimeline;
use kraken_session::timeline::fixtures::{self, SessionBuilder};
use rust_decimal_macros::dec;

/// A stopped one-fill session: buy 0.1 BTC at 50k mid (0.26% fee), mark to 52k.
fn stopped_session() -> SessionTimeline {
    let mut builder = SessionBuilder::with_rates(dec!(10_000), dec!(0.0026), dec!(0.0));
    builder.ticker(
        "2026-01-01T00:00:01Z",
        "BTC/USD",
        dec!(50_000),
        dec!(50_000),
    );
    builder.market_fill(
        "2026-01-01T00:00:02Z",
        Origin::Cli,
        OrderSide::Buy,
        "BTCUSD",
        dec!(0.1),
        (dec!(50_000), dec!(50_000)),
    );
    builder.ticker(
        "2026-01-01T00:00:03Z",
        "BTC/USD",
        dec!(52_000),
        dec!(52_000),
    );
    let mut manifest = fixtures::manifest(vec![]);
    manifest.summary = Some(SessionOutcome {
        ended_at: "2026-01-01T00:00:04Z".parse().unwrap(),
        final_value: dec!(10_187),
        pnl: dec!(187),
    });
    builder.session(None, manifest)
}

/// A closed winning round trip (buy then sell), zero rates — exercises the
/// hit-rate and profit-factor metrics a fill-less scorecard leaves absent.
fn round_trip_session() -> SessionTimeline {
    let mut builder = SessionBuilder::with_rates(dec!(10_000), dec!(0.0), dec!(0.0));
    builder.ticker(
        "2026-01-01T00:00:01Z",
        "BTC/USD",
        dec!(50_000),
        dec!(50_000),
    );
    builder.market_fill(
        "2026-01-01T00:00:02Z",
        Origin::Cli,
        OrderSide::Buy,
        "BTCUSD",
        dec!(0.1),
        (dec!(50_000), dec!(50_000)),
    );
    builder.ticker(
        "2026-01-01T00:00:03Z",
        "BTC/USD",
        dec!(52_000),
        dec!(52_000),
    );
    builder.market_fill(
        "2026-01-01T00:00:04Z",
        Origin::Cli,
        OrderSide::Sell,
        "BTCUSD",
        dec!(0.1),
        (dec!(52_000), dec!(52_000)),
    );
    let mut manifest = fixtures::manifest(vec![]);
    manifest.summary = Some(SessionOutcome {
        ended_at: "2026-01-01T00:00:05Z".parse().unwrap(),
        final_value: dec!(10_200),
        pnl: dec!(200),
    });
    builder.session(None, manifest)
}

/// The worked example replayed against a recorded tape — pins the full
/// honesty envelope (replay + reaction-time caveats, replay `source` block).
fn replay_provenance_session() -> SessionTimeline {
    let mut session = stopped_session();
    session.manifest.source = Some(SessionSource {
        tape: "tape:jun-crash".to_string(),
        speed: 10.0,
        anchor: "2026-06-01T00:00:00Z".parse().unwrap(),
        started_at: "2026-07-01T00:00:00Z".parse().unwrap(),
        content_hash: None,
    });
    session
}

fn scorecard() -> Scorecard {
    score(&stopped_session()).unwrap()
}

#[test]
fn scorecard_of_the_worked_example() {
    assert_json_snapshot!(scorecard());
}

#[test]
fn scorecard_of_a_closed_round_trip() {
    assert_json_snapshot!(score(&round_trip_session()).unwrap());
}

#[test]
fn scorecard_of_a_replayed_session_carries_the_honesty_envelope() {
    assert_json_snapshot!(score(&replay_provenance_session()).unwrap());
}

#[test]
fn replay_curve_and_ledger() {
    let folded: Replay = replay(&stopped_session()).unwrap();
    assert_json_snapshot!(folded);
}

#[test]
fn verdict_passes_and_fails_against_the_same_card() {
    let card = scorecard();
    let pass = Criteria {
        min_return_pct: Some(dec!(1.0)),
        max_drawdown_pct: None,
        min_fills: Some(1),
    }
    .evaluate(&card);
    let fail = Criteria {
        min_return_pct: Some(dec!(100.0)),
        max_drawdown_pct: None,
        min_fills: Some(5),
    }
    .evaluate(&card);
    assert_json_snapshot!("verdict_pass", pass);
    assert_json_snapshot!("verdict_fail", fail);
}
