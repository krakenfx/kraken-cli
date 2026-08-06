//! The curve fold driven through the public API only — the boundary proof
//! for the scorecard and the binary's lab command above.

use kraken_lab::{LabError, replay};
use kraken_paper::OrderSide;
use kraken_paper::account::Origin;
use kraken_session::timeline::fixtures::{self, SessionBuilder};
use rust_decimal_macros::dec;

fn marked_session() -> kraken_session::timeline::SessionTimeline {
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
    builder.session(None, fixtures::manifest(vec![]))
}

/// Fold determinism restated from outside the crate: the same session folds
/// to a byte-identical curve and ledger, twice.
#[test]
fn folding_twice_is_byte_identical() {
    let session = marked_session();
    let first = serde_json::to_vec(&replay(&session).unwrap()).unwrap();
    let second = serde_json::to_vec(&replay(&session).unwrap()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn curve_and_ledger_reflect_the_fold() {
    let session = marked_session();
    let folded = replay(&session).unwrap();

    assert_eq!(folded.fills.len(), 1, "one fill on the ledger");
    assert_eq!(folded.fills[0].volume, dec!(0.1));
    assert_eq!(folded.curve.currency, "USD");
    assert!(
        folded.curve.points.iter().all(|p| p.complete),
        "every holding marked"
    );
    // The mark move from 50k to 52k re-values the held 0.1 BTC upward.
    let last = folded.curve.points.last().unwrap();
    let first = &folded.curve.points[0];
    assert!(
        last.value > first.value - dec!(200),
        "curve tracks the mark"
    );
}

/// A journal with no epoch refuses the fold — visible to consumers as the
/// typed `NoEpoch`, never a fabricated equity line.
#[test]
fn epochless_journal_is_refused_as_no_epoch() {
    let mut builder = SessionBuilder::with_rates(dec!(10_000), dec!(0.0026), dec!(0.0));
    builder.ticker(
        "2026-01-01T00:00:01Z",
        "BTC/USD",
        dec!(50_000),
        dec!(50_000),
    );
    let mut session = builder.session(None, fixtures::manifest(vec![]));
    // Strip the account track: only market frames remain.
    session.events.retain(|event| {
        matches!(
            event.payload,
            kraken_session::timeline::event::EventPayload::Market(_)
        )
    });

    let err = replay(&session).unwrap_err();
    assert!(matches!(err, LabError::NoEpoch { .. }), "got: {err}");
}
