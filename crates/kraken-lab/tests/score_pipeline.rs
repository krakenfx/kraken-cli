//! The scorecard driven through the public API only: a stopped fixture
//! session scores, and its waterfall is byte-equal to `explain_pnl`'s —
//! the can't-disagree contract proven from outside the crate.

use kraken_lab::score;
use kraken_paper::OrderSide;
use kraken_paper::account::Origin;
use kraken_replay::explain_pnl;
use kraken_session::manifest::SessionOutcome;
use kraken_session::timeline::SessionTimeline;
use kraken_session::timeline::fixtures::{self, SessionBuilder};
use rust_decimal_macros::dec;

// Fixture misuse is a test bug; the parse target is a literal.
#[allow(clippy::unwrap_used)]
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

#[test]
fn scorecard_components_are_byte_equal_to_explain_pnl() {
    let session = stopped_session();
    let card = score(&session).unwrap();
    let report = explain_pnl(&session).unwrap();

    assert_eq!(
        serde_json::to_vec(&card.components).unwrap(),
        serde_json::to_vec(&report.components).unwrap(),
        "the waterfall crosses the lab→replay edge verbatim"
    );
    assert_eq!(
        serde_json::to_vec(&card.anchor).unwrap(),
        serde_json::to_vec(&report.anchor).unwrap()
    );
}

#[test]
fn scorecard_metrics_derive_from_the_anchor_and_curve() {
    let card = score(&stopped_session()).unwrap();
    assert_eq!(card.metrics.return_pct, Some(dec!(1.87)));
    assert_eq!(card.trades.fills, 1);
    // Two ticker observations: the mark table's warm-up averages 50k and
    // 52k to a 51k mark — 4_987 cash + 0.1 × 51_000.
    assert_eq!(card.curve.last_value, Some(dec!(10_087)));
}
