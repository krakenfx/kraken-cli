//! The curve fold driven through the public API only — the boundary proof
//! for the scorecard and the binary's lab command above.

use kraken_lab::{LabError, replay};
use kraken_session::timeline::fixtures::{self, SessionBuilder};
use rust_decimal_macros::dec;

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
